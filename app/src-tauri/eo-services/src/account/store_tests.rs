//! The store: keyframes and changes, order, rebuilding, and the cached
//! session measure.

use proptest::prelude::*;

use super::measure::{SessionWindow, MEASURE_VERSION};
use super::reading::Container;
use super::roles::{CostCategory, RoleSource};
use super::store::{
    cached_measure, last_stored, load_reading, readings_for_session, readings_for_window, record,
    record_reading, refresh_session_measure, session_measure, CachedMeasure, Frame, Purpose,
    ReadingId, KEYFRAME_EVERY,
};
use super::store::{record_reading_within, ChainLimits, CHAIN_CAP};
use super::test_support::{reading, recorded_hunt, ReadingSpec};
use super::AccountError;
use crate::db::migrated_in_memory;

/// No player data and no game data.
struct Nothing;

impl RoleSource for Nothing {
    fn library_category(&self, _: &str) -> Option<CostCategory> {
        None
    }
    fn catalogue_category(&self, _: &str) -> Option<CostCategory> {
        None
    }
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect("count")
}

fn ammo(at: f64, quantity: i64) -> super::Reading {
    reading(
        at,
        25_000,
        &[
            (
                "Universal Ammo",
                Container::Carried,
                quantity,
                quantity / 100,
            ),
            ("Opalo", Container::Carried, 1, 500),
            (
                "Common Dung",
                Container::Storage {
                    planet: "The Hub".into(),
                },
                29,
                0,
            ),
        ],
    )
}

#[test]
fn the_first_reading_is_a_keyframe_and_a_minute_later_only_changes_are_stored() {
    let mut conn = migrated_in_memory().expect("a migrated database");
    let first = record_reading(&mut conn, &ammo(1_000.0, 900_000), Purpose::Session, None).unwrap();
    assert_eq!(first.frame, Frame::Key);
    assert_eq!(first.prev, None);
    let second =
        record_reading(&mut conn, &ammo(1_060.0, 800_000), Purpose::Session, None).unwrap();
    assert_eq!(second.frame, Frame::Delta);
    assert_eq!(second.prev, Some(first.id));
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT COUNT(*) FROM account_reading_holdings WHERE reading_id = {}",
                first.id.0
            )
        ),
        3
    );
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT COUNT(*) FROM account_reading_holdings WHERE reading_id = {}",
                second.id.0
            )
        ),
        1,
        "only the ammo changed"
    );
    assert_eq!(
        load_reading(&conn, second.id).unwrap().unwrap().reading,
        ammo(1_060.0, 800_000)
    );
    assert_eq!(
        load_reading(&conn, first.id).unwrap().unwrap().reading,
        ammo(1_000.0, 900_000)
    );
    assert_eq!(load_reading(&conn, ReadingId(99)).unwrap(), None);
}

#[test]
fn a_holding_that_is_gone_is_stored_as_an_empty_state() {
    let mut conn = migrated_in_memory().unwrap();
    record_reading(&mut conn, &ammo(1_000.0, 900_000), Purpose::Session, None).unwrap();
    let emptied = reading(1_060.0, 25_000, &[("Opalo", Container::Carried, 1, 500)]);
    let header = record_reading(&mut conn, &emptied, Purpose::Session, None).unwrap();
    assert_eq!(header.frame, Frame::Delta);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM account_reading_holdings \
             WHERE reading_id = 2 AND row_count = 0 AND rows_json = '[]'"
        ),
        2
    );
    assert_eq!(
        load_reading(&conn, header.id).unwrap().unwrap().reading,
        emptied
    );
}

