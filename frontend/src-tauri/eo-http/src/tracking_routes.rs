//! Native tracking session-read surface:
//! the `/api/tracking/sessions`, `/api/tracking/session/{id}`, and
//! `/api/tracking/tag-suggestions` GETs.
//!
//! These three reads are router-resident SQL aggregation over the shared
//! database (and, for the session detail, the skill-calibration table) plus
//! the injected clock for an active session's running duration. The port
//! mirrors the reference query for query and shapes the result to the
//! `TrackingSession` / `SessionDetail` / `list[str]` response models
//! byte-for-byte, under the `/api/tracking` ETag middleware's conditional-GET
//! contract.
//!
//! The fidelity cruxes:
//! - `_ts_to_iso`: `datetime.fromtimestamp(ts, tz=UTC).isoformat()`, which
//!   emits `+00:00` and 6-digit microseconds only when the fraction is
//!   non-zero. `ts_to_iso` reproduces that exactly, splitting the fraction
//!   out (CPython's `modf`) before rounding so it does not inherit the
//!   sub-microsecond precision loss of a whole-timestamp `* 1e6`.
//! - pydantic coercion: a `float`-declared field coerces an engine-typed
//!   integer to its float form (`0` -> `0.0`); an `int`-declared field stays
//!   integer. The `cost`/`returns`/`net`/`returnRate` columns are
//!   `round(.., n)` over float-space sums, so they always carry a fraction;
//!   the `level`/`ttValueGained`/`ttValue`/`damageDealt`/`costAttributed`
//!   fields are float-declared and pass through `float_field`.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::tracker::naive_to_epoch;
use serde_json::{json, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use crate::hydration::{
    detail, error_response, internal_error, json_response, plain_json_response, HydrationState,
};

/// The attribute skills the session-detail skill-gain aggregate excludes
/// (`backend.services.character_calc.ATTRIBUTE_SKILLS`). A Python `set`, so
/// the `NOT IN (...)` placeholder order is the set's iteration order; the
/// membership test is what matters, not the order, so a sorted constant
/// keeps the same exclusion deterministically.
const ATTRIBUTE_SKILLS: [&str; 6] = [
    "Agility",
    "Health",
    "Intelligence",
    "Psyche",
    "Stamina",
    "Strength",
];

// ── Engine-typed numeric primitives (the analytics-router siblings) ──

/// A SQLite numeric read preserving the engine type: a REAL decodes to a
/// float, an INTEGER (the `COALESCE(SUM(...), 0)` empty case) to an integer.
fn sql_number(row: &SqliteRow, index: usize) -> Value {
    match row.try_get::<f64, _>(index) {
        Ok(value) => json!(value),
        Err(_) => json!(row.get::<i64, _>(index)),
    }
}

/// `float(value)` over an engine-typed number.
fn as_f64(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

/// `round(value, places)`: banker's rounding, always producing a float
/// (these columns are float-space sums).
fn round(value: f64, places: usize) -> f64 {
    eo_wire::normalizer::round_half_even(value, places)
}

/// A model-declared `float` field: coerce an engine-typed integer to its
/// float form, so an integer zero leaves the wire as `0.0`.
fn float_field(value: Value) -> Value {
    match value.as_i64() {
        Some(integer) => json!(integer as f64),
        None => value,
    }
}

/// `datetime.fromtimestamp(ts, tz=UTC).isoformat()`.
///
/// Python rounds the POSIX timestamp to the nearest microsecond, then
/// renders `YYYY-MM-DDTHH:MM:SS[.ffffff]+00:00`, omitting the fractional
/// part entirely when it rounds to zero microseconds. `None` maps to JSON
/// null (the nullable `startTime` / `endTime`).
fn ts_to_iso(ts: Option<f64>) -> Value {
    let Some(ts) = ts else {
        return Value::Null;
    };
    // Mirror CPython's `datetime.fromtimestamp`: split the timestamp into
    // its integral seconds and fractional part with `modf`, then round ONLY
    // the fraction to the nearest microsecond (round-half-to-even). Rounding
    // the WHOLE `ts * 1e6` instead loses sub-microsecond precision at the
    // current epoch (magnitude ~1.78e15 vs an f64 ULP of ~0.25us), which
    // diverged from CPython on ~12% of realistic timestamps.
    let frac = ts.fract();
    let whole = ts.trunc() as i64;
    let mut micros = eo_wire::normalizer::round_half_even(frac * 1_000_000.0, 0) as i64;
    let mut secs = whole;
    // The fraction can round to +/- 1e6; carry/borrow as CPython does.
    if micros >= 1_000_000 {
        secs += 1;
        micros -= 1_000_000;
    } else if micros < 0 {
        secs -= 1;
        micros += 1_000_000;
    }
    let dt = chrono::DateTime::from_timestamp(secs, (micros as u32) * 1_000)
        .expect("timestamp within range");
    let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros == 0 {
        json!(format!("{base}+00:00"))
    } else {
        json!(format!("{base}.{micros:06}+00:00"))
    }
}

/// Duration in whole seconds: stored span for an ended session, the clock's
/// running span for an active one, else zero (`get_session_impl` /
/// `list_sessions_impl`).
fn duration_seconds(
    started_at: Option<f64>,
    ended_at: Option<f64>,
    is_active: bool,
    now: f64,
) -> i64 {
    match (ended_at, started_at) {
        (Some(end), Some(start)) => (end - start) as i64,
        _ if is_active => match started_at {
            Some(start) => (now - start) as i64,
            None => 0,
        },
        _ => 0,
    }
}

// ── list_sessions_impl ──

pub(crate) async fn list_sessions_impl(pool: &SqlitePool, now: f64) -> Result<Value, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, started_at, ended_at, is_active \
         FROM tracking_sessions ORDER BY started_at DESC LIMIT 20",
    )
    .fetch_all(pool)
    .await?;

    let mut sessions = Vec::with_capacity(rows.len());
    for row in &rows {
        let sid = row.get::<String, _>(0);
        let started_at = row.try_get::<Option<f64>, _>(1).ok().flatten();
        let ended_at = row.try_get::<Option<f64>, _>(2).ok().flatten();
        let is_active = row.get::<i64, _>(3) != 0;

        let duration = duration_seconds(started_at, ended_at, is_active, now);

        // Cost: weapon cycling + heal + enhancer + armour + dangling.
        let weapon_cost = scalar(
            pool,
            "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
             FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE k.session_id = ?",
            &sid,
        )
        .await?;
        let enhancer_cost = scalar(
            pool,
            "SELECT COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE k.session_id = ?",
            &sid,
        )
        .await?;
        let sess_costs = sqlx::query(
            "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) \
             FROM tracking_sessions WHERE id = ?",
        )
        .bind(&sid)
        .fetch_one(pool)
        .await?;
        let armour_cost = as_f64(&sql_number(&sess_costs, 0));
        let heal_cost = as_f64(&sql_number(&sess_costs, 1));
        let dangling_cost = as_f64(&sql_number(&sess_costs, 2));
        let weapon_cost = as_f64(&weapon_cost);
        let enhancer_cost = as_f64(&enhancer_cost);
        let cost = weapon_cost + heal_cost + enhancer_cost + armour_cost + dangling_cost;

        // Returns: sum of loot.
        let returns = as_f64(
            &scalar(
                pool,
                "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
                &sid,
            )
            .await?,
        );

        let primary_mobs = string_column(
            pool,
            "SELECT mob_name FROM kills \
             WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
             GROUP BY mob_name ORDER BY COUNT(*) DESC LIMIT 3",
            &sid,
        )
        .await?;
        let primary_weapons = string_column(
            pool,
            "SELECT ts.tool_name FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
             WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
             GROUP BY ts.tool_name ORDER BY SUM(ts.shots_fired) DESC LIMIT 3",
            &sid,
        )
        .await?;

        let net = returns - cost;
        let return_rate = if cost > 0.0 { returns / cost } else { 0.0 };

        let counts = sqlx::query(
            "SELECT \
               COALESCE(SUM(CASE WHEN event_type LIKE 'global_%' THEN 1 ELSE 0 END), 0), \
               COALESCE(SUM(CASE WHEN event_type LIKE 'hof_%' THEN 1 ELSE 0 END), 0) \
             FROM notable_events WHERE session_id = ?",
        )
        .bind(&sid)
        .fetch_one(pool)
        .await?;
        let globals = counts.get::<i64, _>(0);
        let hofs = counts.get::<i64, _>(1);

        sessions.push(json!({
            "id": sid,
            "startTime": ts_to_iso(started_at),
            "endTime": ts_to_iso(ended_at),
            "duration": duration,
            "primaryMobs": primary_mobs,
            "primaryWeapons": primary_weapons,
            "cost": round(cost, 2),
            "returns": round(returns, 2),
            "net": round(net, 2),
            "returnRate": round(return_rate, 4),
            "globals": globals,
            "hofs": hofs,
        }));
    }
    Ok(Value::Array(sessions))
}

