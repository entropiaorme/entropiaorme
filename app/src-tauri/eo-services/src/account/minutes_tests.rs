//! A measured session minute by minute: loot matched item by item against
//! what arrived, the weapon lane's stretches, and the handles an arrival
//! and a missing loot line carry.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::minutes::{
    read_minutes, stretches, LootCorrection, LootSource, NoCorrection, SessionMinutes, StepGap,
};
use super::proposals::{
    claim, dismiss, link, open_arrival, pending, restore_arrival, session_arrivals, sync_arrivals,
    ArrivalDecision, Decided, NotProposed, ProposalId, ProposalKind, RecordedAs,
};
use super::reading::{Container, Reading};
use super::roles::KnownRoles;
use super::store::{record_reading, Purpose};
use super::test_support::reading;
use crate::analytics::{AnalyticsService, UnknownOriginUndo};
use crate::clock::MockClock;
use crate::db::{migrated_in_memory, Db};

const C: Container = Container::Carried;
const PEC: i64 = 10_000;
const SESSION: &str = "hunt";

/// A sealed session from 1 to 175, with its readings a minute apart from
/// 0: the first reads its start, the last its stop.
fn sealed_session(conn: &mut rusqlite::Connection, readings: &[Reading]) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, 1.0, 175.0, 0)",
        [SESSION],
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

/// A loot line of `kill`; its row's id.
fn loot(conn: &rusqlite::Connection, kill: &str, item: &str, quantity: i64, value: f64) -> i64 {
    conn.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![kill, item, quantity, value],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn minutes(conn: &rusqlite::Connection) -> SessionMinutes {
    read_minutes(conn, SESSION, &KnownRoles::default())
        .unwrap()
        .expect("a known session")
}

/// Animal Oil Residue held throughout, and in the second minute 0.45 of it
/// arriving beside 5.00 of Lysterium Ingot: the readings show +5.45, the
/// chat log looted +0.45.
fn five_forty_five() -> Vec<Reading> {
    let oil = |units: i64| ("Animal Oil Residue", C, units, units);
    vec![
        reading(0.0, 0, &[oil(100)]),
        reading(60.0, 0, &[oil(100)]),
        reading(120.0, 0, &[oil(145), ("Lysterium Ingot", C, 50, 500)]),
        reading(180.0, 0, &[oil(145), ("Lysterium Ingot", C, 50, 500)]),
    ]
}

#[test]
fn five_ped_arriving_beside_forty_five_pec_looted_flags_the_five_ped_alone() {
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &five_forty_five());
    kill(&conn, "k1", 90.0, 1.25);
    loot(&conn, "k1", "Animal Oil Residue", 45, 0.45);
    let found = minutes(&conn);
    assert_eq!(found.steps.len(), 3);
    assert!(found.coverage.is_complete());
    let minute = &found.steps[1];
    assert!((minute.looted - 0.45).abs() < 1e-9);
    assert_eq!(minute.arrived.micro_ped(), 5_450_000);
    assert_eq!(minute.arrived_not_looted.len(), 1, "{minute:?}");
    let flagged = &minute.arrived_not_looted[0];
    assert_eq!(flagged.key.name, "Lysterium Ingot");
    assert_eq!(flagged.units, 50);
    assert_eq!(flagged.amount.micro_ped(), 5_000_000);
    assert!(!flagged.bought_at_tt);
    assert_eq!(
        flagged.decision,
        ArrivalDecision::NotProposed(NotProposed::Awaiting)
    );
    // The kill's spend is the weapon's tracked figure in the minute it ended.
    assert!((minute.weapon.tracked - 1.25).abs() < 1e-9);
    assert_eq!(found.steps[0].weapon.tracked, 0.0);
    for step in &found.steps {
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
    assert!(found.steps[0].arrived_not_looted.is_empty());
    assert!(found.steps[2].arrived_not_looted.is_empty());
}

#[test]
fn a_loot_line_that_arrives_in_the_next_step_is_matched_there() {
    let oil = |units: i64| ("Animal Oil Residue", C, units, units);
    let readings = vec![
        reading(0.0, 0, &[oil(100)]),
        reading(60.0, 0, &[oil(100)]),
        reading(120.0, 0, &[oil(100)]),
        reading(180.0, 0, &[oil(145)]),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    kill(&conn, "k1", 100.0, 1.0);
    loot(&conn, "k1", "Animal Oil Residue", 45, 0.45);
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
    assert_eq!(found.steps[2].arrived.micro_ped(), 450_000);
}

/// Iron Stone looted in the first minute that never arrives, beside oil
/// that does.
fn iron_never_arrives(conn: &mut rusqlite::Connection) -> (i64, i64) {
    let oil = |units: i64| ("Animal Oil Residue", C, units, units);
    sealed_session(
        conn,
        &[
            reading(0.0, 0, &[oil(100)]),
            reading(60.0, 0, &[oil(120)]),
            reading(120.0, 0, &[oil(120)]),
            reading(180.0, 0, &[oil(120)]),
        ],
    );
    kill(conn, "k1", 30.0, 1.0);
    let oil_line = loot(conn, "k1", "Animal Oil Residue", 20, 0.20);
    let iron_line = loot(conn, "k1", "Iron Stone", 10, 0.30);
    (oil_line, iron_line)
}

#[test]
fn loot_that_never_arrives_is_flagged_only_once_the_next_step_has_passed() {
    let mut conn = migrated_in_memory().unwrap();
    let (_, iron_line) = iron_never_arrives(&mut conn);
    let found = minutes(&conn);
    assert!(
        found.steps[0].looted_never_arrived.is_empty(),
        "its own step may still be catching up"
    );
    let missing = &found.steps[1].looted_never_arrived;
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert_eq!(missing[0].item, "Iron Stone");
    assert_eq!(missing[0].units, 10);
    assert!((missing[0].value - 0.30).abs() < 1e-9);
    assert_eq!(missing[0].looted_in, found.steps[0].between);
    assert_eq!(missing[0].lines, vec![(LootSource::Kill, iron_line)]);
    assert_eq!(
        missing[0].correction,
        LootCorrection::Offered {
            session_id: SESSION.to_string(),
            item_name: "Iron Stone".to_string(),
        }
    );
    assert!(found.steps[2].looted_never_arrived.is_empty());
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
    }
}

#[test]
fn the_correction_is_not_offered_when_it_would_take_lines_that_arrived() {
    let mut conn = migrated_in_memory().unwrap();
    iron_never_arrives(&mut conn);
    // A second line of Iron Stone, in the last minute, which arrived.
    let readings_after = [reading(
        240.0,
        0,
        &[
            ("Animal Oil Residue", C, 120, 120),
            ("Iron Stone", C, 5, 15),
        ],
    )];
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 235.0 WHERE id = ?1",
        [SESSION],
    )
    .unwrap();
    for reading in &readings_after {
        record_reading(&mut conn, reading, Purpose::Session, Some(SESSION)).unwrap();
    }
    kill(&conn, "k2", 200.0, 1.0);
    loot(&conn, "k2", "Iron Stone", 5, 0.15);
    let found = minutes(&conn);
    let missing = &found.steps[1].looted_never_arrived;
    assert_eq!(missing.len(), 1);
    assert_eq!(
        missing[0].correction,
        LootCorrection::NotOffered(NoCorrection::OtherLinesArrived)
    );
}

