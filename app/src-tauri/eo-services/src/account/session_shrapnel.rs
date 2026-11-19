//! How much Shrapnel a tracked session turned into Universal Ammo, read
//! once the session has closed.
//!
//! Inside a session Shrapnel is looted as it is converted and the ammo it
//! pays is fired as it arrives, so no step shows a conversion's size. The
//! session whole does: what the chat log looted, less the change in the
//! Shrapnel held from the session's first reading to its last, left as
//! ammo.
//!
//! That figure rests on the chat log, so it is recorded only when the
//! session's weapon cost bears it out. Had that much Shrapnel become ammo,
//! the readings show how much ammo was fired; with what the weapon's items
//! wore, that is a weapon cost, and it has to sit within
//! [`WITHIN_OF_TRACKED`] of the tracked one. Loot the chat log missed,
//! Shrapnel handed to someone, or ammo bought in the same session each
//! move the implied figure away from the tracked one, and then nothing is
//! recorded: the stock check's own default stands.
//!
//! Nothing is read for a session that is not measured whole, whose ammo
//! split is unsettled, that sold Shrapnel or left some unexplained, that
//! held Shrapnel on the auction, that shows no ammo arriving at all, or in
//! which a record the readings made of Shrapnel has been taken back.

use super::amount::{checked, Amount};
use super::costs::{self, Basis, CostBucket, Status};
use super::diff::exact_unit;
use super::follow::{history_ends_at, HAND_ENTRY_LOOK_BACK, STOCK_SETTLES};
use super::measure::{ammo_for_shrapnel, AmmoSplit, CoverageLevel, NotableEvent, SessionMeasure};
use super::reading::{Container, Reading};
use super::roles::{KnownRoles, SHRAPNEL, UNIVERSAL_AMMO};
use super::sessions::{measure_to_seal, tracked_session};
use super::store::{load_reading, ReadingId, StoredReading};
use super::AccountError;
use crate::analytics::AccountStep;
use crate::game_data_store::GameDataStore;

/// How far the weapon cost a conversion implies may sit from the tracked
/// one, as a share of the tracked one, for the conversion to be recorded.
pub const WITHIN_OF_TRACKED: f64 = 0.02;

/// A unit of Universal Ammo in micro-PED, when the rows do not give it.
const AMMO_UNIT_MICRO_PED: i64 = 100;

/// The Shrapnel a session converted that nothing has recorded yet, with
/// the step that spans the session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionShrapnel {
    pub step: AccountStep,
    pub units: i64,
}

fn held(reading: &Reading, name: &str) -> (i64, bool) {
    let mut units = 0i64;
    let mut on_auction = false;
    for (key, holding) in reading.holdings() {
        if key.name != name {
            continue;
        }
        if key.container == Container::Auction {
            on_auction |= holding.quantity() > 0;
        } else {
            units = units.saturating_add(holding.quantity());
        }
    }
    (units, on_auction)
}

