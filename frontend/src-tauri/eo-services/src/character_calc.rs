//! Character calculation service, ported from
//! `backend/services/character_calc.py`: profession levels, skill
//! ranks, HP, codex prediction. Pure functions; no I/O. Inputs arrive
//! in the catalogue's nested JSON shape, and two adapter helpers hide
//! that shape from the maths exactly as the backend's iterators do.

use serde_json::{Map, Value};

use crate::codex_categories::{get_codex_category, reward_divisor};
use crate::tt_value_curve::{levels_for_tt_value, max_tt_curve_level, tt_value_at};
use eo_wire::normalizer::round_half_even;

/// Attribute skills receive a x20 multiplier in profession calculations.
const ATTRIBUTE_SKILLS: [&str; 6] = [
    "Agility",
    "Health",
    "Intelligence",
    "Psyche",
    "Stamina",
    "Strength",
];

fn is_attribute(skill_name: &str) -> bool {
    ATTRIBUTE_SKILLS.contains(&skill_name)
}

/// Apply the x20 multiplier for attribute skills.
pub fn effective_points(skill_name: &str, level: f64) -> f64 {
    if is_attribute(skill_name) {
        level * 20.0
    } else {
        level
    }
}

fn level_of(skill_levels: &Map<String, Value>, name: &str) -> f64 {
    skill_levels
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// `float(value or 0)` over the JSON shapes a weight can carry: falsy
/// values coalesce to 0, numbers pass through, numeric strings parse,
/// and unconvertible shapes fall to the error value.
fn python_float_or(value: Option<&Value>, on_error: Option<f64>) -> Option<f64> {
    let Some(value) = value else {
        return Some(0.0);
    };
    match value {
        Value::Null => Some(0.0),
        Value::Bool(false) => Some(0.0),
        Value::Bool(true) => Some(1.0),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Some(0.0);
            }
            trimmed.parse::<f64>().ok().or(on_error)
        }
        Value::Array(a) if a.is_empty() => Some(0.0),
        Value::Object(o) if o.is_empty() => Some(0.0),
        _ => on_error,
    }
}

/// Bare `float(value)`: no falsy coalescing, so empty strings and
/// containers fail (the caller skips), exactly as the backend's
/// try/except around a plain conversion does.
pub(crate) fn python_float_bare(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) => Some(0.0),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `(skill_name, weight)` for each usable skill entry on a profession;
/// entries with no usable name are skipped and a missing weight
/// surfaces as 0, exactly as the backend's iterator documents.
fn iter_profession_skills(profession: &Value) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let entries = profession
        .get("skills")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let name = entry
            .get("skill")
            .and_then(|skill| skill.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let weight = python_float_or(entry.get("weight"), Some(0.0)).unwrap_or(0.0);
        out.push((name.to_string(), weight));
    }
    out
}

/// `(skill_name, hp_increase)` for skills that contribute to HP;
/// unconvertible or non-positive contributions skip the entry.
fn iter_hp_skills(skills_data: &[Value]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for skill in skills_data {
        let Some(skill) = skill.as_object() else {
            continue;
        };
        let Some(hp_inc) = python_float_or(skill.get("hp_increase"), None) else {
            continue;
        };
        if hp_inc <= 0.0 {
            continue;
        }
        let name = skill.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), hp_inc));
    }
    out
}

/// The un-rounded numerator behind profession level.
fn raw_profession_total(skill_levels: &Map<String, Value>, profession: &Value) -> f64 {
    let mut total = 0.0;
    for (name, weight) in iter_profession_skills(profession) {
        let level = level_of(skill_levels, &name);
        total += effective_points(&name, level) * weight;
    }
    total
}

/// `round(sum(effective_points * weight) / 10000, 2)`.
pub fn profession_level(skill_levels: &Map<String, Value>, profession: &Value) -> f64 {
    round_half_even(raw_profession_total(skill_levels, profession) / 10000.0, 2)
}

/// `profession_level` over every named entity, `{name: level}` in
/// entity order (zero levels included).
pub fn all_profession_levels(
    skill_levels: &Map<String, Value>,
    professions: &[Value],
) -> Map<String, Value> {
    let mut result = Map::new();
    for prof in professions {
        let Some(name) = prof
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        result.insert(
            name.to_string(),
            Value::from(profession_level(skill_levels, prof)),
        );
    }
    result
}

