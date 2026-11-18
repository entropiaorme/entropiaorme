//! Behavioural pins for the review of what the screen read as the tool in
//! hand that is not in the library, over the typed facade: the typed
//! review, each settlement and its undo, the refusals, that adding moves no
//! figure of the session, and that the held-tool catalogue follows every
//! change to the library and the dismissed names.

use std::path::Path;
use std::sync::Arc;

use eo_api::equipment::{EquipmentKind, EquipmentRequest};
use eo_api::tool_sighting::{
    SightedToolKind, ToolEntryKeptBecause, ToolSighted, ToolSightingAction, ToolSightingChange,
    ToolSightingEntryOnUndo, ToolSightingRefusal, ToolSightingReview, ToolSightingSettleResult,
    ToolSightingSettled, ToolSightingSettledKind, ToolUsesCosting,
};
use eo_api::{Api, ApiError};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::tool_bar::ToolName;
use eo_services::game_ui::tool_names::{resolve, ToolMatch, ToolNameProfile};
use eo_services::tool_catalogue::ToolCatalogues;
use eo_services::tool_watch::ToolCatalogueCell;

use crate::common;

/// A weapon, a healer whose catalogue gives its heal, and one whose
/// catalogue does not.
fn write_snapshot(dir: &Path) {
    std::fs::write(
        dir.join("weapons.json"),
        r#"[{"id": "42", "name": "Sollomate Opalo", "damage": {"impact": 8}, "economy": {"decay": 5.0, "ammo_burn": 0}}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("medical_tools.json"),
        r#"[{"id": "7", "name": "Vivo T1", "min_heal": 8, "max_heal": 12, "economy": {"decay": 1}},
            {"id": "8", "name": "Plain Chip", "economy": {"decay": 1}}]"#,
    )
    .unwrap();
}

struct Rig {
    _dir: tempfile::TempDir,
    api: Api,
    db: Db,
    cell: Arc<ToolCatalogueCell>,
    names: ToolNameProfile,
}

/// The facade over the snapshot above, with the held-tool reading on so
/// the catalogue is rebuilt at every change, and an ended session `s`
/// seeded by [`seed`].
async fn rig() -> Rig {
    rig_over(None).await
}

/// [`rig`] over `snapshot` when given (the game's own data), else the
/// snapshot above.
async fn rig_over(snapshot: Option<std::path::PathBuf>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = snapshot.unwrap_or_else(|| {
        let written = dir.path().join("snapshot");
        std::fs::create_dir_all(&written).unwrap();
        write_snapshot(&written);
        written
    });
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("snapshot store"));
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let catalogues = ToolCatalogues::new(db.clone(), game_data.clone(), Arc::new(|| true));
    catalogues.ensure_current().await;
    let cell = catalogues.cell();
    let api = Api::new(
        db.clone(),
        game_data,
        Arc::new(eo_services::clock::RealClock::new()),
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
    .with_tool_reading(catalogues, None);
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources/ui_profile.json");
    let names = eo_services::game_ui::profile::UiProfile::load(&path)
        .expect("the bundled interface profile loads")
        .tool_names;
    seed(&db, "s", false).await;
    Rig {
        _dir: dir,
        api,
        db,
        cell,
        names,
    }
}

