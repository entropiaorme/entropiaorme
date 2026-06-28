//! Natively-served character handlers, byte-faithful to the original
//! Python implementation: calibration status, stats, skills,
//! professions, the Prospect forecast family, the optimizers, and the
//! codex progress list, all computed from the calibrated skill levels
//! plus the bundled game-data catalogue through the already-ported
//! calculation services.

use axum::body::Body;
use axum::http::Response;
use eo_services::character_calc::{
    all_profession_levels, codex_next_reward, codex_tier_progress, effective_points,
    hp_skill_optimizer, is_attribute, profession_level, profession_path_optimizer,
    profession_skill_optimizer, skill_rank,
};
use eo_services::codex_categories::get_codex_category;
use eo_services::tracker::{naive_to_epoch, to_iso_utc};
use eo_services::tt_value_curve::{levels_for_tt_value, tt_value_at};
use eo_wire::normalizer::round_half_even;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::hydration::{internal_error, plain_json_response, HydrationState};

/// Skills are considered stale after 30 days without recalibration.
const STALE_DAYS: f64 = 30.0;
const PROSPECT_SAMPLE_WARN_SESSIONS: i64 = 3;
const PROSPECT_SAMPLE_WARN_HOURS: f64 = 2.0;
const PROSPECT_SAMPLE_WARN_CYCLED_PED: f64 = 50.0;

/// The validated `/character/prospect` query parameters.
pub struct ProspectQuery {
    pub profession: String,
    pub target_level: f64,
    pub slice_type: String,
    pub slice_value: Option<String>,
    pub markup_uplift: f64,
}

impl HydrationState {
    /// GET /api/character/calibration.
    pub async fn character_calibration(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let last_ts = match self.last_calibration_ts().await {
            Ok(ts) => ts,
            Err(_) => return internal_error(),
        };
        let Some(last_ts) = last_ts else {
            return plain_json_response(
                &json!({"calibrated": false, "lastCalibration": null, "stale": true}),
            );
        };
        let age_days = (naive_to_epoch(self.clock.now()) - last_ts) / 86400.0;
        plain_json_response(&json!({
            "calibrated": true,
            "lastCalibration": to_iso_utc(last_ts),
            "stale": age_days > STALE_DAYS,
        }))
    }

    /// GET /api/character/stats.
    pub async fn character_stats(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        // `int(...)`: Python truncates the float toward zero.
        let hp = skill_levels
            .get("Health")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;

        let professions_data = self.game_data.get_entities("professions");
        let levels_by_name = all_profession_levels(&skill_levels, professions_data);
        let mut prof_levels: Vec<Value> = Vec::new();
        for prof in professions_data {
            let Some(name) = prof.get("name").and_then(Value::as_str) else {
                continue;
            };
            let level = levels_by_name
                .get(name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if level > 0.0 {
                prof_levels.push(json!({
                    "name": name,
                    "level": level,
                    "category": prof.get("category").cloned().unwrap_or(json!("General")),
                }));
            }
        }
        sort_desc_by_f64(&mut prof_levels, "level");
        prof_levels.truncate(5);
        plain_json_response(&json!({"hp": hp, "topProfessions": prof_levels}))
    }

    /// GET /api/character/skills.
    pub async fn character_skills(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        if skill_levels.is_empty() {
            return plain_json_response(&json!([]));
        }
        let anchor_levels = match self.skill_calibrations(Some("scan")).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let skills_data = self.game_data.get_entities("skills");
        let ranks = get_ranks(&self.game_data);

        let mut result: Vec<Value> = Vec::new();
        for (name, level_value) in &skill_levels {
            let level = level_value.as_f64().unwrap_or(0.0);
            let entity = skills_data
                .iter()
                .find(|s| s.get("name").and_then(Value::as_str) == Some(name.as_str()));
            let category = entity
                .and_then(|e| e.get("category"))
                .filter(|c| json_truthy(c))
                .and_then(Value::as_object)
                .and_then(|c| c.get("name"))
                .cloned()
                .unwrap_or(json!("General"));
            let anchor = anchor_levels.get(name).and_then(Value::as_f64);
            let gain = anchor.map(|a| round_half_even(level - a, 4));
            result.push(json!({
                "name": name,
                "category": category,
                "level": level_value,
                "anchorLevel": anchor,
                "gainSinceAnchor": gain,
                "rankName": skill_rank(level, &ranks),
                "ttValue": round_half_even(tt_value_at(level), 2),
                "isAttribute": is_attribute(name),
            }));
        }
        sort_desc_by_f64(&mut result, "level");
        plain_json_response(&Value::Array(result))
    }

    /// GET /api/character/professions.
    pub async fn character_professions(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let professions_data = self.game_data.get_entities("professions");
        if professions_data.is_empty() {
            return plain_json_response(&json!([]));
        }
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let anchor_skills = match self.skill_calibrations(Some("scan")).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let current_levels = all_profession_levels(&skill_levels, professions_data);
        let anchor_levels = all_profession_levels(&anchor_skills, professions_data);
        let has_anchor = !anchor_skills.is_empty();

        let mut result: Vec<Value> = Vec::new();
        for prof in professions_data {
            let Some(name) = prof.get("name").and_then(Value::as_str) else {
                continue;
            };
            let level = current_levels
                .get(name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let anchor = if has_anchor {
                Some(
                    anchor_levels
                        .get(name)
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                )
            } else {
                None
            };
            let gain = anchor.map(|a| round_half_even(level - a, 4));
            result.push(json!({
                "name": name,
                "level": level,
                "anchorLevel": anchor,
                "gainSinceAnchor": gain,
                "category": prof.get("category").cloned().unwrap_or(json!("General")),
            }));
        }
        sort_desc_by_f64(&mut result, "level");
        plain_json_response(&Value::Array(result))
    }

    /// GET /api/character/prospect-options.
    pub async fn character_prospect_options(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let sessions = match eo_services::session_summary::load_prospect_sessions(self.pool()).await
        {
            Ok(sessions) => sessions,
            Err(_) => return internal_error(),
        };
        plain_json_response(&json!({
            "tags": prospect_option_list(&sessions, "dominantTag"),
            "mobs": prospect_option_list(&sessions, "dominantMob"),
            "weapons": prospect_option_list(&sessions, "dominantWeapon"),
        }))
    }

    /// GET /api/character/prospect (parameters already validated).
    pub async fn character_prospect(
        &self,
        query: &ProspectQuery,
        _if_none_match: Option<&str>,
    ) -> Response<Body> {
        let Some(profession_entity) = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|prof| {
                prof.get("name").and_then(Value::as_str) == Some(query.profession.as_str())
            })
            .cloned()
        else {
            return plain_json_response(&prospect_projection(json!({
                "error": format!("Profession '{}' not found", query.profession),
                "rows": [],
                "warnings": [],
            })));
        };
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let sessions = match eo_services::session_summary::load_prospect_sessions(self.pool()).await
        {
            Ok(sessions) => sessions,
            Err(_) => return internal_error(),
        };
        let matched = match_prospect_sessions(&sessions, &query.slice_type, &query.slice_value);
        let sample = prospect_sample(&matched);
        let result = build_prospect_result(
            &query.profession,
            &profession_entity,
            &skill_levels,
            query.target_level,
            sample,
            &query.slice_type,
            &query.slice_value,
            query.markup_uplift,
        );
        plain_json_response(&prospect_projection(result))
    }

    /// GET /api/character/profession-optimizer.
    pub async fn character_profession_optimizer(
        &self,
        profession: &str,
        _if_none_match: Option<&str>,
    ) -> Response<Body> {
        let prof_entity = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(profession))
            .cloned();
        let Some(prof_entity) = prof_entity else {
            return plain_json_response(&optimizer_projection(json!({
                "error": format!("Profession '{profession}' not found"),
                "skills": [],
                "attributes": [],
            })));
        };
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let mut result = profession_skill_optimizer(&skill_levels, &prof_entity);
        if let Some(map) = result.as_object_mut() {
            map.insert("profession".into(), json!(profession));
        }
        float_divisors(&mut result["skills"]);
        float_field(&mut result, "nextLevel");
        plain_json_response(&optimizer_projection(result))
    }

