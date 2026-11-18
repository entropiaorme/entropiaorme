//! A running session's costs so far: what its readings measured up to the
//! latest of them, published for the live figures, and how the live figure
//! meets the sealed costs at the session's close.

use super::amount::MICRO_PER_PEC;
use super::costs::{session_costs, Basis, CostBucket};
use super::live::{publish, so_far, SoFar};
use super::reading::Container;
use super::roles::KnownRoles;
use super::store::{record_reading, Purpose};
use super::test_support::reading;
use crate::db::migrated_in_memory;
use crate::session_costs::{self, Bucket, BucketCosts};

const C: Container = Container::Carried;
const PEC: i64 = MICRO_PER_PEC;

/// The player's weapon, and an unlimited armour set.
fn gear(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) VALUES \
         ('Opalo', 'weapon', '{\"weapon_entity\": {\"name\": \"Opalo\"}}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('armour', 'Frontier', 'unlimited', NULL, 1)",
        [],
    )
    .unwrap();
}

/// A session started at 1000, still running.
fn running(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, 1000.0, NULL, 1)",
        [id],
    )
    .unwrap();
}

/// The account at `at`, as the connection stores a reading of the session
/// `id`: the weapon, the harness and the teleport chip at these PEC.
fn read(
    conn: &mut rusqlite::Connection,
    id: &str,
    purpose: Purpose,
    at: f64,
    (opalo, harness, chip): (i64, i64, i64),
) {
    let reading = reading(
        at,
        1_000,
        &[
            ("Opalo", C, 1, opalo),
            ("Frontier Harness, Adjusted (M)", C, 1, harness),
            ("Teleportation Chip 1", C, 1, chip),
        ],
    );
    record_reading(conn, &reading, purpose, Some(id)).unwrap();
}

/// The hunt's first three readings: its baseline, then a minute and two
/// minutes in. The weapon wears 10 then 5 PEC, the harness 10 then 5, and
/// a teleport in the first minute wears the chip 2.
fn three_minutes(conn: &mut rusqlite::Connection, id: &str) {
    read(conn, id, Purpose::Baseline, 1_000.5, (5_000, 4_000, 8_665));
    read(conn, id, Purpose::Session, 1_060.0, (4_990, 3_990, 8_663));
    read(conn, id, Purpose::Session, 1_120.0, (4_985, 3_985, 8_663));
}

fn so_far_of(conn: &rusqlite::Connection, id: &str) -> Option<SoFar> {
    let roles = KnownRoles::read(conn, None).unwrap();
    so_far(conn, id, &roles).unwrap()
}

fn published_rows(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM account_live_session_costs",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn nothing_is_measured_so_far_before_a_reading_after_the_baseline() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    assert_eq!(so_far_of(&conn, "hunt"), None, "no reading at all");
    read(
        &mut conn,
        "hunt",
        Purpose::Baseline,
        1_000.5,
        (5_000, 4_000, 8_665),
    );
    assert_eq!(so_far_of(&conn, "hunt"), None, "the baseline alone");
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_060.0,
        (4_990, 3_990, 8_663),
    );
    assert!(so_far_of(&conn, "hunt").is_some());
}

#[test]
fn a_running_session_is_measured_up_to_its_latest_reading() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    three_minutes(&mut conn, "hunt");
    let so_far = so_far_of(&conn, "hunt").expect("measured so far");
    assert_eq!(so_far.through, 1_120.0);
    assert_eq!(so_far.measured(CostBucket::Weapon), Some(15 * PEC));
    assert_eq!(so_far.measured(CostBucket::Armour), Some(15 * PEC));
    assert_eq!(so_far.measured(CostBucket::Travel), Some(2 * PEC));
    assert_eq!(so_far.measured(CostBucket::Healing), Some(0));
    // The readings never measure the enhancer.
    assert_eq!(so_far.measured(CostBucket::Enhancer), None);

    assert!(publish(&mut conn, &so_far, 1_121.0).unwrap());
    let read_back = session_costs::measured_so_far(&conn, "hunt")
        .unwrap()
        .expect("what was published");
    assert_eq!(read_back.through, 1_120.0);
    assert_eq!(read_back.bucket(Bucket::Weapon), Some(0.15));
    assert_eq!(read_back.bucket(Bucket::Armour), Some(0.15));
    assert_eq!(read_back.bucket(Bucket::Healing), Some(0.0));
    assert_eq!(read_back.bucket(Bucket::Enhancer), None);
    assert_eq!(read_back.overhead.travel, 0.02);

    // The next reading replaces it, never adds a second row.
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_180.0,
        (4_980, 3_985, 8_663),
    );
    let next = so_far_of(&conn, "hunt").unwrap();
    assert!(publish(&mut conn, &next, 1_181.0).unwrap());
    assert_eq!(published_rows(&conn), 1);
    let read_back = session_costs::measured_so_far(&conn, "hunt")
        .unwrap()
        .unwrap();
    assert_eq!(read_back.through, 1_180.0);
    assert_eq!(read_back.bucket(Bucket::Weapon), Some(0.2));
}

