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

/// Shrapnel leaving as ammo arrives a unit short of what it pays: converted
/// in two goes, each rounded down.
fn presumed(session: Option<&'static str>) -> Vec<(Reading, Purpose, Option<&'static str>)> {
    let purpose = if session.is_some() {
        Purpose::Session
    } else {
        Purpose::Ambient
    };
    vec![
        (
            reading(
                hour_ago() - 60.0,
                5_000,
                &[
                    ("Shrapnel", C, 100_000, 1_000),
                    ("Universal Ammo", C, 50_000, 500),
                ],
            ),
            purpose,
            session,
        ),
        (
            reading(hour_ago(), 5_000, &[("Universal Ammo", C, 150_999, 1_509)]),
            purpose,
            session,
        ),
    ]
}

#[tokio::test]
async fn shrapnel_presumed_converted_is_recorded_outside_a_session_and_never_inside_one() {
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, presumed(None)).await;
    assert_eq!(pass(&env).await.recorded, 1);
    assert_eq!(
        count(
            &env.db,
            "SELECT COUNT(*) FROM stock_records_from_account \
             WHERE part = 'shrapnel_conversion_presumed'"
        )
        .await,
        1
    );
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);

    // Inside a session Shrapnel is looted as it is converted: what a step
    // shows leaving is not what was converted, so nothing is presumed.
    let env = self::env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    let start = hour_ago() - 70.0;
    env.db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES ('live', ?1, NULL, 1)",
                [start],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    store(&env.db, presumed(Some("live"))).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!((shrapnel_held(&env.analytics).await - 100_000.0).abs() < 1e-6);
}

#[tokio::test]
async fn what_a_session_converted_is_recorded_once_as_its_last_reading_is_looked_at() {
    let env = env().await;
    // 10,000 Shrapnel held from an earlier hunt.
    hunted(&env.db, "k0", "Shrapnel", 10_000, 1.0).await;
    let start = hour_ago() - 180.0;
    env.db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, \
                 definition_id) VALUES ('live', ?1, ?2, 0, 5)",
                [start + 1.0, start + 175.0],
            )?;
            // A kill a minute, each spending 1 PED; the second loots 2,000
            // Shrapnel.
            for (id, at) in [("a", 30.0), ("b", 90.0), ("c", 150.0)] {
                conn.execute(
                    "INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, \
                     loot_total_ped) VALUES (?1, 'live', 'Atrox', ?2, 1.0, 0)",
                    rusqlite::params![id, start + at],
                )?;
                conn.execute(
                    "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, \
                     cost_per_shot) VALUES (?1, 'Opalo', 100, 0.01)",
                    [id],
                )?;
            }
            conn.execute(
                "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
                 VALUES ('b', 'Shrapnel', 2000, 0.2)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    // 10,000 ammo fired a minute; in the second, all 12,000 Shrapnel
    // becomes 12,120 ammo stacked onto the row being fired.
    let shrapnel = || ("Shrapnel", C, 10_000, 100);
    let ammo = |units: i64| ("Universal Ammo", C, units, units / 100);
    store(
        &env.db,
        vec![
            (
                reading(start, 5_000, &[shrapnel(), ammo(100_000)]),
                Purpose::Baseline,
                Some("live"),
            ),
            (
                reading(start + 60.0, 5_000, &[shrapnel(), ammo(90_000)]),
                Purpose::Session,
                Some("live"),
            ),
            (
                reading(start + 120.0, 5_000, &[ammo(92_120)]),
                Purpose::Session,
                Some("live"),
            ),
            (
                reading(start + 180.0, 5_000, &[ammo(82_120)]),
                Purpose::Session,
                Some("live"),
            ),
        ],
    )
    .await;
    assert_eq!(pass(&env).await.recorded, 1);
    assert_eq!(
        count(
            &env.db,
            "SELECT COUNT(*) FROM stock_records_from_account \
             WHERE part = 'shrapnel_conversion_session' AND reading_id = 4 \
               AND prev_reading_id = 1"
        )
        .await,
        1
    );
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);
    look_again(&env.db).await;
    assert_eq!(pass(&env).await.recorded, 0, "never twice");
}