#[test]
fn shrapnel_looted_and_converted_in_the_same_step_is_accounted_for() {
    let readings = vec![
        reading(
            0.0,
            0,
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        ),
        reading(
            60.0,
            0,
            &[
                ("Shrapnel", C, 10_000, 100),
                ("Universal Ammo", C, 100_000, 1_000),
            ],
        ),
        // The 2,000 looted joined the stack and all 12,000 became ammo.
        reading(
            120.0,
            0,
            &[
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 12_120, 121),
            ],
        ),
        reading(
            180.0,
            0,
            &[
                ("Universal Ammo", C, 100_000, 1_000),
                ("Universal Ammo", C, 12_120, 121),
            ],
        ),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    kill(&conn, "k1", 90.0, 1.0);
    loot(&conn, "k1", "Shrapnel", 2_000, 0.20);
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
}

#[test]
fn a_strongbox_opening_s_contents_are_never_flagged() {
    let before = [
        ("Halloween Strongbox", C, 1, 0),
        ("Strongbox Key", C, 1, 1_000),
    ];
    let after = [
        ("Universal Ammo", C, 50_000, 500),
        ("Vibrant Sweat", C, 500, 500),
    ];
    let readings = vec![
        reading(0.0, 0, &before),
        reading(60.0, 0, &before),
        reading(120.0, 0, &after),
        reading(180.0, 0, &after),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
}

#[test]
fn a_recorded_quest_reward_arriving_mid_session_is_not_flagged() {
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &five_forty_five());
    kill(&conn, "k1", 90.0, 1.0);
    loot(&conn, "k1", "Animal Oil Residue", 45, 0.45);
    conn.execute(
        "INSERT INTO session_quest_completions (id, session_id, quest_id, completed_at) \
         VALUES (1, ?1, 7, 100.0)",
        [SESSION],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_quest_completion_reward_items \
             (completion_id, item_name, quantity, value_ped) \
         VALUES (1, 'Lysterium Ingot', 50, 5.0)",
        [],
    )
    .unwrap();
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
    }
}