/// A single-scalar aggregate bound to one session id, engine-typed.
async fn scalar(pool: &SqlitePool, sql: &'static str, sid: &str) -> Result<Value, sqlx::Error> {
    let row = sqlx::query(sql).bind(sid).fetch_one(pool).await?;
    Ok(sql_number(&row, 0))
}

/// The first column of every row as a string list (the primary-mob /
/// primary-weapon top-N selects, whose filters guarantee non-null text).
async fn string_column(
    pool: &SqlitePool,
    sql: &'static str,
    sid: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(sql).bind(sid).fetch_all(pool).await?;
    Ok(rows.iter().map(|r| r.get::<String, _>(0)).collect())
}

// ── get_session_impl ──

pub(crate) async fn get_session_impl(
    pool: &SqlitePool,
    session_id: &str,
    now: f64,
) -> Result<Option<Value>, sqlx::Error> {
    let session_row = sqlx::query(
        "SELECT id, started_at, ended_at, is_active, mob_tracking_mode \
         FROM tracking_sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    let Some(session_row) = session_row else {
        return Ok(None);
    };

    let started_at = session_row.try_get::<Option<f64>, _>(1).ok().flatten();
    let ended_at = session_row.try_get::<Option<f64>, _>(2).ok().flatten();
    let is_active = session_row.get::<i64, _>(3) != 0;
    // `session_row[4] or "mob"`: NULL and the empty string both default.
    let mob_entry_mode = session_row
        .try_get::<Option<String>, _>(4)
        .ok()
        .flatten()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "mob".to_string());

    let duration = duration_seconds(started_at, ended_at, is_active, now);

    // Session-level costs.
    let sess_costs = sqlx::query(
        "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) \
         FROM tracking_sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let armour_cost = as_f64(&sql_number(&sess_costs, 0));
    let session_heal_cost = as_f64(&sql_number(&sess_costs, 1));
    let dangling_cost = as_f64(&sql_number(&sess_costs, 2));

    let kill_totals = sqlx::query(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    let kills = kill_totals.get::<i64, _>(0);
    let total_returns = as_f64(&sql_number(&kill_totals, 1));
    let total_enhancer_cost = as_f64(&sql_number(&kill_totals, 2));

    // Tool stats aggregated across the session, one row per tool_name in
    // SELECT order (the dict insertion order the reference preserves).
    let tool_rows = sqlx::query(
        "SELECT t.tool_name, SUM(t.shots_fired), SUM(t.damage_dealt), SUM(t.critical_hits), \
         SUM(COALESCE(t.cost_per_shot, 0) * COALESCE(t.shots_fired, 0)) \
         FROM kill_tool_stats t JOIN kills k ON k.id = t.kill_id \
         WHERE k.session_id = ? GROUP BY t.tool_name",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let mut weapon_cost = 0.0;
    // (name, shots, damage, crits, cost_attributed) in SELECT order.
    let mut merged_tools: Vec<(String, i64, f64, i64, f64)> = Vec::with_capacity(tool_rows.len());
    for row in &tool_rows {
        let name = row.get::<String, _>(0);
        let shots = row.try_get::<i64, _>(1).unwrap_or(0);
        let dmg = as_f64(&sql_number(row, 2));
        let crits = row.try_get::<i64, _>(3).unwrap_or(0);
        let cost_attr = as_f64(&sql_number(row, 4));
        weapon_cost += cost_attr;
        merged_tools.push((name, shots, dmg, crits, cost_attr));
    }

    // Loot breakdown aggregated by item_name; active vs deactivated, shrapnel
    // excluded from both. Insertion order is SELECT order before the sort.
    let merged_loot = loot_agg(pool, session_id, "l.deactivated_at IS NULL").await?;
    let merged_deactivated_loot =
        loot_agg(pool, session_id, "l.deactivated_at IS NOT NULL").await?;

    // Per-mob breakdown, ordered by COUNT desc (SQL).
    let mob_breakdown_rows = sqlx::query(
        "SELECT mob_name, original_mob_name, COUNT(*) FROM kills \
         WHERE session_id = ? AND mob_name IS NOT NULL \
         GROUP BY mob_name, original_mob_name ORDER BY COUNT(*) DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let mob_breakdown: Vec<Value> = mob_breakdown_rows
        .iter()
        .map(|row| {
            json!({
                "currentName": row.get::<String, _>(0),
                "originalName": row.get::<Option<String>, _>(1),
                "killCount": row.get::<i64, _>(2),
            })
        })
        .collect();

    let total_cost =
        weapon_cost + session_heal_cost + total_enhancer_cost + armour_cost + dangling_cost;

    let detail_skill_tt = as_f64(
        &scalar(
            pool,
            "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
            session_id,
        )
        .await?,
    );

    let net = total_returns - total_cost;
    let return_rate = if total_cost > 0.0 {
        total_returns / total_cost
    } else {
        0.0
    };

    // Loot breakdown sorted by ttValue descending (Python's stable sort;
    // ties keep insertion order, which is SELECT order).
    let loot_breakdown = loot_breakdown_sorted(&merged_loot);
    let deactivated_loot_breakdown = loot_breakdown_sorted(&merged_deactivated_loot);

    // Tool stats sorted by shotsFired descending (stable).
    let mut tool_stats: Vec<(i64, Value)> = merged_tools
        .iter()
        .map(|(name, shots, dmg, crits, cost_attr)| {
            (
                *shots,
                json!({
                    "weaponName": name,
                    "shotsFired": shots,
                    "damageDealt": float_field(json!(dmg)),
                    "crits": crits,
                    "costAttributed": round(*cost_attr, 2),
                }),
            )
        })
        .collect();
    stable_sort_desc_by_key(&mut tool_stats);
    let tool_stats: Vec<Value> = tool_stats.into_iter().map(|(_, v)| v).collect();

    // Notable events ordered by timestamp (SQL).
    let notable_rows = sqlx::query(
        "SELECT event_type, mob_or_item, value_ped FROM notable_events \
         WHERE session_id = ? ORDER BY timestamp",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    let notable_events: Vec<Value> = notable_rows
        .iter()
        .map(|row| {
            let event_type = row.get::<String, _>(0);
            let mob_or_item = row.get::<Option<String>, _>(1);
            let value = sql_number(row, 2);
            json!({
                "type": notable_event_category(&event_type),
                "eventType": event_type,
                "target": mob_or_item,
                "item": mob_or_item,
                "value": float_field(value),
            })
        })
        .collect();

    let skill_gains = session_skill_gains(pool, session_id).await?;

    Ok(Some(json!({
        "sessionId": session_id,
        "summary": {
            "cost": round(total_cost, 2),
            "returns": round(total_returns, 2),
            "pes": round(detail_skill_tt, 2),
            "net": round(net, 2),
            "returnRate": round(return_rate, 4),
            "kills": kills,
            "duration": duration,
            "costBreakdown": {
                "weaponCost": round(weapon_cost, 2),
                "healCost": round(session_heal_cost, 2),
                "enhancerCost": round(total_enhancer_cost, 2),
                "armourCost": round(armour_cost, 2),
            },
        },
        "mobEntryMode": mob_entry_mode,
        "notableEvents": notable_events,
        "lootBreakdown": loot_breakdown,
        "deactivatedLootBreakdown": deactivated_loot_breakdown,
        "mobBreakdown": mob_breakdown,
        "effectiveLoot": round(total_returns, 2),
        "toolStats": tool_stats,
        "skillGains": skill_gains,
    })))
}

/// `_notable_event_category`: the `type` field of a notable event.
fn notable_event_category(event_type: &str) -> &'static str {
    if event_type.starts_with("quest_") {
        "quest"
    } else if event_type.starts_with("hof_") {
        "hof"
    } else {
        "global"
    }
}

/// One loot aggregate (active or deactivated), as `(item_name, quantity,
/// tt_value)` in SELECT row order. Shrapnel is excluded symmetrically.
async fn loot_agg(
    pool: &SqlitePool,
    session_id: &str,
    deactivated_clause: &str,
) -> Result<Vec<(String, i64, f64)>, sqlx::Error> {
    let sql = format!(
        "SELECT l.item_name, SUM(l.quantity), SUM(l.value_ped) \
         FROM kill_loot_items l JOIN kills k ON k.id = l.kill_id \
         WHERE k.session_id = ? AND COALESCE(l.is_enhancer_shrapnel, 0) = 0 AND {deactivated_clause} \
         GROUP BY l.item_name"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(session_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let name = row.get::<String, _>(0);
            let qty = row.try_get::<i64, _>(1).unwrap_or(0);
            let val = as_f64(&sql_number(row, 2));
            (name, qty, val)
        })
        .collect())
}

/// Build a loot breakdown sorted by rounded ttValue descending (stable).
fn loot_breakdown_sorted(rows: &[(String, i64, f64)]) -> Vec<Value> {
    let mut entries: Vec<(f64, Value)> = rows
        .iter()
        .map(|(name, qty, val)| {
            let tt = round(*val, 2);
            (
                tt,
                json!({
                    "name": name,
                    "quantity": qty,
                    "ttValue": float_field(json!(tt)),
                }),
            )
        })
        .collect();
    stable_sort_desc_by_f64(&mut entries);
    entries.into_iter().map(|(_, v)| v).collect()
}

/// `_session_skill_gains`: per-skill totals (attributes excluded), each with
/// the latest calibrated level. Ordered by total ped descending (SQL).
async fn session_skill_gains(pool: &SqlitePool, session_id: &str) -> Result<Value, sqlx::Error> {
    let attr_placeholders = vec!["?"; ATTRIBUTE_SKILLS.len()].join(",");
    let sql = format!(
        "SELECT sg.skill_name, SUM(sg.amount) as total_amount, \
         COALESCE(SUM(sg.ped_value), 0) as total_ped \
         FROM skill_gains sg WHERE sg.session_id = ? \
         AND sg.skill_name NOT IN ({attr_placeholders}) \
         GROUP BY sg.skill_name ORDER BY total_ped DESC"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(session_id);
    for attr in ATTRIBUTE_SKILLS {
        query = query.bind(attr);
    }
    let rows = query.fetch_all(pool).await?;
    if rows.is_empty() {
        return Ok(json!([]));
    }

    let skill_names: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    let placeholders = vec!["?"; skill_names.len()].join(",");
    let cal_sql = format!(
        "SELECT skill_name, level FROM skill_calibrations WHERE id IN ( \
         SELECT MAX(id) FROM skill_calibrations WHERE skill_name IN ({placeholders}) \
         GROUP BY skill_name)"
    );
    let mut cal_query = sqlx::query(sqlx::AssertSqlSafe(cal_sql));
    for name in &skill_names {
        cal_query = cal_query.bind(name);
    }
    let cal_rows = cal_query.fetch_all(pool).await?;
    let mut levels: BTreeMap<String, f64> = BTreeMap::new();
    for row in &cal_rows {
        levels.insert(row.get::<String, _>(0), as_f64(&sql_number(row, 1)));
    }

    let gains: Vec<Value> = rows
        .iter()
        .map(|row| {
            let name = row.get::<String, _>(0);
            // `levels.get(name, 0)`: missing calibration -> int 0, which
            // `round(0, 1)` keeps an int, then the float-declared field
            // coerces it to 0.0.
            let level = match levels.get(&name) {
                Some(level) => float_field(json!(round(*level, 1))),
                None => json!(0.0),
            };
            let tt = as_f64(&sql_number(row, 2));
            json!({
                "skillName": name,
                "level": level,
                "ttValueGained": float_field(json!(round(tt, 4))),
            })
        })
        .collect();
    Ok(Value::Array(gains))
}

/// Stable descending sort by an i64 key (Python's `sorted(reverse=True)`
/// over a single numeric key keeps the original order of equal elements).
fn stable_sort_desc_by_key(entries: &mut [(i64, Value)]) {
    // `sort_by_key` is stable in Rust's std; `Reverse` makes it descending
    // while preserving the original order of equal keys.
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
}

/// Stable descending sort by an f64 key.
fn stable_sort_desc_by_f64(entries: &mut [(f64, Value)]) {
    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
}

// ── tag_suggestions ──

async fn tag_suggestions_impl(
    pool: &SqlitePool,
    q: &str,
    limit: i64,
) -> Result<Value, sqlx::Error> {
    let query = q.trim();
    if query.is_empty() {
        return Ok(json!([]));
    }
    // `max(1, min(limit, 20))`.
    let bounded = limit.clamp(1, 20);
    let like = format!("%{}%", query.to_lowercase());
    let rows = sqlx::query(
        "SELECT mob_name, COUNT(*) as uses FROM kills \
         WHERE mob_name IS NOT NULL AND mob_name != 'Unknown' \
         AND COALESCE(mob_species, '') = '' AND COALESCE(mob_maturity, '') = '' \
         AND lower(mob_name) LIKE ? \
         GROUP BY mob_name ORDER BY uses DESC, mob_name ASC LIMIT ?",
    )
    .bind(&like)
    .bind(bounded)
    .fetch_all(pool)
    .await?;
    let names: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    Ok(json!(names))
}

// ── Session edits (rename-mob / restore-mob / loot flip / armour-cost) ──
//
// Post-hoc edits to ENDED sessions, byte-faithful to the original
// Python implementation. Each mutates only the shared SQLite
// database. The four mob/loot edits share the active-session guard
// (`_validate_session_exists`): 404 when the session is absent, 409 when
// it is still active. `armour-cost` deliberately omits that guard (the
// reference does too), accepting the edit on any session that exists.
//
// These reply as plain JSON 200s; the ETag middleware decorates only
// 2xx GETs, so writes carry no conditional-GET headers.

/// A handler-level edit failure carrying the reference's exact HTTP
/// status and detail string. `Internal` is the unhandled-exception 500.
#[derive(Debug)]
enum EditError {
    Http(StatusCode, String),
    Internal,
}

impl From<sqlx::Error> for EditError {
    fn from(_: sqlx::Error) -> Self {
        EditError::Internal
    }
}

impl EditError {
    fn into_response(self) -> Response<Body> {
        match self {
            EditError::Http(status, message) => error_response(status, &detail(&message)),
            EditError::Internal => internal_error(),
        }
    }
}

/// `_validate_session_exists`: 404 if the session row is absent, 409 if
/// it is still active (`is_active = 1`).
async fn validate_session_exists(pool: &SqlitePool, session_id: &str) -> Result<(), EditError> {
    let row = sqlx::query("SELECT id, is_active FROM tracking_sessions WHERE id = ?")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Err(EditError::Http(
            StatusCode::NOT_FOUND,
            "Session not found".to_string(),
        ));
    };
    if row.get::<i64, _>(1) != 0 {
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            "Session mob edits are only available after the session has ended".to_string(),
        ));
    }
    Ok(())
}