#[tokio::test]
async fn a_step_spanning_hours_records_only_when_its_change_is_read_whole() {
    // Hours apart, but nothing changed but the conversion: no play hides
    // in it, so it is recorded.
    let env = env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    store(&env.db, converted_at(hour_ago(), 3.0 * 3_600.0)).await;
    let done = pass(&env).await;
    assert_eq!(done.recorded, 1);
    assert!(shrapnel_held(&env.analytics).await.abs() < 1e-6);

    // Hours apart, with something arriving that no pattern explains:
    // untracked play may hide in it, so nothing is recorded.
    let env = self::env().await;
    hunted(&env.db, "k1", "Shrapnel", 100_000, 10.0).await;
    let mut readings = converted_at(hour_ago(), 3.0 * 3_600.0);
    readings[1].0 = reading(
        hour_ago(),
        5_000,
        &[
            ("Universal Ammo", C, 50_000, 500),
            ("Universal Ammo", C, 101_000, 1_010),
            ("Animal Hide", C, 40, 400),
        ],
    );
    store(&env.db, readings).await;
    let done = pass(&env).await;
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

// ── Listing outcomes across a step of any length ────────────────────

const A: Container = Container::Auction;

/// Hunted loot to list: ten hides of 5.00 TT and five stones of 1.30 TT.
async fn loot_to_list(db: &Db) {
    hunted(db, "k1", "Animal Hide", 10, 5.0).await;
    hunted(db, "k2", "Iron Stone", 5, 1.3).await;
}

/// The player's own listing of `quantity` `item`, posted at eight that
/// morning at a starting bid of `bid` and a buyout of `buyout`; its id.
async fn listed(env: &Env, item: &str, quantity: f64, bid: f64, buyout: Option<f64>) -> String {
    env.analytics
        .create_auction_listing(
            Profession::Inventory,
            item,
            quantity,
            bid,
            buyout,
            0.5,
            Some("2026-06-01T08:00:00"),
            None,
        )
        .await
        .expect("listed")
        .id
}

/// A listing's status and final price, as it stands.
async fn listing_now(analytics: &AnalyticsService, id: &str) -> (String, Option<f64>) {
    let row = analytics
        .auction_listings(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .expect("the listing stands");
    (row.status, row.final_price)
}

/// Two idle readings `apart` seconds apart, the later an hour before the
/// clock.
fn idle_step(
    apart: f64,
    before: (i64, &[(&str, Container, i64, i64)]),
    after: (i64, &[(&str, Container, i64, i64)]),
) -> Vec<(Reading, Purpose, Option<&'static str>)> {
    vec![
        (
            reading(hour_ago() - apart, before.0, before.1),
            Purpose::Ambient,
            None,
        ),
        (
            reading(hour_ago(), after.0, after.1),
            Purpose::Ambient,
            None,
        ),
    ]
}

/// Rows of a reading: `(name, container, quantity, value in PEC)`.
type Rows = &'static [(&'static str, Container, i64, i64)];

/// Two hours: longer than any step a sale is priced from as a rule.
const AWAY: f64 = 2.0 * 3_600.0;

/// Every proposal still open once every waiting reading is examined: its
/// kind and the listing it names, when it names one.
async fn proposed(db: &Db) -> Vec<(super::proposals::ProposalKind, Option<String>)> {
    use super::proposals::{examine, pending, Detail, Which};
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        while examine(&db, None, Which::Oldest, 1.0).unwrap().is_some() {}
        db.with_reader_blocking(|conn| Ok(pending(conn, None)))
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|proposal| {
                let listing = match &proposal.detail {
                    Detail::Listing(listing) => Some(listing.listing_id.clone()),
                    _ => None,
                };
                (proposal.kind, listing)
            })
            .collect()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_listing_that_came_back_is_recorded_expired_however_long_the_step() {
    let env = env().await;
    loot_to_list(&env.db).await;
    let hides = listed(&env, "Animal Hide", 10.0, 7.0, Some(9.0)).await;
    // Hours apart, and something else arrived meanwhile: what came back is
    // still exactly what left.
    store(
        &env.db,
        idle_step(
            AWAY,
            (5_000, &[("Animal Hide", A, 10, 500)]),
            (
                5_000,
                &[("Animal Hide", C, 10, 500), ("Garcen Grease", C, 1, 100)],
            ),
        ),
    )
    .await;
    assert_eq!(pass(&env).await.recorded, 1);
    assert_eq!(
        listing_now(&env.analytics, &hides).await,
        ("expired".to_string(), None)
    );
    assert!(
        proposed(&env.db)
            .await
            .iter()
            .all(|(_, listing)| listing.as_deref() != Some(hides.as_str())),
        "recorded, so never proposed as well"
    );
}

