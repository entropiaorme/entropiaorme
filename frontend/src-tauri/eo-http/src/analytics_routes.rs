//! Native analytics reads (`backend/routers/analytics.py`): the
//! `/api/analytics/overview` and `/api/analytics/activity` GETs.
//!
//! These handlers are router-resident SQL aggregation: the reference keeps
//! every query and the camelCase shaping in the router itself (no service
//! layer), reading only the single-owner connection and the injected clock.
//! The port mirrors that, running the same statements over `self.pool()` and
//! `self.clock`, and shaping the result to the `AnalyticsOverview` /
//! `AnalyticsActivity` response models byte-for-byte.
//!
//! The fidelity crux is pydantic's response-model coercion. A field typed
//! `float` coerces an engine-typed integer to its float form (`0` -> `0.0`);
//! a field typed `Any` (the `cycledBreakdown` map and the `ledgerGains` /
//! `ledgerLosses` timeline maps) passes the value through untouched, so an
//! empty `COALESCE(SUM(...), 0)` leaves the wire as the integer `0`. The
//! `sql_number` reader preserves the engine type (the quest-analytics
//! precedent), `rounded` applies Python's type-preserving `round`, and
//! `float_field` performs the model's int-to-float coercion only where the
//! model declares a float.

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::tracker::naive_to_epoch;
use serde_json::{json, Map, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::hydration::{
    detail, error_response, internal_error, plain_json_response, HydrationState,
};

const ACTIVITY_DOMINANCE_THRESHOLD: f64 = 0.6;

// ── Engine-typed numeric primitives (the quest-analytics siblings in
//    eo-services::quests; kept local so this router stays self-contained,
//    matching the per-file formatter convention in hydration/character) ──

/// A SQLite numeric read preserving the engine type: a REAL decodes to a
/// float, an INTEGER (including the `COALESCE(SUM(...), 0)` empty case) to an
/// integer. `try_get::<f64>` rejects an integer-affinity value, so the
/// integer arm fires for the NULL-sum zeros.
fn sql_number(row: &SqliteRow, index: usize) -> Value {
    match row.try_get::<f64, _>(index) {
        Ok(value) => json!(value),
        Err(_) => json!(row.get::<i64, _>(index)),
    }
}

/// The sum of two engine-typed numbers, integer when both are (Python's `+`).
fn number_sum(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(left), Some(right)) => json!(left + right),
        _ => json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0)),
    }
}

/// `round(value, places)`: banker's rounding on a float, an integer left as
/// an integer (Python keeps `round(int, n)` an int).
fn rounded(value: &Value, places: usize) -> Value {
    match value.as_f64() {
        Some(number) if value.is_f64() => {
            json!(eo_wire::normalizer::round_half_even(number, places))
        }
        _ => value.clone(),
    }
}

/// A model-declared `float` field: coerce an engine-typed integer to its
/// float form, so an integer zero leaves the wire as `0.0`.
fn float_field(value: Value) -> Value {
    match value.as_i64() {
        Some(integer) => json!(integer as f64),
        None => value,
    }
}

/// `float(value)` over an engine-typed number (the activity path, where every
/// numeric is summed in float space).
fn as_float(row: &SqliteRow, index: usize) -> f64 {
    sql_number(row, index).as_f64().unwrap_or(0.0)
}

// ── Period + WHERE helpers (mirroring `_period_epoch` / `_where` /
//    `_where_iso` / `_epoch_to_iso`) ──

/// Epoch start for a named period, or `None` for all-time (and for any
/// unrecognised value, exactly as the reference's `dict.get` miss).
fn period_epoch(period: &str, now: f64) -> Option<f64> {
    let days = match period {
        "30d" => 30.0,
        "90d" => 90.0,
        "1y" => 365.0,
        _ => return None,
    };
    Some(now - days * 86400.0)
}

/// `datetime.fromtimestamp(epoch, tz=UTC).strftime("%Y-%m-%d")`.
fn epoch_to_iso(epoch: f64) -> String {
    chrono::DateTime::from_timestamp(epoch.floor() as i64, 0)
        .expect("epoch within range")
        .format("%Y-%m-%d")
        .to_string()
}

/// WHERE clause + epoch params for a numeric (unix-timestamp) column.
fn where_epoch(col: &str, start: Option<f64>, end: Option<f64>) -> (String, Vec<f64>) {
    let mut parts = Vec::new();
    let mut params = Vec::new();
    if let Some(s) = start {
        parts.push(format!("{col} >= ?"));
        params.push(s);
    }
    if let Some(e) = end {
        parts.push(format!("{col} < ?"));
        params.push(e);
    }
    (
        if parts.is_empty() {
            "1=1".to_string()
        } else {
            parts.join(" AND ")
        },
        params,
    )
}

/// WHERE clause + ISO-date params for an ISO-date TEXT column.
fn where_iso(col: &str, start: Option<f64>, end: Option<f64>) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut params = Vec::new();
    if let Some(s) = start {
        parts.push(format!("{col} >= ?"));
        params.push(epoch_to_iso(s));
    }
    if let Some(e) = end {
        parts.push(format!("{col} < ?"));
        params.push(epoch_to_iso(e));
    }
    (
        if parts.is_empty() {
            "1=1".to_string()
        } else {
            parts.join(" AND ")
        },
        params,
    )
}

/// Run a single-scalar epoch-filtered aggregate, returning the engine-typed
/// number (`COALESCE(SUM(...), 0)`).
async fn scalar_epoch(
    pool: &SqlitePool,
    sql: String,
    params: &[f64],
) -> Result<Value, sqlx::Error> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for value in params {
        query = query.bind(*value);
    }
    let row = query.fetch_one(pool).await?;
    Ok(sql_number(&row, 0))
}

/// A day/month-keyed aggregate (`SELECT <bucket>, COALESCE(SUM(...), 0) ...
/// GROUP BY <bucket>`) collected as `bucket -> engine-typed number`,
/// preserving the SQL row order.
async fn bucketed_epoch(
    pool: &SqlitePool,
    sql: String,
    params: &[f64],
) -> Result<Map<String, Value>, sqlx::Error> {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for value in params {
        query = query.bind(*value);
    }
    let rows = query.fetch_all(pool).await?;
    let mut out = Map::new();
    for row in &rows {
        out.insert(row.get::<String, _>(0), sql_number(row, 1));
    }
    Ok(out)
}

// ── _compute_metrics ──

/// The gains/losses breakdown for one window (`_compute_metrics`).
struct Metrics {
    loot_tt: Value,
    skill_tt: Value,
    codex_pes: Value,
    quest_pes: Value,
    weapon: Value,
    healing: Value,
    enhancer: Value,
    armour: Value,
    dangling: Value,
    tracking_cost: Value,
    ledger_gains: Map<String, Value>,
    ledger_losses: Map<String, Value>,
}

