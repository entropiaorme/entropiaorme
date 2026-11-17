//! The inventory following the account: a step recorded once however often
//! it is looked at, an undone record never made again, a strongbox run's
//! steps left to its own flow, a session's steps read like any other, each
//! record carrying the step it came from, long steps and history left
//! alone, stock still landing waited for, and a failing step passed over.

use std::sync::Arc;

use super::follow::{follow, next_unrecognised, FollowPass, STOCK_SETTLES};
use super::reading::{Container, Reading};
use super::store::{record_reading, Purpose, ReadingId};
use super::test_support::reading;
use crate::analytics::{AnalyticsService, Profession};
use crate::clock::{Clock, MockClock};
use crate::db::Db;
use crate::time::naive_to_epoch;

const C: Container = Container::Carried;

fn noon_naive() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
}

/// The analytics clock, as the app reads it: every reading below is
/// placed from it, so nothing depends on the machine's time zone.
fn noon() -> f64 {
    naive_to_epoch(noon_naive())
}

/// An hour before the clock: a step there has long settled.
fn hour_ago() -> f64 {
    noon() - 3_600.0
}

struct Env {
    _dir: tempfile::TempDir,
    db: Db,
    analytics: AnalyticsService,
    clock: Arc<MockClock>,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("follow.db")).await.unwrap();
    let clock = Arc::new(MockClock::new(Some(noon_naive()), 0.0));
    let analytics = AnalyticsService::new(db.clone(), clock.clone());
    Env {
        _dir: dir,
        db,
        analytics,
        clock,
    }
}

/// Hunted loot of `item` held as stock, from the kill `kill`.
async fn hunted(db: &Db, kill: &'static str, item: &'static str, quantity: i64, tt: f64) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO session_definitions(id, name, ad_hoc_segments, is_active) \
             VALUES(5, 'Atrox', 0, 1)",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO tracking_sessions(id, started_at, ended_at, is_active, \
             definition_id) VALUES('hunt', 1000.0, 2000.0, 0, 5)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES(?1, 'hunt', 'Atrox', 'Atrox', '', 1500.0, 1, 1.0, 0.1, ?2, 0, 0)",
            rusqlite::params![kill, tt],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![kill, item, quantity, tt],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// Store readings in order, with what each was taken for.
async fn store(db: &Db, readings: Vec<(Reading, Purpose, Option<&'static str>)>) {
    db.with_writer(move |conn| {
        for (reading, purpose, session) in &readings {
            record_reading(conn, reading, *purpose, *session).expect("stored");
        }
        Ok(())
    })
    .await
    .unwrap();
}

async fn pass_over(env: &Env, over: Option<ReadingId>) -> FollowPass {
    let now = naive_to_epoch(env.clock.now());
    let db = env.db.clone();
    let analytics = AnalyticsService::new(env.db.clone(), env.clock.clone());
    tokio::task::spawn_blocking(move || follow(&db, &analytics, None, now, over))
        .await
        .unwrap()
        .unwrap()
}

async fn pass(env: &Env) -> FollowPass {
    pass_over(env, None).await
}

async fn count(db: &Db, sql: &'static str) -> i64 {
    db.with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?))
        .await
        .unwrap()
}

async fn waiting(db: &Db) -> Option<ReadingId> {
    db.with_reader(|conn| Ok(next_unrecognised(conn)))
        .await
        .unwrap()
        .unwrap()
}

/// Forget that every step was looked at, as a step looked at again would
/// be.
async fn look_again(db: &Db) {
    db.with_writer(|conn| {
        conn.execute("DELETE FROM account_steps_recognised", [])?;
        Ok(())
    })
    .await
    .unwrap();
}

/// Shrapnel turned into Universal Ammo between two idle readings, the
/// later taken at `at`, `apart` seconds after the earlier.
fn converted_at(at: f64, apart: f64) -> Vec<(Reading, Purpose, Option<&'static str>)> {
    vec![
        (
            reading(
                at - apart,
                5_000,
                &[
                    ("Shrapnel", C, 100_000, 1_000),
                    ("Universal Ammo", C, 50_000, 500),
                ],
            ),
            Purpose::Ambient,
            None,
        ),
        (
            reading(
                at,
                5_000,
                &[
                    ("Universal Ammo", C, 50_000, 500),
                    ("Universal Ammo", C, 101_000, 1_010),
                ],
            ),
            Purpose::Ambient,
            None,
        ),
    ]
}

/// [`converted_at`], a minute apart and an hour before the clock.
fn converted() -> Vec<(Reading, Purpose, Option<&'static str>)> {
    converted_at(hour_ago(), 60.0)
}

async fn shrapnel_held(analytics: &AnalyticsService) -> f64 {
    analytics
        .stock_positions(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.item_name == "Shrapnel")
        .map_or(0.0, |row| row.quantity)
}

#[tokio::test]
async fn a_step_is_recorded_once_however_often_it_is_looked_at() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted()).await;
    let first = pass(&env).await;
    assert_eq!(first.recognised, 2);
    assert_eq!(first.recorded, 1);
    assert!(!first.more);
    assert_eq!(pass(&env).await, FollowPass::default());
    look_again(&env.db).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert_eq!(
        count(&env.db, "SELECT COUNT(*) FROM stock_conversions").await,
        1
    );
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);
    assert_eq!(waiting(&env.db).await, None);
}

