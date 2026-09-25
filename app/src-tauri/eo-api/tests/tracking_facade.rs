//! Behavioural pins for the tracking family over the typed facade, ported
//! from the family's HTTP-era route behaviour: the session reads (list,
//! detail, tag suggestions), the idle dashboard snapshot, the post-hoc
//! session edits (rename mob, armour cost), the lifecycle guards (start /
//! stop), and the repair-scan gate. Each shape carries a transport-
//! invariance pin: the typed response serialises to the exact bytes the
//! HTTP route answered, save for the one ratified movement documented in
//! `eo_api::tracking` (the snapshot's exclude-unset -> exclude-none
//! narrowing: a present-null field is dropped rather than serialised null).

use std::path::Path;
use std::sync::Arc;

use eo_api::activities::ActivityTargetKind;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database. `settings` writes a
/// `settings.json` into the data dir first (for the config-flag reads);
/// `seed` places one ended session with two `Atrox` kills.
async fn make_api(dir: &Path, seed: bool, settings: Option<&str>) -> Api {
    make_api_db(dir, seed, settings).await.0
}

/// [`make_api`] plus a clone of the underlying database handle, for tests
/// that seed or verify rows directly.
async fn make_api_db(dir: &Path, seed: bool, settings: Option<&str>) -> (Api, Db) {
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
    if seed {
        seed_ended(&db).await;
    }
    let verify = db.clone();
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
        None,
    );
    (api, verify)
}

/// One ended session (`ended`) with two `Atrox` kills carrying loot.
async fn seed_ended(db: &Db) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) \
             VALUES('ended',1000.0,4600.0,0,0,0,0,'mob',4600.0)",
            [],
        )?;
        for (id, loot) in [("k1", 10.0), ("k2", 20.0)] {
            conn.execute(
                "INSERT INTO kills(id,session_id,mob_name,timestamp,loot_total_ped) VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params![id, "ended", "Atrox", 1001.0, loot],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_session_list_serialises_to_the_empty_page() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;
    let page = api.tracking_sessions(None, None, None).await.unwrap();
    assert_eq!(
        serde_json::to_string(&page).unwrap(),
        "{\"sessions\":[],\"nextCursor\":null,\"total\":0}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_session_cursor_is_a_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;
    let error = api
        .tracking_sessions(Some("not-a-cursor".to_string()), None, None)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&error).unwrap()["kind"], "badRequest");
}