    /// GET /api/character/profession-path-optimizer (mode arguments
    /// already validated: exactly one of target / budget).
    pub async fn character_path_optimizer(
        &self,
        profession: &str,
        target_level: Option<f64>,
        ped_budget: Option<f64>,
        _if_none_match: Option<&str>,
    ) -> Response<Body> {
        let prof_entity = self
            .game_data
            .get_entities("professions")
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(profession))
            .cloned();
        let Some(prof_entity) = prof_entity else {
            return plain_json_response(&path_projection(json!({
                "error": format!("Profession '{profession}' not found"),
                "allocations": [],
                "attributes": [],
            })));
        };
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let mut result = match profession_path_optimizer(
            &skill_levels,
            &prof_entity,
            target_level,
            ped_budget,
        ) {
            Ok(result) => result,
            // The mode contract is validated at the route; a
            // service-level rejection here is unreachable.
            Err(_) => return internal_error(),
        };
        if let Some(map) = result.as_object_mut() {
            map.insert("profession".into(), json!(profession));
        }
        float_divisors(&mut result["allocations"]);
        plain_json_response(&path_projection(result))
    }

    /// GET /api/character/hp-optimizer.
    pub async fn character_hp_optimizer(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let skills_data = self.game_data.get_entities("skills");
        let mut result = hp_skill_optimizer(&skill_levels, skills_data);
        float_divisors(&mut result["skills"]);
        plain_json_response(&result)
    }

    /// GET /api/character/codex.
    pub async fn character_codex(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let skill_levels = match self.skill_calibrations(None).await {
            Ok(levels) => levels,
            Err(_) => return internal_error(),
        };
        let mut result: Vec<Value> = Vec::new();
        for (name, level_value) in &skill_levels {
            if get_codex_category(name).is_none() {
                continue;
            }
            let level = level_value.as_f64().unwrap_or(0.0);
            let (Some(next_reward), Some(progress)) = (
                codex_next_reward(name, level),
                codex_tier_progress(name, level),
            ) else {
                continue;
            };
            result.push(json!({
                "skillName": name,
                "currentLevel": level_value,
                "nextRewardValue": round_half_even(next_reward, 2),
                "progress": progress,
            }));
        }
        sort_desc_by_f64(&mut result, "currentLevel");
        plain_json_response(&Value::Array(result))
    }

    /// Latest calibrated level per skill: believed-current when
    /// `source` is None, the scan anchor when `source='scan'`,
    /// mirroring `_get_skill_calibrations` (the `MAX(scanned_at)` /
    /// `MAX(id)` tiebreaker SQL verbatim).
    async fn skill_calibrations(
        &self,
        source: Option<&str>,
    ) -> Result<Map<String, Value>, sqlx::Error> {
        let rows = match source {
            None => {
                sqlx::query(
                    "WITH latest_ts AS (\n                        SELECT skill_name, MAX(scanned_at) AS ts\n                        FROM skill_calibrations\n                        GROUP BY skill_name\n                    )\n                    SELECT skill_name, level FROM skill_calibrations\n                    WHERE id IN (\n                        SELECT MAX(s2.id) FROM skill_calibrations s2\n                        JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts\n                        GROUP BY s2.skill_name\n                    )",
                )
                .fetch_all(self.pool())
                .await?
            }
            Some(source) => {
                sqlx::query(
                    "WITH latest_ts AS (\n                        SELECT skill_name, MAX(scanned_at) AS ts\n                        FROM skill_calibrations\n                        WHERE source = ?\n                        GROUP BY skill_name\n                    )\n                    SELECT skill_name, level FROM skill_calibrations\n                    WHERE id IN (\n                        SELECT MAX(s2.id) FROM skill_calibrations s2\n                        JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts\n                        WHERE s2.source = ?\n                        GROUP BY s2.skill_name\n                    )",
                )
                .bind(source)
                .bind(source)
                .fetch_all(self.pool())
                .await?
            }
        };
        let mut levels = Map::new();
        for row in rows {
            let name: String = row.get("skill_name");
            let level: f64 = row.get("level");
            levels.insert(name, json!(level));
        }
        Ok(levels)
    }

    /// Epoch timestamp of the most recent calibration, or None.
    async fn last_calibration_ts(&self) -> Result<Option<f64>, sqlx::Error> {
        let row = sqlx::query("SELECT MAX(scanned_at) as ts FROM skill_calibrations")
            .fetch_one(self.pool())
            .await?;
        Ok(row.get("ts"))
    }
}

/// pydantic's `float | None` coercion over one declared field: the
/// calculation service emits the backend's raw integer (`nextLevel`),
/// and the response model renders it as a float (`6.0`).
fn float_field(object: &mut Value, name: &str) {
    if let Some(value) = object.get_mut(name) {
        if let Some(int_form) = value.as_i64() {
            *value = json!(int_form as f64);
        }
    }
}

/// pydantic's `float | None` coercion over the optimizer rows: the
/// calculation service emits the codex divisor as the backend's raw
/// integer, and the response model renders it as a float (`320.0`).
fn float_divisors(rows: &mut Value) {
    let Some(items) = rows.as_array_mut() else {
        return;
    };
    for item in items {
        if let Some(divisor) = item.get_mut("codexDivisor") {
            if let Some(int_form) = divisor.as_i64() {
                *divisor = json!(int_form as f64);
            }
        }
    }
}

/// Python truthiness over a JSON value.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Stable descending sort by a float key (Python `sort(reverse=True)`).
fn sort_desc_by_f64(items: &mut [Value], key: &str) {
    items.sort_by(|a, b| {
        let left = a.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let right = b.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        right.partial_cmp(&left).expect("levels are finite")
    });
}

