//! The tracking read/edit computation: the session list and detail
//! reads, the tag suggestions, the post-hoc session edits, and the
//! snapshot's trifecta attribution summary, over the shared database.
//!
//! The shaping here produces the wire `serde_json::Value` forms the
//! facade's DTOs pin byte-for-byte: the exclude-none projections, the
//! engine-typed numeric reads, and the string renderings are contract
//! lineage (pinned by the frozen goldens and the facade byte-pin
//! tests; ADR-0017/0019), so the `Value` edge is deliberate.

use std::collections::BTreeMap;

use chrono::DateTime;
use serde_json::{json, Map, Value};

use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::config_service::{active_trifecta_preset, AppConfig};
use crate::db::{Db, DbError};
use crate::time::to_iso_utc;
use eo_wire::normalizer::round_half_even;

// ── Engine-typed numeric primitives ─────────────────────────────────

/// A SQLite numeric read preserving the engine type: a REAL decodes to a
/// float, an INTEGER (the `COALESCE(SUM(...), 0)` empty case) to an integer.
/// The stored value's affinity (`ValueRef`) drives the branch directly,
/// preserving the same behaviour as before: an integer value never
/// masquerades as a float.
pub fn sql_number(row: &rusqlite::Row, index: usize) -> Value {
    match row.get_ref_unwrap(index) {
        rusqlite::types::ValueRef::Real(value) => json!(value),
        value => json!(value.as_i64().expect("sql_number reads a numeric column")),
    }
}

/// `float(value)` over an engine-typed number.
pub fn as_f64(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

/// `round(value, places)`: banker's rounding, always producing a float.
pub fn round(value: f64, places: usize) -> f64 {
    round_half_even(value, places)
}

/// A model-declared `float` field: coerce an engine-typed integer to its
/// float form, so an integer zero leaves the wire as `0.0`.
pub fn float_field(value: Value) -> Value {
    match value.as_i64() {
        Some(integer) => json!(integer as f64),
        None => value,
    }
}

/// `datetime.fromtimestamp(ts, tz=UTC).isoformat()` (the session-list
/// start / end times). Emits `+00:00` and 6-digit microseconds only when
/// the fraction is non-zero; `None` maps to JSON null.
pub fn list_ts_to_iso(ts: Option<f64>) -> Value {
    let Some(ts) = ts else {
        return Value::Null;
    };
    let frac = ts.fract();
    let whole = ts.trunc() as i64;
    let mut micros = round_half_even(frac * 1_000_000.0, 0) as i64;
    let mut secs = whole;
    if micros >= 1_000_000 {
        secs += 1;
        micros -= 1_000_000;
    } else if micros < 0 {
        secs -= 1;
        micros += 1_000_000;
    }
    let dt =
        DateTime::from_timestamp(secs, (micros as u32) * 1_000).expect("timestamp within range");
    let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros == 0 {
        json!(format!("{base}+00:00"))
    } else {
        json!(format!("{base}.{micros:06}+00:00"))
    }
}

/// `_ts_to_iso` (the snapshot's recent-event timestamps): a Unix timestamp
/// to the `+00:00`-suffixed ISO string the domain events stamp, or null.
pub fn event_ts_to_iso(ts: Option<f64>) -> Value {
    match ts {
        Some(ts) => Value::String(to_iso_utc(ts)),
        None => Value::Null,
    }
}

/// Duration in whole seconds: stored span for an ended session, the clock's
/// running span for an active one, else zero.
pub fn duration_seconds(
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

/// `_notable_event_category`: the `type` field of a notable event.
pub fn notable_event_category(event_type: &str) -> &'static str {
    if event_type.starts_with("quest_") {
        "quest"
    } else if event_type.starts_with("hof_") {
        "hof"
    } else {
        "global"
    }
}

// ── Session list ────────────────────────────────────────────────────

/// The session-list columns read from `session_summaries`.
struct ListSummary {
    weapon_cost: f64,
    heal_cost: f64,
    enhancer_cost: f64,
    armour_cost: f64,
    dangling_cost: f64,
    harvest_cost: f64,
    loot_tt: f64,
    primary_mobs: Value,
    primary_weapons: Value,
    globals: i64,
    hofs: i64,
}

/// The default session page size when the client names no `limit`.
const SESSION_PAGE_DEFAULT: i64 = 50;
/// The largest session page a client may request; larger `limit` values
/// clamp here, bounding the work a single request can ask for.
const SESSION_PAGE_MAX: i64 = 200;

/// One keyset page of session-list rows plus the cursor for the next page
/// (`None` on the last page) and the whole-table session count (so a
/// pager can report true bounds while loading windows on demand).
pub struct SessionListPage {
    pub sessions: Value,
    pub next_cursor: Option<String>,
    pub total: i64,
}

/// The opaque keyset cursor over `[started_at, id]` of the last row on a
/// page (the shared [`crate::keyset`] codec).
fn encode_session_cursor(started_at: f64, id: &str) -> String {
    crate::keyset::encode_cursor(&(started_at, id))
}

/// Decode a keyset cursor back to its `(started_at, id)` seek key, or
/// `None` for a malformed token (which the facade answers as a bad
/// request).
pub fn decode_session_cursor(token: &str) -> Option<(f64, String)> {
    crate::keyset::decode_cursor(token)
}

pub async fn list_sessions_impl(
    db: &Db,
    now: f64,
    seek: Option<(f64, String)>,
    limit: Option<i64>,
    definition_id: Option<i64>,
) -> Result<SessionListPage, DbError> {
    // Heal so ended sessions carry current summaries (a write on the read
    // path, preserved), then read each page session's row as one
    // synchronous unit on a reader-core connection.
    db.with_writer(|conn| crate::session_summary::heal_summaries(conn))
        .await?;
    db.with_reader(move |conn| list_sessions_read(conn, now, seek.as_ref(), limit, definition_id))
        .await
}

/// The session-list read proper: keyset (seek) pagination newest first
/// over `(started_at DESC, id DESC)`, each page row shaped from its
/// summary (ended) or the raw tables. One synchronous pass over a
/// reader-core connection; one extra row is fetched to detect a further
/// page.
///
/// `definition_id` narrows the read to one definition's instances (the
/// review surface's scope). It narrows the count as well as the rows, so
/// a scoped pager reports the scope's own bounds rather than the whole
/// table's.
pub fn list_sessions_read(
    conn: &rusqlite::Connection,
    now: f64,
    seek: Option<&(f64, String)>,
    limit: Option<i64>,
    definition_id: Option<i64>,
) -> Result<SessionListPage, DbError> {
    let page = limit
        .unwrap_or(SESSION_PAGE_DEFAULT)
        .clamp(1, SESSION_PAGE_MAX);
    let total: i64 = match definition_id {
        Some(id) => conn.query_row(
            "SELECT COUNT(*) FROM tracking_sessions WHERE definition_id = ?",
            rusqlite::params![id],
            |row| row.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
            row.get(0)
        })?,
    };

    // Both predicates are optional and compose, so the seek's own `OR`
    // is parenthesised: unwrapped it would bind looser than the AND and
    // silently widen a scoped page back to the whole table.
    let mut sql = String::from("SELECT id, started_at, ended_at, is_active FROM tracking_sessions");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut clauses: Vec<&str> = Vec::new();
    if let Some(id) = definition_id {
        clauses.push("definition_id = ?");
        params.push(Box::new(id));
    }
    // id-order: tiebreak (the seek is by started_at; id splits equal instants).
    if let Some((started_at, id)) = seek {
        clauses.push("(started_at < ? OR (started_at = ? AND id < ?))");
        params.push(Box::new(*started_at));
        params.push(Box::new(*started_at));
        params.push(Box::new(id.clone()));
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT ?");
    params.push(Box::new(page + 1));

    let meta: Vec<(String, Option<f64>, Option<f64>, bool)> = {
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<_> {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, i64>(3)? != 0,
            ))
        };
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    // A full extra row means another page follows: drop it and cut the
    // next cursor from the last row actually served.
    let has_more = meta.len() as i64 > page;
    let kept = if has_more {
        &meta[..page as usize]
    } else {
        &meta[..]
    };

    let ids: Vec<String> = kept.iter().map(|(id, ..)| id.clone()).collect();
    let summaries = fetch_list_summaries(conn, &ids)?;

    let mut sessions = Vec::with_capacity(kept.len());
    for (sid, started_at, ended_at, is_active) in kept {
        let session = match summaries.get(sid) {
            Some(summary) if !*is_active => {
                list_row_from_summary(sid, *started_at, *ended_at, *is_active, now, summary)
            }
            _ => list_row_from_raw(conn, sid, *started_at, *ended_at, *is_active, now)?,
        };
        sessions.push(session);
    }
    let next_cursor = has_more
        .then(|| kept.last())
        .flatten()
        .map(|(id, started_at, ..)| encode_session_cursor(started_at.unwrap_or(0.0), id));
    Ok(SessionListPage {
        sessions: Value::Array(sessions),
        next_cursor,
        total,
    })
}

fn fetch_list_summaries(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<std::collections::HashMap<String, ListSummary>, DbError> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT session_id, weapon_cost, heal_cost, enhancer_cost, armour_cost, dangling_cost, \
         harvest_cost, loot_tt, primary_mobs_json, primary_weapons_json, globals, hofs \
         FROM session_summaries WHERE session_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(ids))?;
    let mut out = std::collections::HashMap::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        let sid = row.get::<_, String>(0)?;
        out.insert(
            sid,
            ListSummary {
                weapon_cost: as_f64(&sql_number(row, 1)),
                heal_cost: as_f64(&sql_number(row, 2)),
                enhancer_cost: as_f64(&sql_number(row, 3)),
                armour_cost: as_f64(&sql_number(row, 4)),
                dangling_cost: as_f64(&sql_number(row, 5)),
                harvest_cost: as_f64(&sql_number(row, 6)),
                loot_tt: as_f64(&sql_number(row, 7)),
                primary_mobs: parse_string_array(&row.get::<_, String>(8)?),
                primary_weapons: parse_string_array(&row.get::<_, String>(9)?),
                globals: row.get::<_, i64>(10)?,
                hofs: row.get::<_, i64>(11)?,
            },
        );
    }
    Ok(out)
}

pub fn parse_string_array(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::Array(Vec::new()))
}

