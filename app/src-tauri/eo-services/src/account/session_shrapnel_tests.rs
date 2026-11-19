//! What a session turned from Shrapnel into ammo, read from the session
//! whole: found when the weapon cost bears it out, and left alone when the
//! chat log missed loot, the tracked cost disagrees, nothing was converted,
//! or the reading is not the session's last.

use super::reading::{Container, Reading};
use super::session_shrapnel::at_close;
use super::store::{record_reading, Purpose, ReadingId};
use super::test_support::reading;
use crate::db::migrated_in_memory;

const C: Container = Container::Carried;
const SESSION: &str = "hunt";

/// A session from 1 to 175 read a minute apart from 0. Ten thousand ammo
/// (1 PED) is fired each minute; in the second minute 2,000 Shrapnel is
/// looted and, when `converting`, all 12,000 held becomes 12,120 ammo
/// stacked onto the row being fired.
fn readings(converting: bool) -> Vec<Reading> {
    let shrapnel = |units: i64| ("Shrapnel", C, units, units / 100);
    let ammo = |units: i64| ("Universal Ammo", C, units, units / 100);
    if converting {
        vec![
            reading(0.0, 0, &[shrapnel(10_000), ammo(100_000)]),
            reading(60.0, 0, &[shrapnel(10_000), ammo(90_000)]),
            reading(120.0, 0, &[ammo(92_120)]),
            reading(180.0, 0, &[ammo(82_120)]),
        ]
    } else {
        vec![
            reading(0.0, 0, &[shrapnel(10_000), ammo(100_000)]),
            reading(60.0, 0, &[shrapnel(10_000), ammo(90_000)]),
            reading(120.0, 0, &[shrapnel(12_000), ammo(80_000)]),
            reading(180.0, 0, &[shrapnel(12_000), ammo(70_000)]),
        ]
    }
}

fn session(conn: &mut rusqlite::Connection, readings: &[Reading], active: bool) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, 1.0, ?2, ?3)",
        rusqlite::params![SESSION, (!active).then_some(175.0), active],
    )
    .unwrap();
    for reading in readings {
        record_reading(conn, reading, Purpose::Session, Some(SESSION)).unwrap();
    }
}

/// A kill ended at `at` whose weapon spent `spend` PED.
fn kill(conn: &rusqlite::Connection, id: &str, at: f64, spend: f64) {
    conn.execute(
        "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, loot_total_ped) \
         VALUES (?1, ?2, 'Atrox', ?3, ?4, 0)",
        rusqlite::params![id, SESSION, at, spend],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
         VALUES (?1, 'Opalo', 100, ?2)",
        rusqlite::params![id, spend / 100.0],
    )
    .unwrap();
}

fn loot(conn: &rusqlite::Connection, kill: &str, units: i64) {
    conn.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
         VALUES (?1, 'Shrapnel', ?2, ?3)",
        rusqlite::params![kill, units, units as f64 / 10_000.0],
    )
    .unwrap();
}

/// The session with a kill a minute, each spending `spend` PED.
fn hunted(converting: bool, spend: f64, looted: bool) -> rusqlite::Connection {
    let mut conn = migrated_in_memory().unwrap();
    session(&mut conn, &readings(converting), false);
    for (id, at) in [("k1", 30.0), ("k2", 90.0), ("k3", 150.0)] {
        kill(&conn, id, at, spend);
    }
    if looted {
        loot(&conn, "k2", 2_000);
    }
    conn
}

fn found(conn: &rusqlite::Connection, closing: i64) -> Option<i64> {
    at_close(conn, ReadingId(closing), None, 1_000.0)
        .unwrap()
        .map(|session| session.units)
}

#[test]
fn what_was_looted_less_what_is_held_was_converted_when_the_weapon_cost_bears_it_out() {
    let conn = hunted(true, 1.0, true);
    let session = at_close(&conn, ReadingId(4), None, 1_000.0)
        .unwrap()
        .expect("a conversion");
    // The 10,000 held and the 2,000 looted: 30,000 ammo fired, 3 PED, as
    // the three kills tracked.
    assert_eq!(session.units, 12_000);
    assert_eq!(
        (session.step.prev_reading_id, session.step.reading_id),
        (1, 4)
    );
    assert_eq!((session.step.seen_from, session.step.seen_to), (0.0, 180.0));
}

#[test]
fn a_session_that_converted_nothing_records_nothing() {
    assert_eq!(found(&hunted(false, 1.0, true), 4), None);
}

#[test]
fn loot_the_chat_log_missed_leaves_the_weapon_cost_short_and_nothing_recorded() {
    // Without the 2,000 looted, 10,000 converted pays 10,100: the ammo
    // fired comes to 2.80 PED against 3.00 tracked.
    assert_eq!(found(&hunted(true, 1.0, false), 4), None);
}

#[test]
fn a_tracked_weapon_cost_that_disagrees_records_nothing() {
    assert_eq!(found(&hunted(true, 2.0, true), 4), None);
    // Within two in a hundred it stands.
    assert_eq!(found(&hunted(true, 1.015, true), 4), Some(12_000));
}

#[test]
fn only_the_last_reading_of_a_session_that_has_ended_is_read() {
    let conn = hunted(true, 1.0, true);
    assert_eq!(found(&conn, 3), None, "not the last reading");
    let mut running = migrated_in_memory().unwrap();
    session(&mut running, &readings(true), true);
    assert_eq!(found(&running, 4), None, "still running");
}