#[test]
fn the_last_step_of_a_running_session_offers_no_arrival_yet() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, 1.0, NULL, 1)",
        [SESSION],
    )
    .unwrap();
    for reading in &five_forty_five()[..3] {
        record_reading(&mut conn, reading, Purpose::Session, Some(SESSION)).unwrap();
    }
    let found = minutes(&conn);
    assert!(found.running);
    assert!(!found.coverage.is_complete());
    let last = found.steps.last().expect("steps");
    assert!(!last.settled);
    assert_eq!(last.arrived_not_looted.len(), 2, "{last:?}");
    for arrival in &last.arrived_not_looted {
        assert_eq!(
            arrival.decision,
            ArrivalDecision::NotProposed(NotProposed::Settling)
        );
    }
    assert!(found.to_propose().is_empty());
    assert_eq!(
        last.arrived.micro_ped(),
        5_450_000,
        "what arrived still shows"
    );
}

/// A step's part in the weapon's gap: measured less tracked, in PEC, with
/// how many kills it ends and the largest one's spend, in PEC.
fn step(residual: i64, kills: usize, largest: i64) -> StepGap {
    StepGap {
        residual: Some(residual * PEC),
        truncated: BTreeSet::new(),
        events: kills,
        largest_event: largest * PEC,
    }
}

/// Thirty minutes, a kill of 80 PEC landing in each, measured as tracked.
fn steady() -> Vec<StepGap> {
    (0..30).map(|_| step(0, 1, 80)).collect()
}

#[test]
fn a_ten_minute_wrong_cost_stretch_is_flagged_and_nothing_else() {
    // From the tenth minute to the nineteenth the tracked cost runs 20 PEC
    // a minute below what was fired.
    let mut steps = steady();
    for gap in &mut steps[10..20] {
        *gap = step(20, 1, 80);
    }
    assert_eq!(stretches(&steps), vec![(10, 19, 200 * PEC)]);
}

#[test]
fn a_fight_across_three_readings_is_no_stretch() {
    // Its spend measured over three minutes with no kill, tracked whole in
    // the fourth, where the kill landed.
    let mut steps = steady();
    steps[5] = step(40, 0, 0);
    steps[6] = step(40, 0, 0);
    steps[7] = step(40, 0, 0);
    steps[8] = step(-120, 1, 120);
    assert!(stretches(&steps).is_empty());
}

#[test]
fn a_session_of_two_minute_kills_is_no_stretch() {
    let steps: Vec<StepGap> = (0..30)
        .map(|k| {
            if k % 2 == 0 {
                step(50, 0, 0)
            } else {
                step(-50, 1, 100)
            }
        })
        .collect();
    assert!(stretches(&steps).is_empty());
}

#[test]
fn a_step_left_unmeasured_mid_fight_is_no_stretch() {
    let mut steps = steady();
    steps[5] = step(60, 0, 0);
    steps[6].residual = None;
    steps[7] = step(-60, 1, 120);
    assert!(stretches(&steps).is_empty());
}

#[test]
fn a_steady_bias_all_session_is_no_stretch() {
    let steps: Vec<StepGap> = (0..30).map(|_| step(3, 1, 80)).collect();
    assert!(stretches(&steps).is_empty());
}

