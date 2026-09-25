//! Review reads: a session's stored shots of one group, the weapons an
//! unpriced shot could be assigned to, and the session-detail summary.

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{
    CorrectionWeapon, ReviewShot, ReviewShotPage, ShotCandidate, ShotGroup, WeaponReviewError,
};
use crate::cost_engine::cost_per_shot_from_props;
use crate::db::DbError;

/// A weapon's name and its per-shot cost in PED as it is configured now, or
/// None when the item is gone or is not a weapon.
pub(super) fn weapon_price(
    conn: &rusqlite::Connection,
    equipment_id: i64,
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
                e.tool_name, e.cost_per_shot, e.candidates_json, c.id, r.decision \
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
        ) = row?;
        shots.push(ReviewShot {
            correctable: ended && group == ShotGroup::Unresolved && tool_name.is_none(),
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
) -> Result<Vec<CorrectionWeapon>, WeaponReviewError> {
    let candidates: Option<String> = conn
        .query_row(
            "SELECT candidates_json FROM weapon_shot_evidence WHERE id = ?1",
            [evidence_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(candidates) = candidates else {
        return Err(WeaponReviewError::NotFound("Shot not found"));
    };
    let mut weapons = Vec::new();
    for candidate in parse_candidates(&candidates)? {
        if let Some((name, cost)) = weapon_price(conn, candidate.equipment_id)? {
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
               AND session_id IN ({placeholders})"
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
/// its stored shots by state, and the decisions made on a mismatch while it
/// ran. A session with nothing to tell reads as all zeros and no decisions.
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
    let (evidence, unresolved, unpriced, assigned, effect_ticks): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(attribution = 'evidence'), 0), \
                    COALESCE(SUM(attribution = 'unresolved'), 0), \
                    COALESCE(SUM(attribution = 'unresolved' AND tool_name IS NULL), 0), \
                    COALESCE(SUM(attribution = 'unresolved' AND correction_id IS NOT NULL), 0), \
                    COALESCE(SUM(attribution = 'effect_tick'), 0) \
             FROM weapon_shot_evidence WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
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
        "effectTicks": effect_ticks,
        "reviews": reviews,
    }))
}