/// The session's first reading and its last, when `closing` is the last
/// and was taken once the session had ended.
fn ends(
    conn: &rusqlite::Connection,
    closing: ReadingId,
) -> Result<Option<(String, StoredReading, StoredReading)>, AccountError> {
    let Some(last) = load_reading(conn, closing)? else {
        return Ok(None);
    };
    let Some(session_id) = last.header.session_id.clone() else {
        return Ok(None);
    };
    let Some(session) = tracked_session(conn, &session_id)? else {
        return Ok(None);
    };
    let ended = match session.window.ended_at {
        Some(ended) if !session.active => ended,
        _ => return Ok(None),
    };
    if last.header.taken_at < ended {
        return Ok(None);
    }
    let (first_id, last_id): (i64, i64) = conn.query_row(
        // id-order: insertion (a session's readings are stored as they are taken)
        "SELECT MIN(id), MAX(id) FROM account_readings WHERE session_id = ?1",
        [&session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if last_id != closing.0 || first_id == last_id || first_id <= history_ends_at(conn)? {
        return Ok(None);
    }
    let Some(first) = load_reading(conn, ReadingId(first_id))? else {
        return Ok(None);
    };
    Ok(Some((session_id, first, last)))
}

/// Units of Shrapnel the chat log looted from `from` to `to`, an enhancer's
/// return among them: all of it arrives in the account.
fn looted(conn: &rusqlite::Connection, from: f64, to: f64) -> Result<i64, AccountError> {
    Ok(conn.query_row(
        // every-kill: spend (any row's loot, a claimed reward's arrival
        // included: all of it arrived in the account, whichever session
        // logged it)
        "SELECT COALESCE((SELECT SUM(l.quantity) FROM kill_loot_items l \
                          JOIN kills k ON k.id = l.kill_id \
                          WHERE l.item_name = ?1 AND l.deactivated_at IS NULL \
                            AND k.timestamp >= ?2 AND k.timestamp <= ?3), 0) \
              + COALESCE((SELECT SUM(l.quantity) FROM harvest_loot_items l \
                          JOIN harvest_events e ON e.id = l.harvest_id \
                          WHERE l.item_name = ?1 AND l.deactivated_at IS NULL \
                            AND e.timestamp >= ?2 AND e.timestamp <= ?3), 0)",
        rusqlite::params![SHRAPNEL, from, to],
        |row| row.get::<_, i64>(0),
    )?)
}

/// Units of Shrapnel the records made from the readings inside the window
/// took out of stock; `None` when one of them has been taken back.
fn recorded(conn: &rusqlite::Connection, from: f64, to: f64) -> Result<Option<i64>, AccountError> {
    let taken_back: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM stock_records_from_account s \
                        WHERE s.item_name = ?1 AND s.seen_from >= ?2 AND s.seen_to <= ?3 \
                          AND NOT EXISTS (SELECT 1 FROM stock_movements m \
                                          WHERE m.ref_id = s.record_id))",
        rusqlite::params![SHRAPNEL, from, to],
        |row| row.get(0),
    )?;
    if taken_back {
        return Ok(None);
    }
    let units: f64 = conn.query_row(
        "SELECT COALESCE(SUM(-m.quantity), 0.0) FROM stock_movements m \
         JOIN stock_records_from_account s ON s.record_id = m.ref_id \
         WHERE m.item_name = ?1 AND m.quantity < 0 AND s.seen_from >= ?2 AND s.seen_to <= ?3",
        rusqlite::params![SHRAPNEL, from, to],
        |row| row.get(0),
    )?;
    Ok(Some(units.round() as i64))
}

/// Whether the measure shows what a conversion leaves: ammo arriving, with
/// nothing about the Shrapnel left for the player to settle.
fn readable(measure: &SessionMeasure) -> bool {
    let arrived = measure.events.iter().any(|event| {
        matches!(
            event,
            NotableEvent::ShrapnelPresumedConverted { .. }
                | NotableEvent::ShrapnelConverted { .. }
                | NotableEvent::AmmoToppedUp { .. }
        )
    });
    let sold = measure.events.iter().any(|event| {
        matches!(event, NotableEvent::Sale { items, .. }
            if items.iter().any(|item| item.key.name == SHRAPNEL))
    });
    let unexplained = measure
        .unexplained_departures
        .iter()
        .any(|item| item.key.name == SHRAPNEL);
    arrived && !sold && !unexplained
}

