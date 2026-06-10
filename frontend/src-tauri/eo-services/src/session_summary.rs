//! Materialised per-session summaries, ported from
//! `backend/services/session_summary.py`: a cache of derived state
//! whose source of truth is the tracking tables. Summaries write
//! eagerly when a session ends and clear when a session stops
//! qualifying; the lazy rebuild-on-read path lands with its reader.
//! (The summary table sits outside the snapshot catalogue, so parity
//! here surfaces through the prospect reads rather than the goldens.)

use serde_json::{Map, Value};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::db::DbError;
use eo_wire::normalizer::{round_half_even, to_python_json};

pub const SUMMARY_VERSION: i64 = 1;
pub const DOMINANCE_THRESHOLD: f64 = 0.6;

/// The computed summary for one completed session, or None when the
/// session is active, has no skill gains, or fails the qualifying
/// filters (zero cycled value, zero duration, no gain totals).
#[allow(clippy::too_many_lines)]
pub async fn compute_session_summary(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<Map<String, Value>>, DbError> {
    let session = sqlx::query(
        "SELECT started_at, ended_at, \
         COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) \
         FROM tracking_sessions WHERE id = ? AND ended_at IS NOT NULL",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    let Some(session) = session else {
        return Ok(None);
    };
    let started_at: f64 = session.try_get(0)?;
    let ended_at: f64 = session.try_get(1)?;
    let armour_cost: f64 = session.try_get(2)?;
    let heal_cost: f64 = session.try_get(3)?;
    let dangling_cost: f64 = session.try_get(4)?;

    let has_gains = sqlx::query("SELECT 1 FROM skill_gains WHERE session_id = ? LIMIT 1")
        .bind(session_id)
        .fetch_optional(pool)
        .await;
    // The original tolerates the gains table being absent entirely.
    let Ok(Some(_)) = has_gains else {
        return Ok(None);
    };

    let kill_totals = sqlx::query(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let kills: i64 = kill_totals.try_get(0)?;
    let loot_tt: f64 = kill_totals.try_get::<f64, _>(1).unwrap_or(0.0);
    let enhancer_cost: f64 = kill_totals.try_get::<f64, _>(2).unwrap_or(0.0);

    let weapon_row = sqlx::query(
        "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
         FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let weapon_cost: f64 = weapon_row.try_get::<f64, _>(0).unwrap_or(0.0);

    let mob_rows = sqlx::query(
        "SELECT mob_name, COALESCE(mob_species, ''), COALESCE(mob_maturity, ''), COUNT(*) \
         FROM kills \
         WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
         GROUP BY mob_name, mob_species, mob_maturity \
         ORDER BY COUNT(*) DESC, mob_name ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let mut dominant_mob: Option<String> = None;
    let mut dominant_tag: Option<String> = None;
    if !mob_rows.is_empty() {
        let total_known: i64 = mob_rows
            .iter()
            .map(|row| row.try_get::<i64, _>(3).unwrap_or(0))
            .sum();
        if total_known > 0 {
            let top_name: String = mob_rows[0].try_get(0)?;
            let top_species: String = mob_rows[0].try_get(1)?;
            let top_maturity: String = mob_rows[0].try_get(2)?;
            let top_count: i64 = mob_rows[0].try_get(3)?;
            if top_count as f64 / total_known as f64 >= DOMINANCE_THRESHOLD {
                if !top_species.is_empty() || !top_maturity.is_empty() {
                    dominant_mob = Some(top_name);
                } else {
                    dominant_tag = Some(top_name);
                }
            }
        }
    }

    let tool_rows = sqlx::query(
        "SELECT ts.tool_name, COALESCE(SUM(ts.shots_fired), 0) \
         FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
         GROUP BY ts.tool_name \
         ORDER BY SUM(ts.shots_fired) DESC, ts.tool_name ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let mut dominant_weapon: Option<String> = None;
    if !tool_rows.is_empty() {
        let total_shots: f64 = tool_rows
            .iter()
            .map(|row| row.try_get::<f64, _>(1).unwrap_or(0.0))
            .sum();
        let top_name: String = tool_rows[0].try_get(0)?;
        let top_shots: f64 = tool_rows[0].try_get::<f64, _>(1).unwrap_or(0.0);
        if total_shots > 0.0 && top_shots / total_shots >= DOMINANCE_THRESHOLD {
            dominant_weapon = Some(top_name);
        }
    }

    let regular_rows = sqlx::query(
        "SELECT skill_name, COALESCE(SUM(ped_value), 0) \
         FROM skill_gains \
         WHERE session_id = ? AND ped_value IS NOT NULL \
         GROUP BY skill_name",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let mut regular_skill_ped = Map::new();
    for row in &regular_rows {
        let name: String = row.try_get(0)?;
        let total: f64 = row.try_get::<f64, _>(1).unwrap_or(0.0);
        if total > 0.0 {
            regular_skill_ped.insert(name, Value::from(total));
        }
    }

    let placeholders = vec!["?"; ATTRIBUTE_SKILLS.len()].join(",");
    let attr_sql = format!(
        "SELECT skill_name, COALESCE(SUM(amount), 0) \
         FROM skill_gains \
         WHERE session_id = ? AND skill_name IN ({placeholders}) \
         GROUP BY skill_name"
    );
    let mut attr_query = sqlx::query(sqlx::AssertSqlSafe(attr_sql)).bind(session_id);
    for skill in ATTRIBUTE_SKILLS {
        attr_query = attr_query.bind(skill);
    }
    let attr_rows = attr_query.fetch_all(pool).await?;
    let mut attribute_levels = Map::new();
    for row in &attr_rows {
        let name: String = row.try_get(0)?;
        let total: f64 = row.try_get::<f64, _>(1).unwrap_or(0.0);
        if total > 0.0 {
            attribute_levels.insert(name, Value::from(total));
        }
    }

    let duration_hours = ((ended_at - started_at) / 3600.0).max(0.0);
    let cycled_ped = weapon_cost + enhancer_cost + armour_cost + heal_cost + dangling_cost;
    let regular_skill_tt: f64 = regular_skill_ped.values().filter_map(Value::as_f64).sum();
    let attribute_levels_total: f64 = attribute_levels.values().filter_map(Value::as_f64).sum();

    if cycled_ped <= 0.0 || duration_hours <= 0.0 {
        return Ok(None);
    }
    if regular_skill_tt <= 0.0 && attribute_levels_total <= 0.0 {
        return Ok(None);
    }

    let mut summary = Map::new();
    summary.insert("id".into(), Value::from(session_id));
    summary.insert("startedAt".into(), Value::from(started_at));
    summary.insert("endedAt".into(), Value::from(ended_at));
    summary.insert("durationHours".into(), Value::from(duration_hours));
    summary.insert("armourCost".into(), Value::from(armour_cost));
    summary.insert("healCost".into(), Value::from(heal_cost));
    summary.insert("danglingCost".into(), Value::from(dangling_cost));
    summary.insert("weaponCost".into(), Value::from(weapon_cost));
    summary.insert("enhancerCost".into(), Value::from(enhancer_cost));
    summary.insert("kills".into(), Value::from(kills));
    summary.insert("lootTt".into(), Value::from(loot_tt));
    summary.insert("regularSkillPed".into(), Value::Object(regular_skill_ped));
    summary.insert("attributeLevels".into(), Value::Object(attribute_levels));
    summary.insert(
        "dominantMob".into(),
        dominant_mob.map(Value::from).unwrap_or(Value::Null),
    );
    summary.insert(
        "dominantTag".into(),
        dominant_tag.map(Value::from).unwrap_or(Value::Null),
    );
    summary.insert(
        "dominantWeapon".into(),
        dominant_weapon.map(Value::from).unwrap_or(Value::Null),
    );
    summary.insert(
        "regularSkillTt".into(),
        Value::from(round_half_even(regular_skill_tt, 4)),
    );
    summary.insert(
        "attributeLevelsTotal".into(),
        Value::from(round_half_even(attribute_levels_total, 4)),
    );
    summary.insert(
        "cycledPed".into(),
        Value::from(round_half_even(cycled_ped, 4)),
    );
    Ok(Some(summary))
}

/// Compute and upsert the summary row; clears any stale row when the
/// session does not qualify. The caller owns the surrounding commit
/// semantics, exactly as the original documents.
pub async fn write_session_summary(pool: &SqlitePool, session_id: &str) -> Result<(), DbError> {
    let Some(summary) = compute_session_summary(pool, session_id).await? else {
        sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
            .bind(session_id)
            .execute(pool)
            .await?;
        return Ok(());
    };
    sqlx::query(
        "INSERT OR REPLACE INTO session_summaries (\
         session_id, summary_version, started_at, ended_at, duration_hours, \
         kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
         dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
         regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, \
         dominant_weapon, computed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch('now'))",
    )
    .bind(summary["id"].as_str())
    .bind(SUMMARY_VERSION)
    .bind(summary["startedAt"].as_f64())
    .bind(summary["endedAt"].as_f64())
    .bind(summary["durationHours"].as_f64())
    .bind(summary["kills"].as_i64())
    .bind(summary["lootTt"].as_f64())
    .bind(summary["weaponCost"].as_f64())
    .bind(summary["enhancerCost"].as_f64())
    .bind(summary["armourCost"].as_f64())
    .bind(summary["healCost"].as_f64())
    .bind(summary["danglingCost"].as_f64())
    .bind(summary["cycledPed"].as_f64())
    .bind(to_python_json(&summary["regularSkillPed"], None))
    .bind(to_python_json(&summary["attributeLevels"], None))
    .bind(summary["regularSkillTt"].as_f64())
    .bind(summary["attributeLevelsTotal"].as_f64())
    .bind(summary["dominantMob"].as_str())
    .bind(summary["dominantTag"].as_str())
    .bind(summary["dominantWeapon"].as_str())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a session's summary row; idempotent.
pub async fn delete_session_summary(pool: &SqlitePool, session_id: &str) -> Result<(), DbError> {
    sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}