/// Seed `count` ended one-kill sessions at distinct start times (newest
/// last inserted), for the pagination walks.
async fn seed_many_sessions(db: &Db, count: usize) {
    db.with_writer(move |conn| {
        for i in 0..count {
            let sid = format!("s{i:03}");
            let start = 1000.0 + (i as f64) * 10_000.0;
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,\
                 heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
                 VALUES(?1,?2,?3,0,0,0,0,'mob',?3)",
                rusqlite::params![sid, start, start + 600.0],
            )?;
            conn.execute(
                "INSERT INTO kills(id,session_id,mob_name,timestamp,loot_total_ped) \
                 VALUES(?1,?2,'Atrox',?3,5.0)",
                rusqlite::params![format!("{sid}-k"), sid, start + 1.0],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_harvest_session_nets_the_same_on_the_list_and_the_detail() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    // A harvest-only ended session: swings cost 26.43 in decay, loot
    // returned 16.45. No kills, so every non-harvest cost family is zero.
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,\
             heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
             VALUES('woods',1000.0,4600.0,0,0,0,0,'mob',4600.0)",
            [],
        )?;
        for (id, cost, loot) in [("h1", 13.21, 16.45), ("h2", 13.22, 0.0)] {
            conn.execute(
                "INSERT INTO harvest_events(id,session_id,timestamp,success,tool_name,cost_ped,\
                 loot_total_ped) VALUES(?1,'woods',1001.0,1,'Axe',?2,?3)",
                rusqlite::params![id, cost, loot],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();

    // The list row (served from the healed summary) and the detail read
    // must agree: net = harvest loot - harvest swing decay.
    let page = api.tracking_sessions(None, None, None).await.unwrap();
    assert_eq!(page.sessions.len(), 1);
    let row = &page.sessions[0];
    let detail = api
        .tracking_session_detail("woods".to_string())
        .await
        .unwrap();
    assert_eq!(row.cost, 26.43);
    assert_eq!(row.returns, 16.45);
    assert_eq!(row.net, -9.98);
    assert_eq!(row.net, detail.summary.net);
    assert_eq!(row.cost, detail.summary.cost);
    assert_eq!(row.returns, detail.summary.returns);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_keyset_walk_serves_every_session_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    seed_many_sessions(&db, 25).await;

    // First page: newest first, the whole-table count riding along, a
    // further page signalled by the cursor.
    let first = api.tracking_sessions(None, Some(10), None).await.unwrap();
    assert_eq!(first.sessions.len(), 10);
    assert_eq!(first.total, 25);
    assert_eq!(first.sessions[0].id, "s024");
    let cursor = first.next_cursor.0.clone().expect("a further page follows");

    // Walk the cursor to exhaustion: every session appears exactly once.
    let mut seen: Vec<String> = first.sessions.iter().map(|s| s.id.clone()).collect();
    let mut token = Some(cursor);
    while let Some(current) = token {
        let page = api
            .tracking_sessions(Some(current), Some(10), None)
            .await
            .unwrap();
        seen.extend(page.sessions.iter().map(|s| s.id.clone()));
        token = page.next_cursor.0.clone();
    }
    assert_eq!(seen.len(), 25, "every session is served");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 25, "no session is served twice");
    // The walk is newest-first end to end.
    assert_eq!(seen.first().map(String::as_str), Some("s024"));
    assert_eq!(seen.last().map(String::as_str), Some("s000"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_idle_snapshot_serialises_the_dashboard_way() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;
    // The resting idle dashboard over a default config: the exclude-none
    // projection drops the present-null mob / tool fields (the ratified
    // movement), while the trifecta summary (a fixed-shape nested object,
    // present because the default config ships a `default` preset) keeps
    // its own null bindings on the wire. The session facets are present
    // even with nothing configured: an unconfigured install resolves to
    // the protected default, so the readout shows what a start would
    // stamp, and the Activities block rides idle too: the seeded default
    // rosters nothing and defaults to whole-session armour costs, so it
    // reports no activity surface. The lifetime block rides
    // idle for the same reason, reading all-zero over a definition that
    // has never been run: a family with no history still HAS a family,
    // so the flip is offered and honestly reports an empty span.
    let snapshot = api.tracking_snapshot().await.unwrap();
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap(),
        "{\"status\":\"idle\",\"hotbarListenerActive\":false,\"weaponAttribution\":\"trifecta\",\
         \"repairOcrEnabled\":false,\
         \"sessionName\":\"Default Tracking\",\"sessionDefinitionId\":\"1\",\
         \"trackProtectionCosts\":true,\
         \"activities\":{\"visible\":false,\"adHocSegments\":false,\"readyCount\":0,\
         \"active\":[]},\
         \"lifetime\":{\"instanceCount\":0,\"cycled\":0.0,\"lootTt\":0.0,\"net\":0.0,\
         \"returnRate\":0.0,\"pes\":0.0,\"durationSeconds\":0.0},\
         \"trifectaAttribution\":{\"activePresetId\":\"default\",\
         \"presetName\":\"Default\",\"presets\":[{\"id\":\"default\",\"name\":\"Default\"}],\
         \"smallWeapon\":null,\"bigWeapon\":null,\"healTool\":null},\"recentEvents\":[]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_session_detail_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;
    let error = api
        .tracking_session_detail("nope".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&error).unwrap()["kind"], "notFound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seeded_session_detail_shapes_the_summary() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), true, None).await;
    let detail = api
        .tracking_session_detail("ended".to_string())
        .await
        .unwrap();
    // Two Atrox kills, 30 PED loot; the summary carries the float-typed
    // returns and the integer kill count.
    assert_eq!(detail.session_id, "ended");
    assert_eq!(detail.summary.kills, 2);
    assert_eq!(detail.summary.returns, 30.0);
    assert_eq!(detail.mob_breakdown.len(), 1);
    assert_eq!(detail.mob_breakdown[0].current_name, "Atrox");
    // The nullable per-mob original serialises present-null (a fixed-shape
    // response, unlike the polymorphic snapshot).
    let wire = serde_json::to_string(&detail).unwrap();
    assert!(
        wire.contains("\"currentName\":\"Atrox\",\"originalName\":null,\"killCount\":2"),
        "{wire}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_scan_is_bad_request_when_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;
    let error = api.tracking_repair_scan("ended".to_string()).unwrap_err();
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({"kind": "badRequest", "message": "Repair OCR is disabled"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_scan_soft_error_rides_the_body() {
    let dir = tempfile::tempdir().unwrap();
    // Enabled, but the inert providers find no game window: the service's
    // logical refusal rides the 200 body (declared fields first, then the
    // extra `error` key), byte-for-byte as the HTTP plain-200.
    let api = make_api(dir.path(), false, Some("{\"repair_ocr_enabled\": true}")).await;
    let result = api.tracking_repair_scan("ended".to_string()).unwrap();
    assert_eq!(
        serde_json::to_string(&result).unwrap(),
        "{\"cost_ped\":0.0,\"raw_text\":\"\",\"confidence\":0.0,\
         \"error\":\"Entropia Universe window not found: start the game first\"}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_mob_happy_path_and_validation_legs() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), true, None).await;

    // Happy path: both Atrox kills rename to Argo, byte-for-byte.
    let result = api
        .tracking_rename_mob("ended".to_string(), "Atrox".to_string(), "Argo".to_string())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&result).unwrap(),
        "{\"sessionId\":\"ended\",\"mobName\":\"Argo\",\"killCount\":2}"
    );

    // A blank name is a bad-request.
    let blank = api
        .tracking_rename_mob("ended".to_string(), "  ".to_string(), "X".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&blank).unwrap()["kind"], "badRequest");

    // An absent session is a not-found.
    let missing = api
        .tracking_rename_mob("nope".to_string(), "Argo".to_string(), "Zed".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn armour_cost_echoes_the_submitted_value() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), true, None).await;
    let result = api
        .tracking_armour_cost("ended".to_string(), 2.5)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&result).unwrap(),
        "{\"sessionId\":\"ended\",\"armourCost\":2.5}"
    );
    // An absent session is a not-found (no active-session guard on this leg).
    let missing = api
        .tracking_armour_cost("nope".to_string(), 1.0)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lifecycle_guards_hold() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;

    // Stop with no active session -> conflict.
    let stop = api.tracking_stop().await.unwrap_err();
    assert_eq!(serde_json::to_value(&stop).unwrap()["kind"], "conflict");

    // Start under the default config (trifecta mode, the default preset
    // present but its weapon / heal slots unbound) fails the attribution
    // gate with `validate_trifecta`'s verbatim reason.
    let start = api.tracking_start().await.unwrap_err();
    assert_eq!(
        serde_json::to_value(&start).unwrap(),
        serde_json::json!({
            "kind": "badRequest",
            "message": "Trifecta attribution requires a configured small weapon, big weapon, and healing tool"
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_session_cascades_and_guards_active_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");

    // One ended session (`ended`) whose kill `k1` fans out into every child
    // table, plus an active session (`live`) that must resist deletion.
    seed_ended(&db).await;
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO kill_tool_stats(kill_id,tool_name) VALUES('k1','Weapon')",
            [],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id,item_name,value_ped) VALUES('k1','Shrapnel',5.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO skill_gains(session_id,timestamp,skill_name,amount) \
             VALUES('ended',1001.0,'Rifle',1.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO notable_events(session_id,event_type,mob_or_item,value_ped,timestamp) \
             VALUES('ended','global','Atrox',60.0,1001.0)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let verify = db.clone();
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
        None,
    );

    // Insert the active session AFTER composition so the tracker's startup
    // orphan-recovery (which ends dangling active sessions) leaves it active.
    verify
        .with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions(id,started_at,is_active,mob_tracking_mode) \
                 VALUES('live',2000.0,1,'mob')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    // An active session is a conflict; an absent one is a not-found.
    let active = api
        .tracking_session_delete("live".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&active).unwrap()["kind"], "conflict");
    let missing = api
        .tracking_session_delete("nope".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");

    // Happy path: the ended session and every row that references it are gone,
    // and the active session is untouched.
    api.tracking_session_delete("ended".to_string())
        .await
        .unwrap();
    for (label, query) in [
        (
            "kills",
            "SELECT COUNT(*) FROM kills WHERE session_id='ended'",
        ),
        (
            "skill_gains",
            "SELECT COUNT(*) FROM skill_gains WHERE session_id='ended'",
        ),
        (
            "notable_events",
            "SELECT COUNT(*) FROM notable_events WHERE session_id='ended'",
        ),
        (
            "tracking_sessions",
            "SELECT COUNT(*) FROM tracking_sessions WHERE id='ended'",
        ),
        (
            "kill_tool_stats",
            "SELECT COUNT(*) FROM kill_tool_stats WHERE kill_id='k1'",
        ),
        (
            "kill_loot_items",
            "SELECT COUNT(*) FROM kill_loot_items WHERE kill_id='k1'",
        ),
    ] {
        let query = query.to_string();
        let count: i64 = verify
            .with_reader(move |conn| Ok(conn.query_row(&query, [], |row| row.get::<_, i64>(0))?))
            .await
            .unwrap();
        assert_eq!(count, 0, "{label} still holds rows for the deleted session");
    }
    let live: i64 = verify
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tracking_sessions WHERE id='live'",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(live, 1, "the active session must survive");

    // Deleting the now-gone session is a not-found.
    let again = api
        .tracking_session_delete("ended".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&again).unwrap()["kind"], "notFound");
}

/// The facet keys must actually reach the wire, with the values that
/// were declared.
///
/// Pinned because nothing else pins it: every other snapshot assertion
/// leaves both facets undeclared, so the exclude-none projection drops
/// the keys and a serialiser that emitted null unconditionally would
/// pass the whole suite. These are newly emitted fields; first
/// generation is exactly when an unpinned field is cheapest to get
/// wrong and hardest to notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_idle_snapshot_carries_the_declared_facets() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(dir.path(), false, None).await;

    api.tracking_session_config(Some("ARIS Dailies".into()), Some(50))
        .await
        .unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["sessionName"], "ARIS Dailies");
    assert_eq!(value["skillBoostPercent"], 50);

    // The declared zero survives the wire as 0, NOT as null and NOT
    // dropped: null is reserved for "nothing declared", and collapsing
    // the two would erase the unboosted baseline the whole
    // boost-measurement question rests on.
    api.tracking_session_config(Some("ARIS Dailies".into()), Some(0))
        .await
        .unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["skillBoostPercent"], 0);

    // Withdrawn: the boost key drops out of the projection entirely.
    // The name does not, because a withdrawn declaration falls through
    // to the protected default rather than to nothing.
    api.tracking_session_config(None, None).await.unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert!(value.get("skillBoostPercent").is_none());
    assert_eq!(value["sessionName"], "Default Tracking");
}

/// The ACTIVE branch's boost, which has a different source from the
/// idle branch's and so could regress without the idle pin noticing.
///
/// It is read from the running session's interval state, NOT from the
/// session row, because the row's column cannot hold a zero (migration
/// 0018 constrains it to `> 0 OR NULL`). A serialiser that went back to
/// the row would silently lose the declared baseline for every running
/// session; that is the regression this pins.
///
/// Only the boost is asserted here. The session name is seeded onto the
/// session from config at start, and this harness composes its tracker
/// with the inert config providers, so an active session in it never
/// carries one. The name's projection is the same `name_value` helper
/// the idle branch pins above; the boost's is not shared, which is why
/// it needs its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_active_snapshot_carries_the_declared_boost() {
    let dir = tempfile::tempdir().unwrap();
    // Hotbar attribution with one slot bound, so the start satisfies
    // that gate rather than the trifecta loadout one.
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    api.tracking_start().await.unwrap();

    api.tracking_session_config(None, Some(50)).await.unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["status"], "active");
    assert_eq!(value["skillBoostPercent"], 50);

    // Re-declared mid-session to the unboosted baseline: the running
    // session must report 0, not null and not the opening 50.
    api.tracking_session_config(None, Some(0)).await.unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["skillBoostPercent"], 0);

    // Withdrawn mid-session: the key leaves the projection.
    api.tracking_session_config(None, None).await.unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert!(value.get("skillBoostPercent").is_none());
}

