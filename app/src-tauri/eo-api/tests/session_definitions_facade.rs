//! Behavioural pins for the session-definitions family over the typed
//! facade: the empty-database read, the create / read-back / update
//! (roster replaced wholesale) / archive / restore ladder with its not-found and
//! validation legs, the transport-invariance byte pin, and the
//! tracking-family selection verb (config writeback, snapshot
//! exposure, the free-text-rename disavow, and the fixed-while-active
//! conflict).

use std::path::Path;
use std::sync::Arc;

use eo_api::quests::QuestFamilyInput;
use eo_api::session_definitions::{
    SessionDefinitionInput, SessionRosterEntryInput, SessionRosterEntryKind,
};
use eo_api::settings::SettingsPatch;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot, with the database handle kept for timestamp
/// pinning (`created_at` stamps from the wall clock).
async fn definitions_api(dir: &Path) -> (Api, Db) {
    definitions_api_with_settings(dir, None).await
}

/// [`definitions_api`] with a seeded `settings.json` (the conflict pin
/// starts a real session, which needs a bound hotbar slot so the start
/// satisfies the tool gate).
async fn definitions_api_with_settings(dir: &Path, settings: Option<&str>) -> (Api, Db) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    if let Some(settings) = settings {
        std::fs::write(data_dir.join("settings.json"), settings).unwrap();
    }
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let api = Api::new(
        db.clone(),
        game_data,
        clock,
        data_dir,
        handles.config_service,
        handles.tracker,
        handles.hotbar,
        handles.watcher,
        handles.skill_tracker,
        handles.skill_scan,
        handles.spacebar,
        handles.repair_ocr,
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
        None,
    );
    (api, db)
}

fn definition(name: &str, roster: Vec<SessionRosterEntryInput>) -> SessionDefinitionInput {
    SessionDefinitionInput {
        name: name.to_string(),
        ad_hoc_segments: false,
        track_protection_costs: true,
        roster,
    }
}

fn segment(label: &str) -> SessionRosterEntryInput {
    SessionRosterEntryInput {
        kind: SessionRosterEntryKind::Segment,
        ref_id: None,
        label: Some(label.to_string()),
    }
}

