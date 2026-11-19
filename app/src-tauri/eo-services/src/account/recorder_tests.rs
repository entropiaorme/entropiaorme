//! The account connection, driven over paused time against the scripted
//! stand-in: when it reads, what it keeps, and what it drops.
//!
//! Every test plans its tracked sessions' rows up front and then only
//! publishes session starts and stops at planned instants, so tokio's
//! clock (and the paced wall clock riding it) moves from one scheduled
//! moment to the next exactly as the app's would.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::Value;

use super::costs::Status;
use super::measure::{CoverageLevel, Edge};
use super::recorder::{
    AccountRecorder, ConnectOutcome, ConnectionFlag, ConnectionState, ConnectionStatus, MemoryFlag,
    ReadingMode, RecorderParts,
};
use super::scripted::{
    responses, PacedClock, ScriptedFault, ScriptedRead, ScriptedReading, ScriptedRow, ScriptedSite,
};
use super::sessions::MeasureRead;
use super::site::SessionSecret;
use crate::bus_events::{BusEvent, SessionLifecyclePayload};
use crate::db::Db;
use crate::event_bus::{EventBus, Topic};

const COOKIE: &str = "signed-in";

/// `count` readings, each differing from the one before it.
fn script(count: usize) -> Vec<(Value, Value)> {
    let rows: Vec<Vec<ScriptedRow>> = (0..count as i64)
        .map(|k| {
            let ammo = 1_000_000 - k * 1_000;
            vec![
                ("Universal Ammo", "CARRIED", ammo, ammo / 100),
                ("Common Dung", "STORAGE (Calypso)", 29, 0),
            ]
        })
        .collect();
    let readings: Vec<ScriptedReading> = rows
        .iter()
        .enumerate()
        .map(|(k, rows)| (25_000 - k as i64, rows.as_slice()))
        .collect();
    responses(&readings)
}

struct Rig {
    _dir: tempfile::TempDir,
    db: Db,
    bus: Arc<EventBus>,
    site: Arc<ScriptedSite>,
    flag: Arc<MemoryFlag>,
    clock: Arc<PacedClock>,
    changes: Arc<AtomicUsize>,
    recorder: Arc<AccountRecorder>,
}

/// A planned tracked session: its id and its span, in seconds from the
/// clock's start.
struct Planned<'a> {
    id: &'a str,
    start: f64,
    end: Option<f64>,
    active: bool,
}

async fn rig(readings: Vec<(Value, Value)>, connected_before: bool, plan: &[Planned<'_>]) -> Rig {
    rig_seeded(readings, connected_before, plan, |_| {}).await
}

/// [`rig`], with `seed` written before the connection starts.
async fn rig_seeded(
    readings: Vec<(Value, Value)>,
    connected_before: bool,
    plan: &[Planned<'_>],
    seed: impl FnOnce(&mut rusqlite::Connection) + Send + 'static,
) -> Rig {
    rig_built(
        ReadingMode::OnDemand,
        readings,
        connected_before,
        plan,
        seed,
    )
    .await
}

/// [`rig`], reading as the installed app does: always while connected.
async fn always_rig(readings: Vec<(Value, Value)>, plan: &[Planned<'_>]) -> Rig {
    rig_built(ReadingMode::Always, readings, false, plan, |_| {}).await
}

async fn rig_built(
    mode: ReadingMode,
    readings: Vec<(Value, Value)>,
    connected_before: bool,
    plan: &[Planned<'_>],
    seed: impl FnOnce(&mut rusqlite::Connection) + Send + 'static,
) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("account.db")).await.unwrap();
    let clock = Arc::new(PacedClock::new(
        NaiveDate::from_ymd_opt(2026, 7, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
    ));
    let rows: Vec<(String, f64, Option<f64>, bool)> = plan
        .iter()
        .map(|planned| {
            (
                planned.id.to_string(),
                clock.epoch_at(planned.start),
                planned.end.map(|end| clock.epoch_at(end)),
                planned.active,
            )
        })
        .collect();
    db.with_writer(move |conn| {
        for (id, started, ended, active) in rows {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, started, ended, i64::from(active)],
            )?;
        }
        seed(conn);
        Ok(())
    })
    .await
    .unwrap();
    let bus = Arc::new(EventBus::new());
    let site = Arc::new(ScriptedSite::new(COOKIE, readings));
    let flag = Arc::new(MemoryFlag::new(connected_before));
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = changes.clone();
    let recorder = AccountRecorder::start(
        &bus,
        RecorderParts {
            db: db.clone(),
            clock: clock.clone(),
            site: site.clone(),
            flag: flag.clone(),
            game_data: None,
            chat_log: None,
            changed: Some(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
            mode,
            store: Arc::new(super::adoption::MemoryOnly),
        },
        &tokio::runtime::Handle::current(),
    );
    Rig {
        _dir: dir,
        db,
        bus,
        site,
        flag,
        clock,
        changes,
        recorder,
    }
}

impl Rig {
    async fn connect(&self, cookie: &str) -> ConnectOutcome {
        self.recorder
            .connect(
                self.recorder.epoch(),
                SessionSecret::new(cookie.to_string()).unwrap(),
            )
            .await
    }

    /// Move on to `seconds` after the start. Unless the site is holding a
    /// request, the connection first finishes what it is doing: paused
    /// time would otherwise leap to this instant while the database works.
    async fn at(&self, seconds: f64) {
        if !self.site.is_held() {
            // Once shut down there is nothing left to finish.
            let _ = self
                .recorder
                .watch_status()
                .wait_for(|status| !status.working)
                .await;
        }
        tokio::time::sleep_until(self.clock.instant_at(seconds)).await;
    }

    fn start(&self, id: &str) {
        self.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: id.to_string(),
            }));
    }

    /// Start a session planned as not yet running the way the tracker
    /// does: its row is marked running, then the start is published.
    async fn start_running(&self, id: &str) {
        let marked = id.to_string();
        write(&self.db, move |conn| {
            conn.execute(
                "UPDATE tracking_sessions SET is_active = 1 WHERE id = ?1",
                [marked],
            )?;
            Ok(())
        })
        .await;
        self.start(id);
    }

    fn stop(&self, id: &str) {
        self.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: id.to_string(),
            }));
    }

    /// Wait until the connection shows `condition`. No timer guards the
    /// wait: one would let paused time leap to it while the database
    /// works. The test runner's own timeout catches a connection that
    /// never gets there.
    async fn until(&self, condition: impl Fn(&ConnectionStatus) -> bool) -> ConnectionStatus {
        let mut status = self.recorder.watch_status();
        let reached = status
            .wait_for(|status| condition(status))
            .await
            .expect("the connection task is running")
            .clone();
        reached
    }

    /// Every stored reading: when it was taken (seconds from the start),
    /// why, and the session it is tagged with.
    async fn stored(&self) -> Vec<(f64, String, Option<String>)> {
        let rows: Vec<(f64, String, Option<String>)> = read(&self.db, |conn| {
            let mut stmt = conn.prepare(
                // id-order: insertion
                "SELECT taken_at, purpose, session_id FROM account_readings ORDER BY id",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await;
        rows.into_iter()
            .map(|(at, purpose, session)| (self.clock.since_start(at), purpose, session))
            .collect()
    }

    async fn sealed_at(&self, session: &str) -> Option<f64> {
        let session = session.to_string();
        let sealed: Option<Option<f64>> = read(&self.db, move |conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT sealed_at FROM account_session_measures WHERE session_id = ?1",
                    [session],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .await;
        sealed
            .flatten()
            .map(|sealed| self.clock.since_start(sealed))
    }
}

/// Read the database without letting paused time move: a read awaited on
/// a runtime worker would let the clock leap to the connection's next
/// minute while the database works.
async fn read<T: Send + 'static>(
    db: &Db,
    job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, crate::db::DbError> + Send + 'static,
) -> T {
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.with_reader_blocking(job))
        .await
        .unwrap()
        .unwrap()
}

/// Write the database without letting paused time move.
async fn write(
    db: &Db,
    job: impl FnOnce(&mut rusqlite::Connection) -> Result<(), crate::db::DbError> + Send + 'static,
) {
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.with_writer_blocking(job))
        .await
        .unwrap()
        .unwrap();
}

fn tagged(at: f64, purpose: &str, session: Option<&str>) -> (f64, String, Option<String>) {
    (at, purpose.to_string(), session.map(str::to_string))
}

/// A stored session in memory, counting how often it is read.
struct FakeStore {
    value: Option<&'static str>,
    loads: AtomicUsize,
}

impl super::adoption::SessionStore for FakeStore {
    fn load(&self) -> Option<SessionSecret> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.value
            .map(|value| SessionSecret::new(value.to_string()).unwrap())
    }

    fn save(&self, _session: &SessionSecret) {}

    fn forget(&self) {}
}

fn store(value: Option<&'static str>) -> Arc<FakeStore> {
    Arc::new(FakeStore {
        value,
        loads: AtomicUsize::new(0),
    })
}

fn calls(rig: &Rig) -> usize {
    rig.site.calls().len()
}

/// The full readings asked for: each ends with its item read. The
/// keep-alive's balance checks are not readings.
fn item_reads(rig: &Rig) -> usize {
    rig.site
        .calls()
        .iter()
        .filter(|call| **call == ScriptedRead::Items)
        .count()
}

