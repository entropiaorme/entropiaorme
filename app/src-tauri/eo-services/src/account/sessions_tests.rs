//! Measured sessions: one session's measure, the page of them beside their
//! tracked figures, and what a deleted session leaves behind.

use super::amount::{Amount, Precision};
use super::measure::CoverageLevel;
use super::roles::KnownRoles;
use super::sessions::{
    cache_sealed, measure_to_seal, measured_sessions, release_session, session_measure, AmmoTotal,
    MeasureRead, MEASURED_PAGE_MAX,
};
use super::store::{cached_measure, record_reading, CachedMeasure, Purpose};
use super::test_support::recorded_hunt;
use crate::db::migrated_in_memory;

/// The recorded hunt's span.
const STARTED: f64 = 1_794_754_640.0;
const ENDED: f64 = 1_794_755_350.0;

fn session_row(conn: &rusqlite::Connection, id: &str, started: f64, ended: Option<f64>) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, session_name) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id,
            started,
            ended,
            i64::from(ended.is_none()),
            format!("{id} run")
        ],
    )
    .unwrap();
}

/// The recorded hunt stored as the connection stores it: the readings
/// before the session untagged, the rest tagged with it.
fn store_the_hunt(conn: &mut rusqlite::Connection, session: &str) {
    session_row(conn, session, STARTED, Some(ENDED));
    for (index, reading) in recorded_hunt().iter().enumerate() {
        let tag = (index >= 3).then_some(session);
        record_reading(conn, reading, Purpose::Session, tag).unwrap();
    }
}

fn summary_row(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO session_summaries \
             (session_id, started_at, ended_at, duration_hours, kills, loot_tt, weapon_cost, \
              enhancer_cost, armour_cost, heal_cost, dangling_cost, cycled_ped, \
              regular_skill_ped_json, attribute_levels_json, regular_skill_tt, \
              attribute_levels_total) \
         VALUES (?1, ?2, ?3, 0.2, 12, 37.5, 66.0, 0.0, 1.2, 0.3, 0.0, 67.5, '{}', '{}', 0, 0)",
        rusqlite::params![id, STARTED, ENDED],
    )
    .unwrap();
    // The tracked facts that summary sums: the weapon's 66.00 on one kill
    // looting 37.50 and 0.50 fired after it, armour 1.20 and healing 0.30.
    conn.execute(
        "UPDATE tracking_sessions SET armour_cost = 1.2, heal_cost = 0.3, dangling_cost = 0.5 \
         WHERE id = ?1",
        [id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, loot_total_ped) \
         VALUES (?1 || '-k', ?1, 'Atrox', ?2, 66.0, 37.5)",
        rusqlite::params![id, STARTED + 1.0],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
         VALUES (?1 || '-k', 'Rifle', 132, 0.5)",
        [id],
    )
    .unwrap();
}

fn cached_rows(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM account_session_measures", [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn a_measure_is_read_only_for_a_known_session_with_readings() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt");
    session_row(&conn, "quiet", ENDED + 600.0, Some(ENDED + 900.0));
    let roles = KnownRoles::default();
    assert_eq!(
        session_measure(&conn, "absent", &roles, None, 1.0, None).unwrap(),
        (MeasureRead::UnknownSession, None)
    );
    assert_eq!(
        session_measure(&conn, "quiet", &roles, None, 1.0, None).unwrap(),
        (MeasureRead::NoReadings, None)
    );
    // Still being read (waiting for its closing readings): computed, not
    // sealed, and nothing to cache.
    let (read, to_seal) = session_measure(&conn, "hunt", &roles, None, 1.0, Some("hunt")).unwrap();
    let MeasureRead::Measured { measure, sealed } = read else {
        panic!("the hunt has readings");
    };
    assert!(!sealed && to_seal.is_none());
    assert_eq!(measure.coverage.level, CoverageLevel::Complete);
    assert_eq!(cached_rows(&conn), 0, "a read caches nothing itself");
    // Ended and no longer read (the app closed during the wait): sealed
    // then, handed back to cache, and served from the cache after.
    let (read, to_seal) = session_measure(&conn, "hunt", &roles, None, 2.0, None).unwrap();
    assert!(matches!(read, MeasureRead::Measured { sealed: true, .. }));
    let to_seal = to_seal.expect("a seal to cache");
    assert_eq!(to_seal.sealed_at, 2.0);
    cache_sealed(&mut conn, &[to_seal], 2.0).unwrap();
    assert!(matches!(
        cached_measure(&conn, "hunt").unwrap(),
        CachedMeasure::Current {
            sealed_at: Some(2.0),
            ..
        }
    ));
    let (read, to_seal) = session_measure(&conn, "hunt", &roles, None, 3.0, None).unwrap();
    assert!(matches!(read, MeasureRead::Measured { sealed: true, .. }));
    assert!(
        to_seal.is_none(),
        "a sealed measure is served, not recomputed"
    );
}