#[test]
fn a_connect_reading_a_long_pause_and_a_session_s_first_reading_are_keyframes() {
    let mut conn = migrated_in_memory().unwrap();
    let frame = |conn: &mut rusqlite::Connection, at: f64, purpose, session: Option<&str>| {
        record_reading(conn, &ammo(at, 900_000), purpose, session)
            .unwrap()
            .frame
    };
    assert_eq!(frame(&mut conn, 0.0, Purpose::Manual, None), Frame::Key);
    assert_eq!(frame(&mut conn, 60.0, Purpose::Connect, None), Frame::Key);
    assert_eq!(frame(&mut conn, 120.0, Purpose::Run, None), Frame::Delta);
    assert_eq!(
        frame(&mut conn, 720.0, Purpose::Run, None),
        Frame::Delta,
        "ten minutes is not more"
    );
    // A pause of more than ten minutes before a reading that changes
    // something starts a keyframe.
    assert_eq!(
        record_reading(&mut conn, &ammo(1_320.5, 899_000), Purpose::Run, None)
            .unwrap()
            .frame,
        Frame::Key
    );
    assert_eq!(
        frame(&mut conn, 1_380.0, Purpose::Session, Some("s1")),
        Frame::Key
    );
    assert_eq!(
        frame(&mut conn, 1_440.0, Purpose::Session, Some("s1")),
        Frame::Delta
    );
    assert_eq!(
        frame(&mut conn, 1_500.0, Purpose::Session, Some("s2")),
        Frame::Key
    );
    let latest = last_stored(&conn).unwrap().unwrap();
    assert_eq!(latest.session_id.as_deref(), Some("s2"));
    assert_eq!(latest.purpose, Purpose::Session);
    assert_eq!(latest.row_count, 3);
}

#[test]
fn readings_that_change_nothing_store_no_holdings_however_long_the_pause() {
    let mut conn = migrated_in_memory().unwrap();
    let holdings =
        |conn: &rusqlite::Connection| count(conn, "SELECT COUNT(*) FROM account_reading_holdings");
    let first = record_reading(&mut conn, &ammo(0.0, 900_000), Purpose::Ambient, None).unwrap();
    assert_eq!(first.frame, Frame::Key);
    let kept = holdings(&conn);
    assert_eq!(kept, 3);
    // Far more readings than a keyframe allows, five minutes apart, then
    // one after an hour's pause: every one a row of its own, with its own
    // balance, and no holding stored.
    let mut last = first;
    for index in 1..=(2 * KEYFRAME_EVERY as i64) {
        let mut same = ammo(300.0 * index as f64, 900_000);
        if index % 7 == 0 {
            same = reading(
                300.0 * index as f64,
                24_000,
                &[
                    ("Universal Ammo", Container::Carried, 900_000, 9_000),
                    ("Opalo", Container::Carried, 1, 500),
                    (
                        "Common Dung",
                        Container::Storage {
                            planet: "The Hub".into(),
                        },
                        29,
                        0,
                    ),
                ],
            );
        }
        last = record_reading(&mut conn, &same, Purpose::Ambient, None).unwrap();
        assert_eq!(last.frame, Frame::Delta);
        assert_eq!(last.prev.map(|prev| prev.0), Some(last.id.0 - 1));
    }
    let paused = record_reading(
        &mut conn,
        &ammo(300.0 * (2 * KEYFRAME_EVERY) as f64 + 3_600.0, 900_000),
        Purpose::Ambient,
        None,
    )
    .unwrap();
    assert_eq!(paused.frame, Frame::Delta, "nothing changed in the pause");
    assert_eq!(holdings(&conn), kept, "the holdings did not grow");
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM account_readings"),
        2 * KEYFRAME_EVERY as i64 + 2
    );
    // Each rebuilds whole, the Card as it was read.
    assert_eq!(
        load_reading(&conn, paused.id).unwrap().unwrap().reading,
        ammo(300.0 * (2 * KEYFRAME_EVERY) as f64 + 3_600.0, 900_000)
    );
    assert_eq!(
        load_reading(&conn, ReadingId(8))
            .unwrap()
            .unwrap()
            .reading
            .balance(),
        super::Pec(24_000)
    );
    assert_eq!(load_reading(&conn, last.id).unwrap().unwrap().header, last);
    // A change after the long chain starts a keyframe again.
    let changed = record_reading(
        &mut conn,
        &ammo(300.0 * (2 * KEYFRAME_EVERY) as f64 + 3_660.0, 899_000),
        Purpose::Ambient,
        None,
    )
    .unwrap();
    assert_eq!(changed.frame, Frame::Key);
}