/// The thin interval read over a live lifecycle: the boost declaration
/// and a segment each record an interval with real bounds, and the stop
/// seals whatever is still open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_intervals_read_demonstrates_the_live_contract() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    let started = api.tracking_start().await.unwrap();
    let session_id = started.session_id.clone();

    api.tracking_session_config(None, Some(50)).await.unwrap();
    api.tracking_activity_activate(
        ActivityTargetKind::Segment,
        None,
        Some("Rotation 1".to_string()),
        None,
    )
    .await
    .unwrap();

    let read = api
        .tracking_session_intervals(session_id.clone())
        .await
        .unwrap();
    assert_eq!(read.session_id, session_id);
    let value = serde_json::to_value(&read).unwrap();
    let rows = value["intervals"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["kind"], "modifier");
    assert_eq!(rows[0]["magnitude"], 50.0);
    assert_eq!(rows[0]["endedAt"], serde_json::Value::Null);
    assert_eq!(rows[1]["kind"], "segment");
    assert_eq!(rows[1]["label"], "Rotation 1");
    assert_eq!(rows[1]["endedAt"], serde_json::Value::Null);

    api.tracking_stop().await.unwrap();
    let read = api.tracking_session_intervals(session_id).await.unwrap();
    assert!(
        read.intervals
            .iter()
            .all(|row| serde_json::to_value(row.ended_at).unwrap() != serde_json::Value::Null),
        "the stop seals every open interval"
    );

    let missing = api
        .tracking_session_intervals("no-such-session".to_string())
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");
}

/// Attribution counts come from the stamped contexts, never from
/// comparing timestamps: an event whose context names two intervals
/// counts for both, and an event with no context (predating the
/// interval model) counts for none, whatever its timestamp says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_intervals_read_counts_by_context_membership() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;

    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('sess-i', 1000.0, 2000.0, 0)",
            [],
        )?;
        // Two intervals overlapping: a modifier and a segment.
        conn.execute(
            "INSERT INTO session_intervals (id, session_id, kind, label, magnitude, started_at, ended_at) \
             VALUES (1, 'sess-i', 'modifier', NULL, 0.0, 1000.0, 2000.0), \
                    (2, 'sess-i', 'segment', 'Boss 1', NULL, 1200.0, 1800.0)",
            [],
        )?;
        // Context 10 names only the modifier; context 11 names both.
        conn.execute(
            "INSERT INTO session_contexts (id, session_id, created_at) \
             VALUES (10, 'sess-i', 1000.0), (11, 'sess-i', 1200.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_context_intervals (context_id, interval_id) \
             VALUES (10, 1), (11, 1), (11, 2)",
            [],
        )?;
        // One kill inside the segment's context, one before it, and one
        // with no context at all (a pre-model row). The timestamps are
        // deliberately nonsense relative to the interval bounds: they
        // must not matter.
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
             damage_taken, critical_hits, cost_ped, loot_total_ped, context_id) \
             VALUES ('k1', 'sess-i', 'Atrox', 1.0, 5, 10.0, 0.0, 0, 0.5, 1.0, 11), \
                    ('k2', 'sess-i', 'Atrox', 9999.0, 5, 10.0, 0.0, 0, 0.5, 1.0, 10), \
                    ('k3', 'sess-i', 'Atrox', 1500.0, 5, 10.0, 0.0, 0, 0.5, 1.0, NULL)",
            [],
        )?;
        conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value, context_id) \
             VALUES ('sess-i', 1.0, 'Rifle', 1.0, 0.1, 11)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let read = api
        .tracking_session_intervals("sess-i".to_string())
        .await
        .unwrap();
    assert_eq!(read.intervals.len(), 2);
    let modifier = &read.intervals[0];
    assert_eq!(
        modifier.kills, 2,
        "both stamped kills sit inside the modifier"
    );
    assert_eq!(modifier.skill_gains, 1);
    let segment = &read.intervals[1];
    assert_eq!(segment.kills, 1, "only the k1 context names the segment");
    assert_eq!(segment.skill_gains, 1);
    assert_eq!(segment.harvests, 0);
}