/// A session whose two kills were made with the weapon of the game's data,
/// its shots booked under its name at its cost without attachments, with
/// its summary, days and cells settled; it saw that weapon (`b`), the two
/// healers (`h`, `p`), a tool the game's data no longer holds (`gone`),
/// and a name matching no tool (`u`).
async fn seed(db: &Db, session_id: &'static str, active: bool) {
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES (?1, 1000, ?2, ?3)",
            rusqlite::params![session_id, (!active).then_some(2000.0), active as i64],
        )?;
        let id = |name: &str| format!("{session_id}-{name}");
        for (kill, at, shots) in [("k1", 1100.0, 2), ("k2", 1200.0, 1)] {
            tx.execute(
                "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                 timestamp, shots_fired, cost_ped, loot_total_ped) \
                 VALUES (?1, ?2, 'Young Atrox', 'Atrox', 'Young', ?3, ?4, ?5, 0.3)",
                rusqlite::params![id(kill), session_id, at, shots, 0.05 * f64::from(shots)],
            )?;
            tx.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
                 cost_per_shot) VALUES (?1, 'Sollomate Opalo', ?2, 16, 0.05)",
                rusqlite::params![id(kill), shots],
            )?;
        }
        for (name, endpoint, item, item_name, at) in [
            ("b", "weapons", "42", "Sollomate Opalo", 1050.0),
            ("h", "medical_tools", "7", "Vivo T1", 1300.0),
            ("p", "medical_tools", "8", "Plain Chip", 1310.0),
            ("gone", "weapons", "99", "Retired Rifle", 1320.0),
        ] {
            tx.execute(
                "INSERT INTO tool_sightings (id, session_id, kind, endpoint, item_id, item_name, \
                 first_seen, last_seen, shots, cost_per_shot) \
                 VALUES (?1, ?2, 'bundled', ?3, ?4, ?5, ?6, ?6, ?7, 0.05)",
                rusqlite::params![
                    id(name),
                    session_id,
                    endpoint,
                    item,
                    item_name,
                    at,
                    (name == "b").then_some(3)
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO tool_sightings (id, session_id, kind, text_key, text_read, first_seen, \
             last_seen) VALUES (?1, ?2, 'unknown', 'teleport chip', 'Teleport Chip', 1400, 1400)",
            rusqlite::params![id("u"), session_id],
        )?;
        eo_services::session_summary::write_session_summary(&tx, session_id)?;
        eo_services::daily_rollup::refresh_session_days(&tx, session_id)?;
        eo_services::session_rollup::recompute_session(&tx, session_id)?;
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
}

impl Rig {
    async fn settle(&self, id: &str, action: ToolSightingAction) -> ToolSightingSettleResult {
        self.api
            .tracking_tool_sighting_settle("s".into(), format!("s-{id}"), action)
            .await
            .unwrap()
    }

    async fn undo(&self, id: &str) -> ToolSightingSettleResult {
        self.api
            .tracking_tool_sighting_undo("s".into(), format!("s-{id}"))
            .await
            .unwrap()
    }

    fn library(&self) -> Vec<String> {
        self.cell
            .current()
            .library()
            .map(|tool| tool.name.clone())
            .collect()
    }

    fn reads(&self, text: &str) -> ToolMatch {
        let name = ToolName {
            text: text.into(),
            cut_short: false,
        };
        resolve(&name, &self.cell.current(), &self.names)
    }

    async fn detail(&self) -> serde_json::Value {
        serde_json::to_value(self.api.tracking_session_detail("s".into()).await.unwrap()).unwrap()
    }
}

fn settled(result: ToolSightingSettleResult) -> (ToolSightingChange, ToolSightingReview) {
    match result {
        ToolSightingSettleResult::Settled { change, review } => (change, review),
        refused => panic!("settled: {refused:?}"),
    }
}

fn refusal(result: ToolSightingSettleResult) -> ToolSightingRefusal {
    match result {
        ToolSightingSettleResult::Refused { reason } => reason,
        done => panic!("refused: {done:?}"),
    }
}

fn state(review: &ToolSightingReview, id: &str) -> Option<ToolSightingSettled> {
    review
        .sightings
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("{id} listed"))
        .settled
        .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_review_lists_tools_then_names_in_closed_vocabularies() {
    let rig = rig().await;
    seed(&rig.db, "live", true).await;
    let review = rig.api.tracking_tool_sightings("s".into()).await.unwrap();
    assert_eq!(review.session_id, "s");
    assert!(review.ended);
    let ids: Vec<&str> = review.sightings.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["s-b", "s-h", "s-p", "s-gone", "s-u"]);
    assert_eq!(
        review.sightings[0].sighted,
        ToolSighted::Bundled {
            name: "Sollomate Opalo".into(),
            tool: SightedToolKind::Weapon,
            shots: Some(3).into(),
        }
    );
    assert_eq!(
        review.sightings[4].sighted,
        ToolSighted::Unknown {
            text: "Teleport Chip".into()
        }
    );
    assert!(review.sightings.iter().all(|s| s.settled.is_none()));

    let live = rig
        .api
        .tracking_tool_sightings("live".into())
        .await
        .unwrap();
    assert!(
        !live.ended,
        "a running session's sightings are shown as they stand"
    );

    assert!(matches!(
        rig.api.tracking_tool_sightings("nowhere".into()).await,
        Err(ApiError::NotFound { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_a_weapon_makes_its_entry_known_at_once_and_moves_no_figure() {
    let rig = rig().await;
    let before = rig.detail().await;
    assert!(matches!(
        rig.reads("Sollomate Opalo"),
        ToolMatch::Bundled { .. }
    ));

    let (change, review) = settled(rig.settle("b", ToolSightingAction::Add).await);
    let ToolSightingChange::Added {
        created: true,
        uses: ToolUsesCosting::WithoutAttachments,
        entry,
    } = change
    else {
        panic!("added: {change:?}");
    };
    assert_eq!(entry.label, "Sollomate Opalo");
    assert!(entry.enabled);
    assert_eq!(
        state(&review, "s-b"),
        Some(ToolSightingSettled::Added { created: true })
    );
    let library = rig.api.equipment_library().await.unwrap();
    assert_eq!(library.entries.len(), 1);
    assert_eq!(library.entries[0].name, "Sollomate Opalo");
    assert_eq!(rig.library(), ["Sollomate Opalo"], "the reader knows it");
    assert!(matches!(
        rig.reads("Sollomate Opalo"),
        ToolMatch::Library { .. }
    ));

    // Its shots were booked under its name at the cost the bare entry
    // prices them at: the session reads exactly as before.
    assert_eq!(rig.detail().await, before);

    // Undone: the entry it made leaves the library and the reader, and the
    // sighting is asked about again.
    let (change, review) = settled(rig.undo("b").await);
    assert_eq!(
        change,
        ToolSightingChange::Reopened {
            was: ToolSightingSettledKind::Added,
            entry: ToolSightingEntryOnUndo::Removed
        }
    );
    assert_eq!(state(&review, "s-b"), None);
    assert!(rig
        .api
        .equipment_library()
        .await
        .unwrap()
        .entries
        .is_empty());
    assert!(rig.library().is_empty());
    assert_eq!(rig.detail().await, before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_links_to_an_entry_of_the_same_item_and_undo_keeps_it() {
    let rig = rig().await;
    let req: EquipmentRequest = serde_json::from_value(serde_json::json!({
        "type": "weapon",
        "catalog_id": "42",
    }))
    .unwrap();
    assert_eq!(req.kind, EquipmentKind::Weapon);
    let held = rig.api.equipment_add(&req).await.unwrap();

    let (change, _) = settled(rig.settle("b", ToolSightingAction::Add).await);
    let ToolSightingChange::Added {
        created: false,
        uses: ToolUsesCosting::WithoutAttachments,
        entry,
    } = change
    else {
        panic!("linked: {change:?}");
    };
    assert_eq!((entry.id.as_str(), entry.enabled), (held.id.as_str(), true));
    assert_eq!(rig.api.equipment_library().await.unwrap().entries.len(), 1);
    let (change, _) = settled(rig.undo("b").await);
    assert_eq!(
        change,
        ToolSightingChange::Reopened {
            was: ToolSightingSettledKind::Added,
            entry: ToolSightingEntryOnUndo::None
        }
    );
    assert_eq!(rig.api.equipment_library().await.unwrap().entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_healer_is_added_with_its_heals_left_uncosted_or_refused_without_its_heal() {
    let rig = rig().await;
    let (change, _) = settled(rig.settle("h", ToolSightingAction::Add).await);
    assert!(matches!(
        change,
        ToolSightingChange::Added {
            created: true,
            uses: ToolUsesCosting::NotCosted,
            ..
        }
    ));
    let library = rig.api.equipment_library().await.unwrap();
    assert_eq!(library.entries[0].name, "Vivo T1");

    assert_eq!(
        refusal(rig.settle("p", ToolSightingAction::Add).await),
        ToolSightingRefusal::NeedsSetUp
    );
    assert_eq!(
        refusal(rig.settle("gone", ToolSightingAction::Add).await),
        ToolSightingRefusal::NotInGameData
    );
    assert_eq!(rig.api.equipment_library().await.unwrap().entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dismissed_name_reads_as_not_a_tool_until_the_dismissal_is_undone() {
    let rig = rig().await;
    assert!(!matches!(
        rig.reads("Teleport Chip"),
        ToolMatch::NotTracked { .. }
    ));

    let (change, review) = settled(rig.settle("u", ToolSightingAction::Dismiss).await);
    assert_eq!(change, ToolSightingChange::Dismissed);
    assert_eq!(state(&review, "s-u"), Some(ToolSightingSettled::Dismissed));
    assert!(
        matches!(rig.reads("Teleport Chip"), ToolMatch::NotTracked { .. }),
        "the reader takes it as not a tool at once"
    );

    let (change, review) = settled(rig.undo("u").await);
    assert_eq!(
        change,
        ToolSightingChange::Reopened {
            was: ToolSightingSettledKind::Dismissed,
            entry: ToolSightingEntryOnUndo::None
        }
    );
    assert_eq!(state(&review, "s-u"), None);
    assert!(!matches!(
        rig.reads("Teleport Chip"),
        ToolMatch::NotTracked { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_refusal_names_its_reason_and_changes_nothing() {
    let rig = rig().await;
    seed(&rig.db, "live", true).await;
    let before = rig.api.tracking_tool_sightings("s".into()).await.unwrap();

    let running = rig
        .api
        .tracking_tool_sighting_settle("live".into(), "live-b".into(), ToolSightingAction::Add)
        .await
        .unwrap();
    assert_eq!(refusal(running), ToolSightingRefusal::SessionRunning);
    let elsewhere = rig
        .api
        .tracking_tool_sighting_settle("s".into(), "live-u".into(), ToolSightingAction::Leave)
        .await
        .unwrap();
    assert_eq!(refusal(elsewhere), ToolSightingRefusal::OtherSession);
    assert_eq!(
        refusal(rig.settle("u", ToolSightingAction::Add).await),
        ToolSightingRefusal::NotATool
    );
    assert_eq!(
        refusal(rig.settle("missing", ToolSightingAction::Leave).await),
        ToolSightingRefusal::NotFound
    );
    assert_eq!(
        refusal(rig.undo("b").await),
        ToolSightingRefusal::NotSettled
    );
    assert_eq!(
        rig.api.tracking_tool_sightings("s".into()).await.unwrap(),
        before
    );
    assert!(rig
        .api
        .equipment_library()
        .await
        .unwrap()
        .entries
        .is_empty());

    // Settled once, a second settlement says what it already is.
    settled(rig.settle("b", ToolSightingAction::Leave).await);
    for action in [
        ToolSightingAction::Add,
        ToolSightingAction::Dismiss,
        ToolSightingAction::Leave,
    ] {
        assert_eq!(
            refusal(rig.settle("b", action).await),
            ToolSightingRefusal::AlreadySettled {
                settled: ToolSightingSettledKind::Left
            }
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_links_to_a_turned_off_entry_and_says_so() {
    let rig = rig().await;
    let req: EquipmentRequest = serde_json::from_value(serde_json::json!({
        "type": "weapon",
        "catalog_id": "42",
    }))
    .unwrap();
    let held = rig.api.equipment_add(&req).await.unwrap();
    rig.api
        .equipment_set_enabled(held.id.parse().unwrap(), false)
        .await
        .unwrap();

    let (change, _) = settled(rig.settle("b", ToolSightingAction::Add).await);
    let ToolSightingChange::Added {
        created: false,
        entry,
        ..
    } = change
    else {
        panic!("linked: {change:?}");
    };
    assert_eq!(entry.id, held.id);
    assert_eq!(entry.label, "Sollomate Opalo");
    assert!(!entry.enabled, "the entry linked to is off, and says so");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_added_entry_bound_to_a_hotbar_slot_stays_through_an_undo() {
    let rig = rig().await;
    let (change, _) = settled(rig.settle("b", ToolSightingAction::Add).await);
    let ToolSightingChange::Added { entry, .. } = change else {
        panic!("added: {change:?}");
    };
    let id: i64 = entry.id.parse().unwrap();
    let mut slots = serde_json::Map::new();
    slots.insert("1".into(), serde_json::json!(id));
    rig.api
        .settings_update(eo_api::settings::SettingsPatch {
            hotbar: Some(slots),
            ..Default::default()
        })
        .await
        .unwrap();

    let (change, review) = settled(rig.undo("b").await);
    assert_eq!(
        change,
        ToolSightingChange::Reopened {
            was: ToolSightingSettledKind::Added,
            entry: ToolSightingEntryOnUndo::Kept {
                because: ToolEntryKeptBecause::InSettings
            }
        }
    );
    assert_eq!(state(&review, "s-b"), None);
    assert_eq!(
        rig.api.equipment_library().await.unwrap().entries.len(),
        1,
        "the bound entry stays"
    );
    assert_eq!(rig.library(), ["Sollomate Opalo"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_set_aside_is_listed_and_brought_back_from_equipment() {
    let rig = rig().await;
    assert!(rig
        .api
        .equipment_dismissed_tools()
        .await
        .unwrap()
        .is_empty());
    settled(rig.settle("u", ToolSightingAction::Dismiss).await);
    let listed = rig.api.equipment_dismissed_tools().await.unwrap();
    assert_eq!(
        listed.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        ["Teleport Chip"]
    );
    assert!(matches!(
        rig.reads("Teleport Chip"),
        ToolMatch::NotTracked { .. }
    ));

    let left = rig
        .api
        .equipment_track_again("Teleport Chip".into())
        .await
        .unwrap();
    assert!(left.is_empty());
    assert!(
        !matches!(rig.reads("Teleport Chip"), ToolMatch::NotTracked { .. }),
        "the reader matches it again at once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_session_that_set_a_name_aside_brings_it_back() {
    let rig = rig().await;
    seed(&rig.db, "t", false).await;
    settled(rig.settle("b", ToolSightingAction::Dismiss).await);
    assert!(matches!(
        rig.reads("Sollomate Opalo"),
        ToolMatch::NotTracked { .. }
    ));

    rig.api.tracking_session_delete("s".into()).await.unwrap();
    assert!(rig
        .api
        .equipment_dismissed_tools()
        .await
        .unwrap()
        .is_empty());
    assert!(
        matches!(rig.reads("Sollomate Opalo"), ToolMatch::Bundled { .. }),
        "the reader costs the weapon again"
    );
}

/// Each kind of tool, a spread of real items of the game's data each: the
/// entry adding makes has exactly the name and the cost per use the tracker
/// was given for the tool before it was in the library, priced as the
/// tracker prices a library entry, under the same reload speed. So adding
/// moves no figure of the session, whatever the item.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_added_entry_costs_what_the_tool_was_costed_at_before_it_was_added() {
    use eo_services::attack_rate::with_attack_rate;
    use eo_services::bundled_tools::bare_tool;
    use eo_services::equipment_pricing::{cost_per_shot_ped, heal_cost_from_props};

    let snapshot =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources/snapshot");
    let store = GameDataStore::new(&snapshot).expect("the game's data");
    let rig = rig_over(Some(snapshot)).await;

    let mut picked: Vec<(&str, serde_json::Value)> = Vec::new();
    for endpoint in ["weapons", "medical_tools", "harvesting_tools"] {
        let entities: Vec<&serde_json::Value> = store
            .get_entities(endpoint)
            .iter()
            .filter(|entity| {
                endpoint != "medical_tools"
                    || (entity.get("min_heal").is_some_and(|v| !v.is_null())
                        && entity.get("max_heal").is_some_and(|v| !v.is_null()))
            })
            .collect();
        let step = (entities.len() / 7).max(1);
        for entity in entities.iter().step_by(step).take(7) {
            picked.push((endpoint, (*entity).clone()));
        }
    }
    assert!(picked.len() >= 7 * 2 + 7, "a spread of each kind");

    let ids: Vec<(String, &str, serde_json::Value)> = picked
        .iter()
        .enumerate()
        .map(|(n, (endpoint, entity))| (format!("real-{n}"), *endpoint, entity.clone()))
        .collect();
    let rows = ids.clone();
    rig.db
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES ('real', 1000, 2000, 0)",
                [],
            )?;
            for (id, endpoint, entity) in &rows {
                let item_id = match &entity["id"] {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                conn.execute(
                    "INSERT INTO tool_sightings (id, session_id, kind, endpoint, item_id, \
                     item_name, first_seen, last_seen) \
                     VALUES (?1, 'real', 'bundled', ?2, ?3, ?4, 1100, 1100)",
                    rusqlite::params![id, endpoint, item_id, entity["name"].as_str()],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    for (id, endpoint, entity) in &ids {
        let added = rig
            .api
            .tracking_tool_sighting_settle("real".into(), id.clone(), ToolSightingAction::Add)
            .await
            .unwrap();
        let (change, _) = settled(added);
        let ToolSightingChange::Added { entry, .. } = change else {
            panic!("{id}: added: {change:?}");
        };
        let entry_id: i64 = entry.id.parse().unwrap();
        let (name, props_json): (String, String) = rig
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT name, properties_json FROM equipment_library WHERE id = ?1",
                    [entry_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        let props: serde_json::Value = serde_json::from_str(&props_json).unwrap();
        let item_id = match &entity["id"] {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        for reload in [0.0, 20.0] {
            let bare = bare_tool(endpoint, &item_id, entity, Some(&store), reload)
                .unwrap_or_else(|| panic!("{id}: a tool of {endpoint}"));
            assert_eq!(name, bare.name, "{endpoint} {item_id}: the name");
            let library_cost = if *endpoint == "weapons" {
                cost_per_shot_ped(&with_attack_rate(&props, Some(&store), reload))
            } else {
                heal_cost_from_props(&props_json).0
            };
            assert!(
                (library_cost - bare.cost_per_use_ped).abs() < 1e-12,
                "{endpoint} {item_id} at reload {reload}: library {library_cost} against \
                 the tracker's {}",
                bare.cost_per_use_ped
            );
        }
    }
}