#[test]
fn readings_that_change_nothing_are_stored_whole_once_the_chain_reaches_its_cap() {
    assert_eq!(CHAIN_CAP, 10 * KEYFRAME_EVERY);
    let mut conn = migrated_in_memory().unwrap();
    let limits = ChainLimits { every: 2, cap: 4 };
    let frames: Vec<Frame> = (0..11)
        .map(|index| {
            record_reading_within(
                &mut conn,
                &ammo(60.0 * f64::from(index), 900_000),
                Purpose::Ambient,
                None,
                limits,
            )
            .unwrap()
            .frame
        })
        .collect();
    // A keyframe, four readings of nothing, a keyframe, and so on.
    let keyframes: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| **frame == Frame::Key)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(keyframes, vec![0, 5, 10]);
    // With the stored limits a change after the cap's worth of nothing is
    // a keyframe, and so is the reading of nothing that reaches the cap.
    let mut conn = migrated_in_memory().unwrap();
    let mut last = Frame::Key;
    for index in 0..=CHAIN_CAP {
        last = record_reading(
            &mut conn,
            &ammo(300.0 * index as f64, 900_000),
            Purpose::Ambient,
            None,
        )
        .unwrap()
        .frame;
    }
    assert_eq!(
        last,
        Frame::Delta,
        "the cap's own reading follows the chain"
    );
    assert_eq!(
        record_reading(
            &mut conn,
            &ammo(300.0 * (CHAIN_CAP + 1) as f64, 900_000),
            Purpose::Ambient,
            None,
        )
        .unwrap()
        .frame,
        Frame::Key
    );
}

#[test]
fn a_reading_taken_after_the_clock_stepped_back_is_stored_and_follows_by_insertion() {
    let mut conn = migrated_in_memory().unwrap();
    let first = record_reading(&mut conn, &ammo(3_600.0, 900_000), Purpose::Session, None).unwrap();
    let second =
        record_reading(&mut conn, &ammo(3_660.0, 890_000), Purpose::Session, None).unwrap();
    // The wall clock steps back an hour: the reading is stored all the
    // same, follows the one stored before it, and stores only its changes.
    let stepped = record_reading(&mut conn, &ammo(120.0, 880_000), Purpose::Session, None).unwrap();
    assert_eq!(stepped.prev, Some(second.id));
    assert_eq!(stepped.frame, Frame::Delta);
    let after = record_reading(&mut conn, &ammo(180.0, 870_000), Purpose::Session, None).unwrap();
    assert_eq!(after.prev, Some(stepped.id));
    assert_eq!(last_stored(&conn).unwrap().unwrap().id, after.id);
    for (id, at, quantity) in [
        (first.id, 3_600.0, 900_000),
        (second.id, 3_660.0, 890_000),
        (stepped.id, 120.0, 880_000),
        (after.id, 180.0, 870_000),
    ] {
        assert_eq!(
            load_reading(&conn, id).unwrap().unwrap().reading,
            ammo(at, quantity)
        );
    }
    // A session in the repeated hour gathers its readings in the order
    // they were stored.
    let window = SessionWindow {
        started_at: 100.0,
        ended_at: Some(200.0),
    };
    let ids: Vec<ReadingId> = readings_for_window(&conn, &window)
        .unwrap()
        .iter()
        .map(|stored| stored.header.id)
        .collect();
    assert_eq!(ids, vec![second.id, stepped.id, after.id]);
}

