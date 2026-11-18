//! Overhead between sessions: which steps lie between them, what they bill
//! and on which day, that what performing an activity costs is left out,
//! that a step a session later covers is never billed twice, and that a
//! step is priced again when the roles move.

use super::between::{self, is_between, overhead_in};
use super::reading::Container;
use super::roles::{CostCategory, KnownRoles};
use super::store::{load_reading, record_reading, Purpose, StoredReading};
use super::test_support::reading;
use crate::db::migrated_in_memory;

const C: Container = Container::Carried;

/// A day in 2026, so the rollup keeps it.
const DAY: f64 = 1_780_000_000.0;

type Rows<'a> = &'a [(&'static str, Container, i64, i64)];

fn stored(
    conn: &mut rusqlite::Connection,
    at: f64,
    rows: Rows,
    purpose: Purpose,
    session: Option<&str>,
) -> StoredReading {
    let header = record_reading(conn, &reading(at, 1_000, rows), purpose, session).unwrap();
    load_reading(conn, header.id).unwrap().unwrap()
}

fn session_row(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, ?2, ?3, 0)",
        rusqlite::params![id, DAY, DAY + 600.0],
    )
    .unwrap();
}

/// A teleport between two idle readings: the chip wears 0.02 and 900 Mind
/// Essence (0.09) burns; the player's weapon wears 0.10 meanwhile; a
/// hoverpod nothing names wears 0.05.
fn teleport(conn: &mut rusqlite::Connection) -> (StoredReading, StoredReading) {
    let before = stored(
        conn,
        DAY,
        &[
            ("Teleportation Chip 1", C, 1, 8_665),
            ("Mind Essence", C, 2_000, 20),
            ("Opalo", C, 1, 5_000),
            ("Exploration Hoverpod MkI (L)", C, 1, 1_200),
        ],
        Purpose::Ambient,
        None,
    );
    let after = stored(
        conn,
        DAY + 60.0,
        &[
            ("Teleportation Chip 1", C, 1, 8_663),
            ("Mind Essence", C, 1_100, 11),
            ("Opalo", C, 1, 4_990),
            ("Exploration Hoverpod MkI (L)", C, 1, 1_195),
        ],
        Purpose::Ambient,
        None,
    );
    (before, after)
}

fn roles(placements: &[(String, CostCategory)]) -> KnownRoles {
    KnownRoles::with_placements(
        &[("Opalo".to_string(), "weapon".to_string(), "{}".to_string())],
        &[],
        placements,
        None,
    )
}

fn close(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("a figure");
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn a_step_lies_between_sessions_unless_a_session_or_a_run_read_it() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "s");
    let rows: Rows = &[("Teleportation Chip 1", C, 1, 8_665)];
    let idle = stored(&mut conn, DAY - 120.0, rows, Purpose::Ambient, None);
    let idle_again = stored(&mut conn, DAY - 60.0, rows, Purpose::Ambient, None);
    let started = stored(&mut conn, DAY, rows, Purpose::Baseline, Some("s"));
    let playing = stored(&mut conn, DAY + 60.0, rows, Purpose::Session, Some("s"));
    let run_start = stored(&mut conn, DAY + 700.0, rows, Purpose::Run, None);
    let run_end = stored(&mut conn, DAY + 760.0, rows, Purpose::Run, None);
    let after_run = stored(&mut conn, DAY + 820.0, rows, Purpose::Ambient, None);

    assert!(is_between(&idle, &idle_again));
    // Up to a session's first reading is between: it read nothing yet.
    assert!(is_between(&idle_again, &started));
    assert!(!is_between(&started, &playing));
    // A strongbox run's own steps are the run's; the step after it is not.
    assert!(!is_between(&run_start, &run_end));
    assert!(is_between(&run_end, &after_run));
}