#[test]
fn loot_used_in_the_step_it_was_looted_in_is_never_flagged_missing() {
    let oil = |units: i64| ("Animal Oil Residue", C, units, units);
    let readings = vec![
        reading(0.0, 0, &[oil(100)]),
        reading(60.0, 0, &[oil(100)]),
        // 45 looted and 65 used: the stack fell.
        reading(120.0, 0, &[oil(80)]),
        reading(180.0, 0, &[oil(80)]),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    kill(&conn, "k1", 90.0, 1.0);
    loot(&conn, "k1", "Animal Oil Residue", 45, 0.45);
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
    assert!((found.loot_unconfirmed - 0.45).abs() < 1e-9);
}

#[test]
fn an_enhancer_s_returned_shrapnel_explains_shrapnel_a_step_before() {
    let shrapnel = |units: i64| ("Shrapnel", C, units, units / 100);
    let readings = vec![
        reading(0.0, 0, &[shrapnel(1_000)]),
        reading(60.0, 0, &[shrapnel(1_000)]),
        reading(120.0, 0, &[shrapnel(1_800)]),
        reading(180.0, 0, &[shrapnel(1_800)]),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    // The break's return is recorded with the kill after it, a step later.
    kill(&conn, "k1", 150.0, 1.0);
    conn.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
                                      is_enhancer_shrapnel) \
         VALUES ('k1', 'Shrapnel', 800, 0.08, 1)",
        [],
    )
    .unwrap();
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
    assert_eq!(found.steps[1].looted, 0.0, "a return is not loot");
}

#[test]
fn what_arrives_in_a_step_wholly_after_the_session_is_not_offered() {
    let mut readings = five_forty_five();
    readings[2] = reading(120.0, 0, &[("Animal Oil Residue", C, 100, 100)]);
    readings[3] = reading(
        180.0,
        0,
        &[
            ("Animal Oil Residue", C, 100, 100),
            ("Lysterium Ingot", C, 50, 500),
        ],
    );
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 100.0 WHERE id = ?1",
        [SESSION],
    )
    .unwrap();
    let found = minutes(&conn);
    let last = found.steps.last().expect("steps");
    assert_eq!(last.arrived_not_looted.len(), 1, "{found:?}");
    assert_eq!(
        last.arrived_not_looted[0].decision,
        ArrivalDecision::NotProposed(NotProposed::OutsideSession)
    );
    assert!(found.to_propose().is_empty());
}

#[test]
fn loot_from_the_session_before_within_the_site_s_lag_explains_an_arrival() {
    let oil = |units: i64| ("Animal Oil Residue", C, units, units);
    let readings = vec![
        reading(0.0, 0, &[oil(100)]),
        reading(60.0, 0, &[oil(145)]),
        reading(120.0, 0, &[oil(145)]),
    ];
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES ('before', -600.0, -20.0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, loot_total_ped) \
         VALUES ('k0', 'before', 'Atrox', -30.0, 1.0, 0.45)",
        [],
    )
    .unwrap();
    loot(&conn, "k0", "Animal Oil Residue", 45, 0.45);
    sealed_session(&mut conn, &readings);
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 115.0 WHERE id = ?1",
        [SESSION],
    )
    .unwrap();
    let found = minutes(&conn);
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
        assert_eq!(step.looted, 0.0, "not this session's loot");
    }
}

#[test]
fn what_may_be_an_opening_s_contents_is_never_flagged() {
    // Boxes with no keys: an opening that cannot settle, so whatever
    // arrived beside it may be its contents.
    let readings = vec![
        reading(0.0, 0, &[("Halloween Strongbox", C, 1, 0)]),
        reading(60.0, 0, &[("Halloween Strongbox", C, 1, 0)]),
        reading(120.0, 0, &[("Vibrant Sweat", C, 500, 500)]),
        reading(180.0, 0, &[("Vibrant Sweat", C, 500, 500)]),
    ];
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &readings);
    let found = minutes(&conn);
    assert_eq!(found.steps[1].arrived.micro_ped(), 5_000_000, "it arrived");
    for step in &found.steps {
        assert!(step.arrived_not_looted.is_empty(), "{step:?}");
    }
}

#[test]
fn a_quest_reward_explains_only_what_arrived_from_its_completion() {
    let mut conn = migrated_in_memory().unwrap();
    sealed_session(&mut conn, &five_forty_five());
    kill(&conn, "k1", 90.0, 1.0);
    loot(&conn, "k1", "Animal Oil Residue", 45, 0.45);
    // Completed a step after the Lysterium arrived.
    conn.execute(
        "INSERT INTO session_quest_completions (id, session_id, quest_id, completed_at) \
         VALUES (1, ?1, 7, 170.0)",
        [SESSION],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_quest_completion_reward_items \
             (completion_id, item_name, quantity, value_ped) \
         VALUES (1, 'Lysterium Ingot', 50, 5.0)",
        [],
    )
    .unwrap();
    let found = minutes(&conn);
    assert_eq!(found.steps[1].arrived_not_looted.len(), 1);
}

async fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    (dir, db)
}

async fn read(db: &Db) -> SessionMinutes {
    db.with_reader(|conn| Ok(minutes(conn))).await.unwrap()
}

/// Read the session's minutes and store their arrivals as proposals, as
/// the connection does; how many were added.
async fn propose(db: &Db) -> usize {
    let found = read(db).await;
    let seen = found.to_propose();
    let withdraw = found.withdraw.clone();
    db.with_writer(move |conn| Ok(sync_arrivals(conn, SESSION, &seen, &withdraw, 500.0).unwrap()))
        .await
        .unwrap()
}

fn only_arrival(found: &SessionMinutes) -> ArrivalDecision {
    found.steps[1].arrived_not_looted[0].decision.clone()
}

fn proposal_of(decision: &ArrivalDecision) -> ProposalId {
    match decision {
        ArrivalDecision::Open { proposal, .. }
        | ArrivalDecision::Recorded { proposal, .. }
        | ArrivalDecision::Ignored { proposal } => *proposal,
        ArrivalDecision::NotProposed(reason) => panic!("not proposed: {reason:?}"),
    }
}

async fn seeded() -> (tempfile::TempDir, Db) {
    let (dir, db) = open().await;
    db.with_writer(|conn| {
        sealed_session(conn, &five_forty_five());
        kill(conn, "k1", 90.0, 1.0);
        loot(conn, "k1", "Animal Oil Residue", 45, 0.45);
        Ok(())
    })
    .await
    .unwrap();
    (dir, db)
}

fn analytics(db: &Db) -> AnalyticsService {
    AnalyticsService::new(db.clone(), Arc::new(MockClock::new(None, 0.0)))
}

#[tokio::test]
async fn an_arrival_recorded_as_a_purchase_is_recorded_once() {
    let (_dir, db) = seeded().await;
    assert_eq!(propose(&db).await, 1);
    let decision = only_arrival(&read(&db).await);
    assert!(matches!(
        decision,
        ArrivalDecision::Open {
            possibly_recorded: false,
            ..
        }
    ));
    let id = proposal_of(&decision);
    // Claimed, recorded as an asset purchase with its price, linked.
    let claimed = db
        .with_writer(move |conn| Ok(claim(conn, &[id], &[], 500.0).unwrap()))
        .await
        .unwrap();
    assert_eq!(claimed, Decided::Done);
    let service = analytics(&db);
    let bought = service
        .create_inventory_item("Lysterium Ingot", Some(50.0), 5.0, 1.5, None, None)
        .await
        .unwrap();
    let purchase = bought.purchases[0].id.to_string();
    let record = purchase.clone();
    let linked = db
        .with_writer(move |conn| Ok(link(conn, &[id], &record, 500.0).unwrap()))
        .await
        .unwrap();
    assert_eq!(linked, Decided::Done);
    assert_eq!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Recorded {
            proposal: id,
            record_id: purchase,
            recorded_as: RecordedAs::Purchase,
            asset_name: Some("Lysterium Ingot".to_string()),
        }
    );
    // Never twice: reading again proposes nothing, and a second purchase
    // cannot take it over.
    assert_eq!(propose(&db).await, 0);
    let other = service
        .create_inventory_item("Lysterium Ingot", Some(50.0), 5.0, 1.5, None, None)
        .await
        .unwrap();
    let other = other.purchases[0].id.to_string();
    let again = db
        .with_writer(move |conn| Ok(link(conn, &[id], &other, 600.0).unwrap()))
        .await
        .unwrap();
    assert!(matches!(again, Decided::Refused(_)), "{again:?}");
}

#[tokio::test]
async fn an_ignored_arrival_is_remembered_and_never_offered_again() {
    let (_dir, db) = seeded().await;
    propose(&db).await;
    let id = proposal_of(&only_arrival(&read(&db).await));
    let dismissed = db
        .with_writer(move |conn| Ok(dismiss(conn, id, 500.0).unwrap()))
        .await
        .unwrap();
    assert_eq!(dismissed, Decided::Done);
    assert_eq!(
        propose(&db).await,
        0,
        "the same arrival is not proposed again"
    );
    assert_eq!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Ignored { proposal: id }
    );
    let open = db
        .with_reader(|conn| Ok(pending(conn, Some(ProposalKind::Arrival)).unwrap()))
        .await
        .unwrap();
    assert!(open.is_empty());
    // Taken back: offered again, as it was first proposed.
    let restored = db
        .with_writer(move |conn| Ok(restore_arrival(conn, id).unwrap()))
        .await
        .unwrap();
    assert_eq!(restored, Decided::Done);
    assert_eq!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Open {
            proposal: id,
            possibly_recorded: false,
            unlinked_record: None,
        }
    );
    let open = db
        .with_reader(|conn| Ok(pending(conn, Some(ProposalKind::Arrival)).unwrap()))
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
}