/// Pin a definition's authored stamp, oldest id first, so list order is
/// deterministic under the byte-for-byte assertions.
async fn pin_timestamps(db: &Db, definition_id: i64) {
    let stamp = 1000.0 * definition_id as f64;
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE session_definitions SET created_at = ?, updated_at = NULL \
             WHERE id = ?",
            rusqlite::params![stamp, definition_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// A fresh database is never definition-less: tracking always needs one
/// to run under, so the migration seeds a protected default and the list
/// answers with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_answers_the_fresh_database_with_the_protected_default() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;
    let listed = api.session_definitions_list(None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "1");
    assert_eq!(listed[0].name, "Default Tracking");
    assert!(listed[0].is_protected);
    // The seeded default declares nothing: no roster and no self-named
    // segments. A session that has neither is the one with no activity
    // structure at all, so once the overlay gains its activity control
    // there is nothing for it to show here rather than empty controls.
    assert!(!listed[0].ad_hoc_segments);
    assert!(listed[0].roster.is_empty());

    // Selecting it and starting is the untouched-install path: the
    // session records under it, still declaring nothing.
    api.tracking_definition_select(Some(1)).await.unwrap();
    let snapshot = api.tracking_snapshot().await.unwrap();
    assert_eq!(snapshot.session_definition_id, Some("1".to_string()));
    assert_eq!(snapshot.session_name, Some("Default Tracking".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_reads_back_the_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = definitions_api(dir.path()).await;

    // A roster spanning all three kinds: a family (created over the
    // facade), a quest (seeded directly; the quest DTO ladder is the
    // quests family's own pin), and a plain segment label.
    api.quest_family_create(QuestFamilyInput {
        name: "Daily Hunting 1".to_string(),
        planet: "ARIS".to_string(),
        cooldown_hours: Some(20.0),
        cooldown_anchor: None,
    })
    .await
    .unwrap();
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO quests (name) VALUES ('The Ultimate Threat')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let created = api
        .session_definition_create(SessionDefinitionInput {
            name: "ARIS Dailies".to_string(),
            ad_hoc_segments: true,
            track_protection_costs: true,
            roster: vec![
                SessionRosterEntryInput {
                    kind: SessionRosterEntryKind::QuestFamily,
                    ref_id: Some(1),
                    label: None,
                },
                SessionRosterEntryInput {
                    kind: SessionRosterEntryKind::Quest,
                    ref_id: Some(1),
                    label: None,
                },
                segment("Warm-up"),
            ],
        })
        .await
        .unwrap();
    // Id 1 is the seeded default, so the first authored definition is 2.
    assert_eq!(created.id, "2");

    // Transport invariance: the exact wire bytes, with the wall-clock
    // stamps pinned first. The seeded default is pinned too, so the
    // list's authored-order sort is deterministic rather than a race
    // between a pinned stamp and a real one.
    pin_timestamps(&db, 1).await;
    pin_timestamps(&db, 2).await;
    let listed = api.session_definitions_list(None).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        serde_json::to_string(&listed[1]).unwrap(),
        "{\"id\":\"2\",\"name\":\"ARIS Dailies\",\"adHocSegments\":true,\
         \"trackProtectionCosts\":true,\
         \"isProtected\":false,\"isActive\":true,\
         \"instanceCount\":0,\"createdAt\":2000.0,\"updatedAt\":null,\"roster\":[\
         {\"id\":\"1\",\"kind\":\"quest_family\",\"refId\":\"1\",\"label\":null,\
         \"displayName\":\"Daily Hunting 1\"},\
         {\"id\":\"2\",\"kind\":\"quest\",\"refId\":\"1\",\"label\":null,\
         \"displayName\":\"The Ultimate Threat\"},\
         {\"id\":\"3\",\"kind\":\"segment\",\"refId\":null,\"label\":\"Warm-up\",\
         \"displayName\":\"Warm-up\"}]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_update_archive_and_restore_ladder_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;

    let created = api
        .session_definition_create(definition("General Hunting", vec![segment("Travel")]))
        .await
        .unwrap();

    // Update replaces the roster wholesale and flips the flag.
    let updated = api
        .session_definition_update(
            2,
            SessionDefinitionInput {
                name: "General Hunting".to_string(),
                ad_hoc_segments: true,
                track_protection_costs: true,
                roster: vec![segment("Grind"), segment("Wind-down")],
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.id, created.id);
    assert!(updated.ad_hoc_segments);
    let labels: Vec<_> = updated
        .roster
        .iter()
        .map(|entry| entry.label.as_deref().unwrap().to_string())
        .collect();
    assert_eq!(labels, vec!["Grind", "Wind-down"]);

    // The not-found legs: an unknown id on update and archive, and both
    // again after the archive (an archived definition reads as absent to
    // active-only writes).
    assert_eq!(
        api.session_definition_update(99, definition("X", vec![]))
            .await
            .unwrap_err(),
        ApiError::not_found("Session definition not found")
    );
    assert_eq!(
        api.session_definition_archive(99).await.unwrap_err(),
        ApiError::not_found("Active session definition not found")
    );
    let archived = api.session_definition_archive(2).await.unwrap();
    assert!(!archived.is_active);
    assert_eq!(archived.roster.len(), 2);
    let remaining = api.session_definitions_list(None).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "Default Tracking");
    assert_eq!(
        api.session_definition_archive(2).await.unwrap_err(),
        ApiError::not_found("Active session definition not found")
    );
    assert_eq!(
        api.session_definition_update(2, definition("X", vec![]))
            .await
            .unwrap_err(),
        ApiError::not_found("Session definition not found")
    );

    let restored = api.session_definition_restore(2).await.unwrap();
    assert!(restored.is_active);
    assert_eq!(restored.roster.len(), 2);
    assert_eq!(
        api.session_definition_restore(2).await.unwrap_err(),
        ApiError::not_found("Archived session definition not found")
    );

    // The protected default refuses archiving in the service, not merely
    // in the UI: something must always be there to track under. It is
    // otherwise an ordinary definition, so a rename still lands.
    let renamed = api
        .session_definition_update(1, definition("General Play", vec![segment("Roam")]))
        .await
        .unwrap();
    assert_eq!(renamed.name, "General Play");
    assert!(renamed.is_protected);
    assert_eq!(
        api.session_definition_archive(1).await.unwrap_err(),
        ApiError::bad_request(
            "General Play cannot be archived; tracking always needs one to run under"
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_rejections_are_bad_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;

    assert!(matches!(
        api.session_definition_create(definition("   ", vec![]))
            .await
            .unwrap_err(),
        ApiError::BadRequest { .. }
    ));
    assert!(matches!(
        api.session_definition_create(definition("A", vec![segment("  ")]))
            .await
            .unwrap_err(),
        ApiError::BadRequest { .. }
    ));
    assert!(matches!(
        api.session_definition_create(definition(
            "A",
            vec![SessionRosterEntryInput {
                kind: SessionRosterEntryKind::QuestFamily,
                ref_id: Some(99),
                label: None,
            }],
        ))
        .await
        .unwrap_err(),
        ApiError::BadRequest { .. }
    ));

    // Duplicate active names split one family's instances invisibly.
    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();
    assert!(matches!(
        api.session_definition_create(definition("aris dailies", vec![]))
            .await
            .unwrap_err(),
        ApiError::BadRequest { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selection_writes_the_config_and_the_snapshot_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;

    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();

    // An unknown id is a 404 and writes nothing.
    assert_eq!(
        api.tracking_definition_select(Some(99)).await.unwrap_err(),
        ApiError::not_found("Session definition not found")
    );

    // Selection acknowledges and writes both config facets; the idle
    // snapshot reports them.
    let selected = api.tracking_definition_select(Some(2)).await.unwrap();
    assert_eq!(selected.session_definition_id, Some("2".to_string()));
    assert_eq!(selected.session_name, Some("ARIS Dailies".to_string()));
    let snapshot = api.tracking_snapshot().await.unwrap();
    assert_eq!(snapshot.session_definition_id, Some("2".to_string()));
    assert_eq!(snapshot.session_name, Some("ARIS Dailies".to_string()));

    // A boost-only config write (name unchanged) keeps the selection; a
    // free-text rename disavows it.
    api.tracking_session_config(Some("ARIS Dailies".into()), Some(50))
        .await
        .unwrap();
    let kept = api.tracking_snapshot().await.unwrap();
    assert_eq!(kept.session_definition_id, Some("2".to_string()));
    api.tracking_session_config(Some("Something Else".into()), None)
        .await
        .unwrap();
    // A free-text rename still disavows the authored selection; what it
    // falls back to is the protected default rather than nothing, while
    // the typed name stays the user's own declaration.
    let disavowed = api.tracking_snapshot().await.unwrap();
    assert_eq!(disavowed.session_definition_id, Some("1".to_string()));
    assert_eq!(disavowed.session_name, Some("Something Else".to_string()));

    // Withdrawing the selection clears the config facets, and the
    // readout resolves to the protected default: "nothing in
    // particular" is a definition, not a hole.
    api.tracking_definition_select(Some(2)).await.unwrap();
    api.tracking_definition_select(None).await.unwrap();
    let cleared = api.tracking_snapshot().await.unwrap();
    assert_eq!(cleared.session_definition_id, Some("1".to_string()));
    assert_eq!(cleared.session_name, Some("Default Tracking".to_string()));

    // Archiving the selected definition writes the protected fallback in
    // the same operation, so the readout and the next start agree.
    api.tracking_definition_select(Some(2)).await.unwrap();
    api.session_definition_archive(2).await.unwrap();
    let fallback = api.tracking_snapshot().await.unwrap();
    assert_eq!(fallback.session_definition_id, Some("1".to_string()));
    assert_eq!(fallback.session_name, Some("Default Tracking".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selection_and_archive_never_persist_an_archived_definition() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;

    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();

    for _ in 0..32 {
        api.tracking_definition_select(Some(1)).await.unwrap();
        let (selection, archive) = tokio::join!(
            api.tracking_definition_select(Some(2)),
            api.session_definition_archive(2)
        );
        archive.unwrap();
        match selection {
            Ok(_) => {}
            Err(error) => assert_eq!(error, ApiError::not_found("Session definition not found")),
        }
        let snapshot = api.tracking_snapshot().await.unwrap();
        assert_eq!(snapshot.session_definition_id, Some("1".to_string()));
        assert_eq!(snapshot.session_name, Some("Default Tracking".to_string()));
        api.session_definition_restore(2).await.unwrap();
    }
}

/// The selection is what writes the name facet, so a name arriving by
/// any other route is a free-text declaration and disavows it. The
/// settings patch is the other route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_written_through_settings_disavows_the_selection() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api(dir.path()).await;

    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();
    api.tracking_definition_select(Some(2)).await.unwrap();
    assert_eq!(
        api.tracking_snapshot().await.unwrap().session_definition_id,
        Some("2".to_string())
    );

    api.settings_update(SettingsPatch {
        session_name: Some("Something Else".to_string()),
        ..Default::default()
    })
    .await
    .unwrap();

    // Disavowed, and resolving to the protected default rather than to
    // nothing; the typed name stays the user's own declaration.
    let snapshot = api.tracking_snapshot().await.unwrap();
    assert_eq!(snapshot.session_definition_id, Some("1".to_string()));
    assert_eq!(snapshot.session_name, Some("Something Else".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_selection_is_fixed_while_a_session_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = definitions_api_with_settings(
        dir.path(),
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;

    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();
    api.tracking_start().await.unwrap();

    // Changing the selection mid-session conflicts; re-affirming the
    // standing value (none) is a tolerated no-op.
    assert!(matches!(
        api.tracking_definition_select(Some(1)).await.unwrap_err(),
        ApiError::Conflict { .. }
    ));
    api.tracking_definition_select(None).await.unwrap();

    api.tracking_stop().await.unwrap();
    api.tracking_definition_select(Some(1)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_definition_in_play_cannot_be_archived() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = definitions_api(dir.path()).await;

    api.session_definition_create(definition("ARIS Dailies", vec![]))
        .await
        .unwrap();
    // Session start persists this row before the tracker becomes active in
    // memory. Seed that exact durable authority here: this facade harness's
    // otherwise inert tracker provider deliberately does not read settings.
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active, definition_id) \
             VALUES ('in-play', 1000, 1, 2)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let error = api.session_definition_archive(2).await.unwrap_err();
    assert!(matches!(error, ApiError::Conflict { .. }), "{error:?}");
    assert!(api
        .session_definitions_list(None)
        .await
        .unwrap()
        .iter()
        .any(|definition| definition.id == "2"));

    db.with_writer(|conn| {
        conn.execute(
            "UPDATE tracking_sessions SET is_active = 0, ended_at = 1100 WHERE id = 'in-play'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(!api.session_definition_archive(2).await.unwrap().is_active);
}
