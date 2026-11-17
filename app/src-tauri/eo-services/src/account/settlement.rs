//! What a repair settles that the readings already billed.
//!
//! A session the readings measure bills its unlimited armour at the TT it
//! lost, so a repair recorded afterwards restores, among other things,
//! decay already in that session's costs. Only the rest is the recording's
//! to spread. Every repair made while the account was read shows as an
//! unlimited piece's TT rising between two stored readings, and every fall
//! is in the readings too, so the decay still outstanding is exact.
//!
//! Walking the stored readings from the first measured session in
//! question: a fall inside a measured session's window adds to what is
//! outstanding, and a rise (a repair the readings saw) settles all of it.
//! When the last rise has no play after it, it is the repair being
//! recorded, seen as it happened: what it restored is what was outstanding
//! just before it. Otherwise it settled earlier decay, and only what fell
//! since is outstanding.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;

use super::amount::{Amount, Pec};
use super::costs::COSTS_VERSION;
use super::measure::MEASURE_VERSION;
use super::reading::Container;
use super::roles::{cost_category, CostCategory, KnownRoles, RoleBook, Use};
use super::store::{readings_from, ReadingId};
use super::AccountError;

/// Unlimited armour decay the readings billed that no repair has restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outstanding {
    pub decay: Amount,
    /// How many unlimited pieces that decay is read from: each value the
    /// site shows is truncated to the PEC, so the decay is known to within
    /// a PEC per piece.
    pub pieces: usize,
}

impl Outstanding {
    pub const NONE: Outstanding = Outstanding {
        decay: Amount::NONE,
        pieces: 0,
    };
}

/// The unlimited armour decay the measured sessions `session_ids` billed
/// that no repair since has restored, as the stored readings show it. A
/// session whose costs are not current, or not measured (whole, or across
/// a gap shown clean), bills none.
pub fn outstanding_unlimited_decay(
    conn: &rusqlite::Connection,
    roles: &KnownRoles,
    session_ids: &[String],
) -> Result<Outstanding, AccountError> {
    let mut windows: Vec<(i64, i64)> = Vec::new();
    for session_id in session_ids {
        let window: Option<(i64, i64)> = conn
            .query_row(
                "SELECT first_reading_id, last_reading_id FROM account_session_costs \
                 WHERE session_id = ?1 AND costs_version = ?2 AND measure_version = ?3 \
                   AND status IN ('measured', 'acrossGap') AND first_reading_id IS NOT NULL",
                rusqlite::params![session_id, COSTS_VERSION, MEASURE_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        windows.extend(window);
    }
    let Some(start) = windows.iter().map(|(first, _)| *first).min() else {
        return Ok(Outstanding::NONE);
    };
    let stored = readings_from(conn, ReadingId(start))?;
    let readings: Vec<_> = stored.iter().map(|stored| stored.reading.clone()).collect();
    let book = RoleBook::resolve(roles, &readings);
    let unlimited = |key: &super::reading::HoldingKey| {
        let parent_role = match &key.container {
            Container::Attached { parent } => Some(book.role(parent)),
            _ => None,
        };
        matches!(
            cost_category(roles, key, book.role(&key.name), Use::Wear, parent_role),
            CostCategory::UnlimitedArmour | CostCategory::UnlimitedPlate
        )
    };

    let mut current: i64 = 0;
    let mut current_pieces: BTreeSet<String> = BTreeSet::new();
    let mut before_last_rise: Option<(i64, BTreeSet<String>, f64)> = None;
    for pair in stored.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        let id = after.header.id.0;
        let in_window = windows
            .iter()
            .any(|(first, last)| id > *first && id <= *last);
        let mut rose = false;
        for (key, was) in before.reading.holdings() {
            let Some(now) = after.reading.holding(key) else {
                continue;
            };
            if now.quantity() != was.quantity() || !unlimited(key) {
                continue;
            }
            let (was, now) = (was.value().0, now.value().0);
            if now > was {
                rose = true;
            } else if now < was && in_window {
                current = current
                    .checked_add(was - now)
                    .ok_or(AccountError::Overflow)?;
                current_pieces.insert(key.name.clone());
            }
        }
        if rose {
            // A repair seen as it happened settles everything up to it.
            before_last_rise = Some((
                current,
                std::mem::take(&mut current_pieces),
                after.reading.taken_at(),
            ));
            current = 0;
        }
    }
    let (decay, pieces) = match before_last_rise {
        Some((settled, pieces, rose_at)) if !played_after(conn, rose_at)? => (
            settled.checked_add(current).ok_or(AccountError::Overflow)?,
            pieces.union(&current_pieces).count(),
        ),
        _ => (current, current_pieces.len()),
    };
    Ok(Outstanding {
        decay: Amount::whole_pec(Pec(decay))?,
        pieces,
    })
}

/// Whether any tracked session was still being played after `at`.
fn played_after(conn: &rusqlite::Connection, at: f64) -> Result<bool, AccountError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE ended_at IS NULL OR ended_at > ?1)",
        [at],
        |row| row.get(0),
    )?)
}