#[test]
fn a_running_session_is_computed_on_every_read_and_never_cached() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "live", STARTED, None);
    for reading in recorded_hunt().iter().skip(3).take(4) {
        record_reading(&mut conn, reading, Purpose::Session, Some("live")).unwrap();
    }
    let (read, to_seal) =
        session_measure(&conn, "live", &KnownRoles::default(), None, 1.0, None).unwrap();
    let MeasureRead::Measured { measure, sealed } = read else {
        panic!("the session has readings");
    };
    assert!(!sealed && to_seal.is_none());
    assert_eq!(measure.coverage.level, CoverageLevel::Partial);
    assert!(measure_to_seal(&conn, "live", None, None, 1.0)
        .unwrap()
        .is_none());
    assert_eq!(cached_rows(&conn), 0);
}

#[test]
fn a_session_s_own_baseline_reads_its_start_and_a_connect_reading_does_not() {
    let readings = recorded_hunt();
    // The session starts just before the reading at index 3; nothing
    // before it is close enough to read the start.
    let started = readings[3].taken_at() - 2.0;
    let ended = readings[6].taken_at() + 5.0;
    for (purpose, expected) in [
        (Purpose::Baseline, CoverageLevel::Complete),
        (Purpose::Connect, CoverageLevel::Partial),
    ] {
        let mut conn = migrated_in_memory().unwrap();
        session_row(&conn, "s", started, Some(ended));
        for (index, reading) in readings.iter().enumerate().skip(3).take(5) {
            let purpose = if index == 3 {
                purpose
            } else {
                Purpose::Session
            };
            record_reading(&mut conn, reading, purpose, Some("s")).unwrap();
        }
        let (read, _) =
            session_measure(&conn, "s", &KnownRoles::default(), None, 1.0, None).unwrap();
        let MeasureRead::Measured { measure, .. } = read else {
            panic!("the session has readings");
        };
        assert_eq!(measure.coverage.level, expected, "{purpose:?}");
    }
}

#[test]
fn measured_sessions_page_newest_first_beside_their_tracked_figures() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt");
    summary_row(&conn, "hunt");
    conn.execute(
        "UPDATE tracking_sessions SET economy_excluded_at = 5.0, exclusion_note = 'test' \
         WHERE id = 'hunt'",
        [],
    )
    .unwrap();
    // A later session read at its start, still running; one with no
    // readings at all is not listed.
    session_row(&conn, "later", ENDED + 3_600.0, None);
    let last = recorded_hunt().pop().unwrap();
    let later =
        super::Reading::new(ENDED + 3_600.0, last.balance(), last.holdings().clone()).unwrap();
    record_reading(&mut conn, &later, Purpose::Session, Some("later")).unwrap();
    session_row(&conn, "unread", ENDED + 7_200.0, Some(ENDED + 7_300.0));

    let (first, sealed) = measured_sessions(&conn, None, Some(1), None, None, 10.0, None).unwrap();
    assert!(sealed.is_empty(), "a running session is never cached");
    assert_eq!(first.total, 2);
    assert_eq!(first.sessions.len(), 1);
    assert_eq!(first.sessions[0].id, "later");
    assert!(first.sessions[0].tracked.is_none());
    assert!(!first.sessions[0].sealed);
    let next = first.next.expect("another page follows");

    let (second, sealed) =
        measured_sessions(&conn, Some(&next), Some(1), None, None, 10.0, None).unwrap();
    assert!(second.next.is_none());
    assert_eq!(sealed.len(), 1, "the ended hunt is sealed by this read");
    cache_sealed(&mut conn, &sealed, 10.0).unwrap();
    let hunt = &second.sessions[0];
    assert_eq!(hunt.id, "hunt");
    assert_eq!(hunt.name.as_deref(), Some("hunt run"));
    assert_eq!((hunt.started_at, hunt.ended_at), (STARTED, Some(ENDED)));
    assert_eq!(hunt.coverage, CoverageLevel::Complete);
    assert_eq!((hunt.reading_count, hunt.gap_count), (13, 0));
    assert!(hunt.sealed);
    assert_eq!(hunt.exclusion.economy_excluded_at, Some(5.0));
    assert_eq!(
        hunt.totals.ammo,
        AmmoTotal::Settled(Amount::exact(66_028_800))
    );
    assert_eq!(hunt.totals.gained.micro_ped(), 36_318_400 + 1_600_000);
    assert_eq!(hunt.totals.gained_possibly_strongbox.micro_ped(), 0);
    assert_eq!(hunt.totals.other_consumption, Amount::exact(90_000));
    assert_eq!(hunt.totals.repairs.unknown, 0);
    assert_eq!(hunt.totals.repairs.known.precision(), Precision::WholePec);
    let tracked = hunt.tracked.expect("the hunt has a summary");
    assert_eq!(
        (
            tracked.weapon_cost,
            tracked.heal_cost,
            tracked.armour_cost,
            tracked.loot_tt,
            tracked.cycled
        ),
        // The weapon is every shot fired, as the measured weapon counts it.
        (66.5, 0.3, 1.2, 37.5, 68.0)
    );
    // Read again with the hunt cached: nothing to write.
    let (_, sealed) = measured_sessions(&conn, None, None, None, None, 11.0, None).unwrap();
    assert!(sealed.is_empty());
}