fn list_row_from_summary(
    session_id: &str,
    started_at: Option<f64>,
    ended_at: Option<f64>,
    is_active: bool,
    now: f64,
    summary: &ListSummary,
) -> Value {
    let duration = duration_seconds(started_at, ended_at, is_active, now);
    // Cost mirrors the summary's own cycled composition (weapon + heal +
    // enhancer + armour + dangling + harvest swing decay), matching the
    // detail read; loot_tt already folds harvest loot in.
    let cost = summary.weapon_cost
        + summary.heal_cost
        + summary.enhancer_cost
        + summary.armour_cost
        + summary.dangling_cost
        + summary.harvest_cost;
    let returns = summary.loot_tt;
    let net = returns - cost;
    let return_rate = if cost > 0.0 { returns / cost } else { 0.0 };
    json!({
        "id": session_id,
        "startTime": list_ts_to_iso(started_at),
        "endTime": list_ts_to_iso(ended_at),
        "duration": duration,
        "primaryMobs": summary.primary_mobs,
        "primaryWeapons": summary.primary_weapons,
        "cost": round(cost, 2),
        "returns": round(returns, 2),
        "net": round(net, 2),
        "returnRate": round(return_rate, 4),
        "globals": summary.globals,
        "hofs": summary.hofs,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn list_row_from_raw(
    conn: &rusqlite::Connection,
    session_id: &str,
    started_at: Option<f64>,
    ended_at: Option<f64>,
    is_active: bool,
    now: f64,
) -> Result<Value, DbError> {
    let duration = duration_seconds(started_at, ended_at, is_active, now);

    let weapon_cost = scalar(
        conn,
        "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
         FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE k.session_id = ?",
        session_id,
    )?;
    let enhancer_cost = scalar(
        conn,
        "SELECT COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE k.session_id = ?",
        session_id,
    )?;
    let (armour_cost, heal_cost, dangling_cost) = conn.query_row(
        "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) \
         FROM tracking_sessions WHERE id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                as_f64(&sql_number(row, 0)),
                as_f64(&sql_number(row, 1)),
                as_f64(&sql_number(row, 2)),
            ))
        },
    )?;
    // Harvest loot and swing decay join the raw shape symmetrically (the
    // summary path and the detail read both carry them).
    let (harvest_loot, harvest_cost) = conn.query_row(
        "SELECT COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(cost_ped), 0) \
         FROM harvest_events WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| Ok((as_f64(&sql_number(row, 0)), as_f64(&sql_number(row, 1)))),
    )?;
    let weapon_cost = as_f64(&weapon_cost);
    let enhancer_cost = as_f64(&enhancer_cost);
    let cost = weapon_cost + heal_cost + enhancer_cost + armour_cost + dangling_cost + harvest_cost;

    let returns = as_f64(&scalar(
        conn,
        "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
        session_id,
    )?) + harvest_loot;

    let primary_mobs = string_column(
        conn,
        "SELECT mob_name FROM kills \
         WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' \
         GROUP BY mob_name ORDER BY COUNT(*) DESC LIMIT 3",
        session_id,
    )?;
    let primary_weapons = string_column(
        conn,
        "SELECT ts.tool_name FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
         GROUP BY ts.tool_name ORDER BY SUM(ts.shots_fired) DESC LIMIT 3",
        session_id,
    )?;

    let net = returns - cost;
    let return_rate = if cost > 0.0 { returns / cost } else { 0.0 };

    let (globals, hofs) = conn.query_row(
        "SELECT \
           COALESCE(SUM(CASE WHEN event_type LIKE 'global_%' THEN 1 ELSE 0 END), 0), \
           COALESCE(SUM(CASE WHEN event_type LIKE 'hof_%' THEN 1 ELSE 0 END), 0) \
         FROM notable_events WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;

    Ok(json!({
        "id": session_id,
        "startTime": list_ts_to_iso(started_at),
        "endTime": list_ts_to_iso(ended_at),
        "duration": duration,
        "primaryMobs": primary_mobs,
        "primaryWeapons": primary_weapons,
        "cost": round(cost, 2),
        "returns": round(returns, 2),
        "net": round(net, 2),
        "returnRate": round(return_rate, 4),
        "globals": globals,
        "hofs": hofs,
    }))
}

pub fn scalar(conn: &rusqlite::Connection, sql: &str, sid: &str) -> Result<Value, DbError> {
    Ok(conn.query_row(sql, rusqlite::params![sid], |row| Ok(sql_number(row, 0)))?)
}

pub fn string_column(
    conn: &rusqlite::Connection,
    sql: &str,
    sid: &str,
) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![sid], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ── Session detail ──────────────────────────────────────────────────

pub async fn get_session_impl(
    db: &Db,
    session_id: &str,
    now: f64,
) -> Result<Option<Value>, DbError> {
    let sid = session_id.to_string();
    db.with_reader(move |conn| get_session_read(conn, &sid, now))
        .await
}

/// One session's full detail, read in one synchronous pass over a reader-core
/// connection; an absent session is `None`.
pub fn get_session_read(
    conn: &rusqlite::Connection,
    session_id: &str,
    now: f64,
) -> Result<Option<Value>, DbError> {
    use rusqlite::OptionalExtension as _;

    let session_meta = conn
        .query_row(
            "SELECT id, started_at, ended_at, is_active, mob_tracking_mode, session_name \
             FROM tracking_sessions WHERE id = ?",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((started_at, ended_at, is_active, mob_mode, session_name)) = session_meta else {
        return Ok(None);
    };

    // The column is free text; anything outside the two modes (a
    // hand-edited row, a legacy value) recovers to the same "mob"
    // default an absent value takes, so the closed wire vocabulary
    // holds for every stored shape.
    let mob_entry_mode = mob_mode
        .filter(|m| m == "tag" || m == "mob")
        .unwrap_or_else(|| "mob".to_string());

    let duration = duration_seconds(started_at, ended_at, is_active, now);

    let (armour_cost, session_heal_cost, dangling_cost) = conn.query_row(
        "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) \
         FROM tracking_sessions WHERE id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                as_f64(&sql_number(row, 0)),
                as_f64(&sql_number(row, 1)),
                as_f64(&sql_number(row, 2)),
            ))
        },
    )?;

    let (kills, total_returns, total_enhancer_cost) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) \
         FROM kills WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                as_f64(&sql_number(row, 1)),
                as_f64(&sql_number(row, 2)),
            ))
        },
    )?;

    let merged_tools: Vec<(String, i64, f64, i64, f64)> = {
        let mut stmt = conn.prepare(
            "SELECT t.tool_name, SUM(t.shots_fired), SUM(t.damage_dealt), SUM(t.critical_hits), \
             SUM(COALESCE(t.cost_per_shot, 0) * COALESCE(t.shots_fired, 0)) \
             FROM kill_tool_stats t JOIN kills k ON k.id = t.kill_id \
             WHERE k.session_id = ? GROUP BY t.tool_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1).unwrap_or(0),
                as_f64(&sql_number(row, 2)),
                row.get::<_, i64>(3).unwrap_or(0),
                as_f64(&sql_number(row, 4)),
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let weapon_cost: f64 = merged_tools
        .iter()
        .map(|(_, _, _, _, cost_attr)| cost_attr)
        .sum();

    // Harvesting swings join the session economy: wood TT is loot,
    // swing decay is cost.
    let (harvest_swings, harvest_successes, harvest_loot, harvest_cost) = conn.query_row(
        "SELECT COUNT(*), \
           COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
           COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(cost_ped), 0) \
         FROM harvest_events WHERE session_id = ?",
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                as_f64(&sql_number(row, 2)),
                as_f64(&sql_number(row, 3)),
            ))
        },
    )?;
    let total_returns = total_returns + harvest_loot;

    let merged_loot = merge_loot_aggs(
        loot_agg(conn, session_id, "l.deactivated_at IS NULL")?,
        harvest_loot_agg(conn, session_id, "l.deactivated_at IS NULL")?,
    );
    let merged_deactivated_loot = merge_loot_aggs(
        loot_agg(conn, session_id, "l.deactivated_at IS NOT NULL")?,
        harvest_loot_agg(conn, session_id, "l.deactivated_at IS NOT NULL")?,
    );

    let mob_breakdown: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT mob_name, original_mob_name, COUNT(*) FROM kills \
             WHERE session_id = ? AND mob_name IS NOT NULL \
             GROUP BY mob_name, original_mob_name ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok(json!({
                "currentName": row.get::<_, String>(0)?,
                "originalName": row.get::<_, Option<String>>(1)?,
                "killCount": row.get::<_, i64>(2)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let total_cost = weapon_cost
        + session_heal_cost
        + total_enhancer_cost
        + armour_cost
        + dangling_cost
        + harvest_cost;

    let detail_skill_tt = as_f64(&scalar(
        conn,
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
        session_id,
    )?);

    let net = total_returns - total_cost;
    let return_rate = if total_cost > 0.0 {
        total_returns / total_cost
    } else {
        0.0
    };

    let loot_breakdown = loot_breakdown_sorted(&merged_loot);
    let deactivated_loot_breakdown = loot_breakdown_sorted(&merged_deactivated_loot);

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

    let notable_events: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT event_type, mob_or_item, value_ped FROM notable_events \
             WHERE session_id = ? ORDER BY timestamp",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            let event_type = row.get::<_, String>(0)?;
            let mob_or_item = row.get::<_, Option<String>>(1)?;
            let value = sql_number(row, 2);
            Ok(json!({
                "type": notable_event_category(&event_type),
                "eventType": event_type,
                "target": mob_or_item,
                "item": mob_or_item,
                "value": float_field(value),
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let skill_gains = session_skill_gains(conn, session_id)?;

    let healing_activations: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.tool_name, a.observed_at, a.cost_ped, a.provenance, \
                    (SELECT MAX(w.expires_at) FROM healing_effect_windows w \
                     WHERE w.activation_id = a.id), \
                    (SELECT COUNT(*) FROM healing_outputs o \
                     WHERE o.activation_id = a.id) \
             FROM healing_activations a WHERE a.session_id = ? \
             ORDER BY a.observed_at, a.id",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "toolName": row.get::<_, String>(1)?,
                "observedAt": row.get::<_, f64>(2)?,
                "cost": row.get::<_, f64>(3)?,
                "provenance": row.get::<_, String>(4)?,
                "effectUntil": row.get::<_, Option<f64>>(5)?,
                "outputCount": row.get::<_, i64>(6)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let (healing_outputs, direct_outputs, effect_outputs, passive_outputs, unattributed_outputs) =
        conn.query_row(
            "SELECT COUNT(*), \
                    COALESCE(SUM(classification = 'direct'), 0), \
                    COALESCE(SUM(classification = 'effect'), 0), \
                    COALESCE(SUM(classification = 'passive'), 0), \
                    COALESCE(SUM(classification = 'unattributed'), 0) \
             FROM healing_outputs WHERE session_id = ?",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
    let healing_activation_count = healing_activations.len();

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
                "harvestCost": round(harvest_cost, 2),
            },
        },
        "harvest": {
            "swings": harvest_swings,
            "successes": harvest_successes,
            "lootTt": round(harvest_loot, 2),
            "cost": round(harvest_cost, 2),
        },
        "sessionName": session_name,
        "mobEntryMode": mob_entry_mode,
        "notableEvents": notable_events,
        "lootBreakdown": loot_breakdown,
        "deactivatedLootBreakdown": deactivated_loot_breakdown,
        "mobBreakdown": mob_breakdown,
        "effectiveLoot": round(total_returns, 2),
        "toolStats": tool_stats,
        "skillGains": skill_gains,
        "healing": {
            "activations": healing_activations,
            "activationCount": healing_activation_count,
            "outputCount": healing_outputs,
            "directOutputs": direct_outputs,
            "effectOutputs": effect_outputs,
            "passiveOutputs": passive_outputs,
            "unattributedOutputs": unattributed_outputs,
        },
    })))
}