#[tokio::test]
async fn an_undone_record_marks_its_step_and_is_never_made_again() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted()).await;
    pass(&env).await;
    let id = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == "conversion")
        .expect("recorded")
        .id;
    assert!(env.analytics.undo_stock_conversion(&id).await.unwrap());
    look_again(&env.db).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!((shrapnel_held(&env.analytics).await - 100_000.0).abs() < 1e-6);
    assert_eq!(
        count(
            &env.db,
            "SELECT COUNT(*) FROM stock_conversions WHERE undone_at IS NULL"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn each_record_carries_the_step_it_came_from_and_is_dated_by_it() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted()).await;
    pass(&env).await;
    let entry = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == "conversion")
        .expect("recorded");
    let source = entry.from_account.expect("from the account");
    assert_eq!(
        (source.seen_from, source.seen_to),
        (hour_ago() - 60.0, hour_ago())
    );
    let day = chrono::DateTime::from_timestamp(hour_ago().floor() as i64, 0)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(entry.occurred_at, day);
}

#[tokio::test]
async fn a_strongbox_run_s_steps_are_left_to_its_own_flow() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    let readings = converted()
        .into_iter()
        .map(|(reading, _, _)| (reading, Purpose::Run, None))
        .collect();
    store(&env.db, readings).await;
    let done = pass(&env).await;
    assert_eq!(done.recognised, 2);
    assert_eq!(done.recorded, 0);
    assert_eq!(
        count(&env.db, "SELECT COUNT(*) FROM stock_conversions").await,
        0
    );
}

#[tokio::test]
async fn a_session_s_steps_are_read_like_any_other() {
    let env = env().await;
    hunted(&env.db, "k1", "Animal Oil Residue", 1_000, 10.0).await;
    let start = hour_ago() - 60.0;
    env.db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES ('live', ?1, NULL, 1)",
                [start - 10.0],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    store(
        &env.db,
        vec![
            (
                reading(start, 5_000, &[("Animal Oil Residue", C, 1_000, 1_000)]),
                Purpose::Baseline,
                Some("live"),
            ),
            (
                reading(hour_ago(), 6_000, &[]),
                Purpose::Session,
                Some("live"),
            ),
        ],
    )
    .await;
    assert_eq!(pass(&env).await.recorded, 1);
    assert_eq!(
        count(&env.db, "SELECT COUNT(*) FROM private_sales").await,
        1
    );
}

#[tokio::test]
async fn a_step_spanning_hours_records_nothing() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted_at(hour_ago(), 3.0 * 3_600.0)).await;
    let done = pass(&env).await;
    assert_eq!(done.recognised, 2);
    assert_eq!(done.recorded, 0);
    assert!((shrapnel_held(&env.analytics).await - 100_000.0).abs() < 1e-6);
}

/// Mark the readings stored so far as history: stored before the app
/// recorded from the account, as the update's migration marks them.
async fn history_so_far(db: &Db) {
    db.with_writer(|conn| {
        conn.execute(
            // id-order: cursor (the boundary is a position in the readings)
            "UPDATE account_recognition_boundary \
             SET last_reading_before = (SELECT MAX(id) FROM account_readings)",
            [],
        )?;
        conn.execute(
            "INSERT INTO account_steps_recognised (reading_id, recognised_at) \
             SELECT id, 0 FROM account_readings",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_step_from_a_reading_stored_before_the_update_records_nothing() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    let mut readings = converted();
    let later = readings.pop().expect("two readings");
    store(&env.db, readings).await;
    history_so_far(&env.db).await;
    // A minute later, the first reading after the update.
    store(&env.db, vec![later]).await;
    let done = pass(&env).await;
    assert_eq!(done.recognised, 1);
    assert_eq!(done.recorded, 0);
    assert!((shrapnel_held(&env.analytics).await - 100_000.0).abs() < 1e-6);
}

#[tokio::test]
async fn the_first_idle_step_after_an_update_reaching_back_days_records_nothing() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    let mut readings = converted_at(hour_ago(), 3.0 * 86_400.0);
    let later = readings.pop().expect("two readings");
    store(&env.db, readings).await;
    history_so_far(&env.db).await;
    store(&env.db, vec![later]).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert_eq!(waiting(&env.db).await, None);
}

#[tokio::test]
async fn loot_still_landing_is_waited_for_and_taken_in_on_the_retry() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 30_000, 3.0).await;
    // The step ended a minute before the clock: young.
    store(&env.db, converted_at(noon() - 60.0, 60.0)).await;
    let first = pass(&env).await;
    assert!(first.waiting);
    assert_eq!(first.recorded, 0);
    let held = waiting(&env.db).await.expect("the step is left unmarked");
    assert_eq!(
        count(&env.db, "SELECT COUNT(*) FROM stock_conversions").await,
        0
    );
    // The rest of the loot lands; a later reading's pass takes it all in.
    hunted(&env.db, "k2", "Shrapnel", 70_000, 7.0).await;
    let retry = pass(&env).await;
    assert!(!retry.waiting);
    assert_eq!(retry.recorded, 1);
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);
    assert_ne!(waiting(&env.db).await, Some(held));
}

#[tokio::test]
async fn stock_that_never_lands_leaves_the_held_part_recorded_once_the_step_has_settled() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 30_000, 3.0).await;
    store(&env.db, converted_at(noon() - 60.0, 60.0)).await;
    assert!(pass(&env).await.waiting);
    env.clock.advance(STOCK_SETTLES).unwrap();
    let settled = pass(&env).await;
    assert!(!settled.waiting);
    assert_eq!(settled.recorded, 1);
    assert_eq!(waiting(&env.db).await, None);
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);
}

#[tokio::test]
async fn a_step_passed_over_is_marked_with_nothing_recorded_and_the_rest_go_on() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted()).await;
    // The first reading's step (it has none) is marked; the second is the
    // one that kept failing.
    let first = waiting(&env.db).await.expect("waiting");
    let failing = ReadingId(first.0 + 1);
    let done = pass_over(&env, Some(failing)).await;
    assert_eq!(done.recognised, 2);
    assert_eq!(done.recorded, 0);
    assert_eq!(waiting(&env.db).await, None);
}