#[test]
fn measured_sessions_order_by_start_with_the_id_only_splitting_ties() {
    let mut conn = migrated_in_memory().unwrap();
    let reading = recorded_hunt().remove(0);
    // Ids that sort against the start times, and two sessions sharing one.
    for (id, started) in [("a", 300.0), ("b", 200.0), ("c", 200.0), ("z", 100.0)] {
        session_row(&conn, id, started, Some(started + 50.0));
        record_reading(&mut conn, &reading, Purpose::Session, Some(id)).unwrap();
    }
    let mut seen = Vec::new();
    let mut seek = None;
    loop {
        let (page, _) =
            measured_sessions(&conn, seek.as_ref(), Some(1), None, None, 1.0, None).unwrap();
        seen.extend(page.sessions.iter().map(|session| session.id.clone()));
        match page.next {
            Some(next) => seek = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, ["a", "c", "b", "z"]);
    // A page is never larger than the cap, whatever is asked for.
    let (page, _) = measured_sessions(&conn, None, Some(1_000), None, None, 1.0, None).unwrap();
    assert_eq!(page.sessions.len(), 4);
    assert_eq!(MEASURED_PAGE_MAX, 50);
}

#[test]
fn a_page_is_capped_at_fifty() {
    let mut conn = migrated_in_memory().unwrap();
    let reading = recorded_hunt().remove(0);
    for index in 0..60 {
        let id = format!("s{index:02}");
        session_row(&conn, &id, f64::from(index), None);
        record_reading(&mut conn, &reading, Purpose::Session, Some(&id)).unwrap();
    }
    let (page, _) = measured_sessions(&conn, None, Some(500), None, None, 1.0, None).unwrap();
    assert_eq!(page.sessions.len(), 50);
    assert!(page.next.is_some());
    assert_eq!(page.total, 60);
}

#[tokio::test]
async fn deleting_a_session_keeps_its_readings_untagged_and_drops_its_measure() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(&dir.path().join("account.db"))
        .await
        .unwrap();
    db.with_writer(|conn| {
        store_the_hunt(conn, "hunt");
        let seal = measure_to_seal(conn, "hunt", None, None, 1.0)
            .unwrap()
            .expect("an ended hunt with readings");
        cache_sealed(conn, &[seal], 1.0).unwrap();
        Ok(())
    })
    .await
    .unwrap();
    crate::session_delete::delete_session_impl(&db, "hunt")
        .await
        .unwrap();
    let (readings, tagged, measures): (i64, i64, i64) = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT (SELECT COUNT(*) FROM account_readings), \
                        (SELECT COUNT(*) FROM account_readings WHERE session_id IS NOT NULL), \
                        (SELECT COUNT(*) FROM account_session_measures)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((readings, tagged, measures), (15, 0, 0));
}