/// A helper for the activity tests: an active quest in the catalogue,
/// optionally started (in progress). Built through the facade's own
/// commands so the pin covers the real path.
async fn seed_quest(api: &Api, name: &str, started: bool) -> i64 {
    seed_family_quest(api, name, started, None).await
}

/// [`seed_quest`] with an explicit family. The facade's create always
/// sends the `family_id` key, so the service's colon-split auto-attach
/// (the chat-log path's convenience) never fires through it: a variant
/// is bound to its family here the way the Quests page binds one.
async fn seed_family_quest(api: &Api, name: &str, started: bool, family_id: Option<i64>) -> i64 {
    let input = serde_json::from_value(serde_json::json!({ "name": name, "family_id": family_id }))
        .expect("quest input shape");
    let quest = api.quest_create(input).await.unwrap();
    let value = serde_json::to_value(&quest).unwrap();
    // The Quest DTO serialises its id as a string (the HTTP-era shape).
    let quest_id: i64 = value["id"]
        .as_str()
        .expect("quest id")
        .parse()
        .expect("numeric quest id");
    if started {
        api.quest_start(quest_id).await.unwrap();
    }
    quest_id
}

/// The names of a standing set, in declaration order.
fn activity_names(standing: &[eo_api::activities::ActiveActivityView]) -> Vec<&str> {
    standing
        .iter()
        .map(|activity| activity.name.as_str())
        .collect()
}

/// The Activities lifecycle at the facade: declaring a quest opens its
/// stretch (the snapshot's `activities` block is the readout), the
/// default re-declaration is the one-tap switch across BOTH kinds,
/// `additive` co-activates, and deactivating ends one stretch leaving
/// siblings. The ready cue rides the same block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activities_declare_switch_and_co_activate() {
    let dir = tempfile::tempdir().unwrap();
    let (api, selection) = make_api_with_selection(dir.path(), "Dailies").await;
    let carabok = seed_quest(&api, "Daily: Carabok", true).await;
    let monura = seed_quest(&api, "Daily: Monura", true).await;
    seed_definition(
        &api,
        &selection,
        serde_json::json!({
            "name": "Dailies",
            "ad_hoc_segments": true,
            "roster": [
                { "kind": "quest", "ref_id": carabok },
                { "kind": "quest", "ref_id": monura },
            ],
        }),
    )
    .await;
    api.tracking_start().await.unwrap();

    // Nothing standing: both rostered dailies are offered and ready.
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["activities"]["visible"], true);
    assert_eq!(value["activities"]["readyCount"], 2);
    assert_eq!(value["activities"]["active"], serde_json::json!([]));

    // Declaring opens the stretch; the ack echoes the set in force.
    let standing = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(carabok), None, None)
        .await
        .unwrap();
    assert_eq!(activity_names(&standing.active), vec!["Daily: Carabok"]);
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["activities"]["active"][0]["name"], "Daily: Carabok");
    assert_eq!(value["activities"]["active"][0]["kind"], "quest");
    assert_eq!(value["activities"]["readyCount"], 1);

    // Co-activation stacks; the default is the exclusive switch.
    let joined = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(monura), None, Some(true))
        .await
        .unwrap();
    assert_eq!(
        activity_names(&joined.active),
        vec!["Daily: Carabok", "Daily: Monura"]
    );
    let switched = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(carabok), None, None)
        .await
        .unwrap();
    assert_eq!(activity_names(&switched.active), vec!["Daily: Carabok"]);

    // A segment joins the standing quest only when asked to; a plain
    // declaration would have sealed it.
    let both = api
        .tracking_activity_activate(
            ActivityTargetKind::Segment,
            None,
            Some("Boss lap".to_string()),
            Some(true),
        )
        .await
        .unwrap();
    assert_eq!(
        activity_names(&both.active),
        vec!["Daily: Carabok", "Boss lap"]
    );

    // Deactivating ends one, leaving the other; then the block empties.
    let left = api
        .tracking_activity_deactivate(ActivityTargetKind::Quest, Some(carabok), None)
        .await
        .unwrap();
    assert_eq!(activity_names(&left.active), vec!["Boss lap"]);
    let cleared = api
        .tracking_activity_deactivate(
            ActivityTargetKind::Segment,
            None,
            Some("Boss lap".to_string()),
        )
        .await
        .unwrap();
    assert!(cleared.active.is_empty());
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["activities"]["active"], serde_json::json!([]));
}