/// `_build_mob_edit_response`: the post-mutation per-mob kill count for
/// the resulting `mob_name`.
async fn build_mob_edit_response(
    pool: &SqlitePool,
    session_id: &str,
    mob_name: &str,
) -> Result<Value, EditError> {
    let row = sqlx::query("SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = ?")
        .bind(session_id)
        .bind(mob_name)
        .fetch_one(pool)
        .await?;
    Ok(json!({
        "sessionId": session_id,
        "mobName": mob_name,
        "killCount": row.get::<i64, _>(0),
    }))
}

/// `_build_loot_item_edit_response`: affected row count, the signed
/// value delta (4dp), and the session's recomputed returns total (2dp).
async fn build_loot_item_edit_response(
    pool: &SqlitePool,
    session_id: &str,
    item_name: &str,
    affected_rows: i64,
    total_value_delta: f64,
) -> Result<Value, EditError> {
    let row =
        sqlx::query("SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await?;
    let session_returns = as_f64(&sql_number(&row, 0));
    Ok(json!({
        "sessionId": session_id,
        "itemName": item_name,
        "affectedRows": affected_rows,
        "totalValueDelta": round(total_value_delta, 4),
        "sessionTotalReturns": round(session_returns, 2),
    }))
}

/// `_rename_session_mob_impl`.
async fn rename_session_mob_impl(
    pool: &SqlitePool,
    session_id: &str,
    from_mob: &str,
    to_mob: &str,
) -> Result<Value, EditError> {
    validate_session_exists(pool, session_id).await?;
    let from_mob = from_mob.trim();
    let to_mob = to_mob.trim();
    if from_mob.is_empty() || to_mob.is_empty() {
        return Err(EditError::Http(
            StatusCode::BAD_REQUEST,
            "Mob names cannot be blank".to_string(),
        ));
    }
    if from_mob == to_mob {
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            "rename target matches the current value (no-op)".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    // Preserve the first original via COALESCE; the rowcount inside the
    // transaction is the precondition (zero matches -> rollback + 409).
    let preserve = sqlx::query(
        "UPDATE kills \
         SET original_mob_name = COALESCE(original_mob_name, mob_name) \
         WHERE session_id = ? AND mob_name = ?",
    )
    .bind(session_id)
    .bind(from_mob)
    .execute(&mut *tx)
    .await?;
    if preserve.rows_affected() == 0 {
        // Drop the transaction (rollback) before the 409.
        drop(tx);
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            format!("No kills in this session match mob_name='{from_mob}'"),
        ));
    }
    // Rewrite mob_name; clear the preservation column on a round-trip to
    // the genuinely-original capture (CASE: original == new -> NULL).
    sqlx::query(
        "UPDATE kills \
         SET mob_name = ?, \
             original_mob_name = CASE \
                 WHEN original_mob_name = ? THEN NULL \
                 ELSE original_mob_name \
             END \
         WHERE session_id = ? AND mob_name = ?",
    )
    .bind(to_mob)
    .bind(to_mob)
    .bind(session_id)
    .bind(from_mob)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    build_mob_edit_response(pool, session_id, to_mob).await
}