#[test]
fn releasing_a_session_touches_no_other() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt");
    session_row(&conn, "other", 1.0, Some(2.0));
    let reading = recorded_hunt().remove(0);
    record_reading(&mut conn, &reading, Purpose::Session, Some("other")).unwrap();
    release_session(&conn, "other").unwrap();
    let tagged: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM account_readings WHERE session_id = 'hunt'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tagged, 12);
}

/// The hunt computed for sealing, as a read hands it back.
fn hunt_to_seal(conn: &mut rusqlite::Connection) -> super::sessions::ToSeal {
    store_the_hunt(conn, "hunt");
    measure_to_seal(conn, "hunt", None, None, 1.0)
        .unwrap()
        .expect("an ended hunt with readings")
}

#[test]
fn a_seal_is_not_written_once_a_reading_was_stored_after_it_was_computed() {
    let mut conn = migrated_in_memory().unwrap();
    let seal = hunt_to_seal(&mut conn);
    let late = recorded_hunt().pop().unwrap();
    record_reading(&mut conn, &late, Purpose::Session, Some("hunt")).unwrap();
    assert_eq!(cache_sealed(&mut conn, &[seal], 2.0).unwrap(), 0);
    assert_eq!(cached_rows(&conn), 0);
    // The next read computes again, over every reading.
    let (_, to_seal) =
        session_measure(&conn, "hunt", &KnownRoles::default(), None, 3.0, None).unwrap();
    assert_eq!(
        cache_sealed(&mut conn, &[to_seal.unwrap()], 3.0).unwrap(),
        1
    );
}

#[test]
fn a_seal_is_not_written_for_a_session_deleted_after_it_was_computed() {
    let mut conn = migrated_in_memory().unwrap();
    let seal = hunt_to_seal(&mut conn);
    release_session(&conn, "hunt").unwrap();
    conn.execute("DELETE FROM tracking_sessions WHERE id = 'hunt'", [])
        .unwrap();
    assert_eq!(cache_sealed(&mut conn, &[seal], 2.0).unwrap(), 0);
    assert_eq!(cached_rows(&conn), 0, "no row is put back");
}

