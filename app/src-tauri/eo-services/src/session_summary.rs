//! Materialised per-session summaries: a cache of derived state
//! whose source of truth is the tracking tables. Summaries write
//! eagerly when a session ends and clear when a session stops
//! qualifying; a stale or missing summary rebuilds lazily on read.
//! (The summary table sits outside the snapshot catalogue, so parity
//! here surfaces through the prospect reads rather than the goldens.)

use rusqlite::OptionalExtension as _;
use serde_json::{json, Map, Value};

use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::db::{Db, DbError};
use eo_wire::normalizer::{round_half_even, to_python_json};

/// Whether a rusqlite error is SQLite's "no such table" report: the
/// summary computation tolerates the `skill_gains` table being absent
/// entirely (an inherited tolerance the goldens exercise), returning no
/// summary rather than propagating.
fn is_missing_table(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("no such table")
    )
}

// Bumped to 2 when the Activity/session-list read columns (dominant kill
// counts, raw session skill-TT, primary mob/weapon lists, global/HOF counts)
// were added; to 3 when harvesting (tree cutting) joined the session economy
// (harvest loot inside lootTt, swing decay inside cycledPed, plus the four
// harvest columns); to 4 when the session facets replaced the exclusive
// tag-or-mob capture (session name and skill boost carried onto the summary,
// mob dominance computed over species-bearing kills only, the dominant-tag
// pair retired to NULL/0); to 5 when protection costs were re-attributed by
// hit count; to 6 when consumed doses joined cycledPed as their own bucket.
// A below-version row heals on the next read.
pub const SUMMARY_VERSION: i64 = 6;
pub const DOMINANCE_THRESHOLD: f64 = 0.6;