#[test]
fn a_keyframe_follows_every_thirty_readings_of_changes() {
    let mut conn = migrated_in_memory().unwrap();
    let mut frames = Vec::new();
    for index in 0..63 {
        let at = 1_000.0 + 60.0 * f64::from(index);
        let header = record_reading(
            &mut conn,
            &ammo(at, 900_000 - i64::from(index)),
            Purpose::Session,
            None,
        )
        .unwrap();
        frames.push(header.frame);
    }
    let keyframes: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| **frame == Frame::Key)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        keyframes,
        vec![0, 31, 62],
        "a keyframe, thirty readings of changes, a keyframe"
    );
}

#[test]
fn an_unknown_stored_purpose_or_place_is_a_typed_decode_error() {
    let mut conn = migrated_in_memory().unwrap();
    let header =
        record_reading(&mut conn, &ammo(1_000.0, 900_000), Purpose::Session, None).unwrap();
    conn.execute("UPDATE account_readings SET purpose = 'idle'", [])
        .unwrap();
    assert!(matches!(
        load_reading(&conn, header.id),
        Err(AccountError::Stored(_))
    ));
    conn.execute("UPDATE account_readings SET purpose = 'session'", [])
        .unwrap();
    conn.execute(
        "UPDATE account_reading_holdings SET container_kind = 'vault', container_name = 'x' \
         WHERE item_name = 'Opalo'",
        [],
    )
    .unwrap();
    assert!(matches!(
        load_reading(&conn, header.id),
        Err(AccountError::Stored(_))
    ));
}

#[test]
fn an_unknown_place_is_stored_under_its_own_kind_and_reads_back() {
    let mut conn = migrated_in_memory().unwrap();
    let estate = reading(
        1_000.0,
        0,
        &[(
            "Odd Chair",
            Container::Other {
                name: "ESTATE (Calypso)".into(),
            },
            1,
            300,
        )],
    );
    let header = record_reading(&mut conn, &estate, Purpose::Manual, None).unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM account_reading_holdings WHERE container_kind = 'other'"
        ),
        1
    );
    assert_eq!(
        load_reading(&conn, header.id).unwrap().unwrap().reading,
        estate
    );
}

#[test]
fn a_stored_holding_whose_rows_do_not_add_up_does_not_decode() {
    let mut conn = migrated_in_memory().unwrap();
    let header =
        record_reading(&mut conn, &ammo(1_000.0, 900_000), Purpose::Session, None).unwrap();
    conn.execute(
        "UPDATE account_reading_holdings SET quantity = quantity + 1 \
         WHERE item_name = 'Universal Ammo'",
        [],
    )
    .unwrap();
    assert!(matches!(
        load_reading(&conn, header.id),
        Err(AccountError::Stored(_))
    ));
}

#[test]
fn a_new_reading_follows_the_one_stored_last_whatever_its_time() {
    let mut conn = migrated_in_memory().unwrap();
    record_reading(&mut conn, &ammo(1_000.0, 900_000), Purpose::Session, None).unwrap();
    let later = record_reading(&mut conn, &ammo(1_060.0, 800_000), Purpose::Session, None).unwrap();
    let earlier = record_reading(&mut conn, &ammo(10.0, 700_000), Purpose::Session, None).unwrap();
    assert_eq!(earlier.prev, Some(later.id));
    assert_eq!(last_stored(&conn).unwrap().unwrap().taken_at, 10.0);
}