/// `SELECT le.tag, COALESCE(SUM(le.amount), 0) ... GROUP BY le.tag`, rounded
/// to two places and collected in SQL row order.
async fn ledger_by_tag(
    pool: &SqlitePool,
    entry_type: &str,
    led_w: &str,
    led_p: &[String],
) -> Result<Map<String, Value>, sqlx::Error> {
    let sql = format!(
        "SELECT le.tag, COALESCE(SUM(le.amount), 0) \
         FROM ledger_entries le WHERE le.type = '{entry_type}' AND {led_w} \
         GROUP BY le.tag"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for value in led_p {
        query = query.bind(value);
    }
    let rows = query.fetch_all(pool).await?;
    let mut out = Map::new();
    for row in &rows {
        out.insert(row.get::<String, _>(0), rounded(&sql_number(row, 1), 2));
    }
    Ok(out)
}

async fn compute_metrics(
    pool: &SqlitePool,
    epoch_start: Option<f64>,
    epoch_end: Option<f64>,
) -> Result<Metrics, sqlx::Error> {
    let (enc_w, enc_p) = where_epoch("k.timestamp", epoch_start, epoch_end);
    let (sg_w, sg_p) = where_epoch("sg.timestamp", epoch_start, epoch_end);
    let (led_w, led_p) = where_iso("le.date", epoch_start, epoch_end);
    let (cc_w, cc_p) = where_epoch("cc.claimed_at", epoch_start, epoch_end);
    let (qc_w, qc_p) = where_epoch("qc.claimed_at", epoch_start, epoch_end);
    let (sess_w, sess_p) = where_epoch("s.started_at", epoch_start, epoch_end);

    let loot_tt = scalar_epoch(
        pool,
        format!("SELECT COALESCE(SUM(k.loot_total_ped), 0) FROM kills k WHERE {enc_w}"),
        &enc_p,
    )
    .await?;

    let weapon = scalar_epoch(
        pool,
        format!(
            "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
             FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE {enc_w}"
        ),
        &enc_p,
    )
    .await?;

    let enhancer = scalar_epoch(
        pool,
        format!("SELECT COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE {enc_w}"),
        &enc_p,
    )
    .await?;

    let sess_row = {
        let sql = format!(
            "SELECT COALESCE(SUM(s.armour_cost), 0), COALESCE(SUM(s.heal_cost), 0), \
             COALESCE(SUM(s.dangling_cost), 0) FROM tracking_sessions s WHERE {sess_w}"
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for value in &sess_p {
            query = query.bind(*value);
        }
        query.fetch_one(pool).await?
    };
    let armour = sql_number(&sess_row, 0);
    let healing = sql_number(&sess_row, 1);
    let dangling = sql_number(&sess_row, 2);

    // weapon + heal + enhancer + armour + dangling (the reference's order).
    let tracking_cost = number_sum(
        &number_sum(
            &number_sum(&number_sum(&weapon, &healing), &enhancer),
            &armour,
        ),
        &dangling,
    );

    let skill_tt = scalar_epoch(
        pool,
        format!("SELECT COALESCE(SUM(sg.ped_value), 0) FROM skill_gains sg WHERE {sg_w}"),
        &sg_p,
    )
    .await?;
    let codex_pes = scalar_epoch(
        pool,
        format!("SELECT COALESCE(SUM(cc.ped_value), 0) FROM codex_claims cc WHERE {cc_w}"),
        &cc_p,
    )
    .await?;
    let quest_pes = scalar_epoch(
        pool,
        format!("SELECT COALESCE(SUM(qc.ped_value), 0) FROM quest_claims qc WHERE {qc_w}"),
        &qc_p,
    )
    .await?;

    let ledger_gains = ledger_by_tag(pool, "markup", &led_w, &led_p).await?;
    let ledger_losses = ledger_by_tag(pool, "expense", &led_w, &led_p).await?;

    Ok(Metrics {
        loot_tt,
        skill_tt,
        codex_pes,
        quest_pes,
        weapon,
        healing,
        enhancer,
        armour,
        dangling,
        tracking_cost,
        ledger_gains,
        ledger_losses,
    })
}

/// Sum of a ledger map's values in float space.
fn sum_values(map: &Map<String, Value>) -> f64 {
    map.values().filter_map(Value::as_f64).sum()
}

/// `_rate_from_metrics`: liquid gains over liquid losses (progression
/// excluded), 0.0 when losses are non-positive.
fn rate_from_metrics(m: &Metrics) -> f64 {
    let total_gains = m.loot_tt.as_f64().unwrap_or(0.0) + sum_values(&m.ledger_gains);
    let total_losses = m.tracking_cost.as_f64().unwrap_or(0.0) + sum_values(&m.ledger_losses);
    if total_losses > 0.0 {
        total_gains / total_losses
    } else {
        0.0
    }
}

/// `bucket -> {tag -> rounded amount}` from a `GROUP BY bucket, le.tag` query,
/// preserving SQL row order for both the outer and inner maps.
async fn ledger_buckets(
    pool: &SqlitePool,
    bucket_expr: &str,
    entry_type: &str,
    led_w: &str,
    led_p: &[String],
) -> Result<std::collections::BTreeMap<String, Map<String, Value>>, sqlx::Error> {
    let sql = format!(
        "SELECT {bucket_expr} as bucket, le.tag, COALESCE(SUM(le.amount), 0) \
         FROM ledger_entries le WHERE le.type = '{entry_type}' AND {led_w} \
         GROUP BY bucket, le.tag"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for value in led_p {
        query = query.bind(value);
    }
    let rows = query.fetch_all(pool).await?;
    let mut out: std::collections::BTreeMap<String, Map<String, Value>> =
        std::collections::BTreeMap::new();
    for row in &rows {
        let bucket = row.get::<String, _>(0);
        let tag = row.get::<String, _>(1);
        let amount = rounded(&sql_number(row, 2), 2);
        out.entry(bucket).or_default().insert(tag, amount);
    }
    Ok(out)
}

// ── overview_impl ──

async fn overview_impl(pool: &SqlitePool, now: f64, period: &str) -> Result<Value, sqlx::Error> {
    let epoch_start = period_epoch(period, now);

    let m = compute_metrics(pool, epoch_start, None).await?;
    let total_ledger_gains = sum_values(&m.ledger_gains);
    let total_ledger_losses = sum_values(&m.ledger_losses);
    let total_gains = m.loot_tt.as_f64().unwrap_or(0.0) + total_ledger_gains;
    let total_losses = m.tracking_cost.as_f64().unwrap_or(0.0) + total_ledger_losses;
    let return_rate = if total_losses > 0.0 {
        total_gains / total_losses
    } else {
        0.0
    };

    // Trend: always recent-30d vs prior-30d, independent of period.
    let day_30 = now - 30.0 * 86400.0;
    let day_60 = now - 60.0 * 86400.0;
    let rate_30d = rate_from_metrics(&compute_metrics(pool, Some(day_30), None).await?);
    let rate_prior = rate_from_metrics(&compute_metrics(pool, Some(day_60), Some(day_30)).await?);
    let trend = if rate_30d > 0.0 && rate_prior > 0.0 {
        if rate_30d > rate_prior * 1.02 {
            "improving"
        } else if rate_30d < rate_prior * 0.98 {
            "declining"
        } else {
            "stable"
        }
    } else {
        "stable"
    };

    // Daily breakdown (the point key is "date", the monthly point's is "month").
    let timeline = breakdown_points(pool, epoch_start, "date", BucketKind::Day).await?;
    // Monthly breakdown.
    let monthly = breakdown_points(pool, epoch_start, "month", BucketKind::Month).await?;

    let cycled_breakdown = json!({
        "weapon": rounded(&m.weapon, 2),
        "healing": rounded(&m.healing, 2),
        "enhancer": rounded(&m.enhancer, 2),
        "armour": rounded(&m.armour, 2),
        "dangling": rounded(&m.dangling, 2),
    });

    Ok(json!({
        "totalReturnRate": json!(eo_wire::normalizer::round_half_even(return_rate, 4)),
        "trend": trend,
        "returnsBreakdown": {
            "lootTt": float_field(rounded(&m.loot_tt, 2)),
            "pes": float_field(rounded(&m.skill_tt, 2)),
            "codexPes": float_field(rounded(&m.codex_pes, 2)),
            "questPes": float_field(rounded(&m.quest_pes, 2)),
            "ledger": coerce_ledger(&m.ledger_gains),
        },
        "lossesBreakdown": {
            "trackingCost": float_field(rounded(&m.tracking_cost, 2)),
            "cycledBreakdown": cycled_breakdown,
            "ledger": coerce_ledger(&m.ledger_losses),
        },
        "totalGains": json!(eo_wire::normalizer::round_half_even(total_gains, 2)),
        "totalLosses": json!(eo_wire::normalizer::round_half_even(total_losses, 2)),
        "timeline": timeline,
        "monthlyBreakdown": monthly,
    }))
}

/// A model `dict[str, float]` ledger map: coerce each value to its float form.
fn coerce_ledger(map: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for (tag, amount) in map {
        out.insert(tag.clone(), float_field(amount.clone()));
    }
    Value::Object(out)
}

#[derive(Clone, Copy)]
enum BucketKind {
    Day,
    Month,
}

/// Build the timeline / monthly breakdown: per-source bucketed sums merged
/// over the union of all buckets, then one point per bucket in sorted order.
async fn breakdown_points(
    pool: &SqlitePool,
    epoch_start: Option<f64>,
    bucket_label: &str,
    kind: BucketKind,
) -> Result<Value, sqlx::Error> {
    let (enc_w, enc_p) = where_epoch("k.timestamp", epoch_start, None);
    let (sg_w, sg_p) = where_epoch("sg.timestamp", epoch_start, None);
    let (cc_w, cc_p) = where_epoch("cc.claimed_at", epoch_start, None);
    let (qc_w, qc_p) = where_epoch("qc.claimed_at", epoch_start, None);
    let (sess_w, sess_p) = where_epoch("s.started_at", epoch_start, None);
    let (led_w, led_p) = where_iso("le.date", epoch_start, None);

    // The bucket expression differs for unix-timestamp columns vs the
    // ledger's ISO-date TEXT column (which strftime parses directly).
    let ts_bucket = |col: &str| match kind {
        BucketKind::Day => format!("date({col}, 'unixepoch')"),
        BucketKind::Month => format!("strftime('%Y-%m', {col}, 'unixepoch')"),
    };
    let iso_bucket = |col: &str| match kind {
        BucketKind::Day => col.to_string(),
        BucketKind::Month => format!("strftime('%Y-%m', {col})"),
    };

    let loot = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(k.loot_total_ped), 0) FROM kills k WHERE {enc_w} GROUP BY bucket",
            ts_bucket("k.timestamp")
        ),
        &enc_p,
    )
    .await?;
    let weapon = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
             FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id WHERE {enc_w} GROUP BY bucket",
            ts_bucket("k.timestamp")
        ),
        &enc_p,
    )
    .await?;
    let enhancer = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE {enc_w} GROUP BY bucket",
            ts_bucket("k.timestamp")
        ),
        &enc_p,
    )
    .await?;
    let sess = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(s.armour_cost), 0) + COALESCE(SUM(s.heal_cost), 0) \
             + COALESCE(SUM(s.dangling_cost), 0) FROM tracking_sessions s WHERE {sess_w} GROUP BY bucket",
            ts_bucket("s.started_at")
        ),
        &sess_p,
    )
    .await?;

    // cost = weapon + enhancer + sess over the union of their buckets.
    let mut cost: Map<String, Value> = Map::new();
    let mut cost_keys: BTreeSet<String> = BTreeSet::new();
    for k in weapon.keys().chain(enhancer.keys()).chain(sess.keys()) {
        cost_keys.insert(k.clone());
    }
    for key in &cost_keys {
        let zero = json!(0);
        let total = number_sum(
            &number_sum(
                weapon.get(key).unwrap_or(&zero),
                enhancer.get(key).unwrap_or(&zero),
            ),
            sess.get(key).unwrap_or(&zero),
        );
        cost.insert(key.clone(), total);
    }

    let skill = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(sg.ped_value), 0) FROM skill_gains sg WHERE {sg_w} GROUP BY bucket",
            ts_bucket("sg.timestamp")
        ),
        &sg_p,
    )
    .await?;
    let codex = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(cc.ped_value), 0) FROM codex_claims cc WHERE {cc_w} GROUP BY bucket",
            ts_bucket("cc.claimed_at")
        ),
        &cc_p,
    )
    .await?;
    let quest = bucketed_epoch(
        pool,
        format!(
            "SELECT {} as bucket, COALESCE(SUM(qc.ped_value), 0) FROM quest_claims qc WHERE {qc_w} GROUP BY bucket",
            ts_bucket("qc.claimed_at")
        ),
        &qc_p,
    )
    .await?;

    let gains = ledger_buckets(pool, &iso_bucket("le.date"), "markup", &led_w, &led_p).await?;
    let losses = ledger_buckets(pool, &iso_bucket("le.date"), "expense", &led_w, &led_p).await?;

    // all buckets, sorted (lexicographic == chronological for these forms).
    let mut all: BTreeSet<String> = BTreeSet::new();
    for k in loot
        .keys()
        .chain(cost.keys())
        .chain(skill.keys())
        .chain(codex.keys())
        .chain(quest.keys())
        .chain(gains.keys())
        .chain(losses.keys())
    {
        all.insert(k.clone());
    }

    let zero = json!(0);
    let mut points = Vec::new();
    for bucket in &all {
        points.push(json!({
            bucket_label: bucket,
            "lootTt": float_field(rounded(loot.get(bucket).unwrap_or(&zero), 4)),
            "pes": float_field(rounded(skill.get(bucket).unwrap_or(&zero), 4)),
            "codexPes": float_field(rounded(codex.get(bucket).unwrap_or(&zero), 4)),
            "questPes": float_field(rounded(quest.get(bucket).unwrap_or(&zero), 4)),
            "ledgerGains": gains.get(bucket).cloned().map(Value::Object).unwrap_or_else(|| json!({})),
            "trackingCost": float_field(rounded(cost.get(bucket).unwrap_or(&zero), 4)),
            "ledgerLosses": losses.get(bucket).cloned().map(Value::Object).unwrap_or_else(|| json!({})),
        }));
    }
    Ok(Value::Array(points))
}