/// A segment declaration carries the name the player gave it, trimmed,
/// and the acknowledgement echoes it so the control renders without a
/// re-read; the next declaration seals the standing one, because a
/// player-drawn slice is a sequential cut of the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_segment_declaration_is_named_trimmed_and_echoed() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    api.tracking_start().await.unwrap();

    let opened = api
        .tracking_activity_activate(
            ActivityTargetKind::Segment,
            None,
            Some("  Boss: Kreltin  ".to_string()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(activity_names(&opened.active), vec!["Boss: Kreltin"]);
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["activities"]["active"][0]["name"], "Boss: Kreltin");
    assert_eq!(value["activities"]["active"][0]["kind"], "segment");

    let next = api
        .tracking_activity_activate(
            ActivityTargetKind::Segment,
            None,
            Some("Boss: Feffoid".to_string()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        activity_names(&next.active),
        vec!["Boss: Feffoid"],
        "the standing slice was sealed by the next one"
    );
}

/// The Activities verbs' refusals: everything needs a running session,
/// each target needs the payload that identifies it, and ending a
/// stretch that is not standing is an idempotent no-op rather than a
/// failure, so a stale control cannot fail the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_activity_commands_refuse_an_idle_tracker_and_incomplete_targets() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;

    let idle = api
        .tracking_activity_activate(
            ActivityTargetKind::Segment,
            None,
            Some("Boss lap".to_string()),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&idle).unwrap()["kind"], "conflict");
    let idle = api
        .tracking_activity_deactivate(ActivityTargetKind::Segment, None, Some("Idle".to_string()))
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&idle).unwrap()["kind"], "conflict");

    // The payload each target needs is checked before any tracker call.
    let no_quest = api
        .tracking_activity_activate(ActivityTargetKind::Quest, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&no_quest).unwrap()["kind"],
        "badRequest"
    );
    let blank_label = api
        .tracking_activity_deactivate(ActivityTargetKind::Segment, None, Some("   ".to_string()))
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&blank_label).unwrap()["kind"],
        "badRequest"
    );

    api.tracking_start().await.unwrap();
    let noop = api
        .tracking_activity_deactivate(
            ActivityTargetKind::Segment,
            None,
            Some("Never opened".to_string()),
        )
        .await
        .unwrap();
    assert!(noop.active.is_empty());
}
/// The quest-declaration refusals: an unknown quest is a not-found, a
/// mission-log quest the log does not carry is a request error (play
/// cannot be toward a mission you have not been given), and an idle
/// tracker is a conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declaring_a_quest_refuses_unknown_unstarted_and_idle() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;

    let missing = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(9_999), None, None)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");

    let unstarted = seed_quest(&api, "Daily: Unpicked", false).await;
    let refused = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(unstarted), None, None)
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&refused).unwrap()["kind"],
        "badRequest"
    );

    let started = seed_quest(&api, "Daily: Idle", true).await;
    let idle = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(started), None, None)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&idle).unwrap()["kind"], "conflict");

    let cold_signal = api
        .quest_create(
            serde_json::from_value(serde_json::json!({
                "name": "Cold signal",
                "completion_trigger": "signal_item",
                "signal_loot_item": "Daily Voucher",
                "cooldown_anchor": "pickup",
                "cooldown_hours": 20,
            }))
            .expect("quest input shape"),
        )
        .await
        .unwrap();
    let cold_signal_id: i64 = cold_signal.id.parse().unwrap();
    let idle = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(cold_signal_id), None, None)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&idle).unwrap()["kind"], "conflict");
    let cold_signal = serde_json::to_value(api.quest_get(cold_signal_id).await.unwrap()).unwrap();
    assert!(cold_signal["startedAt"].is_null());
    assert!(
        cold_signal["lastStartedAt"].is_null(),
        "idle declaration must not stamp a pickup cooldown"
    );
    let idle = api
        .tracking_activity_deactivate(ActivityTargetKind::Quest, Some(started), None)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&idle).unwrap()["kind"], "conflict");

    // With a session running, ending a quest with no open stretch is an
    // idempotent no-op: a stale control cannot fail the user.
    api.tracking_start().await.unwrap();
    let noop = api
        .tracking_activity_deactivate(ActivityTargetKind::Quest, Some(started), None)
        .await
        .unwrap();
    assert!(noop.active.is_empty());
}

/// A scripted tracker config carrying the session facets a start
/// snapshots: what the roster tests need, since the shared harness
/// composes the inert config and its sessions are instances of nothing.
/// The selected definition is settable after construction, because a
/// definition has to be authored (and given an id) before a session can
/// be an instance of it.
struct ScriptedSessionConfig {
    name: &'static str,
    definition_id: Arc<std::sync::atomic::AtomicI64>,
}

impl ScriptedSessionConfig {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            definition_id: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }
    }
}

impl eo_services::tracker::TrackingConfig for ScriptedSessionConfig {
    fn session_name(&self) -> String {
        self.name.to_string()
    }
    fn session_definition_id(&self) -> Option<i64> {
        match self.definition_id.load(std::sync::atomic::Ordering::SeqCst) {
            0 => None,
            id => Some(id),
        }
    }
    fn declared_skill_boost_percent(&self) -> Option<i64> {
        None
    }
    fn manual_mob(&self) -> Option<(String, String)> {
        None
    }
    fn weapon_attribution_trifecta(&self) -> bool {
        false
    }
    fn loot_filter_blacklist(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The facade over a tracker whose sessions snapshot a scripted
/// definition selection; the handle sets which definition once it has
/// been authored.
async fn make_api_with_selection(
    dir: &Path,
    name: &'static str,
) -> (Api, Arc<std::sync::atomic::AtomicI64>) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(
        data_dir.join("settings.json"),
        "{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}",
    )
    .unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let config = ScriptedSessionConfig::new(name);
    let selection = config.definition_id.clone();
    let handles = common::producer_handles_with_tracker(
        &db,
        &data_dir,
        tokio::runtime::Handle::current(),
        eo_services::tracker::Providers {
            config: Arc::new(config),
            ..Default::default()
        },
    )
    .await;
    let api = Api::new(
        db,
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
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
        None,
    );
    (api, selection)
}

/// Author a definition and make it the scripted selection.
async fn seed_definition(
    api: &Api,
    selection: &Arc<std::sync::atomic::AtomicI64>,
    input: serde_json::Value,
) -> String {
    let definition = api
        .session_definition_create(serde_json::from_value(input).expect("definition input shape"))
        .await
        .unwrap();
    let id: i64 = definition.id.parse().expect("numeric definition id");
    selection.store(id, std::sync::atomic::Ordering::SeqCst);
    // The scripted provider is what a session start snapshots; the
    // stored selection is what the idle read resolves through. The app
    // moves both with one verb, so the harness does too.
    api.tracking_definition_select(Some(id)).await.unwrap();
    definition.id
}

/// A definition's stored roster, read back through the facade.
async fn stored_roster(
    api: &Api,
    definition_id: &str,
) -> Vec<eo_api::session_definitions::SessionRosterEntry> {
    api.session_definitions_list(None)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == definition_id)
        .expect("the definition")
        .roster
}

/// Author a quest family (pickup-anchored, the daily's own shape) and
/// answer its numeric id.
async fn seed_family(api: &Api, name: &str, cooldown_hours: f64) -> i64 {
    seed_family_anchored(api, name, cooldown_hours, "pickup").await
}

/// [`seed_family`] with the cooldown anchor named, for the tests that
/// turn on which instant the gate runs from.
async fn seed_family_anchored(api: &Api, name: &str, cooldown_hours: f64, anchor: &str) -> i64 {
    let family = api
        .quest_family_create(
            serde_json::from_value(serde_json::json!({
                "name": name,
                "cooldown_hours": cooldown_hours,
                "cooldown_anchor": anchor,
            }))
            .expect("family input shape"),
        )
        .await
        .unwrap();
    serde_json::to_value(&family).unwrap()["id"]
        .as_str()
        .expect("family id")
        .parse()
        .expect("numeric family id")
}