#[test]
fn a_late_seal_never_replaces_one_computed_from_more_readings() {
    let mut conn = migrated_in_memory().unwrap();
    // A read computes a seal while the connection's closing readings are
    // still to come.
    session_row(&conn, "hunt", STARTED, Some(ENDED));
    let readings = recorded_hunt();
    for reading in readings.iter().take(10) {
        record_reading(&mut conn, reading, Purpose::Session, Some("hunt")).unwrap();
    }
    let early = measure_to_seal(&conn, "hunt", None, None, 1.0)
        .unwrap()
        .unwrap();
    for reading in readings.iter().skip(10) {
        record_reading(&mut conn, reading, Purpose::Session, Some("hunt")).unwrap();
    }
    // The connection's own seal, over every reading, lands first.
    let full = measure_to_seal(&conn, "hunt", None, None, 2.0)
        .unwrap()
        .unwrap();
    assert_eq!(cache_sealed(&mut conn, &[full], 2.0).unwrap(), 1);
    let sealed_count = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row(
            "SELECT reading_count FROM account_session_measures WHERE session_id = 'hunt'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    let before = sealed_count(&conn);
    // The late write is refused: its readings are not the session's now.
    assert_eq!(
        cache_sealed(&mut conn, std::slice::from_ref(&early), 3.0).unwrap(),
        0
    );
    assert_eq!(sealed_count(&conn), before);
    // Even with the reading count put back as it was, a sealed measure of
    // more readings is not replaced by one of fewer.
    conn.execute(
        // id-order: insertion (the five stored last)
        "DELETE FROM account_readings WHERE id IN \
         (SELECT id FROM account_readings ORDER BY id DESC LIMIT 5)",
        [],
    )
    .unwrap();
    assert_eq!(cache_sealed(&mut conn, &[early], 4.0).unwrap(), 0);
    assert_eq!(sealed_count(&conn), before);
}

#[test]
fn a_topped_up_measure_totals_its_ammo_as_topped_up_and_never_as_settled() {
    use super::measure::{measure_session, SessionWindow};
    use super::reading::Container;
    use super::sessions::MeasureTotals;
    use super::test_support::reading;
    const C: Container = Container::Carried;
    // 10 000 fired, a top-up minute, then 15 000 fired.
    let readings = [
        reading(0.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
        reading(60.0, 0, &[("Universal Ammo", C, 90_000, 900)]),
        reading(120.0, 0, &[("Universal Ammo", C, 95_000, 950)]),
        reading(180.0, 0, &[("Universal Ammo", C, 80_000, 800)]),
    ];
    let window = SessionWindow {
        started_at: 0.0,
        ended_at: Some(180.0),
    };
    let roles = KnownRoles::from_parts(&[], None);
    let measure = measure_session(&window, &readings, &roles).unwrap().measure;
    let totals = MeasureTotals::of(&measure).unwrap();
    assert_eq!(
        totals.ammo,
        AmmoTotal::ToppedUp {
            burned: Amount::exact(2_500_000),
            steps: 1
        }
    );
    // The same minutes with nothing topped up total as settled.
    let readings = [
        reading(0.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
        reading(60.0, 0, &[("Universal Ammo", C, 90_000, 900)]),
        reading(120.0, 0, &[("Universal Ammo", C, 85_000, 850)]),
    ];
    let measure = measure_session(&window, &readings, &roles).unwrap().measure;
    assert_eq!(
        MeasureTotals::of(&measure).unwrap().ammo,
        AmmoTotal::Settled(Amount::exact(1_500_000))
    );
}

#[test]
fn the_measured_summary_leaves_a_topped_up_weapon_out_of_its_weapon_pair() {
    use super::reading::Container;
    use super::sessions::measured_summary;
    use super::test_support::reading;
    const C: Container = Container::Carried;
    let mut conn = migrated_in_memory().unwrap();
    // Two sessions read whole, one after the other, each a kill the chat
    // log priced at 1.50: one fires steadily, the other has ammo bought
    // mid-session.
    for (id, middle, from) in [("steady", 85_000, 0.0), ("topped", 95_000, 1_000.0)] {
        session_row(&conn, id, from + 1_000.0, Some(from + 1_300.0));
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, loot_total_ped) \
             VALUES (?1 || '-k', ?1, 'Atrox', ?2, 1.5, 0.5)",
            rusqlite::params![id, from + 1_100.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES (?1 || '-k', 'Opalo', 1, 1.5)",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_summaries \
                 (session_id, started_at, ended_at, duration_hours, kills, loot_tt, weapon_cost, \
                  enhancer_cost, armour_cost, heal_cost, dangling_cost, cycled_ped, \
                  regular_skill_ped_json, attribute_levels_json, regular_skill_tt, \
                  attribute_levels_total) \
             VALUES (?1, ?2, ?3, 0.1, 1, 0.5, 1.5, 0, 0, 0, 0, 1.5, '{}', '{}', 0, 0)",
            rusqlite::params![id, from + 1_000.0, from + 1_300.0],
        )
        .unwrap();
        let readings = [
            reading(from + 970.0, 0, &[("Universal Ammo", C, 100_000, 1_000)]),
            reading(from + 1_060.0, 0, &[("Universal Ammo", C, 90_000, 900)]),
            reading(
                from + 1_180.0,
                0,
                &[("Universal Ammo", C, middle, middle / 100)],
            ),
            reading(from + 1_330.0, 0, &[("Universal Ammo", C, 80_000, 800)]),
        ];
        for reading in &readings {
            record_reading(&mut conn, reading, Purpose::Session, Some(id)).unwrap();
        }
        let seal = measure_to_seal(&conn, id, None, None, 5_000.0)
            .unwrap()
            .expect("an ended session with readings");
        assert_eq!(
            seal.computed.measure.coverage.level,
            CoverageLevel::Complete
        );
        cache_sealed(&mut conn, &[seal], 5_000.0).unwrap();
    }
    let summary = measured_summary(&conn, &std::collections::HashSet::new()).unwrap();
    assert_eq!(summary.included, 2);
    // Only the steady session's weapon is set beside its tracked figure:
    // the topped-up one has no whole-session ammo figure.
    assert_eq!(summary.weapon.sessions, 1);
    assert!((summary.weapon.tracked - 1.5).abs() < 1e-9);
    assert_eq!(summary.weapon.measured, Amount::exact(2_000_000));
    // Its other figures still count.
    assert_eq!(summary.loot.sessions, 2);
}