#[tokio::test(start_paused = true)]
async fn connecting_checks_the_session_once_and_reads_once() {
    let rig = rig(script(3), false, &[]).await;
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    let status = rig.until(|status| status.readings_stored == 1).await;
    assert_eq!(status.state, ConnectionState::Connected);
    assert!(status.has_connected);
    assert!(rig.flag.is_set());
    assert_eq!(status.last_reading_at, Some(rig.clock.epoch_at(0.0)));
    assert_eq!(status.connected_at, Some(rig.clock.epoch_at(0.0)));
    assert_eq!(status.reading_session, None);
    // The check's balance is the reading's: two requests in all.
    assert_eq!(
        rig.site.calls(),
        vec![ScriptedRead::Balance, ScriptedRead::Items]
    );
    // With nothing tracked, nothing more is read: only the keep-alive
    // asks for the balance.
    rig.at(3_600.0).await;
    assert_eq!(item_reads(&rig), 1);
    assert_eq!(rig.stored().await, vec![tagged(0.0, "connect", None)]);
    assert!(rig.changes.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(start_paused = true)]
async fn a_session_the_site_refuses_changes_nothing() {
    let rig = rig(script(1), false, &[]).await;
    assert_eq!(
        rig.connect("someone-else").await,
        ConnectOutcome::NotAccepted
    );
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
    assert!(!rig.flag.is_set());
    assert_eq!(rig.site.calls(), vec![ScriptedRead::Balance]);

    // A player who connected before starts out disconnected: nothing
    // holds a session yet. A refused sign-in leaves it so.
    let rig = self::rig(script(1), true, &[]).await;
    let status = rig.recorder.status();
    assert_eq!(status.state, ConnectionState::Disconnected);
    assert!(status.has_connected);
    assert_eq!(rig.connect("stale").await, ConnectOutcome::NotAccepted);
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
    assert!(rig.flag.is_set());
}

#[tokio::test(start_paused = true)]
async fn a_session_is_read_at_once_then_every_minute_then_closed_and_sealed() {
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: Some(270.0),
        active: false,
    }];
    let rig = rig(script(6), false, &plan).await;
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.at(120.0).await;
    rig.start("hunt");
    let status = rig.until(|status| status.readings_stored == 2).await;
    assert_eq!(status.reading_session.as_deref(), Some("hunt"));
    rig.at(270.0).await;
    rig.stop("hunt");
    // Two closing readings that differ from the one before them, then a
    // third: the session is sealed after it.
    rig.until(|status| status.reading_session.is_none() && status.readings_stored == 7)
        .await;
    rig.at(600.0).await;
    let hunt = Some("hunt");
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(120.0, "baseline", hunt),
            tagged(180.0, "session", hunt),
            tagged(240.0, "session", hunt),
            // 20 seconds after the stop, but not inside the floor.
            tagged(295.0, "session", hunt),
            tagged(355.0, "session", hunt),
            tagged(415.0, "session", hunt),
        ]
    );
    assert!(rig.sealed_at("hunt").await.is_some());
    let MeasureRead::Measured { measure, sealed } =
        rig.recorder.session_measure("hunt".into()).await.unwrap()
    else {
        panic!("the session has readings");
    };
    assert!(sealed);
    assert_eq!(measure.coverage.level, CoverageLevel::Complete);
    // Every reading tagged with the session: its closing readings too.
    assert_eq!(measure.coverage.reading_count, 6);
    assert_eq!(calls(&rig), 2 + 6 * 2);
    // Its costs were stored with the seal, and are what a read serves.
    let costs = rig
        .recorder
        .session_costs("hunt".into())
        .await
        .unwrap()
        .expect("a known session");
    assert_eq!(costs.status, Status::Measured);
    assert_eq!(stored_costs(&rig).await, 1);
}

/// How many sessions have stored costs.
async fn stored_costs(rig: &Rig) -> i64 {
    read(&rig.db, |conn| {
        Ok(
            conn.query_row("SELECT COUNT(*) FROM account_session_costs", [], |row| {
                row.get(0)
            })?,
        )
    })
    .await
}

/// What was published of a running session's costs so far: when they run
/// to, in seconds from the start, and the weapon's measured amount.
async fn published(rig: &Rig, session: &str) -> Option<(f64, Option<i64>)> {
    let session = session.to_string();
    let row: Option<(f64, Option<i64>)> = read(&rig.db, move |conn| {
        use rusqlite::OptionalExtension as _;
        Ok(conn
            .query_row(
                "SELECT measured_through, weapon_micro_ped FROM account_live_session_costs \
                 WHERE session_id = ?1",
                [session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    })
    .await;
    row.map(|(at, weapon)| (rig.clock.since_start(at), weapon))
}

#[tokio::test(start_paused = true)]
async fn a_running_session_s_costs_so_far_are_published_after_each_of_its_readings() {
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(6), false, &plan).await;
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.at(120.0).await;
    rig.start_running("hunt").await;
    rig.until(|status| status.readings_stored == 2).await;
    rig.at(150.0).await;
    // The baseline alone measures nothing yet.
    assert_eq!(published(&rig, "hunt").await, None);
    rig.until(|status| status.readings_stored == 3).await;
    rig.at(210.0).await;
    // Up to the reading at 180: a thousand rounds burned, 0.10 PED.
    assert_eq!(published(&rig, "hunt").await, Some((180.0, Some(100_000))));
    rig.until(|status| status.readings_stored == 4).await;
    rig.at(270.0).await;
    assert_eq!(published(&rig, "hunt").await, Some((240.0, Some(200_000))));
}

#[tokio::test(start_paused = true)]
async fn costs_a_sealed_session_lacks_are_rebuilt_when_the_connection_starts() {
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: Some(270.0),
        active: false,
    }];
    // A session sealed before its costs were kept: its measure is stored,
    // its costs are not.
    let rig = rig_seeded(script(1), false, &plan, |conn| {
        let base = crate::time::naive_to_epoch(
            NaiveDate::from_ymd_opt(2026, 7, 1)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        for (at, ammo) in [
            (110.0, 100_000),
            (180.0, 90_000),
            (240.0, 80_000),
            (290.0, 70_000),
        ] {
            let reading = super::test_support::reading(
                base + at,
                1_000,
                &[(
                    "Universal Ammo",
                    super::reading::Container::Carried,
                    ammo,
                    ammo / 100,
                )],
            );
            let tag = (at > 120.0).then_some("hunt");
            super::store::record_reading(conn, &reading, super::store::Purpose::Session, tag)
                .unwrap();
        }
        let seal = super::sessions::measure_to_seal(conn, "hunt", None, None, base + 400.0)
            .unwrap()
            .expect("an ended session with readings");
        assert_eq!(
            super::sessions::cache_sealed(conn, &[seal], base + 400.0).unwrap(),
            1
        );
        super::costs::clear(conn, "hunt").unwrap();
    })
    .await;
    // Anything asked of the connection is answered once it has started.
    rig.recorder.disconnect().await;
    assert_eq!(stored_costs(&rig).await, 1);
}

/// A session sealed while its end was unread and nothing had been read
/// since keeps no costs; the next reading stored outside any session
/// closes its gap, and its costs are then weighed and stored.
#[tokio::test(start_paused = true)]
async fn a_reading_outside_any_session_lets_a_sealed_session_s_gap_be_weighed() {
    let plan = [Planned {
        id: "hunt",
        start: -600.0,
        end: Some(-300.0),
        active: false,
    }];
    let rig = rig_seeded(script(1), false, &plan, |conn| {
        let base = crate::time::naive_to_epoch(
            NaiveDate::from_ymd_opt(2026, 7, 1)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        // Read from before its start until the sign-in dropped.
        for (at, ammo) in [(-605.0, 100_000), (-540.0, 90_000), (-480.0, 80_000)] {
            let reading = super::test_support::reading(
                base + at,
                1_000,
                &[(
                    "Universal Ammo",
                    super::reading::Container::Carried,
                    ammo,
                    ammo / 100,
                )],
            );
            let tag = (at > -600.0).then_some("hunt");
            super::store::record_reading(conn, &reading, super::store::Purpose::Session, tag)
                .unwrap();
        }
        let seal = super::sessions::measure_to_seal(conn, "hunt", None, None, base - 100.0)
            .unwrap()
            .expect("an ended session with readings");
        assert_eq!(
            super::sessions::cache_sealed(conn, &[seal], base - 100.0).unwrap(),
            1
        );
    })
    .await;
    rig.at(1.0).await;
    assert_eq!(stored_costs(&rig).await, 0, "its gap waits for a reading");
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.until(|status| status.readings_stored == 1).await;
    rig.at(10.0).await;
    // The connect reading closes the gap; with no chat log here the gap is
    // not billed, but it is weighed, and the costs are kept.
    assert_eq!(stored_costs(&rig).await, 1);
}

#[tokio::test(start_paused = true)]
async fn a_closing_reading_identical_to_the_one_before_it_seals_at_once() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: Some(100.0),
        active: false,
    }];
    // Two readings: the connect, the baseline, then the site at rest.
    let rig = rig(script(2), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    rig.at(100.0).await;
    rig.stop("hunt");
    rig.until(|status| status.reading_session.is_none() && status.readings_stored == 3)
        .await;
    rig.at(400.0).await;
    let hunt = Some("hunt");
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "baseline", hunt),
            tagged(120.0, "session", hunt),
        ]
    );
    assert!(rig.sealed_at("hunt").await.is_some());
}