#[tokio::test]
async fn a_claimed_arrival_is_not_dismissed_until_it_is_released() {
    let (_dir, db) = seeded().await;
    propose(&db).await;
    let id = proposal_of(&only_arrival(&read(&db).await));
    let dismissed = db
        .with_writer(move |conn| {
            assert_eq!(claim(conn, &[id], &[], 500.0).unwrap(), Decided::Done);
            Ok(dismiss(conn, id, 500.0).unwrap())
        })
        .await
        .unwrap();
    assert!(matches!(dismissed, Decided::Refused(_)), "{dismissed:?}");
}

#[tokio::test]
async fn a_proposal_whose_flag_is_gone_is_withdrawn() {
    let (_dir, db) = seeded().await;
    assert_eq!(propose(&db).await, 1);
    // The Lysterium turns out to be loot the chat log recorded late.
    db.with_writer(|conn| {
        kill(conn, "k2", 100.0, 1.0);
        loot(conn, "k2", "Lysterium Ingot", 50, 5.0);
        Ok(())
    })
    .await
    .unwrap();
    let found = read(&db).await;
    assert!(found.steps[1].arrived_not_looted.is_empty());
    assert_eq!(found.withdraw.len(), 1);
    assert_eq!(propose(&db).await, 1, "withdrawn");
    let stored = db
        .with_reader(|conn| Ok(session_arrivals(conn, SESSION).unwrap()))
        .await
        .unwrap();
    assert!(stored.is_empty());
}

#[tokio::test]
async fn stock_of_unknown_origin_from_an_arrival_is_taken_back_and_opens_it_again() {
    let (_dir, db) = seeded().await;
    propose(&db).await;
    let id = proposal_of(&only_arrival(&read(&db).await));
    let (arrival, seen_to) = db
        .with_writer(move |conn| {
            let arrival = open_arrival(conn, id).unwrap().expect("open");
            assert_eq!(claim(conn, &[id], &[], 500.0).unwrap(), Decided::Done);
            Ok(arrival)
        })
        .await
        .unwrap();
    assert_eq!(arrival.units, 50);
    let service = analytics(&db);
    let recorded = service
        .record_unknown_origin(&arrival.key.name, 50.0, 5.0, seen_to, Some(id.0))
        .await
        .unwrap();
    assert!(recorded.written);
    let record = recorded.record_id;
    let linked_to = record.clone();
    db.with_writer(move |conn| Ok(link(conn, &[id], &linked_to, 500.0).unwrap()))
        .await
        .unwrap();
    let held = |db: &Db, record: String| {
        let db = db.clone();
        async move {
            db.with_reader(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT COALESCE(SUM(quantity), 0) FROM stock_movements \
                         WHERE ref_id = ?1 AND movement_kind = 'opening_balance' \
                           AND source_kind = 'unattributed'",
                        [record],
                        |row| row.get::<_, f64>(0),
                    )
                    .unwrap())
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(held(&db, record.clone()).await, 50.0);
    assert_eq!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Recorded {
            proposal: id,
            record_id: record.clone(),
            recorded_as: RecordedAs::UnknownOrigin,
            asset_name: None,
        }
    );
    // Taken back: the stock goes and the arrival is open to record again.
    assert_eq!(
        service.undo_unknown_origin(&record).await.unwrap(),
        UnknownOriginUndo::Undone
    );
    assert_eq!(held(&db, record.clone()).await, 0.0);
    assert!(matches!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Open { .. }
    ));
    assert_eq!(
        service.undo_unknown_origin(&record).await.unwrap(),
        UnknownOriginUndo::NotFound,
        "taken back once"
    );
}