#[test]
fn what_is_published_stands_only_for_the_latest_reading_of_a_session_still_running() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    read(
        &mut conn,
        "hunt",
        Purpose::Baseline,
        1_000.5,
        (5_000, 4_000, 8_665),
    );
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_060.0,
        (4_990, 3_990, 8_663),
    );
    let stale = so_far_of(&conn, "hunt").unwrap();
    // A newer reading was stored meanwhile: it publishes its own.
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_120.0,
        (4_985, 3_985, 8_663),
    );
    assert!(!publish(&mut conn, &stale, 1_121.0).unwrap());
    assert_eq!(published_rows(&conn), 0);

    let current = so_far_of(&conn, "hunt").unwrap();
    // The session stopped meanwhile: its close is read by its seal.
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 1130, is_active = 0 WHERE id = 'hunt'",
        [],
    )
    .unwrap();
    assert!(!publish(&mut conn, &current, 1_131.0).unwrap());
    assert_eq!(published_rows(&conn), 0);
    assert_eq!(so_far_of(&conn, "hunt"), None, "a stopped session");
}

#[test]
fn sealing_the_session_drops_what_was_published_of_it() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    three_minutes(&mut conn, "hunt");
    let so_far = so_far_of(&conn, "hunt").unwrap();
    assert!(publish(&mut conn, &so_far, 1_121.0).unwrap());
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 1130, is_active = 0 WHERE id = 'hunt'",
        [],
    )
    .unwrap();
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_150.0,
        (4_985, 3_985, 8_663),
    );
    let sealed = session_costs(&mut conn, "hunt", None, 5_000.0, None)
        .unwrap()
        .unwrap();
    assert!(sealed.status.is_measured());
    assert_eq!(published_rows(&conn), 0);
}

fn sealed_weapon(costs: &super::costs::SessionCosts) -> i64 {
    match costs.bucket(CostBucket::Weapon).unwrap().basis {
        Basis::Measured(amount) => amount.micro_ped(),
        other => panic!("the weapon not measured: {other:?}"),
    }
}

#[test]
fn a_seal_drops_what_was_published_whatever_the_sealed_costs_come_to() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    read(
        &mut conn,
        "hunt",
        Purpose::Baseline,
        1_000.5,
        (5_000, 4_000, 8_665),
    );
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_060.0,
        (4_990, 3_990, 8_663),
    );
    let so_far = so_far_of(&conn, "hunt").unwrap();
    assert!(publish(&mut conn, &so_far, 1_061.0).unwrap());
    // The readings stop for five minutes, then the session stops: sealed
    // with a gap, its costs are not measured, and nothing stays published.
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_400.0,
        (4_980, 3_985, 8_663),
    );
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 1410, is_active = 0 WHERE id = 'hunt'",
        [],
    )
    .unwrap();
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_430.0,
        (4_980, 3_985, 8_663),
    );
    let sealed = session_costs(&mut conn, "hunt", None, 5_000.0, None)
        .unwrap()
        .unwrap();
    assert!(!sealed.status.is_measured());
    assert_eq!(published_rows(&conn), 0);
    // The session's figures are its tracked ones again.
    assert_eq!(
        crate::session_costs::resolve(&conn, "hunt")
            .unwrap()
            .cost_basis(Bucket::Weapon),
        crate::session_costs::CostBasis::Tracked
    );
}

#[test]
fn at_its_close_the_live_figure_meets_the_sealed_one_within_the_last_minutes_spend() {
    let mut conn = migrated_in_memory().unwrap();
    gear(&conn);
    running(&conn, "hunt");
    three_minutes(&mut conn, "hunt");
    let so_far = so_far_of(&conn, "hunt").unwrap();
    assert!(publish(&mut conn, &so_far, 1_121.0).unwrap());
    let measured = session_costs::measured_so_far(&conn, "hunt")
        .unwrap()
        .unwrap();

    // A last minute of play wears the weapon 8 PEC more; the session stops
    // and its closing reading finds nothing further.
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_180.0,
        (4_977, 3_985, 8_663),
    );
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 1185, is_active = 0 WHERE id = 'hunt'",
        [],
    )
    .unwrap();
    read(
        &mut conn,
        "hunt",
        Purpose::Session,
        1_205.0,
        (4_977, 3_985, 8_663),
    );
    let sealed = session_costs(&mut conn, "hunt", None, 5_000.0, None)
        .unwrap()
        .unwrap();
    let sealed = sealed_weapon(&sealed) as f64 / 1_000_000.0;
    assert!((sealed - 0.23).abs() < 1e-9, "{sealed}");

    // The live figure just before the stop: measured up to the reading at
    // 1120, and the tracked shots since. The tracker priced the play up to
    // that reading at 0.18 against the 0.15 measured, so only the blend
    // meets the sealed figure; where the tracker priced the last minute
    // exactly, the two agree to the PEC.
    let since = |weapon: f64| BucketCosts {
        weapon,
        ..BucketCosts::default()
    };
    let tracked = |since: f64| BucketCosts {
        weapon: 0.18 + since,
        ..BucketCosts::default()
    };
    let unblended = session_costs::live(None, &tracked(0.08), &since(0.08));
    assert!((unblended.cycled.weapon - sealed).abs() > 0.02);
    let exact = session_costs::live(Some(&measured), &tracked(0.08), &since(0.08));
    assert!((exact.cycled.weapon - sealed).abs() < 1e-9);
    // A tracker a PEC high over that minute leaves the live figure a PEC
    // from the sealed one: within the last minute's tracked spend.
    let high = session_costs::live(Some(&measured), &tracked(0.09), &since(0.09));
    let apart = (high.cycled.weapon - sealed).abs();
    assert!((apart - 0.01).abs() < 1e-9, "{apart}");
    assert!(apart <= 0.09);
}