#[tokio::test(start_paused = true)]
async fn the_minute_floor_holds_whatever_asks_for_a_reading() {
    let plan = [Planned {
        id: "hunt",
        start: 10.0,
        end: Some(75.0),
        active: false,
    }];
    let rig = rig(script(6), false, &plan).await;
    rig.connect(COOKIE).await;
    // A session starting ten seconds after the connect reading is answered
    // by that reading: no request, and no baseline of its own.
    rig.at(10.0).await;
    rig.start("hunt");
    rig.at(69.0).await;
    assert_eq!(calls(&rig), 2);
    // Its next minute falls a minute after it asked.
    rig.at(71.0).await;
    assert_eq!(calls(&rig), 4);
    // Stopped five seconds later: the closing reading waits for the floor
    // (55 seconds after 70), not just 20 seconds.
    rig.at(75.0).await;
    rig.stop("hunt");
    rig.at(124.0).await;
    assert_eq!(calls(&rig), 4);
    rig.until(|status| status.readings_stored == 3).await;
    // Connecting again inside the floor checks the session but reads no
    // items: the latest stored reading stands.
    rig.at(130.0).await;
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.at(150.0).await;
    let stored = rig.stored().await;
    assert_eq!(
        stored,
        vec![
            tagged(0.0, "connect", None),
            tagged(70.0, "session", Some("hunt")),
            tagged(125.0, "session", Some("hunt")),
        ]
    );
    assert_eq!(calls(&rig), 7);
    assert_eq!(rig.site.calls()[6], ScriptedRead::Balance);
}

#[tokio::test(start_paused = true)]
async fn at_most_one_check_starts_every_ten_seconds() {
    let rig = rig(script(3), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(2.0).await;
    let again = {
        let recorder = rig.recorder.clone();
        tokio::spawn(async move {
            recorder
                .connect(
                    recorder.epoch(),
                    SessionSecret::new(COOKIE.to_string()).unwrap(),
                )
                .await
        })
    };
    tokio::time::sleep_until(rig.clock.instant_at(9.0)).await;
    assert_eq!(calls(&rig), 2, "the second check waits for the bound");
    tokio::time::sleep_until(rig.clock.instant_at(11.0)).await;
    assert_eq!(calls(&rig), 3);
    assert_eq!(again.await.unwrap(), ConnectOutcome::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_reading_that_really_returns_after_a_disconnect_is_dropped() {
    let plan = [Planned {
        id: "hunt",
        start: 0.0,
        end: None,
        active: true,
    }];
    let rig = rig(script(6), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.until(|status| status.readings_stored == 1).await;
    assert_eq!(
        rig.recorder.status().reading_session.as_deref(),
        Some("hunt"),
        "a session still marked running is read once connected"
    );
    rig.site.hold();
    // The minute falls due and its request waits on the site.
    rig.at(61.0).await;
    // The site answers, and the disconnect reaches the connection with the
    // answer: the answer is met by the identity check, not aborted.
    rig.site.release();
    rig.recorder.disconnect().await;
    rig.at(600.0).await;
    assert_eq!(rig.site.readings_served(), 2, "the site did answer");
    let status = rig.recorder.status();
    assert_eq!(status.state, ConnectionState::Disconnected);
    assert!(!status.has_connected);
    assert!(!rig.flag.is_set());
    assert_eq!(status.readings_stored, 1);
    assert_eq!(rig.stored().await.len(), 1);
    assert_eq!(calls(&rig), 4, "and nothing was asked since");
}

#[tokio::test(start_paused = true)]
async fn a_connect_begun_before_a_disconnect_cannot_undo_it() {
    let rig = rig(script(3), false, &[]).await;
    // Begun before, delivered after.
    let before = rig.recorder.epoch();
    rig.recorder.disconnect().await;
    let late = rig
        .recorder
        .connect(before, SessionSecret::new(COOKIE.to_string()).unwrap())
        .await;
    assert_eq!(late, ConnectOutcome::Interrupted);
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
    assert!(rig.site.calls().is_empty());

    // Being checked when the disconnect arrives.
    rig.site.hold();
    let checking = {
        let recorder = rig.recorder.clone();
        tokio::spawn(async move {
            recorder
                .connect(
                    recorder.epoch(),
                    SessionSecret::new(COOKIE.to_string()).unwrap(),
                )
                .await
        })
    };
    tokio::time::sleep_until(rig.clock.instant_at(1.0)).await;
    rig.recorder.disconnect().await;
    rig.site.release();
    assert_eq!(checking.await.unwrap(), ConnectOutcome::Interrupted);
    rig.at(100.0).await;
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
    assert!(!rig.flag.is_set());
    assert!(rig.site.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_reading_for_a_stopped_session_not_yet_sealed_is_kept() {
    let plan = [
        Planned {
            id: "first",
            start: 120.0,
            end: Some(130.0),
            active: false,
        },
        Planned {
            id: "second",
            start: 150.0,
            end: None,
            active: false,
        },
    ];
    let rig = rig(script(6), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.until(|status| status.readings_stored == 1).await;
    rig.site.hold();
    rig.at(120.0).await;
    rig.start("first");
    rig.at(130.0).await;
    rig.stop("first");
    rig.at(150.0).await;
    rig.start("second");
    rig.at(160.0).await;
    rig.site.release();
    rig.until(|status| status.readings_stored == 3).await;
    rig.recorder.shutdown().await;
    // The first session's baseline came back after it had stopped and a
    // new session had started, but before the first was sealed: it is
    // kept, tagged with the first, and sealed into its measure. The new
    // session's baseline (due when it started, at 150) waited for that
    // request and then fell inside its floor, so its first reading is the
    // minute after, due at 210.
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(160.0, "baseline", Some("first")),
            tagged(210.0, "session", Some("second")),
        ]
    );
    assert!(rig.sealed_at("first").await.is_some());
    // The kept reading was taken after the second session began, so it
    // cannot read the second's start: honestly unread.
    let MeasureRead::Measured { measure, .. } =
        rig.recorder.session_measure("second".into()).await.unwrap()
    else {
        panic!("the second session has readings");
    };
    assert_eq!(measure.coverage.opening, Edge::Unread);
}

#[tokio::test(start_paused = true)]
async fn a_reading_for_a_session_deleted_meanwhile_is_not_kept() {
    let plan = [Planned {
        id: "gone",
        start: 120.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(6), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.until(|status| status.readings_stored == 1).await;
    rig.site.hold();
    rig.at(120.0).await;
    rig.start("gone");
    rig.at(125.0).await;
    write(&rig.db, |conn| {
        conn.execute("DELETE FROM tracking_sessions WHERE id = 'gone'", [])?;
        Ok(())
    })
    .await;
    rig.site.release();
    rig.at(130.0).await;
    rig.recorder.shutdown().await;
    assert_eq!(rig.site.readings_served(), 2, "the site did answer");
    assert_eq!(rig.stored().await, vec![tagged(0.0, "connect", None)]);
}

#[tokio::test(start_paused = true)]
async fn a_stop_before_the_baseline_returns_keeps_it_and_closes_the_session() {
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: Some(125.0),
        active: false,
    }];
    let rig = rig(script(3), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.until(|status| status.readings_stored == 1).await;
    rig.site.hold();
    rig.at(120.0).await;
    rig.start("hunt");
    rig.at(125.0).await;
    rig.stop("hunt");
    rig.at(130.0).await;
    rig.site.release();
    rig.until(|status| status.reading_session.is_none() && status.readings_stored == 4)
        .await;
    rig.at(600.0).await;
    let hunt = Some("hunt");
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            // Taken when it answered, after the stop.
            tagged(130.0, "baseline", hunt),
            tagged(175.0, "session", hunt),
            tagged(235.0, "session", hunt),
        ]
    );
    assert!(rig.sealed_at("hunt").await.is_some());
    let MeasureRead::Measured { measure, .. } =
        rig.recorder.session_measure("hunt".into()).await.unwrap()
    else {
        panic!("the session has readings");
    };
    // Its start went unread: honestly partial.
    assert_eq!(measure.coverage.opening, Edge::Unread);
    assert_eq!(measure.coverage.closing, Edge::Read);
    assert_eq!(measure.coverage.level, CoverageLevel::Partial);
}

/// Connect during a running session that stops at 20 seconds, in the same
/// instant the check answers: the stop first, or the answer first. What is
/// stored for the session, and its sealed measure.
async fn connect_around_a_stop(
    stop_first: bool,
) -> (
    Vec<(f64, String, Option<String>)>,
    super::measure::SessionMeasure,
) {
    let plan = [Planned {
        id: "hunt",
        start: 5.0,
        end: Some(20.0),
        active: false,
    }];
    let rig = rig(script(3), false, &plan).await;
    rig.at(5.0).await;
    rig.start("hunt");
    rig.site.hold();
    let connecting = {
        let recorder = rig.recorder.clone();
        tokio::spawn(async move {
            recorder
                .connect(
                    recorder.epoch(),
                    SessionSecret::new(COOKIE.to_string()).unwrap(),
                )
                .await
        })
    };
    tokio::time::sleep_until(rig.clock.instant_at(20.0)).await;
    if stop_first {
        rig.stop("hunt");
        rig.site.release();
    } else {
        rig.site.release();
        rig.until(|status| status.state == ConnectionState::Connected)
            .await;
        rig.stop("hunt");
    }
    assert_eq!(connecting.await.unwrap(), ConnectOutcome::Connected);
    rig.until(|status| status.reading_session.is_none() && !status.working)
        .await;
    rig.at(1_000.0).await;
    assert!(rig.sealed_at("hunt").await.is_some());
    let MeasureRead::Measured { measure, sealed } =
        rig.recorder.session_measure("hunt".into()).await.unwrap()
    else {
        panic!("the session has readings");
    };
    assert!(sealed);
    let stored = rig.stored().await;
    // Its keep-alive would otherwise move paused time under the next rig.
    rig.recorder.shutdown().await;
    (stored, *measure)
}

#[tokio::test(start_paused = true)]
async fn a_stop_while_a_connect_is_checked_comes_out_as_if_the_answer_came_first() {
    let stopped_first = connect_around_a_stop(true).await;
    let answered_first = connect_around_a_stop(false).await;
    assert_eq!(stopped_first, answered_first);
    let hunt = Some("hunt");
    // The connect reading, then the closing readings any stopped session
    // gets (the first held to the floor of the check that began at 5).
    assert_eq!(
        stopped_first.0,
        vec![
            tagged(20.0, "connect", hunt),
            tagged(60.0, "session", hunt),
            tagged(120.0, "session", hunt),
            tagged(180.0, "session", hunt),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_stop_while_a_refused_connect_is_checked_seals_with_what_there_is() {
    let plan = [Planned {
        id: "hunt",
        start: 5.0,
        end: Some(20.0),
        active: false,
    }];
    let rig = rig(script(3), false, &plan).await;
    rig.at(5.0).await;
    rig.start("hunt");
    rig.site.hold();
    let connecting = {
        let recorder = rig.recorder.clone();
        tokio::spawn(async move {
            recorder
                .connect(
                    recorder.epoch(),
                    SessionSecret::new("someone-else".to_string()).unwrap(),
                )
                .await
        })
    };
    tokio::time::sleep_until(rig.clock.instant_at(20.0)).await;
    rig.stop("hunt");
    rig.site.release();
    assert_eq!(connecting.await.unwrap(), ConnectOutcome::NotAccepted);
    rig.at(100.0).await;
    let status = rig.recorder.status();
    assert_eq!(status.state, ConnectionState::Disconnected);
    assert_eq!(status.tracked_session, None);
    assert!(!status.working, "nothing waits on the stopped session");
    assert!(rig.stored().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn connecting_mid_session_is_partial_however_early() {
    let plan = [Planned {
        id: "hunt",
        start: 0.0,
        end: Some(300.0),
        active: false,
    }];
    let rig = rig(script(8), false, &plan).await;
    rig.start("hunt");
    rig.at(5.0).await;
    assert!(
        rig.site.calls().is_empty(),
        "nothing is read before connecting"
    );
    rig.connect(COOKIE).await;
    rig.at(300.0).await;
    rig.stop("hunt");
    rig.until(|status| status.reading_session.is_none() && status.readings_stored >= 5)
        .await;
    rig.at(900.0).await;
    let hunt = Some("hunt");
    let stored = rig.stored().await;
    assert_eq!(
        stored[..5],
        [
            tagged(5.0, "connect", hunt),
            tagged(65.0, "session", hunt),
            tagged(125.0, "session", hunt),
            tagged(185.0, "session", hunt),
            tagged(245.0, "session", hunt),
        ]
    );
    let MeasureRead::Measured { measure, sealed } =
        rig.recorder.session_measure("hunt".into()).await.unwrap()
    else {
        panic!("the session has readings");
    };
    assert!(sealed);
    assert_eq!(measure.coverage.opening, Edge::Unread);
    assert_eq!(measure.coverage.level, CoverageLevel::Partial);
}

#[tokio::test(start_paused = true)]
async fn an_unauthorised_answer_mid_session_expires_the_connection_quietly() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: Some(400.0),
        active: false,
    }];
    let rig = rig(script(8), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    // Calls 0 and 1 connect; 2 and 3 the baseline; 4 and 5 the next
    // minute; the balance read of the minute after that is refused.
    rig.site.fail_at(6, ScriptedFault::Unauthorised);
    let status = rig
        .until(|status| status.state == ConnectionState::Expired)
        .await;
    assert!(status.has_connected, "expiry is not a disconnect");
    assert!(rig.flag.is_set());
    assert_eq!(status.reading_session, None);
    assert_eq!(calls(&rig), 7);
    rig.at(400.0).await;
    rig.stop("hunt");
    rig.at(2_000.0).await;
    assert_eq!(calls(&rig), 7, "nothing is read once expired");
    let MeasureRead::Measured { measure, sealed } =
        rig.recorder.session_measure("hunt".into()).await.unwrap()
    else {
        panic!("the session has readings");
    };
    assert!(sealed);
    assert_eq!(measure.coverage.closing, Edge::Unread);
    assert_eq!(measure.coverage.level, CoverageLevel::Partial);
}

#[tokio::test(start_paused = true)]
async fn signing_in_again_after_an_expiry_reads_the_session_on() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(8), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    rig.site.fail_at(4, ScriptedFault::Unauthorised);
    rig.until(|status| status.state == ConnectionState::Expired)
        .await;
    rig.at(130.0).await;
    rig.site.accept("renewed");
    assert_eq!(rig.connect("renewed").await, ConnectOutcome::Connected);
    rig.at(200.0).await;
    let hunt = Some("hunt");
    let stored = rig.stored().await;
    // The refused minute at 120 had reached the site, so its floor holds
    // the new connection's own reading back; the session reads on at 190.
    assert_eq!(
        stored,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "baseline", hunt),
            tagged(190.0, "session", hunt),
        ]
    );
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_failed_reading_skips_its_minute_without_a_retry() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(8), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    rig.site.fail_at(4, ScriptedFault::Network);
    rig.site.fail_at(6, ScriptedFault::Malformed);
    rig.at(250.0).await;
    let status = rig.recorder.status();
    assert_eq!(status.state, ConnectionState::Connected);
    rig.recorder.shutdown().await;
    let hunt = Some("hunt");
    // The minute at 120 failed on its balance read, the one at 180 on its
    // item read: one attempt each, and the minute at 240 read as usual.
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "baseline", hunt),
            tagged(240.0, "session", hunt),
        ]
    );
    assert_eq!(calls(&rig), 2 + 2 + 1 + 2 + 2);
}

#[tokio::test(start_paused = true)]
async fn three_failures_in_a_row_fall_back_to_every_fifth_minute_until_one_succeeds() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(8), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    for call in [4, 5, 6] {
        rig.site.fail_at(call, ScriptedFault::Network);
    }
    // The minutes at 120, 180 and 240 fail; the next is due five minutes
    // after the last of them.
    rig.at(539.0).await;
    assert_eq!(calls(&rig), 7);
    rig.at(541.0).await;
    assert_eq!(calls(&rig), 9);
    // It succeeded: back to every minute.
    rig.at(601.0).await;
    assert_eq!(calls(&rig), 11);
}

