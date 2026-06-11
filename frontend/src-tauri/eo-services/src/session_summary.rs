//! Materialised per-session summaries, ported from
//! `backend/services/session_summary.py`: a cache of derived state
//! whose source of truth is the tracking tables. Summaries write
//! eagerly when a session ends and clear when a session stops
//! qualifying; the lazy rebuild-on-read path lands with its reader.
//! (The summary table sits outside the snapshot catalogue, so parity
//! here surfaces through the prospect reads rather than the goldens.)

use serde_json::{json, Map, Value};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::db::{decoded_f64, DbError};
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
    // The original tolerates the gains table being absent entirely
    // (its operational-error catch). Any other failure propagates:
    // a transient driver error must not read as "no gains" and let
    // the write path clear a valid summary.
    match has_gains {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(None),
        Err(sqlx::Error::Database(db_error)) if db_error.message().contains("no such table") => {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    }

    let kill_totals = sqlx::query(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let kills: i64 = kill_totals.try_get(0)?;
    let loot_tt = decoded_f64(&kill_totals, 1);
    let enhancer_cost = decoded_f64(&kill_totals, 2);

    let weapon_row = sqlx::query(
        "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
         FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let weapon_cost = decoded_f64(&weapon_row, 0);

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
        let total_shots: f64 = tool_rows.iter().map(|row| decoded_f64(row, 1)).sum();
        let top_name: String = tool_rows[0].try_get(0)?;
        let top_shots = decoded_f64(&tool_rows[0], 1);
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
        let total = decoded_f64(row, 1);
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
        let total = decoded_f64(row, 1);
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
/// One stored summary row to its camelCase prospect shape (the
/// original's column order and coercions: or-zero floats, an integer
/// kill count, JSON columns parsed when non-empty, and the dominant
/// fields passed through raw).
fn row_to_prospect_dict(row: &sqlx::sqlite::SqliteRow) -> Value {
    use sqlx::Row as _;
    let float_or_zero = |index: usize| -> f64 {
        row.try_get::<Option<f64>, _>(index)
            .ok()
            .flatten()
            .unwrap_or(0.0)
    };
    let json_or_empty = |index: usize| -> Value {
        row.get::<Option<String>, _>(index)
            .filter(|text| !text.is_empty())
            .map(|text| serde_json::from_str(&text).expect("stored summary JSON parses"))
            .unwrap_or_else(|| json!({}))
    };
    json!({
        "id": row.get::<String, _>(0),
        "startedAt": float_or_zero(1),
        "endedAt": float_or_zero(2),
        "durationHours": float_or_zero(3),
        "kills": row.try_get::<Option<i64>, _>(4).ok().flatten().unwrap_or(0),
        "lootTt": float_or_zero(5),
        "weaponCost": float_or_zero(6),
        "enhancerCost": float_or_zero(7),
        "armourCost": float_or_zero(8),
        "healCost": float_or_zero(9),
        "danglingCost": float_or_zero(10),
        "cycledPed": float_or_zero(11),
        "regularSkillPed": json_or_empty(12),
        "attributeLevels": json_or_empty(13),
        "regularSkillTt": float_or_zero(14),
        "attributeLevelsTotal": float_or_zero(15),
        "dominantMob": row.get::<Option<String>, _>(16),
        "dominantTag": row.get::<Option<String>, _>(17),
        "dominantWeapon": row.get::<Option<String>, _>(18),
    })
}

/// All qualifying completed-session summaries, lazily rebuilding any
/// missing or stale-version rows first so new installs converge on
/// first read without a migration.
pub async fn load_prospect_sessions(pool: &SqlitePool) -> Result<Vec<Value>, DbError> {
    let missing = sqlx::query(
        "SELECT s.id FROM tracking_sessions s \
         LEFT JOIN session_summaries ss ON ss.session_id = s.id \
         WHERE s.ended_at IS NOT NULL \
         AND EXISTS (SELECT 1 FROM skill_gains sg WHERE sg.session_id = s.id) \
         AND (ss.session_id IS NULL OR ss.summary_version < ?)",
    )
    .bind(SUMMARY_VERSION)
    .fetch_all(pool)
    .await?;
    for row in &missing {
        use sqlx::Row as _;
        write_session_summary(pool, row.get(0)).await?;
    }

    let rows = sqlx::query(
        "SELECT session_id, started_at, ended_at, duration_hours, kills, loot_tt, \
         weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, \
         cycled_ped, regular_skill_ped_json, attribute_levels_json, \
         regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, \
         dominant_weapon \
         FROM session_summaries",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_prospect_dict).collect())
}

pub async fn delete_session_summary(pool: &SqlitePool, session_id: &str) -> Result<(), DbError> {
    sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        let pool = db.pool().clone();
        (dir, pool)
    }

    async fn run(pool: &SqlitePool, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(pool)
            .await
            .unwrap();
    }

    /// One ended session: 2h duration, five kills (3 Young Atrox, 1
    /// Snable, 1 Unknown), Rifle-dominant tool stats, mixed gains.
    async fn seed_standard(pool: &SqlitePool) {
        run(
            pool,
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost, \
              mob_tracking_mode) \
             VALUES ('s1', 1000.0, 8200.0, 0, 0.07, 0.11, 0.13, 'mob')",
        )
        .await;
        for (kill, mob, species, maturity, loot, enhancer) in [
            ("k1", "Young Atrox", "Atrox", "Young", 2.0, 0.02),
            ("k2", "Young Atrox", "Atrox", "Young", 3.0, 0.02),
            ("k3", "Young Atrox", "Atrox", "Young", 4.0, 0.02),
            ("k4", "Snable", "Snable", "", 1.0, 0.02),
            ("k5", "Unknown", "", "", 0.5, 0.02),
        ] {
            run(
                pool,
                &format!(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                     timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
                     cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
                     VALUES ('{kill}', 's1', '{mob}', '{species}', '{maturity}', 1500.0, \
                     1, 1.0, 0.0, 0, 0.1, {enhancer}, {loot}, 0, 0)"
                ),
            )
            .await;
        }
        run(
            pool,
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES \
             ('k1', 'Rifle', 30, 300.0, 3, 0.05), ('k2', 'Pistol', 10, 50.0, 1, 0.01)",
        )
        .await;
        run(
            pool,
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('s1', 1100.0, 'Rifle', 1.0, 0.5), ('s1', 1200.0, 'Rifle', 1.0, 0.25), \
             ('s1', 1300.0, 'Anatomy', 1.0, 0.0), \
             ('s1', 1400.0, 'Agility', 0.25, NULL), ('s1', 1500.0, 'Agility', 0.5, NULL), \
             ('s1', 1600.0, 'Health', 0.0, NULL)",
        )
        .await;
    }

    #[tokio::test]
    async fn the_standard_session_computes_every_field() {
        let (_dir, pool) = pool().await;
        seed_standard(&pool).await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();

        assert_eq!(summary["id"], Value::from("s1"));
        assert_eq!(summary["startedAt"], Value::from(1000.0));
        assert_eq!(summary["endedAt"], Value::from(8200.0));
        assert_eq!(summary["durationHours"], Value::from(2.0));
        assert_eq!(summary["armourCost"], Value::from(0.07));
        assert_eq!(summary["healCost"], Value::from(0.11));
        assert_eq!(summary["danglingCost"], Value::from(0.13));
        // Rifle 30 @ 0.05 + Pistol 10 @ 0.01.
        assert_eq!(summary["weaponCost"], Value::from(1.6));
        assert_eq!(summary["enhancerCost"], Value::from(0.1));
        assert_eq!(summary["kills"], Value::from(5_i64));
        assert_eq!(summary["lootTt"], Value::from(10.5));
        // weapon 1.6 + enhancer 0.1 + armour 0.07 + heal 0.11 +
        // dangling 0.13.
        assert_eq!(summary["cycledPed"], Value::from(2.01));
        // Atrox 3 of 4 known kills (Unknown excluded), with species:
        // a dominant mob, not a tag.
        assert_eq!(summary["dominantMob"], Value::from("Young Atrox"));
        assert_eq!(summary["dominantTag"], Value::Null);
        // Rifle 30 of 40 shots.
        assert_eq!(summary["dominantWeapon"], Value::from("Rifle"));
        // ped_value sums; the zero-total Anatomy row stays out.
        assert_eq!(
            summary["regularSkillPed"],
            serde_json::json!({"Rifle": 0.75})
        );
        assert_eq!(summary["regularSkillTt"], Value::from(0.75));
        // Attribute rows key on SUM(amount); the zero-sum Health rows
        // stay out, like the zero regular.
        assert_eq!(
            summary["attributeLevels"],
            serde_json::json!({"Agility": 0.75})
        );
        assert_eq!(summary["attributeLevelsTotal"], Value::from(0.75));
    }

    #[tokio::test]
    async fn dominance_admits_the_exact_threshold_and_refuses_below() {
        let (_dir, pool) = pool().await;
        seed_standard(&pool).await;
        // Rebalance: 3 Atrox of 5 known = 0.6 exactly (admitted);
        // Rifle 30 of 50 shots = 0.6 exactly (admitted).
        run(
            &pool,
            "UPDATE kills SET mob_name = 'Feffoid', mob_species = 'Feffoid' WHERE id = 'k5'",
        )
        .await;
        run(
            &pool,
            "UPDATE kill_tool_stats SET shots_fired = 20 WHERE tool_name = 'Pistol'",
        )
        .await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(summary["dominantMob"], Value::from("Young Atrox"));
        assert_eq!(summary["dominantWeapon"], Value::from("Rifle"));

        // One more Feffoid: 3 of 6 known = 0.5 (refused); Pistol up
        // to 25 shots: 30 of 55 (refused).
        run(
            &pool,
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('k6', 's1', 'Feffoid', 'Feffoid', '', 1600.0, 1, 1.0, 0.0, 0, \
             0.1, 0.0, 1.0, 0, 0)",
        )
        .await;
        run(
            &pool,
            "UPDATE kill_tool_stats SET shots_fired = 25 WHERE tool_name = 'Pistol'",
        )
        .await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(summary["dominantMob"], Value::Null);
        assert_eq!(summary["dominantTag"], Value::Null);
        assert_eq!(summary["dominantWeapon"], Value::Null);
    }

    #[tokio::test]
    async fn bare_names_classify_as_tags_and_either_field_makes_a_mob() {
        let (_dir, pool) = pool().await;
        seed_standard(&pool).await;
        // Strip the dominant rows bare: a tag, not a mob.
        run(
            &pool,
            "UPDATE kills SET mob_species = '', mob_maturity = '' WHERE mob_species = 'Atrox'",
        )
        .await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(summary["dominantMob"], Value::Null);
        assert_eq!(summary["dominantTag"], Value::from("Young Atrox"));

        // Maturity alone is enough to classify as a mob.
        run(
            &pool,
            "UPDATE kills SET mob_maturity = 'Young' WHERE mob_name = 'Young Atrox'",
        )
        .await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(summary["dominantMob"], Value::from("Young Atrox"));
        assert_eq!(summary["dominantTag"], Value::Null);
    }

    #[tokio::test]
    async fn the_qualifying_filters_refuse_each_axis() {
        let (_dir, pool) = pool().await;
        seed_standard(&pool).await;

        // An active (un-ended) session never summarises.
        run(
            &pool,
            "UPDATE tracking_sessions SET ended_at = NULL WHERE id = 's1'",
        )
        .await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());

        // Zero duration refuses.
        run(
            &pool,
            "UPDATE tracking_sessions SET ended_at = 1000.0 WHERE id = 's1'",
        )
        .await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());
        run(
            &pool,
            "UPDATE tracking_sessions SET ended_at = 8200.0 WHERE id = 's1'",
        )
        .await;

        // No positive gain totals refuses, but EITHER axis alone
        // qualifies: attribute-only first, then regular-only.
        run(
            &pool,
            "UPDATE skill_gains SET ped_value = 0.0 WHERE skill_name = 'Rifle'",
        )
        .await;
        let summary = compute_session_summary(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(summary["regularSkillTt"], Value::from(0.0));
        assert_eq!(summary["attributeLevelsTotal"], Value::from(0.75));
        run(
            &pool,
            "DELETE FROM skill_gains WHERE skill_name = 'Agility'",
        )
        .await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());
        run(
            &pool,
            "UPDATE skill_gains SET ped_value = 0.5 WHERE skill_name = 'Rifle'",
        )
        .await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_some());

        // No skill-gain rows at all refuses; so does the table
        // being absent entirely (the original's tolerated case).
        run(&pool, "DELETE FROM skill_gains").await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());
        run(
            &pool,
            "ALTER TABLE skill_gains RENAME TO skill_gains_parked",
        )
        .await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());
        run(
            &pool,
            "ALTER TABLE skill_gains_parked RENAME TO skill_gains",
        )
        .await;
        run(
            &pool,
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('s1', 1100.0, 'Rifle', 1.0, 0.5)",
        )
        .await;

        // Zero cycled value refuses (no tool stats or session costs).
        run(&pool, "DELETE FROM kill_tool_stats").await;
        run(
            &pool,
            "UPDATE tracking_sessions SET armour_cost = 0, heal_cost = 0, dangling_cost = 0 \
             WHERE id = 's1'",
        )
        .await;
        run(&pool, "UPDATE kills SET enhancer_cost = 0").await;
        assert!(compute_session_summary(&pool, "s1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn write_upserts_clears_and_delete_removes() {
        let (_dir, pool) = pool().await;
        seed_standard(&pool).await;

        write_session_summary(&pool, "s1").await.unwrap();
        let row = sqlx::query(
            "SELECT summary_version, duration_hours, cycled_ped, dominant_mob, dominant_weapon, \
             regular_skill_ped_json FROM session_summaries WHERE session_id = 's1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>(0).unwrap(), SUMMARY_VERSION);
        assert_eq!(row.try_get::<f64, _>(1).unwrap(), 2.0);
        assert_eq!(row.try_get::<f64, _>(2).unwrap(), 2.01);
        assert_eq!(row.try_get::<String, _>(3).unwrap(), "Young Atrox");
        assert_eq!(row.try_get::<String, _>(4).unwrap(), "Rifle");
        assert_eq!(row.try_get::<String, _>(5).unwrap(), "{\"Rifle\": 0.75}");

        // A session that stops qualifying clears its stale row.
        run(
            &pool,
            "UPDATE tracking_sessions SET ended_at = 1000.0 WHERE id = 's1'",
        )
        .await;
        write_session_summary(&pool, "s1").await.unwrap();
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM session_summaries")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(count, 0);

        // Delete is explicit and idempotent.
        run(
            &pool,
            "UPDATE tracking_sessions SET ended_at = 8200.0 WHERE id = 's1'",
        )
        .await;
        write_session_summary(&pool, "s1").await.unwrap();
        delete_session_summary(&pool, "s1").await.unwrap();
        delete_session_summary(&pool, "s1").await.unwrap();
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM session_summaries")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(count, 0);
    }

    /// The prospect reader over a seeded summary table, mirroring the
    /// original's run: a missing summary rebuilds lazily, a
    /// stale-version row for a session that no longer qualifies (zero
    /// cycled PED) clears instead of rebuilding, sessions without
    /// gains or still active never enter, and a minimal current
    /// version row passes through with the falsy-JSON and null
    /// dominant legs intact. Every expected object is the original
    /// implementation's output over byte-identical seeds.
    #[tokio::test]
    async fn the_prospect_reader_matches_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        let pool = db.pool().clone();

        sqlx::query(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, dangling_cost) \
             VALUES ('sess-full', 1000.0, 4600.0, 0, 1.5, 0.25)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
             damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
             VALUES ('pk1', 'sess-full', 'Atrox Young', 1100.0, 10, 100.0, 5.0, 1, 0.3, 0.5, 12.75)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES ('pk1', 'LR-32', 40, 50.0, 0, 0.05)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (sid, ts, skill, amount, ped) in [
            ("sess-full", 1100.0, "Rifle", 1.2, Some(0.8)),
            ("sess-full", 1200.0, "Agility", 1.0, None),
            ("sess-stale", 5050.0, "Anatomy", 0.5, Some(0.1)),
            ("sess-open", 7050.0, "Rifle", 0.1, Some(0.05)),
        ] {
            sqlx::query(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(sid)
            .bind(ts)
            .bind(skill)
            .bind(amount)
            .bind(ped)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (sid, st, en, active) in [
            ("sess-stale", 5000.0, Some(5100.0), 0i64),
            ("sess-nogains", 6000.0, Some(6100.0), 0),
            ("sess-open", 7000.0, None, 1),
        ] {
            sqlx::query(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(sid)
            .bind(st)
            .bind(en)
            .bind(active)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
             duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
             dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
             regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, dominant_weapon) \
             VALUES ('sess-stale', 0, 1.0, 2.0, 0.1, 99, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, \
             '{}', '{}', 1.0, 1.0, 'OLD', 'OLD', 'OLD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
             duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
             dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
             regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, dominant_weapon) \
             VALUES ('sess-manual', ?, 0.0, 0.0, 0.0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, \
             '', '', 0.0, 0.0, NULL, NULL, NULL)",
        )
        .bind(SUMMARY_VERSION)
        .execute(&pool)
        .await
        .unwrap();

        let prospects = load_prospect_sessions(&pool).await.unwrap();
        assert_eq!(
            prospects,
            vec![
                json!({
                    "id": "sess-manual", "startedAt": 0.0, "endedAt": 0.0,
                    "durationHours": 0.0, "kills": 0, "lootTt": 0.0, "weaponCost": 0.0,
                    "enhancerCost": 0.0, "armourCost": 0.0, "healCost": 0.0,
                    "danglingCost": 0.0, "cycledPed": 0.0, "regularSkillPed": {},
                    "attributeLevels": {}, "regularSkillTt": 0.0,
                    "attributeLevelsTotal": 0.0, "dominantMob": null,
                    "dominantTag": null, "dominantWeapon": null,
                }),
                json!({
                    "id": "sess-full", "startedAt": 1000.0, "endedAt": 4600.0,
                    "durationHours": 1.0, "kills": 1, "lootTt": 12.75, "weaponCost": 2.0,
                    "enhancerCost": 0.5, "armourCost": 0.0, "healCost": 1.5,
                    "danglingCost": 0.25, "cycledPed": 4.25,
                    "regularSkillPed": {"Rifle": 0.8},
                    "attributeLevels": {"Agility": 1.0}, "regularSkillTt": 0.8,
                    "attributeLevelsTotal": 1.0, "dominantMob": null,
                    "dominantTag": "Atrox Young", "dominantWeapon": "LR-32",
                }),
            ]
        );

        // The disqualified stale row cleared rather than rebuilding.
        let rows: Vec<String> =
            sqlx::query("SELECT session_id FROM session_summaries ORDER BY session_id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|row| {
                    use sqlx::Row as _;
                    row.get(0)
                })
                .collect();
        assert_eq!(rows, ["sess-full", "sess-manual"]);
    }
}
