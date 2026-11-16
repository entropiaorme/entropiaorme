//! The account connection over the typed facade: its state across
//! connecting, expiring and disconnecting; a session recorded end to end
//! from the recorded hunt and its measure read back over the wire; the
//! measured sessions beside their tracked figures; the minute floor; and
//! an unauthorised answer mid-session.
//!
//! The connection runs on paused time against the scripted stand-in for
//! the account site, with a wall clock riding tokio's, so every reading
//! falls exactly on its minute. Each test plans its tracked sessions' rows
//! up front and then only publishes session starts and stops at planned
//! instants.

use std::path::Path;
use std::sync::Arc;

use chrono::NaiveDate;
use eo_api::account::{
    AccountConnectionState, MeasuredAmmoSplit, MeasuredAmmoTotal, MeasuredCoverageLevel,
    MeasuredEvent, MeasuredGainMark, MeasuredLeftOutReason, MeasuredPrecision, MeasuredRole,
    MeasuredSettlement, MeasuredStacking,
};
use eo_api::{Api, ApiError};
use eo_services::account::recorder::{MemoryFlag, RecorderParts};
use eo_services::account::scripted::{
    recorded_hunt_responses, PacedClock, ScriptedFault, ScriptedSite,
};
use eo_services::account::{AccountRecorder, ConnectOutcome, ConnectionStatus, SessionSecret};
use eo_services::bus_events::{BusEvent, SessionLifecyclePayload};
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use serde_json::Value;

use crate::common;

const COOKIE: &str = "signed-in";

struct Planned<'a> {
    id: &'a str,
    start: f64,
    end: Option<f64>,
}

struct Harness {
    _dir: tempfile::TempDir,
    api: Api,
    db: Db,
    bus: Arc<EventBus>,
    site: Arc<ScriptedSite>,
    clock: Arc<PacedClock>,
    recorder: Arc<AccountRecorder>,
}

/// The facade over a fresh database, with the account connection attached
/// on its own bus, the bundled game data, and the player's own gear as the
/// recorded hunt was played.
async fn harness(readings: Vec<(Value, Value)>, plan: &[Planned<'_>]) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db")).await.unwrap();
    let clock = Arc::new(PacedClock::new(
        NaiveDate::from_ymd_opt(2026, 7, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
    ));
    let rows: Vec<(String, f64, Option<f64>)> = plan
        .iter()
        .map(|planned| {
            (
                planned.id.to_string(),
                clock.epoch_at(planned.start),
                planned.end.map(|end| clock.epoch_at(end)),
            )
        })
        .collect();
    db.with_writer(move |conn| {
        for (id, started, ended) in rows {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, session_name) \
                 VALUES (?1, ?2, ?3, 0, 'Mayhem run')",
                rusqlite::params![id, started, ended],
            )?;
        }
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, properties_json) VALUES \
             ('Exceptional Mayhem Electric Attack Chip 15, Perfected (L)', 'weapon', \
              '{\"weapon_entity\": {\"name\": \"Exceptional Mayhem Electric Attack Chip 15, Perfected (L)\"}, \
                \"amp_entity\": {\"name\": \"Mayhem MF-Amplifier Delta (L)\"}, \
                \"implant_entity\": {\"name\": \"NeoPsion 85-B Mindforce Implant (L)\"}}'), \
             ('Frontier First Aid Pack, Adjusted', 'healing', '{}')",
            [],
        )?;
        conn.execute(
            "INSERT INTO protection_sets (kind, name, economy_kind, created_at) \
             VALUES ('armour', 'Pegasus', 'unlimited', 0)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let snapshot =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources/snapshot");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("the bundled game data loads"));
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let bus = Arc::new(EventBus::new());
    let site = Arc::new(ScriptedSite::new(COOKIE, readings));
    let recorder = AccountRecorder::start(
        &bus,
        RecorderParts {
            db: db.clone(),
            clock: clock.clone(),
            site: site.clone(),
            flag: Arc::new(MemoryFlag::new(false)),
            game_data: Some(game_data.clone()),
            changed: None,
        },
        &tokio::runtime::Handle::current(),
    );
    let api = Api::new(
        db.clone(),
        game_data,
        clock.clone(),
        data_dir,
        handles.config_service,
        handles.tracker,
        handles.hotbar,
        handles.watcher,
        handles.skill_tracker,
        handles.skill_scan,
        handles.spacebar,
        handles.repair_ocr,
        handles.sale_capture,
        handles.quests.clone(),
        None,
        None,
        None,
    )
    .with_account(recorder.clone());
    Harness {
        _dir: dir,
        api,
        db,
        bus,
        site,
        clock,
        recorder,
    }
}

