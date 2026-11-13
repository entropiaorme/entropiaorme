//! Behavioural pins for creature detection over the typed facade: the
//! setting that turns it on and off (and, off, never starts the watcher),
//! the after-session review that lists every detected creature and
//! suggestion and settles a group in one action, and the creature list the
//! player keeps beside the bundled one.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eo_api::mob_detection::{
    CreatureEntryCheck, CreatureEntrySource, DetectedCreature, MobDetectionGrade,
    MobDetectionGroupRef, MobDetectionSettleResult, MobDetectionSettled, MobDetectionTarget,
    MobPin, MobPinTarget, MobPinnedBy, MobSourceView,
};
use eo_api::settings::SettingsPatch;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::config_service::load_config_readonly;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::mob_history::MobHistory;
use eo_services::game_ui::test_support::bundled_profile;
use eo_services::mob_review::build_catalogue;
use eo_services::mob_watch::{CatalogueCell, MobDetector, PulseSink, WatchEnded};
use eo_services::tracker::{HuntTracker, Providers, TrackingConfig};

use crate::common;

/// The tracker's configuration read through the settings file, as the
/// app's own is: what a settings write turns on or off reaches it.
struct FileConfig(std::path::PathBuf);

impl TrackingConfig for FileConfig {
    fn session_name(&self) -> String {
        String::new()
    }

    fn session_definition_id(&self) -> Option<i64> {
        None
    }

    fn declared_skill_boost_percent(&self) -> Option<i64> {
        None
    }

    fn manual_mob(&self) -> Option<(String, String)> {
        None
    }

    fn loot_filter_blacklist(&self) -> Vec<String> {
        Vec::new()
    }

    fn mob_detection_enabled(&self) -> bool {
        load_config_readonly(&self.0).is_ok_and(|config| config.mob_detection_on())
    }
}

/// A watcher that records being started and stopped.
#[derive(Default)]
struct RecordingWatcher {
    starts: Mutex<usize>,
    stops: Mutex<usize>,
    /// The session being watched.
    watching: Mutex<Option<String>>,
}

impl MobDetector for RecordingWatcher {
    fn new_history(&self) -> Option<MobHistory> {
        let profile = bundled_profile();
        Some(MobHistory::new(profile.mob_history, profile.mob_names))
    }

    fn start(&self, session_id: &str, _sink: PulseSink, _ended: WatchEnded) {
        *self.starts.lock().unwrap() += 1;
        *self.watching.lock().unwrap() = Some(session_id.to_string());
    }

    fn stop(&self) {
        *self.stops.lock().unwrap() += 1;
        *self.watching.lock().unwrap() = None;
    }

    fn watches(&self, session_id: &str) -> bool {
        self.watching.lock().unwrap().as_deref() == Some(session_id)
    }

    fn permit_capture(&self) {}
}

const MOBS: &str = r#"[
    {"id": 1, "name": "Atrox", "species": {"name": "Atrox"},
     "maturities": [{"name": "Young", "level": 5}, {"name": "Old", "level": 12}]}
]"#;

async fn detection_api(dir: &Path, watcher: Arc<RecordingWatcher>) -> (Api, Db, Arc<HuntTracker>) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::write(snapshot.join("mobs.json"), MOBS).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("game-data store"));
    let handles = common::producer_handles_with_tracker(
        &db,
        &data_dir,
        tokio::runtime::Handle::current(),
        Providers {
            config: Arc::new(FileConfig(data_dir.clone())),
            detector: watcher,
            ..Providers::default()
        },
    )
    .await;
    let tracker = handles.tracker.clone();
    let names = bundled_profile().mob_names;
    let catalogue = Arc::new(CatalogueCell::new(
        build_catalogue(&db, &game_data, &names).await.unwrap(),
    ));
    let api = Api::new(
        db.clone(),
        game_data,
        Arc::new(RealClock::new()),
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
    .with_mob_detection(catalogue, Some(names), None);
    (api, db, tracker)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_setting_turns_the_watcher_on_and_off_and_off_never_starts_it() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = Arc::new(RecordingWatcher::default());
    let (api, _db, tracker) = detection_api(dir.path(), watcher.clone()).await;

    // Off until the player turns it on: their consent to the screen being
    // read. A session started meanwhile never starts the watcher.
    let settings = api.settings().await.unwrap();
    assert!(!settings.mob_detection_enabled);
    assert_eq!(
        serde_json::to_value(settings.mob_detection_capture).unwrap(),
        "unavailable",
        "a facade with no watcher says detection cannot run"
    );
    tracker.start_session().await.unwrap();
    assert_eq!(
        *watcher.starts.lock().unwrap(),
        0,
        "never started while off"
    );

    let on = api
        .settings_update(SettingsPatch {
            mob_detection_enabled: Some(true),
            ..SettingsPatch::default()
        })
        .await
        .unwrap();
    assert!(on.mob_detection_enabled);
    assert_eq!(
        *watcher.starts.lock().unwrap(),
        1,
        "turned on mid-session, it starts"
    );
    let raw = std::fs::read_to_string(dir.path().join("data/settings.json")).unwrap();
    let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(stored["mob_detection"], true);

    let off = api
        .settings_update(SettingsPatch {
            mob_detection_enabled: Some(false),
            ..SettingsPatch::default()
        })
        .await
        .unwrap();
    assert!(!off.mob_detection_enabled);
    assert_eq!(
        *watcher.stops.lock().unwrap(),
        1,
        "turned off mid-session, it stops"
    );

    // Off: a new session never starts the watcher.
    tracker.stop_session().await.unwrap();
    tracker.start_session().await.unwrap();
    assert_eq!(
        *watcher.starts.lock().unwrap(),
        1,
        "never started while off"
    );
}