/// The whole roster-fed read, on a session that is an instance of an
/// authored definition.
///
/// A family entry resolves to the variant in play and acts on that
/// quest, and a segment entry is always declarable. A quest the mission
/// log happens to carry is NOT offered unless the session rostered it:
/// the roster is the whole offering, and an arbitrary assortment of open
/// quests is not this session's business. The ready cue counts the
/// family once. A name typed in play is promoted into the roster, so it
/// is a one-tap chip next time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_roster_feeds_the_control_and_a_named_segment_is_promoted() {
    let dir = tempfile::tempdir().unwrap();
    let (api, selection) = make_api_with_selection(dir.path(), "ARIS Dailies").await;

    // A family with two rotating variants: one received today, one not.
    let family_id = seed_family(&api, "ARIS - Daily Hunting 1", 20.0).await;
    let today = seed_family_quest(
        &api,
        "ARIS - Daily Hunting 1: Weak Mortirex",
        true,
        Some(family_id),
    )
    .await;
    seed_family_quest(
        &api,
        "ARIS - Daily Hunting 1: Weak Atrox",
        false,
        Some(family_id),
    )
    .await;
    // And a daily the mission log carries that nobody rostered.
    seed_quest(&api, "ARIS - Daily Samples", true).await;

    // Authored family-then-segment; alphabetically the segment leads, so
    // the assertion below can tell the two orders apart.
    let definition_id = seed_definition(
        &api,
        &selection,
        serde_json::json!({
            "name": "ARIS Dailies",
            "ad_hoc_segments": true,
            "roster": [
                { "kind": "quest_family", "ref_id": family_id },
                { "kind": "segment", "label": "A warm-up lap" },
            ],
        }),
    )
    .await;
    api.tracking_start().await.unwrap();

    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.visible);
    assert!(options.ad_hoc_segments);
    let rows: Vec<(&str, bool, bool)> = options
        .options
        .iter()
        .map(|option| (option.name.as_str(), option.available, option.off_roster))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("A warm-up lap", true, false),
            // The family row names the variant in play, because that is
            // what a tap records.
            ("ARIS - Daily Hunting 1: Weak Mortirex", true, false),
        ],
        "what the session offers, alphabetically, and nothing else"
    );
    assert_eq!(
        options.options[1].quest_id,
        Some(today),
        "the family acts on its serving variant"
    );
    assert_eq!(
        options.ready_count, 2,
        "the family counts once, not once per variant"
    );

    // A name typed in play is declared AND promoted, so it is a chip
    // next time.
    api.tracking_activity_activate(
        ActivityTargetKind::Segment,
        None,
        Some("Boss lap".to_string()),
        None,
    )
    .await
    .unwrap();

    let entries = stored_roster(&api, &definition_id).await;
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.display_name.as_deref())
        .collect();
    assert_eq!(
        names,
        vec!["ARIS - Daily Hunting 1", "A warm-up lap", "Boss lap"],
        "the typed name was appended to the stored roster"
    );

    // Declaring it again does not duplicate the chip.
    api.tracking_activity_activate(
        ActivityTargetKind::Segment,
        None,
        Some("  boss lap  ".to_string()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        stored_roster(&api, &definition_id).await.len(),
        3,
        "promotion dedupes case-insensitively"
    );
}

/// A family with nothing in play says so rather than offering a tap that
/// would do nothing, and names the cooldown when one is what is holding
/// it back: the picker must tell the truth about availability, which is
/// the whole reason the family carries the timer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_family_with_no_variant_in_play_is_offered_but_not_available() {
    let dir = tempfile::tempdir().unwrap();
    let (api, selection) = make_api_with_selection(dir.path(), "ARIS Dailies").await;

    // Completion-anchored on purpose: under the pickup anchor the family
    // would already be cooling from the variant's own start, and the
    // completion below would prove nothing.
    let family_id = seed_family_anchored(&api, "ARIS - Daily Hunting 2", 20.0, "completion").await;
    seed_definition(
        &api,
        &selection,
        serde_json::json!({
            "name": "ARIS Dailies",
            "roster": [{ "kind": "quest_family", "ref_id": family_id }],
        }),
    )
    .await;
    api.tracking_start().await.unwrap();

    // No member ever received: nothing to serve, and no timer running.
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.visible, "an authored roster is reason enough");
    assert_eq!(options.options.len(), 1);
    assert_eq!(options.options[0].name, "ARIS - Daily Hunting 2");
    assert!(!options.options[0].available);
    assert_eq!(
        options.options[0].unavailable_reason.as_deref(),
        Some("No variant received yet")
    );
    assert_eq!(options.ready_count, 0, "the ready cue cannot overpromise");

    // A variant received then completed puts the FAMILY on cooldown, so
    // the row's reason changes and its gate rides with it.
    let variant = seed_family_quest(
        &api,
        "ARIS - Daily Hunting 2: Weak Berycled",
        true,
        Some(family_id),
    )
    .await;
    api.quest_complete(variant).await.unwrap();
    let options = api.tracking_activity_options().await.unwrap();
    assert_eq!(options.options.len(), 1, "the completed variant is no fact");
    assert!(!options.options[0].available);
    assert_eq!(
        options.options[0].unavailable_reason.as_deref(),
        Some("On cooldown")
    );
    assert!(
        options.options[0].available_from.is_some(),
        "the gate's lift instant rides with the row so it can count down"
    );
}

/// A session that offers nothing gets no surface at all, whatever the
/// mission log happens to carry. Declaring no activities and leaving
/// self-named segments off IS the choice of a simple session (the
/// seeded default is exactly that), so honouring it is the point: a new
/// player meets no options they have no use for yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_offers_nothing_gets_no_surface() {
    let dir = tempfile::tempdir().unwrap();
    let api = make_api(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;
    api.tracking_start().await.unwrap();

    let options = api.tracking_activity_options().await.unwrap();
    assert!(!options.visible);
    assert!(!options.ad_hoc_segments);
    assert!(options.options.is_empty());

    // Three received missions later: still nothing, because none of
    // them is what this session said it was for.
    for name in ["ARIS - Daily Samples", "Pluck the Wing", "Poison the Hive"] {
        seed_quest(&api, name, true).await;
    }
    let options = api.tracking_activity_options().await.unwrap();
    assert!(!options.visible, "an open mission log is not an offering");
    assert!(options.options.is_empty());
    assert_eq!(options.ready_count, 0);
}