/// A tracked session from `from` to `to`, its chat log looting nothing.
async fn session_between(db: &Db, from: f64, to: f64) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('away', ?1, ?2, 0)",
            [from, to],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// A kill at `at` in the long-ended hunt whose chat log looted `item`.
async fn looted_at(db: &Db, item: &'static str, at: f64) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES('k-away', 'hunt', 'Atrox', 'Atrox', '', ?1, 1, 1.0, 0.1, 5.0, 0, 0)",
            [at],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES('k-away', ?1, 10, 5.0)",
            [item],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_long_step_s_return_is_left_to_the_player_when_play_may_explain_it() {
    // Ten hides back from the auction while away: certain on its own, but
    // not beside a session, loot of the same, or another listing of them.
    for case in ["a session overlapped", "loot logged", "another listing"] {
        let env = env().await;
        loot_to_list(&env.db).await;
        let hides = listed(&env, "Animal Hide", 10.0, 7.0, Some(9.0)).await;
        match case {
            "a session overlapped" => {
                session_between(&env.db, hour_ago() - AWAY + 60.0, hour_ago() - 60.0).await
            }
            "loot logged" => looted_at(&env.db, "Animal Hide", hour_ago() - 600.0).await,
            _ => {
                listed(&env, "Animal Hide", 4.0, 3.0, None).await;
            }
        }
        store(
            &env.db,
            idle_step(
                AWAY,
                (5_000, &[("Animal Hide", A, 10, 500)]),
                (5_000, &[("Animal Hide", C, 10, 500)]),
            ),
        )
        .await;
        assert_eq!(pass(&env).await.recorded, 0, "{case}");
        assert_eq!(
            listing_now(&env.analytics, &hides).await,
            ("pending".to_string(), None),
            "{case}"
        );
        assert_eq!(
            proposed(&env.db).await,
            vec![(super::proposals::ProposalKind::ListingReturned, Some(hides))],
            "{case}: proposed instead"
        );
    }
}

#[tokio::test]
async fn a_long_step_a_session_overlapped_settles_no_sale() {
    let env = env().await;
    session_between(&env.db, hour_ago() - AWAY + 60.0, hour_ago() - 60.0).await;
    let (hides, stones) = sold_while_away(&env, 3.0, 1_000, &[], &[]).await;
    assert_eq!(pass(&env).await.recorded, 0);
    let mut sold: Vec<String> = proposed(&env.db)
        .await
        .into_iter()
        .filter_map(|(_, listing)| listing)
        .collect();
    sold.sort();
    let mut both = vec![hides, stones];
    both.sort();
    assert_eq!(sold, both);
}

#[tokio::test]
async fn sales_beside_anything_that_is_not_a_plain_sale_settle_none_and_are_proposed() {
    let storage = || Container::Storage {
        planet: "Calypso".to_string(),
    };
    /// A row of a reading: `(name, container, quantity, value in PEC)`.
    type Row = (&'static str, Container, i64, i64);
    /// The case, how many stone listings, the rows beside the hides before
    /// the step and after it, and the Card's rise.
    type Case = (&'static str, usize, Vec<Row>, Vec<Row>, i64);
    let cases: Vec<Case> = vec![
        (
            "a move between the player's own places",
            1,
            vec![("Iron Stone", A, 5, 130), ("Opalo", storage(), 1, 3_000)],
            vec![("Opalo", C, 1, 3_000)],
            1_000,
        ),
        (
            "an unrelated item leaving",
            1,
            vec![("Iron Stone", A, 5, 130), ("Garcen Grease", C, 1, 100)],
            vec![],
            1_000,
        ),
        (
            "a departure that is not the only change of its item",
            1,
            vec![("Iron Stone", A, 5, 130), ("Iron Stone", C, 5, 130)],
            vec![("Iron Stone", C, 2, 52)],
            1_000,
        ),
        (
            "two departures matching one listing",
            0,
            vec![("animal hide", A, 10, 500)],
            vec![],
            1_400,
        ),
        (
            "a departure matching no listing",
            0,
            vec![("Iron Stone", A, 5, 130)],
            vec![],
            1_000,
        ),
        (
            "a departure matching several listings",
            2,
            vec![("Iron Stone", A, 5, 130)],
            vec![],
            1_000,
        ),
    ];
    for (case, stone_listings, mut before, after, rise) in cases {
        let env = env().await;
        loot_to_list(&env.db).await;
        let hides = listed(&env, "Animal Hide", 10.0, 7.0, Some(7.0)).await;
        for _ in 0..stone_listings {
            listed(&env, "Iron Stone", 5.0, 3.0, Some(3.0)).await;
        }
        before.push(("Animal Hide", A, 10, 500));
        let readings = vec![
            (
                reading(hour_ago() - AWAY, 5_000, &before),
                Purpose::Ambient,
                None,
            ),
            (
                reading(hour_ago(), 5_000 + rise, &after),
                Purpose::Ambient,
                None,
            ),
        ];
        store(&env.db, readings).await;
        assert_eq!(pass(&env).await.recorded, 0, "{case}");
        assert_eq!(
            listing_now(&env.analytics, &hides).await,
            ("pending".to_string(), None),
            "{case}"
        );
        assert!(
            proposed(&env.db).await.contains(&(
                super::proposals::ProposalKind::ListingSold,
                Some(hides.clone())
            )),
            "{case}: the hides proposed instead"
        );
    }
}

/// Both listings sold while the player was away, the Card up by `rise`
/// PEC; their ids.
async fn sold_while_away(
    env: &Env,
    stone_buyout: f64,
    rise: i64,
    also_after: &[(&str, Container, i64, i64)],
    also_before: &[(&str, Container, i64, i64)],
) -> (String, String) {
    loot_to_list(&env.db).await;
    let hides = listed(env, "Animal Hide", 10.0, 7.0, Some(7.0)).await;
    let stones = listed(env, "Iron Stone", 5.0, 3.0, Some(stone_buyout)).await;
    let mut before = vec![("Animal Hide", A, 10, 500), ("Iron Stone", A, 5, 130)];
    before.extend_from_slice(also_before);
    store(
        &env.db,
        idle_step(AWAY, (5_000, &before), (5_000 + rise, also_after)),
    )
    .await;
    (hides, stones)
}

#[tokio::test]
async fn the_sales_of_a_long_step_that_holds_nothing_else_are_recorded_at_their_one_price() {
    let env = env().await;
    let (hides, stones) = sold_while_away(&env, 3.0, 1_000, &[], &[]).await;
    assert_eq!(pass(&env).await.recorded, 2);
    assert_eq!(
        listing_now(&env.analytics, &hides).await,
        ("sold".to_string(), Some(7.0))
    );
    assert_eq!(
        listing_now(&env.analytics, &stones).await,
        ("sold".to_string(), Some(3.0))
    );
    assert!(
        proposed(&env.db).await.is_empty(),
        "recorded, so not proposed"
    );
}

#[tokio::test]
async fn a_long_step_holding_anything_more_than_its_sales_records_none_and_proposes_them() {
    /// What else a case holds: the stones' buyout, the Card's rise, and the
    /// rows beside the sales after the step and before it.
    struct Case {
        what: &'static str,
        stone_buyout: f64,
        rise: i64,
        after: Rows,
        before: Rows,
    }
    let plain = |what, stone_buyout, rise| Case {
        what,
        stone_buyout,
        rise,
        after: &[],
        before: &[],
    };
    let cases = [
        plain("a Card rise neither total fits", 3.0, 999),
        Case {
            after: &[("Garcen Grease", C, 1, 100)],
            ..plain("something arriving", 3.0, 1_000)
        },
        Case {
            after: &[("Opalo", C, 1, 2_990)],
            before: &[("Opalo", C, 1, 3_000)],
            ..plain("something wearing", 3.0, 1_000)
        },
        plain("a buyout above the starting bid", 4.0, 1_000),
    ];
    for Case {
        what: case,
        stone_buyout,
        rise,
        after,
        before,
    } in cases
    {
        let env = env().await;
        let (hides, stones) = sold_while_away(&env, stone_buyout, rise, after, before).await;
        assert_eq!(pass(&env).await.recorded, 0, "{case}");
        for id in [&hides, &stones] {
            assert_eq!(
                listing_now(&env.analytics, id).await,
                ("pending".to_string(), None),
                "{case}"
            );
        }
        let mut sold: Vec<String> = proposed(&env.db)
            .await
            .into_iter()
            .filter(|(kind, _)| *kind == super::proposals::ProposalKind::ListingSold)
            .filter_map(|(_, listing)| listing)
            .collect();
        sold.sort();
        let mut both = vec![hides, stones];
        both.sort();
        assert_eq!(sold, both, "{case}: each left to the player to price");
    }
}

#[tokio::test]
async fn a_long_step_never_prices_a_sale_a_short_one_would() {
    for (apart, recorded) in [(60.0, 1), (AWAY, 0)] {
        let env = env().await;
        loot_to_list(&env.db).await;
        let hides = listed(&env, "Animal Hide", 10.0, 7.0, Some(8.0)).await;
        // The Card rose by the buyout: the one price that fits.
        store(
            &env.db,
            idle_step(apart, (5_000, &[("Animal Hide", A, 10, 500)]), (5_800, &[])),
        )
        .await;
        assert_eq!(pass(&env).await.recorded, recorded, "{apart} seconds");
        let status = listing_now(&env.analytics, &hides).await.0;
        let proposed = proposed(&env.db).await;
        if recorded == 1 {
            assert_eq!(status, "sold");
            assert!(proposed.is_empty());
        } else {
            assert_eq!(status, "pending");
            assert_eq!(
                proposed,
                vec![(super::proposals::ProposalKind::ListingSold, Some(hides))]
            );
        }
    }
}

// ── A listing the player made ───────────────────────────────────────

/// Ten hides listed between two idle readings a minute apart, the later
/// at `at`: the Card falls by 0.51, a fee whose starting bids run from
/// 5.20 to 5.41 on the hides' 5.00 TT as the site shows it, which may be a
/// PEC off either way.
fn hides_listed_at(at: f64) -> Vec<(Reading, Purpose, Option<&'static str>)> {
    vec![
        (
            reading(at - 60.0, 5_000, &[("Animal Hide", C, 10, 500)]),
            Purpose::Ambient,
            None,
        ),
        (
            reading(at, 4_949, &[("Animal Hide", A, 10, 500)]),
            Purpose::Ambient,
            None,
        ),
    ]
}

/// How many events are new to the player, with nothing yet seen.
async fn unseen(db: &Db) -> i64 {
    db.with_writer(|conn| {
        conn.execute("UPDATE account_activity_seen SET seen_through = 0", [])?;
        Ok(super::activity::marks(conn))
    })
    .await
    .unwrap()
    .unwrap()
    .unseen
}

/// The listings standing in the Inventory.
async fn listings(analytics: &AnalyticsService) -> Vec<crate::analytics::AuctionListingRow> {
    analytics
        .auction_listings(Profession::Inventory)
        .await
        .unwrap()
}

async fn held(analytics: &AnalyticsService, item: &str) -> f64 {
    analytics
        .stock_positions(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.item_name == item)
        .map_or(0.0, |row| row.quantity)
}

#[tokio::test]
async fn a_listing_made_while_the_app_reads_is_created_with_its_fee_exact_and_its_bid_inferred() {
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    store(&env.db, hides_listed_at(hour_ago())).await;
    assert_eq!(pass(&env).await.recorded, 1);
    let [listing] = listings(&env.analytics)
        .await
        .try_into()
        .expect("one listing");
    assert_eq!(listing.item_name, "Animal Hide");
    assert_eq!(listing.quantity, 10.0);
    assert_eq!(listing.status, "pending");
    assert!((listing.listing_fee - 0.51).abs() < 1e-9, "the Card's fall");
    // The midpoint of 5.20 to 5.41, and nothing claimed of the rest.
    assert!((listing.starting_bid - 5.30).abs() < 1e-9);
    assert_eq!((listing.buyout, listing.auction_days), (None, None));
    let source = listing
        .listed_from_account
        .expect("created from the account");
    assert_eq!(source.basis, crate::analytics::RecordBasis::Inferred);
    assert_eq!(
        source.inferred_bid,
        Some(crate::analytics::InferredBid {
            low: 5.20,
            high: 5.41
        })
    );
    assert_eq!(
        (source.seen_from, source.seen_to),
        (hour_ago() - 60.0, hour_ago())
    );
    assert_eq!(source.card_change, Some(-0.51));
    assert_eq!(listing.from_account, None, "no outcome yet");
    assert!(held(&env.analytics, "Animal Hide").await.abs() < 1e-9);
    // On the history, and among what is new to the player.
    let row = env
        .analytics
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == listing.id)
        .expect("on the history");
    assert_eq!(row.listed_from_account, Some(source));
    assert_eq!(unseen(&env.db).await, 1);
}

#[tokio::test]
async fn a_listing_made_is_created_once_and_never_again_once_undone() {
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    store(&env.db, hides_listed_at(hour_ago())).await;
    pass(&env).await;
    look_again(&env.db).await;
    assert_eq!(pass(&env).await.recorded, 0);
    let [listing] = listings(&env.analytics)
        .await
        .try_into()
        .expect("one listing");
    assert!(env
        .analytics
        .undo_auction_listing(&listing.id)
        .await
        .unwrap());
    look_again(&env.db).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!(listings(&env.analytics).await.is_empty());
    assert!((held(&env.analytics, "Animal Hide").await - 10.0).abs() < 1e-9);
    assert_eq!(unseen(&env.db).await, 0, "taken back, so no news");
}

#[tokio::test]
async fn no_listing_is_created_from_a_long_step_or_of_an_item_the_app_does_not_hold() {
    // Hours apart: play no reading saw may have moved the Card.
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    let mut readings = hides_listed_at(hour_ago());
    readings[0].0 = reading(hour_ago() - AWAY, 5_000, &[("Animal Hide", C, 10, 500)]);
    store(&env.db, readings).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!(listings(&env.analytics).await.is_empty());

    // Held by the app not at all, once the step has settled.
    let env = self::env().await;
    store(&env.db, hides_listed_at(hour_ago())).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!(listings(&env.analytics).await.is_empty());
}

#[tokio::test]
async fn a_listing_not_held_in_full_is_passed_without_stopping_the_pass() {
    let env = env().await;
    // The step ended a minute before the clock, and the hides' loot is not
    // in yet; a Shrapnel conversion follows in the next step.
    hunted(&env.db, "k2", "Shrapnel", 100_000, 10.0).await;
    let ammo = || ("Universal Ammo", C, 50_000, 500);
    let shrapnel = || ("Shrapnel", C, 100_000, 1_000);
    store(
        &env.db,
        vec![
            (
                reading(
                    noon() - 180.0,
                    5_000,
                    &[("Animal Hide", C, 10, 500), shrapnel(), ammo()],
                ),
                Purpose::Ambient,
                None,
            ),
            (
                reading(
                    noon() - 120.0,
                    4_949,
                    &[("Animal Hide", A, 10, 500), shrapnel(), ammo()],
                ),
                Purpose::Ambient,
                None,
            ),
            (
                reading(
                    noon() - 60.0,
                    4_949,
                    &[
                        ("Animal Hide", A, 10, 500),
                        ammo(),
                        ("Universal Ammo", C, 101_000, 1_010),
                    ],
                ),
                Purpose::Ambient,
                None,
            ),
        ],
    )
    .await;
    let done = pass(&env).await;
    assert!(!done.waiting, "a listing never waits");
    assert_eq!(done.recorded, 1, "the conversion after it");
    assert_eq!(waiting(&env.db).await, None);
    // The loot landing later creates nothing: the step was looked at and
    // marked, not left to wait.
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    assert_eq!(pass(&env).await, FollowPass::default());
    assert!(listings(&env.analytics).await.is_empty());
}

#[tokio::test]
async fn a_listing_held_in_part_is_left_with_nothing_booked() {
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 6, 3.0).await;
    store(&env.db, hides_listed_at(hour_ago())).await;
    assert_eq!(pass(&env).await.recorded, 0);
    assert!(listings(&env.analytics).await.is_empty());
    assert!((held(&env.analytics, "Animal Hide").await - 6.0).abs() < 1e-9);
    assert_eq!(
        count(
            &env.db,
            "SELECT COUNT(*) FROM stock_movements WHERE movement_kind = 'opening_balance'"
        )
        .await,
        0,
        "no opening balance booked"
    );
}

#[tokio::test]
async fn a_listing_the_account_created_or_one_the_player_took_back_is_no_hand_entry() {
    // Two listings of ten hides in consecutive steps, both seen made.
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 20, 10.0).await;
    let both = [("Animal Hide", C, 10, 500), ("Animal Hide", C, 10, 500)];
    store(
        &env.db,
        vec![
            (
                reading(hour_ago() - 120.0, 5_000, &both),
                Purpose::Ambient,
                None,
            ),
            (
                reading(
                    hour_ago() - 60.0,
                    4_949,
                    &[("Animal Hide", A, 10, 500), ("Animal Hide", C, 10, 500)],
                ),
                Purpose::Ambient,
                None,
            ),
            (
                reading(
                    hour_ago(),
                    4_898,
                    &[("Animal Hide", A, 10, 500), ("Animal Hide", A, 10, 500)],
                ),
                Purpose::Ambient,
                None,
            ),
        ],
    )
    .await;
    assert_eq!(pass(&env).await.recorded, 2);
    assert_eq!(listings(&env.analytics).await.len(), 2);

    // The player's own listing, taken back before the step.
    let env = self::env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    let mine = listed(&env, "Animal Hide", 10.0, 7.0, None).await;
    assert!(env.analytics.undo_auction_listing(&mine).await.unwrap());
    store(&env.db, hides_listed_at(noon() + 180.0)).await;
    assert_eq!(pass(&env).await.recorded, 1);
    assert_eq!(listings(&env.analytics).await.len(), 1);
}