/// An ended session with kills of an unlisted creature detected
/// provisionally, and one suggestion the kill did not take.
async fn seed(db: &Db, active: bool) {
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('s1', 1000.0, ?, ?)",
            rusqlite::params![(!active).then_some(2000.0), i64::from(active)],
        )?;
        for (kill, read) in [("k1", "Gloomrat"), ("k2", "Gloomrot")] {
            tx.execute(
                "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                 mob_stamp_source, mob_detection_grade, mob_read_text, timestamp) \
                 VALUES (?, 's1', 'Gloomrat', 'Gloomrat', '', 'detected', 'provisional', ?, 1500.0)",
                rusqlite::params![kill, read],
            )?;
            tx.execute(
                "INSERT INTO kill_mob_detections (kill_id, session_id, grade, mob_name, \
                 mob_species, mob_maturity, provisional_key, read_text, read_at, \
                 declared_name, declared_species, declared_maturity, declared_source) \
                 VALUES (?, 's1', 'provisional', 'Gloomrat', 'Gloomrat', '', 'GLOOMRAT L4 #0', ?, \
                 1499.0, 'Young Atrox', 'Atrox', 'Young', 'declared')",
                rusqlite::params![kill, read],
            )?;
        }
        tx.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             mob_stamp_source, timestamp) \
             VALUES ('k3', 's1', 'Young Atrox', 'Atrox', 'Young', 'declared', 1600.0)",
            [],
        )?;
        tx.execute(
            "INSERT INTO kill_mob_detections (kill_id, session_id, grade, mob_name, \
             mob_species, mob_maturity, read_text, read_at, declared_name, \
             declared_species, declared_maturity, declared_source) \
             VALUES ('k3', 's1', 'latest_read', 'Old Atrox', 'Atrox', 'Old', 'Atrox Old', \
             1599.0, 'Young Atrox', 'Atrox', 'Young', 'declared')",
            [],
        )?;
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn stamp(db: &Db, kill: &'static str) -> (String, String, Option<String>, Option<String>) {
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT mob_name, mob_species, mob_stamp_source, mob_detection_grade \
             FROM kills WHERE id = ?",
            [kill],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?)
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_review_lists_every_detection_and_settles_a_group_in_one_action() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db, _) = detection_api(dir.path(), Arc::default()).await;
    seed(&db, false).await;

    let review = api.tracking_mob_detections("s1".into()).await.unwrap();
    let wire = serde_json::to_value(&review).unwrap();
    assert_eq!(wire["groups"][0]["group"]["grade"], "latest_read");
    assert_eq!(wire["groups"][0]["currentName"], "Young Atrox");
    assert_eq!(wire["groups"][1]["group"]["grade"], "provisional");
    assert_eq!(
        wire["groups"][1]["group"]["provisionalKey"],
        "GLOOMRAT L4 #0"
    );
    assert_eq!(wire["groups"][1]["killCount"], 2);
    let provisional = review.groups[1].group.clone();
    let suggestion = review.groups[0].group.clone();

    // A new name for both kills at once, kept from now on.
    let settled = api
        .tracking_mob_detection_settle(
            "s1".into(),
            provisional.clone(),
            MobDetectionTarget::Creature {
                name: "Gloomrat".into(),
            },
        )
        .await
        .unwrap();
    let MobDetectionSettleResult::Settled {
        settled: kind,
        review: after,
    } = settled
    else {
        panic!("settled, not offered: {settled:?}");
    };
    assert_eq!(
        kind,
        MobDetectionSettled::NewName {
            creature: DetectedCreature {
                name: "Gloomrat".into(),
                species: "Gloomrat".into(),
                maturity: String::new(),
            }
        }
    );
    assert!(after.groups[1].reviewed);
    for kill in ["k1", "k2"] {
        assert_eq!(
            stamp(&db, kill).await,
            (
                "Gloomrat".into(),
                "Gloomrat".into(),
                Some("declared".into()),
                None
            )
        );
    }

    // The suggestion confirmed as the listed creature it named.
    let settled = api
        .tracking_mob_detection_settle(
            "s1".into(),
            suggestion.clone(),
            MobDetectionTarget::Creature {
                name: "Old Atrox".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        settled,
        MobDetectionSettleResult::Settled {
            settled: MobDetectionSettled::Listed { .. },
            ..
        }
    ));
    assert_eq!(stamp(&db, "k3").await.0, "Old Atrox");

    // And back to the declaration.
    api.tracking_mob_detection_settle("s1".into(), suggestion, MobDetectionTarget::Declared)
        .await
        .unwrap();
    assert_eq!(
        stamp(&db, "k3").await,
        (
            "Young Atrox".into(),
            "Atrox".into(),
            Some("declared".into()),
            None
        )
    );

    // Refusals: a name that is not one, a group the session has not.
    let refused = api
        .tracking_mob_detection_settle(
            "s1".into(),
            provisional.clone(),
            MobDetectionTarget::Creature { name: "42".into() },
        )
        .await;
    assert!(
        matches!(refused, Err(ApiError::BadRequest { .. })),
        "{refused:?}"
    );
    let unknown = MobDetectionGroupRef {
        grade: MobDetectionGrade::Named,
        ..provisional
    };
    let refused = api
        .tracking_mob_detection_settle("s1".into(), unknown, MobDetectionTarget::Declared)
        .await;
    assert!(
        matches!(refused, Err(ApiError::NotFound { .. })),
        "{refused:?}"
    );
    assert!(matches!(
        api.tracking_mob_detections("nope".into()).await,
        Err(ApiError::NotFound { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_only_loosely_like_a_listed_creature_is_offered_back() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db, _) = detection_api(dir.path(), Arc::default()).await;
    seed(&db, false).await;
    let group = api
        .tracking_mob_detections("s1".into())
        .await
        .unwrap()
        .groups[1]
        .group
        .clone();
    let offered = api
        .tracking_mob_detection_settle(
            "s1".into(),
            group,
            MobDetectionTarget::Creature {
                name: "Atrok Yung".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&offered).unwrap(),
        serde_json::json!({
            "kind": "didYouMean",
            "creature": {"name": "Young Atrox", "species": "Atrox", "maturity": "Young"}
        })
    );
    assert_eq!(stamp(&db, "k1").await.3.as_deref(), Some("provisional"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_session_is_not_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db, _) = detection_api(dir.path(), Arc::default()).await;
    seed(&db, true).await;
    let group = api
        .tracking_mob_detections("s1".into())
        .await
        .unwrap()
        .groups[1]
        .group
        .clone();
    let refused = api
        .tracking_mob_detection_settle("s1".into(), group, MobDetectionTarget::Declared)
        .await;
    assert!(
        matches!(refused, Err(ApiError::Conflict { .. })),
        "{refused:?}"
    );
    assert_eq!(stamp(&db, "k1").await.2.as_deref(), Some("detected"));
}

/// The snapshot's mob source as the overlay reads it.
async fn mob_source(api: &Api) -> MobSourceView {
    api.tracking_snapshot()
        .await
        .unwrap()
        .mob_source
        .expect("the snapshot carries the mob source")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creature_is_pinned_by_typing_and_released_through_the_facade() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = Arc::new(RecordingWatcher::default());
    let (api, _db, tracker) = detection_api(dir.path(), watcher.clone()).await;
    let typed = |species: &str, maturity: Option<&str>| MobPinTarget::Typed {
        species: species.into(),
        maturity: maturity.map(str::to_string).into(),
    };

    // Idle: only the creature the next session starts with, and no pin.
    api.tracking_manual_mob_lock("Atrox".into(), Some("Young".into()))
        .await
        .unwrap();
    let idle = mob_source(&api).await;
    assert!(!idle.reading_screen);
    assert_eq!(idle.pin.0, None);
    assert_eq!(
        idle.typed.0.map(|creature| creature.name).as_deref(),
        Some("Young Atrox")
    );
    assert!(matches!(
        api.tracking_mob_pin(typed("Atrox", Some("Old"))).await,
        Err(ApiError::Conflict { .. })
    ));

    // Detection on: the session reads the screen, the typed creature its
    // backstop.
    api.settings_update(SettingsPatch {
        mob_detection_enabled: Some(true),
        ..SettingsPatch::default()
    })
    .await
    .unwrap();
    tracker.start_session().await.unwrap();
    let reading = mob_source(&api).await;
    assert!(reading.reading_screen);
    assert_eq!(*watcher.starts.lock().unwrap(), 1);

    // Only a listed creature can be typed to pin.
    assert!(matches!(
        api.tracking_mob_pin(typed("Gloomrat", None)).await,
        Err(ApiError::BadRequest { .. })
    ));
    // Nothing is read yet: a pin from the screen is refused.
    assert!(matches!(
        api.tracking_mob_pin(MobPinTarget::Shown {
            creature: DetectedCreature {
                name: "Old Atrox".into(),
                species: "Atrox".into(),
                maturity: "Old".into(),
            },
        })
        .await,
        Err(ApiError::Conflict { .. })
    ));

    let pinned = api
        .tracking_mob_pin(typed(" Atrox ", Some("Old")))
        .await
        .unwrap();
    assert_eq!(
        pinned,
        MobPin {
            creature: DetectedCreature {
                name: "Old Atrox".into(),
                species: "Atrox".into(),
                maturity: "Old".into(),
            },
            by: MobPinnedBy::Typed,
        }
    );
    assert_eq!(*watcher.stops.lock().unwrap(), 1, "pinned: not read");
    let view = mob_source(&api).await;
    assert!(!view.reading_screen);
    assert_eq!(view.pin.0, Some(pinned));
    let wire = serde_json::to_value(&view).unwrap();
    assert_eq!(wire["pin"]["by"], "typed");
    assert_eq!(wire["readingScreen"], false);

    assert!(api.tracking_mob_unpin().await.unwrap());
    assert_eq!(*watcher.starts.lock().unwrap(), 2, "released: read again");
    assert!(!api.tracking_mob_unpin().await.unwrap());
    assert!(mob_source(&api).await.reading_screen);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_creature_list_checks_keeps_edits_and_removes_a_creature_typed_as_its_plate_prints_it()
{
    let dir = tempfile::tempdir().unwrap();
    let (api, _db, _) = detection_api(dir.path(), Arc::default()).await;
    assert!(api.creature_entries().await.unwrap().is_empty());
    // A creature ending in a number: ready, at any number.
    let check = api
        .creature_entry_check("Gloomrat Brute 04".into(), None)
        .await
        .unwrap();
    assert_eq!(
        check,
        CreatureEntryCheck::Ready {
            name: "Gloomrat Brute".into(),
            maturity: Some("04".to_string()).into(),
            any_number: true,
        }
    );
    // One the bundled list has: known, by its own name and as its kills
    // are recorded.
    match api
        .creature_entry_check("Atrox Old".into(), None)
        .await
        .unwrap()
    {
        CreatureEntryCheck::Known {
            name,
            maturity,
            recorded_as,
        } => {
            assert_eq!(name, "Atrox");
            assert_eq!(maturity.0.as_deref(), Some("Old"));
            assert_eq!(recorded_as.name, "Old Atrox");
        }
        other => panic!("{other:?}"),
    }
    let kept = api
        .creature_entry_save("Gloomrat Brute 04".into(), Some("Gloomrats".into()), None)
        .await
        .unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "Gloomrat Brute");
    assert_eq!(kept[0].species.0.as_deref(), Some("Gloomrats"));
    assert_eq!(kept[0].maturity.0.as_deref(), Some("04"));
    assert!(kept[0].any_number);
    assert_eq!(kept[0].source, CreatureEntrySource::Player);
    // Kept again: refused as already known, and nothing changes.
    let refused = api
        .creature_entry_save("Gloomrat Brute 04".into(), None, None)
        .await;
    assert!(
        matches!(refused, Err(ApiError::BadRequest { .. })),
        "{refused:?}"
    );
    // Edited in place, under another name.
    let kept = api
        .creature_entry_save(
            "Gloomrat Bruiser 04".into(),
            None,
            Some("Gloomrat Brute".into()),
        )
        .await
        .unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "Gloomrat Bruiser");
    // Removed; removing it again finds nothing.
    assert!(api
        .creature_entry_remove("Gloomrat Bruiser".into())
        .await
        .unwrap()
        .is_empty());
    let again = api.creature_entry_remove("Gloomrat Bruiser".into()).await;
    assert!(matches!(again, Err(ApiError::NotFound { .. })), "{again:?}");
}