/// A signal quest rostered on the session is a standing, repeatable row
/// whose in-progress state survives session boundaries: it is offered
/// before any start, declaring it from cold starts it in the same motion
/// (there is no mission log to mirror), and stopping the tracking
/// session ends the stretch but NOT the run, so the next session offers
/// it again and can re-declare without a restart. This is the
/// collect-now-finish-later flow pinned end to end, on the roster entry
/// that names a single quest rather than a family.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rostered_signal_quest_declares_from_cold_and_survives_session_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let (api, selection) = make_api_with_selection(dir.path(), "Boss runs").await;

    let input = serde_json::from_value(serde_json::json!({
        "name": "Hyperion Boss 1",
        "signal_loot_item": "Hyperion Daily Voucher",
    }))
    .expect("quest input shape");
    let boss = api.quest_create(input).await.unwrap();
    let boss_id: i64 = serde_json::to_value(&boss).unwrap()["id"]
        .as_str()
        .expect("quest id")
        .parse()
        .expect("numeric quest id");
    seed_definition(
        &api,
        &selection,
        serde_json::json!({
            "name": "Boss runs",
            "roster": [{ "kind": "quest", "ref_id": boss_id }],
        }),
    )
    .await;

    // Idle: the session is picked but not running, and the control
    // already shows what it will offer. Nothing is standing, because
    // nothing can be until a session does.
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.visible, "picking a session shows what it offers");
    assert_eq!(options.options.len(), 1);
    assert!(options.active.is_empty());

    // Running: the roster offers it even though nothing has started it,
    // which is what makes a repeatable run reachable at all.
    api.tracking_start().await.unwrap();
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.visible);
    assert_eq!(options.options.len(), 1);
    assert!(options.options[0].available);
    assert!(!options.options[0].active);
    assert_eq!(options.ready_count, 1);

    // Declaring from cold starts the run and opens the stretch.
    let standing = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(boss_id), None, None)
        .await
        .unwrap();
    assert_eq!(activity_names(&standing.active), vec!["Hyperion Boss 1"]);
    let quest = api.quest_get(boss_id).await.unwrap();
    assert!(
        !serde_json::to_value(&quest).unwrap()["startedAt"].is_null(),
        "declaring a cold signal quest started it"
    );
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.options[0].active);
    assert_eq!(options.ready_count, 0, "what is standing is not also ready");

    // Session stop ends the stretch, not the run.
    api.tracking_stop().await.unwrap();
    let quest = api.quest_get(boss_id).await.unwrap();
    assert!(
        !serde_json::to_value(&quest).unwrap()["startedAt"].is_null(),
        "the run itself is still going"
    );

    // The next session offers it again, unstanding, and re-declares it.
    api.tracking_start().await.unwrap();
    let options = api.tracking_activity_options().await.unwrap();
    assert!(
        !options.options[0].active,
        "the stretch died with its session"
    );
    assert_eq!(options.ready_count, 1);
    let standing = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(boss_id), None, None)
        .await
        .unwrap();
    assert_eq!(activity_names(&standing.active), vec!["Hyperion Boss 1"]);
}

/// A manual-hand-in quest uses the same one-tap declaration semantics as a
/// signal quest, then exposes its persisted waiting state through both the
/// contextual command and the existing Activities picture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rostered_manual_quest_starts_on_selection_and_surfaces_hand_in_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (api, selection) = make_api_with_selection(dir.path(), "AI dailies").await;
    let input = serde_json::from_value(serde_json::json!({
        "name": "AI Daily terminal",
        "completion_trigger": "manual_hand_in",
    }))
    .expect("quest input shape");
    let quest = api.quest_create(input).await.unwrap();
    let quest_id: i64 = serde_json::to_value(&quest).unwrap()["id"]
        .as_str()
        .expect("quest id")
        .parse()
        .expect("numeric quest id");
    seed_definition(
        &api,
        &selection,
        serde_json::json!({
            "name": "AI dailies",
            "roster": [{ "kind": "quest", "ref_id": quest_id }],
        }),
    )
    .await;

    api.tracking_start().await.unwrap();
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.options[0].manual_hand_in);
    assert!(options.options[0].available);
    assert!(!options.options[0].hand_in_waiting);

    let standing = api
        .tracking_activity_activate(ActivityTargetKind::Quest, Some(quest_id), None, None)
        .await
        .unwrap();
    assert!(standing.active[0].manual_hand_in);
    assert!(!standing.active[0].hand_in_waiting);
    let quest = api.quest_get(quest_id).await.unwrap();
    assert!(!serde_json::to_value(&quest).unwrap()["startedAt"].is_null());

    let waiting = api.quest_hand_in_begin(quest_id).await.unwrap();
    assert!(waiting.waiting);
    assert!(waiting.candidate.is_none());
    let options = api.tracking_activity_options().await.unwrap();
    assert!(options.active[0].hand_in_waiting);
    assert!(options.options[0].hand_in_waiting);

    api.quest_hand_in_cancel(quest_id).await.unwrap();
    let options = api.tracking_activity_options().await.unwrap();
    assert!(!options.active[0].hand_in_waiting);
    assert!(!options.options[0].hand_in_waiting);
}

/// The lifetime block: derived from the summed parts, folding the
/// in-flight instance in, and absent entirely when no definition is in
/// force.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lifetime_block_sums_the_family_and_folds_the_live_instance() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(
        dir.path(),
        false,
        Some("{\"hotbar_hooks_enabled\": true, \"hotbar\": {\"1\": 1}}"),
    )
    .await;

    // Two ended instances of the seeded protected default (id 1), of
    // very different sizes: 100 cycled returning 60, and 2 cycled
    // returning 10.
    for (id, started, ended, cost, loot) in [
        ("past-1", 1000.0, 11_800.0, 100.0, 60.0),
        ("past-2", 20_000.0, 20_240.0, 2.0, 10.0),
    ] {
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost, definition_id) \
                 VALUES (?1, ?2, ?3, 0, ?4, 1)",
                rusqlite::params![id, started, ended, cost],
            )?;
            conn.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, loot_total_ped) \
                 VALUES (?1, ?2, 'Atrox', ?3, ?4)",
                rusqlite::params![format!("{id}-k"), id, started + 1.0, loot],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    let lifetime = &value["lifetime"];
    assert_eq!(lifetime["instanceCount"], 2);
    assert_eq!(lifetime["cycled"], 102.0);
    assert_eq!(lifetime["lootTt"], 70.0);
    assert_eq!(lifetime["net"], -32.0);
    // 70/102, the ratio of the sums. The mean of the two instance rates
    // (0.6 and 5.0) would read 2.8: an accounting lie built out of four
    // minutes of luck.
    assert_eq!(lifetime["returnRate"], 0.6863);

    // Starting a session folds the in-flight instance into the family:
    // a lifetime that excluded the session being watched would be a
    // trap. It has cycled nothing yet, so only the span moves.
    api.tracking_start().await.unwrap();
    let value = serde_json::to_value(api.tracking_snapshot().await.unwrap()).unwrap();
    assert_eq!(value["status"], "active");
    assert_eq!(value["lifetime"]["instanceCount"], 3);
    assert_eq!(value["lifetime"]["cycled"], 102.0);
}

// ── The review surface: definition-scoped reads and re-filing ───────

