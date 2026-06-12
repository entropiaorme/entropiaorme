//! Natively-served character handlers, byte-faithful to
//! `backend/routers/character.py`: calibration status, stats, skills,
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