/// The computed summary for one completed session, or None when the
/// session is active or fails the economic filters (zero cycled value,
/// zero duration). A session that gained no skill still summarises.
#[allow(clippy::too_many_lines)]
pub fn compute_session_summary(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<Map<String, Value>>, DbError> {
    let session = conn
        .query_row(
            "SELECT started_at, ended_at, \
             COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0), \
             session_name, skill_boost_percent, COALESCE(consumable_cost, 0) \
             FROM tracking_sessions WHERE id = ? AND ended_at IS NOT NULL",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, f64>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        started_at,
        ended_at,
        armour_cost,
        heal_cost,
        dangling_cost,
        session_name,
        skill_boost_percent,
        consumable_cost,
    )) = session
    else {
        return Ok(None);
    };

    // A session that gained no skill is still a real session: a couple
    // of kills that happened not to tick a skill still cycled PED and
    // still belong in every total. So the gains probe no longer gates
    // the summary; only the economic filters below (zero cycled value,
    // zero duration) exclude a row.
    //
    // The gains table being absent entirely stays tolerated as the
    // no-summary case, since every skill read below would fail on it.
    // Any other failure propagates: a transient driver error must not
    // read as "no gains" and let the write path clear a valid summary.
    let has_gains = conn
        .query_row(
            "SELECT 1 FROM skill_gains WHERE session_id = ? LIMIT 1",
            rusqlite::params![session_id],
            |_| Ok(()),
        )
        .optional();
    match has_gains {
        Ok(_) => {}
        Err(error) if is_missing_table(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let (kills, kill_loot_tt, enhancer_cost) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        },
    )?;

    // Harvesting (tree cutting) swings: wood TT is liquid loot and
    // swing decay is cycled spend, so both join the session economy
    // (`lootTt` / `cycledPed`) beside their own explicit columns.
    let (harvest_swings, harvest_successes, harvest_loot_tt, harvest_cost) = conn.query_row(
        "SELECT COUNT(*), \
           COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
           COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(cost_ped), 0) \
         FROM harvest_events WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        },
    )?;
    let loot_tt = kill_loot_tt + harvest_loot_tt;

    let weapon_cost: f64 = conn.query_row(
        "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
         FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ?",
        rusqlite::params![session_id],
        |row| row.get::<_, f64>(0),
    )?;

    // Mob dominance is computed over species-bearing stamps only: a legacy
    // tag-mode kill carries the tag in mob_name with an empty species, and
    // that is a session name, not a mob (migration 0018 lifted it onto the
    // session row). The dominant-tag pair retired with the exclusive capture
    // model and stays NULL/0 for the columns' sake.
    let mob_rows: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT mob_name, COUNT(*) \
             FROM kills \
             WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
               AND COALESCE(mob_species, '') != '' \
             GROUP BY mob_name \
             ORDER BY COUNT(*) DESC, mob_name ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut dominant_mob: Option<String> = None;
    // The dominant's own kill count (Activity sums these across sessions).
    let mut dominant_mob_kills: i64 = 0;
    if !mob_rows.is_empty() {
        let total_known: i64 = mob_rows.iter().map(|row| row.1).sum();
        if total_known > 0 {
            let (top_name, top_count) = mob_rows[0].clone();
            if top_count as f64 / total_known as f64 >= DOMINANCE_THRESHOLD {
                dominant_mob = Some(top_name);
                dominant_mob_kills = top_count;
            }
        }
    }

    let tool_rows: Vec<(String, f64)> = {
        let mut stmt = conn.prepare(
            "SELECT ts.tool_name, COALESCE(SUM(ts.shots_fired), 0) \
             FROM kill_tool_stats ts \
             JOIN kills k ON k.id = ts.kill_id \
             WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
             GROUP BY ts.tool_name \
             ORDER BY SUM(ts.shots_fired) DESC, ts.tool_name ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut dominant_weapon: Option<String> = None;
    if !tool_rows.is_empty() {
        let total_shots: f64 = tool_rows.iter().map(|row| row.1).sum();
        let (top_name, top_shots) = tool_rows[0].clone();
        if total_shots > 0.0 && top_shots / total_shots >= DOMINANCE_THRESHOLD {
            dominant_weapon = Some(top_name);
        }
    }

    // The session-list "primary" top-three lists: ungated (unlike the dominant
    // fields), by kill count for mobs and by total shots for weapons. Same SQL
    // the list read runs, so the stored order matches it row for row.
    let primary_mobs: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT mob_name FROM kills \
             WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
               AND COALESCE(mob_species, '') != '' \
             GROUP BY mob_name ORDER BY COUNT(*) DESC LIMIT 3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let primary_weapons: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT ts.tool_name FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
             WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
             GROUP BY ts.tool_name ORDER BY SUM(ts.shots_fired) DESC LIMIT 3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Activity's raw session skill-TT: SUM(ped_value) over the session (not the
    // per-skill, positive-only regular_skill_tt), so the Activity read reproduces
    // its pesPer100Ped exactly.
    let activity_skill_tt: f64 = conn.query_row(
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains \
         WHERE session_id = ? AND ped_value IS NOT NULL",
        rusqlite::params![session_id],
        |row| row.get::<_, f64>(0),
    )?;

    // The session's global / HOF counts, from the notable-events prefixes the
    // session-list read counts.
    let (globals, hofs) = conn.query_row(
        "SELECT \
           COALESCE(SUM(CASE WHEN event_type LIKE 'global_%' THEN 1 ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN event_type LIKE 'hof_%' THEN 1 ELSE 0 END), 0) \
         FROM notable_events WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;

    let regular_rows: Vec<(String, f64)> = {
        let mut stmt = conn.prepare(
            "SELECT skill_name, COALESCE(SUM(ped_value), 0) \
             FROM skill_gains \
             WHERE session_id = ? AND ped_value IS NOT NULL \
             GROUP BY skill_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut regular_skill_ped = Map::new();
    for (name, total) in regular_rows {
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
    let attr_rows: Vec<(String, f64)> = {
        let mut params: Vec<&str> = Vec::with_capacity(1 + ATTRIBUTE_SKILLS.len());
        params.push(session_id);
        params.extend(ATTRIBUTE_SKILLS);
        let mut stmt = conn.prepare(&attr_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut attribute_levels = Map::new();
    for (name, total) in attr_rows {
        if total > 0.0 {
            attribute_levels.insert(name, Value::from(total));
        }
    }

    let duration_hours = ((ended_at - started_at) / 3600.0).max(0.0);
    let cycled_ped = weapon_cost
        + enhancer_cost
        + armour_cost
        + heal_cost
        + consumable_cost
        + dangling_cost
        + harvest_cost;
    let regular_skill_tt: f64 = regular_skill_ped.values().filter_map(Value::as_f64).sum();
    let attribute_levels_total: f64 = attribute_levels.values().filter_map(Value::as_f64).sum();

    if cycled_ped <= 0.0 || duration_hours <= 0.0 {
        return Ok(None);
    }

    let mut summary = Map::new();
    summary.insert("id".into(), Value::from(session_id));
    summary.insert("startedAt".into(), Value::from(started_at));
    summary.insert("endedAt".into(), Value::from(ended_at));
    summary.insert("durationHours".into(), Value::from(duration_hours));
    summary.insert("armourCost".into(), Value::from(armour_cost));
    summary.insert("healCost".into(), Value::from(heal_cost));
    summary.insert("consumableCost".into(), Value::from(consumable_cost));
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
    // Retired with the exclusive capture model (SUMMARY_VERSION 4): the
    // designated axis is the session name below.
    summary.insert("dominantTag".into(), Value::Null);
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
    // Activity/session-list read columns (SUMMARY_VERSION 2).
    summary.insert("dominantMobKills".into(), Value::from(dominant_mob_kills));
    summary.insert("dominantTagKills".into(), Value::from(0));
    summary.insert("activitySkillTt".into(), Value::from(activity_skill_tt));
    summary.insert(
        "primaryMobs".into(),
        Value::Array(primary_mobs.into_iter().map(Value::from).collect()),
    );
    summary.insert(
        "primaryWeapons".into(),
        Value::Array(primary_weapons.into_iter().map(Value::from).collect()),
    );
    summary.insert("globals".into(), Value::from(globals));
    summary.insert("hofs".into(), Value::from(hofs));
    // Harvesting columns (SUMMARY_VERSION 3).
    summary.insert("harvestSwings".into(), Value::from(harvest_swings));
    summary.insert("harvestSuccesses".into(), Value::from(harvest_successes));
    summary.insert(
        "harvestLootTt".into(),
        Value::from(round_half_even(harvest_loot_tt, 4)),
    );
    summary.insert(
        "harvestCost".into(),
        Value::from(round_half_even(harvest_cost, 4)),
    );
    // Session facets (SUMMARY_VERSION 4), copied from the session row.
    summary.insert(
        "sessionName".into(),
        session_name
            .filter(|name| !name.is_empty())
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    summary.insert(
        "skillBoostPercent".into(),
        skill_boost_percent
            .filter(|percent| *percent > 0)
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    Ok(Some(summary))
}

/// Compute and upsert the summary row; clears any stale row when the
/// session does not qualify. The caller owns the surrounding commit
/// semantics, exactly as the original documents: the session-stop and
/// orphan-recovery paths run this inside their write transaction, so
/// the computation reads the not-yet-committed session end stamp.
pub fn write_session_summary(conn: &rusqlite::Connection, session_id: &str) -> Result<(), DbError> {
    let Some(summary) = compute_session_summary(conn, session_id)? else {
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?",
            rusqlite::params![session_id],
        )?;
        return Ok(());
    };
    // The two primary lists persist as compact JSON arrays; the read paths
    // parse them straight back into the same string vectors.
    let primary_mobs_json =
        serde_json::to_string(&summary["primaryMobs"]).expect("primaryMobs serialises");
    let primary_weapons_json =
        serde_json::to_string(&summary["primaryWeapons"]).expect("primaryWeapons serialises");
    conn.execute(
        "INSERT OR REPLACE INTO session_summaries (\
         session_id, summary_version, started_at, ended_at, duration_hours, \
         kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
         dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
         regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, \
         dominant_weapon, dominant_mob_kills, dominant_tag_kills, activity_skill_tt, \
         primary_mobs_json, primary_weapons_json, globals, hofs, \
         harvest_swings, harvest_successes, harvest_loot_tt, harvest_cost, \
         session_name, skill_boost_percent, consumable_cost, computed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
         ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch('now'))",
        rusqlite::params![
            summary["id"].as_str(),
            SUMMARY_VERSION,
            summary["startedAt"].as_f64(),
            summary["endedAt"].as_f64(),
            summary["durationHours"].as_f64(),
            summary["kills"].as_i64(),
            summary["lootTt"].as_f64(),
            summary["weaponCost"].as_f64(),
            summary["enhancerCost"].as_f64(),
            summary["armourCost"].as_f64(),
            summary["healCost"].as_f64(),
            summary["danglingCost"].as_f64(),
            summary["cycledPed"].as_f64(),
            to_python_json(&summary["regularSkillPed"], None),
            to_python_json(&summary["attributeLevels"], None),
            summary["regularSkillTt"].as_f64(),
            summary["attributeLevelsTotal"].as_f64(),
            summary["dominantMob"].as_str(),
            summary["dominantTag"].as_str(),
            summary["dominantWeapon"].as_str(),
            summary["dominantMobKills"].as_i64(),
            summary["dominantTagKills"].as_i64(),
            summary["activitySkillTt"].as_f64(),
            primary_mobs_json,
            primary_weapons_json,
            summary["globals"].as_i64(),
            summary["hofs"].as_i64(),
            summary["harvestSwings"].as_i64(),
            summary["harvestSuccesses"].as_i64(),
            summary["harvestLootTt"].as_f64(),
            summary["harvestCost"].as_f64(),
            summary["sessionName"].as_str(),
            summary["skillBoostPercent"].as_i64(),
            summary["consumableCost"].as_f64(),
        ],
    )?;
    Ok(())
}

/// Remove a session's summary row; idempotent.
/// One stored summary row to its camelCase prospect shape (the
/// original's column order and coercions: or-zero floats, an integer
/// kill count, JSON columns parsed when non-empty, and the dominant
/// fields passed through raw).
fn row_to_prospect_dict(row: &rusqlite::Row) -> Value {
    let float_or_zero = |index: usize| -> f64 {
        row.get::<_, Option<f64>>(index)
            .ok()
            .flatten()
            .unwrap_or(0.0)
    };
    let json_or_empty = |index: usize| -> Value {
        row.get::<_, Option<String>>(index)
            .ok()
            .flatten()
            .filter(|text| !text.is_empty())
            .map(|text| serde_json::from_str(&text).expect("stored summary JSON parses"))
            .unwrap_or_else(|| json!({}))
    };
    json!({
        "id": row.get_unwrap::<_, String>(0),
        "startedAt": float_or_zero(1),
        "endedAt": float_or_zero(2),
        "durationHours": float_or_zero(3),
        "kills": row.get::<_, Option<i64>>(4).ok().flatten().unwrap_or(0),
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
        "dominantMob": row.get_unwrap::<_, Option<String>>(16),
        "dominantTag": row.get_unwrap::<_, Option<String>>(17),
        "dominantWeapon": row.get_unwrap::<_, Option<String>>(18),
    })
}

/// Rebuild every missing or stale-version summary row, so a read taken after a
/// `SUMMARY_VERSION` bump (or on a fresh install) sees current rows without a
/// data migration. Shared by every summary reader (the prospect surface and the
/// Activity / session-list reads); once the rows converge it finds nothing and
/// is cheap.
pub fn heal_summaries(conn: &rusqlite::Connection) -> Result<(), DbError> {
    let missing: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT s.id FROM tracking_sessions s \
             LEFT JOIN session_summaries ss ON ss.session_id = s.id \
             WHERE s.ended_at IS NOT NULL \
             AND (ss.summary_version < ?1 OR (ss.session_id IS NULL \
               AND s.ended_at > s.started_at \
               AND (COALESCE(s.armour_cost, 0) > 0 \
                 OR COALESCE(s.heal_cost, 0) > 0 \
                 OR COALESCE(s.consumable_cost, 0) > 0 \
                 OR COALESCE(s.dangling_cost, 0) > 0 \
                 OR EXISTS (SELECT 1 FROM kills k \
                            WHERE k.session_id = s.id AND k.enhancer_cost > 0) \
                 OR EXISTS (SELECT 1 FROM kills k JOIN kill_tool_stats ts ON ts.kill_id = k.id \
                            WHERE k.session_id = s.id \
                              AND COALESCE(ts.cost_per_shot, 0) \
                                  * COALESCE(ts.shots_fired, 0) > 0) \
                 OR EXISTS (SELECT 1 FROM harvest_events h \
                            WHERE h.session_id = s.id AND h.cost_ped > 0))))",
        )?;
        let rows = stmt.query_map(rusqlite::params![SUMMARY_VERSION], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in &missing {
        write_session_summary(conn, id)?;
    }
    Ok(())
}

/// All qualifying completed-session summaries, lazily rebuilding any
/// missing or stale-version rows first so new installs converge on
/// first read without a migration. A summarised session may carry no
/// skill gains at all, so consumers reading the skill maps must
/// tolerate them being empty.
pub async fn load_prospect_sessions(db: &Db) -> Result<Vec<Value>, DbError> {
    // Heal (a write) on the writer core; read the prospect rows in one
    // synchronous pass on a reader-core connection.
    db.with_writer(|conn| heal_summaries(conn)).await?;
    db.with_reader(|conn| {
        let mut stmt = conn.prepare(
            "SELECT session_id, started_at, ended_at, duration_hours, kills, loot_tt, \
             weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, \
             cycled_ped, regular_skill_ped_json, attribute_levels_json, \
             regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, \
             dominant_weapon \
             FROM session_summaries",
        )?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_prospect_dict(row)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
}

pub fn delete_session_summary(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM session_summaries WHERE session_id = ?",
        rusqlite::params![session_id],
    )?;
    Ok(())
}

/// Drop and regenerate every session-summary row from the raw tracking
/// tables: the proof the summaries are a pure function of those tables, and
/// the maintenance reset behind the rebuild command. [`heal_summaries`]
/// rewrites exactly the qualifying (ended, economically non-empty)
/// sessions, so a full delete followed by a heal reproduces the
/// incrementally-maintained set. Runs on the writer.
pub fn rebuild_summaries(conn: &rusqlite::Connection) -> Result<(), DbError> {
    conn.execute("DELETE FROM session_summaries", [])?;
    heal_summaries(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn env() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Compute one session's summary on the synchronous core (reader path).
    async fn compute(db: &Db, session_id: &str) -> Option<Map<String, Value>> {
        let session_id = session_id.to_string();
        db.with_writer(move |conn| compute_session_summary(conn, &session_id))
            .await
            .unwrap()
    }

    /// Write one session's summary on the synchronous core's writer.
    async fn write_summary(db: &Db, session_id: &str) {
        let session_id = session_id.to_string();
        db.with_writer(move |conn| write_session_summary(conn, &session_id))
            .await
            .unwrap();
    }

    /// Delete one session's summary on the synchronous core's writer.
    async fn delete_summary(db: &Db, session_id: &str) {
        let session_id = session_id.to_string();
        db.with_writer(move |conn| delete_session_summary(conn, &session_id))
            .await
            .unwrap();
    }

    async fn run(db: &Db, sql: &str) {
        let sql = sql.to_string();
        db.with_writer(move |conn| {
            conn.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    }

    /// One ended session: 2h duration, five kills (3 Young Atrox, 1
    /// Snable, 1 Unknown), Rifle-dominant tool stats, mixed gains.
    async fn seed_standard(db: &Db) {
        run(
            db,
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
                db,
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
            db,
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES \
             ('k1', 'Rifle', 30, 300.0, 3, 0.05), ('k2', 'Pistol', 10, 50.0, 1, 0.01)",
        )
        .await;
        run(
            db,
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
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        let summary = compute(&db, "s1").await.unwrap();

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
        // Read columns (SUMMARY_VERSION 2). Atrox is the dominant mob with its
        // 3 kills; no dominant tag.
        assert_eq!(summary["dominantMobKills"], Value::from(3_i64));
        assert_eq!(summary["dominantTagKills"], Value::from(0_i64));
        // Raw SUM(ped_value): Rifle 0.5 + 0.25 + Anatomy 0.0 (NULL Agility/Health
        // excluded).
        assert_eq!(summary["activitySkillTt"], Value::from(0.75));
        // Primary lists: mobs by kill count, weapons by total shots (Unknown
        // excluded).
        assert_eq!(
            summary["primaryMobs"],
            serde_json::json!(["Young Atrox", "Snable"])
        );
        assert_eq!(
            summary["primaryWeapons"],
            serde_json::json!(["Rifle", "Pistol"])
        );
        // No notable events seeded.
        assert_eq!(summary["globals"], Value::from(0_i64));
        assert_eq!(summary["hofs"], Value::from(0_i64));
    }

    #[tokio::test]
    async fn harvest_swings_join_loot_and_cycled_spend() {
        let (_dir, db) = env().await;
        run(
            &db,
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost, \
              mob_tracking_mode) \
             VALUES ('s1', 1000.0, 8200.0, 0, 0.0, 0.0, 0.0, 'mob')",
        )
        .await;
        run(
            &db,
            "INSERT INTO kills (id, session_id, mob_name, timestamp, enhancer_cost, loot_total_ped) \
             VALUES ('k1', 's1', 'Atrox', 1500.0, 0.0, 5.0)",
        )
        .await;
        run(
            &db,
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES ('k1', 'Rifle', 10, 0.5)",
        )
        .await;
        run(
            &db,
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('s1', 1100.0, 'Rifle', 1.0, 0.5)",
        )
        .await;
        // Two swings: one successful with 3.0 wood TT, one failed; 2.0 decay.
        run(
            &db,
            "INSERT INTO harvest_events \
             (id, session_id, timestamp, success, cost_ped, loot_total_ped) \
             VALUES ('h1', 's1', 1700.0, 1, 1.0, 3.0), \
                    ('h2', 's1', 1800.0, 0, 1.0, 0.0)",
        )
        .await;

        let summary = compute(&db, "s1").await.unwrap();

        // Wood TT joins kill loot in lootTt: 5.0 kill + 3.0 harvest.
        assert_eq!(summary["lootTt"], Value::from(8.0));
        // Swing decay joins weapon cost in cycledPed: 5.0 weapon + 2.0 harvest
        // (armour/heal/dangling/enhancer all zero).
        assert_eq!(summary["cycledPed"], Value::from(7.0));
        // The per-activity harvest columns carry the raw swing tallies.
        assert_eq!(summary["harvestSwings"], Value::from(2_i64));
        assert_eq!(summary["harvestSuccesses"], Value::from(1_i64));
        assert_eq!(summary["harvestLootTt"], Value::from(3.0));
        assert_eq!(summary["harvestCost"], Value::from(2.0));
    }

    #[tokio::test]
    async fn dominance_admits_the_exact_threshold_and_refuses_below() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        // Rebalance: 3 Atrox of 5 known = 0.6 exactly (admitted);
        // Rifle 30 of 50 shots = 0.6 exactly (admitted).
        run(
            &db,
            "UPDATE kills SET mob_name = 'Feffoid', mob_species = 'Feffoid' WHERE id = 'k5'",
        )
        .await;
        run(
            &db,
            "UPDATE kill_tool_stats SET shots_fired = 20 WHERE tool_name = 'Pistol'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["dominantMob"], Value::from("Young Atrox"));
        assert_eq!(summary["dominantWeapon"], Value::from("Rifle"));

        // One more Feffoid: 3 of 6 known = 0.5 (refused); Pistol up
        // to 25 shots: 30 of 55 (refused).
        run(
            &db,
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('k6', 's1', 'Feffoid', 'Feffoid', '', 1600.0, 1, 1.0, 0.0, 0, \
             0.1, 0.0, 1.0, 0, 0)",
        )
        .await;
        run(
            &db,
            "UPDATE kill_tool_stats SET shots_fired = 25 WHERE tool_name = 'Pistol'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["dominantMob"], Value::Null);
        assert_eq!(summary["dominantTag"], Value::Null);
        assert_eq!(summary["dominantWeapon"], Value::Null);
    }

    #[tokio::test]
    async fn species_presence_decides_the_mob_axis_and_the_name_facet_rides_along() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        // Strip the dominant rows bare. A species-less stamp is a legacy
        // tag-mode kill, which migration 0018 lifted onto the session row
        // as its name, so it must not re-enter the mob axis here.
        run(
            &db,
            "UPDATE kills SET mob_species = '', mob_maturity = '' WHERE mob_species = 'Atrox'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        // The stripped rows leave both the numerator and the denominator,
        // so the one remaining species-bearing kill dominates outright
        // rather than the tag out-counting it.
        assert_eq!(summary["dominantMob"], Value::from("Snable"));
        assert_eq!(summary["dominantMobKills"], Value::from(1));
        // The retired dominant-tag pair stays empty rather than being
        // repurposed.
        assert_eq!(summary["dominantTag"], Value::Null);
        assert_eq!(summary["dominantTagKills"], Value::from(0));
        // Nor do the stripped rows reach the session list's primary mobs.
        assert_eq!(summary["primaryMobs"], json!(["Snable"]));

        // Maturity without a species is not an identity either: species is
        // the stable axis.
        run(
            &db,
            "UPDATE kills SET mob_maturity = 'Young' WHERE mob_name = 'Young Atrox'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["dominantMob"], Value::from("Snable"));

        // Restoring the species restores the mob axis: 3 of 4 known kills.
        run(
            &db,
            "UPDATE kills SET mob_species = 'Atrox' WHERE mob_name = 'Young Atrox'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["dominantMob"], Value::from("Young Atrox"));
    }

    #[tokio::test]
    async fn the_session_facets_ride_onto_the_summary() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        // Undeclared facets record as null, never as a guessed default.
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["sessionName"], Value::Null);
        assert_eq!(summary["skillBoostPercent"], Value::Null);

        run(
            &db,
            "UPDATE tracking_sessions SET session_name = 'ARIS Dailies', \
             skill_boost_percent = 50 WHERE id = 's1'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["sessionName"], Value::from("ARIS Dailies"));
        assert_eq!(summary["skillBoostPercent"], Value::from(50));

        // "No boost" is NULL, never 0: the schema refuses the ambiguous
        // encoding outright, so a zero can never reach the summary.
        let refused = db
            .with_writer(|conn| {
                Ok(conn.execute(
                    "UPDATE tracking_sessions SET skill_boost_percent = 0 WHERE id = 's1'",
                    [],
                )?)
            })
            .await;
        assert!(refused.is_err(), "a zero boost violates the column check");

        run(
            &db,
            "UPDATE tracking_sessions SET skill_boost_percent = NULL, session_name = NULL \
             WHERE id = 's1'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["skillBoostPercent"], Value::Null);
        assert_eq!(summary["sessionName"], Value::Null);
    }

    #[tokio::test]
    async fn the_qualifying_filters_refuse_the_economic_axes_but_never_skill() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;

        // An active (un-ended) session never summarises.
        run(
            &db,
            "UPDATE tracking_sessions SET ended_at = NULL WHERE id = 's1'",
        )
        .await;
        assert!(compute(&db, "s1").await.is_none());

        // Zero duration refuses.
        run(
            &db,
            "UPDATE tracking_sessions SET ended_at = 1000.0 WHERE id = 's1'",
        )
        .await;
        assert!(compute(&db, "s1").await.is_none());
        run(
            &db,
            "UPDATE tracking_sessions SET ended_at = 8200.0 WHERE id = 's1'",
        )
        .await;

        // Skill gain does NOT gate the summary: a session that cycled
        // PED still summarises on either axis alone, on neither, and
        // with no gain rows at all. A couple of kills that happened not
        // to tick a skill are still a real session whose costs belong
        // in every total.
        run(
            &db,
            "UPDATE skill_gains SET ped_value = 0.0 WHERE skill_name = 'Rifle'",
        )
        .await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["regularSkillTt"], Value::from(0.0));
        assert_eq!(summary["attributeLevelsTotal"], Value::from(0.75));
        run(&db, "DELETE FROM skill_gains WHERE skill_name = 'Agility'").await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["regularSkillTt"], Value::from(0.0));
        assert_eq!(summary["attributeLevelsTotal"], Value::from(0.0));
        run(
            &db,
            "UPDATE skill_gains SET ped_value = 0.5 WHERE skill_name = 'Rifle'",
        )
        .await;
        assert!(compute(&db, "s1").await.is_some());

        // No skill-gain rows at all still summarises; the table being
        // absent entirely stays the tolerated no-summary case, since
        // every skill read would fail on it.
        run(&db, "DELETE FROM skill_gains").await;
        let summary = compute(&db, "s1").await.unwrap();
        assert_eq!(summary["regularSkillTt"], Value::from(0.0));
        run(&db, "ALTER TABLE skill_gains RENAME TO skill_gains_parked").await;
        assert!(compute(&db, "s1").await.is_none());
        run(&db, "ALTER TABLE skill_gains_parked RENAME TO skill_gains").await;
        run(
            &db,
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('s1', 1100.0, 'Rifle', 1.0, 0.5)",
        )
        .await;

        // Zero cycled value refuses (no tool stats or session costs).
        run(&db, "DELETE FROM kill_tool_stats").await;
        run(
            &db,
            "UPDATE tracking_sessions SET armour_cost = 0, heal_cost = 0, dangling_cost = 0 \
             WHERE id = 's1'",
        )
        .await;
        run(&db, "UPDATE kills SET enhancer_cost = 0").await;
        assert!(compute(&db, "s1").await.is_none());
    }

    #[tokio::test]
    async fn write_upserts_clears_and_delete_removes() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;

        write_summary(&db, "s1").await;
        let (
            summary_version,
            duration_hours,
            cycled_ped,
            dominant_mob,
            dominant_weapon,
            regular_skill_ped_json,
        ): (i64, f64, f64, String, String, String) = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT summary_version, duration_hours, cycled_ped, dominant_mob, \
                     dominant_weapon, regular_skill_ped_json FROM session_summaries \
                     WHERE session_id = 's1'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, f64>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(summary_version, SUMMARY_VERSION);
        assert_eq!(duration_hours, 2.0);
        assert_eq!(cycled_ped, 2.01);
        assert_eq!(dominant_mob, "Young Atrox");
        assert_eq!(dominant_weapon, "Rifle");
        assert_eq!(regular_skill_ped_json, "{\"Rifle\": 0.75}");

        // A session that stops qualifying clears its stale row.
        run(
            &db,
            "UPDATE tracking_sessions SET ended_at = 1000.0 WHERE id = 's1'",
        )
        .await;
        write_summary(&db, "s1").await;
        let count: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM session_summaries", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 0);

        // Delete is explicit and idempotent.
        run(
            &db,
            "UPDATE tracking_sessions SET ended_at = 8200.0 WHERE id = 's1'",
        )
        .await;
        write_summary(&db, "s1").await;
        delete_summary(&db, "s1").await;
        delete_summary(&db, "s1").await;
        let count: i64 = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM session_summaries", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
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

        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, dangling_cost) \
                 VALUES ('sess-full', 1000.0, 4600.0, 0, 1.5, 0.25)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
                 damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
                 VALUES ('pk1', 'sess-full', 'Atrox Young', 1100.0, 10, 100.0, 5.0, 1, 0.3, 0.5, 12.75)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
                 critical_hits, cost_per_shot) VALUES ('pk1', 'LR-32', 40, 50.0, 0, 0.05)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        for (sid, ts, skill, amount, ped) in [
            ("sess-full", 1100.0, "Rifle", 1.2, Some(0.8)),
            ("sess-full", 1200.0, "Agility", 1.0, None),
            ("sess-stale", 5050.0, "Anatomy", 0.5, Some(0.1)),
            ("sess-open", 7050.0, "Rifle", 0.1, Some(0.05)),
        ] {
            let sid = sid.to_string();
            let skill = skill.to_string();
            db.with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![sid, ts, skill, amount, ped],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }
        for (sid, st, en, active) in [
            ("sess-stale", 5000.0, Some(5100.0), 0i64),
            ("sess-nogains", 6000.0, Some(6100.0), 0),
            ("sess-open", 7000.0, None, 1),
        ] {
            let sid = sid.to_string();
            db.with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![sid, st, en, active],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
                 duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
                 dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
                 regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, dominant_weapon) \
                 VALUES ('sess-stale', 0, 1.0, 2.0, 0.1, 99, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, \
                 '{}', '{}', 1.0, 1.0, 'OLD', 'OLD', 'OLD')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
                 duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
                 dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
                 regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, dominant_weapon) \
                 VALUES ('sess-manual', ?1, 0.0, 0.0, 0.0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, \
                 '', '', 0.0, 0.0, NULL, NULL, NULL)",
                rusqlite::params![SUMMARY_VERSION],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let prospects = load_prospect_sessions(&db).await.unwrap();
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
                    // The seeded kill carries no species, so it reaches
                    // neither axis here; its designated identity lives on
                    // the session row as the name facet.
                    "dominantTag": null, "dominantWeapon": "LR-32",
                }),
            ]
        );

        // The disqualified stale row cleared rather than rebuilding.
        let rows: Vec<String> = db
            .with_reader(|conn| {
                let mut stmt =
                    conn.prepare("SELECT session_id FROM session_summaries ORDER BY session_id")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(rows, ["sess-full", "sess-manual"]);
    }

    /// A session that cycled PED but ticked no skill is a real session:
    /// a couple of kills that happened not to gain skill still cost
    /// what they cost. The heal path must pick it up unprompted, so it
    /// reaches every total that reads the summaries.
    #[tokio::test]
    async fn a_session_that_gained_no_skill_still_summarises_and_heals_in() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        run(&db, "DELETE FROM skill_gains").await;
        // Nothing has written a summary yet; the heal path is the only
        // thing that can produce one.
        db.with_writer(|conn| {
            conn.execute("DELETE FROM session_summaries", [])?;
            heal_summaries(conn)
        })
        .await
        .unwrap();

        let (present, cycled, kills): (i64, f64, i64) = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*), COALESCE(MAX(cycled_ped), 0), COALESCE(MAX(kills), 0) \
                     FROM session_summaries WHERE session_id = 's1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(present, 1, "the zero-skill session must be summarised");
        assert!(cycled > 0.0, "its cycled spend must survive into the row");
        assert_eq!(kills, 5);
    }

    #[test]
    fn is_missing_table_matches_only_the_no_such_table_report() {
        let missing = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("no such table: skill_gains".to_string()),
        );
        assert!(is_missing_table(&missing));
        // A different SQLite failure is not tolerated.
        let other = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("no such column: session_id".to_string()),
        );
        assert!(!is_missing_table(&other));
        assert!(!is_missing_table(&rusqlite::Error::QueryReturnedNoRows));
    }

    /// A gains-probe failure that is NOT the tolerated missing-table case must
    /// propagate rather than reading as "no gains" and clearing a valid summary.
    #[tokio::test]
    async fn a_non_missing_table_gains_error_propagates() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        // Recreate skill_gains without session_id so the gains probe raises
        // "no such column" (present table, missing column), not a missing table.
        run(&db, "DROP TABLE skill_gains").await;
        run(&db, "CREATE TABLE skill_gains (id INTEGER)").await;
        let result = db
            .with_writer(|conn| Ok(compute_session_summary(conn, "s1")))
            .await
            .unwrap();
        assert!(result.is_err());
    }

    /// Rebuild drops every summary row and regenerates exactly the qualifying
    /// (ended, skill-bearing) sessions, so an orphan row clears and a missing
    /// one reappears.
    #[tokio::test]
    async fn rebuild_clears_orphans_and_regenerates_qualifying_rows() {
        let (_dir, db) = env().await;
        seed_standard(&db).await;
        // An orphan summary row backing no session at all.
        run(
            &db,
            "INSERT INTO session_summaries \
             (session_id, summary_version, started_at, ended_at, duration_hours, kills, loot_tt, \
              weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, cycled_ped, \
              regular_skill_ped_json, attribute_levels_json, regular_skill_tt, attribute_levels_total) \
             VALUES ('ghost', 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}', '{}', 0, 0)",
        )
        .await;

        db.with_writer(|conn| rebuild_summaries(conn))
            .await
            .unwrap();

        let ids: Vec<String> = db
            .with_reader(|conn| {
                let mut stmt =
                    conn.prepare("SELECT session_id FROM session_summaries ORDER BY session_id")?;
                let out = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(out)
            })
            .await
            .unwrap();
        // The orphan is gone; the qualifying s1 was regenerated.
        assert_eq!(ids, ["s1"]);
    }
}