/// The units of `item` held as stock of unknown origin.
async fn unknown_origin_units(db: &Db, item: &str) -> f64 {
    let item = item.to_string();
    db.with_reader(move |conn| {
        Ok(conn
            .query_row(
                "SELECT COALESCE(SUM(quantity), 0) FROM stock_movements \
                 WHERE item_name = ?1 AND movement_kind = 'opening_balance'",
                [item],
                |row| row.get::<_, f64>(0),
            )
            .unwrap())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn stock_recorded_from_an_arrival_and_never_linked_keeps_its_handle_and_is_never_written_twice(
) {
    let (_dir, db) = seeded().await;
    propose(&db).await;
    let id = proposal_of(&only_arrival(&read(&db).await));
    let (arrival, seen_to) = db
        .with_writer(move |conn| {
            let arrival = open_arrival(conn, id).unwrap().expect("open");
            assert_eq!(claim(conn, &[id], &[], 500.0).unwrap(), Decided::Done);
            Ok(arrival)
        })
        .await
        .unwrap();
    let service = analytics(&db);
    // Written, and the link never made (the app closed between the two).
    let first = service
        .record_unknown_origin(&arrival.key.name, 50.0, 5.0, seen_to, Some(id.0))
        .await
        .unwrap();
    assert!(first.written);
    // The arrival stays open, possibly recorded, with the record as its
    // handle.
    assert_eq!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Open {
            proposal: id,
            possibly_recorded: true,
            unlinked_record: Some(first.record_id.clone()),
        }
    );
    // Released, it still cannot be dismissed while that stock stands.
    let dismissed = db
        .with_writer(move |conn| {
            assert_eq!(
                super::proposals::release(conn, &[id]).unwrap(),
                Decided::Done
            );
            Ok(dismiss(conn, id, 600.0).unwrap())
        })
        .await
        .unwrap();
    assert!(matches!(dismissed, Decided::Refused(_)), "{dismissed:?}");
    // Taken over: the standing record is handed back and nothing is written.
    let again = service
        .record_unknown_origin(&arrival.key.name, 50.0, 5.0, seen_to, Some(id.0))
        .await
        .unwrap();
    assert!(!again.written);
    assert_eq!(again.record_id, first.record_id);
    assert_eq!(unknown_origin_units(&db, "Lysterium Ingot").await, 50.0);
    let record = again.record_id.clone();
    let linked = db
        .with_writer(move |conn| Ok(link(conn, &[id], &record, 700.0).unwrap()))
        .await
        .unwrap();
    assert_eq!(linked, Decided::Done);
    assert!(matches!(
        only_arrival(&read(&db).await),
        ArrivalDecision::Recorded {
            recorded_as: RecordedAs::UnknownOrigin,
            ..
        }
    ));
    // Taken back, a new recording writes afresh.
    assert_eq!(
        service.undo_unknown_origin(&first.record_id).await.unwrap(),
        UnknownOriginUndo::Undone
    );
    let fresh = service
        .record_unknown_origin(&arrival.key.name, 50.0, 5.0, seen_to, Some(id.0))
        .await
        .unwrap();
    assert!(fresh.written);
    assert_ne!(fresh.record_id, first.record_id);
}

#[tokio::test]
async fn the_loot_correction_handle_deactivates_the_line_that_never_arrived() {
    let (_dir, db) = open().await;
    let (oil_line, iron_line) = db
        .with_writer(|conn| Ok(iron_never_arrives(conn)))
        .await
        .unwrap();
    let found = read(&db).await;
    let LootCorrection::Offered {
        session_id,
        item_name,
    } = found.steps[1].looted_never_arrived[0].correction.clone()
    else {
        panic!("the correction is offered");
    };
    crate::tracking_reads::bulk_flip_loot_item(&db, &session_id, &item_name, "deactivated")
        .await
        .unwrap();
    let deactivated = |line: i64| {
        let db = db.clone();
        async move {
            db.with_reader(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT deactivated_at IS NOT NULL FROM kill_loot_items WHERE id = ?1",
                        [line],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap())
            })
            .await
            .unwrap()
        }
    };
    assert!(deactivated(iron_line).await);
    assert!(
        !deactivated(oil_line).await,
        "the oil arrived and stays loot"
    );
    let after = read(&db).await;
    for step in &after.steps {
        assert!(step.looted_never_arrived.is_empty(), "{step:?}");
    }
    let stored = db
        .with_reader(|conn| Ok(session_arrivals(conn, SESSION).unwrap()))
        .await
        .unwrap();
    assert!(stored.is_empty());
}