#[test]
fn a_session_window_gathers_its_bracketing_readings_in_time_order() {
    let mut conn = migrated_in_memory().unwrap();
    for (index, at) in [100.0, 200.0, 260.0, 320.0, 400.0, 500.0]
        .into_iter()
        .enumerate()
    {
        let quantity = 900_000 - 10_000 * i64::try_from(index).unwrap();
        record_reading(&mut conn, &ammo(at, quantity), Purpose::Session, None).unwrap();
    }
    let window = SessionWindow {
        started_at: 230.0,
        ended_at: Some(350.0),
    };
    let readings = readings_for_window(&conn, &window).unwrap();
    let times: Vec<f64> = readings
        .iter()
        .map(|stored| stored.reading.taken_at())
        .collect();
    assert_eq!(times, vec![200.0, 260.0, 320.0, 400.0]);
    assert_eq!(readings[3].reading, ammo(400.0, 860_000));
    let running = SessionWindow {
        started_at: 230.0,
        ended_at: None,
    };
    assert_eq!(readings_for_window(&conn, &running).unwrap().len(), 5);
}

fn spec_strategy() -> impl Strategy<Value = ReadingSpec> {
    let names = prop::sample::select(vec!["Universal Ammo", "Shrapnel", "Opalo", "Gloves (M)"]);
    let containers = prop::sample::select(vec![
        Container::Carried,
        Container::Auction,
        Container::Storage {
            planet: "Calypso".into(),
        },
        Container::Attached {
            parent: "Gloves (M)".into(),
        },
    ]);
    (
        0i64..30_000,
        prop::collection::vec((names, containers, 0i64..2_000_000, 0i64..20_000), 0..10),
    )
        .prop_map(|(balance, rows)| ReadingSpec { balance, rows })
}

/// One migrated database shared by the property's cases, emptied of
/// readings before each: migrating afresh per case would dominate the run.
fn emptied_database() -> std::sync::MutexGuard<'static, rusqlite::Connection> {
    static DATABASE: std::sync::OnceLock<std::sync::Mutex<rusqlite::Connection>> =
        std::sync::OnceLock::new();
    let guard = DATABASE
        .get_or_init(|| std::sync::Mutex::new(migrated_in_memory().unwrap()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .execute_batch(
            "DELETE FROM account_reading_holdings; DELETE FROM account_readings; \
             DELETE FROM sqlite_sequence WHERE name = 'account_readings';",
        )
        .unwrap();
    guard
}

proptest! {
    /// Any sequence of readings, keyframes and changes mixed (readings
    /// that repeat the one before them among them, under chain limits low
    /// enough to be reached), loads back as the same readings, each on its
    /// own and as a session's run.
    #[test]
    fn stored_readings_load_back_as_the_same_states(
        sequence in prop::collection::vec(
            (
                spec_strategy(),
                1.0f64..900.0,
                prop::sample::select(vec![Purpose::Connect, Purpose::Session, Purpose::Ambient]),
                0usize..4,
                prop::bool::weighted(0.4),
            ),
            1..16,
        ),
        every in 1usize..4,
        beyond in 0usize..6,
    ) {
        let mut conn = emptied_database();
        let limits = ChainLimits { every, cap: every + beyond };
        let mut at = 1_000.0;
        let mut written: Vec<(ReadingId, super::Reading)> = Vec::new();
        for (spec, pause, purpose, session, repeat) in &sequence {
            at += pause;
            let session = ["a", "b", "c"].get(*session).copied();
            // The holdings of the reading before, read again.
            let original = match written.last() {
                Some((_, before)) if *repeat => super::Reading::new(
                    at,
                    super::Pec(spec.balance),
                    before.holdings().clone(),
                )
                .unwrap(),
                _ => spec.reading(at),
            };
            let header =
                record_reading_within(&mut conn, &original, *purpose, session, limits).unwrap();
            written.push((header.id, original));
        }
        for (id, original) in &written {
            let loaded = load_reading(&conn, *id).unwrap().unwrap();
            prop_assert_eq!(&loaded.reading, original);
        }
        let window = SessionWindow { started_at: 0.0, ended_at: None };
        let run = readings_for_window(&conn, &window).unwrap();
        prop_assert_eq!(run.len(), written.len());
        for (stored, (_, original)) in run.iter().zip(&written) {
            prop_assert_eq!(&stored.reading, original);
        }
    }
}

#[tokio::test]
async fn a_reading_is_recorded_through_the_writer() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(&dir.path().join("account.db"))
        .await
        .unwrap();
    let header = record(&db, ammo(10.0, 5_000), Purpose::Connect, Some("s".into()))
        .await
        .unwrap();
    assert_eq!(header.frame, Frame::Key);
    let loaded = db
        .with_reader(move |conn| Ok(load_reading(conn, header.id)))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(loaded.reading, ammo(10.0, 5_000));
    assert_eq!(loaded.header.session_id.as_deref(), Some("s"));
}