impl Harness {
    async fn connect(&self) -> ConnectOutcome {
        let recorder = self
            .api
            .account_recorder()
            .expect("the connection is attached");
        recorder
            .connect(
                recorder.epoch(),
                SessionSecret::new(COOKIE.to_string()).unwrap(),
            )
            .await
    }

    /// Move on to `seconds` after the start, once the connection has
    /// finished what it is doing (paused time would otherwise leap there
    /// while the database works).
    async fn at(&self, seconds: f64) {
        let _ = self
            .recorder
            .watch_status()
            .wait_for(|status| !status.working)
            .await;
        tokio::time::sleep_until(self.clock.instant_at(seconds)).await;
    }

    async fn until(&self, condition: impl Fn(&ConnectionStatus) -> bool) {
        let _ = self
            .recorder
            .watch_status()
            .wait_for(|status| condition(status))
            .await
            .expect("the connection task is running");
    }

    fn publish(&self, started: bool, id: &str) {
        let payload = SessionLifecyclePayload {
            session_id: id.to_string(),
        };
        self.bus.publish(&if started {
            BusEvent::SessionStarted(payload)
        } else {
            BusEvent::SessionStopped(payload)
        });
    }
}

/// The recorded hunt from the reading taken just before the session began.
fn the_hunt_from_its_opening_reading() -> Vec<(Value, Value)> {
    recorded_hunt_responses().split_off(2)
}

/// The hunt's span, in seconds after its opening reading: it ran from
/// three seconds after that reading to 713 seconds after it.
const HUNT: Planned = Planned {
    id: "hunt",
    start: 3.0,
    end: Some(713.0),
};