/// The weapon cost the readings imply had `converted` units of Shrapnel
/// become ammo over the session, in micro-PED: the ammo fired, with what
/// the weapon's items wore. `None` when the measure cannot give one.
fn implied_weapon(
    measure: &SessionMeasure,
    session_costs: &costs::SessionCosts,
    roles: &KnownRoles,
    first: &Reading,
    last: &Reading,
    converted: i64,
) -> Result<Option<i64>, AccountError> {
    let Some(ammo) = measure.ammo.iter().find(|use_| use_.name == UNIVERSAL_AMMO) else {
        return Ok(None);
    };
    let AmmoSplit::Settled {
        burned_units,
        strongbox_units,
        ..
    } = &ammo.split
    else {
        return Ok(None);
    };
    let measured = match session_costs.bucket(CostBucket::Weapon).map(|b| b.basis) {
        Some(Basis::Measured(amount) | Basis::Mixed(amount)) => amount,
        _ => return Ok(None),
    };
    // Everything fired: what was paid in, less the net change.
    let paid = checked(ammo_for_shrapnel(converted)?.checked_add(*strongbox_units))?;
    let fired = checked(paid.checked_sub(ammo.net_units))?;
    // Of it, what the measured figure does not hold already.
    let unmeasured = checked(fired.checked_sub(*burned_units))?;
    if unmeasured < 0 {
        return Ok(None);
    }
    let unit = exact_unit(
        first
            .holdings()
            .iter()
            .chain(last.holdings())
            .filter(|(key, _)| key.name == UNIVERSAL_AMMO)
            .flat_map(|(_, holding)| holding.rows()),
    )
    .unwrap_or(AMMO_UNIT_MICRO_PED);
    let withheld = costs::withheld_weapon_wear(measure, roles)?;
    let implied = measured
        .plus(Amount::exact_units(unmeasured, unit)?)?
        .plus(withheld)?;
    Ok(Some(implied.micro_ped()))
}

/// The Shrapnel the session closed by the reading `closing` converted that
/// nothing has recorded, when the session's weapon cost bears it out. A
/// read: run it on a reader.
pub fn at_close(
    conn: &rusqlite::Connection,
    closing: ReadingId,
    game_data: Option<&GameDataStore>,
    now: f64,
) -> Result<Option<SessionShrapnel>, AccountError> {
    let Some((session_id, first, last)) = ends(conn, closing)? else {
        return Ok(None);
    };
    let (from, to) = (first.header.taken_at, last.header.taken_at);
    let (held_first, listed_first) = held(&first.reading, SHRAPNEL);
    let (held_last, listed_last) = held(&last.reading, SHRAPNEL);
    if listed_first || listed_last {
        return Ok(None);
    }
    let converted = checked(
        looted(conn, from, to)?
            .checked_add(held_first)
            .and_then(|units| units.checked_sub(held_last)),
    )?;
    let Some(recorded) = recorded(conn, from, to)? else {
        return Ok(None);
    };
    let units = checked(converted.checked_sub(recorded))?;
    if units <= 0 {
        return Ok(None);
    }
    let Some(seal) = measure_to_seal(conn, &session_id, game_data, None, now)? else {
        return Ok(None);
    };
    let measure = &seal.computed.measure;
    if seal.costs.status != Status::Measured
        || measure.coverage.level != CoverageLevel::Complete
        || !readable(measure)
    {
        return Ok(None);
    }
    let roles = KnownRoles::read(conn, game_data)?;
    let Some(implied) = implied_weapon(
        measure,
        &seal.costs,
        &roles,
        &first.reading,
        &last.reading,
        converted,
    )?
    else {
        return Ok(None);
    };
    let tracked = crate::session_costs::tracked(conn, &session_id)?;
    let tracked = (tracked.weapon + tracked.dangling) * 1_000_000.0;
    if tracked <= 0.0 || (implied as f64 - tracked).abs() > WITHIN_OF_TRACKED * tracked {
        tracing::info!(target: "eo::account", session = %session_id, implied, tracked, units, "Shrapnel a session seems to have converted is not borne out by its weapon cost; it is left to the stock check");
        return Ok(None);
    }
    Ok(Some(SessionShrapnel {
        step: AccountStep {
            reading_id: closing.0,
            prev_reading_id: first.header.id.0,
            seen_from: from,
            seen_to: to,
            hand_entries_since: from - HAND_ENTRY_LOOK_BACK,
            stock_settles_by: to + STOCK_SETTLES,
            card_change_pec: None,
        },
        units,
    }))
}