fn store_the_hunt(conn: &mut rusqlite::Connection) {
    for (index, hunt_reading) in recorded_hunt().iter().enumerate() {
        let session = (index >= 3).then_some("hunt");
        record_reading(conn, hunt_reading, Purpose::Session, session).unwrap();
    }
}

const HUNT: SessionWindow = SessionWindow {
    started_at: 1_794_754_640.0,
    ended_at: Some(1_794_755_350.0),
};

#[test]
fn a_session_measure_is_cached_with_its_readings_and_version() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn);
    let measure =
        refresh_session_measure(&conn, "hunt", &HUNT, &Nothing, Some(1_794_755_400.0), 1.0)
            .unwrap();
    let (first, last, coverage, readings, gaps, version): (i64, i64, String, i64, i64, i64) = conn
        .query_row(
            "SELECT first_reading_id, last_reading_id, coverage, reading_count, gap_count, \
             measure_version FROM account_session_measures WHERE session_id = 'hunt'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        (first, last),
        (3, 15),
        "from the last reading before the start to the first after the end"
    );
    assert_eq!(coverage, "complete");
    assert_eq!((readings, gaps), (13, 0));
    assert_eq!(version, MEASURE_VERSION);
    assert_eq!(
        cached_measure(&conn, "hunt").unwrap(),
        CachedMeasure::Current {
            measure: Box::new(measure),
            sealed_at: Some(1_794_755_400.0),
            computed_at: 1.0,
        }
    );
    assert_eq!(
        cached_measure(&conn, "other").unwrap(),
        CachedMeasure::Missing
    );
}

#[test]
fn a_sealed_current_measure_is_served_from_the_cache_and_an_older_one_recomputed() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn);
    let measure = refresh_session_measure(&conn, "hunt", &HUNT, &Nothing, Some(9.0), 1.0).unwrap();
    // A sealed measure of the current rules is not recomputed.
    assert_eq!(
        session_measure(&conn, "hunt", &HUNT, &Nothing, 2.0).unwrap(),
        measure
    );
    let computed_at = |conn: &rusqlite::Connection| -> f64 {
        conn.query_row(
            "SELECT computed_at FROM account_session_measures WHERE session_id = 'hunt'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(computed_at(&conn), 1.0);
    // One cached by other rules is, and so is one that no longer decodes.
    conn.execute(
        "UPDATE account_session_measures SET measure_version = ?1, measure_json = '{\"other\": 1}'",
        [MEASURE_VERSION + 1],
    )
    .unwrap();
    assert_eq!(
        cached_measure(&conn, "hunt").unwrap(),
        CachedMeasure::Stale {
            sealed_at: Some(9.0)
        }
    );
    assert_eq!(
        session_measure(&conn, "hunt", &HUNT, &Nothing, 3.0).unwrap(),
        measure
    );
    assert_eq!(computed_at(&conn), 3.0);
    conn.execute(
        "UPDATE account_session_measures SET measure_json = '{\"other\": 1}'",
        [],
    )
    .unwrap();
    assert_eq!(
        cached_measure(&conn, "hunt").unwrap(),
        CachedMeasure::Stale {
            sealed_at: Some(9.0)
        }
    );
    assert_eq!(
        session_measure(&conn, "hunt", &HUNT, &Nothing, 4.0).unwrap(),
        measure
    );
    assert_eq!(computed_at(&conn), 4.0);
    assert!(matches!(
        cached_measure(&conn, "hunt").unwrap(),
        CachedMeasure::Current {
            sealed_at: Some(9.0),
            ..
        }
    ));
}

#[test]
fn an_unsealed_measure_is_recomputed_on_every_read() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn);
    let running = SessionWindow {
        started_at: HUNT.started_at,
        ended_at: None,
    };
    refresh_session_measure(&conn, "hunt", &running, &Nothing, None, 1.0).unwrap();
    let measure = session_measure(&conn, "hunt", &running, &Nothing, 2.0).unwrap();
    assert_eq!(
        measure.coverage.level,
        super::measure::CoverageLevel::Partial
    );
    let computed_at: f64 = conn
        .query_row(
            "SELECT computed_at FROM account_session_measures",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(computed_at, 2.0);
}

