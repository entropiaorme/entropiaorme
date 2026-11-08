//! The live session's Cycled, split by what it was spent on.
//!
//! The tracker accrues weapon, healing, consumable, and harvesting spend in
//! memory as the events arrive. Armour and plate cost is different: it is
//! recorded when the player repairs or reads a Trade Terminal, by the
//! protection service, which spreads it over the sessions they tick and
//! writes each session's share to its row. When a recording reaches the
//! running session the tracker re-reads that share here, so the live
//! readout carries it from the moment it is recorded rather than only once
//! the session is read back from the database.
//!
//! Every line built here sums, with the others, to the session's Cycled:
//! the breakdown and the total come from the same in-memory figures.

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;

use crate::db::DbError;
use crate::ped::Ped;
use crate::tracking_models::{CycledBucket, CycledLine};

use super::session::ActiveSession;

/// What one healing tool has cost the session: the paid total, and the
/// part of it that was TT the game consumed (the rest is the markup paid
/// over TT for a limited tool).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct HealSpend {
    pub(super) cost: Ped,
    pub(super) tt: Ped,
}

/// The armour and plate cost recorded against the running session.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionArmour {
    /// The session row's armour cost: the figure every recorded read uses.
    pub(crate) total: Ped,
    /// The part of `total` that was TT the protection consumed. A limited
    /// set's recording books its TT loss at the set's markup, so the
    /// difference is consumed acquisition premium; a repair is TT.
    pub(crate) tt: Ped,
    /// One line per stream: each limited set by name, unlimited repairs
    /// pooled.
    pub(crate) lines: Vec<(String, Ped)>,
}

/// The label unlimited repairs are pooled under: repair cost is one stream
/// at raw TT, not a set of its own.
pub(super) const UNLIMITED_ARMOUR_LABEL: &str = "Armour repairs";
/// The label for armour cost on the session row that no live recording
/// explains (a recording booked before the per-session spread existed).
pub(super) const OTHER_ARMOUR_LABEL: &str = "Other armour";