pub fn loot_agg(
    conn: &rusqlite::Connection,
    session_id: &str,
    deactivated_clause: &str,
) -> Result<Vec<(String, i64, f64)>, DbError> {
    let sql = format!(
        "SELECT l.item_name, SUM(l.quantity), SUM(l.value_ped) \
         FROM kill_loot_items l JOIN kills k ON k.id = l.kill_id \
         WHERE k.session_id = ? AND COALESCE(l.is_enhancer_shrapnel, 0) = 0 AND {deactivated_clause} \
         GROUP BY l.item_name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1).unwrap_or(0),
            as_f64(&sql_number(row, 2)),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The harvest half of the session loot aggregation: same shape as
/// [`loot_agg`], over the harvest tables (no shrapnel concept there).
pub fn harvest_loot_agg(
    conn: &rusqlite::Connection,
    session_id: &str,
    deactivated_clause: &str,
) -> Result<Vec<(String, i64, f64)>, DbError> {
    let sql = format!(
        "SELECT l.item_name, SUM(l.quantity), SUM(l.value_ped) \
         FROM harvest_loot_items l JOIN harvest_events h ON h.id = l.harvest_id \
         WHERE h.session_id = ? AND {deactivated_clause} \
         GROUP BY l.item_name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1).unwrap_or(0),
            as_f64(&sql_number(row, 2)),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Merge two per-item aggregations by item name (kill loot + harvest
/// loot into one session loot list), preserving the first list's order
/// for shared names and appending the second's new names.
pub fn merge_loot_aggs(
    mut base: Vec<(String, i64, f64)>,
    extra: Vec<(String, i64, f64)>,
) -> Vec<(String, i64, f64)> {
    for (name, qty, val) in extra {
        match base.iter_mut().find(|(existing, _, _)| *existing == name) {
            Some(entry) => {
                entry.1 += qty;
                entry.2 += val;
            }
            None => base.push((name, qty, val)),
        }
    }
    base
}

pub fn loot_breakdown_sorted(rows: &[(String, i64, f64)]) -> Vec<Value> {
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

pub fn session_skill_gains(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Value, DbError> {
    let attr_placeholders = vec!["?"; ATTRIBUTE_SKILLS.len()].join(",");
    let sql = format!(
        "SELECT sg.skill_name, SUM(sg.amount) as total_amount, \
         COALESCE(SUM(sg.ped_value), 0) as total_ped \
         FROM skill_gains sg WHERE sg.session_id = ? \
         AND sg.skill_name NOT IN ({attr_placeholders}) \
         GROUP BY sg.skill_name ORDER BY total_ped DESC"
    );
    // Each row's skill name (index 0) and engine-typed total_ped (index 2);
    // total_amount (index 1) is unread, kept in the projection for SQL parity.
    let rows: Vec<(String, Value)> = {
        let mut params: Vec<&str> = Vec::with_capacity(1 + ATTRIBUTE_SKILLS.len());
        params.push(session_id);
        params.extend(ATTRIBUTE_SKILLS);
        let mut stmt = conn.prepare(&sql)?;
        let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok((row.get::<_, String>(0)?, sql_number(row, 2)))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.is_empty() {
        return Ok(json!([]));
    }

    // The current level per gained skill is the latest calibration in
    // time, never the highest id: a calibration recorded later from an
    // older screenshot must not displace a newer reading.
    let skill_names: Vec<String> = rows.iter().map(|(name, _)| name.clone()).collect();
    let levels: BTreeMap<String, f64> =
        crate::db::latest_skill_levels(conn, None, Some(&skill_names))?
            .into_iter()
            .collect();

    let gains: Vec<Value> = rows
        .iter()
        .map(|(name, total_ped)| {
            let level = match levels.get(name) {
                Some(level) => float_field(json!(round(*level, 1))),
                None => json!(0.0),
            };
            let tt = as_f64(total_ped);
            json!({
                "skillName": name,
                "level": level,
                "ttValueGained": float_field(json!(round(tt, 4))),
            })
        })
        .collect();
    Ok(Value::Array(gains))
}

pub fn stable_sort_desc_by_key(entries: &mut [(i64, Value)]) {
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
}

pub fn stable_sort_desc_by_f64(entries: &mut [(f64, Value)]) {
    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
}

/// Whether a tracking session with the given id exists (the
/// session-existence precondition the quest-link operations apply).
pub async fn session_exists(db: &Db, session_id: &str) -> Result<bool, DbError> {
    let sid = session_id.to_string();
    db.with_reader(move |conn| {
        use rusqlite::OptionalExtension as _;
        Ok(conn
            .query_row(
                "SELECT id FROM tracking_sessions WHERE id = ?",
                rusqlite::params![sid],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some())
    })
    .await
}

// ── Session edits ───────────────────────────────────────────────────
//
// Post-hoc edits to ENDED sessions, byte-faithful to the original. The
// four mob/loot edits share the active-session guard (404 absent, 409
// still active); `armour-cost` deliberately omits it. The transport's
// `EditError` maps onto [`ApiError`]: an `Internal` (a `DbError` at the
// boundary) is logged server-side and collapses to the fixed reply.

#[derive(Debug)]
pub enum EditError {
    NotFound(String),
    Conflict(String),
    BadRequest(String),
    Internal,
}

impl From<DbError> for EditError {
    fn from(_: DbError) -> Self {
        EditError::Internal
    }
}

pub async fn validate_session_exists(db: &Db, session_id: &str) -> Result<(), EditError> {
    let sid = session_id.to_string();
    let is_active = db
        .with_reader(move |conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT id, is_active FROM tracking_sessions WHERE id = ?",
                    rusqlite::params![sid],
                    |row| row.get::<_, i64>(1),
                )
                .optional()?)
        })
        .await?;
    let Some(is_active) = is_active else {
        return Err(EditError::NotFound("Session not found".to_string()));
    };
    if is_active != 0 {
        return Err(EditError::Conflict(
            "Session record edits are only available after the session has ended".to_string(),
        ));
    }
    Ok(())
}

pub async fn build_mob_edit_response(
    db: &Db,
    session_id: &str,
    mob_name: &str,
) -> Result<Value, EditError> {
    let sid = session_id.to_string();
    let mob = mob_name.to_string();
    let kill_count = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = ?",
                rusqlite::params![sid, mob],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await?;
    Ok(json!({
        "sessionId": session_id,
        "mobName": mob_name,
        "killCount": kill_count,
    }))
}

pub async fn build_loot_item_edit_response(
    db: &Db,
    session_id: &str,
    item_name: &str,
    affected_rows: i64,
    total_value_delta: f64,
) -> Result<Value, EditError> {
    let sid = session_id.to_string();
    let session_returns = db
        .with_reader(move |conn| {
            Ok(as_f64(&conn.query_row(
                "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
                rusqlite::params![sid],
                |row| Ok(sql_number(row, 0)),
            )?))
        })
        .await?;
    Ok(json!({
        "sessionId": session_id,
        "itemName": item_name,
        "affectedRows": affected_rows,
        "totalValueDelta": round(total_value_delta, 4),
        "sessionTotalReturns": round(session_returns, 2),
    }))
}

/// Re-file an ended session under a different definition: the recorded
/// correction for the session started while the picker still held
/// yesterday's definition, and the only post-hoc route between them.
///
/// The target must be an ACTIVE definition. Filing into an archived
/// one would put an instance under a definition nothing offers any more,
/// which is the one arrangement the review surface could not show
/// honestly; filing *out* of one is how such an instance is rescued.
///
/// The stamped `session_name` always follows the reference. Identity
/// comes from the definition, so the stamp is a copy of its name rather
/// than a label of its own: there is nothing per-session to preserve,
/// and a stamp left behind would name the wrong definition.
pub async fn reassign_session_definition_impl(
    db: &Db,
    session_id: &str,
    definition_id: i64,
) -> Result<Value, EditError> {
    enum Outcome {
        SessionNotFound,
        SessionActive,
        DefinitionNotFound,
        Updated(String),
    }

    let sid = session_id.to_string();
    let outcome = db
        .with_writer(move |conn| {
            use rusqlite::OptionalExtension as _;
            let tx = conn.transaction()?;

            let is_active: Option<i64> = tx
                .query_row(
                    "SELECT is_active FROM tracking_sessions WHERE id = ?",
                    rusqlite::params![sid],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(is_active) = is_active else {
                return Ok(Outcome::SessionNotFound);
            };
            if is_active != 0 {
                return Ok(Outcome::SessionActive);
            }

            // Resolve the target inside the transaction, so the
            // active-definition guard and the write cannot straddle a
            // concurrent soft-delete.
            let target: Option<String> = tx
                .query_row(
                    "SELECT name FROM session_definitions WHERE id = ? AND is_active = 1",
                    rusqlite::params![definition_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(target_name) = target else {
                return Ok(Outcome::DefinitionNotFound);
            };

            tx.execute(
                "UPDATE tracking_sessions SET definition_id = ?, session_name = ? WHERE id = ?",
                rusqlite::params![definition_id, target_name, sid],
            )?;
            // The summary cache carries the facet beside its aggregates;
            // leaving it to the next rebuild would let the comparison
            // axis read the old definition meanwhile.
            tx.execute(
                "UPDATE session_summaries SET session_name = ? WHERE session_id = ?",
                rusqlite::params![target_name, sid],
            )?;
            tx.commit()?;
            Ok(Outcome::Updated(target_name))
        })
        .await?;

    match outcome {
        Outcome::SessionNotFound => Err(EditError::NotFound("Session not found".to_string())),
        Outcome::SessionActive => Err(EditError::Conflict(
            "Session record edits are only available after the session has ended".to_string(),
        )),
        Outcome::DefinitionNotFound => Err(EditError::NotFound(
            "Session definition not found".to_string(),
        )),
        Outcome::Updated(session_name) => Ok(json!({
            "sessionId": session_id,
            "definitionId": definition_id.to_string(),
            "sessionName": session_name,
        })),
    }
}

pub async fn rename_session_mob_impl(
    db: &Db,
    session_id: &str,
    from_mob: &str,
    to_mob: &str,
) -> Result<Value, EditError> {
    validate_session_exists(db, session_id).await?;
    let from_mob = from_mob.trim();
    let to_mob = to_mob.trim();
    if from_mob.is_empty() || to_mob.is_empty() {
        return Err(EditError::BadRequest(
            "Mob names cannot be blank".to_string(),
        ));
    }
    if from_mob == to_mob {
        return Err(EditError::Conflict(
            "rename target matches the current value (no-op)".to_string(),
        ));
    }

    // The domain outcome of the writer-core transaction: the "nothing matched"
    // branch the original raised as a conflict, or the completed rename.
    enum RenameOutcome {
        NoMatch,
        Renamed,
    }

    let sid = session_id.to_string();
    let from = from_mob.to_string();
    let to = to_mob.to_string();
    let outcome = db
        .with_writer(move |conn| {
            let tx = conn.transaction()?;
            let preserve = tx.execute(
                "UPDATE kills \
                 SET original_mob_name = COALESCE(original_mob_name, mob_name) \
                 WHERE session_id = ? AND mob_name = ?",
                rusqlite::params![sid, from],
            )?;
            if preserve == 0 {
                return Ok(RenameOutcome::NoMatch);
            }
            tx.execute(
                "UPDATE kills \
                 SET mob_name = ?, \
                     original_mob_name = CASE \
                         WHEN original_mob_name = ? THEN NULL \
                         ELSE original_mob_name \
                     END \
                 WHERE session_id = ? AND mob_name = ?",
                rusqlite::params![to, to, sid, from],
            )?;
            tx.execute(
                "DELETE FROM session_summaries WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.commit()?;
            Ok(RenameOutcome::Renamed)
        })
        .await?;

    match outcome {
        RenameOutcome::NoMatch => Err(EditError::Conflict(format!(
            "No kills in this session match mob_name='{from_mob}'"
        ))),
        RenameOutcome::Renamed => build_mob_edit_response(db, session_id, to_mob).await,
    }
}

pub async fn restore_session_mob_impl(
    db: &Db,
    session_id: &str,
    current_mob: &str,
) -> Result<Value, EditError> {
    validate_session_exists(db, session_id).await?;
    let current_mob = current_mob.trim();
    if current_mob.is_empty() {
        return Err(EditError::BadRequest(
            "Mob name cannot be blank".to_string(),
        ));
    }

    // The domain outcome of the writer-core transaction: the two conflict
    // branches the original raised as edit errors, or the restored prior name.
    enum RestoreOutcome {
        NoMatch,
        Ambiguous(usize),
        Restored(String),
    }

    let sid = session_id.to_string();
    let current = current_mob.to_string();
    let outcome = db
        .with_writer(move |conn| {
            let tx = conn.transaction()?;
            let restored_rows: Vec<String> = {
                let mut stmt = tx.prepare(
                    "UPDATE kills \
                     SET mob_name = original_mob_name, original_mob_name = NULL \
                     WHERE session_id = ? AND mob_name = ? AND original_mob_name IS NOT NULL \
                     RETURNING mob_name",
                )?;
                let rows = stmt.query_map(rusqlite::params![sid, current], |row| {
                    row.get::<_, String>(0)
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            if restored_rows.is_empty() {
                return Ok(RestoreOutcome::NoMatch);
            }

            let mut distinct: Vec<String> = Vec::new();
            for original in &restored_rows {
                if !distinct.contains(original) {
                    distinct.push(original.clone());
                }
            }
            if distinct.len() > 1 {
                return Ok(RestoreOutcome::Ambiguous(distinct.len()));
            }
            let restored_to = distinct.into_iter().next().expect("one distinct original");

            tx.execute(
                "DELETE FROM session_summaries WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.commit()?;
            Ok(RestoreOutcome::Restored(restored_to))
        })
        .await?;

    match outcome {
        RestoreOutcome::NoMatch => Err(EditError::Conflict(format!(
            "No restorable kills in this session for mob_name='{current_mob}' \
             (either no rename has happened or the preservation column is empty)"
        ))),
        RestoreOutcome::Ambiguous(count) => Err(EditError::Conflict(format!(
            "Ambiguous restore for mob_name='{current_mob}': {count} distinct prior names merged into it."
        ))),
        RestoreOutcome::Restored(restored_to) => {
            build_mob_edit_response(db, session_id, &restored_to).await
        }
    }
}

pub async fn bulk_flip_loot_item(
    db: &Db,
    session_id: &str,
    item_name: &str,
    to_state: &str,
) -> Result<Value, EditError> {
    validate_session_exists(db, session_id).await?;
    let item_name = item_name.trim();
    if item_name.is_empty() {
        return Err(EditError::BadRequest(
            "Item name cannot be blank".to_string(),
        ));
    }

    let (opposite_clause, new_flag_sql, delta_sign) = match to_state {
        "deactivated" => ("l.deactivated_at IS NULL", "unixepoch('now')", -1.0_f64),
        "active" => ("l.deactivated_at IS NOT NULL", "NULL", 1.0_f64),
        other => panic!("unsupported to_state: {other:?}"),
    };

    // The domain outcome of the writer-core transaction: the two "nothing
    // flipped" branches the original raised as edit errors, or the flip's
    // affected-row count and net delta for the response.
    enum FlipOutcome {
        NoLoot,
        AllAlready,
        Flipped { affected: i64, total_delta: f64 },
    }

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
    let sid = session_id.to_string();
    let item = item_name.to_string();
    let outcome = db
        .with_writer(move |conn| {
            use rusqlite::OptionalExtension as _;
            let tx = conn.transaction()?;
            let flipped: Vec<(String, f64)> = {
                let mut stmt = tx.prepare(&flip_sql)?;
                let rows = stmt.query_map(rusqlite::params![sid, item], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            if flipped.is_empty() {
                let any_row: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM kill_loot_items l \
                         JOIN kills k ON k.id = l.kill_id \
                         WHERE k.session_id = ? AND l.item_name = ? \
                         LIMIT 1",
                        rusqlite::params![sid, item],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if any_row.is_none() {
                    return Ok(FlipOutcome::NoLoot);
                }
                return Ok(FlipOutcome::AllAlready);
            }

            let mut order: Vec<String> = Vec::new();
            let mut per_kill: BTreeMap<String, f64> = BTreeMap::new();
            let mut total_delta = 0.0;
            for (kill_id, value) in &flipped {
                if !per_kill.contains_key(kill_id) {
                    order.push(kill_id.clone());
                }
                *per_kill.entry(kill_id.clone()).or_insert(0.0) += value;
                total_delta += value;
            }
            for kill_id in &order {
                let kill_delta = per_kill[kill_id];
                tx.execute(
                    "UPDATE kills SET loot_total_ped = loot_total_ped + ? WHERE id = ?",
                    rusqlite::params![delta_sign * kill_delta, kill_id],
                )?;
            }
            tx.execute(
                "DELETE FROM session_summaries WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            crate::daily_rollup::refresh_session_days(&tx, &sid)?;
            crate::session_rollup::recompute_session(&tx, &sid)?;
            tx.commit()?;

            Ok(FlipOutcome::Flipped {
                affected: flipped.len() as i64,
                total_delta: delta_sign * total_delta,
            })
        })
        .await?;

    match outcome {
        FlipOutcome::NoLoot => Err(EditError::NotFound(format!(
            "No loot named '{item_name}' in this session"
        ))),
        FlipOutcome::AllAlready => Err(EditError::Conflict(format!(
            "All '{item_name}' rows in this session are already {to_state}"
        ))),
        FlipOutcome::Flipped {
            affected,
            total_delta,
        } => build_loot_item_edit_response(db, session_id, item_name, affected, total_delta).await,
    }
}

pub async fn set_armour_cost_impl(
    db: &Db,
    session_id: &str,
    cost: f64,
) -> Result<Value, EditError> {
    // The existence check, the update, and its projection hooks run together on
    // the writer core connection: the read sees the not-yet-committed guard row
    // and the write commits atomically with the rollup and summary refresh.
    let sid = session_id.to_string();
    let found = db
        .with_writer(move |conn| {
            use rusqlite::OptionalExtension as _;
            let started_at: Option<f64> = conn
                .query_row(
                    "SELECT started_at FROM tracking_sessions WHERE id = ?",
                    rusqlite::params![sid],
                    |row| row.get::<_, f64>(0),
                )
                .optional()?;
            let Some(started_at) = started_at else {
                return Ok(false);
            };
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE tracking_sessions SET armour_cost = COALESCE(armour_cost, 0) + ? \
                 WHERE id = ?",
                rusqlite::params![cost, sid],
            )?;
            crate::daily_rollup::refresh_days(&tx, [crate::daily_rollup::epoch_day(started_at)])?;
            crate::session_summary::write_session_summary(&tx, &sid)?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !found {
        return Err(EditError::NotFound("Session not found".to_string()));
    }
    Ok(json!({
        "sessionId": session_id,
        "armourCost": round(cost, 2),
    }))
}

/// Delete a tracking session and every row that references it. An active
/// session cannot be deleted (its writes are still in flight); a missing one
/// is a not-found. The cascade runs in one transaction, child-scoped rows
/// first, and repairs the daily rollups for exactly the calendar days the
/// session touched (captured before the deletes empty those tables).
pub async fn delete_session_impl(db: &Db, session_id: &str) -> Result<(), EditError> {
    // The domain outcome of the writer-core transaction: the guard branches the
    // original raised as edit errors, or the completed cascade.
    enum DeleteOutcome {
        NotFound,
        Active,
        Deleted,
    }

    let sid = session_id.to_string();
    let outcome = db
        .with_writer(move |conn| {
            use rusqlite::OptionalExtension as _;
            let tx = conn.transaction()?;

            // Read the existence-and-active guard inside the transaction, so the
            // check and the cascade are one atomic unit (a separate acquisition
            // for the check could interleave with another write between the read
            // and the delete).
            let is_active: Option<i64> = tx
                .query_row(
                    "SELECT is_active FROM tracking_sessions WHERE id = ?",
                    rusqlite::params![sid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(is_active) = is_active else {
                return Ok(DeleteOutcome::NotFound);
            };
            if is_active != 0 {
                return Ok(DeleteOutcome::Active);
            }

            // Capture the days this session touches before its rows are deleted,
            // so the rollup repair below recomputes exactly those days.
            let days: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT date(timestamp, 'unixepoch') FROM kills WHERE session_id = ? \
                     UNION SELECT DISTINCT date(timestamp, 'unixepoch') FROM skill_gains \
                           WHERE session_id = ? \
                     UNION SELECT date(started_at, 'unixepoch') FROM tracking_sessions WHERE id = ? \
                     UNION SELECT date(ended_at, 'unixepoch') FROM tracking_sessions \
                           WHERE id = ? AND ended_at IS NOT NULL",
                )?;
                let rows = stmt.query_map(
                    rusqlite::params![sid, sid, sid, sid],
                    |row| row.get::<_, String>(0),
                )?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            // Child-scoped rows first (the kill-scoped tables have no ON DELETE
            // cascade), then the kills, the session-scoped rows, and the session.
            tx.execute(
                "DELETE FROM kill_tool_stats \
                 WHERE kill_id IN (SELECT id FROM kills WHERE session_id = ?)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kill_loot_items \
                 WHERE kill_id IN (SELECT id FROM kills WHERE session_id = ?)",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM kills WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM skill_gains WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM notable_events WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM session_summaries WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            // SQLite foreign keys are deliberately disabled for this schema,
            // so healing evidence must participate in the explicit cascade.
            tx.execute(
                "DELETE FROM healing_outputs WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM healing_effect_windows WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM healing_activations WHERE session_id = ?",
                rusqlite::params![sid],
            )?;
            tx.execute(
                "DELETE FROM tracking_sessions WHERE id = ?",
                rusqlite::params![sid],
            )?;
            crate::session_rollup::drop_session(&tx, &sid)?;

            crate::daily_rollup::refresh_days(&tx, days)?;
            tx.commit()?;
            Ok(DeleteOutcome::Deleted)
        })
        .await?;
    match outcome {
        DeleteOutcome::NotFound => Err(EditError::NotFound("Session not found".to_string())),
        DeleteOutcome::Active => Err(EditError::Conflict(
            "Cannot delete an active session".to_string(),
        )),
        DeleteOutcome::Deleted => Ok(()),
    }
}

// ── Producer helpers ────────────────────────────────────────────────

/// `_validate_hotbar`: hotbar attribution is workable while at least one
/// slot is bound (a non-null library id).
pub fn validate_hotbar(config: &AppConfig) -> (bool, Option<String>) {
    let any_bound = config
        .hotbar
        .values()
        .any(|library_id| !library_id.is_null());
    if any_bound {
        (true, None)
    } else {
        (
            false,
            Some(
                "Bind at least one hotbar slot in the Equipment page before tracking.".to_string(),
            ),
        )
    }
}

/// The idle-state mob label: the standing declaration, exactly as the
/// active-session label derives it. A declaration outlives the session
/// that set it, so idle reads the same facet rather than any retired
/// exclusive-capture value.
pub fn declared_mob_label(config: &AppConfig) -> Value {
    let species = config.manual_mob_species.trim();
    let maturity = config.manual_mob_maturity.trim();
    if species.is_empty() {
        return Value::Null;
    }
    let display = if maturity.is_empty() {
        species.to_string()
    } else {
        format!("{maturity} {species}")
    };
    Value::String(display)
}

/// `_notable_event_label`: the curated label for known event types, else
/// the category title-cased.
pub fn notable_event_label(event_type: &str) -> String {
    match event_type {
        "global_kill" => "Global Kill".to_string(),
        "global_item" => "Global Item".to_string(),
        "hof_kill" => "HoF Kill".to_string(),
        "hof_item" => "HoF Item".to_string(),
        "quest_started" => "Quest Started".to_string(),
        "quest_completed" => "Quest Completed".to_string(),
        _ => {
            let category = notable_event_category(event_type);
            if category == "hof" {
                "HoF".to_string()
            } else {
                capitalize(category)
            }
        }
    }
}

/// `_notable_event_description`: the label with the mob or item, and the
/// value in PED for everything but the quest events.
pub fn notable_event_description(event_type: &str, mob_or_item: &str, value_ped: f64) -> String {
    let label = notable_event_label(event_type);
    if event_type.starts_with("quest_") {
        format!("{label}: {mob_or_item}")
    } else {
        format!("{label}: {mob_or_item} ({value_ped:.2} PED)")
    }
}

/// Python `str.capitalize` over an ASCII category.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The config update that clears the stored manual-mob selection.
pub fn clear_manual_mob() -> Map<String, Value> {
    let mut updates = Map::new();
    updates.insert("manual_mob_species".into(), json!(""));
    updates.insert("manual_mob_maturity".into(), json!(""));
    updates
}

/// The released-mob display value.
pub fn mob_display(species: &str, maturity: &str) -> Value {
    if species.is_empty() {
        Value::Null
    } else if maturity.is_empty() {
        Value::String(species.to_string())
    } else {
        Value::String(format!("{maturity} {species}"))
    }
}

/// Project a service value into a response model's field order, emitting
/// only the keys present (Pydantic's `exclude_unset`).
pub fn project(value: &Value, order: &[&str]) -> Value {
    let mut out = Map::new();
    if let Some(object) = value.as_object() {
        for &field in order {
            if let Some(found) = object.get(field) {
                out.insert(field.to_string(), found.clone());
            }
        }
    }
    Value::Object(out)
}

/// `str(...) or None` as an owned `Option<String>` (for the typed DTOs).
pub fn opt_str(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// `_trifecta_attribution_summary`: the active preset's bound
/// weapon/heal names plus the preset list, or null when nothing exists.
pub async fn trifecta_attribution_summary(db: &Db, config: &AppConfig) -> Result<Value, DbError> {
    let active = active_trifecta_preset(config);
    let small = active.and_then(|preset| preset.small_weapon_id);
    let big = active.and_then(|preset| preset.big_weapon_id);
    let heal = active.and_then(|preset| preset.heal_id);
    let presets: Vec<Value> = config
        .trifecta_presets
        .iter()
        .map(|preset| json!({"id": preset.id, "name": preset.name}))
        .collect();
    if presets.is_empty() && small.is_none() && big.is_none() && heal.is_none() {
        return Ok(Value::Null);
    }
    let mut summary = Map::new();
    summary.insert(
        "activePresetId".into(),
        match &config.active_trifecta_preset_id {
            Some(id) => Value::String(id.clone()),
            None => Value::Null,
        },
    );
    summary.insert(
        "presetName".into(),
        match active {
            Some(preset) => Value::String(preset.name.clone()),
            None => Value::Null,
        },
    );
    summary.insert("presets".into(), Value::Array(presets));
    summary.insert(
        "smallWeapon".into(),
        equipment_name(db, small, "weapon").await?,
    );
    summary.insert("bigWeapon".into(), equipment_name(db, big, "weapon").await?);
    summary.insert(
        "healTool".into(),
        equipment_name(db, heal, "healing").await?,
    );
    Ok(Value::Object(summary))
}

/// The equipment-library name for a bound id and type, or null.
pub async fn equipment_name(db: &Db, id: Option<i64>, item_type: &str) -> Result<Value, DbError> {
    let Some(id) = id else {
        return Ok(Value::Null);
    };
    match db.equipment_item(id, item_type).await? {
        Some((_id, name, _properties)) => Ok(Value::String(name)),
        None => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_service::{AppConfig, TrifectaPresetConfig};
    use crate::db::Db;
    use rusqlite::{params, Connection};
    use serde_json::json;

    // ── Test database and seeds ─────────────────────────────────────

    /// A real database over a temp file: the synchronous core opens its own
    /// connections, which an in-memory database cannot share, so the reads
    /// under test see the committed seeds under WAL.
    async fn open_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("entropia_orme.db"))
            .await
            .unwrap();
        (dir, db)
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_session(
        conn: &Connection,
        id: &str,
        started: f64,
        ended: Option<f64>,
        active: bool,
        mode: &str,
        armour: f64,
        heal: f64,
        dangling: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, mob_tracking_mode, armour_cost, heal_cost, dangling_cost) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![id, started, ended, active as i64, mode, armour, heal, dangling],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_kill(
        conn: &Connection,
        id: &str,
        session: &str,
        mob: Option<&str>,
        original: Option<&str>,
        loot: f64,
        enhancer: f64,
        ts: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO kills \
             (id, session_id, mob_name, original_mob_name, loot_total_ped, enhancer_cost, timestamp) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, session, mob, original, loot, enhancer, ts],
        )?;
        Ok(())
    }

    fn seed_tool(
        conn: &Connection,
        kill: &str,
        tool: &str,
        shots: i64,
        dmg: f64,
        crits: i64,
        cost_per_shot: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO kill_tool_stats \
             (kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![kill, tool, shots, dmg, crits, cost_per_shot],
        )?;
        Ok(())
    }

    fn seed_loot(
        conn: &Connection,
        kill: &str,
        name: &str,
        qty: i64,
        value: f64,
        shrapnel: bool,
        deactivated: Option<f64>,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO kill_loot_items \
             (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel, deactivated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![kill, name, qty, value, shrapnel as i64, deactivated],
        )?;
        Ok(())
    }

    fn seed_notable(
        conn: &Connection,
        session: &str,
        event_type: &str,
        mob_or_item: &str,
        value: f64,
        ts: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO notable_events (session_id, event_type, mob_or_item, value_ped, timestamp) \
             VALUES (?, ?, ?, ?, ?)",
            params![session, event_type, mob_or_item, value, ts],
        )?;
        Ok(())
    }

    fn seed_skill_gain(
        conn: &Connection,
        session: &str,
        skill: &str,
        amount: f64,
        ped: f64,
        ts: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES (?, ?, ?, ?, ?)",
            params![session, ts, skill, amount, ped],
        )?;
        Ok(())
    }

    fn seed_calibration(conn: &Connection, skill: &str, level: f64) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO skill_calibrations (skill_name, level, source) VALUES (?, ?, 'manual')",
            params![skill, level],
        )?;
        Ok(())
    }

    fn seed_calibration_at(
        conn: &Connection,
        skill: &str,
        level: f64,
        scanned_at: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, 'scan', ?)",
            params![skill, level, scanned_at],
        )?;
        Ok(())
    }

    fn seed_equipment(
        conn: &Connection,
        id: i64,
        name: &str,
        item_type: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO equipment_library (id, name, item_type, properties_json) VALUES (?, ?, ?, '{}')",
            params![id, name, item_type],
        )?;
        Ok(())
    }

    fn seed_harvest(
        conn: &Connection,
        id: &str,
        session: &str,
        success: bool,
        cost: f64,
        loot: f64,
        ts: f64,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO harvest_events \
             (id, session_id, timestamp, success, cost_ped, loot_total_ped) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![id, session, ts, success as i64, cost, loot],
        )?;
        Ok(())
    }

    fn seed_harvest_loot(
        conn: &Connection,
        harvest: &str,
        name: &str,
        qty: i64,
        value: f64,
        deactivated: Option<f64>,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO harvest_loot_items \
             (harvest_id, item_name, quantity, value_ped, deactivated_at) \
             VALUES (?, ?, ?, ?, ?)",
            params![harvest, name, qty, value, deactivated],
        )?;
        Ok(())
    }

    // ── Engine-typed numeric primitives ─────────────────────────────

    #[test]
    fn sql_number_preserves_the_engine_type() {
        let conn = Connection::open_in_memory().unwrap();
        conn.query_row("SELECT 2.5, 7", [], |row| {
            // A REAL decodes to a float, an INTEGER to an integer: neither
            // masquerades as the other.
            assert_eq!(sql_number(row, 0), json!(2.5));
            assert_eq!(sql_number(row, 1), json!(7));
            assert_ne!(sql_number(row, 1), json!(7.0));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn as_f64_reads_numbers_and_defaults_null_to_zero() {
        assert_eq!(as_f64(&json!(2.5)), 2.5);
        assert_eq!(as_f64(&json!(3)), 3.0);
        assert_eq!(as_f64(&Value::Null), 0.0);
    }

    #[test]
    fn round_applies_bankers_rounding() {
        assert_eq!(round(1.23456, 2), 1.23);
        // Half-to-even: 2.5 rounds down to the even 2, 1.25 to the even 1.2.
        assert_eq!(round(2.5, 0), 2.0);
        assert_eq!(round(1.25, 1), 1.2);
    }

    #[test]
    fn float_field_coerces_integers_and_leaves_floats() {
        assert_eq!(float_field(json!(5)), json!(5.0));
        assert_ne!(float_field(json!(5)), json!(5));
        assert_eq!(float_field(json!(2.5)), json!(2.5));
        assert_eq!(float_field(Value::Null), Value::Null);
    }

    #[test]
    fn list_ts_to_iso_renders_whole_fractional_and_carry_cases() {
        assert_eq!(list_ts_to_iso(None), Value::Null);
        assert_eq!(
            list_ts_to_iso(Some(0.0)),
            json!("1970-01-01T00:00:00+00:00")
        );
        assert_eq!(
            list_ts_to_iso(Some(1.5)),
            json!("1970-01-01T00:00:01.500000+00:00")
        );
        // A fraction that rounds up to a full second carries into the whole.
        assert_eq!(
            list_ts_to_iso(Some(0.9999999)),
            json!("1970-01-01T00:00:01+00:00")
        );
        // A negative timestamp borrows a second so the microseconds stay positive.
        assert_eq!(
            list_ts_to_iso(Some(-1.5)),
            json!("1969-12-31T23:59:58.500000+00:00")
        );
    }

    #[test]
    fn event_ts_to_iso_renders_or_nulls() {
        assert_eq!(event_ts_to_iso(None), Value::Null);
        assert_eq!(
            event_ts_to_iso(Some(0.0)),
            json!("1970-01-01T00:00:00+00:00")
        );
        assert_eq!(
            event_ts_to_iso(Some(1.5)),
            json!("1970-01-01T00:00:01.500000+00:00")
        );
    }

    #[test]
    fn duration_seconds_covers_ended_active_and_zero() {
        // Ended: the stored span, regardless of the clock.
        assert_eq!(
            duration_seconds(Some(100.0), Some(250.0), false, 999.0),
            150
        );
        // Active with no end: the running span from the clock.
        assert_eq!(duration_seconds(Some(100.0), None, true, 250.0), 150);
        // Active but never started, and inactive with no end: both zero.
        assert_eq!(duration_seconds(None, None, true, 250.0), 0);
        assert_eq!(duration_seconds(Some(100.0), None, false, 250.0), 0);
    }

    #[test]
    fn notable_event_category_maps_every_branch() {
        assert_eq!(notable_event_category("quest_started"), "quest");
        assert_eq!(notable_event_category("hof_kill"), "hof");
        assert_eq!(notable_event_category("global_kill"), "global");
        assert_eq!(notable_event_category(""), "global");
        assert_eq!(notable_event_category("anything_else"), "global");
    }

    #[test]
    fn parse_string_array_parses_or_falls_back_empty() {
        assert_eq!(parse_string_array("[\"a\",\"b\"]"), json!(["a", "b"]));
        assert_eq!(parse_string_array("not json"), json!([]));
    }

    // ── Session list ────────────────────────────────────────────────

    #[test]
    fn list_row_from_summary_shapes_the_summary_row() {
        let summary = ListSummary {
            weapon_cost: 1.0,
            heal_cost: 2.0,
            enhancer_cost: 3.0,
            armour_cost: 4.0,
            dangling_cost: 5.0,
            harvest_cost: 6.0,
            loot_tt: 30.0,
            primary_mobs: json!(["Argonaut"]),
            primary_weapons: json!(["Gun"]),
            globals: 2,
            hofs: 1,
        };
        let row = list_row_from_summary("s1", Some(1000.0), Some(4600.0), false, 9999.0, &summary);
        assert_eq!(
            row,
            json!({
                "id": "s1",
                "startTime": "1970-01-01T00:16:40+00:00",
                "endTime": "1970-01-01T01:16:40+00:00",
                "duration": 3600,
                "primaryMobs": ["Argonaut"],
                "primaryWeapons": ["Gun"],
                "cost": 21.0,
                "returns": 30.0,
                "net": 9.0,
                "returnRate": round(30.0 / 21.0, 4),
                "globals": 2,
                "hofs": 1,
            })
        );
    }

    #[tokio::test]
    async fn scalar_reads_a_number_and_zero_defaults() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 12.5, 0.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 7.5, 0.0, 1100.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let sum = db
            .with_reader(|conn| {
                scalar(
                    conn,
                    "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
                    "s1",
                )
            })
            .await
            .unwrap();
        assert_eq!(sum, json!(20.0));

        // The empty COALESCE case stays an engine integer zero.
        let empty = db
            .with_reader(|conn| {
                scalar(
                    conn,
                    "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
                    "absent",
                )
            })
            .await
            .unwrap();
        assert_eq!(empty, json!(0));
    }

    #[tokio::test]
    async fn string_column_collects_grouped_names() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 0.0, 0.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 0.0, 0.0, 1100.0)?;
            seed_kill(conn, "k3", "s1", Some("Atrox"), None, 0.0, 0.0, 1200.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let names = db
            .with_reader(|conn| {
                string_column(
                    conn,
                    "SELECT mob_name FROM kills WHERE session_id = ? \
                     GROUP BY mob_name ORDER BY COUNT(*) DESC, mob_name ASC",
                    "s1",
                )
            })
            .await
            .unwrap();
        assert_eq!(names, vec!["Argonaut".to_string(), "Atrox".to_string()]);
    }

    #[tokio::test]
    async fn list_row_from_raw_aggregates_the_raw_tables() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                4.0,
                2.0,
                1.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 3.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 0.0, 0.0, 1100.0)?;
            seed_tool(conn, "k1", "Gun", 10, 100.0, 1, 0.5)?;
            seed_tool(conn, "k2", "Gun", 10, 100.0, 0, 0.5)?;
            seed_notable(conn, "s1", "global_kill", "Argonaut", 50.0, 1000.0)?;
            seed_notable(conn, "s1", "hof_item", "Sword", 100.0, 1100.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let row = db
            .with_reader(|conn| {
                list_row_from_raw(conn, "s1", Some(1000.0), Some(4600.0), false, 0.0)
            })
            .await
            .unwrap();
        // weapon 10 + heal 2 + enhancer 3 + armour 4 + dangling 1 = 20 cost;
        // returns 30; net 10; rate 30/20 = 1.5.
        assert_eq!(
            row,
            json!({
                "id": "s1",
                "startTime": "1970-01-01T00:16:40+00:00",
                "endTime": "1970-01-01T01:16:40+00:00",
                "duration": 3600,
                "primaryMobs": ["Argonaut"],
                "primaryWeapons": ["Gun"],
                "cost": 20.0,
                "returns": 30.0,
                "net": 10.0,
                "returnRate": 1.5,
                "globals": 1,
                "hofs": 1,
            })
        );
    }

    #[tokio::test]
    async fn list_row_from_raw_folds_in_harvesting() {
        // The list read folds harvest loot and swing decay independently of the
        // detail read; this covers the mixed hunting-and-harvesting case
        // in-crate.
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 20.0, 0.0, 1000.0)?;
            seed_tool(conn, "k1", "Gun", 10, 100.0, 0, 0.5)?;
            // Two swings: one successful (8.0 wood TT), one failed; 3.0 decay.
            seed_harvest(conn, "h1", "s1", true, 2.0, 8.0, 1100.0)?;
            seed_harvest(conn, "h2", "s1", false, 1.0, 0.0, 1200.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let row = db
            .with_reader(|conn| {
                list_row_from_raw(conn, "s1", Some(1000.0), Some(4600.0), false, 0.0)
            })
            .await
            .unwrap();
        // Cost is weapon 5 + harvest decay 3 = 8; returns are kill 20 +
        // harvest 8 = 28; net 20; rate 28/8 = 3.5. The same cost and returns figures
        // as `get_session_read_folds_in_harvesting`, which seeds the same
        // swings, kill, and tool: both reads take money from
        // `kills.loot_total_ped` and `harvest_events`, never from the item
        // tables. The single assertion that ties the two surfaces together is
        // the facade test `a_harvest_session_nets_the_same_on_the_list_and_the_detail`;
        // it cannot kill a mutant here, because the mutation campaign runs only
        // the `eo-wire` and `eo-services` tests.
        assert_eq!(
            row,
            json!({
                "id": "s1",
                "startTime": "1970-01-01T00:16:40+00:00",
                "endTime": "1970-01-01T01:16:40+00:00",
                "duration": 3600,
                "primaryMobs": ["Argonaut"],
                "primaryWeapons": ["Gun"],
                "cost": 8.0,
                "returns": 28.0,
                "net": 20.0,
                "returnRate": 3.5,
                "globals": 0,
                "hofs": 0,
            })
        );
    }

    #[tokio::test]
    async fn list_sessions_read_picks_summary_only_for_ended_rows() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            // An ended session carrying a summary: the summary path.
            seed_session(
                conn,
                "s-ended",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            conn.execute(
                "INSERT INTO session_summaries \
                 (session_id, started_at, ended_at, duration_hours, kills, loot_tt, weapon_cost, \
                  enhancer_cost, armour_cost, heal_cost, dangling_cost, cycled_ped, \
                  regular_skill_ped_json, attribute_levels_json, regular_skill_tt, \
                  attribute_levels_total, primary_mobs_json, primary_weapons_json, globals, hofs) \
                 VALUES ('s-ended', 1000.0, 4600.0, 1.0, 2, 30.0, 1.0, 3.0, 4.0, 2.0, 5.0, 0.0, \
                  '{}', '{}', 0.0, 0.0, '[\"Argonaut\"]', '[\"Gun\"]', 2, 1)",
                [],
            )?;
            // An active session that also has a summary must still read raw,
            // so the summary's numbers never leak onto an in-progress row.
            seed_session(conn, "s-active", 2000.0, None, true, "mob", 0.0, 0.0, 0.0)?;
            conn.execute(
                "INSERT INTO session_summaries \
                 (session_id, started_at, ended_at, duration_hours, kills, loot_tt, weapon_cost, \
                  enhancer_cost, armour_cost, heal_cost, dangling_cost, cycled_ped, \
                  regular_skill_ped_json, attribute_levels_json, regular_skill_tt, \
                  attribute_levels_total, primary_mobs_json, primary_weapons_json, globals, hofs) \
                 VALUES ('s-active', 2000.0, 2000.0, 1.0, 9, 999.0, 9.0, 9.0, 9.0, 9.0, 9.0, 0.0, \
                  '{}', '{}', 0.0, 0.0, '[\"Leak\"]', '[\"Leak\"]', 9, 9)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let page = db
            .with_reader(|conn| list_sessions_read(conn, 5000.0, None, None, None))
            .await
            .unwrap();
        let rows = page.sessions.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        // Ordered by started_at DESC: the active session first.
        assert_eq!(rows[0]["id"], json!("s-active"));
        assert_eq!(rows[1]["id"], json!("s-ended"));

        // The active row read raw: none of the summary's leak values survive.
        assert_eq!(rows[0]["cost"], json!(0.0));
        assert_eq!(rows[0]["returns"], json!(0.0));
        assert_eq!(rows[0]["globals"], json!(0));
        assert_eq!(rows[0]["primaryMobs"], json!([]));
        assert_eq!(rows[0]["duration"], json!(3000));

        // The ended row read its summary.
        assert_eq!(rows[1]["cost"], json!(15.0));
        assert_eq!(rows[1]["returns"], json!(30.0));
        assert_eq!(rows[1]["returnRate"], json!(2.0));
        assert_eq!(rows[1]["globals"], json!(2));
        assert_eq!(rows[1]["primaryMobs"], json!(["Argonaut"]));
    }

    // ── Session detail ──────────────────────────────────────────────

    #[tokio::test]
    async fn get_session_read_shapes_the_full_detail() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                4.0,
                2.0,
                1.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 3.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 0.0, 0.0, 1100.0)?;
            seed_tool(conn, "k1", "Gun", 10, 100.0, 1, 0.5)?;
            seed_tool(conn, "k2", "Gun", 10, 100.0, 0, 0.5)?;
            seed_loot(conn, "k1", "Oil", 2, 20.0, false, None)?;
            seed_loot(conn, "k1", "Enhancer Shrapnel", 1, 5.0, true, None)?;
            seed_loot(conn, "k2", "Hide", 1, 10.0, false, Some(4700.0))?;
            seed_notable(conn, "s1", "global_kill", "Argonaut", 50.0, 1000.0)?;
            seed_notable(conn, "s1", "hof_item", "Sword", 100.0, 1100.0)?;
            seed_skill_gain(conn, "s1", "Laser Weaponry Technology", 1.0, 0.5, 1000.0)?;
            seed_skill_gain(conn, "s1", "Agility", 2.0, 1.0, 1000.0)?;
            seed_calibration(conn, "Laser Weaponry Technology", 42.5)?;
            conn.execute_batch(
                "INSERT INTO healing_activations (
                    id, session_id, tool_name, intent_at, observed_at,
                    chat_timestamp, cost_ped, profile_json, provenance
                 ) VALUES (
                    'ha1', 's1', 'Restoration Chip', 1000, 1000,
                    '2026-01-01 00:00:00', 0.04, '{}', 'direct'
                 );
                 INSERT INTO healing_effect_windows (
                    id, activation_id, session_id, tool_name, started_at, expires_at
                 ) VALUES ('hw1', 'ha1', 's1', 'Restoration Chip', 1000, 1030);
                 INSERT INTO healing_outputs (
                    id, session_id, activation_id, observed_at, chat_timestamp,
                    amount, classification, reason
                 ) VALUES (
                    'ho1', 's1', 'ha1', 1000, '2026-01-01 00:00:00',
                    30, 'direct', 'intent_confirmed'
                 );
                 INSERT INTO healing_outputs (
                    id, session_id, activation_id, effect_window_id, observed_at,
                    chat_timestamp, amount, classification, reason
                 ) VALUES (
                    'ho2', 's1', 'ha1', 'hw1', 1010,
                    '2026-01-01 00:00:10', 10, 'effect', 'effect_window'
                 );
                 INSERT INTO healing_outputs (
                    id, session_id, observed_at, chat_timestamp, amount,
                    classification, reason
                 ) VALUES (
                    'ho3', 's1', 1020, '2026-01-01 00:00:20', 2,
                    'passive', 'damage_correlated'
                 );",
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let value = db
            .with_reader(|conn| get_session_read(conn, "s1", 0.0))
            .await
            .unwrap()
            .expect("the session exists");

        // weapon 10 + heal 2 + enhancer 3 + armour 4 + dangling 1 = 20 cost;
        // returns 30; net 10; rate 1.5; pes = 0.5 + 1.0 skill TT.
        assert_eq!(
            value,
            json!({
                "sessionId": "s1",
                "summary": {
                    "cost": 20.0,
                    "returns": 30.0,
                    "pes": 1.5,
                    "net": 10.0,
                    "returnRate": 1.5,
                    "kills": 2,
                    "duration": 3600,
                    "costBreakdown": {
                        "weaponCost": 10.0,
                        "healCost": 2.0,
                        "enhancerCost": 3.0,
                        "armourCost": 4.0,
                        "harvestCost": 0.0,
                    },
                },
                "harvest": {
                    "swings": 0,
                    "successes": 0,
                    "lootTt": 0.0,
                    "cost": 0.0,
                },
                "sessionName": null,
                "mobEntryMode": "mob",
                "notableEvents": [
                    {"type": "global", "eventType": "global_kill", "target": "Argonaut", "item": "Argonaut", "value": 50.0},
                    {"type": "hof", "eventType": "hof_item", "target": "Sword", "item": "Sword", "value": 100.0},
                ],
                "lootBreakdown": [{"name": "Oil", "quantity": 2, "ttValue": 20.0}],
                "deactivatedLootBreakdown": [{"name": "Hide", "quantity": 1, "ttValue": 10.0}],
                "mobBreakdown": [{"currentName": "Argonaut", "originalName": null, "killCount": 2}],
                "effectiveLoot": 30.0,
                "toolStats": [{"weaponName": "Gun", "shotsFired": 20, "damageDealt": 200.0, "crits": 1, "costAttributed": 10.0}],
                "skillGains": [{"skillName": "Laser Weaponry Technology", "level": 42.5, "ttValueGained": 0.5}],
                "healing": {
                    "activations": [{
                        "id": "ha1",
                        "toolName": "Restoration Chip",
                        "observedAt": 1000.0,
                        "cost": 0.04,
                        "provenance": "direct",
                        "effectUntil": 1030.0,
                        "outputCount": 2,
                    }],
                    "activationCount": 1,
                    "outputCount": 3,
                    "directOutputs": 1,
                    "effectOutputs": 1,
                    "passiveOutputs": 1,
                    "unattributedOutputs": 0,
                },
            })
        );
    }

    #[test]
    fn merge_loot_aggs_sums_shared_names_and_appends_new() {
        // "Wood Shavings" is in both halves: its quantity and TT sum
        // (3 + 2, 1.5 + 2.5). "Oil" is base-only and keeps its place;
        // "Short Moonleaf Board" is extra-only and appends after the base
        // order. The function is generic over the triples, but the wood names
        // are ones the harvest routing actually accepts; `Oil` is mob loot,
        // which only the base side can carry.
        let base = vec![
            ("Wood Shavings".to_string(), 3, 1.5),
            ("Oil".to_string(), 1, 10.0),
        ];
        let extra = vec![
            ("Wood Shavings".to_string(), 2, 2.5),
            ("Short Moonleaf Board".to_string(), 4, 8.0),
        ];
        assert_eq!(
            merge_loot_aggs(base, extra),
            vec![
                ("Wood Shavings".to_string(), 5, 4.0),
                ("Oil".to_string(), 1, 10.0),
                ("Short Moonleaf Board".to_string(), 4, 8.0),
            ]
        );
    }

    #[tokio::test]
    async fn get_session_read_folds_in_harvesting() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 20.0, 0.0, 1000.0)?;
            seed_tool(conn, "k1", "Gun", 10, 100.0, 0, 0.5)?;
            seed_loot(conn, "k1", "Oil", 2, 20.0, false, None)?;
            // Two swings: one successful (8.0 wood TT), one failed; 3.0 decay.
            seed_harvest(conn, "h1", "s1", true, 2.0, 8.0, 1100.0)?;
            seed_harvest(conn, "h2", "s1", false, 1.0, 0.0, 1200.0)?;
            seed_harvest_loot(conn, "h1", "Short Moonleaf Board", 5, 8.0, None)?;
            // A deactivated harvest item: it leaves the active breakdown and
            // appears in the deactivated one, while the money figures (which
            // come from harvest_events, not the item rows) stay put.
            seed_harvest_loot(conn, "h1", "Wood Shavings", 2, 1.0, Some(1300.0))?;
            Ok(())
        })
        .await
        .unwrap();

        let value = db
            .with_reader(|conn| get_session_read(conn, "s1", 0.0))
            .await
            .unwrap()
            .expect("the session exists");

        // Harvest wood joins returns (20 kill + 8 harvest), swing decay joins
        // cost (5 weapon + 3 harvest), harvest loot merges into the loot
        // breakdown, and the harvest block reports the swing tallies.
        assert_eq!(
            value,
            json!({
                "sessionId": "s1",
                "summary": {
                    "cost": 8.0,
                    "returns": 28.0,
                    "pes": 0.0,
                    "net": 20.0,
                    "returnRate": 3.5,
                    "kills": 1,
                    "duration": 3600,
                    "costBreakdown": {
                        "weaponCost": 5.0,
                        "healCost": 0.0,
                        "enhancerCost": 0.0,
                        "armourCost": 0.0,
                        "harvestCost": 3.0,
                    },
                },
                "harvest": {
                    "swings": 2,
                    "successes": 1,
                    "lootTt": 8.0,
                    "cost": 3.0,
                },
                "sessionName": null,
                "mobEntryMode": "mob",
                "notableEvents": [],
                "lootBreakdown": [
                    {"name": "Oil", "quantity": 2, "ttValue": 20.0},
                    {"name": "Short Moonleaf Board", "quantity": 5, "ttValue": 8.0},
                ],
                "deactivatedLootBreakdown": [
                    {"name": "Wood Shavings", "quantity": 2, "ttValue": 1.0},
                ],
                "mobBreakdown": [{"currentName": "Argonaut", "originalName": null, "killCount": 1}],
                "effectiveLoot": 28.0,
                "toolStats": [{"weaponName": "Gun", "shotsFired": 10, "damageDealt": 100.0, "crits": 0, "costAttributed": 5.0}],
                "skillGains": [],
                "healing": {
                    "activations": [],
                    "activationCount": 0,
                    "outputCount": 0,
                    "directOutputs": 0,
                    "effectOutputs": 0,
                    "passiveOutputs": 0,
                    "unattributedOutputs": 0,
                },
            })
        );
    }

    #[tokio::test]
    async fn get_session_read_absent_is_none() {
        let (_dir, db) = open_db().await;
        let value = db
            .with_reader(|conn| get_session_read(conn, "absent", 0.0))
            .await
            .unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn get_session_read_recovers_the_mob_entry_mode() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "tag",
                1000.0,
                Some(2000.0),
                false,
                "tag",
                0.0,
                0.0,
                0.0,
            )?;
            seed_session(
                conn,
                "weird",
                1000.0,
                Some(2000.0),
                false,
                "legacy",
                0.0,
                0.0,
                0.0,
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let tag = db
            .with_reader(|conn| get_session_read(conn, "tag", 0.0))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tag["mobEntryMode"], json!("tag"));
        // Anything outside the closed vocabulary recovers to the mob default.
        let weird = db
            .with_reader(|conn| get_session_read(conn, "weird", 0.0))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(weird["mobEntryMode"], json!("mob"));
    }

    #[tokio::test]
    async fn loot_agg_splits_active_and_deactivated_and_drops_shrapnel() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 0.0, 0.0, 1000.0)?;
            seed_loot(conn, "k1", "Oil", 2, 20.0, false, None)?;
            seed_loot(conn, "k1", "Enhancer Shrapnel", 1, 5.0, true, None)?;
            seed_loot(conn, "k1", "Hide", 1, 10.0, false, Some(1500.0))?;
            Ok(())
        })
        .await
        .unwrap();

        let active = db
            .with_reader(|conn| loot_agg(conn, "s1", "l.deactivated_at IS NULL"))
            .await
            .unwrap();
        assert_eq!(active, vec![("Oil".to_string(), 2, 20.0)]);

        let deactivated = db
            .with_reader(|conn| loot_agg(conn, "s1", "l.deactivated_at IS NOT NULL"))
            .await
            .unwrap();
        assert_eq!(deactivated, vec![("Hide".to_string(), 1, 10.0)]);
    }

    #[test]
    fn loot_breakdown_sorted_orders_by_tt_descending() {
        let rows = vec![
            ("Shrapnel".to_string(), 5, 10.0),
            ("Oil".to_string(), 2, 25.5),
        ];
        let out = loot_breakdown_sorted(&rows);
        assert_eq!(
            out,
            vec![
                json!({"name": "Oil", "quantity": 2, "ttValue": 25.5}),
                json!({"name": "Shrapnel", "quantity": 5, "ttValue": 10.0}),
            ]
        );
    }

    #[tokio::test]
    async fn session_skill_gains_excludes_attributes_and_joins_levels() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_skill_gain(conn, "s1", "Laser Weaponry Technology", 1.0, 0.5, 1000.0)?;
            seed_skill_gain(conn, "s1", "Agility", 2.0, 1.0, 1000.0)?;
            seed_calibration(conn, "Laser Weaponry Technology", 42.5)?;
            Ok(())
        })
        .await
        .unwrap();

        let gains = db
            .with_reader(|conn| session_skill_gains(conn, "s1"))
            .await
            .unwrap();
        assert_eq!(
            gains,
            json!([{"skillName": "Laser Weaponry Technology", "level": 42.5, "ttValueGained": 0.5}])
        );

        // No gains: an empty array, not null.
        let empty = db
            .with_reader(|conn| session_skill_gains(conn, "absent"))
            .await
            .unwrap();
        assert_eq!(empty, json!([]));
    }

    #[tokio::test]
    async fn session_skill_gains_reports_the_level_latest_in_time_not_by_id() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_skill_gain(conn, "s1", "Laser Weaponry Technology", 1.0, 0.5, 1000.0)?;
            // The newer reading is written first; an older screenshot is
            // recorded afterwards and so carries the higher id. The level
            // reported is the one latest in time.
            seed_calibration_at(conn, "Laser Weaponry Technology", 50.0, 5000.0)?;
            seed_calibration_at(conn, "Laser Weaponry Technology", 40.0, 4000.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let gains = db
            .with_reader(|conn| session_skill_gains(conn, "s1"))
            .await
            .unwrap();
        assert_eq!(
            gains,
            json!([{"skillName": "Laser Weaponry Technology", "level": 50.0, "ttValueGained": 0.5}])
        );
    }

    #[test]
    fn stable_sort_desc_by_key_orders_descending() {
        let mut entries = vec![(1i64, json!("a")), (3, json!("b")), (2, json!("c"))];
        stable_sort_desc_by_key(&mut entries);
        let keys: Vec<i64> = entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![3, 2, 1]);
    }

    #[test]
    fn stable_sort_desc_by_f64_orders_descending_and_is_stable() {
        let mut entries = vec![(1.0f64, json!("a")), (3.0, json!("b")), (3.0, json!("c"))];
        stable_sort_desc_by_f64(&mut entries);
        let order: Vec<&Value> = entries.iter().map(|(_, v)| v).collect();
        // Descending, and the two equal keys keep their input order.
        assert_eq!(order, vec![&json!("b"), &json!("c"), &json!("a")]);
    }

    // ── Session existence and tag suggestions ───────────────────────

    #[tokio::test]
    async fn session_exists_reports_presence() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )
        })
        .await
        .unwrap();
        assert!(session_exists(&db, "s1").await.unwrap());
        assert!(!session_exists(&db, "absent").await.unwrap());
    }

    // ── Async read impls over the real core ─────────────────────────

    #[tokio::test]
    async fn list_sessions_impl_returns_the_recent_sessions() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )
        })
        .await
        .unwrap();
        let page = list_sessions_impl(&db, 5000.0, None, None, None)
            .await
            .unwrap();
        let rows = page.sessions.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("s1"));
    }

    #[tokio::test]
    async fn get_session_impl_returns_detail_or_none() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )
        })
        .await
        .unwrap();
        let found = get_session_impl(&db, "s1", 0.0).await.unwrap();
        assert_eq!(found.unwrap()["sessionId"], json!("s1"));
        assert!(get_session_impl(&db, "absent", 0.0)
            .await
            .unwrap()
            .is_none());
    }

    // ── Session edits ───────────────────────────────────────────────

    #[tokio::test]
    async fn validate_session_exists_guards_absent_and_active() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "ended",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_session(conn, "active", 1000.0, None, true, "mob", 0.0, 0.0, 0.0)?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(validate_session_exists(&db, "ended").await.is_ok());
        assert!(matches!(
            validate_session_exists(&db, "absent").await,
            Err(EditError::NotFound(_))
        ));
        assert!(matches!(
            validate_session_exists(&db, "active").await,
            Err(EditError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn build_mob_edit_response_counts_kills() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 0.0, 0.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 0.0, 0.0, 1100.0)?;
            Ok(())
        })
        .await
        .unwrap();
        let value = build_mob_edit_response(&db, "s1", "Argonaut")
            .await
            .unwrap();
        assert_eq!(
            value,
            json!({"sessionId": "s1", "mobName": "Argonaut", "killCount": 2})
        );
    }

    #[tokio::test]
    async fn build_loot_item_edit_response_shapes_the_delta() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 0.0, 1000.0)?;
            Ok(())
        })
        .await
        .unwrap();
        let value = build_loot_item_edit_response(&db, "s1", "Oil", 3, -12.3456)
            .await
            .unwrap();
        assert_eq!(
            value,
            json!({
                "sessionId": "s1",
                "itemName": "Oil",
                "affectedRows": 3,
                "totalValueDelta": -12.3456,
                "sessionTotalReturns": 30.0,
            })
        );
    }

    #[tokio::test]
    async fn rename_session_mob_impl_covers_the_flow_and_guards() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 0.0, 0.0, 1000.0)?;
            seed_kill(conn, "k2", "s1", Some("Argonaut"), None, 0.0, 0.0, 1100.0)?;
            Ok(())
        })
        .await
        .unwrap();

        // Blank targets and a no-op rename are rejected before any write.
        assert!(matches!(
            rename_session_mob_impl(&db, "s1", "  ", "Atrox").await,
            Err(EditError::BadRequest(_))
        ));
        assert!(matches!(
            rename_session_mob_impl(&db, "s1", "Argonaut", "Argonaut").await,
            Err(EditError::Conflict(_))
        ));
        assert!(matches!(
            rename_session_mob_impl(&db, "s1", "Nonexistent", "Atrox").await,
            Err(EditError::Conflict(_))
        ));

        // The rename lands and reports the new count.
        let value = rename_session_mob_impl(&db, "s1", "Argonaut", "Atrox")
            .await
            .unwrap();
        assert_eq!(
            value,
            json!({"sessionId": "s1", "mobName": "Atrox", "killCount": 2})
        );
    }

    #[tokio::test]
    async fn restore_session_mob_impl_covers_the_flow_and_guards() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Blank and no-match branches.
        assert!(matches!(
            restore_session_mob_impl(&db, "s1", "  ").await,
            Err(EditError::BadRequest(_))
        ));
        assert!(matches!(
            restore_session_mob_impl(&db, "s1", "Atrox").await,
            Err(EditError::Conflict(_))
        ));

        // A single prior name restores cleanly.
        db.with_writer(|conn| {
            seed_kill(
                conn,
                "k1",
                "s1",
                Some("Atrox"),
                Some("Argonaut"),
                0.0,
                0.0,
                1000.0,
            )
        })
        .await
        .unwrap();
        let value = restore_session_mob_impl(&db, "s1", "Atrox").await.unwrap();
        assert_eq!(value["mobName"], json!("Argonaut"));

        // Two distinct prior names merged into one is ambiguous.
        db.with_writer(|conn| {
            seed_kill(
                conn,
                "k2",
                "s1",
                Some("Merged"),
                Some("First"),
                0.0,
                0.0,
                1000.0,
            )?;
            seed_kill(
                conn,
                "k3",
                "s1",
                Some("Merged"),
                Some("Second"),
                0.0,
                0.0,
                1100.0,
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(matches!(
            restore_session_mob_impl(&db, "s1", "Merged").await,
            Err(EditError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn bulk_flip_loot_item_flips_and_guards() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 0.0, 1000.0)?;
            seed_loot(conn, "k1", "Oil", 1, 20.0, false, None)?;
            Ok(())
        })
        .await
        .unwrap();

        // No such loot is a not-found; deactivating an active row nets it out.
        assert!(matches!(
            bulk_flip_loot_item(&db, "s1", "Ghost", "deactivated").await,
            Err(EditError::NotFound(_))
        ));
        let value = bulk_flip_loot_item(&db, "s1", "Oil", "deactivated")
            .await
            .unwrap();
        assert_eq!(
            value,
            json!({
                "sessionId": "s1",
                "itemName": "Oil",
                "affectedRows": 1,
                "totalValueDelta": -20.0,
                "sessionTotalReturns": 10.0,
            })
        );

        // A second deactivate finds nothing left to flip: a conflict.
        assert!(matches!(
            bulk_flip_loot_item(&db, "s1", "Oil", "deactivated").await,
            Err(EditError::Conflict(_))
        ));

        // Flipping it back to active restores the value.
        let back = bulk_flip_loot_item(&db, "s1", "Oil", "active")
            .await
            .unwrap();
        assert_eq!(back["totalValueDelta"], json!(20.0));
        assert_eq!(back["sessionTotalReturns"], json!(30.0));
    }

    #[tokio::test]
    async fn set_armour_cost_impl_accumulates_or_not_found() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                1.0,
                0.0,
                0.0,
            )
        })
        .await
        .unwrap();

        let value = set_armour_cost_impl(&db, "s1", 5.0).await.unwrap();
        assert_eq!(value, json!({"sessionId": "s1", "armourCost": 5.0}));
        // The stored cost accumulated onto the seeded 1.0.
        let stored = db
            .with_reader(|conn| {
                Ok::<f64, DbError>(conn.query_row(
                    "SELECT armour_cost FROM tracking_sessions WHERE id = 's1'",
                    [],
                    |row| row.get::<_, f64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(stored, 6.0);

        assert!(matches!(
            set_armour_cost_impl(&db, "absent", 5.0).await,
            Err(EditError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn delete_session_impl_cascades_and_guards() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(2000.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 0.0, 1000.0)?;
            seed_tool(conn, "k1", "Gun", 10, 100.0, 1, 0.5)?;
            seed_loot(conn, "k1", "Oil", 1, 20.0, false, None)?;
            seed_notable(conn, "s1", "global_kill", "Argonaut", 50.0, 1000.0)?;
            seed_skill_gain(conn, "s1", "Laser Weaponry Technology", 1.0, 0.5, 1000.0)?;
            conn.execute_batch(
                "INSERT INTO healing_activations (
                    id, session_id, tool_name, intent_at, observed_at,
                    chat_timestamp, cost_ped, profile_json, provenance
                 ) VALUES (
                    'ha1', 's1', 'Restoration Chip', 1000, 1000,
                    '2026-01-01 00:00:00', 0.01, '{}', 'direct'
                 );
                 INSERT INTO healing_effect_windows (
                    id, activation_id, session_id, tool_name, started_at, expires_at
                 ) VALUES ('hw1', 'ha1', 's1', 'Restoration Chip', 1000, 1030);
                 INSERT INTO healing_outputs (
                    id, session_id, activation_id, effect_window_id, observed_at,
                    chat_timestamp, amount, classification, reason
                 ) VALUES (
                    'ho1', 's1', 'ha1', 'hw1', 1000,
                    '2026-01-01 00:00:00', 30, 'direct', 'intent_confirmed'
                 );",
            )?;
            seed_session(conn, "active", 1000.0, None, true, "mob", 0.0, 0.0, 0.0)?;
            Ok(())
        })
        .await
        .unwrap();

        assert!(matches!(
            delete_session_impl(&db, "absent").await,
            Err(EditError::NotFound(_))
        ));
        assert!(matches!(
            delete_session_impl(&db, "active").await,
            Err(EditError::Conflict(_))
        ));

        delete_session_impl(&db, "s1").await.unwrap();
        assert!(!session_exists(&db, "s1").await.unwrap());
        // The kill-scoped children are gone with it.
        let kill_count = db
            .with_reader(|conn| {
                Ok::<i64, DbError>(conn.query_row(
                    "SELECT COUNT(*) FROM kills WHERE session_id = 's1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(kill_count, 0);
        let healing_count = db
            .with_reader(|conn| {
                Ok::<i64, DbError>(conn.query_row(
                    "SELECT
                         (SELECT COUNT(*) FROM healing_activations WHERE session_id = 's1') +
                         (SELECT COUNT(*) FROM healing_effect_windows WHERE session_id = 's1') +
                         (SELECT COUNT(*) FROM healing_outputs WHERE session_id = 's1')",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(healing_count, 0);
    }

    // ── Producer helpers ────────────────────────────────────────────

    #[test]
    fn validate_hotbar_needs_a_bound_slot() {
        let mut config = AppConfig::default();
        // The default hotbar is all-null: not workable.
        let (ok, message) = validate_hotbar(&config);
        assert!(!ok);
        assert!(message.unwrap().contains("Bind at least one"));

        // Binding one slot makes it workable.
        config.hotbar.insert("1".to_string(), json!(5));
        assert_eq!(validate_hotbar(&config), (true, None));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn declared_mob_label_reads_the_standing_declaration() {
        let mut config = AppConfig::default();

        // No declaration at all.
        assert_eq!(declared_mob_label(&config), Value::Null);

        // Species with and without maturity, and a whitespace-only species.
        config.manual_mob_species = "Argonaut".to_string();
        config.manual_mob_maturity = "Young".to_string();
        assert_eq!(declared_mob_label(&config), json!("Young Argonaut"));
        config.manual_mob_maturity = String::new();
        assert_eq!(declared_mob_label(&config), json!("Argonaut"));
        config.manual_mob_species = "  ".to_string();
        assert_eq!(declared_mob_label(&config), Value::Null);
    }

    #[test]
    fn notable_event_label_covers_known_and_fallbacks() {
        assert_eq!(notable_event_label("global_kill"), "Global Kill");
        assert_eq!(notable_event_label("global_item"), "Global Item");
        assert_eq!(notable_event_label("hof_kill"), "HoF Kill");
        assert_eq!(notable_event_label("hof_item"), "HoF Item");
        assert_eq!(notable_event_label("quest_started"), "Quest Started");
        assert_eq!(notable_event_label("quest_completed"), "Quest Completed");
        // Unknown types fall back to the category rendering.
        assert_eq!(notable_event_label("hof_other"), "HoF");
        assert_eq!(notable_event_label("quest_other"), "Quest");
        assert_eq!(notable_event_label("anything"), "Global");
    }

    #[test]
    fn notable_event_description_formats_quest_and_valued() {
        assert_eq!(
            notable_event_description("quest_started", "Dragon", 0.0),
            "Quest Started: Dragon"
        );
        assert_eq!(
            notable_event_description("global_kill", "Argonaut", 12.5),
            "Global Kill: Argonaut (12.50 PED)"
        );
    }

    #[test]
    fn capitalize_uppercases_the_first_char() {
        assert_eq!(capitalize("global"), "Global");
        assert_eq!(capitalize("a"), "A");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn clear_manual_mob_clears_species_and_maturity() {
        let updates = clear_manual_mob();
        assert_eq!(updates.get("manual_mob_species"), Some(&json!("")));
        assert_eq!(updates.get("manual_mob_maturity"), Some(&json!("")));
        assert_eq!(updates.len(), 2);
    }

    #[test]
    fn mob_display_joins_maturity_and_species() {
        assert_eq!(mob_display("Argonaut", "Young"), json!("Young Argonaut"));
        assert_eq!(mob_display("Argonaut", ""), json!("Argonaut"));
        assert_eq!(mob_display("", ""), Value::Null);
    }

    #[test]
    fn project_keeps_present_fields_in_order() {
        let value = json!({"a": 1, "b": 2, "c": 3});
        let projected = project(&value, &["b", "a", "z"]);
        // Present fields only, in the requested order.
        assert_eq!(
            serde_json::to_string(&projected).unwrap(),
            r#"{"b":2,"a":1}"#
        );
    }

    #[test]
    fn opt_str_reads_strings_only() {
        assert_eq!(opt_str(&json!("x")), Some("x".to_string()));
        assert_eq!(opt_str(&Value::Null), None);
        assert_eq!(opt_str(&json!(5)), None);
    }

    // ── Trifecta ────────────────────────────────────────────────────

    #[tokio::test]
    async fn trifecta_attribution_summary_builds_or_nulls() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_equipment(conn, 1, "Small Gun", "weapon")?;
            seed_equipment(conn, 2, "Heal Tool", "healing")?;
            Ok(())
        })
        .await
        .unwrap();

        let config = AppConfig {
            trifecta_presets: vec![TrifectaPresetConfig {
                id: "p1".to_string(),
                name: "Preset One".to_string(),
                small_weapon_id: Some(1),
                big_weapon_id: None,
                heal_id: Some(2),
            }],
            active_trifecta_preset_id: Some("p1".to_string()),
            ..AppConfig::default()
        };
        let summary = trifecta_attribution_summary(&db, &config).await.unwrap();
        assert_eq!(
            summary,
            json!({
                "activePresetId": "p1",
                "presetName": "Preset One",
                "presets": [{"id": "p1", "name": "Preset One"}],
                "smallWeapon": "Small Gun",
                "bigWeapon": null,
                "healTool": "Heal Tool",
            })
        );

        // No presets and no bound ids collapses to null.
        let empty = AppConfig {
            trifecta_presets: vec![],
            active_trifecta_preset_id: None,
            ..AppConfig::default()
        };
        assert_eq!(
            trifecta_attribution_summary(&db, &empty).await.unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn equipment_name_resolves_by_id_and_type() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| seed_equipment(conn, 1, "Small Gun", "weapon"))
            .await
            .unwrap();
        assert_eq!(
            equipment_name(&db, None, "weapon").await.unwrap(),
            Value::Null
        );
        assert_eq!(
            equipment_name(&db, Some(1), "weapon").await.unwrap(),
            json!("Small Gun")
        );
        // An absent id and a type mismatch both read null.
        assert_eq!(
            equipment_name(&db, Some(999), "weapon").await.unwrap(),
            Value::Null
        );
        assert_eq!(
            equipment_name(&db, Some(1), "healing").await.unwrap(),
            Value::Null
        );
    }

    // ── Return-rate zero-cost guards and active-duration ────────────

    /// A zero-cost summary row with loot returns a 0.0 rate, never a
    /// divide-by-zero: relaxing the `cost > 0.0` guard to `>=` would divide
    /// loot by zero and drop the field to null.
    #[test]
    fn list_row_from_summary_zero_cost_yields_a_zero_rate() {
        let summary = ListSummary {
            weapon_cost: 0.0,
            heal_cost: 0.0,
            enhancer_cost: 0.0,
            armour_cost: 0.0,
            dangling_cost: 0.0,
            harvest_cost: 0.0,
            loot_tt: 30.0,
            primary_mobs: json!([]),
            primary_weapons: json!([]),
            globals: 0,
            hofs: 0,
        };
        let row = list_row_from_summary("s1", Some(1000.0), Some(4600.0), false, 9999.0, &summary);
        assert_eq!(row["cost"], json!(0.0));
        assert_eq!(row["returns"], json!(30.0));
        assert_eq!(row["returnRate"], json!(0.0));
    }

    /// The same guard on the raw path: a session with no costs and a loot kill
    /// holds the rate at 0.0.
    #[tokio::test]
    async fn list_row_from_raw_zero_cost_yields_a_zero_rate() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(
                conn,
                "s1",
                1000.0,
                Some(4600.0),
                false,
                "mob",
                0.0,
                0.0,
                0.0,
            )?;
            // A loot kill with no tool rows and no enhancer cost: cost is zero.
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 0.0, 1000.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let row = db
            .with_reader(|conn| {
                list_row_from_raw(conn, "s1", Some(1000.0), Some(4600.0), false, 0.0)
            })
            .await
            .unwrap();
        assert_eq!(row["cost"], json!(0.0));
        assert_eq!(row["returns"], json!(30.0));
        assert_eq!(row["returnRate"], json!(0.0));
    }

    /// An active session with no end time measures its duration to `now` and
    /// guards a zero-cost rate. This pins two boundary conditions in the full
    /// detail read: the `is_active != 0` decode (mutating it to `== 0` would
    /// zero an active session's duration) and the `total_cost > 0.0` rate
    /// guard (relaxing it to `>=` would divide loot by zero).
    #[tokio::test]
    async fn get_session_read_active_zero_cost_measures_duration_and_guards_the_rate() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed_session(conn, "s1", 1000.0, None, true, "mob", 0.0, 0.0, 0.0)?;
            seed_kill(conn, "k1", "s1", Some("Argonaut"), None, 30.0, 0.0, 1000.0)?;
            Ok(())
        })
        .await
        .unwrap();

        let value = db
            .with_reader(|conn| get_session_read(conn, "s1", 4600.0))
            .await
            .unwrap()
            .expect("the session exists");
        assert_eq!(value["summary"]["duration"], json!(3600));
        assert_eq!(value["summary"]["cost"], json!(0.0));
        assert_eq!(value["summary"]["returnRate"], json!(0.0));
    }

    /// A preset exists but none is active, so every bound id is None while the
    /// preset list is non-empty. The early-return guard is a conjunction of
    /// four terms; turning any `&&` into `||` would collapse this real preset
    /// list to null.
    #[tokio::test]
    async fn trifecta_attribution_summary_keeps_an_unbound_preset_list() {
        let (_dir, db) = open_db().await;
        let config = AppConfig {
            trifecta_presets: vec![TrifectaPresetConfig {
                id: "p1".to_string(),
                name: "Preset One".to_string(),
                small_weapon_id: None,
                big_weapon_id: None,
                heal_id: None,
            }],
            active_trifecta_preset_id: None,
            ..AppConfig::default()
        };
        let summary = trifecta_attribution_summary(&db, &config).await.unwrap();
        assert_ne!(summary, Value::Null);
        assert_eq!(
            summary["presets"],
            json!([{"id": "p1", "name": "Preset One"}])
        );
        assert_eq!(summary["activePresetId"], Value::Null);
        assert_eq!(summary["smallWeapon"], Value::Null);
    }
}