#[test]
fn travel_between_sessions_is_billed_on_its_day_and_an_activity_s_wear_is_left() {
    let mut conn = migrated_in_memory().unwrap();
    // The days are rolled up to a few days on, as the app keeps them.
    crate::daily_rollup::heal_rollups(&mut conn, DAY + 3.0 * 86_400.0).unwrap();
    let (before, after) = teleport(&mut conn);
    let roles = roles(&[]);
    let step = between::measure(&before, &after, &roles)
        .unwrap()
        .expect("something wore");
    between::store(&conn, &step, &roles, DAY + 100.0).unwrap();
    // Kept again, nothing changes.
    between::store(&conn, &step, &roles, DAY + 200.0).unwrap();

    let (steps, [travel, logistics, unclassified]) =
        overhead_in(&conn, DAY - 1.0, DAY + 86_400.0).unwrap();
    assert_eq!(steps, 1);
    // The chip and its Mind Essence: 0.11 of travel.
    close(travel, 0.11);
    close(logistics, 0.0);
    // The hoverpod nothing names is unclassified; the weapon's wear is
    // play, never overhead.
    close(unclassified, 0.05);

    // The day's rollup carries it, as overhead and as what was billed
    // between sessions.
    let day = crate::daily_rollup::epoch_day(DAY + 60.0);
    let (rolled_travel, rolled_between): (f64, f64) = conn
        .query_row(
            "SELECT overhead_travel, overhead_between_sessions FROM daily_rollups WHERE day = ?1",
            [&day],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!((rolled_travel - 0.11).abs() < 1e-9);
    assert!((rolled_between - 0.16).abs() < 1e-9);
}

#[test]
fn a_step_with_nothing_worn_is_not_kept() {
    let mut conn = migrated_in_memory().unwrap();
    let rows: Rows = &[("Teleportation Chip 1", C, 1, 8_665)];
    let before = stored(&mut conn, DAY, rows, Purpose::Ambient, None);
    let after = stored(&mut conn, DAY + 60.0, rows, Purpose::Ambient, None);
    assert!(between::measure(&before, &after, &roles(&[]))
        .unwrap()
        .is_none());
}

#[test]
fn a_step_a_session_came_to_cover_is_billed_to_it_never_twice() {
    let mut conn = migrated_in_memory().unwrap();
    let (before, after) = teleport(&mut conn);
    let roles = roles(&[]);
    let step = between::measure(&before, &after, &roles).unwrap().unwrap();
    between::store(&conn, &step, &roles, DAY + 100.0).unwrap();

    // A session's costs, read across a gap, came to span the step.
    session_row(&conn, "s");
    conn.execute(
        "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
         measure_computed_at, status, reason, first_reading_id, last_reading_id, computed_at) \
         VALUES ('s', 1, 1, 1.0, 'measured', NULL, ?1, ?2, 2.0)",
        rusqlite::params![before.header.id.0, after.header.id.0],
    )
    .unwrap();
    let (steps, [travel, ..]) = overhead_in(&conn, DAY - 1.0, DAY + 86_400.0).unwrap();
    assert_eq!(steps, 0);
    assert!(travel.is_none());
}

#[test]
fn a_step_is_priced_again_when_the_player_places_what_it_used() {
    let mut conn = migrated_in_memory().unwrap();
    let (before, after) = teleport(&mut conn);
    let unplaced = roles(&[]);
    let step = between::measure(&before, &after, &unplaced)
        .unwrap()
        .unwrap();
    between::store(&conn, &step, &unplaced, DAY + 100.0).unwrap();
    assert!(!between::any_stale(&conn, &unplaced).unwrap());

    // The player places the hoverpod as travel: the step waits to be
    // priced again, and is.
    let placed = roles(&[(
        "Exploration Hoverpod MkI (L)".to_string(),
        CostCategory::Travel,
    )]);
    assert!(between::any_stale(&conn, &placed).unwrap());
    assert_eq!(between::heal(&mut conn, &placed, DAY + 300.0).unwrap(), 1);
    assert!(!between::any_stale(&conn, &placed).unwrap());
    let (_, [travel, _, unclassified]) = overhead_in(&conn, DAY - 1.0, DAY + 86_400.0).unwrap();
    close(travel, 0.16);
    close(unclassified, 0.0);
    // Healed once, nothing is left to heal.
    assert_eq!(between::heal(&mut conn, &placed, DAY + 400.0).unwrap(), 0);
}