// ── activity_impl ──

/// One completed session's aggregates (`_load_activity_sessions`).
#[derive(Default)]
struct SessionAgg {
    duration_hours: f64,
    armour_cost: f64,
    heal_cost: f64,
    dangling_cost: f64,
    weapon_cost: f64,
    enhancer_cost: f64,
    weapon_shots: f64,
    kills: i64,
    loot_tt: f64,
    skill_tt: f64,
    dominant_mob: Option<String>,
    dominant_mob_kills: i64,
    dominant_tag: Option<String>,
    dominant_tag_kills: i64,
    dominant_weapon: Option<String>,
    cycled_ped: f64,
}

async fn load_activity_sessions(pool: &SqlitePool) -> Result<Vec<SessionAgg>, sqlx::Error> {
    // Ordered map keyed by session id, preserving SELECT row order.
    let mut ids: Vec<String> = Vec::new();
    let mut sessions: std::collections::HashMap<String, SessionAgg> =
        std::collections::HashMap::new();

    let session_rows = sqlx::query(
        "SELECT id, started_at, ended_at, COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), \
         COALESCE(dangling_cost, 0) FROM tracking_sessions WHERE ended_at IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    for row in &session_rows {
        let id = row.get::<String, _>(0);
        let started: f64 = row.try_get::<f64, _>(1).unwrap_or(0.0);
        let ended: f64 = row.try_get::<f64, _>(2).unwrap_or(0.0);
        let duration_seconds = (ended - started).max(0.0);
        let agg = SessionAgg {
            duration_hours: duration_seconds / 3600.0,
            armour_cost: as_float(row, 3),
            heal_cost: as_float(row, 4),
            dangling_cost: as_float(row, 5),
            ..SessionAgg::default()
        };
        ids.push(id.clone());
        sessions.insert(id, agg);
    }

    if sessions.is_empty() {
        return Ok(Vec::new());
    }

    let kill_rows = sqlx::query(
        "SELECT session_id, COUNT(*), COALESCE(SUM(loot_total_ped), 0), \
         COALESCE(SUM(enhancer_cost), 0) FROM kills GROUP BY session_id",
    )
    .fetch_all(pool)
    .await?;
    for row in &kill_rows {
        // The FK session_id is NOT NULL in production; a NULL is skipped to
        // mirror the reference's `sessions.get(None)` miss (and to keep a
        // malformed row from aborting the decode) rather than to admit one.
        let Some(sid) = row.try_get::<Option<String>, _>(0).ok().flatten() else {
            continue;
        };
        if let Some(s) = sessions.get_mut(&sid) {
            s.kills = row.get::<i64, _>(1);
            s.loot_tt = as_float(row, 2);
            s.enhancer_cost = as_float(row, 3);
        }
    }

    let weapon_rows = sqlx::query(
        "SELECT k.session_id, COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0), \
         COALESCE(SUM(ts.shots_fired), 0) FROM kill_tool_stats ts \
         JOIN kills k ON k.id = ts.kill_id GROUP BY k.session_id",
    )
    .fetch_all(pool)
    .await?;
    for row in &weapon_rows {
        let Some(sid) = row.try_get::<Option<String>, _>(0).ok().flatten() else {
            continue;
        };
        if let Some(s) = sessions.get_mut(&sid) {
            s.weapon_cost = as_float(row, 1);
            s.weapon_shots = as_float(row, 2);
        }
    }

    let skill_rows = sqlx::query(
        "SELECT session_id, COALESCE(SUM(ped_value), 0) FROM skill_gains \
         WHERE ped_value IS NOT NULL GROUP BY session_id",
    )
    .fetch_all(pool)
    .await?;
    for row in &skill_rows {
        let Some(sid) = row.try_get::<Option<String>, _>(0).ok().flatten() else {
            continue;
        };
        if let Some(s) = sessions.get_mut(&sid) {
            s.skill_tt = as_float(row, 1);
        }
    }

    // Dominant mob/tag: groups per session, ordered COUNT desc then name asc.
    let group_rows = sqlx::query(
        "SELECT session_id, mob_name, COALESCE(mob_species, ''), COALESCE(mob_maturity, ''), \
         COUNT(*) FROM kills WHERE mob_name IS NOT NULL AND mob_name != 'Unknown' \
         GROUP BY session_id, mob_name, mob_species, mob_maturity \
         ORDER BY session_id, COUNT(*) DESC, mob_name ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut groups_by_session: std::collections::HashMap<
        String,
        Vec<(String, String, String, i64)>,
    > = std::collections::HashMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for row in &group_rows {
        let Some(sid) = row.try_get::<Option<String>, _>(0).ok().flatten() else {
            continue;
        };
        let entry = groups_by_session.entry(sid.clone()).or_insert_with(|| {
            group_order.push(sid.clone());
            Vec::new()
        });
        entry.push((
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<String, _>(3),
            row.get::<i64, _>(4),
        ));
    }
    for sid in &group_order {
        let groups = &groups_by_session[sid];
        let Some(s) = sessions.get_mut(sid) else {
            continue;
        };
        let total_known: i64 = groups.iter().map(|g| g.3).sum();
        if total_known <= 0 {
            continue;
        }
        let top = &groups[0];
        if (top.3 as f64 / total_known as f64) < ACTIVITY_DOMINANCE_THRESHOLD {
            continue;
        }
        if !top.1.is_empty() || !top.2.is_empty() {
            s.dominant_mob = Some(top.0.clone());
            s.dominant_mob_kills = top.3;
        } else {
            s.dominant_tag = Some(top.0.clone());
            s.dominant_tag_kills = top.3;
        }
    }

    // Dominant weapon: by total shots, ordered desc then name asc.
    let weapon_groups = sqlx::query(
        "SELECT k.session_id, ts.tool_name, COALESCE(SUM(ts.shots_fired), 0) as total_shots \
         FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
         WHERE ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' \
         GROUP BY k.session_id, ts.tool_name \
         ORDER BY k.session_id, total_shots DESC, ts.tool_name ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut weapons_by_session: std::collections::HashMap<String, Vec<(String, f64)>> =
        std::collections::HashMap::new();
    let mut weapon_order: Vec<String> = Vec::new();
    for row in &weapon_groups {
        let Some(sid) = row.try_get::<Option<String>, _>(0).ok().flatten() else {
            continue;
        };
        let entry = weapons_by_session.entry(sid.clone()).or_insert_with(|| {
            weapon_order.push(sid.clone());
            Vec::new()
        });
        entry.push((row.get::<String, _>(1), as_float(row, 2)));
    }
    for sid in &weapon_order {
        let groups = &weapons_by_session[sid];
        let Some(s) = sessions.get_mut(sid) else {
            continue;
        };
        let total_shots: f64 = groups.iter().map(|g| g.1).sum();
        if total_shots <= 0.0 {
            continue;
        }
        let top = &groups[0];
        if (top.1 / total_shots) >= ACTIVITY_DOMINANCE_THRESHOLD {
            s.dominant_weapon = Some(top.0.clone());
        }
    }

    // cycledPed + the three filters, in original session order.
    let mut result = Vec::new();
    for id in &ids {
        let mut s = sessions.remove(id).expect("session present");
        s.cycled_ped = eo_wire::normalizer::round_half_even(
            s.weapon_cost + s.enhancer_cost + s.armour_cost + s.heal_cost + s.dangling_cost,
            4,
        );
        if s.duration_hours <= 0.0 || s.cycled_ped <= 0.0 || s.kills <= 0 {
            continue;
        }
        result.push(s);
    }
    Ok(result)
}

/// `_build_activity_slice_rows`: group sessions by a dominant field, sum the
/// per-group stats, and sort by (-kills, -cycled, name).
fn build_activity_slice_rows(
    sessions: &[SessionAgg],
    select: impl Fn(&SessionAgg) -> Option<String>,
    kills_of: impl Fn(&SessionAgg) -> i64,
    name_field: &str,
) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<&SessionAgg>> =
        std::collections::HashMap::new();
    for session in sessions {
        if let Some(value) = select(session) {
            if value.is_empty() {
                continue;
            }
            grouped.entry(value.clone()).or_insert_with(|| {
                order.push(value.clone());
                Vec::new()
            });
            grouped.get_mut(&value).unwrap().push(session);
        }
    }

    let mut rows: Vec<(i64, f64, String, Value)> = Vec::new();
    for value in &order {
        let matched = &grouped[value];
        let sessions_count = matched.len() as i64;
        let kills: i64 = matched.iter().map(|s| kills_of(s)).sum();
        let hours: f64 = matched.iter().map(|s| s.duration_hours).sum();
        let cycled: f64 = matched.iter().map(|s| s.cycled_ped).sum();
        let loot_tt: f64 = matched.iter().map(|s| s.loot_tt).sum();
        let skill_tt: f64 = matched.iter().map(|s| s.skill_tt).sum();
        let hours_r = eo_wire::normalizer::round_half_even(hours, 2);
        let cycled_r = eo_wire::normalizer::round_half_even(cycled, 2);
        let pes_per_100 = if cycled > 0.0 {
            eo_wire::normalizer::round_half_even((skill_tt / cycled) * 100.0, 2)
        } else {
            0.0
        };
        let loot_rate = if cycled > 0.0 {
            eo_wire::normalizer::round_half_even(loot_tt / cycled, 4)
        } else {
            0.0
        };
        let row = json!({
            name_field: value,
            "sessions": sessions_count,
            "kills": kills,
            "hours": hours_r,
            "cycled": cycled_r,
            "pesPer100Ped": pes_per_100,
            "lootRate": loot_rate,
        });
        rows.push((kills, cycled, value.clone(), row));
    }
    // sort by (-kills, -cycled, name)
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.2.cmp(&b.2))
    });
    rows.into_iter().map(|(_, _, _, row)| row).collect()
}

