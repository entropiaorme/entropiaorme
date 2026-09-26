//! Review reads: a session's stored shots of one group, the weapons an
//! unpriced shot could be assigned to, and the session-detail summary with
//! the damage-over-time effects that ticked in the session.

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{
    CorrectionWeapon, EffectCandidate, ReviewShot, ReviewShotPage, ShotCandidate, ShotGroup,
    WeaponCorrectionKind, WeaponReviewError,
};
use crate::attack_rate::WeaponPricing;
use crate::cost_engine::cost_per_shot_from_props;
use crate::db::DbError;

/// How a weapon's stored props are prepared for pricing, when configured.
pub(super) type Pricing<'a> = Option<&'a WeaponPricing>;

/// A weapon's name and its per-shot cost in PED as it is configured now,
/// its props prepared by `pricing` under the reload speed the shot was
/// charged at (`reload_speed_percent`; a shot stored before that was kept
/// takes the reload speed in effect now), or None when the item is gone or
/// is not a weapon.
pub(super) fn weapon_price(
    conn: &rusqlite::Connection,
    equipment_id: i64,
    pricing: Pricing<'_>,
    reload_speed_percent: Option<f64>,
) -> Result<Option<(String, f64)>, WeaponReviewError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT name, properties_json FROM equipment_library \
             WHERE id = ?1 AND item_type = 'weapon'",
            [equipment_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((name, properties)) = row else {
        return Ok(None);
    };
    let props: Value = serde_json::from_str(&properties).map_err(|source| DbError::Decode {
        context: "weapon properties parse",
        source,
    })?;
    let props = match (pricing, reload_speed_percent) {
        (Some(pricing), Some(speed)) => pricing.prepare_at(&props, speed),
        (Some(pricing), None) => pricing.prepare(&props),
        (None, _) => props,
    };
    let cost = cost_per_shot_from_props(&props, None)
        .get("totalCostPerUse")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        / 100.0;
    Ok(Some((name, cost)))
}

pub(super) fn parse_candidates(raw: &str) -> Result<Vec<ShotCandidate>, WeaponReviewError> {
    serde_json::from_str(raw).map_err(|_| WeaponReviewError::Stored("unreadable shot candidates"))
}

