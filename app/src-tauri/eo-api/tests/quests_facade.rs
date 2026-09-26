//! Behavioural pins for the quest family over the typed
//! facade, ported from the family's HTTP-era hermetic router tests: the
//! reads over an empty database, the quest create / read-back / update
//! (present-null clears) ladder, the lifecycle (start / complete /
//! cancel), and the not-found legs. Plus a transport-invariance pin: the
//! created quest serialises to the exact bytes the HTTP route answered.
//!
//! The framework-validation legs (the create/update field-type 422s, the
//! string→type lax coercions, the surrogate-taint / beyond-`i64` deferred
//! 500s) do not port: they are unrepresentable over the typed DTOs and
//! retire as ratified contract movements.

use std::path::Path;
use std::sync::Arc;

use eo_api::quests::{QuestCooldownAnchor, QuestFamilyInput, QuestInput};
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot, matching the HTTP-era quests router assertions
/// (quest CRUD is catalogue-independent).
async fn quests_api(dir: &Path) -> Api {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    Api::new(
        db,
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
    )
}

/// A minimal quest payload (name only; every other field defaults).
fn minimal(name: &str) -> QuestInput {
    QuestInput {
        name: name.to_string(),
        planet: "Calypso".to_string(),
        category: None,
        waypoint: None,
        cooldown_hours: None,
        reward_ped: None,
        reward_is_skill: false,
        reward_description: None,
        completion_trigger: None,
        reward_policy: None,
        reward_item_names: Vec::new(),
        notes: None,
        chain_name: None,
        chain_position: None,
        chain_total: None,
        mobs: Vec::new(),
        signal_loot_item: None,
        family_id: None,
        cooldown_anchor: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reads_answer_the_empty_database() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    assert!(api.quests_list().await.unwrap().is_empty());
    assert!(api.quests_mobs().await.unwrap().is_empty());
    assert!(api.quests_analytics().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_minimal_create_reads_back_the_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    // Transport invariance: the created quest serialises to the exact
    // bytes the HTTP route answered (id "1" over the fresh database, the
    // planet/reward_is_skill defaults, null-or-empty text columns), plus
    // the ratified extensions: the `signalLootItem` key (null for
    // mission-log quests) added with signal-completed quests, and the
    // typed completion/reward defaults, plus the trailing anchor and
    // family availability keys (a standalone quest carries the
    // mission-log, no-reward, and completion defaults plus nulls).
    let created = api.quest_create(minimal("Alpha")).await.unwrap();
    assert_eq!(
        serde_json::to_string(&created).unwrap(),
        "{\"id\":\"1\",\"name\":\"Alpha\",\"category\":null,\"targetMobs\":[],\
         \"planet\":\"Calypso\",\"waypoint\":null,\"cooldownDurationHours\":null,\
         \"cooldownExpiresAt\":null,\"reward\":null,\"rewardIsSkill\":false,\
         \"rewardDescription\":\"\",\"notes\":\"\",\
         \"chainName\":null,\"chainPosition\":null,\"chainTotal\":null,\
         \"startedAt\":null,\"signalLootItem\":null,\
         \"completionTrigger\":\"mission_log\",\"rewardPolicy\":\"none\",\
         \"rewardItemNames\":[],\
         \"cooldownAnchor\":\"completion\",\"lastStartedAt\":null,\"familyId\":null,\
         \"familyName\":null,\"familyCooldownDurationHours\":null,\
         \"familyCooldownAnchor\":null,\"familyCooldownExpiresAt\":null,\
         \"rewardUndoAvailable\":false}"
    );

    // The read-back through the listing and the by-id read agree.
    assert_eq!(api.quests_list().await.unwrap().len(), 1);
    let one = api.quest_get(1).await.unwrap();
    assert_eq!(one.id, "1");
    assert_eq!(one.name, "Alpha");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_populated_create_carries_its_typed_fields() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    let mut input = minimal("Beta");
    input.reward_ped = Some(10.5);
    input.reward_is_skill = true;
    input.chain_position = Some(2);
    input.mobs = vec!["Atrox".to_string()];
    let created = api.quest_create(input).await.unwrap();

    assert_eq!(created.reward, Some(10.5));
    assert!(created.reward_is_skill);
    assert_eq!(created.chain_position, Some(2));
    assert_eq!(created.target_mobs, vec!["Atrox".to_string()]);

    // The distinct mob names surface for autocomplete.
    assert_eq!(api.quests_mobs().await.unwrap(), vec!["Atrox".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_applies_sent_fields_and_present_null_clears() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    let mut input = minimal("Alpha");
    input.reward_ped = Some(5.0);
    api.quest_create(input).await.unwrap();

    // The client sends the full field set; a present null clears the
    // column, and the name it re-sends is unchanged.
    let mut patch = minimal("Alpha");
    patch.notes = Some("updated".to_string());
    patch.reward_ped = None; // present-null clears the reward
    patch.reward_policy = Some(eo_api::quests::QuestRewardPolicy::None);
    let updated = api.quest_update(1, patch).await.unwrap();
    assert_eq!(updated.notes, "updated");
    assert_eq!(updated.reward, None);
    assert_eq!(updated.name, "Alpha");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lifecycle_starts_completes_and_cancels() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    let mut input = minimal("Cycle");
    input.cooldown_hours = Some(0.0);
    api.quest_create(input).await.unwrap();

    let started = api.quest_start(1).await.unwrap();
    assert!(started.started_at.is_some());

    let completed = api.quest_complete(1).await.unwrap();
    assert_eq!(completed.cooldown_expires_at, None); // zero-cooldown

    api.quest_start(1).await.unwrap();
    let cancelled = api.quest_cancel(1, false).await.unwrap();
    assert_eq!(cancelled.started_at, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_not_found_legs_answer_the_typed_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    assert_eq!(
        api.quest_get(424242).await.unwrap_err(),
        ApiError::not_found("Quest not found")
    );
    assert_eq!(
        api.quest_update(424242, minimal("Z")).await.unwrap_err(),
        ApiError::not_found("Quest not found")
    );
    assert_eq!(
        api.quest_delete(424242).await.unwrap_err(),
        ApiError::not_found("Quest not found")
    );
    assert_eq!(
        api.quest_start(424242).await.unwrap_err(),
        ApiError::not_found("Quest not found")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn populated_analytics_serialise_to_the_wire_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    // Keep a seeding handle before the database moves into the facade.
    let seed_db = db.clone();
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let api = Api::new(
        db,
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
    );

    // Quest 1: fully costed sessions with no invented authored reward.
    api.quest_create(minimal("Alpha")).await.unwrap();
    // Quest 2: no reward, a bare completed session; its aggregates are
    // the engine's INTEGER zeros, which the facade coerces to floats.
    api.quest_create(minimal("Nul")).await.unwrap();

    seed_db
        .with_writer(move |conn| {
            for (sid, start, end, heal, armour) in [
                ("sess-1", 1000.0, 1030.5, Some(1.5), Some(0.25)),
                ("sess-n", 7000.0, 7050.0, None, None),
                ("sess-p", 2000.0, 2100.0, Some(0.5), Some(0.0)),
            ] {
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
                     VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                    rusqlite::params![sid, start, end, heal, armour],
                )?;
            }
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
                 damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
                 VALUES ('k1', 'sess-1', 'Atrox', 1100.0, 40, 100.0, 5.0, 1, 10.0, 0.5, 12.75)",
                [],
            )?;
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
                 critical_hits, cost_per_shot) VALUES ('k1', 'LR-32', 40, 50.0, 0, 0.25)",
                [],
            )?;
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES ('sess-1', 1100.0, 'Rifle', 1.0, 0.75)",
                [],
            )?;
            for (sid, qid, at) in [
                ("sess-1", 1i64, 1500.0),
                ("sess-n", 2, 7040.0),
                ("sess-p", 1, 2050.0),
            ] {
                conn.execute(
                    "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![sid, qid, at],
                )?;
            }
            // Membership is the recorded quest stretch: one interval per
            // session naming the quest it ran.
            for (sid, qid, start, end) in [
                ("sess-1", 1i64, 1000.0, 1030.5),
                ("sess-n", 2, 7000.0, 7050.0),
                ("sess-p", 1, 2000.0, 2100.0),
            ] {
                conn.execute(
                    "INSERT INTO session_intervals \
                     (session_id, kind, label, ref_id, started_at, ended_at) \
                     VALUES (?1, 'quest', 'Quest', ?2, ?3, ?4)",
                    rusqlite::params![sid, qid, start, end],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    // Observed completions and the sessions' recorded quest stretches are the
    // only analytics input. There is no authored liquid-reward projection.
    let quest_rows = api.quests_analytics().await.unwrap();
    assert_eq!(quest_rows.len(), 2);
    let alpha = quest_rows
        .iter()
        .find(|row| row.quest_name == "Alpha")
        .unwrap();
    assert_eq!(alpha.recorded_completions, 2);
    assert_eq!(alpha.confirmed_completions, 0);
    assert_eq!(alpha.linked_sessions, 2);
    assert_eq!(alpha.total_duration_sec, 130.5);
    assert_eq!(alpha.total_weapon_cost, 10.0);
    assert_eq!(alpha.total_loot_tt, 12.75);
    assert_eq!(alpha.total_recorded_reward_tt, 0.0);
    assert_eq!(alpha.total_realised_reward_markup, 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_families_round_trip_over_the_typed_surface() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    assert!(api.quest_families_list().await.unwrap().is_empty());

    // Create with the pickup default; the wire shape is a declared DTO.
    let created = api
        .quest_family_create(QuestFamilyInput {
            name: "Daily Hunting 1".to_string(),
            planet: "ARIS".to_string(),
            cooldown_hours: Some(20.0),
            cooldown_anchor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&created).unwrap(),
        "{\"id\":\"1\",\"name\":\"Daily Hunting 1\",\"planet\":\"ARIS\",\
         \"cooldownDurationHours\":20.0,\"cooldownAnchor\":\"pickup\",\
         \"cooldownExpiresAt\":null,\"memberCount\":0,\"lastStartedAt\":null,\
         \"lastCompletedAt\":null}"
    );

    // The typed input always sends `family_id` explicitly (the form's
    // visible name-match suggestion fills the select), so a create
    // without one stays standalone even with a matching name; the
    // absent-key auto-attach is the service layer's, exercised by the
    // chat-log auto-create path.
    let standalone = api
        .quest_create(minimal("Daily Hunting 1: Standalone"))
        .await
        .unwrap();
    assert_eq!(standalone.family_id, None);

    // An explicit member carries the family picture, and starting it
    // opens the family window on the wire.
    let mut member_input = minimal("Daily Hunting 1: Weak Mortirex");
    member_input.family_id = Some(1);
    let member = api.quest_create(member_input).await.unwrap();
    assert_eq!(member.family_id, Some("1".to_string()));
    assert_eq!(member.family_name, Some("Daily Hunting 1".to_string()));
    assert_eq!(member.family_cooldown_duration_hours, Some(20.0));
    api.quest_start(2).await.unwrap();
    let started = api.quest_get(2).await.unwrap();
    assert!(started.last_started_at.is_some());
    assert!(started.family_cooldown_expires_at.is_some());
    let families = api.quest_families_list().await.unwrap();
    assert_eq!(families[0].member_count, 1);
    assert!(families[0].cooldown_expires_at.is_some());

    // A validation refusal surfaces as the typed bad request.
    let refused = api
        .quest_family_create(QuestFamilyInput {
            name: "  ".to_string(),
            planet: "Calypso".to_string(),
            cooldown_hours: None,
            cooldown_anchor: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(refused, ApiError::BadRequest { .. }));

    // Update binds; delete detaches the member and 404s thereafter.
    let updated = api
        .quest_family_update(
            1,
            QuestFamilyInput {
                name: "Daily Hunting 1".to_string(),
                planet: "ARIS".to_string(),
                cooldown_hours: None,
                cooldown_anchor: Some(QuestCooldownAnchor::Completion),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.cooldown_duration_hours, None);
    assert_eq!(updated.cooldown_anchor, QuestCooldownAnchor::Completion);
    api.quest_family_delete(1).await.unwrap();
    assert!(api.quest_families_list().await.unwrap().is_empty());
    let detached = api.quest_get(2).await.unwrap();
    assert_eq!(detached.family_id, None);
    assert_eq!(
        api.quest_family_delete(1).await.unwrap_err(),
        ApiError::not_found("Quest family not found")
    );
    assert_eq!(
        api.quest_family_update(
            424242,
            QuestFamilyInput {
                name: "Z".to_string(),
                planet: "Calypso".to_string(),
                cooldown_hours: None,
                cooldown_anchor: None,
            }
        )
        .await
        .unwrap_err(),
        ApiError::not_found("Quest family not found")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_soft_deletes_off_the_active_list() {
    let dir = tempfile::tempdir().unwrap();
    let api = quests_api(dir.path()).await;

    api.quest_create(minimal("Doomed")).await.unwrap();
    api.quest_delete(1).await.unwrap();
    // Delete is a soft delete: the quest drops off the active listing but
    // stays retrievable by id (the HTTP route's own contract; `get_quest`
    // carries no `is_active` filter).
    assert!(api.quests_list().await.unwrap().is_empty());
    assert_eq!(api.quest_get(1).await.unwrap().name, "Doomed");

    // A second delete finds no active row to soft-delete: the 404 leg.
    assert_eq!(
        api.quest_delete(1).await.unwrap_err(),
        ApiError::not_found("Quest not found")
    );
}