#[tokio::test(start_paused = true)]
async fn shutting_down_abandons_a_request_in_flight_and_leaves_the_bus() {
    let plan = [Planned {
        id: "hunt",
        start: 0.0,
        end: None,
        active: true,
    }];
    let rig = rig(script(4), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.site.hold();
    rig.at(61.0).await;
    let asked = tokio::time::Instant::now();
    rig.recorder.shutdown().await;
    assert!(asked.elapsed() < Duration::from_secs(1));
    assert!(!rig.bus.has_subscribers(Topic::SessionStarted));
    assert!(!rig.bus.has_subscribers(Topic::SessionStopped));
    rig.site.release();
    rig.at(600.0).await;
    assert_eq!(calls(&rig), 2);
    assert_eq!(
        rig.connect(COOKIE).await,
        ConnectOutcome::Interrupted,
        "nothing answers once shut down"
    );
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_changes_is_announced_once() {
    let rig = rig(script(1), false, &[]).await;
    for session in ["a", "b", "c"] {
        rig.start(session);
        rig.stop(session);
    }
    rig.at(1.0).await;
    assert_eq!(rig.changes.load(Ordering::SeqCst), 1);
    rig.start("d");
    rig.at(2.0).await;
    assert_eq!(rig.changes.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn a_stored_session_is_confirmed_refused_or_found_missing() {
    // Never connected before: nothing is read from the store at all.
    let never = rig(script(2), false, &[]).await;
    let kept = store(Some(COOKIE));
    super::adoption::confirm_stored_session(never.recorder.clone(), kept.clone()).await;
    assert_eq!(kept.loads.load(Ordering::SeqCst), 0);
    // The app's own store holds nothing: a player who connected before
    // starts disconnected, offered a sign-in.
    let before = rig(script(2), true, &[]).await;
    super::adoption::confirm_stored_session(
        before.recorder.clone(),
        Arc::new(super::adoption::MemoryOnly),
    )
    .await;
    before.at(1.0).await;
    assert_eq!(
        before.recorder.status().state,
        ConnectionState::Disconnected
    );
    assert!(before.recorder.status().has_connected);

    for (stored, expected, loads) in [
        (Some(COOKIE), ConnectionState::Connected, 1),
        // An unconfirmed stored session the site refuses is expired.
        (Some("stale"), ConnectionState::Expired, 1),
        // With nothing stored, nothing changes.
        (None, ConnectionState::Disconnected, 1),
    ] {
        let rig = rig(script(2), true, &[]).await;
        let kept = store(stored);
        super::adoption::confirm_stored_session(rig.recorder.clone(), kept.clone()).await;
        rig.at(1.0).await;
        assert_eq!(rig.recorder.status().state, expected, "{stored:?}");
        assert_eq!(kept.loads.load(Ordering::SeqCst), loads);
    }
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_site_leaves_the_stored_session_unconfirmed_and_tried_again() {
    let plan = [Planned {
        id: "hunt",
        start: 400.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(3), true, &plan).await;
    for call in [0, 1, 2] {
        rig.site.fail_at(call, ScriptedFault::Network);
    }
    let kept = store(Some(COOKIE));
    let confirming = tokio::spawn(super::adoption::confirm_stored_session(
        rig.recorder.clone(),
        kept.clone(),
    ));
    let loads = || kept.loads.load(Ordering::SeqCst);
    rig.at(29.0).await;
    assert_eq!(loads(), 1);
    assert_eq!(rig.recorder.status().state, ConnectionState::Unconfirmed);
    rig.at(31.0).await;
    assert_eq!(loads(), 2, "again after 30 seconds");
    rig.at(149.0).await;
    assert_eq!(loads(), 2);
    rig.at(151.0).await;
    assert_eq!(loads(), 3, "then after 2 minutes");
    // Then every 10 minutes, or at once when a session starts.
    rig.at(400.0).await;
    assert_eq!(loads(), 3);
    rig.start("hunt");
    rig.until(|status| status.state == ConnectionState::Connected)
        .await;
    assert_eq!(loads(), 4);
    confirming.await.unwrap();
    rig.at(500.0).await;
    let stored = rig.stored().await;
    assert_eq!(stored[0], tagged(400.0, "connect", Some("hunt")));
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_ends_the_trying() {
    let rig = rig(script(3), true, &[]).await;
    rig.site.fail_at(0, ScriptedFault::Network);
    let kept = store(Some(COOKIE));
    let confirming = tokio::spawn(super::adoption::confirm_stored_session(
        rig.recorder.clone(),
        kept.clone(),
    ));
    rig.at(10.0).await;
    rig.recorder.disconnect().await;
    confirming.await.unwrap();
    rig.at(3_600.0).await;
    assert_eq!(kept.loads.load(Ordering::SeqCst), 1);
    assert_eq!(rig.recorder.status().state, ConnectionState::Disconnected);
}

#[test]
fn the_stored_session_is_tried_again_after_30_seconds_2_minutes_then_every_10() {
    use super::adoption::retry_after;
    assert_eq!(retry_after(1), Duration::from_secs(30));
    assert_eq!(retry_after(2), Duration::from_secs(120));
    assert_eq!(retry_after(3), Duration::from_secs(600));
    assert_eq!(retry_after(9), Duration::from_secs(600));
}

/// Fail the minutes at 120 and 180 of a session that started at 60, then
/// `between` at 200, then fail one more minute: the minute after it falls
/// a minute later only if the failures were counted afresh.
async fn failures_counted_afresh_after(
    between: impl AsyncFnOnce(&Rig),
    expected_calls_at_321: usize,
) {
    let plan = [
        Planned {
            id: "hunt",
            start: 60.0,
            end: None,
            active: false,
        },
        Planned {
            id: "next",
            start: 200.0,
            end: None,
            active: false,
        },
    ];
    let rig = rig(script(12), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    rig.site.fail_at(4, ScriptedFault::Network);
    rig.site.fail_at(5, ScriptedFault::Network);
    rig.at(200.0).await;
    between(&rig).await;
    // The next minute (at 260) fails too: a third failure only if nothing
    // above counted afresh.
    let next = calls(&rig);
    rig.site.fail_at(next, ScriptedFault::Network);
    rig.at(321.0).await;
    assert_eq!(calls(&rig), expected_calls_at_321);
}

#[tokio::test(start_paused = true)]
async fn the_failures_are_counted_afresh_for_a_new_session() {
    failures_counted_afresh_after(
        async |rig: &Rig| {
            rig.stop("hunt");
            rig.start("next");
        },
        // 2 connect, 2 baseline, 2 failed minutes, 1 failed at 260, then
        // the minute at 320 read.
        2 + 2 + 2 + 1 + 2,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn the_failures_are_counted_afresh_for_a_new_connection() {
    failures_counted_afresh_after(
        async |rig: &Rig| {
            assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
        },
        // The check at 200 is a call of its own.
        2 + 2 + 2 + 1 + 1 + 2,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn the_failures_are_counted_afresh_after_a_disconnect() {
    failures_counted_afresh_after(
        async |rig: &Rig| {
            rig.recorder.disconnect().await;
            assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
        },
        2 + 2 + 2 + 1 + 1 + 2,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn the_failures_are_counted_afresh_after_an_expiry() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: None,
        active: false,
    }];
    let rig = rig(script(12), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    // The minutes at 120 and 180 fail; the site refuses the one at 240.
    rig.site.fail_at(4, ScriptedFault::Network);
    rig.site.fail_at(5, ScriptedFault::Network);
    rig.site.fail_at(6, ScriptedFault::Unauthorised);
    rig.until(|status| status.state == ConnectionState::Expired)
        .await;
    // The player signs in again at 250; its reading waits out the floor
    // of the refused minute, so the session's next minute falls at 310.
    rig.at(250.0).await;
    rig.site.accept(COOKIE);
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.site.fail_at(8, ScriptedFault::Network);
    // That minute fails: the first of the new connection, so the next
    // falls a minute later, not five.
    rig.at(371.0).await;
    assert_eq!(calls(&rig), 11);
}

#[tokio::test(start_paused = true)]
async fn a_sign_in_the_site_refuses_leaves_an_unconfirmed_session_to_its_retries() {
    let rig = rig(script(3), true, &[]).await;
    rig.site.fail_at(0, ScriptedFault::Network);
    let kept = store(Some(COOKIE));
    let confirming = tokio::spawn(super::adoption::confirm_stored_session(
        rig.recorder.clone(),
        kept.clone(),
    ));
    rig.at(10.0).await;
    assert_eq!(rig.recorder.status().state, ConnectionState::Unconfirmed);
    // A sign-in through the window that the site refuses is not the
    // stored session: nothing changes, and the retries go on.
    assert_eq!(
        rig.connect("someone-else").await,
        ConnectOutcome::NotAccepted
    );
    assert_eq!(rig.recorder.status().state, ConnectionState::Unconfirmed);
    rig.at(31.0).await;
    confirming.await.unwrap();
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
    assert_eq!(kept.loads.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn taking_up_the_stored_session_does_nothing_once_the_player_has_signed_in() {
    let rig = rig(script(3), true, &[]).await;
    assert_eq!(rig.connect(COOKIE).await, ConnectOutcome::Connected);
    rig.at(1.0).await;
    let kept = store(Some("kept-elsewhere"));
    super::adoption::confirm_stored_session(rig.recorder.clone(), kept.clone()).await;
    rig.at(20.0).await;
    assert_eq!(kept.loads.load(Ordering::SeqCst), 0);
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
    // Nor is a stored session taken while connected, however it arrives.
    assert_eq!(
        rig.recorder
            .connect_stored(
                rig.recorder.epoch(),
                SessionSecret::new("kept-elsewhere".into()).unwrap()
            )
            .await,
        ConnectOutcome::Interrupted
    );
    assert_eq!(calls(&rig), 2);
}

#[tokio::test(start_paused = true)]
async fn the_sign_in_window_shows_in_the_status_and_is_announced() {
    let rig = rig(script(1), false, &[]).await;
    assert!(!rig.recorder.status().signing_in);
    rig.recorder.set_signing_in(true);
    rig.until(|status| status.signing_in).await;
    rig.at(1.0).await;
    assert_eq!(rig.changes.load(Ordering::SeqCst), 1);
    // Told twice, announced once.
    rig.recorder.set_signing_in(true);
    rig.at(2.0).await;
    assert_eq!(rig.changes.load(Ordering::SeqCst), 1);
    rig.recorder.set_signing_in(false);
    rig.until(|status| !status.signing_in).await;
    rig.at(3.0).await;
    assert_eq!(rig.changes.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn an_unconfirmed_session_says_whether_the_site_could_be_reached() {
    use super::recorder::Confirming;
    let rig = rig(script(3), true, &[]).await;
    assert_eq!(
        rig.recorder.status().confirming,
        None,
        "only while unconfirmed"
    );
    rig.site.fail_at(0, ScriptedFault::Network);
    rig.site.hold();
    let confirming = tokio::spawn(super::adoption::confirm_stored_session(
        rig.recorder.clone(),
        store(Some(COOKIE)),
    ));
    rig.until(|status| status.state == ConnectionState::Unconfirmed)
        .await;
    assert_eq!(rig.recorder.status().confirming, Some(Confirming::Checking));
    rig.site.release();
    rig.until(|status| status.confirming == Some(Confirming::SiteUnreachable))
        .await;
    // Tried again after 30 seconds, and reached.
    rig.at(31.0).await;
    confirming.await.unwrap();
    let status = rig.recorder.status();
    assert_eq!(status.state, ConnectionState::Connected);
    assert_eq!(status.confirming, None);
}

#[tokio::test(start_paused = true)]
async fn whether_any_reading_was_ever_stored_outlives_the_connection() {
    let rig = rig(script(2), false, &[]).await;
    assert!(!rig.recorder.status().has_readings);
    rig.connect(COOKIE).await;
    rig.until(|status| status.has_readings).await;
    rig.recorder.disconnect().await;
    assert!(rig.recorder.status().has_readings);
    rig.recorder.shutdown().await;
    // A later run reads it from the store.
    let later = AccountRecorder::start(
        &rig.bus,
        RecorderParts {
            db: rig.db.clone(),
            clock: rig.clock.clone(),
            site: rig.site.clone(),
            flag: rig.flag.clone(),
            game_data: None,
            chat_log: None,
            changed: None,
            mode: ReadingMode::OnDemand,
            store: Arc::new(super::adoption::MemoryOnly),
        },
        &tokio::runtime::Handle::current(),
    );
    let mut status = later.watch_status();
    let seen = status
        .wait_for(|status| status.has_readings)
        .await
        .map(|status| status.has_readings);
    assert_eq!(seen.ok(), Some(true));
}

fn read_at(rig: &Rig, reading: super::recorder::RunReading) -> Option<f64> {
    match reading {
        super::recorder::RunReading::Read { taken_at, .. } => Some(rig.clock.since_start(taken_at)),
        _ => None,
    }
}

#[tokio::test(start_paused = true)]
async fn a_run_reads_from_its_start_every_minute_until_done_and_never_inside_the_floor() {
    let rig = rig(script(10), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(10.0).await;
    // The connect reading, ten seconds old, is from before the boxes.
    let begun = rig.recorder.run_begin().await;
    assert_eq!(read_at(&rig, begun), Some(0.0));
    assert_eq!(calls(&rig), 2, "no reading inside the floor");
    rig.at(59.0).await;
    assert_eq!(calls(&rig), 2);
    rig.at(61.0).await;
    assert_eq!(calls(&rig), 4, "a minute after the last");
    // Done at 70: the floor holds it until 115.
    rig.at(70.0).await;
    let recorder = rig.recorder.clone();
    let done = tokio::spawn(async move { recorder.run_done().await });
    rig.at(114.0).await;
    assert_eq!(calls(&rig), 4);
    rig.at(116.0).await;
    let done = done.await.unwrap();
    assert_eq!(read_at(&rig, done), Some(115.0));
    // Then reading stops.
    rig.at(600.0).await;
    assert_eq!(item_reads(&rig), 3);
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "run", None),
            tagged(115.0, "run", None),
        ]
    );
    // Every stored reading was examined.
    let examined: i64 = read(&rig.db, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM account_readings_examined",
            [],
            |row| row.get(0),
        )?)
    })
    .await;
    assert_eq!(examined, 3);
}

/// How many stored readings' steps have been looked at for the stock.
async fn steps_followed(rig: &Rig) -> i64 {
    read(&rig.db, |conn| {
        Ok(
            conn.query_row("SELECT COUNT(*) FROM account_steps_recognised", [], |row| {
                row.get(0)
            })?,
        )
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn no_step_is_followed_for_the_stock_while_a_strongbox_run_is_open() {
    let rig = rig(script(10), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(10.0).await;
    assert_eq!(steps_followed(&rig).await, 1, "the connect reading");
    let begun = rig.recorder.run_begin().await;
    assert_eq!(read_at(&rig, begun), Some(0.0));
    // The run's reading at 60 waits while the run is open.
    rig.at(61.0).await;
    assert_eq!(rig.stored().await.len(), 2);
    assert_eq!(steps_followed(&rig).await, 1);
    rig.at(70.0).await;
    let recorder = rig.recorder.clone();
    let done = tokio::spawn(async move { recorder.run_done().await });
    rig.at(116.0).await;
    done.await.unwrap();
    // Once the run is over, every step is looked at.
    rig.at(120.0).await;
    assert_eq!(steps_followed(&rig).await, 3);
}

#[tokio::test(start_paused = true)]
async fn a_run_needs_a_connection_and_ends_with_it() {
    use super::recorder::RunReading;
    let rig = rig(script(10), false, &[]).await;
    assert_eq!(rig.recorder.run_begin().await, RunReading::NotConnected);
    assert_eq!(rig.recorder.run_done().await, RunReading::NotConnected);
    assert_eq!(calls(&rig), 0);

    rig.connect(COOKIE).await;
    // A minute on, the start of a run reads at once; the site refuses it.
    rig.at(100.0).await;
    rig.site.fail_at(calls(&rig), ScriptedFault::Unauthorised);
    assert_eq!(rig.recorder.run_begin().await, RunReading::SignedOut);
    assert_eq!(rig.recorder.status().state, ConnectionState::Expired);
    assert_eq!(rig.recorder.run_done().await, RunReading::SignedOut);
}

#[tokio::test(start_paused = true)]
async fn a_failed_reading_ends_the_run_and_a_disconnect_ends_it() {
    use super::recorder::RunReading;
    let rig = rig(script(10), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(100.0).await;
    rig.site.fail_at(calls(&rig), ScriptedFault::Network);
    assert_eq!(rig.recorder.run_begin().await, RunReading::Unreadable);
    // The run ended with the failure: nothing more is read for it.
    rig.at(161.0).await;
    assert_eq!(calls(&rig), 3);
    // Begun again, then done; a disconnect answers the wait.
    let begun = rig.recorder.run_begin().await;
    assert_eq!(read_at(&rig, begun), Some(161.0));
    let recorder = rig.recorder.clone();
    let done = tokio::spawn(async move { recorder.run_done().await });
    rig.at(170.0).await;
    rig.recorder.disconnect().await;
    assert_eq!(done.await.unwrap(), RunReading::NotConnected);
    rig.at(600.0).await;
    assert_eq!(calls(&rig), 5);
}

#[tokio::test(start_paused = true)]
async fn a_reading_whose_step_cannot_be_examined_stands_and_the_run_hears_it_unread() {
    use super::recorder::RunReading;
    let rig = rig(script(10), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.until(|status| status.readings_stored == 1).await;
    // The reading before the next one no longer decodes.
    write(&rig.db, |conn| {
        conn.execute(
            "UPDATE account_reading_holdings SET quantity = quantity + 1 \
             WHERE row_count > 0",
            [],
        )?;
        Ok(())
    })
    .await;
    rig.at(700.0).await;
    assert_eq!(rig.recorder.run_begin().await, RunReading::Unreadable);
    // The reading itself was stored, and is marked examined.
    let status = rig.until(|status| !status.working).await;
    assert_eq!(status.readings_stored, 2);
    let examined: i64 = read(&rig.db, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM account_readings_examined",
            [],
            |row| row.get(0),
        )?)
    })
    .await;
    assert_eq!(examined, 2);
    // And the run ended with it.
    rig.at(900.0).await;
    assert_eq!(item_reads(&rig), 2);
}

#[tokio::test(start_paused = true)]
async fn a_run_closes_when_its_view_goes_and_after_thirty_minutes() {
    let rig = rig(script(80), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(10.0).await;
    rig.recorder.run_begin().await;
    rig.recorder.run_cancel();
    rig.at(600.0).await;
    assert_eq!(item_reads(&rig), 1, "nothing read once the view went");

    // Left open: read every minute for thirty minutes, then no more.
    let begun = rig.recorder.run_begin().await;
    assert_eq!(read_at(&rig, begun), Some(600.0));
    rig.at(600.0 + 30.0 * 60.0 + 5.0).await;
    let read_for_the_run = item_reads(&rig);
    assert_eq!(read_for_the_run, 1 + 31);
    rig.at(4_000.0).await;
    assert_eq!(item_reads(&rig), read_for_the_run);
}

#[tokio::test(start_paused = true)]
async fn a_run_request_given_up_on_ends_the_run() {
    use super::recorder::{RunReading, RUN_WAIT};
    let rig = rig(script(10), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(100.0).await;
    // The site stops answering: the start of the run waits, then gives up.
    rig.site.hold();
    let begun = rig.recorder.run_begin().await;
    assert_eq!(begun, RunReading::Unreadable);
    let before = item_reads(&rig);
    rig.site.release();
    // The reading the held call began completes; nothing more is read for
    // the run.
    rig.at(100.0 + RUN_WAIT.as_secs_f64() + 600.0).await;
    assert_eq!(
        item_reads(&rig),
        before + 1,
        "the held reading, and no more"
    );
}

#[tokio::test(start_paused = true)]
async fn the_readings_left_unexamined_are_examined_at_start_one_at_a_time() {
    use super::recorder::RunReading;
    use super::store::{record_reading, Purpose};
    use super::test_support::recorded_hunt;
    let rig = rig_seeded(script(10), false, &[], |conn| {
        let readings = recorded_hunt();
        record_reading(conn, &readings[0], Purpose::Run, None).unwrap();
        let bad = record_reading(conn, &readings[1], Purpose::Connect, None)
            .unwrap()
            .id;
        conn.execute(
            "UPDATE account_reading_holdings SET quantity = quantity + 1 \
             WHERE reading_id = ?1 AND row_count > 0",
            [bad.0],
        )
        .unwrap();
        // A fresh keyframe, then the hunt's readings up to its opening of
        // seven and on.
        record_reading(conn, &readings[2], Purpose::Connect, None).unwrap();
        for reading in &readings[3..] {
            record_reading(conn, reading, Purpose::Run, None).unwrap();
        }
    })
    .await;
    let unexamined = |rig: &Rig| {
        let db = rig.db.clone();
        async move {
            read(&db, |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM account_readings r WHERE NOT EXISTS \
                     (SELECT 1 FROM account_readings_examined e WHERE e.reading_id = r.id)",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
        }
    };
    // A run begun while the start's examining is under way is answered
    // after one reading's examination, not all of them.
    rig.connect(COOKIE).await;
    let begun = rig.recorder.run_begin().await;
    assert!(matches!(begun, RunReading::Read { .. }), "{begun:?}");
    assert!(unexamined(&rig).await >= 10, "the rest still wait");
    // In time every one is examined: the unreadable one passed, and the
    // good one's opening proposed.
    rig.until(|status| !status.working).await;
    assert_eq!(unexamined(&rig).await, 0);
    // Of the readings left from before: the opening of seven, and not the
    // five whose step could not be read.
    let counts: Vec<i64> = rig
        .recorder
        .proposals(None)
        .await
        .unwrap()
        .iter()
        .filter(|proposal| proposal.reading.0 <= 15)
        .filter_map(|proposal| match &proposal.detail {
            super::proposals::Detail::Opening(opening) => Some(opening.boxes[0].count),
            super::proposals::Detail::Listing(_)
            | super::proposals::Detail::Purchase(_)
            | super::proposals::Detail::Arrival(_)
            | super::proposals::Detail::Sale(_) => None,
        })
        .collect();
    assert_eq!(counts, vec![7]);
}

/// The calls after the connect's own two, by kind.
fn after_connect(rig: &Rig) -> Vec<ScriptedRead> {
    rig.site.calls().into_iter().skip(2).collect()
}

#[tokio::test(start_paused = true)]
async fn an_idle_connection_asks_for_the_balance_every_five_minutes_and_stores_nothing() {
    use super::recorder::KEEP_ALIVE_EVERY;
    let rig = rig(script(3), false, &[]).await;
    rig.connect(COOKIE).await;
    let every = KEEP_ALIVE_EVERY.as_secs_f64();
    rig.at(every - 1.0).await;
    assert!(
        after_connect(&rig).is_empty(),
        "nothing inside the first five minutes"
    );
    rig.at(3.0 * every + 1.0).await;
    assert_eq!(after_connect(&rig), vec![ScriptedRead::Balance; 3]);
    assert_eq!(rig.stored().await, vec![tagged(0.0, "connect", None)]);
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
}

#[tokio::test(start_paused = true)]
async fn any_request_to_the_site_puts_the_keep_alive_back_five_minutes() {
    use super::recorder::KEEP_ALIVE_EVERY;
    let rig = rig(script(5), false, &[]).await;
    rig.connect(COOKIE).await;
    // A strongbox run's reading at 200 is a request like any other.
    rig.at(200.0).await;
    rig.recorder.run_begin().await;
    rig.recorder.run_cancel();
    let every = KEEP_ALIVE_EVERY.as_secs_f64();
    rig.at(200.0 + every - 1.0).await;
    assert_eq!(
        after_connect(&rig),
        vec![ScriptedRead::Balance, ScriptedRead::Items],
        "no keep-alive five minutes after the connect"
    );
    rig.at(200.0 + every + 1.0).await;
    assert_eq!(
        after_connect(&rig),
        vec![
            ScriptedRead::Balance,
            ScriptedRead::Items,
            ScriptedRead::Balance
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_tracked_session_s_minutes_keep_the_session_without_a_keep_alive() {
    let plan = [Planned {
        id: "hunt",
        start: 60.0,
        end: None,
        active: true,
    }];
    let rig = rig(script(40), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(60.0).await;
    rig.start("hunt");
    rig.at(60.0 + 20.0 * 60.0 + 5.0).await;
    // Every request is a full reading: a balance and its items, in pairs.
    let calls = rig.site.calls();
    assert_eq!(calls.len() % 2, 0);
    assert!(calls
        .chunks(2)
        .all(|pair| pair == [ScriptedRead::Balance, ScriptedRead::Items]));
}

#[tokio::test(start_paused = true)]
async fn a_keep_alive_the_site_refuses_signs_the_player_out() {
    use super::recorder::KEEP_ALIVE_EVERY;
    let rig = rig(script(3), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.site.fail_at(2, ScriptedFault::Unauthorised);
    let status = rig
        .until(|status| status.state == ConnectionState::Expired)
        .await;
    assert!(status.has_connected, "being signed out is not a disconnect");
    assert!(rig.flag.is_set());
    // Nothing more is asked of the site once it has refused.
    rig.at(10.0 * KEEP_ALIVE_EVERY.as_secs_f64()).await;
    assert_eq!(after_connect(&rig), vec![ScriptedRead::Balance]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_keep_alive_keeps_the_connection_and_tries_again_five_minutes_on() {
    use super::recorder::KEEP_ALIVE_EVERY;
    let rig = rig(script(3), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.site.fail_at(2, ScriptedFault::Network);
    let every = KEEP_ALIVE_EVERY.as_secs_f64();
    rig.at(every + 1.0).await;
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
    rig.at(2.0 * every + 1.0).await;
    assert_eq!(after_connect(&rig), vec![ScriptedRead::Balance; 2]);
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_stops_the_keep_alive() {
    use super::recorder::KEEP_ALIVE_EVERY;
    let rig = rig(script(3), false, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(100.0).await;
    rig.recorder.disconnect().await;
    rig.at(10.0 * KEEP_ALIVE_EVERY.as_secs_f64()).await;
    assert!(after_connect(&rig).is_empty());
}

/// How many holding states the store holds, across every reading.
async fn holdings_stored(rig: &Rig) -> i64 {
    read(&rig.db, |conn| {
        Ok(
            conn.query_row("SELECT COUNT(*) FROM account_reading_holdings", [], |row| {
                row.get(0)
            })?,
        )
    })
    .await
}

/// Whether every request so far was a full reading: a balance and its
/// items, in pairs, never a balance asked for on its own.
fn only_full_readings(rig: &Rig) -> bool {
    let calls = rig.site.calls();
    calls.len().is_multiple_of(2)
        && calls
            .chunks(2)
            .all(|pair| pair == [ScriptedRead::Balance, ScriptedRead::Items])
}

#[tokio::test(start_paused = true)]
async fn installed_an_idle_connection_is_read_whole_every_five_minutes_and_nothing_new_is_kept_light(
) {
    use super::recorder::IDLE_READ_EVERY;
    let rig = always_rig(script(1), &[]).await;
    rig.connect(COOKIE).await;
    // The connect reading is the first ever: a change, so the next is a
    // minute on.
    rig.at(59.0).await;
    assert_eq!(item_reads(&rig), 1);
    rig.at(61.0).await;
    assert_eq!(item_reads(&rig), 2);
    let kept = holdings_stored(&rig).await;
    assert_eq!(kept, 2, "the connect reading's keyframe");
    // It agreed with the one before it: five minutes to the next.
    let every = IDLE_READ_EVERY.as_secs_f64();
    rig.at(60.0 + every - 1.0).await;
    assert_eq!(item_reads(&rig), 2);
    rig.at(60.0 + 11.0 * every + 30.0).await;
    let mut expected = vec![tagged(0.0, "connect", None)];
    for k in 0..12 {
        expected.push(tagged(60.0 + every * f64::from(k), "ambient", None));
    }
    assert_eq!(rig.stored().await, expected);
    // The readings keep the session: no balance is asked for on its own.
    assert!(only_full_readings(&rig));
    // Every reading after the first is a row of its own and no holdings.
    assert_eq!(holdings_stored(&rig).await, kept);
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
}

#[tokio::test(start_paused = true)]
async fn installed_a_change_brings_the_next_reading_a_minute_on_until_two_agree() {
    let rows = |ammo: i64| -> Vec<ScriptedRow> {
        vec![
            ("Universal Ammo", "CARRIED", ammo, ammo / 100),
            ("Common Dung", "STORAGE (Calypso)", 29, 0),
        ]
    };
    let (a, b, c) = (rows(1_000_000), rows(999_000), rows(998_000));
    let readings = responses(&[
        (25_000, a.as_slice()),
        (25_000, a.as_slice()),
        (25_000, b.as_slice()),
        (25_000, c.as_slice()),
    ]);
    let rig = always_rig(readings, &[]).await;
    rig.connect(COOKIE).await;
    rig.at(1_000.0).await;
    let times: Vec<f64> = rig.stored().await.iter().map(|(at, _, _)| *at).collect();
    // The first reading; one that agrees, five minutes to the next; two
    // changes a minute apart; then one that agrees, and five minutes.
    assert_eq!(times, vec![0.0, 60.0, 360.0, 420.0, 480.0, 780.0]);
}

#[tokio::test(start_paused = true)]
async fn installed_a_tracked_session_takes_over_the_idle_readings_and_hands_them_back() {
    let plan = [Planned {
        id: "hunt",
        start: 500.0,
        end: Some(690.0),
        active: false,
    }];
    let rig = always_rig(script(1), &plan).await;
    rig.connect(COOKIE).await;
    rig.at(500.0).await;
    rig.start("hunt");
    rig.at(690.0).await;
    rig.stop("hunt");
    rig.at(1_100.0).await;
    let hunt = Some("hunt");
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "ambient", None),
            tagged(360.0, "ambient", None),
            // The session's minutes, not the idle reading due at 660.
            tagged(500.0, "baseline", hunt),
            tagged(560.0, "session", hunt),
            tagged(620.0, "session", hunt),
            tagged(680.0, "session", hunt),
            // Its closing reading agrees with the one before: sealed.
            tagged(735.0, "session", hunt),
            // Five minutes after it, the idle readings again.
            tagged(1_035.0, "ambient", None),
        ]
    );
    assert!(rig.sealed_at("hunt").await.is_some());
    assert!(only_full_readings(&rig));
}

#[tokio::test(start_paused = true)]
async fn installed_an_idle_reading_the_site_refuses_signs_the_player_out() {
    let rig = always_rig(script(1), &[]).await;
    rig.connect(COOKIE).await;
    // Calls 0 and 1 connect; the idle reading's balance is refused.
    rig.site.fail_at(2, ScriptedFault::Unauthorised);
    let status = rig
        .until(|status| status.state == ConnectionState::Expired)
        .await;
    assert!(status.has_connected, "being signed out is not a disconnect");
    rig.at(3_600.0).await;
    assert_eq!(
        calls(&rig),
        3,
        "nothing is asked of the site once it refused"
    );
    assert_eq!(rig.stored().await, vec![tagged(0.0, "connect", None)]);
}

#[tokio::test(start_paused = true)]
async fn installed_a_disconnect_or_shutting_down_stops_the_idle_readings() {
    let rig = always_rig(script(1), &[]).await;
    rig.connect(COOKIE).await;
    rig.at(100.0).await;
    assert_eq!(calls(&rig), 4);
    rig.recorder.disconnect().await;
    rig.at(3_600.0).await;
    assert_eq!(calls(&rig), 4);

    let rig = always_rig(script(1), &[]).await;
    rig.connect(COOKIE).await;
    rig.at(100.0).await;
    rig.recorder.shutdown().await;
    rig.at(3_600.0).await;
    assert_eq!(calls(&rig), 4);
    assert_eq!(
        rig.stored().await,
        vec![tagged(0.0, "connect", None), tagged(60.0, "ambient", None)]
    );
}

#[tokio::test(start_paused = true)]
async fn a_development_build_reads_nothing_between_sessions_and_runs() {
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: Some(230.0),
        active: false,
    }];
    let rig = rig(script(10), false, &plan).await;
    rig.connect(COOKIE).await;
    rig.at(120.0).await;
    rig.start("hunt");
    rig.at(230.0).await;
    rig.stop("hunt");
    rig.at(1_000.0).await;
    assert!(rig.sealed_at("hunt").await.is_some());
    assert!(matches!(
        rig.recorder.run_begin().await,
        super::recorder::RunReading::Read { .. }
    ));
    assert!(matches!(
        rig.recorder.run_done().await,
        super::recorder::RunReading::Read { .. }
    ));
    rig.at(1_200.0).await;
    let reads = item_reads(&rig);
    let before = calls(&rig);
    rig.at(1_200.0 + 3_600.0).await;
    assert_eq!(item_reads(&rig), reads, "no item read outside them");
    let since: Vec<ScriptedRead> = rig.site.calls().into_iter().skip(before).collect();
    assert!(!since.is_empty());
    assert!(
        since.iter().all(|call| *call == ScriptedRead::Balance),
        "only the balance keeps the session"
    );
    assert!(rig
        .stored()
        .await
        .iter()
        .all(|(_, purpose, _)| purpose != "ambient"));
}

/// The opening edge of a session's measure as it stands.
async fn opening_edge(rig: &Rig, session: &str) -> Edge {
    let MeasureRead::Measured { measure, .. } = rig
        .recorder
        .session_measure(session.to_string())
        .await
        .unwrap()
    else {
        panic!("the session has readings");
    };
    measure.coverage.opening
}

#[tokio::test(start_paused = true)]
async fn installed_a_session_started_during_an_idle_reading_takes_its_own_baseline_at_once() {
    let plan = [Planned {
        id: "hunt",
        start: 61.0,
        end: None,
        active: false,
    }];
    let rig = always_rig(script(1), &plan).await;
    rig.connect(COOKIE).await;
    rig.at(59.0).await;
    // The idle reading at 60 is still on its way when the session starts.
    rig.site.hold();
    rig.at(61.0).await;
    rig.start_running("hunt").await;
    rig.at(62.0).await;
    rig.site.release();
    rig.until(|status| status.readings_stored == 2).await;
    rig.at(100.0).await;
    let hunt = Some("hunt");
    // The idle reading was abandoned; the baseline was asked for at once
    // and is the session's own.
    assert_eq!(
        rig.stored().await,
        vec![tagged(0.0, "connect", None), tagged(62.0, "baseline", hunt)]
    );
    assert_eq!(opening_edge(&rig, "hunt").await, Edge::Read);
}

#[tokio::test(start_paused = true)]
async fn installed_a_session_started_inside_an_idle_reading_s_floor_takes_its_own_baseline() {
    let plan = [Planned {
        id: "hunt",
        start: 90.0,
        end: None,
        active: false,
    }];
    let rig = always_rig(script(1), &plan).await;
    rig.connect(COOKIE).await;
    rig.at(90.0).await;
    rig.start_running("hunt").await;
    rig.at(100.0).await;
    let hunt = Some("hunt");
    assert_eq!(
        rig.stored().await,
        vec![
            tagged(0.0, "connect", None),
            tagged(60.0, "ambient", None),
            // Thirty seconds after the idle reading, not at its floor.
            tagged(90.0, "baseline", hunt),
        ]
    );
    assert_eq!(opening_edge(&rig, "hunt").await, Edge::Read);
}

#[tokio::test(start_paused = true)]
async fn installed_a_failed_idle_reading_is_tried_again_a_minute_on() {
    let rig = always_rig(script(3), &[]).await;
    rig.connect(COOKIE).await;
    // Calls 0 and 1 connect, 2 and 3 read at 60; the reading at 120
    // fails at its balance.
    rig.site.fail_at(4, ScriptedFault::Network);
    rig.at(121.0).await;
    assert_eq!(calls(&rig), 5);
    rig.at(179.0).await;
    assert_eq!(calls(&rig), 5, "not before a minute on");
    rig.at(1_000.0).await;
    let times: Vec<f64> = rig.stored().await.iter().map(|(at, _, _)| *at).collect();
    // A change, a minute; a failure, a minute, never the idle interval;
    // a change, a minute; agreement, five.
    assert_eq!(times, vec![0.0, 60.0, 180.0, 240.0, 540.0, 840.0]);
    assert_eq!(rig.recorder.status().state, ConnectionState::Connected);
}

#[tokio::test(start_paused = true)]
async fn installed_a_failed_idle_reading_leaves_a_waiting_strongbox_run_to_its_own() {
    use super::recorder::RunReading;
    let rig = always_rig(script(2), &[]).await;
    rig.connect(COOKIE).await;
    rig.at(59.0).await;
    // The idle reading at 60 is held, then fails at its balance.
    rig.site.fail_at(2, ScriptedFault::Network);
    rig.site.hold();
    rig.at(61.0).await;
    let recorder = rig.recorder.clone();
    let begun = tokio::spawn(async move { recorder.run_begin().await });
    rig.at(62.0).await;
    rig.site.release();
    // The run is not told the idle reading failed: it reads for itself,
    // once the floor allows.
    let begun = begun.await.unwrap();
    assert_eq!(read_at(&rig, begun), Some(115.0), "{begun:?}");
    assert!(matches!(begun, RunReading::Read { .. }));
    rig.recorder.run_cancel();
}

#[tokio::test(start_paused = true)]
async fn installed_an_unchanged_idle_reading_after_a_session_lets_its_gap_be_weighed() {
    use super::costs::Unmeasured;
    let plan = [Planned {
        id: "hunt",
        start: 120.0,
        end: Some(230.0),
        active: false,
    }];
    let rig = always_rig(script(1), &plan).await;
    rig.connect(COOKIE).await;
    rig.at(120.0).await;
    rig.start("hunt");
    rig.at(230.0).await;
    // Calls 0 and 1 connect, 2 and 3 the idle reading at 60, 4 to 7 the
    // session's two; every closing reading fails, so the session is sealed
    // with its close unread.
    for call in 8..=10 {
        rig.site.fail_at(call, ScriptedFault::Network);
    }
    rig.stop("hunt");
    rig.at(600.0).await;
    assert!(rig.sealed_at("hunt").await.is_some());
    assert_eq!(stored_costs(&rig).await, 0, "its gap waits for a reading");
    // The next idle reading changes nothing, and is the first after the
    // session's own: the gap is weighed.
    rig.at(700.0).await;
    assert_eq!(
        rig.stored().await.last(),
        Some(&tagged(670.0, "ambient", None))
    );
    assert_eq!(stored_costs(&rig).await, 1);
    let costs = rig
        .recorder
        .session_costs("hunt".into())
        .await
        .unwrap()
        .expect("a known session");
    assert_ne!(
        costs.status,
        Status::Unmeasured(Unmeasured::AwaitingGapReading)
    );
}