/// Read the armour cost recorded against `session_id`, with its stream
/// split and the TT it represents.
///
/// The session row's `armour_cost` is the authority for the total, since it
/// is what the session summary and every recorded read report. The lines
/// come from the standing (not undone) recordings' allocations; any gap
/// between the two becomes one residual line, so the lines always sum to
/// the total.
pub(crate) fn read_session_armour(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<SessionArmour, DbError> {
    let total: f64 = conn
        .query_row(
            "SELECT COALESCE(armour_cost, 0) FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0.0);
    let mut stmt = conn.prepare(
        "SELECT w.kind, s.name, w.consumed_tt_ped, w.cost_ped, a.cost_ped \
         FROM protection_cost_allocations a \
         JOIN protection_cost_windows w ON w.id = a.window_id \
         LEFT JOIN protection_sets s ON s.id = w.set_id \
         WHERE a.session_id = ?1 AND w.superseded_at IS NULL \
         ORDER BY w.created_at, w.id",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Streams keep the order they were first recorded in.
    let mut order: Vec<String> = Vec::new();
    let mut by_stream: BTreeMap<String, f64> = BTreeMap::new();
    let mut tt = 0.0;
    let mut explained = 0.0;
    for (kind, set_name, consumed_tt, window_cost, allocated) in rows {
        let limited = kind == "limited_decay";
        let label = match (limited, set_name) {
            (true, Some(name)) => name,
            _ => UNLIMITED_ARMOUR_LABEL.to_string(),
        };
        // A limited recording's cost is its consumed TT at the set's
        // markup, so each session's share carries the same TT fraction.
        let tt_fraction = match (limited, consumed_tt) {
            (true, Some(consumed)) if window_cost > 0.0 => (consumed / window_cost).clamp(0.0, 1.0),
            _ => 1.0,
        };
        tt += allocated * tt_fraction;
        explained += allocated;
        if !by_stream.contains_key(&label) {
            order.push(label.clone());
        }
        *by_stream.entry(label).or_default() += allocated;
    }
    let mut lines: Vec<(String, Ped)> = order
        .into_iter()
        .map(|label| {
            let cost = by_stream[&label];
            (label, Ped(cost))
        })
        .collect();
    let residual = total - explained;
    if residual.abs() >= 0.005 {
        lines.push((OTHER_ARMOUR_LABEL.to_string(), Ped(residual)));
        // Nothing records a residual's markup, so it counts as TT.
        tt += residual;
    }
    Ok(SessionArmour {
        total: Ped(total),
        tt: Ped(tt.clamp(0.0, total.max(0.0))),
        lines,
    })
}

/// Build the live Cycled breakdown from the session's in-memory figures.
///
/// `weapon_lines` is each fired tool's cost in first-fired order (the
/// caller already walked the kills for the weapon total); the other
/// buckets come from the session's own ledgers. Zero lines are dropped:
/// a tool that cost nothing has nothing to say in a cost breakdown.
pub(super) fn cycled_breakdown(
    active: &ActiveSession,
    weapon_lines: Vec<(String, Ped)>,
    enhancer_cost: Ped,
) -> Vec<CycledLine> {
    let mut lines = Vec::new();
    let mut push = |bucket: CycledBucket, label: String, cost: Ped| {
        let cost = cost.value();
        if cost.abs() >= 1e-9 {
            lines.push(CycledLine {
                bucket,
                label,
                cost_ped: cost,
            });
        }
    };
    for (label, cost) in weapon_lines {
        push(CycledBucket::Weapon, label, cost);
    }
    push(
        CycledBucket::Enhancer,
        "Enhancers".to_string(),
        enhancer_cost,
    );
    for (tool, spend) in &active.heal_spend {
        push(CycledBucket::Healing, tool.clone(), spend.cost);
    }
    for (label, cost) in &active.armour.lines {
        push(CycledBucket::Armour, label.clone(), *cost);
    }
    for (item, cost) in &active.consumable_spend {
        push(CycledBucket::Consumable, item.clone(), *cost);
    }
    let mut harvest_order: Vec<String> = Vec::new();
    let mut harvest: BTreeMap<String, Ped> = BTreeMap::new();
    for event in &active.session.harvests {
        let label = event
            .tool_name
            .clone()
            .unwrap_or_else(|| "Harvesting".to_string());
        if !harvest.contains_key(&label) {
            harvest_order.push(label.clone());
        }
        *harvest.entry(label).or_insert(Ped::ZERO) += event.cost_ped;
    }
    for label in harvest_order {
        let cost = harvest[&label];
        push(CycledBucket::Harvesting, label, cost);
    }
    lines
}

/// Each fired tool's cost this session, merged across its phases, in the
/// order the tools were first fired.
pub(super) fn weapon_lines(active: &ActiveSession) -> Vec<(String, Ped)> {
    let mut order: Vec<String> = Vec::new();
    let mut by_tool: BTreeMap<String, Ped> = BTreeMap::new();
    let phases = active
        .session
        .kills
        .iter()
        .flat_map(|kill| kill.tool_stats.iter().map(|(_, stats)| stats))
        .chain(active.accumulator.tool_stats.iter().map(|(_, stats)| stats));
    for stats in phases {
        let cost = stats.cost_per_shot * stats.shots_fired;
        if !by_tool.contains_key(&stats.tool_name) {
            order.push(stats.tool_name.clone());
        }
        *by_tool.entry(stats.tool_name.clone()).or_insert(Ped::ZERO) += cost;
    }
    order
        .into_iter()
        .map(|tool| {
            let cost = by_tool[&tool];
            (tool, cost)
        })
        .collect()
}

/// Expected loot over the session's whole Cycled, at every looter basis.
///
/// The offensive phases carry their own evidence; the unknown-efficiency TT
/// is what else the session cycled into the loot pool: the decay implants
/// and absorbers took over, the TT each healing tool consumed, the armour
/// and plate TT recorded against the session, and harvesting decay. The
/// rest of Cycled (limited markup over TT, doses, enhancer breakage) is lost.
pub(super) fn live_expectation(
    active: &ActiveSession,
    offensive: &crate::expected_hunting::OffensiveLoadoutEvidence,
    transferred_tt: f64,
    cost: Ped,
) -> Option<crate::tracking_models::LiveExpectation> {
    use crate::expected_hunting::LooterSource;
    use crate::loaded_expectation::{cycled_split, loaded_expectation, CycledStreams};
    let heal_tt: f64 = active
        .heal_spend
        .values()
        .map(|spend| spend.tt.value())
        .sum();
    let harvest_tt: f64 = active
        .session
        .harvests
        .iter()
        .map(|harvest| harvest.cost_ped.value())
        .sum();
    let streams = CycledStreams {
        offensive: offensive.clone(),
        unknown_efficiency_tt: transferred_tt + heal_tt + active.armour.tt.value() + harvest_tt,
        cycled: cost.value(),
    };
    let split = cycled_split(&streams).ok().flatten()?;
    let at = |source| loaded_expectation(&streams, source).ok().flatten();
    Some(crate::tracking_models::LiveExpectation {
        model_version: crate::expected_hunting::COMMUNITY_MODEL_V1,
        split,
        three_looter_mean: at(LooterSource::ThreeLooterMean)?,
        animal: at(LooterSource::Animal)?,
        mutant: at(LooterSource::Mutant)?,
        robot: at(LooterSource::Robot)?,
    })
}

impl super::actor::TrackerActor {
    /// A protection write committed (a recording, a reading, or an undo):
    /// it may have moved the running session's armour share.
    pub(super) async fn on_protection_updated(&mut self) {
        if let Err(error) = self.refresh_armour().await {
            tracing::warn!(target: "eo::tracker", %error, "live armour cost refresh failed");
        }
    }

    /// Re-read the running session's armour cost after a protection write
    /// or a direct armour edit, and push the change to the live readout at once:
    /// a recording made between kills would otherwise wait for the next
    /// tick that happens to change something else.
    pub(super) async fn refresh_armour(&mut self) -> Result<bool, DbError> {
        let Some(session_id) = self
            .session
            .active()
            .map(|active| active.session.id.clone())
        else {
            return Ok(false);
        };
        let read_id = session_id.clone();
        let armour = self
            .db
            .with_reader(move |conn| read_session_armour(conn, &read_id))
            .await?;
        let Some(active) = self.session.active_mut() else {
            return Ok(false);
        };
        // The session may have stopped and another started while the read
        // ran; the read belongs to the one it named.
        if active.session.id != session_id || active.armour == armour {
            return Ok(false);
        }
        active.armour = armour;
        let now = self.epoch_now();
        self.emit_session_event(
            eo_wire::domain_events::TrackingReason::Updated,
            eo_wire::domain_events::TrackingStatus::Active,
            now,
            Some(&session_id),
        );
        Ok(true)
    }
}