/// Record the hunt end to end: connect (the opening reading), start, a
/// reading every minute, stop, closing readings, sealed.
async fn record_the_hunt(harness: &Harness) {
    assert_eq!(harness.connect().await, ConnectOutcome::Connected);
    harness.at(3.0).await;
    harness.publish(true, "hunt");
    harness.at(713.0).await;
    harness.publish(false, "hunt");
    harness
        .until(|status| status.reading_session.is_none() && !status.working)
        .await;
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

#[tokio::test(start_paused = true)]
async fn the_status_follows_connecting_expiring_and_disconnecting() {
    let plan = [Planned {
        id: "hunt",
        start: 10.0,
        end: Some(400.0),
    }];
    let harness = harness(recorded_hunt_responses(), &plan).await;
    let status = harness.api.account_status().await.unwrap();
    assert_eq!(status.state, AccountConnectionState::Disconnected);
    assert!(!status.has_connected);
    assert!(status.connected_at.is_none() && status.last_reading_at.is_none());

    harness.connect().await;
    harness.until(|status| status.readings_stored == 1).await;
    let status = harness.api.account_status().await.unwrap();
    assert_eq!(status.state, AccountConnectionState::Connected);
    assert!(status.has_connected && !status.reading_session);
    assert!(status.connected_at.is_some());
    assert_eq!(status.last_reading_at, status.connected_at);
    let wire = serde_json::to_value(&status).unwrap();
    let keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "state",
            "connectedAt",
            "lastReadingAt",
            "readingSession",
            "hasConnected",
            "confirming",
            "signingIn",
            "hasReadings"
        ]
    );
    assert_eq!(wire["state"], "connected");
    assert_eq!(wire["confirming"], Value::Null, "only while unconfirmed");
    assert_eq!(wire["signingIn"], false);
    assert_eq!(wire["hasReadings"], true);

    harness.at(10.0).await;
    harness.publish(true, "hunt");
    harness
        .until(|status| status.reading_session.is_some())
        .await;
    assert!(harness.api.account_status().await.unwrap().reading_session);

    // The site stops accepting the session: expired, quietly.
    harness
        .site
        .fail_at(harness.site.calls().len(), ScriptedFault::Unauthorised);
    harness
        .until(|status| status.state == eo_services::account::ConnectionState::Expired)
        .await;
    let status = harness.api.account_status().await.unwrap();
    assert_eq!(status.state, AccountConnectionState::Expired);
    assert!(status.has_connected && !status.reading_session);
    assert!(status.connected_at.is_none());

    harness.api.account_disconnect().await.unwrap();
    let status = harness.api.account_status().await.unwrap();
    assert_eq!(status.state, AccountConnectionState::Disconnected);
    assert!(!status.has_connected);
    // The readings stay, and so does the review of them.
    assert!(status.has_readings);

    // The sign-in window opening and going away shows in the status.
    harness.recorder.set_signing_in(true);
    harness.until(|status| status.signing_in).await;
    assert!(harness.api.account_status().await.unwrap().signing_in);
    harness.recorder.set_signing_in(false);
    harness.until(|status| !status.signing_in).await;
    assert!(!harness.api.account_status().await.unwrap().signing_in);
}