/// Whether an effect window still stands: it exists and no decision took
/// it back.
pub(super) fn window_stands(
    conn: &rusqlite::Connection,
    window_id: &str,
) -> Result<bool, WeaponReviewError> {
    Ok(conn
        .query_row(
            "SELECT withdrawn_at IS NULL FROM weapon_effect_windows WHERE id = ?1",
            [window_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub(super) fn parse_effect_candidates(
    raw: &str,
) -> Result<Vec<EffectCandidate>, WeaponReviewError> {
    serde_json::from_str(raw).map_err(|_| WeaponReviewError::Stored("unreadable effect candidates"))
}

pub(super) fn session_shots(
    conn: &rusqlite::Connection,
    session_id: &str,
    group: ShotGroup,
    offset: i64,
    limit: i64,
) -> Result<ReviewShotPage, WeaponReviewError> {
    let ended: bool = conn
        .query_row(
            "SELECT is_active = 0 FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ?1 AND attribution = ?2",
        rusqlite::params![session_id, group.attribution()],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT e.id, e.observed_at, e.amount, e.critical, e.reason, e.hotbar_tool, \
                e.tool_name, e.cost_per_shot, e.candidates_json, c.id, r.decision, \
                e.effect_candidates_json, e.effect_window_id, c.kind, c.effect_window_id \
         FROM weapon_shot_evidence e \
         LEFT JOIN weapon_attribution_corrections c \
                ON c.id = e.correction_id AND c.undone_at IS NULL \
         LEFT JOIN weapon_attribution_reviews r ON r.id = e.review_id \
         WHERE e.session_id = ?1 AND e.attribution = ?2 \
         ORDER BY e.observed_at, e.id \
         LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![session_id, group.attribution(), limit, offset],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
            ))
        },
    )?;
    let mut shots = Vec::new();
    for row in rows {
        let (
            id,
            observed_at,
            amount,
            critical,
            reason,
            hotbar_tool,
            tool_name,
            cost_per_shot,
            candidates,
            correction_id,
            review_decision,
            effect_candidates,
            effect_window_id,
            correction_kind,
            correction_window_id,
        ) = row?;
        let correction_kind = match correction_kind.as_deref() {
            None => None,
            Some(kind) => Some(
                WeaponCorrectionKind::parse(kind)
                    .ok_or(WeaponReviewError::Stored("unknown correction kind"))?,
            ),
        };
        let mut effect_candidates = parse_effect_candidates(&effect_candidates)?;
        for candidate in &mut effect_candidates {
            candidate.standing = window_stands(conn, &candidate.window_id)?;
        }
        shots.push(ReviewShot {
            correctable: ended
                && group != ShotGroup::Evidence
                && tool_name.is_none()
                && correction_id.is_none(),
            group,
            effect_candidates,
            effect_window_id,
            correction_kind,
            correction_window_id,
            id,
            observed_at,
            amount,
            critical,
            reason,
            hotbar_tool,
            tool_name,
            cost_per_shot,
            candidates: parse_candidates(&candidates)?,
            correction_id,
            review_decision,
        });
    }
    Ok(ReviewShotPage { shots, total })
}

pub(super) fn correction_weapons(
    conn: &rusqlite::Connection,
    evidence_id: &str,
    pricing: Pricing<'_>,
) -> Result<Vec<CorrectionWeapon>, WeaponReviewError> {
    let stored: Option<(String, Option<f64>)> = conn
        .query_row(
            "SELECT candidates_json, reload_speed_percent FROM weapon_shot_evidence WHERE id = ?1",
            [evidence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((candidates, reload_speed_percent)) = stored else {
        return Err(WeaponReviewError::NotFound("Shot not found"));
    };
    let mut weapons = Vec::new();
    for candidate in parse_candidates(&candidates)? {
        if let Some((name, cost)) =
            weapon_price(conn, candidate.equipment_id, pricing, reload_speed_percent)?
        {
            weapons.push(CorrectionWeapon {
                equipment_id: candidate.equipment_id,
                name,
                cost_per_shot_ped: cost,
                fits: candidate.fits,
            });
        }
    }
    // Stable: fitting weapons first, then the carried order.
    weapons.sort_by_key(|weapon| !weapon.fits);
    Ok(weapons)
}

/// The subset of `session_ids` holding at least one unpriced shot.
pub(super) fn sessions_with_unpriced_shots(
    conn: &rusqlite::Connection,
    session_ids: &[String],
) -> Result<std::collections::BTreeSet<String>, DbError> {
    let mut unpriced = std::collections::BTreeSet::new();
    // Bounded batches keep the statement under SQLite's parameter limit
    // whatever page size a caller asks about.
    for chunk in session_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT session_id FROM weapon_shot_evidence \
             WHERE attribution = 'unresolved' AND tool_name IS NULL \
               AND correction_id IS NULL AND session_id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            unpriced.insert(row?);
        }
    }
    Ok(unpriced)
}

/// The session detail's weapon attribution block: the tallies the session
/// recorded at its stop (null for a session recorded before they were kept),
/// its stored shots by state, the decisions made on a mismatch while it
/// ran, and the damage-over-time effects that ticked in it. A session with
/// nothing to tell reads as all zeros, no decisions, and no effects.
pub fn session_detail_block(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Value, DbError> {
    let (agreed, evidenced, correctable): (Option<i64>, Option<i64>, bool) = conn
        .query_row(
            "SELECT weapon_shots_agreed, weapon_shots_evidenced, COALESCE(is_active, 0) = 0 \
             FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .unwrap_or((None, None, false));
    // A shot's standing: the live correction (if any) decides what it is
    // now, while `attribution` keeps how it was classified as it landed.
    let counts: [i64; 7] = conn.query_row(
        "SELECT COALESCE(SUM(e.attribution = 'evidence'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND e.tool_name IS NULL \
                             AND e.correction_id IS NULL), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND c.kind = 'priced'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND c.kind = 'effect_tick'), 0), \
                COALESCE(SUM(e.attribution = 'effect_tick'), 0), \
                COALESCE(SUM(e.attribution = 'effect_tick' AND c.kind = 'priced'), 0) \
         FROM weapon_shot_evidence e \
         LEFT JOIN weapon_attribution_corrections c \
                ON c.id = e.correction_id AND c.undone_at IS NULL \
         WHERE e.session_id = ?1",
        [session_id],
        |row| {
            Ok([
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ])
        },
    )?;
    let [evidence, unresolved, unpriced, assigned, marked_ticks, effect_ticks, priced_ticks] =
        counts;
    let reviews: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT id, decision, hotbar_tool, evidence_tool, mismatch_since, decided_at, \
                    repriced_shots, cost_delta_ped \
             FROM weapon_attribution_reviews WHERE session_id = ?1 \
             ORDER BY decided_at, id",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "decision": row.get::<_, String>(1)?,
                "hotbarTool": row.get::<_, String>(2)?,
                "evidenceTool": row.get::<_, String>(3)?,
                "since": row.get::<_, f64>(4)?,
                "decidedAt": row.get::<_, f64>(5)?,
                "repricedShots": row.get::<_, i64>(6)?,
                "costDelta": row.get::<_, f64>(7)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(json!({
        "correctable": correctable,
        "agreed": agreed,
        "evidenced": evidenced,
        "evidenceShots": evidence,
        "unresolved": unresolved,
        "unpriced": unpriced,
        "assigned": assigned,
        "markedTicks": marked_ticks,
        "effectTicks": effect_ticks,
        "pricedTicks": priced_ticks,
        "unclaimedTicks": unclaimed_ticks(conn, session_id)?,
        "effects": session_effects(conn, session_id)?,
        "reviews": reviews,
    }))
}

/// Ticks standing as ticks: effect-tick rows no correction priced, plus
/// unresolved hits a live correction marked as a tick.
/// A tick naming a window that no longer exists (its paying session was
/// deleted while a later one still ran) reads as unclaimed.
const STANDING_TICKS: &str = "\
    SELECT e.amount, \
           (SELECT w.id FROM weapon_effect_windows w \
            WHERE w.id = COALESCE(c.effect_window_id, e.effect_window_id)) AS window_id \
    FROM weapon_shot_evidence e \
    LEFT JOIN weapon_attribution_corrections c \
           ON c.id = e.correction_id AND c.undone_at IS NULL \
    WHERE e.session_id = ?1 \
      AND ((e.attribution = 'effect_tick' AND c.id IS NULL) \
        OR (e.attribution = 'unresolved' AND c.kind = 'effect_tick'))";

/// Standing ticks no one effect claims: several overlapping effects
/// explained them, or the session that paid for their effect was deleted.
fn unclaimed_ticks(conn: &rusqlite::Connection, session_id: &str) -> Result<i64, DbError> {
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM ({STANDING_TICKS}) WHERE window_id IS NULL"),
        [session_id],
        |row| row.get(0),
    )?)
}

/// Every effect the session paid for or saw tick, oldest first: who opened
/// it and when, what the opening hit cost, whether a decision took it back,
/// and the ticks it claims in this session. An effect paid for in an
/// earlier session is listed where its ticks landed, marked as paid
/// elsewhere, so its cost is never counted twice.
fn session_effects(conn: &rusqlite::Connection, session_id: &str) -> Result<Vec<Value>, DbError> {
    let sql = format!(
        "WITH ticks AS ({STANDING_TICKS}) \
         SELECT w.id, w.tool_name, w.started_at, w.expires_at, w.hit_amount, w.cost_per_shot, \
                w.session_id = ?1, w.withdrawn_at IS NOT NULL, \
                (SELECT COUNT(*) FROM ticks t WHERE t.window_id = w.id), \
                (SELECT COALESCE(SUM(t.amount), 0) FROM ticks t WHERE t.window_id = w.id) \
         FROM weapon_effect_windows w \
         WHERE w.session_id = ?1 OR w.id IN (SELECT window_id FROM ticks) \
         ORDER BY w.started_at, w.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "toolName": row.get::<_, String>(1)?,
            "activatedAt": row.get::<_, f64>(2)?,
            "expiresAt": row.get::<_, f64>(3)?,
            "hitAmount": row.get::<_, Option<f64>>(4)?,
            "costPerShot": row.get::<_, f64>(5)?,
            "paidHere": row.get::<_, bool>(6)?,
            "withdrawn": row.get::<_, bool>(7)?,
            "ticks": row.get::<_, i64>(8)?,
            "tickDamage": row.get::<_, f64>(9)?,
        }))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