/// `_restore_session_mob_impl`.
async fn restore_session_mob_impl(
    pool: &SqlitePool,
    session_id: &str,
    current_mob: &str,
) -> Result<Value, EditError> {
    validate_session_exists(pool, session_id).await?;
    let current_mob = current_mob.trim();
    if current_mob.is_empty() {
        return Err(EditError::Http(
            StatusCode::BAD_REQUEST,
            "Mob name cannot be blank".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    let restored_rows = sqlx::query(
        "UPDATE kills \
         SET mob_name = original_mob_name, original_mob_name = NULL \
         WHERE session_id = ? AND mob_name = ? AND original_mob_name IS NOT NULL \
         RETURNING mob_name",
    )
    .bind(session_id)
    .bind(current_mob)
    .fetch_all(&mut *tx)
    .await?;

    if restored_rows.is_empty() {
        drop(tx);
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            format!(
                "No restorable kills in this session for mob_name='{current_mob}' \
                 (either no rename has happened or the preservation column is empty)"
            ),
        ));
    }

    // Distinct originals (a Python set): >1 means several prior names
    // merged into the current one; the single-result shape cannot split
    // them, so refuse with the ambiguous 409.
    let mut distinct: Vec<String> = Vec::new();
    for row in &restored_rows {
        let original = row.get::<String, _>(0);
        if !distinct.contains(&original) {
            distinct.push(original);
        }
    }
    if distinct.len() > 1 {
        drop(tx);
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            format!(
                "Ambiguous restore for mob_name='{current_mob}': {} distinct prior names merged into it.",
                distinct.len()
            ),
        ));
    }
    let restored_to = distinct.into_iter().next().expect("one distinct original");

    sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    build_mob_edit_response(pool, session_id, &restored_to).await
}