#[tokio::test]
async fn a_listing_the_player_recorded_by_hand_of_the_same_item_and_quantity_keeps_it_out() {
    for (by_hand, created) in [(10.0, 0), (4.0, 1)] {
        let env = env().await;
        hunted(&env.db, "k1", "Animal Hide", 20, 10.0).await;
        // The player's own listing, made at the clock.
        listed(&env, "Animal Hide", by_hand, 7.0, None).await;
        // Seen in a step that began two minutes after it.
        store(&env.db, hides_listed_at(noon() + 180.0)).await;
        assert_eq!(pass(&env).await.recorded, created, "{by_hand} by hand");
        assert_eq!(listings(&env.analytics).await.len(), 1 + created);
    }
}

#[tokio::test]
async fn a_listing_the_account_created_is_never_taken_for_the_player_s_own_entry() {
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 20, 10.0).await;
    // Ten hides listed, then the other ten sold to the Trade Terminal a
    // minute later.
    let mut readings = vec![(
        reading(
            hour_ago() - 120.0,
            5_000,
            &[("Animal Hide", C, 10, 500), ("Animal Hide", C, 10, 500)],
        ),
        Purpose::Ambient,
        None,
    )];
    readings.push((
        reading(
            hour_ago() - 60.0,
            4_949,
            &[("Animal Hide", A, 10, 500), ("Animal Hide", C, 10, 500)],
        ),
        Purpose::Ambient,
        None,
    ));
    readings.push((
        reading(hour_ago(), 5_449, &[("Animal Hide", A, 10, 500)]),
        Purpose::Ambient,
        None,
    ));
    store(&env.db, readings).await;
    assert_eq!(pass(&env).await.recorded, 2, "the listing, then the sale");
    assert_eq!(
        count(&env.db, "SELECT COUNT(*) FROM private_sales").await,
        1
    );
}