#[test]
fn a_session_stopped_the_moment_it_started_caches_a_partial_measure() {
    let mut conn = migrated_in_memory().unwrap();
    record_reading(
        &mut conn,
        &ammo(1_000.0, 900_000),
        Purpose::Baseline,
        Some("zero"),
    )
    .unwrap();
    // Its one reading reads both its start and its end; complete would
    // break the cache's own rule that a complete measure has two readings.
    let window = SessionWindow {
        started_at: 1_000.0,
        ended_at: Some(1_000.0),
    };
    let measure = refresh_session_measure(&conn, "zero", &window, &Nothing, Some(1.0), 1.0)
        .expect("the cache takes it");
    assert_eq!(measure.coverage.reading_count, 1);
    assert_eq!(
        measure.coverage.level,
        super::measure::CoverageLevel::Partial
    );
}

#[test]
fn a_session_s_readings_are_its_own_never_another_session_s_between_them() {
    let mut conn = migrated_in_memory().unwrap();
    // Two sessions' readings stored interleaved: A's ammo holds steady,
    // B's falls.
    let mut ids = Vec::new();
    for (at, session, quantity) in [
        (100.0, "a", 900_000),
        (150.0, "b", 500_000),
        (200.0, "a", 900_000),
        (250.0, "b", 400_000),
        (300.0, "a", 900_000),
    ] {
        ids.push(
            record_reading(
                &mut conn,
                &ammo(at, quantity),
                Purpose::Session,
                Some(session),
            )
            .unwrap()
            .id,
        );
    }
    let window = SessionWindow {
        started_at: 90.0,
        ended_at: Some(310.0),
    };
    let own = readings_for_session(&conn, "a", &window).unwrap();
    let read: Vec<ReadingId> = own.readings.iter().map(|stored| stored.header.id).collect();
    assert_eq!(read, vec![ids[0], ids[2], ids[4]]);
    assert_eq!(own.own, Some(0..3));
    let measure = refresh_session_measure(&conn, "a", &window, &Nothing, None, 1.0).unwrap();
    assert_eq!(measure.coverage.reading_count, 3);
    assert!(measure.ammo.is_empty(), "nothing of B's ammo is A's");
    // B's readings bring A's neighbours: the reading stored just before
    // its first (A's) reads its start when close enough.
    let b = readings_for_session(
        &conn,
        "b",
        &SessionWindow {
            started_at: 140.0,
            ended_at: Some(260.0),
        },
    )
    .unwrap();
    let read: Vec<ReadingId> = b.readings.iter().map(|stored| stored.header.id).collect();
    assert_eq!(read, vec![ids[0], ids[1], ids[3], ids[4]]);
    assert_eq!(b.own, Some(1..3));
    // Readings tagged with no session are chosen by the window.
    let untagged = readings_for_session(&conn, "c", &window).unwrap();
    assert_eq!(untagged.own, None);
    assert_eq!(untagged.readings.len(), 5);
}