/// `_bulk_flip_loot_item`: flip every matching loot row in the opposite
/// state, recompute each parent kill's denormalised `loot_total_ped`,
/// invalidate the session summary. `to_state` is "deactivated" or
/// "active".
async fn bulk_flip_loot_item(
    pool: &SqlitePool,
    session_id: &str,
    item_name: &str,
    to_state: &str,
) -> Result<Value, EditError> {
    validate_session_exists(pool, session_id).await?;
    let item_name = item_name.trim();
    if item_name.is_empty() {
        return Err(EditError::Http(
            StatusCode::BAD_REQUEST,
            "Item name cannot be blank".to_string(),
        ));
    }

    // `unixepoch('now')` is the wall clock, exactly as the reference's
    // flag write is; the flip STATE (null vs not) is what callers and
    // the A/B db-state comparison observe, not the literal timestamp.
    let (opposite_clause, new_flag_sql, delta_sign) = match to_state {
        "deactivated" => ("l.deactivated_at IS NULL", "unixepoch('now')", -1.0_f64),
        "active" => ("l.deactivated_at IS NOT NULL", "NULL", 1.0_f64),
        other => panic!("unsupported to_state: {other:?}"),
    };

    let mut tx = pool.begin().await?;
    let flip_sql = format!(
        "UPDATE kill_loot_items \
         SET deactivated_at = {new_flag_sql} \
         WHERE id IN ( \
             SELECT l.id \
             FROM kill_loot_items l \
             JOIN kills k ON k.id = l.kill_id \
             WHERE k.session_id = ? AND l.item_name = ? AND {opposite_clause} \
         ) \
         RETURNING kill_id, value_ped"
    );
    let flipped = sqlx::query(sqlx::AssertSqlSafe(flip_sql))
        .bind(session_id)
        .bind(item_name)
        .fetch_all(&mut *tx)
        .await?;

    if flipped.is_empty() {
        // 404 (item not in session) vs 409 (already in target state),
        // decided from the same locked transaction.
        let any_row = sqlx::query(
            "SELECT 1 FROM kill_loot_items l \
             JOIN kills k ON k.id = l.kill_id \
             WHERE k.session_id = ? AND l.item_name = ? \
             LIMIT 1",
        )
        .bind(session_id)
        .bind(item_name)
        .fetch_optional(&mut *tx)
        .await?;
        drop(tx);
        if any_row.is_none() {
            return Err(EditError::Http(
                StatusCode::NOT_FOUND,
                format!("No loot named '{item_name}' in this session"),
            ));
        }
        return Err(EditError::Http(
            StatusCode::CONFLICT,
            format!("All '{item_name}' rows in this session are already {to_state}"),
        ));
    }

    // Aggregate per-kill deltas from RETURNING so each parent gets one
    // UPDATE rather than N. Insertion order preserves the Python dict's.
    let mut order: Vec<String> = Vec::new();
    let mut per_kill: BTreeMap<String, f64> = BTreeMap::new();
    let mut total_delta = 0.0;
    for row in &flipped {
        let kill_id = row.get::<String, _>(0);
        let value = as_f64(&sql_number(row, 1));
        if !per_kill.contains_key(&kill_id) {
            order.push(kill_id.clone());
        }
        *per_kill.entry(kill_id).or_insert(0.0) += value;
        total_delta += value;
    }
    for kill_id in &order {
        let kill_delta = per_kill[kill_id];
        sqlx::query("UPDATE kills SET loot_total_ped = loot_total_ped + ? WHERE id = ?")
            .bind(delta_sign * kill_delta)
            .bind(kill_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM session_summaries WHERE session_id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    build_loot_item_edit_response(
        pool,
        session_id,
        item_name,
        flipped.len() as i64,
        delta_sign * total_delta,
    )
    .await
}

/// `set_armour_cost`: 404 if absent (NO active-session guard), else add
/// the cost to the session's COALESCE(armour_cost, 0). The response
/// echoes `round(cost, 2)` (the submitted value, float-coerced), NOT
/// the new total.
async fn set_armour_cost_impl(
    pool: &SqlitePool,
    session_id: &str,
    cost: f64,
) -> Result<Value, EditError> {
    let row = sqlx::query("SELECT id FROM tracking_sessions WHERE id = ?")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(EditError::Http(
            StatusCode::NOT_FOUND,
            "Session not found".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE tracking_sessions SET armour_cost = COALESCE(armour_cost, 0) + ? WHERE id = ?",
    )
    .bind(cost)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(json!({
        "sessionId": session_id,
        "armourCost": round(cost, 2),
    }))
}

// ── The three handlers on the composition-root state ──

impl HydrationState {
    /// GET /api/tracking/sessions
    pub async fn tracking_sessions(&self, if_none_match: Option<&str>) -> Response<Body> {
        let now = naive_to_epoch(self.clock.now());
        match list_sessions_impl(self.pool(), now).await {
            Ok(value) => json_response(&value, if_none_match),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/tracking/session/{session_id}
    pub async fn tracking_session(
        &self,
        session_id: &str,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        let now = naive_to_epoch(self.clock.now());
        match get_session_impl(self.pool(), session_id, now).await {
            Ok(Some(value)) => json_response(&value, if_none_match),
            Ok(None) => error_response(StatusCode::NOT_FOUND, &detail("Session not found")),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/tracking/tag-suggestions?q=&limit=
    pub async fn tracking_tag_suggestions(
        &self,
        q: &str,
        limit: i64,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        match tag_suggestions_impl(self.pool(), q, limit).await {
            Ok(value) => json_response(&value, if_none_match),
            Err(_) => internal_error(),
        }
    }

    /// POST /api/tracking/session/{session_id}/rename-mob
    pub async fn tracking_rename_mob(
        &self,
        session_id: &str,
        from_mob: &str,
        to_mob: &str,
    ) -> Response<Body> {
        match rename_session_mob_impl(self.pool(), session_id, from_mob, to_mob).await {
            Ok(value) => plain_json_response(&value),
            Err(error) => error.into_response(),
        }
    }

    /// POST /api/tracking/session/{session_id}/restore-mob
    pub async fn tracking_restore_mob(
        &self,
        session_id: &str,
        current_mob: &str,
    ) -> Response<Body> {
        match restore_session_mob_impl(self.pool(), session_id, current_mob).await {
            Ok(value) => plain_json_response(&value),
            Err(error) => error.into_response(),
        }
    }

    /// POST /api/tracking/session/{session_id}/loot-item/{item_name:path}/deactivate
    pub async fn tracking_deactivate_loot_item(
        &self,
        session_id: &str,
        item_name: &str,
    ) -> Response<Body> {
        match bulk_flip_loot_item(self.pool(), session_id, item_name, "deactivated").await {
            Ok(value) => plain_json_response(&value),
            Err(error) => error.into_response(),
        }
    }

    /// POST /api/tracking/session/{session_id}/loot-item/{item_name:path}/activate
    pub async fn tracking_activate_loot_item(
        &self,
        session_id: &str,
        item_name: &str,
    ) -> Response<Body> {
        match bulk_flip_loot_item(self.pool(), session_id, item_name, "active").await {
            Ok(value) => plain_json_response(&value),
            Err(error) => error.into_response(),
        }
    }

    /// POST /api/tracking/session/{session_id}/armour-cost
    pub async fn tracking_set_armour_cost(&self, session_id: &str, cost: f64) -> Response<Body> {
        match set_armour_cost_impl(self.pool(), session_id, cost).await {
            Ok(value) => plain_json_response(&value),
            Err(error) => error.into_response(),
        }
    }
}

// The expected values in these tests are the original backend's own outputs,
// frozen as committed goldens; these hermetic pins guard the same surface
// with no second implementation present.
#[cfg(test)]
mod tests {
    use super::*;
    use eo_wire::normalizer::to_wire_json;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn memory_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory pool");
        for ddl in [
            "CREATE TABLE tracking_sessions(id TEXT PRIMARY KEY, started_at REAL, ended_at REAL, \
             is_active INTEGER, armour_cost REAL, heal_cost REAL, dangling_cost REAL, \
             mob_tracking_mode TEXT, updated_at REAL)",
            "CREATE TABLE kills(id TEXT PRIMARY KEY, session_id TEXT, mob_name TEXT, \
             mob_species TEXT, mob_maturity TEXT, timestamp REAL, shots_fired INTEGER, \
             damage_dealt REAL, damage_taken REAL, critical_hits INTEGER, cost_ped REAL, \
             enhancer_cost REAL, loot_total_ped REAL, is_global INTEGER, is_hof INTEGER, \
             original_mob_name TEXT)",
            "CREATE TABLE kill_tool_stats(id INTEGER PRIMARY KEY, kill_id TEXT, tool_name TEXT, \
             shots_fired INTEGER, damage_dealt REAL, critical_hits INTEGER, cost_per_shot REAL)",
            "CREATE TABLE kill_loot_items(id INTEGER PRIMARY KEY, kill_id TEXT, item_name TEXT, \
             quantity INTEGER, value_ped REAL, is_enhancer_shrapnel INTEGER, deactivated_at REAL)",
            "CREATE TABLE skill_gains(id INTEGER PRIMARY KEY, session_id TEXT, timestamp REAL, \
             skill_name TEXT, amount REAL, ped_value REAL, created_at REAL)",
            "CREATE TABLE skill_calibrations(id INTEGER PRIMARY KEY, skill_name TEXT, level REAL, \
             source TEXT, scanned_at REAL)",
            "CREATE TABLE notable_events(id INTEGER PRIMARY KEY, session_id TEXT, kill_id TEXT, \
             event_type TEXT, mob_or_item TEXT, value_ped REAL, timestamp REAL)",
            "CREATE TABLE session_summaries(session_id TEXT PRIMARY KEY, computed_at REAL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.expect("ddl");
        }
        pool
    }

    async fn seed(pool: &SqlitePool) {
        let sid = "s1";
        let start = 1_747_735_200.0_f64; // whole-second instant
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(sid)
        .bind(start)
        .bind(start + 3600.0)
        .bind(0_i64)
        .bind(1.0)
        .bind(2.0)
        .bind(0.5)
        .bind("mob")
        .bind(start + 3600.0)
        .execute(pool)
        .await
        .unwrap();
        for i in 0..5 {
            let kid = format!("k{i}");
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,\
                 shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,\
                 loot_total_ped,is_global,is_hof,original_mob_name) \
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(&kid)
            .bind(sid)
            .bind("Atrox")
            .bind("Atrox")
            .bind("Young")
            .bind(start + i as f64)
            .bind(50_i64)
            .bind(500.0)
            .bind(10.0)
            .bind(3_i64)
            .bind(0.55)
            .bind(0.1)
            .bind(10.0)
            .bind(if i == 0 { 1_i64 } else { 0 })
            .bind(0_i64)
            .bind(if i == 0 { Some("Atroxx") } else { None })
            .execute(pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,damage_dealt,critical_hits,cost_per_shot) VALUES(?,?,?,?,?,?)")
                .bind(&kid).bind("Opalo").bind(50_i64).bind(500.0).bind(3_i64).bind(0.011)
                .execute(pool).await.unwrap();
            sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
                .bind(&kid).bind("Animal Hide").bind(2_i64).bind(3.0).bind(0_i64).bind(Option::<f64>::None)
                .execute(pool).await.unwrap();
        }
        // species-less tag mob
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,\
             shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,\
             loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind("kt")
        .bind(sid)
        .bind("Atrocious Tag")
        .bind("")
        .bind("")
        .bind(start + 100.0)
        .bind(10_i64)
        .bind(50.0)
        .bind(0.0)
        .bind(0_i64)
        .bind(0.1)
        .bind(0.0)
        .bind(1.0)
        .bind(0_i64)
        .bind(0_i64)
        .bind(Option::<String>::None)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO notable_events(session_id,kill_id,event_type,mob_or_item,value_ped,timestamp) VALUES(?,?,?,?,?,?)")
            .bind(sid).bind("k0").bind("global_kill").bind("Atrox").bind(55.0).bind(start + 1.0)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO notable_events(session_id,kill_id,event_type,mob_or_item,value_ped,timestamp) VALUES(?,?,?,?,?,?)")
            .bind(sid).bind("k1").bind("hof_item").bind("Rare Item").bind(1500.0).bind(start + 2.0)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
            .bind(sid).bind(start + 1800.0).bind("Laser Weaponry Technology").bind(3.0).bind(3.0).bind(start + 1800.0)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
            .bind(sid).bind(start + 1801.0).bind("Agility").bind(1.0).bind(1.0).bind(start + 1801.0)
            .execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO skill_calibrations(skill_name,level,source,scanned_at) VALUES(?,?,?,?)",
        )
        .bind("Laser Weaponry Technology")
        .bind(42.5)
        .bind("manual")
        .bind(start + 1000.0)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn ts_to_iso_matches_python_isoformat() {
        assert_eq!(ts_to_iso(None), Value::Null);
        assert_eq!(
            ts_to_iso(Some(1_747_735_200.0)),
            json!("2025-05-20T10:00:00+00:00")
        );
        // Fractional seconds render 6-digit microseconds.
        assert_eq!(
            ts_to_iso(Some(1_747_735_200.5)),
            json!("2025-05-20T10:00:00.500000+00:00")
        );
        assert_eq!(ts_to_iso(Some(0.0)), json!("1970-01-01T00:00:00+00:00"));
        // Sub-microsecond fractions at the current epoch: CPython splits the
        // fraction out before rounding (modf), so the whole-ts `* 1e6`
        // precision loss does not apply. These pins were verified against
        // `datetime.fromtimestamp(ts, tz=UTC).isoformat()`.
        assert_eq!(
            ts_to_iso(Some(1_747_735_200.000_000_5)),
            json!("2025-05-20T10:00:00+00:00")
        );
        assert_eq!(
            ts_to_iso(Some(1_747_735_200.123_456_5)),
            json!("2025-05-20T10:00:00.123456+00:00")
        );
        // Negative (pre-epoch) timestamps borrow a second for the fraction.
        assert_eq!(
            ts_to_iso(Some(-0.000_001_5)),
            json!("1969-12-31T23:59:59.999998+00:00")
        );
        // A fraction that rounds up to a whole second carries into the second.
        assert_eq!(
            ts_to_iso(Some(1_747_735_200.999_999_6)),
            json!("2025-05-20T10:00:01+00:00")
        );
    }

    #[test]
    fn notable_category_buckets_by_prefix() {
        assert_eq!(notable_event_category("quest_started"), "quest");
        assert_eq!(notable_event_category("hof_kill"), "hof");
        assert_eq!(notable_event_category("global_item"), "global");
        assert_eq!(notable_event_category("anything_else"), "global");
    }

    #[tokio::test]
    async fn sessions_list_shapes_the_summary_row() {
        let pool = memory_pool().await;
        seed(&pool).await;
        let value = list_sessions_impl(&pool, 0.0).await.unwrap();
        let wire = to_wire_json(&value);
        // cost = weapon(2.75) + heal(2.0) + enhancer(0.5) + armour(1.0) + dangling(0.5) = 6.75
        // returns = 5*10 + 1 = 51.0 ; net = 44.25 ; rate = 51/6.75 = 7.5556
        assert!(wire.contains("\"cost\":6.75"), "{wire}");
        assert!(wire.contains("\"returns\":51.0"), "{wire}");
        assert!(wire.contains("\"net\":44.25"), "{wire}");
        assert!(wire.contains("\"returnRate\":7.5556"), "{wire}");
        assert!(wire.contains("\"globals\":1"), "{wire}");
        assert!(wire.contains("\"hofs\":1"), "{wire}");
        assert!(
            wire.contains("\"startTime\":\"2025-05-20T10:00:00+00:00\""),
            "{wire}"
        );
        assert!(wire.contains("\"duration\":3600"), "{wire}");
        // primaryMobs by kill count desc: Atrox(5) then Atrocious Tag(1).
        assert!(
            wire.contains("\"primaryMobs\":[\"Atrox\",\"Atrocious Tag\"]"),
            "{wire}"
        );
        assert!(wire.contains("\"primaryWeapons\":[\"Opalo\"]"), "{wire}");
    }

    #[tokio::test]
    async fn session_detail_shapes_every_branch() {
        let pool = memory_pool().await;
        seed(&pool).await;
        let value = get_session_impl(&pool, "s1", 0.0).await.unwrap().unwrap();
        let wire = to_wire_json(&value);
        // summary: pes sums every skill_gain ped_value (attributes included,
        // unlike the skillGains list), so Laser(3.0) + Agility(1.0) = 4.0.
        assert!(wire.contains("\"pes\":4.0"), "{wire}");
        assert!(wire.contains("\"kills\":6"), "{wire}");
        assert!(wire.contains("\"weaponCost\":2.75"), "{wire}");
        // summary cost = the five cost components summed; returns/net/rate
        // derived (pins the sum + the rate division and its zero-guard).
        assert!(wire.contains("\"cost\":6.75"), "{wire}");
        assert!(wire.contains("\"returns\":51.0"), "{wire}");
        assert!(wire.contains("\"net\":44.25"), "{wire}");
        assert!(wire.contains("\"returnRate\":7.5556"), "{wire}");
        // notable events: global then hof, target==item, value coerced float.
        assert!(wire.contains("\"type\":\"global\",\"eventType\":\"global_kill\",\"target\":\"Atrox\",\"item\":\"Atrox\",\"value\":55.0"), "{wire}");
        // loot breakdown: only Animal Hide (shrapnel none seeded).
        assert!(
            wire.contains(
                "\"lootBreakdown\":[{\"name\":\"Animal Hide\",\"quantity\":10,\"ttValue\":15.0}]"
            ),
            "{wire}"
        );
        assert!(wire.contains("\"deactivatedLootBreakdown\":[]"), "{wire}");
        // tool stats: damageDealt float, crits int.
        assert!(wire.contains("\"weaponName\":\"Opalo\",\"shotsFired\":250,\"damageDealt\":2500.0,\"crits\":15,\"costAttributed\":2.75"), "{wire}");
        // skill gains: attribute (Agility) excluded; level from calibration.
        assert!(
            wire.contains(
                "\"skillName\":\"Laser Weaponry Technology\",\"level\":42.5,\"ttValueGained\":3.0"
            ),
            "{wire}"
        );
        assert!(
            !wire.contains("Agility"),
            "attribute skill excluded: {wire}"
        );
        // mob breakdown surfaces the renamed Atrox row.
        assert!(
            wire.contains("\"currentName\":\"Atrox\",\"originalName\":\"Atroxx\",\"killCount\":1"),
            "{wire}"
        );
    }

    #[tokio::test]
    async fn missing_session_is_none() {
        let pool = memory_pool().await;
        seed(&pool).await;
        assert!(get_session_impl(&pool, "nope", 0.0)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn skill_gains_zero_level_when_uncalibrated() {
        let pool = memory_pool().await;
        seed(&pool).await;
        // Remove the calibration; the level falls back to 0.0 (float-coerced).
        sqlx::query("DELETE FROM skill_calibrations")
            .execute(&pool)
            .await
            .unwrap();
        let gains = session_skill_gains(&pool, "s1").await.unwrap();
        assert_eq!(
            to_wire_json(&gains),
            "[{\"skillName\":\"Laser Weaponry Technology\",\"level\":0.0,\"ttValueGained\":3.0}]"
        );
    }

    #[tokio::test]
    async fn tag_suggestions_filters_to_speciesless_mobs() {
        let pool = memory_pool().await;
        seed(&pool).await;
        // Species-bearing Atrox is excluded; the bare tag matches case-insensitively.
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "At", 10).await.unwrap()),
            "[\"Atrocious Tag\"]"
        );
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "atro", 10).await.unwrap()),
            "[\"Atrocious Tag\"]"
        );
        // Empty / whitespace query short-circuits to [].
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "", 10).await.unwrap()),
            "[]"
        );
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "   ", 10).await.unwrap()),
            "[]"
        );
        // Leading/trailing whitespace is stripped before matching.
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "  At  ", 10).await.unwrap()),
            "[\"Atrocious Tag\"]"
        );
        // No match.
        assert_eq!(
            to_wire_json(&tag_suggestions_impl(&pool, "zzz", 10).await.unwrap()),
            "[]"
        );
    }

    #[tokio::test]
    async fn active_session_duration_reads_the_clock() {
        let pool = memory_pool().await;
        let start = 1_000_000.0_f64;
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind("act")
        .bind(start)
        .bind(Option::<f64>::None)
        .bind(1_i64)
        .bind(0.0)
        .bind(0.0)
        .bind(0.0)
        .bind("mob")
        .bind(start)
        .execute(&pool)
        .await
        .unwrap();
        let value = list_sessions_impl(&pool, start + 120.0).await.unwrap();
        assert!(to_wire_json(&value).contains("\"duration\":120"));
        // endTime is null for an active session.
        assert!(to_wire_json(&value).contains("\"endTime\":null"));
    }

    // ── Session-edit hermetic pins ──
    //
    // These exercise the edit impls directly against an in-memory pool;
    // the committed goldens hold the same surface byte-for-byte (the
    // cross-language oracle that first proved it has been retired).

    /// An ended session with three kills (two `Atrox`, one `Foul`), one
    /// of the Atrox already renamed from `Daikiba`, plus active loot
    /// (`Animal Hide` on both Atrox, a slash-bearing `Metal/Residue` on
    /// Foul) and a `session_summaries` cache row to watch invalidate.
    async fn seed_edit(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) VALUES('ended',1000.0,4600.0,0,5.0,0,0,'mob',4600.0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) VALUES('act',1000.0,NULL,1,0,0,0,'mob',1000.0)",
        )
        .execute(pool)
        .await
        .unwrap();
        for (id, mob, ts, loot, orig) in [
            ("k1", "Atrox", 1001.0, 10.0, None),
            ("k2", "Atrox", 1002.0, 20.0, Some("Daikiba")),
            ("k3", "Foul", 1003.0, 5.0, None),
        ] {
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,timestamp,loot_total_ped,original_mob_name) \
                 VALUES(?,?,?,?,?,?)",
            )
            .bind(id)
            .bind("ended")
            .bind(mob)
            .bind(ts)
            .bind(loot)
            .bind(orig)
            .execute(pool)
            .await
            .unwrap();
        }
        for (kid, item, qty, val) in [
            ("k1", "Animal Hide", 2_i64, 3.0),
            ("k2", "Animal Hide", 1, 1.5),
            ("k3", "Metal/Residue", 1, 2.25),
        ] {
            sqlx::query(
                "INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,\
                 is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,0,NULL)",
            )
            .bind(kid)
            .bind(item)
            .bind(qty)
            .bind(val)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO session_summaries(session_id,computed_at) VALUES('ended',1.0)")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn summary_exists(pool: &SqlitePool) -> bool {
        sqlx::query("SELECT 1 FROM session_summaries WHERE session_id = 'ended'")
            .fetch_optional(pool)
            .await
            .unwrap()
            .is_some()
    }

    async fn original_of(pool: &SqlitePool, kill_id: &str) -> Option<String> {
        sqlx::query("SELECT original_mob_name FROM kills WHERE id = ?")
            .bind(kill_id)
            .fetch_one(pool)
            .await
            .unwrap()
            .get::<Option<String>, _>(0)
    }

    async fn loot_state(pool: &SqlitePool, kill_id: &str, item: &str) -> (bool, f64) {
        let row =
            sqlx::query("SELECT deactivated_at, value_ped FROM kill_loot_items WHERE kill_id = ? AND item_name = ?")
                .bind(kill_id)
                .bind(item)
                .fetch_one(pool)
                .await
                .unwrap();
        (
            row.try_get::<Option<f64>, _>(0).ok().flatten().is_some(),
            row.get::<f64, _>(1),
        )
    }

    async fn kill_loot_total(pool: &SqlitePool, kill_id: &str) -> f64 {
        as_f64(&sql_number(
            &sqlx::query("SELECT loot_total_ped FROM kills WHERE id = ?")
                .bind(kill_id)
                .fetch_one(pool)
                .await
                .unwrap(),
            0,
        ))
    }

    fn assert_http(result: Result<Value, EditError>, status: StatusCode, message: &str) {
        match result {
            Err(EditError::Http(got_status, got_message)) => {
                assert_eq!(got_status, status);
                assert_eq!(got_message, message);
            }
            other => panic!("expected Http({status}, {message:?}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rename_mob_preserves_first_original_and_invalidates_summary() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        let value = rename_session_mob_impl(&pool, "ended", "Atrox", "Argo")
            .await
            .unwrap();
        assert_eq!(
            to_wire_json(&value),
            "{\"sessionId\":\"ended\",\"mobName\":\"Argo\",\"killCount\":2}"
        );
        // k1 had no original -> COALESCE records Atrox; k2 keeps the
        // genuinely-first Daikiba.
        assert_eq!(original_of(&pool, "k1").await.as_deref(), Some("Atrox"));
        assert_eq!(original_of(&pool, "k2").await.as_deref(), Some("Daikiba"));
        assert!(!summary_exists(&pool).await, "summary invalidated");
    }

    #[tokio::test]
    async fn rename_mob_round_trip_clears_the_preservation_column() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // Atrox -> Argo, then Argo -> Daikiba on k2 lands back at its
        // genuine original, clearing original_mob_name via the CASE.
        rename_session_mob_impl(&pool, "ended", "Atrox", "Argo")
            .await
            .unwrap();
        // Only k2 had Daikiba preserved; rename the cohort to Daikiba.
        // k1's original was Atrox (not Daikiba) so it stays set.
        rename_session_mob_impl(&pool, "ended", "Argo", "Daikiba")
            .await
            .unwrap();
        assert_eq!(
            original_of(&pool, "k2").await,
            None,
            "round-trip to the genuine original clears preservation"
        );
        assert_eq!(original_of(&pool, "k1").await.as_deref(), Some("Atrox"));
    }

    #[tokio::test]
    async fn rename_mob_error_legs() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        assert_http(
            rename_session_mob_impl(&pool, "nope", "Atrox", "Argo").await,
            StatusCode::NOT_FOUND,
            "Session not found",
        );
        assert_http(
            rename_session_mob_impl(&pool, "act", "Atrox", "Argo").await,
            StatusCode::CONFLICT,
            "Session mob edits are only available after the session has ended",
        );
        assert_http(
            rename_session_mob_impl(&pool, "ended", "Atrox", "Atrox").await,
            StatusCode::CONFLICT,
            "rename target matches the current value (no-op)",
        );
        assert_http(
            rename_session_mob_impl(&pool, "ended", "  ", "Argo").await,
            StatusCode::BAD_REQUEST,
            "Mob names cannot be blank",
        );
        assert_http(
            rename_session_mob_impl(&pool, "ended", "Zzz", "Argo").await,
            StatusCode::CONFLICT,
            "No kills in this session match mob_name='Zzz'",
        );
        // A failed precondition leaves no side effects.
        assert!(summary_exists(&pool).await);
    }

    #[tokio::test]
    async fn restore_mob_clean_and_ambiguous() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // Rename both Atrox -> Argo: now k1 original=Atrox, k2 original=Daikiba.
        rename_session_mob_impl(&pool, "ended", "Atrox", "Argo")
            .await
            .unwrap();
        // Two distinct originals merged into Argo -> ambiguous 409.
        assert_http(
            restore_session_mob_impl(&pool, "ended", "Argo").await,
            StatusCode::CONFLICT,
            "Ambiguous restore for mob_name='Argo': 2 distinct prior names merged into it.",
        );
        // Nothing to restore for a name with no preserved original.
        assert_http(
            restore_session_mob_impl(&pool, "ended", "Foul").await,
            StatusCode::CONFLICT,
            "No restorable kills in this session for mob_name='Foul' \
             (either no rename has happened or the preservation column is empty)",
        );
    }

    #[tokio::test]
    async fn restore_mob_clean_reverts() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // Give both Atrox the SAME original so the restore is unambiguous.
        sqlx::query(
            "UPDATE kills SET mob_name='Argo', original_mob_name='Wolf' WHERE mob_name='Atrox'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let value = restore_session_mob_impl(&pool, "ended", "Argo")
            .await
            .unwrap();
        assert_eq!(
            to_wire_json(&value),
            "{\"sessionId\":\"ended\",\"mobName\":\"Wolf\",\"killCount\":2}"
        );
        assert_eq!(original_of(&pool, "k1").await, None);
        assert_eq!(original_of(&pool, "k2").await, None);
        assert!(!summary_exists(&pool).await);
    }

    #[tokio::test]
    async fn loot_deactivate_recomputes_totals_and_response() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        let value = bulk_flip_loot_item(&pool, "ended", "Animal Hide", "deactivated")
            .await
            .unwrap();
        // delta = -(3.0 + 1.5) = -4.5; session returns = 7.0 + 18.5 + 5.0.
        assert_eq!(
            to_wire_json(&value),
            "{\"sessionId\":\"ended\",\"itemName\":\"Animal Hide\",\"affectedRows\":2,\
             \"totalValueDelta\":-4.5,\"sessionTotalReturns\":30.5}"
        );
        assert!(
            loot_state(&pool, "k1", "Animal Hide").await.0,
            "deactivated"
        );
        assert_eq!(kill_loot_total(&pool, "k1").await, 7.0);
        assert_eq!(kill_loot_total(&pool, "k2").await, 18.5);
        assert!(!summary_exists(&pool).await);
    }

    #[tokio::test]
    async fn loot_activate_is_the_inverse() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        bulk_flip_loot_item(&pool, "ended", "Animal Hide", "deactivated")
            .await
            .unwrap();
        let value = bulk_flip_loot_item(&pool, "ended", "Animal Hide", "active")
            .await
            .unwrap();
        assert_eq!(
            to_wire_json(&value),
            "{\"sessionId\":\"ended\",\"itemName\":\"Animal Hide\",\"affectedRows\":2,\
             \"totalValueDelta\":4.5,\"sessionTotalReturns\":35.0}"
        );
        assert!(
            !loot_state(&pool, "k1", "Animal Hide").await.0,
            "reactivated"
        );
        assert_eq!(kill_loot_total(&pool, "k1").await, 10.0);
    }

    #[tokio::test]
    async fn loot_slash_item_name_flows_through() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // The `:path` item name carries a literal slash end to end.
        let value = bulk_flip_loot_item(&pool, "ended", "Metal/Residue", "deactivated")
            .await
            .unwrap();
        assert!(
            to_wire_json(&value).contains("\"itemName\":\"Metal/Residue\""),
            "{}",
            to_wire_json(&value)
        );
        assert!(loot_state(&pool, "k3", "Metal/Residue").await.0);
    }

    #[tokio::test]
    async fn loot_error_legs() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        assert_http(
            bulk_flip_loot_item(&pool, "nope", "Animal Hide", "deactivated").await,
            StatusCode::NOT_FOUND,
            "Session not found",
        );
        assert_http(
            bulk_flip_loot_item(&pool, "act", "Animal Hide", "deactivated").await,
            StatusCode::CONFLICT,
            "Session mob edits are only available after the session has ended",
        );
        assert_http(
            bulk_flip_loot_item(&pool, "ended", "Nonexist", "deactivated").await,
            StatusCode::NOT_FOUND,
            "No loot named 'Nonexist' in this session",
        );
        // Already active -> activate finds nothing in the opposite state.
        assert_http(
            bulk_flip_loot_item(&pool, "ended", "Animal Hide", "active").await,
            StatusCode::CONFLICT,
            "All 'Animal Hide' rows in this session are already active",
        );
        assert_http(
            bulk_flip_loot_item(&pool, "ended", "  ", "deactivated").await,
            StatusCode::BAD_REQUEST,
            "Item name cannot be blank",
        );
    }

    #[tokio::test]
    async fn armour_cost_adds_and_echoes_the_submitted_value() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // Echoes round(cost, 2) (the submitted value), NOT the new total.
        let value = set_armour_cost_impl(&pool, "ended", 2.5).await.unwrap();
        assert_eq!(
            to_wire_json(&value),
            "{\"sessionId\":\"ended\",\"armourCost\":2.5}"
        );
        let total = as_f64(&sql_number(
            &sqlx::query("SELECT armour_cost FROM tracking_sessions WHERE id='ended'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
        ));
        assert_eq!(total, 7.5, "5.0 + 2.5 accumulates");
        // Integer cost coerces to a float on the wire.
        assert_eq!(
            to_wire_json(&set_armour_cost_impl(&pool, "ended", 3.0).await.unwrap()),
            "{\"sessionId\":\"ended\",\"armourCost\":3.0}"
        );
        // Banker's rounding on the echo.
        assert_eq!(
            to_wire_json(&set_armour_cost_impl(&pool, "ended", 2.675).await.unwrap()),
            "{\"sessionId\":\"ended\",\"armourCost\":2.67}"
        );
    }

    #[tokio::test]
    async fn armour_cost_has_no_active_guard_and_404s_when_absent() {
        let pool = memory_pool().await;
        seed_edit(&pool).await;
        // Active session: armour-cost still succeeds (no _validate guard).
        assert_eq!(
            to_wire_json(&set_armour_cost_impl(&pool, "act", 1.0).await.unwrap()),
            "{\"sessionId\":\"act\",\"armourCost\":1.0}"
        );
        assert_http(
            set_armour_cost_impl(&pool, "nope", 1.0).await,
            StatusCode::NOT_FOUND,
            "Session not found",
        );
    }
}