/// Seed `count` ended sessions under `definition_id` (or unattached when
/// `None`), ids prefixed so each caller's rows are distinguishable.
async fn seed_instances(db: &Db, prefix: &str, definition_id: Option<i64>, count: usize) {
    for index in 0..count {
        let id = format!("{prefix}-{index}");
        let started = 1000.0 + (index as f64) * 100.0;
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost, definition_id) \
                 VALUES (?1, ?2, ?3, 0, 1.0, ?4)",
                rusqlite::params![id, started, started + 60.0, definition_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }
}

/// The scope narrows the rows AND the count: a pager over one
/// definition must report its bounds, not the whole table's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_list_scopes_to_one_definitions_instances() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let mine = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Carabok Skilling".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let mine_id: i64 = mine.id.parse().unwrap();

    seed_instances(&db, "mine", Some(mine_id), 3).await;
    seed_instances(&db, "default", Some(1), 2).await;
    seed_instances(&db, "loose", None, 4).await;

    let scoped = api
        .tracking_sessions(None, None, Some(mine_id))
        .await
        .unwrap();
    assert_eq!(scoped.total, 3);
    assert_eq!(scoped.sessions.len(), 3);
    assert!(scoped.sessions.iter().all(|s| s.id.starts_with("mine-")));

    // Unscoped stays the whole table, unchanged by the new argument.
    let all = api.tracking_sessions(None, None, None).await.unwrap();
    assert_eq!(all.total, 9);
}

/// The scope composes with the keyset cursor rather than being dropped
/// by it: paging one definition must not widen back to every session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_page_keeps_its_scope_across_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    seed_instances(&db, "mine", Some(1), 3).await;
    seed_instances(&db, "loose", None, 5).await;

    let first = api.tracking_sessions(None, Some(2), Some(1)).await.unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.sessions.len(), 2);
    let cursor = first.next_cursor.0.clone().expect("a next page");

    let second = api
        .tracking_sessions(Some(cursor), Some(2), Some(1))
        .await
        .unwrap();
    assert_eq!(second.sessions.len(), 1);
    assert!(second.sessions.iter().all(|s| s.id.starts_with("mine-")));
    assert!(second.next_cursor.is_none());
}

/// Re-filing moves the reference and carries the stamped name with it:
/// the stamp is a copy of the definition's name, so it always follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_filing_moves_the_instance_and_restamps_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let target = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Carabok Skilling".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let target_id: i64 = target.id.parse().unwrap();

    // An instance of the protected default, carrying that definition's
    // own name exactly as selection would have stamped it.
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, definition_id, session_name) \
             VALUES ('misfiled', 1000.0, 2000.0, 0, 0, 1, 'Default Tracking')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let result = api
        .tracking_reassign_session("misfiled".to_string(), target_id)
        .await
        .unwrap();
    assert_eq!(result.definition_id, target.id);
    assert_eq!(result.session_name, Some("Carabok Skilling".to_string()));

    // The instance now reads under its new definition, and only there.
    let scoped = api
        .tracking_sessions(None, None, Some(target_id))
        .await
        .unwrap();
    assert_eq!(scoped.total, 1);
    assert_eq!(scoped.sessions[0].id, "misfiled");
    assert_eq!(
        api.tracking_sessions(None, None, Some(1))
            .await
            .unwrap()
            .total,
        0
    );
}

/// A free-text name from before definitions existed is replaced, not
/// preserved: retiring the old naming scheme is the point of the move,
/// and keeping the legacy string would carry the thing being retired
/// into the definition that replaced it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_filing_replaces_a_legacy_free_text_name() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let target = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Carabok Skilling".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let target_id: i64 = target.id.parse().unwrap();

    // A pre-definitions session: no reference, a hand-typed name.
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, definition_id, session_name) \
             VALUES ('legacy', 1000.0, 2000.0, 0, 0, NULL, 'carabok skilling run 3')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let result = api
        .tracking_reassign_session("legacy".to_string(), target_id)
        .await
        .unwrap();
    assert_eq!(result.definition_id, target.id);
    assert_eq!(result.session_name, Some("Carabok Skilling".to_string()));
}

/// Re-filing is a post-hoc correction. A running session keeps the
/// definition it started under until the tracker has sealed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_filing_refuses_an_active_session() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let target = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Carabok Skilling".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let target_id: i64 = target.id.parse().unwrap();

    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, is_active, armour_cost, definition_id, session_name) \
             VALUES ('active', 1000.0, 1, 0, 1, 'Default Tracking')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let error = api
        .tracking_reassign_session("active".to_string(), target_id)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::Conflict { .. }), "{error:?}");

    let unchanged: (i64, String) = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT definition_id, session_name FROM tracking_sessions WHERE id = 'active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(unchanged, (1, "Default Tracking".to_string()));
}

/// An archived definition takes no new instances: filing into a
/// definition nothing offers any more is the one arrangement the review
/// surface could not show honestly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_filing_refuses_an_archived_definition() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let archived = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Retired".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let archived_id: i64 = archived.id.parse().unwrap();
    seed_instances(&db, "instance", Some(1), 1).await;
    api.session_definition_archive(archived_id).await.unwrap();

    let error = api
        .tracking_reassign_session("instance-0".to_string(), archived_id)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::NotFound { .. }), "{error:?}");

    // An unknown definition is the same refusal, and an unknown session
    // never reaches the definition guard at all.
    let error = api
        .tracking_reassign_session("instance-0".to_string(), 9999)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::NotFound { .. }), "{error:?}");
    let error = api
        .tracking_reassign_session("no-such-session".to_string(), 1)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::NotFound { .. }), "{error:?}");
}

/// The instances of an archived definition stay reachable: the
/// listing that asks for the inactive ones is how the review surface
/// finds recorded play whose definition has been archived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archived_definition_keeps_its_instances_reachable() {
    let dir = tempfile::tempdir().unwrap();
    let (api, db) = make_api_db(dir.path(), false, None).await;
    let archived = api
        .session_definition_create(eo_api::session_definitions::SessionDefinitionInput {
            name: "Retired".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: true,
            roster: Vec::new(),
        })
        .await
        .unwrap();
    let archived_id: i64 = archived.id.parse().unwrap();
    seed_instances(&db, "kept", Some(archived_id), 2).await;
    api.session_definition_archive(archived_id).await.unwrap();

    // Absent from the offered list, present in the full one.
    let offered = api.session_definitions_list(None).await.unwrap();
    assert!(offered.iter().all(|d| d.id != archived.id));
    assert!(offered.iter().all(|d| d.is_active));

    let all = api.session_definitions_list(Some(true)).await.unwrap();
    let found = all
        .iter()
        .find(|d| d.id == archived.id)
        .expect("the archived definition");
    assert!(!found.is_active);
    assert_eq!(found.instance_count, 2);

    // And its recorded instances still read.
    let scoped = api
        .tracking_sessions(None, None, Some(archived_id))
        .await
        .unwrap();
    assert_eq!(scoped.total, 2);
}