/// The rank name for a skill level: a right-bisect over the valid
/// thresholds, clamped to the list.
pub fn skill_rank(level: f64, ranks: &[Value]) -> String {
    if ranks.is_empty() {
        return "Unknown".to_string();
    }
    let mut valid: Vec<(f64, &str)> = Vec::new();
    for rank in ranks {
        let Some(name) = rank.get("name").and_then(Value::as_str) else {
            continue;
        };
        let threshold = match rank.get("skill") {
            None | Some(Value::Null) => continue,
            Some(value) => match python_float_bare(value) {
                Some(t) => t,
                None => continue,
            },
        };
        valid.push((threshold, name));
    }
    if valid.is_empty() {
        return "Unknown".to_string();
    }
    let i = valid.partition_point(|(threshold, _)| *threshold <= level);
    let i = i.saturating_sub(1).min(valid.len() - 1);
    valid[i].1.to_string()
}

fn codex_fields(name: &str) -> (Value, Value) {
    match get_codex_category(name) {
        Some(category) => (
            Value::from(category),
            reward_divisor(category)
                .map(Value::from)
                .unwrap_or(Value::Null),
        ),
        None => (Value::Null, Value::Null),
    }
}

fn sort_stable_by_f64(rows: &mut [Value], key: &str, descending: bool) {
    rows.sort_by(|a, b| {
        let av = a[key].as_f64().unwrap_or(0.0);
        let bv = b[key].as_f64().unwrap_or(0.0);
        let ordering = av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal);
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn sort_stable_by_name(rows: &mut [Value]) {
    rows.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
}

/// Analyse skills for levelling a profession: regular skills ranked by
/// PED cost to reach the next integer level via that skill alone, and
/// attribute skills ranked by raw contribution factor.
pub fn profession_skill_optimizer(skill_levels: &Map<String, Value>, profession: &Value) -> Value {
    let current_prof = raw_profession_total(skill_levels, profession) / 10000.0;
    let next_level = current_prof.trunc() as i64 + 1;
    let gap = next_level as f64 - current_prof;

    let mut skills: Vec<Value> = Vec::new();
    let mut attributes: Vec<Value> = Vec::new();

    for (name, weight) in iter_profession_skills(profession) {
        if weight <= 0.0 {
            continue;
        }
        let current_level = level_of(skill_levels, &name);

        if is_attribute(&name) {
            attributes.push(serde_json::json!({
                "name": name,
                "weight": weight,
                "currentLevel": current_level,
                "contributionFactor": weight * 20.0,
            }));
        } else {
            let levels_needed = gap * 10000.0 / weight;
            let target_level = current_level + levels_needed;
            let ped_cost = tt_value_at(target_level) - tt_value_at(current_level);
            let (codex_category, codex_divisor) = codex_fields(&name);
            skills.push(serde_json::json!({
                "name": name,
                "weight": weight,
                "currentLevel": current_level,
                "levelsNeeded": round_half_even(levels_needed, 1),
                "pedToNextLevel": round_half_even(ped_cost, 2),
                "codexCategory": codex_category,
                "codexDivisor": codex_divisor,
            }));
        }
    }

    sort_stable_by_f64(&mut skills, "pedToNextLevel", false);
    sort_stable_by_f64(&mut attributes, "contributionFactor", true);

    serde_json::json!({
        "skills": skills,
        "attributes": attributes,
        "currentLevel": round_half_even(current_prof, 2),
        "nextLevel": next_level,
        "gap": round_half_even(gap, 4),
    })
}

struct PathSkill {
    name: String,
    weight: f64,
    current_level: f64,
    allocated: f64,
    ped: f64,
}

/// Cheapest skill allocation to reach a target profession level, or
/// the best allocation for a PED budget, by greedy marginal-cost
/// steps (optimal because the TT curve is convex). Exactly one of
/// `target_level` / `ped_budget` must be provided.
pub fn profession_path_optimizer(
    skill_levels: &Map<String, Value>,
    profession: &Value,
    target_level: Option<f64>,
    ped_budget: Option<f64>,
) -> Result<Value, String> {
    if target_level.is_none() == ped_budget.is_none() {
        return Err("Exactly one of target_level or ped_budget must be provided".to_string());
    }

    let current_prof = raw_profession_total(skill_levels, profession) / 10000.0;

    let mut skills: Vec<PathSkill> = Vec::new();
    let mut excluded: Vec<Value> = Vec::new();
    let mut attributes: Vec<Value> = Vec::new();
    for (name, weight) in iter_profession_skills(profession) {
        if weight <= 0.0 {
            continue;
        }
        if is_attribute(&name) {
            let current_level = level_of(skill_levels, &name);
            attributes.push(serde_json::json!({
                "name": name,
                "weight": weight,
                "currentLevel": current_level,
                "contributionFactor": weight * 20.0,
            }));
        } else if !skill_levels.contains_key(&name) {
            excluded.push(serde_json::json!({
                "name": name,
                "weight": weight,
                "reason": "not unlocked",
            }));
        } else {
            skills.push(PathSkill {
                current_level: level_of(skill_levels, &name),
                name,
                weight,
                allocated: 0.0,
                ped: 0.0,
            });
        }
    }
    sort_stable_by_f64(&mut attributes, "contributionFactor", true);
    sort_stable_by_name(&mut excluded);

    let mode = if target_level.is_some() {
        "target"
    } else {
        "budget"
    };
    let max_skill_level = max_tt_curve_level() as f64;

    if let Some(target) = target_level {
        if target <= current_prof {
            return Ok(path_result(
                mode,
                target_level,
                ped_budget,
                current_prof,
                current_prof,
                &skills,
                attributes,
                excluded,
            ));
        }
        let mut points_remaining = (target - current_prof) * 10000.0;

        while points_remaining > 0.0 {
            let Some((best_idx, _)) = best_marginal(&skills, max_skill_level) else {
                break;
            };
            let s = &mut skills[best_idx];
            if points_remaining < s.weight {
                let frac_levels = points_remaining / s.weight;
                let pos = s.current_level + s.allocated;
                let frac_ped = tt_value_at(pos + frac_levels) - tt_value_at(pos);
                s.allocated += frac_levels;
                s.ped += frac_ped;
                points_remaining = 0.0;
            } else {
                let pos = s.current_level + s.allocated;
                let step_ped = tt_value_at(pos + 1.0) - tt_value_at(pos);
                s.allocated += 1.0;
                s.ped += step_ped;
                points_remaining -= s.weight;
            }
        }
    } else {
        let mut budget_remaining = ped_budget.expect("validated by the entry check");

        while budget_remaining > 1e-6 {
            let Some((best_idx, best_ped)) = best_marginal(&skills, max_skill_level) else {
                break;
            };
            let s = &mut skills[best_idx];
            if best_ped > budget_remaining {
                let pos = s.current_level + s.allocated;
                let frac_levels = levels_for_tt_value(pos, budget_remaining);
                if frac_levels <= 0.0 {
                    break;
                }
                s.allocated += frac_levels;
                s.ped += budget_remaining;
                budget_remaining = 0.0;
            } else {
                s.allocated += 1.0;
                s.ped += best_ped;
                budget_remaining -= best_ped;
            }
        }
    }

    let mut end_prof = 0.0;
    for s in &skills {
        end_prof += (s.current_level + s.allocated) * s.weight;
    }
    for a in &attributes {
        let name = a["name"].as_str().unwrap_or("");
        let current_level = a["currentLevel"].as_f64().unwrap_or(0.0);
        end_prof += effective_points(name, current_level) * a["weight"].as_f64().unwrap_or(0.0);
    }
    end_prof /= 10000.0;

    Ok(path_result(
        mode,
        target_level,
        ped_budget,
        current_prof,
        end_prof,
        &skills,
        attributes,
        excluded,
    ))
}

/// The greedy step: the first skill (in order) with the strictly best
/// marginal-PED-per-weight ratio, skipping skills at the curve ceiling.
fn best_marginal(skills: &[PathSkill], max_skill_level: f64) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    let mut best_ratio = f64::INFINITY;
    for (i, s) in skills.iter().enumerate() {
        let pos = s.current_level + s.allocated;
        if pos >= max_skill_level {
            continue;
        }
        let marginal_ped = tt_value_at(pos + 1.0) - tt_value_at(pos);
        let ratio = marginal_ped / s.weight;
        if ratio < best_ratio {
            best_ratio = ratio;
            best = Some((i, marginal_ped));
        }
    }
    best
}

/// The path optimizer's return shape: allocated skills first (by PED
/// cost descending), then unallocated alphabetically, with the input
/// echoes and rounded aggregates.
#[allow(clippy::too_many_arguments)]
fn path_result(
    mode: &str,
    target_level: Option<f64>,
    ped_budget: Option<f64>,
    current_prof: f64,
    end_prof: f64,
    skills: &[PathSkill],
    attributes: Vec<Value>,
    excluded: Vec<Value>,
) -> Value {
    let mut allocations: Vec<Value> = Vec::new();
    for s in skills {
        let (codex_category, codex_divisor) = codex_fields(&s.name);
        allocations.push(serde_json::json!({
            "name": s.name,
            "weight": s.weight,
            "currentLevel": s.current_level,
            "levelsToGain": round_half_even(s.allocated, 2),
            "pedCost": round_half_even(s.ped, 2),
            "newLevel": round_half_even(s.current_level + s.allocated, 2),
            "codexCategory": codex_category,
            "codexDivisor": codex_divisor,
        }));
    }

    // Filtered on the ROUNDED gain, exactly as the backend filters its
    // already-rounded dicts.
    let (mut allocated, mut unallocated): (Vec<Value>, Vec<Value>) = allocations
        .into_iter()
        .partition(|a| a["levelsToGain"].as_f64().unwrap_or(0.0) > 0.0);
    sort_stable_by_f64(&mut allocated, "pedCost", true);
    sort_stable_by_name(&mut unallocated);
    allocated.extend(unallocated);

    let total_ped = round_half_even(skills.iter().map(|s| s.ped).sum::<f64>(), 2);

    serde_json::json!({
        "mode": mode,
        "inputTargetLevel": target_level,
        "inputPedBudget": ped_budget,
        "currentLevel": round_half_even(current_prof, 2),
        "endLevel": round_half_even(end_prof, 2),
        "professionLevelsGained": round_half_even(end_prof - current_prof, 2),
        "totalPed": total_ped,
        "allocations": allocated,
        "attributes": attributes,
        "excluded": excluded,
    })
}

/// `80 + sum(effective_points(skill) / hp_increase)` over contributing
/// skills with a positive level.
pub fn calculate_hp(skill_levels: &Map<String, Value>, skills_data: &[Value]) -> f64 {
    let mut hp = 80.0;
    for (name, hp_inc) in iter_hp_skills(skills_data) {
        let level = level_of(skill_levels, &name);
        if level > 0.0 {
            hp += effective_points(&name, level) / hp_inc;
        }
    }
    hp
}

/// Rank skills by cost-efficiency for gaining HP: regular skills by
/// PED cost per +1 HP, attributes by HP contribution factor.
pub fn hp_skill_optimizer(skill_levels: &Map<String, Value>, skills_data: &[Value]) -> Value {
    let current_hp = calculate_hp(skill_levels, skills_data);

    let mut skills: Vec<Value> = Vec::new();
    let mut attributes: Vec<Value> = Vec::new();

    for (name, hp_inc) in iter_hp_skills(skills_data) {
        let current_level = level_of(skill_levels, &name);

        if is_attribute(&name) {
            let levels_per_hp = hp_inc / 20.0;
            let hp_contributed = if current_level > 0.0 {
                effective_points(&name, current_level) / hp_inc
            } else {
                0.0
            };
            attributes.push(serde_json::json!({
                "name": name,
                "hpIncrease": hp_inc,
                "currentLevel": current_level,
                "levelsPerHp": round_half_even(levels_per_hp, 2),
                "hpContribution": round_half_even(hp_contributed, 2),
            }));
        } else {
            let levels_per_hp = hp_inc;
            let target_level = current_level + levels_per_hp;
            let ped_per_hp = tt_value_at(target_level) - tt_value_at(current_level);
            let hp_per_ped = if ped_per_hp > 0.0 {
                1.0 / ped_per_hp
            } else {
                0.0
            };
            let (codex_category, codex_divisor) = codex_fields(&name);
            skills.push(serde_json::json!({
                "name": name,
                "hpIncrease": hp_inc,
                "currentLevel": current_level,
                "levelsPerHp": round_half_even(levels_per_hp, 1),
                "pedPerHp": round_half_even(ped_per_hp, 2),
                "hpPerPed": round_half_even(hp_per_ped, 4),
                "codexCategory": codex_category,
                "codexDivisor": codex_divisor,
            }));
        }
    }

    sort_stable_by_f64(&mut skills, "pedPerHp", false);
    sort_stable_by_f64(&mut attributes, "levelsPerHp", false);

    serde_json::json!({
        "currentHp": round_half_even(current_hp, 2),
        "skills": skills,
        "attributes": attributes,
    })
}

/// Predicted TT value of the next codex reward (`level / divisor`), or
/// None when the skill has no codex category.
pub fn codex_next_reward(skill_name: &str, current_level: f64) -> Option<f64> {
    let category = get_codex_category(skill_name)?;
    let divisor = reward_divisor(category).expect("every category has a divisor") as f64;
    Some(round_half_even(current_level / divisor, 4))
}

/// Estimated progress through the current codex tier (0-1) via level
/// modulo divisor, or None when the skill has no codex category.
pub fn codex_tier_progress(skill_name: &str, current_level: f64) -> Option<f64> {
    let category = get_codex_category(skill_name)?;
    let divisor = reward_divisor(category).expect("every category has a divisor") as f64;
    if divisor == 0.0 {
        return Some(0.0);
    }
    Some(round_half_even((current_level % divisor) / divisor, 4))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn levels(pairs: &[(&str, f64)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(name, level)| (name.to_string(), json!(level)))
            .collect()
    }

    fn profession() -> Value {
        json!({
            "name": "Sharpshooter",
            "skills": [
                {"skill": {"name": "Rifle"}, "weight": 5},
                {"skill": {"name": "Agility"}, "weight": 2},
                {"skill": {"name": ""}, "weight": 9},
                {"skill": {"name": "Zero Weight"}},
                {"skill": {"name": "Marksmanship"}, "weight": 3},
                "not an object",
            ],
        })
    }

    #[test]
    fn profession_level_weights_attributes_twenty_fold() {
        // (1000*5 + 50*20*2 + 0 + 0)/10000 = 0.7.
        let levels = levels(&[("Rifle", 1000.0), ("Agility", 50.0)]);
        assert_eq!(profession_level(&levels, &profession()), 0.7);
        assert_eq!(profession_level(&Map::new(), &profession()), 0.0);

        let all = all_profession_levels(&levels, &[profession(), json!({"skills": []})]);
        assert_eq!(all.len(), 1, "unnamed professions are skipped");
        assert_eq!(all["Sharpshooter"], 0.7);
    }

    #[test]
    fn skill_ranks_bisect_with_invalid_entries_skipped() {
        let ranks = vec![
            json!({"name": "Novice", "skill": 0}),
            json!({"name": null, "skill": 5}),
            json!({"name": "Broken", "skill": "not a number"}),
            json!({"name": "Apprentice", "skill": 10}),
            json!({"name": "Adept", "skill": 25.5}),
        ];
        assert_eq!(skill_rank(-1.0, &ranks), "Novice", "clamped below");
        assert_eq!(skill_rank(0.0, &ranks), "Novice");
        assert_eq!(skill_rank(10.0, &ranks), "Apprentice", "right bisect");
        assert_eq!(skill_rank(9.99, &ranks), "Novice");
        assert_eq!(skill_rank(99.0, &ranks), "Adept", "clamped above");
        assert_eq!(skill_rank(1.0, &[]), "Unknown");
        assert_eq!(skill_rank(1.0, &[json!({"name": "X"})]), "Unknown");
    }

    #[test]
    fn skill_optimizer_shapes_rank_and_round() {
        let levels = levels(&[
            ("Rifle", 100.0),
            ("Agility", 50.0),
            ("Marksmanship", 2000.0),
        ]);
        // (100*5 + 50*20*2 + 2000*3)/10000 = 0.85.
        let result = profession_skill_optimizer(&levels, &profession());
        assert_eq!(result["currentLevel"], 0.85);
        assert_eq!(result["nextLevel"], 1);
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 2, "attributes and zero weights excluded");
        // Marksmanship (higher level, steeper curve) costs more than
        // Rifle to push the same gap; cheapest first.
        assert_eq!(skills[0]["name"], "Rifle");
        let attrs = result["attributes"].as_array().unwrap();
        assert_eq!(attrs[0]["contributionFactor"], 40.0);
    }

    #[test]
    fn path_optimizer_validates_inputs_and_partitions_results() {
        let level_map = levels(&[("Rifle", 100.0), ("Agility", 50.0)]);
        assert!(profession_path_optimizer(&level_map, &profession(), None, None).is_err());
        assert!(
            profession_path_optimizer(&level_map, &profession(), Some(1.0), Some(1.0)).is_err()
        );

        // Marksmanship is absent from the map: excluded as not unlocked.
        let result = profession_path_optimizer(&level_map, &profession(), Some(1.0), None).unwrap();
        assert_eq!(result["mode"], "target");
        assert_eq!(result["inputTargetLevel"], 1.0);
        assert_eq!(result["inputPedBudget"], Value::Null);
        let excluded = result["excluded"].as_array().unwrap();
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0]["name"], "Marksmanship");
        assert_eq!(excluded[0]["reason"], "not unlocked");
        // Rifle is the only allocatable skill; it carries the whole gap.
        let allocations = result["allocations"].as_array().unwrap();
        assert_eq!(allocations[0]["name"], "Rifle");
        assert!(allocations[0]["levelsToGain"].as_f64().unwrap() > 0.0);
        assert_eq!(
            result["endLevel"].as_f64().unwrap(),
            1.0,
            "target reached exactly after rounding"
        );

        // A target at or below the current level returns immediately.
        let result = profession_path_optimizer(&level_map, &profession(), Some(0.0), None).unwrap();
        assert_eq!(result["totalPed"], 0.0);
        assert_eq!(result["professionLevelsGained"], 0.0);

        // Budget mode spends the whole budget.
        let result = profession_path_optimizer(&level_map, &profession(), None, Some(5.0)).unwrap();
        assert_eq!(result["mode"], "budget");
        assert_eq!(result["totalPed"], 5.0);
    }

    #[test]
    fn hp_calculations_gate_on_positive_levels_and_contributions() {
        let skills_data = vec![
            json!({"name": "Athletics", "hp_increase": 80}),
            json!({"name": "Health", "hp_increase": 6}),
            json!({"name": "No Contribution", "hp_increase": 0}),
            json!({"name": "Broken", "hp_increase": {"bad": 1}}),
            json!({"name": "", "hp_increase": 5}),
        ];
        let level_map = levels(&[("Athletics", 800.0), ("Health", 30.0)]);
        // 80 + 800/80 + 30*20/6 = 80 + 10 + 100 = 190.
        assert_eq!(calculate_hp(&level_map, &skills_data), 190.0);
        assert_eq!(calculate_hp(&Map::new(), &skills_data), 80.0);

        let result = hp_skill_optimizer(&level_map, &skills_data);
        assert_eq!(result["currentHp"], 190.0);
        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "Athletics");
        assert_eq!(skills[0]["levelsPerHp"], 80.0);
        let attrs = result["attributes"].as_array().unwrap();
        assert_eq!(attrs[0]["name"], "Health");
        assert_eq!(attrs[0]["levelsPerHp"], 0.3);
        assert_eq!(attrs[0]["hpContribution"], 100.0);
    }

    #[test]
    fn codex_helpers_return_none_without_a_category() {
        assert_eq!(codex_next_reward("No Such Skill", 100.0), None);
        assert_eq!(codex_tier_progress("No Such Skill", 100.0), None);
        assert_eq!(codex_next_reward("Rifle", 1234.5), Some(6.1725));
        let progress = codex_tier_progress("Rifle", 1234.5).unwrap();
        assert!((0.0..=1.0).contains(&progress));
    }

    /// Full-output equality against values captured from the backend
    /// implementation for this exact fixture: one pin kills arithmetic,
    /// rounding, ordering, and shape mutants across the whole surface.
    #[test]
    fn rich_fixture_outputs_match_the_backend_pins() {
        use eo_wire::normalizer::to_python_json;

        let level_map = levels(&[
            ("Rifle", 1500.25),
            ("Agility", 80.0),
            ("Marksmanship", 320.5),
            ("Anatomy", 45.0),
            ("Strength", 12.0),
        ]);
        let profession = json!({
            "name": "Test Prof",
            "skills": [
                {"skill": {"name": "Rifle"}, "weight": 5},
                {"skill": {"name": "Agility"}, "weight": 2.5},
                {"skill": {"name": "Marksmanship"}, "weight": 3},
                {"skill": {"name": "Anatomy"}, "weight": 0.5},
                {"skill": {"name": "Strength"}, "weight": 1},
                {"skill": {"name": "Locked Skill"}, "weight": 2},
            ],
        });
        let skills_data = vec![
            json!({"name": "Athletics", "hp_increase": 80}),
            json!({"name": "Rifle", "hp_increase": 320}),
            json!({"name": "Health", "hp_increase": 6}),
            json!({"name": "Strength", "hp_increase": 12}),
        ];
        let hp_levels = levels(&[
            ("Athletics", 800.0),
            ("Rifle", 1500.25),
            ("Health", 30.0),
            ("Strength", 12.0),
        ]);

        let pin = |value: &Value, expected: &str, context: &str| {
            assert_eq!(to_python_json(value, None), expected, "{context}");
        };

        pin(
            &profession_skill_optimizer(&level_map, &profession),
            r#"{"attributes": [{"contributionFactor": 50.0, "currentLevel": 80.0, "name": "Agility", "weight": 2.5}, {"contributionFactor": 20.0, "currentLevel": 12.0, "name": "Strength", "weight": 1.0}], "currentLevel": 1.27, "gap": 0.7275, "nextLevel": 2, "skills": [{"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 1500.25, "levelsNeeded": 1455.0, "name": "Rifle", "pedToNextLevel": 21.39, "weight": 5.0}, {"codexCategory": null, "codexDivisor": null, "currentLevel": 320.5, "levelsNeeded": 2424.9, "name": "Marksmanship", "pedToNextLevel": 23.08, "weight": 3.0}, {"codexCategory": null, "codexDivisor": null, "currentLevel": 0.0, "levelsNeeded": 3637.4, "name": "Locked Skill", "pedToNextLevel": 52.08, "weight": 2.0}, {"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 45.0, "levelsNeeded": 14549.5, "name": "Anatomy", "pedToNextLevel": 7762.67, "weight": 0.5}]}"#,
            "skill_optimizer",
        );
        pin(
            &profession_path_optimizer(&level_map, &profession, Some(2.0), None).unwrap(),
            r#"{"allocations": [{"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 1500.25, "levelsToGain": 754.05, "name": "Rifle", "newLevel": 2254.3, "pedCost": 8.17, "weight": 5.0}, {"codexCategory": null, "codexDivisor": null, "currentLevel": 320.5, "levelsToGain": 1167.0, "name": "Marksmanship", "newLevel": 1487.5, "pedCost": 6.08, "weight": 3.0}, {"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 45.0, "levelsToGain": 7.0, "name": "Anatomy", "newLevel": 52.0, "pedCost": 0.0, "weight": 0.5}], "attributes": [{"contributionFactor": 50.0, "currentLevel": 80.0, "name": "Agility", "weight": 2.5}, {"contributionFactor": 20.0, "currentLevel": 12.0, "name": "Strength", "weight": 1.0}], "currentLevel": 1.27, "endLevel": 2.0, "excluded": [{"name": "Locked Skill", "reason": "not unlocked", "weight": 2.0}], "inputPedBudget": null, "inputTargetLevel": 2.0, "mode": "target", "professionLevelsGained": 0.73, "totalPed": 14.25}"#,
            "path_target",
        );
        pin(
            &profession_path_optimizer(&level_map, &profession, None, Some(100.0)).unwrap(),
            r#"{"allocations": [{"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 1500.25, "levelsToGain": 2458.78, "name": "Rifle", "newLevel": 3959.03, "pedCost": 58.92, "weight": 5.0}, {"codexCategory": null, "codexDivisor": null, "currentLevel": 320.5, "levelsToGain": 3096.0, "name": "Marksmanship", "newLevel": 3416.5, "pedCost": 41.08, "weight": 3.0}, {"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 45.0, "levelsToGain": 7.0, "name": "Anatomy", "newLevel": 52.0, "pedCost": 0.0, "weight": 0.5}], "attributes": [{"contributionFactor": 50.0, "currentLevel": 80.0, "name": "Agility", "weight": 2.5}, {"contributionFactor": 20.0, "currentLevel": 12.0, "name": "Strength", "weight": 1.0}], "currentLevel": 1.27, "endLevel": 3.43, "excluded": [{"name": "Locked Skill", "reason": "not unlocked", "weight": 2.0}], "inputPedBudget": 100.0, "inputTargetLevel": null, "mode": "budget", "professionLevelsGained": 2.16, "totalPed": 100.0}"#,
            "path_budget",
        );
        pin(
            &hp_skill_optimizer(&hp_levels, &skills_data),
            r#"{"attributes": [{"currentLevel": 30.0, "hpContribution": 100.0, "hpIncrease": 6.0, "levelsPerHp": 0.3, "name": "Health"}, {"currentLevel": 12.0, "hpContribution": 20.0, "hpIncrease": 12.0, "levelsPerHp": 0.6, "name": "Strength"}], "currentHp": 214.69, "skills": [{"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 800.0, "hpIncrease": 80.0, "hpPerPed": 3.0303, "levelsPerHp": 80.0, "name": "Athletics", "pedPerHp": 0.33}, {"codexCategory": "cat1", "codexDivisor": 200, "currentLevel": 1500.25, "hpIncrease": 320.0, "hpPerPed": 0.3704, "levelsPerHp": 320.0, "name": "Rifle", "pedPerHp": 2.7}]}"#,
            "hp_optimizer",
        );
        assert_eq!(calculate_hp(&hp_levels, &skills_data), 214.68828125);
        pin(
            &Value::Object(all_profession_levels(
                &level_map,
                std::slice::from_ref(&profession),
            )),
            r#"{"Test Prof": 1.27}"#,
            "all_levels",
        );
        assert_eq!(codex_tier_progress("Rifle", 1234.5), Some(0.1725));
    }

    #[test]
    fn python_float_coercion_covers_every_shape() {
        let f = |v: Value| python_float_or(Some(&v), Some(-1.0));
        assert_eq!(python_float_or(None, Some(-1.0)), Some(0.0));
        assert_eq!(f(Value::Null), Some(0.0));
        assert_eq!(f(json!(false)), Some(0.0));
        assert_eq!(f(json!(true)), Some(1.0));
        assert_eq!(f(json!(2.5)), Some(2.5));
        assert_eq!(f(json!(" 3.5 ")), Some(3.5));
        assert_eq!(f(json!("")), Some(0.0));
        assert_eq!(f(json!("junk")), Some(-1.0));
        assert_eq!(f(json!([])), Some(0.0));
        assert_eq!(f(json!([1])), Some(-1.0));
        assert_eq!(f(json!({})), Some(0.0));
        assert_eq!(f(json!({"a": 1})), Some(-1.0));
        assert_eq!(python_float_or(Some(&json!("junk")), None), None);
    }

    #[test]
    fn greedy_ties_pick_the_first_skill_in_order() {
        // Two identical skills: equal marginal ratios every step, so the
        // backend's strict-less-than keeps allocating to the first.
        let level_map = levels(&[("Zeta", 100.0), ("Alpha", 100.0)]);
        let profession = json!({
            "name": "Tied",
            "skills": [
                {"skill": {"name": "Zeta"}, "weight": 5},
                {"skill": {"name": "Alpha"}, "weight": 5},
            ],
        });
        let result = profession_path_optimizer(&level_map, &profession, None, Some(0.05)).unwrap();
        let allocations = result["allocations"].as_array().unwrap();
        assert_eq!(allocations[0]["name"], "Zeta", "first tied skill wins");
        assert!(allocations[0]["levelsToGain"].as_f64().unwrap() > 0.0);
        assert_eq!(allocations[1]["levelsToGain"], 0.0);
    }

    #[test]
    fn zero_allocation_results_sort_alphabetically() {
        // A target at the current level allocates nothing: every skill
        // lands in the unallocated bucket, alphabetical, not entity order.
        let level_map = levels(&[("Zeta", 100.0), ("Alpha", 100.0)]);
        let profession = json!({
            "name": "Untouched",
            "skills": [
                {"skill": {"name": "Zeta"}, "weight": 5},
                {"skill": {"name": "Alpha"}, "weight": 5},
            ],
        });
        let result = profession_path_optimizer(&level_map, &profession, Some(0.1), None).unwrap();
        let names: Vec<&str> = result["allocations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Alpha", "Zeta"]);
    }

    #[test]
    fn flat_curve_hp_costs_report_zero_efficiency() {
        // Levels 0 to 1 sit on the curve's flat start: zero PED per HP,
        // and the inverse reports 0.0 rather than dividing by zero.
        let skills_data = vec![json!({"name": "Flat Skill", "hp_increase": 1})];
        let result = hp_skill_optimizer(&Map::new(), &skills_data);
        let skill = &result["skills"][0];
        assert_eq!(skill["pedPerHp"], 0.0);
        assert_eq!(skill["hpPerPed"], 0.0);
    }

    #[test]
    fn fractional_final_steps_price_from_the_current_position() {
        // One steep-curve skill and a half-level gap: the whole target
        // resolves in a single fractional step, so the price is exactly
        // tt(pos + frac) - tt(pos) at the high position.
        let level_map = levels(&[("Solo", 19000.0)]);
        let profession = json!({
            "name": "Steep",
            "skills": [{"skill": {"name": "Solo"}, "weight": 10000}],
        });
        // Profession level 19000.0; a one-and-a-half-level target takes
        // one whole step and then the fractional one, so the fraction
        // prices from the ADVANCED position (current + allocated), not
        // the starting level.
        let result =
            profession_path_optimizer(&level_map, &profession, Some(19001.5), None).unwrap();
        let allocation = &result["allocations"][0];
        assert_eq!(allocation["levelsToGain"], 1.5);
        let expected = round_half_even(tt_value_at(19001.5) - tt_value_at(19000.0), 2);
        assert_eq!(allocation["pedCost"].as_f64().unwrap(), expected);
        assert!(expected > 1.5, "the curve is steep here");
    }

    #[test]
    fn rank_thresholds_parse_bare_so_falsy_shapes_skip() {
        let ranks = vec![
            json!({"name": "Empty", "skill": ""}),
            json!({"name": "Container", "skill": []}),
            json!({"name": "Real", "skill": 50}),
        ];
        assert_eq!(skill_rank(10.0, &ranks), "Real", "falsy thresholds skip");
        assert_eq!(python_float_bare(&json!("")), None);
        assert_eq!(python_float_bare(&json!([])), None);
        assert_eq!(python_float_bare(&json!(" 12 ")), Some(12.0));
        assert_eq!(python_float_bare(&json!(true)), Some(1.0));
        assert_eq!(python_float_bare(&json!(false)), Some(0.0));
    }

    #[test]
    fn skills_at_the_curve_ceiling_cannot_allocate() {
        let level_map = levels(&[("Capped", 20000.0)]);
        let profession = json!({
            "name": "Ceiling",
            "skills": [{"skill": {"name": "Capped"}, "weight": 10000}],
        });
        let result = profession_path_optimizer(&level_map, &profession, None, Some(50.0)).unwrap();
        assert_eq!(result["totalPed"], 0.0, "all skills at ceiling: break");
        assert_eq!(result["allocations"][0]["levelsToGain"], 0.0);
        let result =
            profession_path_optimizer(&level_map, &profession, Some(20001.0), None).unwrap();
        assert_eq!(result["totalPed"], 0.0);
        assert_eq!(
            result["endLevel"], 20000.0,
            "the target stays unreached when nothing can level"
        );
    }
}
