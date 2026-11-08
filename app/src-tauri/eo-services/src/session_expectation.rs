//! What a recorded session's Cycled was expected to return, read back from
//! the tracking tables.
//!
//! The live readout works this out from the running session's memory; once
//! a session has ended, the same streams are rebuilt from what it recorded:
//! the weapon evidence each fired phase carried, the healing activations
//! that still stand, the armour recorded against it, and its harvesting
//! decay. Being a pure function of those tables, it stays correct through
//! everything that moves them after the session ends, such as a repair
//! recorded later.
//!
//! A session with no weapon evidence has no expectation. That is every
//! session from before the evidence existed, and the session is left out of
//! a family's figure rather than counted as expecting nothing.

use crate::db::DbError;
use crate::expected_hunting::{LooterSource, OffensiveLoadoutEvidence};
use crate::healing_profile::HealingProfile;
use crate::loaded_expectation::{
    loaded_expectation, CycledStreams, FlattenedPhases, LoadedExpectation,
};

/// Expected loot TT at one looter basis, with the ends of its span: three
/// figures that sum across sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ExpectedSpan {
    pub expected: f64,
    pub low: f64,
    pub high: f64,
}

impl From<&LoadedExpectation> for ExpectedSpan {
    fn from(at: &LoadedExpectation) -> Self {
        Self {
            expected: at.expected_loot_tt,
            low: at.expected_loot_tt_low,
            high: at.expected_loot_tt_high,
        }
    }
}

impl ExpectedSpan {
    pub fn add(&mut self, other: Self) {
        self.expected += other.expected;
        self.low += other.low;
        self.high += other.high;
    }
}

/// A recorded session's expected loot at every looter basis, each read at
/// the looter levels the session started with.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionExpectation {
    pub three_looter_mean: ExpectedSpan,
    pub animal: ExpectedSpan,
    pub mutant: ExpectedSpan,
    pub robot: ExpectedSpan,
}

/// The session's expectation over `cycled`, its whole Cycled; `None` when it
/// recorded no weapon evidence.
pub fn compute(
    conn: &rusqlite::Connection,
    session_id: &str,
    cycled: f64,
) -> Result<Option<SessionExpectation>, DbError> {
    let mut phases = FlattenedPhases::default();
    let mut looters = None;
    {
        let mut stmt = conn.prepare(
            "SELECT ts.expected_economics_json, ts.tool_name, COALESCE(ts.cost_per_shot, 0), \
                    COALESCE(SUM(ts.shots_fired), 0) \
             FROM kills k CROSS JOIN kill_tool_stats ts \
             WHERE k.session_id = ?1 AND ts.kill_id = k.id \
             GROUP BY ts.expected_economics_json, ts.tool_name, ts.cost_per_shot",
        )?;
        let mut rows = stmt.query([session_id])?;
        while let Some(row) = rows.next()? {
            // This runs inside the transaction that ends a session, so
            // evidence it cannot read costs that phase its Efficiency and
            // never the session's own end.
            let encoded: Option<String> = row.get(0)?;
            let evidence = encoded.and_then(|encoded| {
                serde_json::from_str::<OffensiveLoadoutEvidence>(&encoded).ok()
            });
            if let Some(evidence) = &evidence {
                looters.get_or_insert(evidence.looters);
            }
            let tool_name: String = row.get(1)?;
            phases.add(evidence.as_ref(), &tool_name, row.get(2)?, row.get(3)?);
        }
    }
    let Some(looters) = looters else {
        return Ok(None);
    };

    let streams = CycledStreams {
        unknown_efficiency_tt: phases.transferred_tt
            + healing_tt(conn, session_id)?
            + crate::tracker::read_session_armour(conn, session_id)?
                .tt
                .value()
            + harvesting_tt(conn, session_id)?,
        offensive: OffensiveLoadoutEvidence {
            components: phases.components,
            looters,
            looter_source: LooterSource::ThreeLooterMean,
            transferred_tt_per_use: 0.0,
        },
        cycled,
    };
    let at = |source| -> Option<ExpectedSpan> {
        loaded_expectation(&streams, source)
            .ok()
            .flatten()
            .as_ref()
            .map(ExpectedSpan::from)
    };
    let (Some(three_looter_mean), Some(animal), Some(mutant), Some(robot)) = (
        at(LooterSource::ThreeLooterMean),
        at(LooterSource::Animal),
        at(LooterSource::Mutant),
        at(LooterSource::Robot),
    ) else {
        return Ok(None);
    };
    Ok(Some(SessionExpectation {
        three_looter_mean,
        animal,
        mutant,
        robot,
    }))
}

/// The TT the session's standing healing activations consumed.
fn healing_tt(conn: &rusqlite::Connection, session_id: &str) -> Result<f64, DbError> {
    let mut stmt = conn.prepare(
        "SELECT cost_ped, profile_json FROM healing_activations \
         WHERE session_id = ?1 AND superseded_at IS NULL",
    )?;
    let mut rows = stmt.query([session_id])?;
    let mut total = 0.0;
    while let Some(row) = rows.next()? {
        let cost: f64 = row.get(0)?;
        let profile: String = row.get(1)?;
        total += serde_json::from_str::<HealingProfile>(&profile)
            .map_or(cost, |profile| profile.tt_of_use(cost));
    }
    Ok(total)
}

fn harvesting_tt(conn: &rusqlite::Connection, session_id: &str) -> Result<f64, DbError> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(cost_ped), 0) FROM harvest_events WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?)
}