#[tokio::test(start_paused = true)]
async fn a_session_recorded_end_to_end_measures_as_the_recorded_hunt() {
    let harness = harness(the_hunt_from_its_opening_reading(), &[HUNT]).await;
    record_the_hunt(&harness).await;
    let measure = harness
        .api
        .account_session_measure("hunt".into())
        .await
        .unwrap()
        .0
        .expect("the hunt has readings");
    assert!(measure.sealed);
    assert_eq!(measure.coverage.level, MeasuredCoverageLevel::Complete);
    assert_eq!(measure.coverage.reading_count, 13);
    assert_eq!(measure.coverage.gap_count, 0);

    assert_eq!(measure.ammo.len(), 1);
    let ammo = &measure.ammo[0];
    assert_eq!(ammo.name, "Universal Ammo");
    assert_eq!(ammo.net_units, -60_288);
    let MeasuredAmmoSplit::Settled {
        burned_units,
        burned,
        strongbox_units,
        strongbox,
        converted_units,
        ..
    } = &ammo.split
    else {
        panic!("the hunt's opening settled");
    };
    assert_eq!(
        (*burned_units, *strongbox_units, *converted_units),
        (660_288, 600_000, 0)
    );
    assert!(close(burned.ped, 66.0288));
    assert_eq!(burned.precision, MeasuredPrecision::Exact);
    assert!(close(strongbox.ped, 60.0));

    let openings: Vec<_> = measure
        .events
        .iter()
        .filter_map(|event| match event {
            MeasuredEvent::StrongboxOpening {
                keys_used,
                contents,
                settlement,
                ..
            } => Some((*keys_used, contents.clone(), *settlement)),
            _ => None,
        })
        .collect();
    assert_eq!(openings.len(), 1);
    assert_eq!(openings[0].0, 7);
    assert_eq!(openings[0].1[0].item.name, "Skeleton Pants Red (F)");
    assert_eq!(openings[0].2, MeasuredSettlement::Settled);
    assert_eq!(measure.events.len(), 2);

    let gained: Vec<(&str, MeasuredStacking, MeasuredGainMark, i64)> = measure
        .gained
        .iter()
        .map(|gain| {
            (
                gain.item.name.as_str(),
                gain.stacking,
                gain.mark,
                gain.units,
            )
        })
        .collect();
    assert_eq!(
        gained,
        vec![
            (
                "Mayhem Token",
                MeasuredStacking::Single,
                MeasuredGainMark::Arrived,
                1
            ),
            (
                "Output Amplifier Component",
                MeasuredStacking::Stack,
                MeasuredGainMark::Arrived,
                4
            ),
            (
                "Shrapnel",
                MeasuredStacking::Stack,
                MeasuredGainMark::Arrived,
                363_184
            ),
        ]
    );
    assert!(close(measure.gained_total.ped, 37.9184));
    // The measure carries its totals, the same the measured list carries.
    let MeasuredAmmoTotal::Settled {
        burned: by_category,
    } = &measure.totals.ammo
    else {
        panic!("the hunt's ammo settled");
    };
    assert!(close(by_category.ped, 66.0288));
    assert!(close(measure.totals.gained.ped, 37.9184));
    assert!(close(measure.totals.healing_decay.ped, 0.30));
    assert!(close(measure.totals.other_consumption.ped, 0.09));
    assert!(close(measure.gained_possibly_strongbox.ped, 0.0));
    assert_eq!(measure.other_consumption.len(), 1);
    assert_eq!(measure.other_consumption[0].item.name, "Mind Essence");
    assert_eq!(measure.other_consumption[0].units, 900);
    assert!(close(measure.other_consumption[0].amount.ped, 0.09));

    let healing: Vec<_> = measure
        .decay
        .iter()
        .filter(|item| item.role == MeasuredRole::Healing)
        .collect();
    assert_eq!(healing.len(), 1);
    assert_eq!(healing[0].item.name, "Frontier First Aid Pack, Adjusted");
    assert!(close(healing[0].decay.ped, 0.30));
    assert_eq!(healing[0].decay.precision, MeasuredPrecision::WholePec);

    // Times cross the wire as instants; amounts as PED with their
    // precision beside them.
    let wire = serde_json::to_value(&measure).unwrap();
    assert_eq!(
        wire["gainedTotal"]["precision"],
        Value::String("wholePec".into())
    );
    assert!(wire["events"][0]["between"]["from"].is_string());

    // Unknown sessions are not found; a session without readings is null.
    assert!(matches!(
        harness.api.account_session_measure("absent".into()).await,
        Err(ApiError::NotFound { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn measured_sessions_stand_beside_their_tracked_figures() {
    let quiet = Planned {
        id: "quiet",
        start: 2_000.0,
        end: Some(2_100.0),
    };
    let harness = harness(the_hunt_from_its_opening_reading(), &[HUNT, quiet]).await;
    record_the_hunt(&harness).await;
    harness
        .db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO session_summaries \
                     (session_id, started_at, ended_at, duration_hours, kills, loot_tt, \
                      weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, \
                      cycled_ped, regular_skill_ped_json, attribute_levels_json, \
                      regular_skill_tt, attribute_levels_total) \
                 SELECT id, started_at, ended_at, 0.2, 31, 38.2, 64.8, 0, 1.4, 0.3, 0, 66.5, \
                        '{}', '{}', 0, 0 FROM tracking_sessions WHERE id = 'hunt'",
                [],
            )?;
            conn.execute(
                "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 'hunt'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let page = harness
        .api
        .account_measured_sessions(None, None)
        .await
        .unwrap();
    assert_eq!(page.total, 1, "a session without readings is not listed");
    assert!(page.next_cursor.is_none());
    let hunt = &page.sessions[0];
    assert_eq!(hunt.id, "hunt");
    assert_eq!(hunt.name.as_deref(), Some("Mayhem run"));
    assert_eq!(hunt.coverage, MeasuredCoverageLevel::Complete);
    assert_eq!((hunt.reading_count, hunt.gap_count), (13, 0));
    assert!(hunt.sealed);
    assert!(
        hunt.exclusion.is_some(),
        "an excluded session keeps its measure"
    );
    let MeasuredAmmoTotal::Settled { burned } = &hunt.measured.ammo else {
        panic!("the hunt's ammo settled");
    };
    assert!(close(burned.ped, 66.0288));
    assert!(close(hunt.measured.gained.ped, 37.9184));
    assert!(close(hunt.measured.other_consumption.ped, 0.09));
    assert!(close(hunt.measured.healing_decay.ped, 0.30));
    let tracked = hunt.tracked.as_ref().expect("the hunt has a summary");
    assert_eq!(
        (
            tracked.weapon_cost,
            tracked.healing_cost,
            tracked.armour_cost,
            tracked.loot_tt,
            tracked.total_cycled
        ),
        (64.8, 0.3, 1.4, 38.2, 66.5)
    );
    assert!(!tracked.armour_unrecorded);
    // A hit no armour recording covers yet: the armour cost is not
    // recorded, whatever figure the summary holds.
    harness
        .db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO protection_defence_events (session_id, damage, deflected) \
                 VALUES ('hunt', 4.5, 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let page = harness
        .api
        .account_measured_sessions(None, None)
        .await
        .unwrap();
    let tracked = page.sessions[0].tracked.as_ref().unwrap();
    assert!(tracked.armour_unrecorded);
    assert_eq!(tracked.armour_cost, 1.4);
    // The tracked figures are read, never moved.
    let summary: f64 = harness
        .db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT weapon_cost FROM session_summaries WHERE session_id = 'hunt'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(summary, 64.8);
    assert!(matches!(
        harness
            .api
            .account_measured_sessions(Some("not a cursor".into()), None)
            .await,
        Err(ApiError::BadRequest { .. })
    ));
    assert!(harness
        .api
        .account_session_measure("quiet".into())
        .await
        .unwrap()
        .0
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn the_minute_floor_holds_over_the_facade() {
    let plan = [Planned {
        id: "hunt",
        start: 10.0,
        end: None,
    }];
    let harness = harness(recorded_hunt_responses(), &plan).await;
    harness.connect().await;
    harness.at(10.0).await;
    harness.publish(true, "hunt");
    // The session's first reading is the connect reading, ten seconds old.
    harness.at(69.0).await;
    assert_eq!(harness.site.calls().len(), 2);
    harness.at(71.0).await;
    assert_eq!(harness.site.calls().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn an_unauthorised_answer_mid_session_leaves_its_measure_partial() {
    let harness = harness(the_hunt_from_its_opening_reading(), &[HUNT]).await;
    assert_eq!(harness.connect().await, ConnectOutcome::Connected);
    harness.at(3.0).await;
    harness.publish(true, "hunt");
    // The minute at 303 seconds is refused.
    harness.at(250.0).await;
    harness
        .site
        .fail_at(harness.site.calls().len(), ScriptedFault::Unauthorised);
    harness.at(713.0).await;
    harness.publish(false, "hunt");
    harness.at(1_000.0).await;
    let status = harness.api.account_status().await.unwrap();
    assert_eq!(status.state, AccountConnectionState::Expired);
    let measure = harness
        .api
        .account_session_measure("hunt".into())
        .await
        .unwrap()
        .0
        .expect("the hunt has readings");
    assert_eq!(measure.coverage.level, MeasuredCoverageLevel::Partial);
    assert_eq!(measure.coverage.reading_count, 5);
    assert!(measure.sealed);
}

/// How a seeded session's readings measure it.
#[derive(Clone, Copy)]
pub(crate) struct Seeded {
    /// A closing reading after its end: complete coverage.
    pub complete: bool,
    /// Its measure is sealed.
    pub sealed: bool,
    /// Its ammo split is unsettled.
    pub unsettled: bool,
}

pub(crate) const COMPLETE: Seeded = Seeded {
    complete: true,
    sealed: true,
    unsettled: false,
};

/// Readings for an existing tracked session from `started` to `ended`:
/// ammo burned, a weapon worn, Shrapnel gained; tagged with it, and its
/// measure sealed as `how` says.
pub(crate) fn seed_measured(
    conn: &mut rusqlite::Connection,
    id: &str,
    started: f64,
    ended: f64,
    how: Seeded,
) {
    use eo_services::account::amount::Pec;
    use eo_services::account::measure::AmmoSplit;
    use eo_services::account::reading::{Container, Holding, HoldingKey, Reading, Row};
    use eo_services::account::sessions::{cache_sealed, measure_to_seal};
    use eo_services::account::store::{record_reading, Purpose};
    use std::collections::BTreeMap;

    let reading = |at: f64, ammo: i64, wear: i64, shrapnel: i64| {
        let mut holdings = BTreeMap::new();
        let mut hold = |name: &str, quantity: i64, value: i64| {
            holdings.insert(
                HoldingKey {
                    name: name.to_string(),
                    container: Container::Carried,
                },
                Holding::new(vec![Row {
                    quantity,
                    value: Pec(value),
                }])
                .unwrap(),
            );
        };
        hold("Universal Ammo", ammo, ammo / 100);
        hold("Opalo", 1, 500 - wear);
        if shrapnel > 0 {
            hold("Shrapnel", shrapnel, shrapnel / 100);
        }
        Reading::new(at, Pec(10_000), holdings).unwrap()
    };
    // An opening reading, one every two minutes through the session, and
    // (when complete) a closing one: no gap between them.
    let mut readings = vec![reading(started - 30.0, 1_000_000, 0, 0)];
    let mut at = started + 60.0;
    let mut step = 1;
    while at < ended {
        readings.push(reading(at, 1_000_000 - 5_000 * step, 5, 0));
        at += 120.0;
        step += 1;
    }
    if how.complete {
        readings.push(reading(ended + 30.0, 1_000_000 - 5_000 * step, 10, 1_000));
    }
    for reading in &readings {
        record_reading(conn, reading, Purpose::Session, Some(id)).unwrap();
    }
    if !how.sealed {
        return;
    }
    let mut seal = measure_to_seal(conn, id, None, ended + 100.0)
        .unwrap()
        .expect("an ended session with readings");
    if how.unsettled {
        for ammo in &mut seal.computed.measure.ammo {
            ammo.split = AmmoSplit::Unsettled {
                burned_units_by_rows: 0,
                strongbox_units_by_rows: 0,
                converted_units: 0,
            };
        }
    }
    assert_eq!(cache_sealed(conn, &[seal], ended + 100.0).unwrap(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_measured_summary_sums_what_both_sides_know() {
    let harness = harness(recorded_hunt_responses(), &[]).await;
    let sessions: [(&str, f64, Seeded); 6] = [
        ("c1", 10_000.0, COMPLETE),
        ("c2", 20_000.0, COMPLETE),
        (
            "p",
            30_000.0,
            Seeded {
                complete: false,
                ..COMPLETE
            },
        ),
        ("e", 40_000.0, COMPLETE),
        (
            "u",
            50_000.0,
            Seeded {
                unsettled: true,
                ..COMPLETE
            },
        ),
        (
            "n",
            60_000.0,
            Seeded {
                sealed: false,
                ..COMPLETE
            },
        ),
    ];
    harness
        .db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO equipment_library (name, item_type, properties_json) \
                 VALUES ('Opalo', 'weapon', '{}')",
                [],
            )?;
            for (id, started, how) in sessions {
                let ended = started + 300.0;
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                     VALUES (?1, ?2, ?3, 0)",
                    rusqlite::params![id, started, ended],
                )?;
                conn.execute(
                    "INSERT INTO session_summaries \
                         (session_id, started_at, ended_at, duration_hours, kills, loot_tt, \
                          weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, \
                          cycled_ped, regular_skill_ped_json, attribute_levels_json, \
                          regular_skill_tt, attribute_levels_total) \
                     VALUES (?1, ?2, ?3, 0.2, 3, 0.25, 1.5, 0, 0.5, 0.25, 0, 2.25, \
                             '{}', '{}', 0, 0)",
                    rusqlite::params![id, started, ended],
                )?;
                seed_measured(conn, id, started, ended, how);
            }
            conn.execute(
                "UPDATE tracking_sessions SET economy_excluded_at = 1.0 WHERE id = 'e'",
                [],
            )?;
            // c2's armour has a hit no recording covers yet.
            conn.execute(
                "INSERT INTO protection_defence_events (session_id, damage, deflected) \
                 VALUES ('c2', 3.0, 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let cached = |db: &Db| {
        let db = db.clone();
        async move {
            db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT session_id, sealed_at, computed_at, measure_json \
                     FROM account_session_measures ORDER BY session_id",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<f64>>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap()
        }
    };
    let before = cached(&harness.db).await;
    let summary = harness.api.account_measured_summary().await.unwrap();
    assert_eq!(cached(&harness.db).await, before, "the read writes nothing");

    // c1, c2 and u went in: p is partial, e excluded, n not sealed.
    assert_eq!(summary.included, 3);
    let left_out: Vec<(MeasuredLeftOutReason, i64)> = summary
        .left_out
        .iter()
        .map(|left| (left.reason, left.sessions))
        .collect();
    assert_eq!(
        left_out,
        [
            (MeasuredLeftOutReason::PartialCoverage, 1),
            (MeasuredLeftOutReason::Excluded, 1),
            (MeasuredLeftOutReason::NotSealed, 1),
        ]
    );
    // Each pair takes only the sessions whose both sides are known: u's
    // unsettled ammo leaves it out of the weapon pair, c2's unrecorded
    // armour out of the armour pair.
    assert_eq!(
        (
            summary.weapon.sessions,
            summary.healing.sessions,
            summary.armour.sessions,
            summary.other.sessions,
            summary.loot.sessions
        ),
        (2, 3, 2, 3, 3)
    );
    assert!(close(summary.weapon.tracked, 3.0));
    assert!(close(summary.healing.tracked, 0.75));
    assert!(close(summary.armour.tracked, 1.0));
    assert!(close(summary.loot.tracked, 0.75));
    // The measured side sums the sessions' own totals.
    let mut weapon = 0.0;
    let mut healing = 0.0;
    let mut armour = 0.0;
    let mut other = 0.0;
    let mut loot = 0.0;
    for id in ["c1", "c2", "u"] {
        let measure = harness
            .api
            .account_session_measure(id.into())
            .await
            .unwrap()
            .0
            .unwrap();
        let totals = &measure.totals;
        if let MeasuredAmmoTotal::Settled { burned } = &totals.ammo {
            weapon += burned.ped + totals.weapon_decay.ped;
        }
        healing += totals.healing_decay.ped;
        if id != "c2" {
            armour += totals.armour_decay.ped;
        }
        other += totals.other_equipment_decay.ped + totals.other_consumption.ped;
        loot += totals.gained.ped;
    }
    assert!(weapon > 0.0 && loot > 0.0);
    assert!(close(summary.weapon.measured.ped, weapon));
    assert!(close(summary.healing.measured.ped, healing));
    assert!(close(summary.armour.measured.ped, armour));
    assert!(close(summary.other.measured.ped, other));
    assert!(close(summary.loot.measured.ped, loot));
    // A sum keeps the weaker precision of its parts: the weapon's wear is
    // read from truncated values.
    assert_eq!(
        summary.weapon.measured.precision,
        MeasuredPrecision::WholePec
    );
}