#[tokio::test]
async fn a_created_listing_s_sale_is_proposed_never_priced_and_its_return_is_recorded() {
    // Sold a minute later, the Card up by exactly the bid it stores.
    let env = env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    let mut readings = hides_listed_at(hour_ago() - 60.0);
    readings.push((reading(hour_ago(), 5_479, &[]), Purpose::Ambient, None));
    store(&env.db, readings).await;
    assert_eq!(pass(&env).await.recorded, 1, "the listing alone");
    let [listing] = listings(&env.analytics)
        .await
        .try_into()
        .expect("one listing");
    assert_eq!(listing.status, "pending");
    assert_eq!(
        proposed(&env.db).await,
        vec![(
            super::proposals::ProposalKind::ListingSold,
            Some(listing.id)
        )]
    );
    // With the Card's rise offered as the proceeds, for the player to say.
    let offered = env
        .db
        .with_reader(|conn| Ok(super::proposals::pending(conn, None)))
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .find_map(|proposal| match proposal.detail {
            super::proposals::Detail::Listing(listing) => Some(listing.proceeds),
            _ => None,
        });
    assert_eq!(offered, Some(Some(super::amount::Pec(530))));

    // Come back a minute later: certain, and recorded.
    let env = self::env().await;
    hunted(&env.db, "k1", "Animal Hide", 10, 5.0).await;
    let mut readings = hides_listed_at(hour_ago() - 60.0);
    readings.push((
        reading(hour_ago(), 4_949, &[("Animal Hide", C, 10, 500)]),
        Purpose::Ambient,
        None,
    ));
    store(&env.db, readings).await;
    assert_eq!(pass(&env).await.recorded, 2);
    let [listing] = listings(&env.analytics)
        .await
        .try_into()
        .expect("one listing");
    assert_eq!(listing.status, "expired");
}