async fn activity_impl(pool: &SqlitePool) -> Result<Value, sqlx::Error> {
    let sessions = load_activity_sessions(pool).await?;
    let mob = build_activity_slice_rows(
        &sessions,
        |s| s.dominant_mob.clone(),
        |s| s.dominant_mob_kills,
        "mobName",
    );
    let tag = build_activity_slice_rows(
        &sessions,
        |s| s.dominant_tag.clone(),
        |s| s.dominant_tag_kills,
        "tagName",
    );
    // Weapon comparisons inline the helper but key kills off the session
    // total (not a dominant-weapon kill count).
    let weapon = build_activity_slice_rows(
        &sessions,
        |s| s.dominant_weapon.clone(),
        |s| s.kills,
        "weaponName",
    );
    Ok(json!({
        "mobComparisons": mob,
        "tagComparisons": tag,
        "weaponComparisons": weapon,
    }))
}

// ── The two handlers on the composition-root state ──

impl HydrationState {
    /// GET /api/analytics/overview?period=...
    pub async fn analytics_overview(&self, period: &str) -> Response<Body> {
        let now = naive_to_epoch(self.clock.now());
        match overview_impl(self.pool(), now, period).await {
            Ok(value) => plain_json_response(&value),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/analytics/activity (no conditional-GET contract: the
    /// analytics surface is outside the ETag middleware's prefixes).
    pub async fn analytics_activity(&self, _if_none_match: Option<&str>) -> Response<Body> {
        match activity_impl(self.pool()).await {
            Ok(value) => plain_json_response(&value),
            Err(_) => internal_error(),
        }
    }
}

// ── Ledger / presets / inventory writes (the CRUD surface) ──

const INVENTORY_SALE_TAG: &str = "inventory_sale";

/// `LedgerItem` / `LedgerPresetItem` share a shape; both select
/// (id, name-or-date, type, description, amount, tag).
fn ledger_item(row: &SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>(0),
        "date": row.get::<String, _>(1),
        "type": row.get::<String, _>(2),
        "description": row.get::<String, _>(3),
        "amount": float_field(sql_number(row, 4)),
        "tag": row.get::<String, _>(5),
    })
}

fn preset_item(row: &SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>(0),
        "name": row.get::<String, _>(1),
        "type": row.get::<String, _>(2),
        "description": row.get::<String, _>(3),
        "amount": float_field(sql_number(row, 4)),
        "tag": row.get::<String, _>(5),
    })
}

/// `_inventory_row_to_dict`: (id, name, tt_value, markup_paid, notes, acquired_at).
fn inventory_item(row: &SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>(0),
        "name": row.get::<String, _>(1),
        "ttValue": float_field(sql_number(row, 2)),
        "markupPaid": float_field(sql_number(row, 3)),
        "notes": row.get::<Option<String>, _>(4),
        "acquiredAt": row.get::<String, _>(5),
    })
}

impl HydrationState {
    /// `_utc_date_str(clock)`: the clock's instant as a UTC YYYY-MM-DD date.
    fn default_date(&self) -> String {
        epoch_to_iso(naive_to_epoch(self.clock.now()))
    }