/// Sorted `{name, skill}` rank thresholds from the catalogue.
fn get_ranks(game_data: &eo_services::game_data_store::GameDataStore) -> Vec<Value> {
    let entities = game_data.get_entities("skill_ranks");
    let Some(first) = entities.first() else {
        return Vec::new();
    };
    let rows = first
        .get("table")
        .and_then(|t| t.get("rows"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut valid: Vec<Value> = Vec::new();
    for row in rows {
        let Some(threshold) = row.get("skill").and_then(Value::as_f64) else {
            continue;
        };
        let Some(name) = row.get("name").filter(|n| !n.is_null()) else {
            continue;
        };
        valid.push(json!({"name": name, "skill": threshold}));
    }
    valid.sort_by(|a, b| {
        let left = a["skill"].as_f64().unwrap_or(0.0);
        let right = b["skill"].as_f64().unwrap_or(0.0);
        left.partial_cmp(&right).expect("thresholds are finite")
    });
    valid
}

// ── Prospect (ported helper for helper) ─────────────────────────────

/// Aggregate a session group into the Prospect sample shape.
fn prospect_sample(sessions: &[&Value]) -> Map<String, Value> {
    let mut regular_skill_ped: Map<String, Value> = Map::new();
    let mut attribute_levels: Map<String, Value> = Map::new();

    // Python's `sum(())` is the INTEGER zero (rendered `0`), and a
    // non-empty sum starts from it, so the float result carries IEEE
    // positive zero; Rust's empty f64 sum folds from -0.0 instead, so
    // the empty case takes the integer literally.
    let sum_of = |key: &str| -> Value {
        if sessions.is_empty() {
            return json!(0);
        }
        let total: f64 = sessions
            .iter()
            .map(|s| s.get(key).and_then(Value::as_f64).unwrap_or(0.0))
            .sum();
        json!(round_half_even(total, 4))
    };
    let kills: i64 = sessions
        .iter()
        .map(|s| s.get("kills").and_then(Value::as_i64).unwrap_or(0))
        .sum();

    let mut sample = Map::new();
    sample.insert("sessions".into(), json!(sessions.len()));
    sample.insert("kills".into(), json!(kills));
    sample.insert("hours".into(), sum_of("durationHours"));
    sample.insert("cycledPed".into(), sum_of("cycledPed"));
    sample.insert("lootTt".into(), sum_of("lootTt"));
    sample.insert("pes".into(), sum_of("regularSkillTt"));
    sample.insert("attributeLevels".into(), sum_of("attributeLevelsTotal"));

    for session in sessions {
        if let Some(map) = session.get("regularSkillPed").and_then(Value::as_object) {
            for (name, ped) in map {
                let current = regular_skill_ped
                    .get(name)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                regular_skill_ped
                    .insert(name.clone(), json!(current + ped.as_f64().unwrap_or(0.0)));
            }
        }
        if let Some(map) = session.get("attributeLevels").and_then(Value::as_object) {
            for (name, amount) in map {
                let current = attribute_levels
                    .get(name)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                attribute_levels.insert(
                    name.clone(),
                    json!(current + amount.as_f64().unwrap_or(0.0)),
                );
            }
        }
    }

    let hours = sample["hours"].as_f64().unwrap_or(0.0);
    let cycled = sample["cycledPed"].as_f64().unwrap_or(0.0);
    let loot_tt = sample["lootTt"].as_f64().unwrap_or(0.0);
    let pes = sample["pes"].as_f64().unwrap_or(0.0);
    sample.insert(
        "cycledPerHour".into(),
        json!(if hours > 0.0 {
            round_half_even(cycled / hours, 4)
        } else {
            0.0
        }),
    );
    sample.insert(
        "lootPerHour".into(),
        json!(if hours > 0.0 {
            round_half_even(loot_tt / hours, 4)
        } else {
            0.0
        }),
    );
    sample.insert(
        "returnRate".into(),
        json!(if cycled > 0.0 {
            round_half_even(loot_tt / cycled, 4)
        } else {
            0.0
        }),
    );
    sample.insert(
        "pesPerPed".into(),
        json!(if cycled > 0.0 {
            round_half_even(pes / cycled, 6)
        } else {
            0.0
        }),
    );
    sample.insert(
        "lootTtPerPed".into(),
        json!(if cycled > 0.0 {
            round_half_even(loot_tt / cycled, 6)
        } else {
            0.0
        }),
    );

    let mut skill_shares = Map::new();
    for (name, ped) in &regular_skill_ped {
        let ped = ped.as_f64().unwrap_or(0.0);
        if pes > 0.0 && ped > 0.0 {
            skill_shares.insert(name.clone(), json!(ped / pes));
        }
    }
    sample.insert("skillShares".into(), Value::Object(skill_shares));

    let mut attribute_rates = Map::new();
    for (name, amount) in &attribute_levels {
        let amount = amount.as_f64().unwrap_or(0.0);
        if cycled > 0.0 && amount > 0.0 {
            attribute_rates.insert(name.clone(), json!(amount / cycled));
        }
    }
    sample.insert("attributeRates".into(), Value::Object(attribute_rates));
    sample
}

/// The grouped option list for one dominant-value key.
fn prospect_option_list(sessions: &[Value], key: &str) -> Vec<Value> {
    let mut grouped: Map<String, Value> = Map::new();
    for session in sessions {
        let Some(value) = session
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        grouped
            .entry(value.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("group lists are arrays")
            .push(session.clone());
    }

    let mut options: Vec<Value> = Vec::new();
    for (value, group) in &grouped {
        let members: Vec<&Value> = group.as_array().expect("array").iter().collect();
        let sample = prospect_sample(&members);
        options.push(json!({
            "value": value,
            "label": value,
            "sessions": sample["sessions"],
            "kills": sample["kills"],
            "hours": round_half_even(sample["hours"].as_f64().unwrap_or(0.0), 2),
            "cycledPed": round_half_even(sample["cycledPed"].as_f64().unwrap_or(0.0), 2),
        }));
    }

    options.sort_by(|a, b| {
        let sessions_cmp = b["sessions"].as_i64().cmp(&a["sessions"].as_i64());
        if sessions_cmp != std::cmp::Ordering::Equal {
            return sessions_cmp;
        }
        let cycled_cmp = b["cycledPed"]
            .as_f64()
            .partial_cmp(&a["cycledPed"].as_f64())
            .expect("cycled values are finite");
        if cycled_cmp != std::cmp::Ordering::Equal {
            return cycled_cmp;
        }
        a["label"].as_str().cmp(&b["label"].as_str())
    });
    options
}

/// Filter sessions to a slice (`global` passes everything through).
fn match_prospect_sessions<'s>(
    sessions: &'s [Value],
    slice_type: &str,
    slice_value: &Option<String>,
) -> Vec<&'s Value> {
    if slice_type == "global" {
        return sessions.iter().collect();
    }
    let Some(slice_value) = slice_value.as_deref().filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    let key = match slice_type {
        "tag" => "dominantTag",
        "mob" => "dominantMob",
        "weapon" => "dominantWeapon",
        _ => return Vec::new(),
    };
    sessions
        .iter()
        .filter(|session| session.get(key).and_then(Value::as_str) == Some(slice_value))
        .collect()
}

fn build_prospect_warnings(sample: &Map<String, Value>, projected_cycled_ped: f64) -> Vec<Value> {
    let mut warnings = Vec::new();
    if sample["sessions"].as_i64().unwrap_or(0) < PROSPECT_SAMPLE_WARN_SESSIONS {
        warnings.push(json!("Thin sample: fewer than 3 matching sessions."));
    }
    if sample["hours"].as_f64().unwrap_or(0.0) < PROSPECT_SAMPLE_WARN_HOURS {
        warnings.push(json!("Thin sample: less than 2 hours of matching play."));
    }
    let cycled = sample["cycledPed"].as_f64().unwrap_or(0.0);
    if cycled < PROSPECT_SAMPLE_WARN_CYCLED_PED {
        warnings.push(json!("Thin sample: less than 50 PED of matching cycling."));
    }
    if cycled > 0.0 && projected_cycled_ped > cycled * 20.0 {
        warnings.push(json!(
            "Long extrapolation: forecast extends far beyond the observed sample."
        ));
    }
    warnings
}

/// Project skill levels after cycling `total_ped` through the sample's
/// observed composition: (projected levels, projected gains).
fn project_prospect_levels(
    skill_levels: &Map<String, Value>,
    sample: &Map<String, Value>,
    total_ped: f64,
) -> (Map<String, Value>, Map<String, Value>) {
    let mut projected_levels: Map<String, Value> = skill_levels
        .iter()
        .map(|(name, level)| (name.clone(), json!(level.as_f64().unwrap_or(0.0))))
        .collect();
    let mut projected_gains: Map<String, Value> = Map::new();

    let pes_per_ped = sample["pesPerPed"].as_f64().unwrap_or(0.0);
    let skill_tt_budget = total_ped * pes_per_ped;
    if let Some(shares) = sample["skillShares"].as_object() {
        for (skill_name, share) in shares {
            let current = projected_levels
                .get(skill_name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let allocated_tt = skill_tt_budget * share.as_f64().unwrap_or(0.0);
            let gained = levels_for_tt_value(current, allocated_tt);
            projected_levels.insert(
                skill_name.clone(),
                json!(round_half_even(current + gained, 4)),
            );
            projected_gains.insert(skill_name.clone(), json!(round_half_even(gained, 4)));
        }
    }
    if let Some(rates) = sample["attributeRates"].as_object() {
        for (skill_name, rate) in rates {
            let current = projected_levels
                .get(skill_name)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let gained = total_ped * rate.as_f64().unwrap_or(0.0);
            projected_levels.insert(
                skill_name.clone(),
                json!(round_half_even(current + gained, 4)),
            );
            projected_gains.insert(skill_name.clone(), json!(round_half_even(gained, 4)));
        }
    }
    (projected_levels, projected_gains)
}

/// Whether the observed sample contains gains that move the profession.
fn relevant_prospect_progress(sample: &Map<String, Value>, profession: &Value) -> bool {
    let observed_regular = sample["skillShares"].as_object();
    let observed_attrs = sample["attributeRates"].as_object();
    let skills = profession
        .get("skills")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for entry in skills {
        let name = entry
            .get("skill")
            .and_then(|s| s.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
        if name.is_empty() || weight <= 0.0 {
            continue;
        }
        if observed_regular.is_some_and(|m| m.contains_key(name))
            || observed_attrs.is_some_and(|m| m.contains_key(name))
        {
            return true;
        }
    }
    false
}

/// An early-return prospect shape (error before any forecast values).
#[allow(clippy::too_many_arguments)]
fn prospect_error_shape(
    profession_name: &str,
    slice_type: &str,
    slice_value: &Option<String>,
    markup_uplift: f64,
    current_level: f64,
    target_level: f64,
    sample: Map<String, Value>,
    error: &str,
) -> Value {
    json!({
        "profession": profession_name,
        "sliceType": slice_type,
        "sliceValue": slice_value,
        "markupUplift": markup_uplift,
        "currentLevel": round_half_even(current_level, 2),
        "targetLevel": round_half_even(target_level, 2),
        "projectedCycledPed": 0.0,
        "projectedHours": 0.0,
        "expectedLootTt": 0.0,
        "expectedNetTtBurn": 0.0,
        "speculativeLootTt": null,
        "speculativeNetTtBurn": null,
        "sample": sample,
        "rows": [],
        "warnings": [],
        "error": error,
    })
}

/// The full forecast, mirroring `_build_prospect_result` (including
/// the doubling search and 60-step bisection over projected cycling).
#[allow(clippy::too_many_arguments)]
fn build_prospect_result(
    profession_name: &str,
    profession: &Value,
    skill_levels: &Map<String, Value>,
    target_level: f64,
    sample: Map<String, Value>,
    slice_type: &str,
    slice_value: &Option<String>,
    markup_uplift: f64,
) -> Value {
    let current_level = profession_level(skill_levels, profession);

    let projected_levels: Map<String, Value>;
    let mut projected_gains: Map<String, Value> = Map::new();
    let projected_cycled_ped: f64;

    if target_level <= current_level {
        projected_levels = skill_levels
            .iter()
            .map(|(name, level)| (name.clone(), json!(level.as_f64().unwrap_or(0.0))))
            .collect();
        projected_cycled_ped = 0.0;
    } else {
        let cycled = sample["cycledPed"].as_f64().unwrap_or(0.0);
        let hours = sample["hours"].as_f64().unwrap_or(0.0);
        if cycled <= 0.0 || hours <= 0.0 {
            return prospect_error_shape(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "Insufficient matching data for a forecast.",
            );
        }
        if !relevant_prospect_progress(&sample, profession) {
            return prospect_error_shape(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "The observed sample does not contain gains that move this profession.",
            );
        }

        let mut lower = 0.0_f64;
        let mut upper = cycled.max(1.0);
        let mut upper_level = profession_level(
            &project_prospect_levels(skill_levels, &sample, upper).0,
            profession,
        );
        while upper_level < target_level && upper < 1_000_000_000.0 {
            lower = upper;
            upper *= 2.0;
            upper_level = profession_level(
                &project_prospect_levels(skill_levels, &sample, upper).0,
                profession,
            );
        }
        if upper_level < target_level {
            return prospect_error_shape(
                profession_name,
                slice_type,
                slice_value,
                markup_uplift,
                current_level,
                target_level,
                sample,
                "Target is outside the reachable forecast range for this sample.",
            );
        }
        for _ in 0..60 {
            let mid = (lower + upper) / 2.0;
            let (test_levels, _) = project_prospect_levels(skill_levels, &sample, mid);
            if profession_level(&test_levels, profession) >= target_level {
                upper = mid;
            } else {
                lower = mid;
            }
        }
        projected_cycled_ped = round_half_even(upper, 2);
        let projected = project_prospect_levels(skill_levels, &sample, projected_cycled_ped);
        projected_levels = projected.0;
        projected_gains = projected.1;
    }

    let loot_tt_per_ped = sample["lootTtPerPed"].as_f64().unwrap_or(0.0);
    let expected_loot_tt = round_half_even(projected_cycled_ped * loot_tt_per_ped, 2);
    let expected_net_tt_burn = round_half_even(projected_cycled_ped - expected_loot_tt, 2);
    let cycled = sample["cycledPed"].as_f64().unwrap_or(0.0);
    let hours = sample["hours"].as_f64().unwrap_or(0.0);
    let projected_hours = if cycled > 0.0 {
        round_half_even(projected_cycled_ped * (hours / cycled), 2)
    } else {
        0.0
    };

    let (speculative_loot_tt, speculative_net_tt_burn) = if markup_uplift > 0.0 {
        let loot = round_half_even(expected_loot_tt * (1.0 + markup_uplift), 2);
        (
            json!(loot),
            json!(round_half_even(projected_cycled_ped - loot, 2)),
        )
    } else {
        (Value::Null, Value::Null)
    };

    let mut weights: Map<String, Value> = Map::new();
    if let Some(skills) = profession.get("skills").and_then(Value::as_array) {
        for entry in skills {
            let name = entry
                .get("skill")
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let weight = entry.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
            weights.insert(name, json!(weight));
        }
    }

    // `set(skillShares) | set(attributeRates)`: Python set union order
    // is arbitrary, and the rows sort below is total (contribution,
    // attribute flag, then the unique name), so insertion order here
    // never reaches the wire.
    let mut row_names: Vec<String> = Vec::new();
    if let Some(shares) = sample["skillShares"].as_object() {
        row_names.extend(shares.keys().cloned());
    }
    if let Some(rates) = sample["attributeRates"].as_object() {
        for name in rates.keys() {
            if !row_names.contains(name) {
                row_names.push(name.clone());
            }
        }
    }

    let mut rows: Vec<Value> = Vec::new();
    for name in &row_names {
        let current_skill_level = skill_levels
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let projected_gain = projected_gains
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let projected_end_level = projected_levels
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(current_skill_level);
        let weight = weights.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        let contribution = if weight > 0.0 {
            (effective_points(name, projected_gain) * weight) / 10000.0
        } else {
            0.0
        };
        let observed_share = sample["skillShares"]
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let observed_rate = sample["attributeRates"]
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        rows.push(json!({
            "name": name,
            "isAttribute": is_attribute(name),
            "weight": weight,
            "currentLevel": round_half_even(current_skill_level, 2),
            "observedShare": round_half_even(observed_share, 4),
            "observedRate": round_half_even(observed_rate, 6),
            "projectedGain": round_half_even(projected_gain, 2),
            "projectedEndLevel": round_half_even(projected_end_level, 2),
            "professionContribution": round_half_even(contribution, 4),
            "relevant": weight > 0.0,
        }));
    }
    rows.sort_by(|a, b| {
        let contribution = b["professionContribution"]
            .as_f64()
            .partial_cmp(&a["professionContribution"].as_f64())
            .expect("contributions are finite");
        if contribution != std::cmp::Ordering::Equal {
            return contribution;
        }
        let attribute = a["isAttribute"].as_bool().cmp(&b["isAttribute"].as_bool());
        if attribute != std::cmp::Ordering::Equal {
            return attribute;
        }
        a["name"].as_str().cmp(&b["name"].as_str())
    });

    let warnings = build_prospect_warnings(&sample, projected_cycled_ped);
    json!({
        "profession": profession_name,
        "sliceType": slice_type,
        "sliceValue": slice_value,
        "markupUplift": markup_uplift,
        "currentLevel": round_half_even(current_level, 2),
        "targetLevel": round_half_even(target_level, 2),
        "projectedCycledPed": projected_cycled_ped,
        "projectedHours": projected_hours,
        "expectedLootTt": expected_loot_tt,
        "expectedNetTtBurn": expected_net_tt_burn,
        "speculativeLootTt": speculative_loot_tt,
        "speculativeNetTtBurn": speculative_net_tt_burn,
        "sample": sample,
        "rows": rows,
        "warnings": warnings,
    })
}

// ── Response-model projections ──────────────────────────────────────
// The three exclude-unset routes serialise in MODEL declaration order:
// declared fields first (only when set), extra keys after in handler
// order, exactly as pydantic's `extra="allow"` emits them.

fn model_projection(declared: &[&str], data: Value) -> Value {
    let Value::Object(map) = data else {
        return data;
    };
    let mut out = Map::new();
    for key in declared {
        if let Some(value) = map.get(*key) {
            out.insert((*key).to_string(), value.clone());
        }
    }
    for (key, value) in map {
        if !declared.contains(&key.as_str()) {
            out.insert(key, value);
        }
    }
    Value::Object(out)
}

fn prospect_projection(data: Value) -> Value {
    model_projection(&["error", "rows", "warnings"], data)
}

fn optimizer_projection(data: Value) -> Value {
    model_projection(
        &[
            "skills",
            "attributes",
            "profession",
            "currentLevel",
            "nextLevel",
            "gap",
            "error",
        ],
        data,
    )
}

fn path_projection(data: Value) -> Value {
    model_projection(
        &[
            "allocations",
            "attributes",
            "profession",
            "mode",
            "inputTargetLevel",
            "inputPedBudget",
            "currentLevel",
            "endLevel",
            "professionLevelsGained",
            "totalPed",
            "excluded",
            "error",
        ],
        data,
    )
}

// The prospect helpers are pure functions over the summary shapes;
// these pins hold their arithmetic, ordering, and early returns
// hermetically (the retired cross-language oracle proved this surface
// byte-for-byte; the committed goldens now hold it).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eo_services::clock::MockClock;
    use eo_services::db::Db;
    use eo_services::game_data_store::GameDataStore;
    use http_body_util::BodyExt;
    use serde_json::json;

    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn session(
        hours: f64,
        cycled: f64,
        loot: f64,
        kills: i64,
        mob: &str,
        tag: &str,
        weapon: &str,
        skill_ped: Value,
        attrs: Value,
        skill_tt: f64,
        attr_total: f64,
    ) -> Value {
        json!({
            "id": "s", "startedAt": 0.0, "endedAt": hours * 3600.0,
            "durationHours": hours, "kills": kills, "lootTt": loot,
            "weaponCost": 0.0, "enhancerCost": 0.0, "armourCost": 0.0,
            "healCost": 0.0, "danglingCost": 0.0, "cycledPed": cycled,
            "regularSkillPed": skill_ped, "attributeLevels": attrs,
            "regularSkillTt": skill_tt, "attributeLevelsTotal": attr_total,
            "dominantMob": mob, "dominantTag": tag, "dominantWeapon": weapon,
        })
    }

    fn marksman() -> Value {
        json!({
            "name": "Marksman", "category": "Combat",
            "skills": [
                {"weight": 40, "skill": {"name": "Rifle"}},
                {"weight": 10, "skill": {"name": "Anatomy"}},
                {"weight": 3, "skill": {"name": "Agility"}},
                {"weight": 0, "skill": {"name": "Zeroed"}},
            ],
        })
    }

    #[test]
    fn the_sample_aggregates_shares_and_rates_in_first_seen_order() {
        let one = session(
            1.0,
            100.0,
            90.0,
            50,
            "Atrox",
            "",
            "Opalo",
            json!({"Rifle": 2.0}),
            json!({"Agility": 0.05}),
            2.0,
            0.05,
        );
        let two = session(
            1.0,
            100.0,
            90.0,
            50,
            "Snable",
            "team",
            "Opalo",
            json!({"Rifle": 1.0, "Anatomy": 1.0}),
            json!({}),
            2.0,
            0.0,
        );
        let sample = prospect_sample(&[&one, &two]);
        assert_eq!(
            serde_json::to_string(&Value::Object(sample)).unwrap(),
            serde_json::to_string(&json!({
                "sessions": 2, "kills": 100, "hours": 2.0, "cycledPed": 200.0,
                "lootTt": 180.0, "pes": 4.0, "attributeLevels": 0.05,
                "cycledPerHour": 100.0, "lootPerHour": 90.0, "returnRate": 0.9,
                "pesPerPed": 0.02, "lootTtPerPed": 0.9,
                "skillShares": {"Rifle": 0.75, "Anatomy": 0.25},
                "attributeRates": {"Agility": 0.00025},
            }))
            .unwrap()
        );

        // The empty sample keeps Python's INTEGER zero sums.
        let empty = prospect_sample(&[]);
        assert_eq!(
            serde_json::to_string(&Value::Object(empty)).unwrap(),
            serde_json::to_string(&json!({
                "sessions": 0, "kills": 0, "hours": 0, "cycledPed": 0,
                "lootTt": 0, "pes": 0, "attributeLevels": 0,
                "cycledPerHour": 0.0, "lootPerHour": 0.0, "returnRate": 0.0,
                "pesPerPed": 0.0, "lootTtPerPed": 0.0,
                "skillShares": {}, "attributeRates": {},
            }))
            .unwrap()
        );

        // Zero-valued shares and rates are filtered (strictly positive).
        let zeroed = session(
            1.0,
            100.0,
            0.0,
            0,
            "",
            "",
            "",
            json!({"Rifle": 0.0}),
            json!({"Agility": 0.0}),
            0.0,
            0.0,
        );
        let sample = prospect_sample(&[&zeroed]);
        assert_eq!(sample["skillShares"], json!({}));
        assert_eq!(sample["attributeRates"], json!({}));
    }

    #[test]
    fn slices_filter_sessions_and_options_group_and_sort() {
        let a1 = session(
            1.0,
            60.0,
            50.0,
            10,
            "Atrox",
            "",
            "Opalo",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        let a2 = session(
            1.0,
            40.0,
            30.0,
            5,
            "Atrox",
            "team",
            "Imk2",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        let b = session(
            2.0,
            200.0,
            150.0,
            70,
            "Snable",
            "team",
            "Opalo",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        let sessions = vec![a1, a2, b];

        assert_eq!(match_prospect_sessions(&sessions, "global", &None).len(), 3);
        assert_eq!(
            match_prospect_sessions(&sessions, "mob", &Some("Atrox".into())).len(),
            2
        );
        assert_eq!(
            match_prospect_sessions(&sessions, "tag", &Some("team".into())).len(),
            2
        );
        assert_eq!(
            match_prospect_sessions(&sessions, "weapon", &Some("Opalo".into())).len(),
            2
        );
        assert!(match_prospect_sessions(&sessions, "mob", &None).is_empty());
        assert!(match_prospect_sessions(&sessions, "mob", &Some(String::new())).is_empty());
        assert!(match_prospect_sessions(&sessions, "other", &Some("x".into())).is_empty());

        // Options sort by sessions desc, then cycled desc, then label.
        let options = prospect_option_list(&sessions, "dominantMob");
        assert_eq!(
            serde_json::to_string(&options[0]).unwrap(),
            serde_json::to_string(&json!({
                "value": "Atrox", "label": "Atrox", "sessions": 2, "kills": 15,
                "hours": 2.0, "cycledPed": 100.0,
            }))
            .unwrap()
        );
        assert_eq!(options[1]["value"], "Snable");
        // A cycled tie within equal session counts falls to the label.
        let t1 = session(
            1.0,
            50.0,
            0.0,
            0,
            "Beta",
            "",
            "",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        let t2 = session(
            1.0,
            50.0,
            0.0,
            0,
            "Alpha",
            "",
            "",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        let tied = prospect_option_list(&[t1, t2], "dominantMob");
        assert_eq!(tied[0]["label"], "Alpha");
        assert_eq!(tied[1]["label"], "Beta");
        // Sessions without the key are skipped entirely.
        let untagged = session(
            1.0,
            10.0,
            0.0,
            0,
            "X",
            "",
            "",
            json!({}),
            json!({}),
            0.0,
            0.0,
        );
        assert!(prospect_option_list(&[untagged], "dominantTag").is_empty());
    }

    #[test]
    fn warnings_fire_strictly_below_their_thresholds() {
        let mut sample = Map::new();
        sample.insert("sessions".into(), json!(3));
        sample.insert("hours".into(), json!(2.0));
        sample.insert("cycledPed".into(), json!(50.0));
        assert!(build_prospect_warnings(&sample, 1000.0).is_empty());

        sample.insert("sessions".into(), json!(2));
        sample.insert("hours".into(), json!(1.99));
        sample.insert("cycledPed".into(), json!(49.99));
        let warnings = build_prospect_warnings(&sample, 1000.0);
        assert_eq!(
            warnings,
            vec![
                json!("Thin sample: fewer than 3 matching sessions."),
                json!("Thin sample: less than 2 hours of matching play."),
                json!("Thin sample: less than 50 PED of matching cycling."),
                json!("Long extrapolation: forecast extends far beyond the observed sample."),
            ]
        );
        // The extrapolation warning needs MORE than 20x the observed.
        sample.insert("sessions".into(), json!(3));
        sample.insert("hours".into(), json!(2.0));
        sample.insert("cycledPed".into(), json!(50.0));
        assert!(build_prospect_warnings(&sample, 1000.0).is_empty());
        assert_eq!(build_prospect_warnings(&sample, 1000.01).len(), 1);
    }

    #[test]
    fn projection_applies_shares_then_attribute_rates() {
        let mut sample = Map::new();
        sample.insert("pesPerPed".into(), json!(0.02));
        sample.insert("skillShares".into(), json!({"Rifle": 1.0}));
        sample.insert("attributeRates".into(), json!({"Agility": 0.001}));
        let mut levels = Map::new();
        levels.insert("Rifle".into(), json!(100.0));

        let (projected, gains) = project_prospect_levels(&levels, &sample, 50.0);
        // The attribute leg is exact: 50 PED * 0.001 levels/PED.
        assert_eq!(projected["Agility"], json!(0.05));
        assert_eq!(gains["Agility"], json!(0.05));
        // The skill leg runs the real curve over a 1.0 PED TT budget.
        let expected_gain = eo_services::tt_value_curve::levels_for_tt_value(100.0, 1.0);
        assert_eq!(
            gains["Rifle"],
            json!(eo_wire::normalizer::round_half_even(expected_gain, 4))
        );
        // Zero cycling projects nothing.
        let (unchanged, gains) = project_prospect_levels(&levels, &sample, 0.0);
        assert_eq!(unchanged["Rifle"], json!(100.0));
        assert_eq!(gains["Rifle"], json!(0.0));
    }

    #[test]
    fn relevance_requires_a_weighted_skill_in_the_observed_sample() {
        let profession = marksman();
        let mut sample = Map::new();
        sample.insert("skillShares".into(), json!({"Rifle": 1.0}));
        sample.insert("attributeRates".into(), json!({}));
        assert!(relevant_prospect_progress(&sample, &profession));
        sample.insert("skillShares".into(), json!({"Unrelated": 1.0}));
        assert!(!relevant_prospect_progress(&sample, &profession));
        // A zero-weight match does not count.
        sample.insert("skillShares".into(), json!({"Zeroed": 1.0}));
        assert!(!relevant_prospect_progress(&sample, &profession));
        // An attribute-rate match does.
        sample.insert("attributeRates".into(), json!({"Agility": 0.1}));
        assert!(relevant_prospect_progress(&sample, &profession));
    }

    #[test]
    fn the_forecast_walks_its_ladder_of_early_returns_into_a_full_result() {
        let profession = marksman();
        let mut levels = Map::new();
        levels.insert("Rifle".into(), json!(1000.0));

        // target <= current: a zero forecast with no error key.
        let current = profession_level(&levels, &profession);
        let result = build_prospect_result(
            "Marksman",
            &profession,
            &levels,
            current,
            prospect_sample(&[]),
            "global",
            &None,
            0.0,
        );
        assert!(result.get("error").is_none());
        assert_eq!(result["projectedCycledPed"], json!(0.0));
        assert_eq!(result["rows"], json!([]));
        assert_eq!(
            result["warnings"],
            json!([
                "Thin sample: fewer than 3 matching sessions.",
                "Thin sample: less than 2 hours of matching play.",
                "Thin sample: less than 50 PED of matching cycling.",
            ])
        );

        // No observed cycling: the insufficient-data error.
        let result = build_prospect_result(
            "Marksman",
            &profession,
            &levels,
            current + 1.0,
            prospect_sample(&[]),
            "global",
            &None,
            0.0,
        );
        assert_eq!(
            result["error"],
            json!("Insufficient matching data for a forecast.")
        );

        // Observed cycling that cannot move the profession.
        let unrelated = session(
            1.0,
            100.0,
            90.0,
            10,
            "",
            "",
            "",
            json!({"Unrelated": 2.0}),
            json!({}),
            2.0,
            0.0,
        );
        let result = build_prospect_result(
            "Marksman",
            &profession,
            &levels,
            current + 1.0,
            prospect_sample(&[&unrelated]),
            "global",
            &None,
            0.0,
        );
        assert_eq!(
            result["error"],
            json!("The observed sample does not contain gains that move this profession.")
        );

        // A reachable target: the full forecast shape.
        let rich = session(
            2.0,
            200.0,
            150.0,
            80,
            "Atrox",
            "",
            "Opalo",
            json!({"Rifle": 4.0, "Unrelated": 1.0}),
            json!({"Agility": 0.06}),
            5.0,
            0.06,
        );
        let sample = prospect_sample(&[&rich]);
        let result = build_prospect_result(
            "Marksman",
            &profession,
            &levels,
            current + 0.05,
            sample.clone(),
            "mob",
            &Some("Atrox".into()),
            0.1,
        );
        assert!(result.get("error").is_none());
        let keys: Vec<&str> = result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "profession",
                "sliceType",
                "sliceValue",
                "markupUplift",
                "currentLevel",
                "targetLevel",
                "projectedCycledPed",
                "projectedHours",
                "expectedLootTt",
                "expectedNetTtBurn",
                "speculativeLootTt",
                "speculativeNetTtBurn",
                "sample",
                "rows",
                "warnings",
            ]
        );
        let projected = result["projectedCycledPed"].as_f64().unwrap();
        assert!(projected > 0.0);
        // The hours/loot expectations scale off the observed ratios.
        assert_eq!(
            result["projectedHours"],
            json!(eo_wire::normalizer::round_half_even(
                projected * (2.0 / 200.0),
                2
            ))
        );
        let expected_loot = eo_wire::normalizer::round_half_even(projected * 0.75, 2);
        assert_eq!(result["expectedLootTt"], json!(expected_loot));
        assert_eq!(
            result["expectedNetTtBurn"],
            json!(eo_wire::normalizer::round_half_even(
                projected - expected_loot,
                2
            ))
        );
        // The speculative branch applies the uplift to the loot side.
        let speculative = eo_wire::normalizer::round_half_even(expected_loot * 1.1, 2);
        assert_eq!(result["speculativeLootTt"], json!(speculative));
        assert_eq!(
            result["speculativeNetTtBurn"],
            json!(eo_wire::normalizer::round_half_even(
                projected - speculative,
                2
            ))
        );
        // Rows: relevant first by contribution, attributes flagged, the
        // unrelated zero-weight skill last with relevant=false.
        let rows = result["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        let row = |name: &str| {
            rows.iter()
                .find(|r| r["name"] == name)
                .unwrap_or_else(|| panic!("row {name}"))
        };
        assert_eq!(row("Rifle")["relevant"], json!(true));
        assert_eq!(row("Rifle")["isAttribute"], json!(false));
        assert_eq!(row("Rifle")["observedShare"], json!(0.8));
        assert_eq!(row("Agility")["isAttribute"], json!(true));
        assert_eq!(row("Agility")["observedRate"], json!(0.0003));
        // The attribute's rounded contribution ties at zero with the
        // unrelated skill; the tie breaks non-attribute first, then
        // both sit below the contributing skill.
        assert_eq!(rows[0]["name"], "Rifle");
        assert_eq!(rows[1]["name"], "Unrelated");
        assert_eq!(rows[1]["relevant"], json!(false));
        assert_eq!(rows[1]["professionContribution"], json!(0.0));
        assert_eq!(rows[2]["name"], "Agility");
        let contributions: Vec<f64> = rows
            .iter()
            .map(|r| r["professionContribution"].as_f64().unwrap())
            .collect();
        assert!(contributions[0] >= contributions[1]);

        // An unreachable target reports the range error.
        let result = build_prospect_result(
            "Marksman",
            &profession,
            &levels,
            1.0e9,
            sample,
            "global",
            &None,
            0.0,
        );
        assert_eq!(
            result["error"],
            json!("Target is outside the reachable forecast range for this sample.")
        );
    }

    #[test]
    fn the_projections_order_declared_fields_then_extras() {
        let shaped = prospect_projection(json!({
            "zeta": 1, "warnings": [], "rows": [], "error": "e", "alpha": 2,
        }));
        let keys: Vec<&str> = shaped
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["error", "rows", "warnings", "zeta", "alpha"]);
        let shaped = path_projection(json!({
            "excluded": [], "allocations": [], "mode": "target", "attributes": [],
        }));
        let keys: Vec<&str> = shaped
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["allocations", "attributes", "mode", "excluded"]);
        let shaped = optimizer_projection(json!({
            "error": "e", "skills": [], "attributes": [],
        }));
        let keys: Vec<&str> = shaped
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["skills", "attributes", "error"]);
        // Non-objects pass through untouched.
        assert_eq!(prospect_projection(json!([1])), json!([1]));
    }

    #[test]
    fn float_coercions_and_the_stable_sort_behave_like_the_models() {
        let mut value = json!({"nextLevel": 6, "other": 7});
        float_field(&mut value, "nextLevel");
        float_field(&mut value, "missing");
        assert_eq!(value["nextLevel"], json!(6.0));
        assert_eq!(value["other"], json!(7));

        let mut rows = json!([
            {"codexDivisor": 320}, {"codexDivisor": null}, {"name": "x"},
        ]);
        float_divisors(&mut rows);
        assert_eq!(rows[0]["codexDivisor"], json!(320.0));
        assert_eq!(rows[1]["codexDivisor"], json!(null));
        float_divisors(&mut json!({}));

        let mut items = vec![
            json!({"level": 1.0, "name": "a"}),
            json!({"level": 3.0, "name": "b"}),
            json!({"level": 1.0, "name": "c"}),
        ];
        sort_desc_by_f64(&mut items, "level");
        let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["b", "a", "c"], "descending and stable on ties");

        for (value, expected) in [
            (json!(null), false),
            (json!(false), false),
            (json!(true), true),
            (json!(0), false),
            (json!(0.0), false),
            (json!(2), true),
            (json!(""), false),
            (json!("x"), true),
            (json!([]), false),
            (json!([1]), true),
            (json!({}), false),
            (json!({"a": 1}), true),
        ] {
            assert_eq!(json_truthy(&value), expected, "{value}");
        }
    }

    fn write_fixture(dir: &std::path::Path, name: &str, value: &Value) {
        std::fs::write(dir.join(name), serde_json::to_string(value).unwrap()).unwrap();
    }

    async fn seeded_state(dir: &std::path::Path) -> HydrationState {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        write_fixture(
            &snapshot,
            "professions.json",
            &json!([
                {"name": "Marksman", "category": "Combat", "skills": [
                    {"weight": 40, "skill": {"name": "Rifle"}},
                    {"weight": 10, "skill": {"name": "Anatomy"}},
                ]},
                {"name": "Healer", "skills": [
                    {"weight": 50, "skill": {"name": "Anatomy"}},
                ]},
            ]),
        );
        write_fixture(
            &snapshot,
            "skills.json",
            &json!([
                {"name": "Rifle", "category": {"name": "Combat"}},
                {"name": "Anatomy", "category": {"name": "Medical"}},
                {"name": "Health"},
            ]),
        );
        write_fixture(
            &snapshot,
            "skill_ranks.json",
            &json!({"table": {"rows": [
                {"name": "Adept", "skill": 1000},
                {"name": "Novice", "skill": 0},
                {"name": "Broken", "skill": null},
                {"name": null, "skill": 5},
            ]}}),
        );
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        for (name, level, source, ts) in [
            ("Rifle", 1200.0, "scan", 1700000000.5),
            ("Rifle", 1250.0, "chatlog", 1700003600.0),
            ("Anatomy", 800.0, "scan", 1700000000.5),
            ("Health", 142.7, "scan", 1700000000.5),
        ] {
            sqlx::query(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(name)
            .bind(level)
            .bind(source)
            .bind(ts)
            .execute(db.pool())
            .await
            .unwrap();
        }
        HydrationState::new(
            db,
            Arc::new(GameDataStore::new(&snapshot).unwrap()),
            Arc::new(MockClock::new(
                Some(
                    chrono::NaiveDateTime::parse_from_str(
                        "2023-11-20 12:00:00",
                        "%Y-%m-%d %H:%M:%S",
                    )
                    .unwrap(),
                ),
                0.0,
            )),
            dir.to_path_buf(),
        )
    }

    async fn body_of(response: Response<Body>) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn the_handlers_shape_the_seeded_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = seeded_state(dir.path()).await;

        // Calibration: the believed-latest timestamp in the UTC ISO
        // form; the frozen clock sits days past it, inside the
        // 30-day staleness window.
        let body = body_of(state.character_calibration(None).await).await;
        assert_eq!(
            body,
            b"{\"calibrated\":true,\"lastCalibration\":\"2023-11-14T23:13:20+00:00\",\"stale\":false}"
        );

        // Stats: Python int() truncation of Health, professions ranked.
        let body = body_of(state.character_stats(None).await).await;
        let stats: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats["hp"], json!(142));
        let top = stats["topProfessions"].as_array().unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0]["name"], "Marksman");
        assert_eq!(top[0]["category"], "Combat");
        assert_eq!(top[1]["name"], "Healer");
        assert_eq!(top[1]["category"], "General");

        // Skills: believed-current levels, anchors, gains, ranks, TT.
        let body = body_of(state.character_skills(None).await).await;
        let skills: Value = serde_json::from_slice(&body).unwrap();
        let rifle = &skills[0];
        assert_eq!(rifle["name"], "Rifle");
        assert_eq!(rifle["category"], "Combat");
        assert_eq!(rifle["level"], json!(1250.0));
        assert_eq!(rifle["anchorLevel"], json!(1200.0));
        assert_eq!(rifle["gainSinceAnchor"], json!(50.0));
        assert_eq!(rifle["rankName"], "Adept");
        assert_eq!(
            rifle["ttValue"],
            json!(eo_wire::normalizer::round_half_even(
                eo_services::tt_value_curve::tt_value_at(1250.0),
                2
            ))
        );
        assert_eq!(rifle["isAttribute"], json!(false));
        let health = &skills[2];
        assert_eq!(health["name"], "Health");
        assert_eq!(health["category"], "General");
        assert_eq!(health["isAttribute"], json!(true));

        // Professions: anchor levels computed over the scan snapshot.
        let body = body_of(state.character_professions(None).await).await;
        let professions: Value = serde_json::from_slice(&body).unwrap();
        let marksman = &professions[0];
        assert_eq!(marksman["name"], "Marksman");
        let level = marksman["level"].as_f64().unwrap();
        let anchor = marksman["anchorLevel"].as_f64().unwrap();
        assert!(level > anchor, "the chatlog gain moves believed-current");
        assert_eq!(
            marksman["gainSinceAnchor"],
            json!(eo_wire::normalizer::round_half_even(level - anchor, 4))
        );

        // The codex list serves only codex-category skills.
        let body = body_of(state.character_codex(None).await).await;
        let codex: Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = codex
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["skillName"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["Rifle", "Anatomy"],
            "Health carries no codex category"
        );
        assert_eq!(codex[0]["currentLevel"], json!(1250.0));
        assert_eq!(codex[0]["nextRewardValue"], json!(6.25));
        assert_eq!(codex[0]["progress"], json!(0.25));

        // The optimizer composes the calc service with the projections.
        let body = body_of(state.character_profession_optimizer("Marksman", None).await).await;
        let optimizer: Value = serde_json::from_slice(&body).unwrap();
        let keys: Vec<&str> = optimizer
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "skills",
                "attributes",
                "profession",
                "currentLevel",
                "nextLevel",
                "gap"
            ]
        );
        assert_eq!(optimizer["profession"], "Marksman");
        assert!(optimizer["nextLevel"].is_f64(), "the model renders floats");

        // Both path-optimizer modes carry their mode inputs.
        let body = body_of(
            state
                .character_path_optimizer("Marksman", Some(7.0), None, None)
                .await,
        )
        .await;
        let path: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(path["mode"], "target");
        assert_eq!(path["inputTargetLevel"], json!(7.0));
        assert_eq!(path["inputPedBudget"], json!(null));
        let body = body_of(
            state
                .character_path_optimizer("Marksman", None, Some(25.0), None)
                .await,
        )
        .await;
        let path: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(path["mode"], "budget");
        assert_eq!(path["inputPedBudget"], json!(25.0));
    }

    #[test]
    fn ranks_parse_and_sort_from_the_catalogue_table() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            "skill_ranks.json",
            &json!({"table": {"rows": [
                {"name": "B", "skill": 200},
                {"name": "A", "skill": 100.5},
                {"name": null, "skill": 5},
                {"name": "NoThreshold"},
                {"name": "Bad", "skill": "x"},
            ]}}),
        );
        let store = GameDataStore::new(dir.path()).unwrap();
        let ranks = get_ranks(&store);
        assert_eq!(
            ranks,
            vec![
                json!({"name": "A", "skill": 100.5}),
                json!({"name": "B", "skill": 200.0}),
            ]
        );
        let empty = GameDataStore::new(&dir.path().join("missing")).unwrap();
        assert!(get_ranks(&empty).is_empty());
    }
}