    /// GET /api/analytics/ledger
    pub async fn list_ledger(&self) -> Response<Body> {
        match sqlx::query(
            "SELECT id, date, type, description, amount, tag FROM ledger_entries \
             ORDER BY date DESC, id DESC",
        )
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => plain_json_response(&Value::Array(rows.iter().map(ledger_item).collect())),
            Err(_) => internal_error(),
        }
    }

    /// POST /api/analytics/ledger
    pub async fn create_ledger_entry(
        &self,
        date: &str,
        kind: &str,
        description: &str,
        amount: f64,
        tag: &str,
    ) -> Response<Body> {
        let id = Uuid::new_v4().to_string();
        match sqlx::query(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(date)
        .bind(kind)
        .bind(description)
        .bind(amount)
        .bind(tag)
        .execute(self.pool())
        .await
        {
            Ok(_) => plain_json_response(&json!({
                "id": id, "date": date, "type": kind,
                "description": description, "amount": amount, "tag": tag,
            })),
            Err(_) => internal_error(),
        }
    }

    /// DELETE /api/analytics/ledger/{entry_id}
    pub async fn delete_ledger_entry(&self, entry_id: &str) -> Response<Body> {
        match sqlx::query("DELETE FROM ledger_entries WHERE id = ?")
            .bind(entry_id)
            .execute(self.pool())
            .await
        {
            Ok(result) if result.rows_affected() == 0 => {
                error_response(StatusCode::NOT_FOUND, &detail("Entry not found"))
            }
            Ok(_) => plain_json_response(&json!({"status": "deleted"})),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/analytics/ledger/presets
    pub async fn list_ledger_presets(&self) -> Response<Body> {
        match sqlx::query(
            "SELECT id, name, type, description, amount, tag FROM ledger_presets \
             ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => plain_json_response(&Value::Array(rows.iter().map(preset_item).collect())),
            Err(_) => internal_error(),
        }
    }

    /// POST /api/analytics/ledger/presets
    pub async fn create_ledger_preset(
        &self,
        name: &str,
        kind: &str,
        description: &str,
        amount: f64,
        tag: &str,
    ) -> Response<Body> {
        if kind != "expense" && kind != "markup" {
            return error_response(
                StatusCode::BAD_REQUEST,
                &detail("type must be 'expense' or 'markup'"),
            );
        }
        let id = Uuid::new_v4().to_string();
        match sqlx::query(
            "INSERT INTO ledger_presets (id, name, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(kind)
        .bind(description)
        .bind(amount)
        .bind(tag)
        .execute(self.pool())
        .await
        {
            Ok(_) => plain_json_response(&json!({
                "id": id, "name": name, "type": kind,
                "description": description, "amount": amount, "tag": tag,
            })),
            Err(_) => internal_error(),
        }
    }

    /// DELETE /api/analytics/ledger/presets/{preset_id}
    pub async fn delete_ledger_preset(&self, preset_id: &str) -> Response<Body> {
        match sqlx::query("DELETE FROM ledger_presets WHERE id = ?")
            .bind(preset_id)
            .execute(self.pool())
            .await
        {
            Ok(result) if result.rows_affected() == 0 => {
                error_response(StatusCode::NOT_FOUND, &detail("Preset not found"))
            }
            Ok(_) => plain_json_response(&json!({"status": "deleted"})),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/analytics/inventory
    pub async fn list_inventory(&self) -> Response<Body> {
        match sqlx::query(
            "SELECT id, name, tt_value, markup_paid, notes, acquired_at FROM inventory_items \
             ORDER BY acquired_at DESC, id DESC",
        )
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => {
                plain_json_response(&Value::Array(rows.iter().map(inventory_item).collect()))
            }
            Err(_) => internal_error(),
        }
    }

    /// The stored inventory row re-read and shaped (create / patch reply).
    async fn inventory_response(&self, item_id: &str) -> Response<Body> {
        match sqlx::query(
            "SELECT id, name, tt_value, markup_paid, notes, acquired_at \
             FROM inventory_items WHERE id = ?",
        )
        .bind(item_id)
        .fetch_optional(self.pool())
        .await
        {
            Ok(Some(row)) => plain_json_response(&inventory_item(&row)),
            _ => internal_error(),
        }
    }

    /// POST /api/analytics/inventory
    pub async fn create_inventory_item(
        &self,
        name: &str,
        tt_value: f64,
        markup_paid: f64,
        notes: Option<&str>,
        acquired_at: Option<&str>,
    ) -> Response<Body> {
        let id = Uuid::new_v4().to_string();
        // `item.acquired_at or _utc_date_str(clock)`: the reference's `or`
        // treats an empty string as falsy, so "" defaults to the clock date.
        let date = acquired_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        if sqlx::query(
            "INSERT INTO inventory_items (id, name, tt_value, markup_paid, notes, acquired_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(tt_value)
        .bind(markup_paid)
        .bind(notes)
        .bind(&date)
        .execute(self.pool())
        .await
        .is_err()
        {
            return internal_error();
        }
        self.inventory_response(&id).await
    }

    /// PATCH /api/analytics/inventory/{item_id}: only provided (non-null)
    /// fields update, bumping updated_at; an absent body of fields still
    /// re-reads and returns the row (the reference's shape).
    pub async fn update_inventory_item(
        &self,
        item_id: &str,
        name: Option<&str>,
        tt_value: Option<f64>,
        markup_paid: Option<f64>,
        notes: Option<&str>,
    ) -> Response<Body> {
        match sqlx::query("SELECT id FROM inventory_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(self.pool())
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error_response(StatusCode::NOT_FOUND, &detail("Inventory item not found"))
            }
            Err(_) => return internal_error(),
        }

        let mut sets: Vec<&str> = Vec::new();
        if name.is_some() {
            sets.push("name = ?");
        }
        if tt_value.is_some() {
            sets.push("tt_value = ?");
        }
        if markup_paid.is_some() {
            sets.push("markup_paid = ?");
        }
        if notes.is_some() {
            sets.push("notes = ?");
        }
        if !sets.is_empty() {
            sets.push("updated_at = unixepoch('now')");
            let sql = format!(
                "UPDATE inventory_items SET {} WHERE id = ?",
                sets.join(", ")
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            if let Some(value) = name {
                query = query.bind(value);
            }
            if let Some(value) = tt_value {
                query = query.bind(value);
            }
            if let Some(value) = markup_paid {
                query = query.bind(value);
            }
            if let Some(value) = notes {
                query = query.bind(value);
            }
            query = query.bind(item_id);
            if query.execute(self.pool()).await.is_err() {
                return internal_error();
            }
        }
        self.inventory_response(item_id).await
    }

    /// DELETE /api/analytics/inventory/{item_id}
    pub async fn delete_inventory_item(&self, item_id: &str) -> Response<Body> {
        match sqlx::query("DELETE FROM inventory_items WHERE id = ?")
            .bind(item_id)
            .execute(self.pool())
            .await
        {
            Ok(result) if result.rows_affected() == 0 => {
                error_response(StatusCode::NOT_FOUND, &detail("Inventory item not found"))
            }
            Ok(_) => plain_json_response(&json!({"status": "deleted"})),
            Err(_) => internal_error(),
        }
    }

    /// POST /api/analytics/inventory/{item_id}/sell: emit the realised delta
    /// to the ledger and remove the row, atomically; a zero-delta sale skips
    /// the ledger row and returns ledgerEntry null.
    pub async fn sell_inventory_item(
        &self,
        item_id: &str,
        sale_price: f64,
        description: Option<&str>,
        sold_at: Option<&str>,
    ) -> Response<Body> {
        let row = match sqlx::query(
            "SELECT id, name, tt_value, markup_paid, notes, acquired_at \
             FROM inventory_items WHERE id = ?",
        )
        .bind(item_id)
        .fetch_optional(self.pool())
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                return error_response(StatusCode::NOT_FOUND, &detail("Inventory item not found"))
            }
            Err(_) => return internal_error(),
        };

        let name = row.get::<String, _>(1);
        let tt_value = sql_number(&row, 2).as_f64().unwrap_or(0.0);
        let markup_paid = sql_number(&row, 3).as_f64().unwrap_or(0.0);
        let cost_basis = tt_value + markup_paid;
        let delta = sale_price - cost_basis;
        // `payload.sold_at or _utc_date_str(clock)`: empty string is falsy.
        let sold_at = sold_at
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.default_date());
        let sold_item = inventory_item(&row);

        let mut tx = match self.pool().begin().await {
            Ok(tx) => tx,
            Err(_) => return internal_error(),
        };
        let ledger_entry = if delta != 0.0 {
            let entry_id = Uuid::new_v4().to_string();
            let entry_type = if delta > 0.0 { "markup" } else { "expense" };
            let amount = delta.abs();
            // `payload.description or "Inventory Sale: {name}"`: "" is falsy.
            let description = description
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Inventory Sale: {name}"));
            if sqlx::query(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&entry_id)
            .bind(&sold_at)
            .bind(entry_type)
            .bind(&description)
            .bind(amount)
            .bind(INVENTORY_SALE_TAG)
            .execute(&mut *tx)
            .await
            .is_err()
            {
                return internal_error();
            }
            json!({
                "id": entry_id, "date": sold_at, "type": entry_type,
                "description": description, "amount": amount, "tag": INVENTORY_SALE_TAG,
            })
        } else {
            Value::Null
        };
        if sqlx::query("DELETE FROM inventory_items WHERE id = ?")
            .bind(item_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return internal_error();
        }
        if tx.commit().await.is_err() {
            return internal_error();
        }
        plain_json_response(&json!({"ledgerEntry": ledger_entry, "soldItem": sold_item}))
    }
}

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
             armour_cost REAL, heal_cost REAL, dangling_cost REAL)",
            "CREATE TABLE kills(id TEXT PRIMARY KEY, session_id TEXT, mob_name TEXT, \
             mob_species TEXT, mob_maturity TEXT, timestamp REAL, enhancer_cost REAL, \
             loot_total_ped REAL)",
            "CREATE TABLE kill_tool_stats(id INTEGER PRIMARY KEY, kill_id TEXT, tool_name TEXT, \
             shots_fired INTEGER, cost_per_shot REAL)",
            "CREATE TABLE skill_gains(id INTEGER PRIMARY KEY, session_id TEXT, timestamp REAL, \
             ped_value REAL)",
            "CREATE TABLE codex_claims(id INTEGER PRIMARY KEY, claimed_at REAL, ped_value REAL)",
            "CREATE TABLE quest_claims(id INTEGER PRIMARY KEY, claimed_at REAL, ped_value REAL)",
            "CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, date TEXT, type TEXT, \
             description TEXT, amount REAL, tag TEXT)",
            "CREATE TABLE ledger_presets(id TEXT PRIMARY KEY, name TEXT, type TEXT, \
             description TEXT, amount REAL, tag TEXT, created_at REAL)",
            "CREATE TABLE inventory_items(id TEXT PRIMARY KEY, name TEXT, tt_value REAL, \
             markup_paid REAL, notes TEXT, acquired_at TEXT, updated_at REAL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.expect("ddl");
        }
        pool
    }

    /// A minimal `HydrationState` over an in-memory pool, its clock frozen so
    /// `default_date()` (`_utc_date_str(clock)`) is deterministic. The
    /// game-data store loads empty (no snapshot dir), which the write surface
    /// never touches.
    async fn write_state() -> crate::hydration::HydrationState {
        use eo_services::clock::MockClock;
        use eo_services::game_data_store::GameDataStore;
        use std::path::Path;
        use std::sync::Arc;
        let pool = memory_pool().await;
        let db = eo_services::db::Db::from_pool(pool);
        let naive =
            chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S")
                .unwrap();
        crate::hydration::HydrationState::new(
            db,
            Arc::new(GameDataStore::new(Path::new("/nonexistent/snapshot")).unwrap()),
            Arc::new(MockClock::new(Some(naive), 0.0)),
            std::path::PathBuf::from("."),
        )
    }

    /// Status + parsed JSON body of a handler response.
    async fn body_of(
        response: axum::http::Response<axum::body::Body>,
    ) -> (axum::http::StatusCode, Value) {
        use http_body_util::BodyExt;
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes()
            .to_vec();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn empty_overview_emits_the_engine_typed_zeros() {
        let pool = memory_pool().await;
        let value = overview_impl(&pool, 1_800_000_000.0, "all").await.unwrap();
        // cycledBreakdown is an `Any` field: empty COALESCE sums leave the
        // integer zero on the wire, while the float-declared aggregates coerce.
        assert_eq!(
            to_wire_json(&value),
            "{\"totalReturnRate\":0.0,\"trend\":\"stable\",\"returnsBreakdown\":{\"lootTt\":0.0,\
             \"pes\":0.0,\"codexPes\":0.0,\"questPes\":0.0,\"ledger\":{}},\"lossesBreakdown\":\
             {\"trackingCost\":0.0,\"cycledBreakdown\":{\"weapon\":0,\"healing\":0,\"enhancer\":0,\
             \"armour\":0,\"dangling\":0},\"ledger\":{}},\"totalGains\":0.0,\"totalLosses\":0.0,\
             \"timeline\":[],\"monthlyBreakdown\":[]}"
        );
    }

    #[tokio::test]
    async fn empty_activity_emits_three_empty_tables() {
        let pool = memory_pool().await;
        let value = activity_impl(&pool).await.unwrap();
        assert_eq!(
            to_wire_json(&value),
            "{\"mobComparisons\":[],\"tagComparisons\":[],\"weaponComparisons\":[]}"
        );
    }

    /// Seed the representative scenario the live probe grounded, with the
    /// window relative to a fixed `now`, and assert the computed aggregates,
    /// the trend, dominance, and the filters.
    async fn seed_scenario(pool: &SqlitePool, now: f64) {
        let day = 86400.0;
        let recent = now - 11.0 * day; // inside the 30d window
        let prior = now - 37.0 * day; // inside the 30-60d window
                                      // sessions
        for (id, start, armour, heal, dangling) in [
            ("sess-a", recent, 1.0, 2.0, 0.5),
            ("sess-b", prior, 0.5, 1.0, 0.0),
            ("sess-z", recent, 0.0, 0.0, 0.0), // zero-kill, zero-cost: filtered from activity
        ] {
            sqlx::query(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
                 VALUES(?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(start)
            .bind(start + 3600.0)
            .bind(armour)
            .bind(heal)
            .bind(dangling)
            .execute(pool)
            .await
            .expect("seed");
        }
        for i in 0..5 {
            let kid = format!("k-a-{i}");
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,?,?,?,?,?,?,?)",
            )
            .bind(&kid).bind("sess-a").bind("Atrox").bind("Atrox").bind("Young")
            .bind(recent + i as f64).bind(0.1).bind(10.0)
            .execute(pool).await.expect("seed");
            sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?,?,?,?)")
                .bind(&kid).bind("Opalo").bind(50_i64).bind(0.011)
                .execute(pool).await.expect("seed");
        }
        for i in 0..3 {
            let kid = format!("k-b-{i}");
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,?,?,NULL,NULL,?,?,?)",
            )
            .bind(&kid).bind("sess-b").bind("Thing")
            .bind(prior + i as f64).bind(0.0).bind(5.0)
            .execute(pool).await.expect("seed");
            sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?,?,?,?)")
                .bind(&kid).bind("Opalo").bind(30_i64).bind(0.01)
                .execute(pool).await.expect("seed");
        }
        sqlx::query("INSERT INTO skill_gains(session_id,timestamp,ped_value) VALUES(?,?,?)")
            .bind("sess-a")
            .bind(recent)
            .bind(3.0)
            .execute(pool)
            .await
            .expect("seed");
        sqlx::query("INSERT INTO skill_gains(session_id,timestamp,ped_value) VALUES(?,?,?)")
            .bind("sess-b")
            .bind(prior)
            .bind(1.0)
            .execute(pool)
            .await
            .expect("seed");
        sqlx::query("INSERT INTO codex_claims(claimed_at,ped_value) VALUES(?,?)")
            .bind(recent)
            .bind(7.0)
            .execute(pool)
            .await
            .expect("seed");
        sqlx::query("INSERT INTO quest_claims(claimed_at,ped_value) VALUES(?,?)")
            .bind(recent)
            .bind(4.0)
            .execute(pool)
            .await
            .expect("seed");
        // ledger: a recent markup and a prior expense, dated by the ISO form.
        sqlx::query(
            "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?,?,?,?,?,?)",
        )
        .bind("led-1")
        .bind(epoch_to_iso(recent))
        .bind("markup")
        .bind("Sold hides")
        .bind(12.5)
        .bind("loot_sale")
        .execute(pool)
        .await
        .expect("seed");
        sqlx::query(
            "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?,?,?,?,?,?)",
        )
        .bind("led-2")
        .bind(epoch_to_iso(prior))
        .bind("expense")
        .bind("Deposit")
        .bind(8.0)
        .bind("deposit")
        .execute(pool)
        .await
        .expect("seed");
    }

    #[tokio::test]
    async fn seeded_overview_aggregates_match() {
        let now = 1_800_000_000.0;
        let pool = memory_pool().await;
        seed_scenario(&pool, now).await;
        let v = overview_impl(&pool, now, "all").await.unwrap();
        assert_eq!(v["returnsBreakdown"]["lootTt"], json!(65.0));
        assert_eq!(v["returnsBreakdown"]["pes"], json!(4.0));
        assert_eq!(v["returnsBreakdown"]["codexPes"], json!(7.0));
        assert_eq!(v["returnsBreakdown"]["ledger"]["loot_sale"], json!(12.5));
        assert_eq!(v["lossesBreakdown"]["trackingCost"], json!(9.15));
        assert_eq!(
            v["lossesBreakdown"]["cycledBreakdown"]["weapon"],
            json!(3.65)
        );
        assert_eq!(
            v["lossesBreakdown"]["cycledBreakdown"]["armour"],
            json!(1.5)
        );
        assert_eq!(v["lossesBreakdown"]["ledger"]["deposit"], json!(8.0));
        // totalGains = loot 65 + markup 12.5; totalLosses = cost 9.15 + expense 8.0.
        assert_eq!(v["totalGains"], json!(77.5));
        assert_eq!(v["totalLosses"], json!(17.15));
        assert_eq!(v["totalReturnRate"], json!(4.519));
        // timeline points key the day as "date"; monthly points as "month".
        assert!(v["timeline"][0].get("date").is_some());
        assert!(v["monthlyBreakdown"][0].get("month").is_some());
        // trend: recent-30d rate exceeds prior-30d rate beyond the 2% band.
        assert_eq!(v["trend"], json!("improving"));
        // period filter: 30d keeps only the recent window (markup in, expense out).
        let v30 = overview_impl(&pool, now, "30d").await.unwrap();
        assert_eq!(v30["returnsBreakdown"]["lootTt"], json!(50.0));
        assert_eq!(v30["returnsBreakdown"]["ledger"]["loot_sale"], json!(12.5));
        assert_eq!(v30["lossesBreakdown"]["ledger"], json!({}));
        assert_eq!(v30["timeline"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn seeded_activity_dominance_and_filters() {
        let now = 1_800_000_000.0;
        let pool = memory_pool().await;
        seed_scenario(&pool, now).await;
        let v = activity_impl(&pool).await.unwrap();
        // sess-z (zero kills) filtered out; sess-a -> dominant mob, sess-b -> tag.
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["mobName"], json!("Atrox"));
        assert_eq!(mobs[0]["kills"], json!(5));
        assert_eq!(mobs[0]["hours"], json!(1.0)); // 3600s / 3600
        assert_eq!(mobs[0]["cycled"], json!(6.75));
        // pesPer100Ped = (skill 3.0 / cycled 6.75) * 100; lootRate = loot 50 / cycled.
        assert_eq!(mobs[0]["pesPer100Ped"], json!(44.44));
        assert_eq!(mobs[0]["lootRate"], json!(7.4074));
        let tags = v["tagComparisons"].as_array().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0]["tagName"], json!("Thing"));
        assert_eq!(tags[0]["kills"], json!(3));
        assert_eq!(tags[0]["cycled"], json!(2.4));
        assert_eq!(tags[0]["pesPer100Ped"], json!(41.67));
        assert_eq!(tags[0]["lootRate"], json!(6.25));
        // weapon comparison keys kills off the session total (5 + 3 = 8) and
        // aggregates both sessions' hours / cycled / rates.
        let weapons = v["weaponComparisons"].as_array().unwrap();
        assert_eq!(weapons.len(), 1);
        assert_eq!(weapons[0]["weaponName"], json!("Opalo"));
        assert_eq!(weapons[0]["kills"], json!(8));
        assert_eq!(weapons[0]["hours"], json!(2.0));
        assert_eq!(weapons[0]["cycled"], json!(9.15));
        assert_eq!(weapons[0]["pesPer100Ped"], json!(43.72));
        assert_eq!(weapons[0]["lootRate"], json!(7.1038));
    }

    /// The activity filter drops a session failing ANY of the three guards
    /// (duration > 0, cycled > 0, kills > 0); `||` not `&&`. Three sessions,
    /// each dominated by its own mob, each failing exactly one guard except
    /// the keeper: only the keeper's mob survives.
    #[tokio::test]
    async fn activity_filter_drops_a_session_failing_any_single_guard() {
        let pool = memory_pool().await;
        // keeper: kills, duration, cost all positive.
        seed_filter_session(&pool, "keep", "Keeper", 1000.0, 1000.0 + 3600.0, 5.0, 2).await;
        // zero cost -> cycled 0 -> dropped by the cycled guard alone.
        seed_filter_session(&pool, "zcost", "Zerocost", 1000.0, 1000.0 + 3600.0, 0.0, 2).await;
        // zero duration (start == end) -> dropped by the duration guard alone.
        seed_filter_session(&pool, "zdur", "Zerodur", 1000.0, 1000.0, 5.0, 2).await;
        let v = activity_impl(&pool).await.unwrap();
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1, "only the keeper survives the OR filter");
        assert_eq!(mobs[0]["mobName"], json!("Keeper"));
    }

    async fn seed_filter_session(
        pool: &SqlitePool,
        id: &str,
        mob: &str,
        start: f64,
        end: f64,
        armour: f64,
        kills: i64,
    ) {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
             VALUES(?,?,?,?,0,0)",
        )
        .bind(id).bind(start).bind(end).bind(armour)
        .execute(pool).await.expect("seed");
        for i in 0..kills {
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,?,?,?,?,?,?,?)",
            )
            .bind(format!("{id}-k{i}")).bind(id).bind(mob).bind("Spec").bind("Young")
            .bind(start + i as f64).bind(0.0).bind(1.0)
            .execute(pool).await.expect("seed");
        }
    }

    /// Seed one session (cost via armour) and `kills` loot rows at `ts`, so a
    /// window's rate is loot_total / armour_cost.
    async fn seed_rate(pool: &SqlitePool, id: &str, ts: f64, cost: f64, kills: i64, loot: f64) {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
             VALUES(?,?,?,?,0,0)",
        )
        .bind(id).bind(ts).bind(ts + 3600.0).bind(cost)
        .execute(pool).await.expect("seed");
        for i in 0..kills {
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,?,?,?,0,?)",
            )
            .bind(format!("{id}-k{i}"))
            .bind(id)
            .bind("M")
            .bind(ts + i as f64)
            .bind(loot)
            .execute(pool)
            .await
            .expect("seed");
        }
    }

    /// The trend compares the recent-30d rate against the prior-30d rate with
    /// a +/-2% band, guarded by both rates being positive.
    #[tokio::test]
    async fn overview_trend_bands() {
        let now = 1_800_000_000.0;
        let day = 86400.0;
        let trend = |v: Value| v["trend"].clone();

        // declining: recent rate 1.0 (10/10) below prior 2.0 (20/10) * 0.98.
        let pool = memory_pool().await;
        seed_rate(&pool, "r", now - 10.0 * day, 10.0, 1, 10.0).await;
        seed_rate(&pool, "p", now - 45.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&pool, now, "all").await.unwrap()),
            json!("declining")
        );

        // improving: recent 2.0 above prior 1.0 * 1.02.
        let pool = memory_pool().await;
        seed_rate(&pool, "r", now - 10.0 * day, 10.0, 1, 20.0).await;
        seed_rate(&pool, "p", now - 45.0 * day, 10.0, 1, 10.0).await;
        assert_eq!(
            trend(overview_impl(&pool, now, "all").await.unwrap()),
            json!("improving")
        );

        // stable: recent equals prior, inside the band.
        let pool = memory_pool().await;
        seed_rate(&pool, "r", now - 10.0 * day, 10.0, 1, 10.0).await;
        seed_rate(&pool, "p", now - 45.0 * day, 10.0, 1, 10.0).await;
        assert_eq!(
            trend(overview_impl(&pool, now, "all").await.unwrap()),
            json!("stable")
        );

        // zero recent rate: the positivity guard short-circuits to stable
        // (a mutated guard would fall through into the banding and declare a
        // direction).
        let pool = memory_pool().await;
        seed_rate(&pool, "p", now - 45.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&pool, now, "all").await.unwrap()),
            json!("stable")
        );

        // zero prior rate: the other half of the guard.
        let pool = memory_pool().await;
        seed_rate(&pool, "r", now - 10.0 * day, 10.0, 1, 20.0).await;
        assert_eq!(
            trend(overview_impl(&pool, now, "all").await.unwrap()),
            json!("stable")
        );
    }

    /// Dominance needs the top group at or above 60% of known kills, and the
    /// species/maturity presence decides mob vs tag.
    #[tokio::test]
    async fn activity_dominance_threshold_and_tag_split() {
        // Non-dominant: three distinct mobs, one kill each (33% each, below
        // the 0.6 floor) -> no dominant element, no comparison rows.
        let pool = memory_pool().await;
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
             VALUES('nd',1000.0,4600.0,5.0,0,0)",
        )
        .execute(&pool).await.expect("seed");
        for (i, mob) in ["Alpha", "Bravo", "Charlie"].iter().enumerate() {
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,'nd',?,'Spec','Young',?,0,1.0)",
            )
            .bind(format!("nd-{i}")).bind(*mob).bind(1000.0 + i as f64)
            .execute(&pool).await.expect("seed");
        }
        let v = activity_impl(&pool).await.unwrap();
        assert_eq!(v["mobComparisons"].as_array().unwrap().len(), 0);
        assert_eq!(v["tagComparisons"].as_array().unwrap().len(), 0);

        // Asymmetric: species present, maturity empty -> still a mob (the
        // presence test is OR, not AND), so it lands in mobComparisons.
        let pool = memory_pool().await;
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,armour_cost,heal_cost,dangling_cost) \
             VALUES('as',1000.0,4600.0,5.0,0,0)",
        )
        .execute(&pool).await.expect("seed");
        for i in 0..2 {
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
                 VALUES(?,'as','Foo','Bar','',?,0,1.0)",
            )
            .bind(format!("as-{i}")).bind(1000.0 + i as f64)
            .execute(&pool).await.expect("seed");
        }
        let v = activity_impl(&pool).await.unwrap();
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["mobName"], json!("Foo"));
        assert_eq!(v["tagComparisons"].as_array().unwrap().len(), 0);
    }

    /// A kill carrying a NULL session_id (forbidden by the production FK but
    /// representable here) is skipped, not decoded into a panic, matching the
    /// reference's `sessions.get(None)` miss.
    #[tokio::test]
    async fn activity_tolerates_a_null_session_id_row() {
        let pool = memory_pool().await;
        // A valid completed session with one dominant-mob kill.
        seed_filter_session(&pool, "ok", "Real", 1000.0, 1000.0 + 3600.0, 5.0, 2).await;
        // An orphan kill with no session_id (and no matching session row).
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,enhancer_cost,loot_total_ped) \
             VALUES('orphan',NULL,'Ghost','Spec','Young',1.0,0,9.0)",
        )
        .execute(&pool).await.expect("seed");
        // Must not panic; only the real session's mob is compared.
        let v = activity_impl(&pool).await.unwrap();
        let mobs = v["mobComparisons"].as_array().unwrap();
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0]["mobName"], json!("Real"));
    }

    #[test]
    fn period_epoch_maps_named_windows_only() {
        let now = 1_000_000.0;
        assert_eq!(period_epoch("all", now), None);
        assert_eq!(period_epoch("bogus", now), None);
        assert_eq!(period_epoch("30d", now), Some(now - 30.0 * 86400.0));
        assert_eq!(period_epoch("90d", now), Some(now - 90.0 * 86400.0));
        assert_eq!(period_epoch("1y", now), Some(now - 365.0 * 86400.0));
    }

    #[test]
    fn float_field_coerces_integers_only() {
        assert_eq!(float_field(json!(0)), json!(0.0));
        assert_eq!(float_field(json!(3)), json!(3.0));
        assert_eq!(float_field(json!(1.5)), json!(1.5));
    }

    #[test]
    fn rounded_preserves_integers_and_banker_rounds_floats() {
        assert_eq!(rounded(&json!(0), 2), json!(0)); // int stays int
        assert_eq!(rounded(&json!(1.005), 2), json!(1.0)); // half-even
        assert_eq!(rounded(&json!(2.675), 2), json!(2.67));
    }

    #[test]
    fn number_sum_is_integral_only_when_both_are() {
        assert_eq!(number_sum(&json!(2), &json!(3)), json!(5));
        assert_eq!(number_sum(&json!(2), &json!(0.5)), json!(2.5));
    }

    // ── Hermetic write-handler tests (the mutation campaign's kills) ──

    /// Create then list round-trips for the ledger: the create echoes the
    /// input plus a generated id, and the list reads it back.
    #[tokio::test]
    async fn ledger_create_and_list_round_trip() {
        let state = write_state().await;
        let (status, body) = body_of(
            state
                .create_ledger_entry("2026-05-01", "expense", "Ammo", 12.5, "ammo")
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["date"], json!("2026-05-01"));
        assert_eq!(body["type"], json!("expense"));
        assert_eq!(body["amount"], json!(12.5));
        assert_eq!(body["tag"], json!("ammo"));
        assert!(body["id"].as_str().is_some(), "create generates an id");

        let (status, list) = body_of(state.list_ledger().await).await;
        assert_eq!(status, StatusCode::OK);
        let rows = list.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["description"], json!("Ammo"));
        assert_eq!(rows[0]["id"], body["id"]);
    }

    /// The preset type guard: only 'expense'/'markup' pass; anything else is
    /// a 400 with the reference's detail and writes nothing.
    #[tokio::test]
    async fn preset_create_validates_type() {
        let state = write_state().await;
        for kind in ["expense", "markup"] {
            let (status, _) =
                body_of(state.create_ledger_preset("P", kind, "d", 1.0, "t").await).await;
            assert_eq!(status, StatusCode::OK, "{kind} accepted");
        }
        let (status, body) = body_of(
            state
                .create_ledger_preset("Bad", "income", "d", 1.0, "t")
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["detail"], json!("type must be 'expense' or 'markup'"));
        // Only the two valid presets were written.
        let (_, list) = body_of(state.list_ledger_presets().await).await;
        assert_eq!(list.as_array().unwrap().len(), 2);
    }

    /// Create with the optional fields absent: notes is null and acquired_at
    /// defaults to the (frozen) clock's UTC date.
    #[tokio::test]
    async fn inventory_create_defaults_date_and_notes() {
        let state = write_state().await;
        let (status, body) = body_of(
            state
                .create_inventory_item("Imk2", 50.0, 5.0, None, None)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Response is camelCase even though the request body is snake_case.
        assert_eq!(body["ttValue"], json!(50.0));
        assert_eq!(body["markupPaid"], json!(5.0));
        assert_eq!(body["notes"], Value::Null);
        assert_eq!(body["acquiredAt"], json!("2026-06-01"));

        // An explicit acquired_at / notes are honoured.
        let (_, body) = body_of(
            state
                .create_inventory_item("X", 1.0, 0.0, Some("spare"), Some("2026-01-02"))
                .await,
        )
        .await;
        assert_eq!(body["notes"], json!("spare"));
        assert_eq!(body["acquiredAt"], json!("2026-01-02"));
    }

    /// PATCH field-selection: only PROVIDED (Some) fields update; a None
    /// field is left untouched, exactly as the reference's
    /// `if patch.x is not None`.
    #[tokio::test]
    async fn inventory_patch_updates_only_provided_fields() {
        let state = write_state().await;
        let (_, created) = body_of(
            state
                .create_inventory_item("Orig", 20.0, 3.0, Some("keep"), Some("2026-03-01"))
                .await,
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();

        // Provide name + tt_value only: markup_paid and notes stay.
        let (status, patched) = body_of(
            state
                .update_inventory_item(&id, Some("Renamed"), Some(25.0), None, None)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(patched["name"], json!("Renamed"));
        assert_eq!(patched["ttValue"], json!(25.0));
        assert_eq!(patched["markupPaid"], json!(3.0), "untouched");
        assert_eq!(patched["notes"], json!("keep"), "untouched");

        // An all-None patch re-reads and returns the row unchanged.
        let (status, same) = body_of(
            state
                .update_inventory_item(&id, None, None, None, None)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(same, patched);

        // Patch a missing id -> 404.
        let (status, body) = body_of(
            state
                .update_inventory_item("no-such", Some("Z"), None, None, None)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], json!("Inventory item not found"));
    }

    /// Sell a created item, asserting the delta/type/description-default
    /// branch for profit / loss / zero-delta and the atomic item removal.
    #[tokio::test]
    async fn sell_emits_the_right_delta_branch() {
        // PROFIT: sale 20 over cost 12 -> markup 8.0; default description.
        let state = write_state().await;
        let (_, item) = body_of(
            state
                .create_inventory_item("Sword", 10.0, 2.0, None, Some("2026-02-01"))
                .await,
        )
        .await;
        let id = item["id"].as_str().unwrap().to_string();
        let (status, body) = body_of(
            state
                .sell_inventory_item(&id, 20.0, None, Some("2026-05-10"))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entry = &body["ledgerEntry"];
        assert_eq!(entry["type"], json!("markup"));
        assert_eq!(entry["amount"], json!(8.0));
        assert_eq!(entry["tag"], json!("inventory_sale"));
        assert_eq!(entry["date"], json!("2026-05-10"));
        assert_eq!(
            entry["description"],
            json!("Inventory Sale: Sword"),
            "default description form"
        );
        assert_eq!(body["soldItem"]["name"], json!("Sword"));
        // Item removed; the emitted ledger row is the only one.
        let (_, inv) = body_of(state.list_inventory().await).await;
        assert_eq!(inv.as_array().unwrap().len(), 0);
        let (_, ledger) = body_of(state.list_ledger().await).await;
        assert_eq!(ledger.as_array().unwrap().len(), 1);

        // LOSS: sale 5 under cost 12 -> expense 7.0; explicit description.
        let state = write_state().await;
        let (_, item) = body_of(
            state
                .create_inventory_item("Shield", 10.0, 2.0, None, Some("2026-02-01"))
                .await,
        )
        .await;
        let id = item["id"].as_str().unwrap().to_string();
        let (_, body) = body_of(
            state
                .sell_inventory_item(&id, 5.0, Some("Dumped it"), None)
                .await,
        )
        .await;
        let entry = &body["ledgerEntry"];
        assert_eq!(entry["type"], json!("expense"));
        assert_eq!(entry["amount"], json!(7.0));
        assert_eq!(entry["description"], json!("Dumped it"));
        // Default sold_at is the frozen clock date.
        assert_eq!(entry["date"], json!("2026-06-01"));

        // ZERO-DELTA: sale == cost -> no ledger entry, item still removed.
        let state = write_state().await;
        let (_, item) = body_of(
            state
                .create_inventory_item("Even", 8.0, 2.0, None, Some("2026-02-01"))
                .await,
        )
        .await;
        let id = item["id"].as_str().unwrap().to_string();
        let (status, body) = body_of(state.sell_inventory_item(&id, 10.0, None, None).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ledgerEntry"], Value::Null);
        assert_eq!(body["soldItem"]["name"], json!("Even"));
        let (_, ledger) = body_of(state.list_ledger().await).await;
        assert_eq!(ledger.as_array().unwrap().len(), 0, "no noise row");
        let (_, inv) = body_of(state.list_inventory().await).await;
        assert_eq!(inv.as_array().unwrap().len(), 0, "item removed");

        // Sell a missing id -> 404.
        let state = write_state().await;
        let (status, body) =
            body_of(state.sell_inventory_item("no-such", 1.0, None, None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], json!("Inventory item not found"));
    }

    #[tokio::test]
    async fn ledger_delete_removes_then_reports_missing() {
        let state = write_state().await;
        let (_, created) = body_of(
            state
                .create_ledger_entry("2026-05-01", "expense", "Ammo", 12.5, "ammo")
                .await,
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        // A successful delete reports "deleted" (the rows_affected == 0 guard
        // is false for an existing row); a second delete hits the 404.
        let (status, body) = body_of(state.delete_ledger_entry(&id).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], json!("deleted"));
        let (status, body) = body_of(state.delete_ledger_entry(&id).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], json!("Entry not found"));
    }

    #[tokio::test]
    async fn preset_list_shapes_rows_then_delete_removes() {
        let state = write_state().await;
        let (_, created) = body_of(
            state
                .create_ledger_preset("Decay", "expense", "d", 0.5, "decay")
                .await,
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        // The list shapes the row via preset_item (not an empty default).
        let (status, list) = body_of(state.list_ledger_presets().await).await;
        assert_eq!(status, StatusCode::OK);
        let rows = list.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], json!("Decay"));
        assert_eq!(rows[0]["amount"], json!(0.5));
        assert_eq!(rows[0]["tag"], json!("decay"));
        let (status, body) = body_of(state.delete_ledger_preset(&id).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], json!("deleted"));
        let (status, _) = body_of(state.delete_ledger_preset(&id).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn inventory_delete_removes_then_reports_missing() {
        let state = write_state().await;
        let (_, created) = body_of(
            state
                .create_inventory_item("Sword", 10.0, 2.0, None, None)
                .await,
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        let (status, body) = body_of(state.delete_inventory_item(&id).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], json!("deleted"));
        let (status, _) = body_of(state.delete_inventory_item(&id).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
