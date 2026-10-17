// Expected values in these tests are the original implementation's
// outputs, computed by running the original Python implementation
// over byte-identical payloads and database seeds (created_at and
// updated_at pinned by direct UPDATE on both sides, since the schema
// stamps them from the wall clock).

use std::sync::Arc;

use rusqlite::params;
use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::bus_events::{
    BusEvent, MissionReceivedPayload, MissionReceivedTag, SessionLifecyclePayload,
};
use crate::chatlog_watcher::{MissionCompletion, RawLootClump, SignalLoot};
use crate::db::Db;

use super::payload::json_truthy;
use super::{QuestError, QuestService};
use crate::ped::Ped;

type ServiceRig = (
    Arc<QuestService>,
    Db,
    Arc<crate::clock::MockClock>,
    Arc<crate::event_bus::EventBus>,
);

async fn service_with_clock(dir: &std::path::Path) -> ServiceRig {
    let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        Some(
            chrono::NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        ),
        0.0,
    ));
    let bus = Arc::new(crate::event_bus::EventBus::new());
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let svc = QuestService::start_with_id_source(
        &bus,
        db.clone(),
        clock.clone(),
        Handle::current(),
        Arc::new(move || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            format!("fixed-{n:04}")
        }),
    );
    (svc, db, clock, bus)
}

async fn service(dir: &std::path::Path) -> (Arc<QuestService>, Db) {
    let (svc, db, _clock, _bus) = service_with_clock(dir).await;
    (svc, db)
}

async fn pin_ts(db: &Db, table: &str, id: i64, ts: f64) {
    let table = table.to_string();
    db.with_writer(move |conn| {
        conn.execute(
            &format!("UPDATE {table} SET created_at = ?1, updated_at = ?2 WHERE id = ?3"),
            params![ts, ts, id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// A `SELECT COUNT(*) FROM ...` with no bound parameters, the shape
/// repeated throughout these tests to check a table's row count.
async fn count_rows(db: &Db, sql: &'static str) -> i64 {
    db.with_reader(move |conn| {
        conn.query_row(sql, [], |row| row.get(0))
            .map_err(Into::into)
    })
    .await
    .unwrap()
}

fn quest_id(value: &Value) -> i64 {
    value["id"].as_i64().unwrap()
}

fn full_quest_payload() -> Value {
    json!({
        "name": "Atrox Cull", "planet": "Foma", "waypoint": "/wp 1,2",
        "cooldown_hours": 24, "notes": "bring fap",
        "chain_name": "Cull", "chain_position": 1, "chain_total": 3,
        "category": "hunt", "reward_description": "ammo",
        "mobs": [" Atrox ", "", "Atrax", "Atrox"],
    })
}

#[tokio::test]
async fn creates_apply_defaults_normalisation_and_mob_rules() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let q1 = quest_id(
        &svc.create_quest(&json!({"name": "Iron Challenge"}))
            .await
            .unwrap(),
    );
    pin_ts(&db, "quests", q1, 1000.0).await;
    let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());
    pin_ts(&db, "quests", q2, 1001.0).await;
    let q3 = quest_id(
        &svc.create_quest(&json!({
            "name": "Skill Run", "reward_ped": 5.0, "reward_is_skill": true,
            "expected_reward_markup_percent": 120.0,
        }))
        .await
        .unwrap(),
    );
    pin_ts(&db, "quests", q3, 1002.0).await;
    assert_eq!((q1, q2, q3), (1, 2, 3));

    // The minimal quest: planet defaults, everything else null,
    // the skill flag stored as integer 0.
    let q1_fresh = svc.get_quest(q1).await.unwrap().unwrap();
    assert_eq!(
        q1_fresh,
        json!({
            "id": 1, "name": "Iron Challenge", "planet": "Calypso", "waypoint": null,
            "cooldown_hours": null, "reward_ped": null, "reward_is_skill": 0,
            "expected_reward_markup_percent": null, "notes": null, "chain_name": null,
            "chain_position": null, "chain_total": null, "started_at": null,
            "is_active": 1, "created_at": 1000.0, "category": null,
            "reward_description": null, "updated_at": 1000.0, "signal_loot_item": null,
            "completion_trigger": "mission_log", "reward_policy": "none",
            "family_id": null, "curated": false, "catalogue_key": null,
            "cooldown_anchor": "completion", "last_started_at": null,
            "family_name": null, "family_cooldown_hours": null,
            "family_cooldown_anchor": null, "last_completed_at": null,
            "last_abandoned_at": null,
            "cooldown_expires_at": null, "family_cooldown_expires_at": null,
            "reward_undo_available": false, "mobs": [], "reward_item_names": [],
        })
    );

    // The full quest: mobs strip, drop empties, dedupe, and read
    // back sorted; the integer cooldown stores as REAL.
    let q2_fresh = svc.get_quest(q2).await.unwrap().unwrap();
    assert_eq!(q2_fresh["planet"], "Foma");
    assert_eq!(q2_fresh["cooldown_hours"], json!(24.0));
    assert_eq!(q2_fresh["expected_reward_markup_percent"], Value::Null);
    assert_eq!(q2_fresh["mobs"], json!(["Atrax", "Atrox"]));
    assert_eq!(q2_fresh["reward_is_skill"], json!(0));
    assert_eq!(q2_fresh["chain_position"], json!(1));

    // A skill reward normalises its markup away at creation.
    let q3_fresh = svc.get_quest(q3).await.unwrap().unwrap();
    assert_eq!(q3_fresh["reward_is_skill"], json!(1));
    assert_eq!(q3_fresh["expected_reward_markup_percent"], Value::Null);
}

#[tokio::test]
async fn cooldown_derives_from_the_latest_completion() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());

    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('sess-1', ?1, 1772366400.0)",
            params![q2],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let quest = svc.get_quest(q2).await.unwrap().unwrap();
    assert_eq!(quest["last_completed_at"], json!(1772366400.0));
    assert_eq!(
        quest["cooldown_expires_at"],
        json!("2026-03-02T12:00:00+00:00"),
        "completion instant plus 24 hours, rendered as a UTC ISO instant"
    );
}

#[tokio::test]
async fn reward_review_refusals_are_typed_and_a_confirmed_review_is_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, clock, _bus) = service_with_clock(dir.path()).await;
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock);
    let reconciled = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reconciled_sink = reconciled.clone();
    svc.set_loot_reconciler(Arc::new(move |source_id| {
        reconciled_sink.lock().unwrap().push(source_id);
        Box::pin(async {})
    }));
    let quest = quest_id(
        &svc.create_quest(&json!({"name": "Ambiguous daily", "cooldown_hours": 24}))
            .await
            .unwrap(),
    );

    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active) \
             VALUES('s-review', 1772366300.0, 1772366400.0, 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_contexts(id, session_id, created_at) \
             VALUES(801, 's-review', 1772366300.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kills(id, loot_source_id, session_id, mob_name, timestamp, context_id, loot_total_ped) \
             VALUES('k-review', 'review-source', 's-review', 'Target', 1772366400.0, 801, 1.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES('k-review', 'Blazar Fragment', 10, 1.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO quest_runs(id, quest_id, status, started_at, completed_at) \
             VALUES(803, ?, 'completed', 1772366300.0, 1772366400.0)",
            params![quest],
        )?;
        conn.execute(
            "INSERT INTO session_quest_completions \
             (id, session_id, quest_id, completed_at, activity_context_id, reward_outcome, \
              reward_policy_snapshot, reward_unresolved_reason, reward_evidence_json, \
              quest_run_id) \
             VALUES(802, 's-review', ?, 1772366400.0, 801, 'unresolved', 'completion_clump', \
                    'ambiguous clump', ?, 803)",
            params![
                quest,
                json!({
                    "loot": [
                        {"item_name": "Universal Ammo", "quantity": 20000, "value": 2.0},
                        {"item_name": "Blazar Fragment", "quantity": 10, "value": 1.0}
                    ],
                    "isolated": true,
                })
                .to_string()
            ],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let before = analytics.overview("all").await.unwrap();
    assert_eq!(before.returns_breakdown.loot_tt, 1.0);
    assert_eq!(before.returns_breakdown.quest_item_tt, 0.0);
    assert!(before.returns_breakdown.ledger.is_empty());

    let error = svc
        .resolve_reward_review(802, &[], false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuestError::Invalid(message) if message == "select at least one reward item")
    );

    let error = svc
        .resolve_reward_review(802, &[2], false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuestError::Invalid(message) if message == "reward selection is out of range")
    );

    db.with_writer(|conn| {
        conn.execute(
            "UPDATE kill_loot_items SET value_ped = 3.0 WHERE kill_id = 'k-review'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let error = svc
        .resolve_reward_review(802, &[1], false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuestError::Invalid(message) if message.contains("expected one exact acquisition, found 0"))
    );
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE kill_loot_items SET value_ped = 1.0 WHERE kill_id = 'k-review'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    svc.resolve_reward_review(802, &[0, 1], false)
        .await
        .unwrap();
    assert!(svc.unresolved_reward_reviews().await.unwrap().is_empty());
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_reward_reviews").await,
        1
    );
    let after = analytics.overview("all").await.unwrap();
    assert_eq!(after.returns_breakdown.loot_tt, 0.0);
    assert_eq!(after.returns_breakdown.quest_item_tt, 1.0);
    assert_eq!(after.returns_breakdown.ledger["quest_reward"], 2.0);
    assert_eq!(after.total_gains, 3.0);
    assert_eq!(*reconciled.lock().unwrap(), vec!["review-source"]);
    let inventory = analytics
        .stock_positions(crate::analytics::Profession::Inventory)
        .await
        .unwrap();
    let blazar = inventory
        .iter()
        .find(|item| item.item_name == "Blazar Fragment")
        .expect("reviewed stock reward enters Inventory");
    assert_eq!(blazar.quantity, 10.0);
    assert_eq!(blazar.tt_value, 1.0);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM kills WHERE id = 'k-review' AND loot_total_ped = 0"
        )
        .await,
        1
    );

    let error = svc
        .resolve_reward_review(802, &[0], false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuestError::Invalid(message) if message == "completion has already been reviewed")
    );

    svc.cancel_quest(quest, true).await.unwrap();
    assert_eq!(
        *reconciled.lock().unwrap(),
        vec!["review-source", "review-source"]
    );
    let reversed = analytics.overview("all").await.unwrap();
    assert_eq!(reversed.returns_breakdown.loot_tt, 1.0);
    assert_eq!(reversed.returns_breakdown.quest_item_tt, 0.0);
    assert_eq!(reversed.returns_breakdown.ledger["quest_reward"], 2.0);
    assert_eq!(reversed.losses_breakdown.ledger["quest_reward"], 2.0);
    assert_eq!(reversed.total_gains - reversed.total_losses, 1.0);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM kills WHERE id = 'k-review' AND loot_total_ped = 1.0"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn legacy_fixed_ped_rows_read_and_complete_as_no_reward_claim() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let generic = quest_id(
        &svc.create_quest(&json!({"name": "Legacy generic"}))
            .await
            .unwrap(),
    );
    let mission = quest_id(
        &svc.create_quest(&json!({"name": "Legacy mission"}))
            .await
            .unwrap(),
    );
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE quests SET reward_policy = 'fixed_ped', reward_ped = 5.0 \
             WHERE id IN (?, ?)",
            params![generic, mission],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(
        svc.get_quest(generic).await.unwrap().unwrap()["reward_policy"],
        json!("none")
    );
    svc.start_quest(generic).await.unwrap();
    svc.complete_quest(generic).await.unwrap();
    svc.start_quest(mission).await.unwrap();
    svc.mission_complete_check(&[MissionCompletion {
        mission_name: "Legacy mission".to_string(),
        loot_items: Vec::new(),
        skill_gains: Vec::new(),
        isolated: true,
    }])
    .await
    .unwrap();

    let outcomes = db
        .with_reader(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT reward_outcome, reward_policy_snapshot, reward_ped IS NULL \
                 FROM session_quest_completions \
                 WHERE quest_id IN (?, ?) ORDER BY quest_id",
            )?;
            let rows = stmt
                .query_map(params![generic, mission], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    assert_eq!(
        outcomes,
        vec![
            ("none".to_string(), "none".to_string(), 1),
            ("none".to_string(), "none".to_string(), 1),
        ]
    );
}

#[tokio::test]
async fn reward_undo_is_available_without_a_cooldown() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let quest = quest_id(
        &svc.create_quest(&json!({
            "name": "No-cooldown voucher",
            "completion_trigger": "signal_item",
            "signal_loot_item": "Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["Daily Voucher"],
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(quest).await.unwrap();
    svc.signal_loot_check(&[marker_value("Daily Voucher", 1, 0.0)])
        .await
        .unwrap();
    assert_eq!(
        svc.get_quest(quest).await.unwrap().unwrap()["reward_undo_available"],
        json!(true)
    );

    svc.cancel_quest(quest, true).await.unwrap();
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_reward_reversals").await,
        1
    );
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_cooldown_resets").await,
        0,
        "economic correction is independent of cooldown correction"
    );
    assert_eq!(
        svc.get_quest(quest).await.unwrap().unwrap()["reward_undo_available"],
        json!(false)
    );
}

#[tokio::test]
async fn create_quest_ignores_a_falsy_mobs_payload_without_dereferencing_it() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    // A present-but-null mobs value is falsy on create: it writes no mob
    // rows and must never be dereferenced as a list.
    let created = svc
        .create_quest(&json!({"name": "Bounty", "mobs": null}))
        .await
        .unwrap();
    let id = quest_id(&created);
    let stored = svc.get_quest(id).await.unwrap().unwrap();
    assert_eq!(stored["name"], "Bounty");
    assert_eq!(stored["mobs"], json!([]), "no mob rows written");
}

#[tokio::test]
async fn soft_deleting_a_quest_keeps_its_mob_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let q1 = quest_id(
        &svc.create_quest(&json!({"name": "Iron Challenge", "mobs": ["Atrox"]}))
            .await
            .unwrap(),
    );

    assert!(svc.delete_quest(q1).await.unwrap());
    // The mob rows stay (the autocomplete reader filters by active quests, so
    // they vanish from that surface without being destroyed).
    let mobs: i64 = db
        .with_reader(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM quest_mobs WHERE quest_id = ?1",
                params![q1],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(mobs, 1);
    assert_eq!(svc.get_all_mob_names().await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn mob_autocomplete_lists_active_quest_mobs_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    svc.create_quest(&full_quest_payload()).await.unwrap();
    svc.create_quest(&json!({"name": "Side Hunt", "mobs": ["Snablesnot"]}))
        .await
        .unwrap();
    assert_eq!(
        svc.get_all_mob_names().await.unwrap(),
        ["Atrax", "Atrox", "Snablesnot"]
    );
}

#[tokio::test]
async fn a_zero_hour_cooldown_never_produces_an_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let mut payload = full_quest_payload();
    payload["cooldown_hours"] = json!(0);
    let q = quest_id(&svc.create_quest(&payload).await.unwrap());
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('sess-1', ?1, 1772366400.0)",
            params![q],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let quest = svc.get_quest(q).await.unwrap().unwrap();
    assert_eq!(quest["last_completed_at"], json!(1772366400.0));
    assert_eq!(
        quest["cooldown_expires_at"],
        Value::Null,
        "the expiry derives only from a strictly positive cooldown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_received_mission_event_starts_its_quest() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db, _clock, bus) = service_with_clock(dir.path()).await;
    let q = quest_id(
        &svc.create_quest(&json!({"name": "Iron Challenge"}))
            .await
            .unwrap(),
    );

    bus.publish(&BusEvent::MissionReceived(MissionReceivedPayload {
        kind: MissionReceivedTag,
        timestamp: "2026-01-01T00:00:01".into(),
        mission_name: "Iron Challenge".into(),
    }));
    assert!(json_truthy(
        svc.get_quest(q).await.unwrap().unwrap().get("started_at")
    ));
    // A nameless event is ignored.
    bus.publish(&BusEvent::MissionReceived(MissionReceivedPayload {
        kind: MissionReceivedTag,
        timestamp: "2026-01-01T00:00:01".into(),
        mission_name: "".into(),
    }));
}

#[tokio::test]
async fn starting_an_inactive_quest_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let q = quest_id(&svc.create_quest(&json!({"name": "Dead"})).await.unwrap());
    svc.delete_quest(q).await.unwrap();
    assert_eq!(svc.start_quest(q).await.unwrap(), None);
}

#[tokio::test]
async fn equal_fuzzy_scores_keep_the_first_quest() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let first = quest_id(
        &svc.create_quest(&json!({"name": "iron chal a"}))
            .await
            .unwrap(),
    );
    pin_ts(&db, "quests", first, 1000.0).await;
    let second = quest_id(
        &svc.create_quest(&json!({"name": "iron chal b"}))
            .await
            .unwrap(),
    );
    pin_ts(&db, "quests", second, 1001.0).await;

    // Both names score 0.9090909090909091 against the mission (the
    // reference's figure); the strictly-greater comparison keeps
    // the earlier quest.
    let matched = svc
        .match_quest_by_mission_name("iron chal c", false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matched["id"], json!(first));
}

#[tokio::test]
async fn cancelling_outside_the_cooldown_window_keeps_completions() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let qa = quest_id(&svc.create_quest(&json!({"name": "Alpha"})).await.unwrap());
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('s4', ?1, 1000.0)",
            params![qa],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // No cooldown configured: never cooling, the completion stays.
    let result = svc.cancel_quest(qa, false).await.unwrap().unwrap();
    assert_eq!(result["last_completed_at"], json!(1000.0));

    // A cooldown that expires exactly at the current instant is no
    // longer cooling (the strict comparison), so the completion
    // stays here too.
    let qe = quest_id(
        &svc.create_quest(&json!({"name": "Edge", "cooldown_hours": 1}))
            .await
            .unwrap(),
    );
    let expires_at = 1772366400.0 - 3600.0;
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('s5', ?1, ?2)",
            params![qe, expires_at],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = svc.cancel_quest(qe, false).await.unwrap().unwrap();
    assert_eq!(result["last_completed_at"], json!(1772362800.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_session_id_skips_overlay_events() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock, bus) = service_with_clock(dir.path()).await;
    svc.create_quest(&json!({"name": "Iron Challenge"}))
        .await
        .unwrap();

    // The original's truthiness gate treats an empty session id as
    // no session: the quest starts but no overlay event records.
    bus.publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
        session_id: "".into(),
    }));
    svc.start_quest_from_mission("Iron Challenge")
        .await
        .unwrap();
    let count = count_rows(&db, "SELECT COUNT(*) FROM notable_events").await;
    assert_eq!(count, 0);
}

#[test]
fn truthiness_matches_python_bool() {
    // The expectation table is Python's bool() over each shape.
    assert!(!json_truthy(None));
    assert!(!json_truthy(Some(&json!(null))));
    assert!(!json_truthy(Some(&json!(false))));
    assert!(json_truthy(Some(&json!(true))));
    assert!(!json_truthy(Some(&json!(0))));
    assert!(!json_truthy(Some(&json!(0.0))));
    assert!(json_truthy(Some(&json!(2))));
    assert!(!json_truthy(Some(&json!(""))));
    assert!(json_truthy(Some(&json!("no"))));
    assert!(!json_truthy(Some(&json!([]))));
    assert!(json_truthy(Some(&json!(["x"]))));
    assert!(!json_truthy(Some(&json!({}))));
    assert!(json_truthy(Some(&json!({"k": 1}))));
}

/// A completion reports the quest to the interval layer so a declared
/// stretch of it closes at the completion moment; a start reports
/// NOTHING, because the mission log only witnesses pickup (bulk pickup
/// separates it from the play that advances the quest), and which
/// stretch of play is toward a quest is the user's own declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_completion_reports_to_the_interval_layer() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let reported = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = reported.clone();
    svc.set_stretch_closer(Arc::new(move |quest_id| {
        sink.lock().unwrap().push(quest_id);
        Box::pin(async {})
    }));

    let quest = quest_id(
        &svc.create_quest(&json!({"name": "Daily: Carabok"}))
            .await
            .unwrap(),
    );

    svc.start_quest(quest).await.unwrap();
    assert!(
        reported.lock().unwrap().is_empty(),
        "a start opens no stretch: the stretch is the user's declaration"
    );

    svc.complete_quest(quest).await.unwrap();
    assert_eq!(*reported.lock().unwrap(), vec![quest]);
}

// ── Signal-completed quests ─────────────────────────────────────────

/// A signal-probe loot line, as the watcher would hand it over.
fn marker(item_name: &str, quantity: i64) -> SignalLoot {
    SignalLoot {
        item_name: item_name.to_string(),
        quantity,
        value_ped: Ped::ZERO,
    }
}

fn marker_value(item_name: &str, quantity: i64, value_ped: f64) -> SignalLoot {
    SignalLoot {
        item_name: item_name.to_string(),
        quantity,
        value_ped: Ped(value_ped),
    }
}

/// Manual hand-in preserves filtered raw items, offers retrospective and
/// prospective candidates through one state machine, and atomically replaces
/// the exact ordinary source clump on confirmation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_hand_in_confirms_one_exact_raw_clump() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, clock, bus) = service_with_clock(dir.path()).await;
    let closed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let closed_sink = closed.clone();
    svc.set_stretch_closer(Arc::new(move |quest_id| {
        closed_sink.lock().unwrap().push(quest_id);
        Box::pin(async {})
    }));
    let quest = quest_id(
        &svc.create_quest(&json!({
            "name": "Daily terminal",
            "completion_trigger": "manual_hand_in",
            "reward_policy": "none",
        }))
        .await
        .unwrap(),
    );
    let created = svc.get_quest(quest).await.unwrap().unwrap();
    assert_eq!(created["completion_trigger"], "manual_hand_in");
    assert_eq!(created["reward_policy"], "completion_clump");
    svc.start_quest(quest).await.unwrap();
    let refused = svc.complete_quest(quest).await.unwrap_err();
    assert!(matches!(
        refused,
        QuestError::Invalid(message)
            if message == "Manual hand-in quests must be completed by confirming an exact reward clump"
    ));
    assert!(svc.get_quest(quest).await.unwrap().unwrap()["started_at"].is_number());
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM session_quest_completions").await,
        0
    );

    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id, started_at, is_active) \
             VALUES('s-manual', 1772366400.0, 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_intervals(id, session_id, kind, label, ref_id, started_at) \
             VALUES(501, 's-manual', 'quest', 'Daily terminal', ?, 1772366400.0)",
            params![quest],
        )?;
        conn.execute(
            "INSERT INTO session_contexts(id, session_id, created_at) \
             VALUES(502, 's-manual', 1772366400.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_context_intervals(context_id, interval_id) VALUES(502, 501)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kills(id, loot_source_id, session_id, mob_name, timestamp, context_id, loot_total_ped) \
             VALUES('k-old', 'clump-old', 's-manual', 'Unknown', 1772366450.0, 502, 0.0023)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES('k-old', 'Blazar Fragment', 100, 0.0023)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    bus.publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
        session_id: "s-manual".into(),
    }));
    clock.advance(60.0).unwrap();

    svc.raw_loot_clump_check(&RawLootClump {
        source_id: "clump-old".to_string(),
        timestamp: Some("2026-03-01T12:00:50".to_string()),
        items: vec![
            marker_value("Universal Ammo", 100_000, 10.0),
            marker_value("Blazar Fragment", 100, 0.0023),
        ],
    })
    .await
    .unwrap();
    let retrospective = svc.hand_in_begin(quest).await.unwrap();
    let old = retrospective.candidate.expect("latest clump offered");
    assert_eq!(old.items.len(), 2, "filtered ammo remains in raw evidence");

    let waiting = svc.hand_in_wait(quest, old.id).await.unwrap();
    assert!(waiting.waiting);
    assert!(waiting.candidate.is_none());
    svc.hand_in_cancel(quest).await.unwrap();
    let cancelled = svc.hand_in_state(quest).await.unwrap();
    assert!(!cancelled.waiting);
    assert_eq!(cancelled.candidate.expect("candidate preserved").id, old.id);
    assert!(svc.get_quest(quest).await.unwrap().unwrap()["started_at"].is_number());
    assert!(closed.lock().unwrap().is_empty());
    svc.hand_in_wait(quest, old.id).await.unwrap();

    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO kills(id, loot_source_id, session_id, mob_name, timestamp, context_id, loot_total_ped) \
             VALUES('k-new', 'clump-new', 's-manual', 'Unknown', 1772366460.0, 502, 0.0023)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
             VALUES('k-new', 'Blazar Fragment', 238, 0.0023)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    svc.raw_loot_clump_check(&RawLootClump {
        source_id: "clump-new".to_string(),
        timestamp: Some("2026-03-01T12:01:00".to_string()),
        items: vec![
            marker_value("Universal Ammo", 316_468, 31.64),
            marker_value("Blazar Fragment", 238, 0.0023),
        ],
    })
    .await
    .unwrap();
    let prospective = svc.hand_in_state(quest).await.unwrap();
    let new = prospective.candidate.expect("next clump offered");
    assert_eq!(new.source_id, "clump-new");

    let reclassified = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = reclassified.clone();
    svc.set_loot_reconciler(Arc::new(move |source_id| {
        sink.lock().unwrap().push(source_id);
        Box::pin(async {})
    }));
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE kills SET loot_source_id = 'clump-detached' WHERE id = 'k-new'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let refused = svc.hand_in_confirm(quest, new.id).await.unwrap_err();
    assert!(matches!(refused, QuestError::Invalid(_)));
    assert!(
        closed.lock().unwrap().is_empty(),
        "a refusal keeps the stretch open"
    );
    assert!(reclassified.lock().unwrap().is_empty());
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM session_quest_completions WHERE quest_id > 0"
        )
        .await,
        0
    );
    db.with_writer(|conn| {
        conn.execute(
            "UPDATE kills SET loot_source_id = 'clump-new' WHERE id = 'k-new'",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    svc.hand_in_confirm(quest, new.id).await.unwrap();
    assert_eq!(*reclassified.lock().unwrap(), vec!["clump-new"]);
    assert_eq!(*closed.lock().unwrap(), vec![quest]);

    let evidence = db
        .with_reader(move |conn| {
            let reward_items = conn
                .prepare(
                    "SELECT ri.item_name, ri.quantity, ri.value_ped, ri.accounting_kind, \
                            ri.ledger_entry_id IS NOT NULL \
                     FROM session_quest_completion_reward_items ri \
                     JOIN session_quest_completions c ON c.id = ri.completion_id \
                     WHERE c.quest_id = ? ORDER BY ri.id",
                )?
                .query_map(params![quest], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let source = conn.query_row(
                "SELECT k.loot_total_ped, li.deactivated_at IS NOT NULL, \
                        c.claimed_completion_id IS NOT NULL \
                 FROM kills k JOIN kill_loot_items li ON li.kill_id = k.id \
                 JOIN quest_reward_clumps c ON c.source_id = k.loot_source_id \
                 WHERE k.id = 'k-new'",
                [],
                |row| {
                    Ok((
                        row.get::<_, f64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?;
            let liquid_gain = conn.query_row(
                "SELECT amount FROM ledger_entries WHERE tag = 'quest_reward'",
                [],
                |row| row.get::<_, f64>(0),
            )?;
            let attribution_weight = conn.query_row(
                "SELECT weight FROM quest_reward_attributions a \
                 JOIN session_quest_completions c ON c.id = a.completion_id \
                 WHERE c.quest_id = ?",
                params![quest],
                |row| row.get::<_, f64>(0),
            )?;
            Ok((reward_items, source, liquid_gain, attribution_weight))
        })
        .await
        .unwrap();
    assert_eq!(
        evidence.0,
        vec![
            (
                "Universal Ammo".to_string(),
                316_468,
                31.64,
                "liquid".to_string(),
                1
            ),
            (
                "Blazar Fragment".to_string(),
                238,
                0.0023,
                "stock".to_string(),
                0
            ),
        ]
    );
    assert_eq!(evidence.1, (0.0, 1, 1));
    assert!((evidence.2 - 31.64).abs() < 1e-9);
    assert!((evidence.3 - 1.0).abs() < 1e-9);
    assert!(svc.get_quest(quest).await.unwrap().unwrap()["started_at"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_hand_in_does_not_reuse_an_overlapping_signal_reward_line() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock, bus) = service_with_clock(dir.path()).await;
    let manual = quest_id(
        &svc.create_quest(&json!({
            "name": "AI Daily terminal",
            "completion_trigger": "manual_hand_in",
        }))
        .await
        .unwrap(),
    );
    let signal = quest_id(
        &svc.create_quest(&json!({
            "name": "Hyperion Boss",
            "completion_trigger": "signal_item",
            "signal_loot_item": "Hyperion Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["Hyperion Daily Voucher"],
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(manual).await.unwrap();
    svc.start_quest(signal).await.unwrap();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id, started_at, is_active) \
             VALUES('s-overlap', 1772366400.0, 1)",
            [],
        )?;
        for (id, quest_id, name) in [
            (601, manual, "AI Daily terminal"),
            (602, signal, "Hyperion Boss"),
        ] {
            conn.execute(
                "INSERT INTO session_intervals(id, session_id, kind, label, ref_id, started_at) \
                 VALUES(?, 's-overlap', 'quest', ?, ?, 1772366400.0)",
                params![id, name, quest_id],
            )?;
        }
        conn.execute(
            "INSERT INTO session_contexts(id, session_id, created_at) \
             VALUES(603, 's-overlap', 1772366400.0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_context_intervals(context_id, interval_id) \
             VALUES(603, 601), (603, 602)",
            [],
        )?;
        conn.execute(
            "INSERT INTO kills(id, loot_source_id, session_id, mob_name, timestamp, context_id) \
             VALUES('k-overlap', 'clump-overlap', 's-overlap', 'Unknown', 1772366450.0, 603)",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    bus.publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
        session_id: "s-overlap".into(),
    }));

    svc.raw_loot_clump_check(&RawLootClump {
        source_id: "clump-overlap".to_string(),
        timestamp: Some("2026-03-01T12:00:50".to_string()),
        items: vec![
            marker_value("Universal Ammo", 316_468, 31.64),
            marker("Hyperion Daily Voucher", 1),
        ],
    })
    .await
    .unwrap();

    let candidate = svc
        .hand_in_begin(manual)
        .await
        .unwrap()
        .candidate
        .expect("manual candidate");
    assert_eq!(candidate.items.len(), 1);
    assert_eq!(candidate.items[0].item_name, "Universal Ammo");
    assert!(svc.get_quest(signal).await.unwrap().unwrap()["started_at"].is_null());
}

/// The signal path end to end at the service: an in-progress signal
/// quest completes when its item arrives (case-insensitively, trimmed),
/// records the completion, reports the stretch close, and clears the
/// in-progress state so the run is over until the next declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_loot_tick_completes_the_in_progress_signal_quest() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let closed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = closed.clone();
    svc.set_stretch_closer(Arc::new(move |quest_id| {
        sink.lock().unwrap().push(quest_id);
        Box::pin(async {})
    }));

    let boss = quest_id(
        &svc.create_quest(&json!({
            "name": "Hyperion Boss 1",
            "signal_loot_item": "Hyperion Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["Hyperion Daily Voucher"],
            "cooldown_hours": 20,
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(boss).await.unwrap();

    // A mission-less loot tick with the marker (case and padding off on
    // purpose) completes the run; unrelated items complete nothing.
    svc.signal_loot_check(&[
        marker("Shrapnel", 4639),
        marker_value(" hyperion daily voucher ", 1, 0.25),
        marker("Hyperium", 2),
    ])
    .await
    .unwrap();

    let quest = svc.get_quest(boss).await.unwrap().unwrap();
    assert!(quest["started_at"].is_null(), "the run ended");
    assert!(!quest["last_completed_at"].is_null(), "completion recorded");
    assert_eq!(*closed.lock().unwrap(), vec![boss]);
    let source = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT reward_source, reward_kind, reward_ped FROM session_quest_completions \
                 WHERE quest_id = ?",
                params![boss],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                    ))
                },
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        source,
        ("tracked_loot".to_string(), "item".to_string(), None)
    );
    let reward_item = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT ri.item_name, ri.quantity, ri.value_ped \
                 FROM session_quest_completion_reward_items ri \
                 JOIN session_quest_completions c ON c.id = ri.completion_id \
                 WHERE c.quest_id = ?",
                params![boss],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )?)
        })
        .await
        .unwrap();
    assert_eq!(reward_item, ("hyperion daily voucher".to_string(), 1, 0.25));

    svc.cancel_quest(boss, false).await.unwrap();
    let remaining_reward_items = count_rows(
        &db,
        "SELECT COUNT(*) FROM session_quest_completion_reward_items",
    )
    .await;
    assert_eq!(
        remaining_reward_items, 1,
        "cooldown reset preserves the reward fact"
    );
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_cooldown_resets").await,
        1
    );
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_reward_reversals").await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reward_undo_is_append_only_and_waits_for_stock_dependants() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, clock, _bus) = service_with_clock(dir.path()).await;
    let analytics = crate::analytics::AnalyticsService::new(db.clone(), clock);
    let quest = quest_id(
        &svc.create_quest(&json!({
            "name": "Voucher boss",
            "completion_trigger": "signal_item",
            "signal_loot_item": "AI Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["AI Daily Voucher"],
            "cooldown_hours": 20,
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(quest).await.unwrap();
    svc.signal_loot_check(&[marker_value("AI Daily Voucher", 1, 0.0)])
        .await
        .unwrap();

    let inventory = analytics
        .stock_positions(crate::analytics::Profession::Inventory)
        .await
        .unwrap();
    assert_eq!(
        inventory
            .iter()
            .find(|item| item.item_name == "AI Daily Voucher")
            .expect("confirmed reward stock")
            .quantity,
        1.0,
    );
    analytics
        .create_private_sale(
            crate::analytics::Profession::Inventory,
            "AI Daily Voucher",
            1.0,
            2.0,
            Some("2026-03-01"),
        )
        .await
        .unwrap();

    let refused = svc.cancel_quest(quest, true).await.unwrap_err();
    assert!(refused
        .to_string()
        .contains("listed, sold, converted, or removed"));
    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_reward_reversals").await,
        0
    );

    let sale_id = analytics
        .activity_history(crate::analytics::Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "trade")
        .expect("sale history")
        .id;
    assert!(analytics.undo_private_sale(&sale_id).await.unwrap());
    svc.cancel_quest(quest, true).await.unwrap();

    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM quest_reward_reversals").await,
        1
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM session_quest_completion_reward_items"
        )
        .await,
        1,
        "undo preserves immutable reward evidence",
    );
    let inventory = analytics
        .stock_positions(crate::analytics::Profession::Inventory)
        .await
        .unwrap();
    assert!(inventory
        .iter()
        .all(|item| item.item_name != "AI Daily Voucher"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_marker_lines_share_one_assignment_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let boss = quest_id(
        &svc.create_quest(&json!({
            "name": "Hyperion Boss",
            "signal_loot_item": "Hyperion Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["Hyperion Daily Voucher"],
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(boss).await.unwrap();

    let tick = vec![
        json!({"item_name": "Hyperion Daily Voucher", "quantity": 1, "value": 0.0}),
        json!({"item_name": "Hyperion Daily Voucher", "quantity": 1, "value": 0.0}),
    ];
    assert_eq!(
        svc.signal_reward_filter(&tick).await.unwrap(),
        Some(json!({"suppress_loot_indices": [0]}))
    );
    svc.signal_loot_check(&[
        marker("Hyperion Daily Voucher", 1),
        marker("Hyperion Daily Voucher", 1),
    ])
    .await
    .unwrap();

    assert_eq!(
        count_rows(&db, "SELECT COUNT(*) FROM session_quest_completions").await,
        1
    );
}

/// A signal quest that is NOT in progress ignores its marker: an
/// undeclared run stays unrecorded rather than being invented from
/// loot (the same honesty rule the Activities control follows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_without_a_declared_run_completes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let boss = quest_id(
        &svc.create_quest(&json!({
            "name": "Hyperion Boss 1",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap(),
    );

    svc.signal_loot_check(&[marker("Hyperion Daily Voucher", 1)])
        .await
        .unwrap();

    let quest = svc.get_quest(boss).await.unwrap().unwrap();
    assert!(
        quest["last_completed_at"].is_null(),
        "no declared run, no completion"
    );
}

/// One marker completes one run: two in-progress quests sharing a
/// signal item draw on the tick's occurrence budget oldest-first, so a
/// single marker cannot complete both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_marker_completes_one_of_two_quests_sharing_the_signal() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let first = quest_id(
        &svc.create_quest(&json!({
            "name": "Boss A",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap(),
    );
    let second = quest_id(
        &svc.create_quest(&json!({
            "name": "Boss B",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(first).await.unwrap();
    svc.start_quest(second).await.unwrap();
    // Pin distinct start instants so "oldest first" is deterministic.
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE quests SET started_at = 100.0 WHERE id = ?",
            params![first],
        )?;
        conn.execute(
            "UPDATE quests SET started_at = 200.0 WHERE id = ?",
            params![second],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    svc.signal_loot_check(&[marker("Hyperion Daily Voucher", 1)])
        .await
        .unwrap();

    let first_quest = svc.get_quest(first).await.unwrap().unwrap();
    let second_quest = svc.get_quest(second).await.unwrap().unwrap();
    assert!(
        !first_quest["last_completed_at"].is_null(),
        "the oldest-started run completed"
    );
    assert!(
        second_quest["last_completed_at"].is_null() && !second_quest["started_at"].is_null(),
        "the newer run keeps going"
    );
}

/// A stacked marker line pays for that many runs: the budget counts
/// units, not lines, so one loot line carrying quantity 2 completes
/// two in-progress quests sharing the signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stacked_marker_line_pays_for_that_many_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let first = quest_id(
        &svc.create_quest(&json!({
            "name": "Boss A",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap(),
    );
    let second = quest_id(
        &svc.create_quest(&json!({
            "name": "Boss B",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap(),
    );
    svc.start_quest(first).await.unwrap();
    svc.start_quest(second).await.unwrap();
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE quests SET started_at = 100.0 WHERE id = ?",
            params![first],
        )?;
        conn.execute(
            "UPDATE quests SET started_at = 200.0 WHERE id = ?",
            params![second],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    svc.signal_loot_check(&[marker("Hyperion Daily Voucher", 2)])
        .await
        .unwrap();

    for quest in [first, second] {
        let row = svc.get_quest(quest).await.unwrap().unwrap();
        assert!(
            !row["last_completed_at"].is_null(),
            "both stacked-paid runs completed"
        );
    }
}

/// The colon-variant discipline: a variant-family quest is matched
/// only by a mission line carrying the same family and a variant that
/// clears the fuzzy bar on its own, so the bare umbrella line a
/// pickup emits (which crosses the whole-string fuzzy bar) and a
/// sibling variant's line can never cross-match it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn variant_family_quests_refuse_umbrella_and_sibling_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let variant = quest_id(
        &svc.create_quest(&json!({"name": "ARIS - Daily Hunting 3: Verderons"}))
            .await
            .unwrap(),
    );
    svc.start_quest(variant).await.unwrap();

    // The umbrella chooser line is a DIFFERENT mission: whole-string
    // scoring puts it exactly at the 0.8 bar, but the structural rule
    // refuses a family-only line against a variant quest.
    assert!(svc
        .match_quest_by_mission_name("ARIS - Daily Hunting 3", true)
        .await
        .unwrap()
        .is_none());
    // A sibling variant shares the long family prefix but is its own
    // mission: variants are compared on their own, and "Fieroids"
    // against "Verderons" is nowhere near the bar.
    assert!(svc
        .match_quest_by_mission_name("ARIS - Daily Hunting 3: Fieroids", true)
        .await
        .unwrap()
        .is_none());
    // The quest's own line still matches through the decorations...
    let matched = svc
        .match_quest_by_mission_name("ARIS - Daily Hunting 3: VERDERONS (Repeatable)", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matched["id"], json!(variant));
    // ...and a near-identical variant spelling fuzzy-matches within
    // the family.
    let matched = svc
        .match_quest_by_mission_name("ARIS - Daily Hunting 3: Verderon", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matched["id"], json!(variant));
}

/// The completion gate composes with the discipline: an umbrella line
/// through the completion check completes nothing even while the
/// variant quest is in progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_umbrella_line_never_completes_a_variant_quest() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let variant = quest_id(
        &svc.create_quest(&json!({"name": "ARIS - Daily Hunting 1: Faint Fieroids"}))
            .await
            .unwrap(),
    );
    svc.start_quest(variant).await.unwrap();

    svc.mission_complete_check(&[MissionCompletion {
        mission_name: "ARIS - Daily Hunting 1".to_string(),
        loot_items: vec![],
        skill_gains: vec![],
        isolated: true,
    }])
    .await
    .unwrap();

    let quest = svc.get_quest(variant).await.unwrap().unwrap();
    assert!(
        quest["last_completed_at"].is_null() && !quest["started_at"].is_null(),
        "the variant keeps running through its umbrella's line"
    );
}

/// Completion evidence and reward policy are independent: a signal item may
/// prove completion while the quest grants PES or names that same item as its
/// additional reward.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_quest_accepts_an_independent_reward_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let skill = svc
        .create_quest(&json!({
            "name": "Boss",
            "signal_loot_item": "Hyperion Daily Voucher",
            "reward_ped": 2.0,
            "reward_is_skill": true,
        }))
        .await
        .unwrap();
    assert_eq!(skill["completion_trigger"], json!("signal_item"));
    assert_eq!(skill["reward_policy"], json!("fixed_pes"));

    let named = svc
        .create_quest(&json!({
            "name": "Named Boss",
            "completion_trigger": "signal_item",
            "signal_loot_item": "Hyperion Daily Voucher",
            "reward_policy": "named_items",
            "reward_item_names": ["Hyperion Daily Voucher"],
        }))
        .await
        .unwrap();
    assert_eq!(named["reward_policy"], json!("named_items"));
    assert_eq!(
        named["reward_item_names"],
        json!(["Hyperion Daily Voucher"])
    );

    let invalid = svc
        .create_quest(&json!({
            "name": "Missing marker",
            "completion_trigger": "signal_item",
        }))
        .await
        .unwrap_err();
    assert!(invalid.to_string().contains("requires a signal loot item"));
}

// ── Quest families: shared, anchor-aware cooldowns ──────────────────

#[tokio::test]
async fn family_crud_round_trips_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    // Defaults: planet Calypso, pickup anchor, no gate, no members.
    let family = svc
        .create_family(&json!({"name": "  Daily Hunting 1  "}))
        .await
        .unwrap();
    assert_eq!(family["name"], json!("Daily Hunting 1"), "name trims");
    assert_eq!(family["planet"], json!("Calypso"));
    assert_eq!(family["cooldown_anchor"], json!("pickup"));
    assert_eq!(family["cooldown_hours"], Value::Null);
    assert_eq!(family["member_count"], json!(0));
    assert_eq!(family["cooldown_expires_at"], Value::Null);

    // Refusals: blank name, bad anchor, non-positive hours.
    assert!(svc.create_family(&json!({"name": "  "})).await.is_err());
    assert!(svc
        .create_family(&json!({"name": "X", "cooldown_anchor": "sometimes"}))
        .await
        .is_err());
    assert!(svc
        .create_family(&json!({"name": "X", "cooldown_hours": 0}))
        .await
        .is_err());

    // Update binds present keys; absent keys keep.
    let fid = family["id"].as_i64().unwrap();
    let updated = svc
        .update_family(
            fid,
            &json!({"cooldown_hours": 20.0, "cooldown_anchor": "completion"}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated["name"], json!("Daily Hunting 1"));
    assert_eq!(updated["cooldown_hours"], json!(20.0));
    assert_eq!(updated["cooldown_anchor"], json!("completion"));

    // Delete soft-deletes off the active list.
    assert!(svc.delete_family(fid).await.unwrap());
    assert!(!svc.delete_family(fid).await.unwrap(), "already inactive");
    assert_eq!(svc.get_families(true).await.unwrap().len(), 0);
    assert_eq!(svc.get_families(false).await.unwrap().len(), 1);
}

#[tokio::test]
async fn creating_a_family_sweeps_matching_unattached_variants() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let a = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Weak Mortirex"}))
            .await
            .unwrap(),
    );
    let b = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Derilect Destroyer"}))
            .await
            .unwrap(),
    );
    let unrelated = quest_id(
        &svc.create_quest(&json!({"name": "Iron Challenge"}))
            .await
            .unwrap(),
    );
    // A variant already claimed by another family is never stolen.
    let other = svc
        .create_family(&json!({"name": "Daily Hunting 2"}))
        .await
        .unwrap();
    let claimed = quest_id(
        &svc.create_quest(&json!({
            "name": "Daily Hunting 1: Poached",
            "family_id": other["id"].as_i64().unwrap(),
        }))
        .await
        .unwrap(),
    );

    let family = svc
        .create_family(&json!({"name": "daily hunting 1", "cooldown_hours": 20.0}))
        .await
        .unwrap();
    assert_eq!(family["member_count"], json!(2), "case-insensitive sweep");
    let fid = family["id"].as_i64().unwrap();
    for (id, expect) in [
        (a, Some(fid)),
        (b, Some(fid)),
        (unrelated, None),
        (claimed, Some(other["id"].as_i64().unwrap())),
    ] {
        let quest = svc.get_quest(id).await.unwrap().unwrap();
        assert_eq!(quest["family_id"], json!(expect), "quest {id}");
    }
}

#[tokio::test]
async fn creating_a_quest_auto_attaches_by_name_only_when_family_id_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let family = svc
        .create_family(&json!({"name": "Daily Hunting 1"}))
        .await
        .unwrap();
    let fid = family["id"].as_i64().unwrap();

    // Absent key: the colon-split name attaches.
    let auto = svc
        .create_quest(&json!({"name": "Daily Hunting 1: Weak Mortirex"}))
        .await
        .unwrap();
    assert_eq!(auto["family_id"], json!(fid));
    assert_eq!(auto["family_name"], json!("Daily Hunting 1"));

    // Present-null key: explicitly standalone, no auto-attach.
    let standalone = svc
        .create_quest(&json!({"name": "Daily Hunting 1: Loner", "family_id": null}))
        .await
        .unwrap();
    assert_eq!(standalone["family_id"], Value::Null);

    // A dangling id refuses; an update never re-attaches implicitly.
    assert!(svc
        .create_quest(&json!({"name": "X", "family_id": 999}))
        .await
        .is_err());
    let renamed = svc
        .update_quest(
            quest_id(&standalone),
            &json!({"name": "Daily Hunting 1: Renamed Loner"}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed["family_id"], Value::Null, "detached stays detached");
}

#[tokio::test]
async fn family_cooldown_derives_from_member_instants_per_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db, clock, _bus) = service_with_clock(dir.path()).await;
    let family = svc
        .create_family(&json!({"name": "Daily Hunting 1", "cooldown_hours": 20.0}))
        .await
        .unwrap();
    let fid = family["id"].as_i64().unwrap();
    let a = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Weak Mortirex"}))
            .await
            .unwrap(),
    );
    let b = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Derilect Destroyer"}))
            .await
            .unwrap(),
    );

    // Pickup anchor: starting ANY member opens the family window on
    // EVERY member row, from the latest member start.
    svc.start_quest(a).await.unwrap();
    let a_row = svc.get_quest(a).await.unwrap().unwrap();
    let start_epoch = a_row["last_started_at"].as_f64().unwrap();
    let expected = crate::time::to_iso_utc(start_epoch + 20.0 * 3600.0);
    for id in [a, b] {
        let row = svc.get_quest(id).await.unwrap().unwrap();
        assert_eq!(row["family_id"], json!(fid));
        assert_eq!(row["family_cooldown_anchor"], json!("pickup"));
        assert_eq!(row["family_cooldown_expires_at"], json!(expected.clone()));
        assert_eq!(row["cooldown_expires_at"], Value::Null, "no own gate");
    }

    // Completing the run later leaves the pickup-anchored window where
    // the start put it; flipping the family to completion re-anchors
    // the same window on the completion instant.
    clock.advance(3600.0).unwrap();
    svc.complete_quest(a).await.unwrap();
    let row = svc.get_quest(b).await.unwrap().unwrap();
    assert_eq!(row["family_cooldown_expires_at"], json!(expected.clone()));
    let completion_epoch = start_epoch + 3600.0;
    svc.update_family(fid, &json!({"cooldown_anchor": "completion"}))
        .await
        .unwrap()
        .unwrap();
    let row = svc.get_quest(b).await.unwrap().unwrap();
    assert_eq!(
        row["family_cooldown_expires_at"],
        json!(crate::time::to_iso_utc(completion_epoch + 20.0 * 3600.0))
    );
}

#[tokio::test]
async fn start_stamps_a_durable_last_started_at_surviving_completion_and_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db, clock, _bus) = service_with_clock(dir.path()).await;
    let q = quest_id(
        &svc.create_quest(&json!({"name": "Iron Challenge"}))
            .await
            .unwrap(),
    );

    svc.start_quest(q).await.unwrap();
    let first_start = svc.get_quest(q).await.unwrap().unwrap()["last_started_at"]
        .as_f64()
        .unwrap();
    svc.complete_quest(q).await.unwrap();
    let row = svc.get_quest(q).await.unwrap().unwrap();
    assert_eq!(row["started_at"], Value::Null);
    assert_eq!(
        row["last_started_at"],
        json!(first_start),
        "survives completion"
    );

    clock.advance(60.0).unwrap();
    svc.start_quest(q).await.unwrap();
    svc.cancel_quest(q, false).await.unwrap();
    let row = svc.get_quest(q).await.unwrap().unwrap();
    assert_eq!(row["started_at"], Value::Null);
    assert_eq!(
        row["last_started_at"],
        json!(first_start + 60.0),
        "an abandon keeps the durable stamp (the giver's timer keeps running)"
    );
}

#[tokio::test]
async fn pickup_anchored_own_cooldown_cools_from_start_and_double_cancel_resets() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let q = quest_id(
        &svc.create_quest(&json!({
            "name": "Standing Contract",
            "cooldown_hours": 20.0,
            "cooldown_anchor": "pickup",
        }))
        .await
        .unwrap(),
    );

    svc.start_quest(q).await.unwrap();
    let row = svc.get_quest(q).await.unwrap().unwrap();
    let start_epoch = row["last_started_at"].as_f64().unwrap();
    assert_eq!(
        row["cooldown_expires_at"],
        json!(crate::time::to_iso_utc(start_epoch + 20.0 * 3600.0)),
        "a pickup-anchored own window opens at the start itself"
    );

    // First cancel un-starts and honours the timer; the second cancel is
    // the explicit reset, clearing the durable stamp (the pickup-anchor
    // parallel of deleting the latest completion).
    svc.cancel_quest(q, false).await.unwrap();
    let row = svc.get_quest(q).await.unwrap().unwrap();
    assert_eq!(row["started_at"], Value::Null);
    assert!(!row["cooldown_expires_at"].is_null(), "still cooling");
    svc.cancel_quest(q, false).await.unwrap();
    let row = svc.get_quest(q).await.unwrap().unwrap();
    assert_eq!(row["last_started_at"], Value::Null);
    assert_eq!(row["cooldown_expires_at"], Value::Null, "reset to ready");
}

#[tokio::test]
async fn a_member_cancel_never_clears_the_family_window() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let family = svc
        .create_family(&json!({"name": "Daily Hunting 1", "cooldown_hours": 20.0}))
        .await
        .unwrap();
    let a = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Weak Mortirex"}))
            .await
            .unwrap(),
    );

    svc.start_quest(a).await.unwrap();
    // Two cancels: the first un-starts; the second finds the member
    // neither started nor own-cooling (its own anchor is 'completion'
    // and it has no own gate), so it returns as-is. FAMILY cooling
    // alone never opens the reset branch, so the family-wide stamp
    // survives. (A member with its OWN pickup gate can still disavow
    // its start via the reset, and the family window derived from that
    // fact moves with it; that is the correction working, not a leak.)
    svc.cancel_quest(a, false).await.unwrap();
    svc.cancel_quest(a, false).await.unwrap();
    let row = svc.get_quest(a).await.unwrap().unwrap();
    assert!(!row["last_started_at"].is_null());
    assert!(
        !row["family_cooldown_expires_at"].is_null(),
        "family still cooling"
    );
    let families = svc.get_families(true).await.unwrap();
    assert_eq!(families[0]["id"], family["id"]);
    assert!(!families[0]["cooldown_expires_at"].is_null());
}

#[tokio::test]
async fn an_unknown_variant_of_a_known_family_auto_creates_and_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let family = svc
        .create_family(
            &json!({"name": "ARIS - Daily Hunting 1", "planet": "ARIS", "cooldown_hours": 20.0}),
        )
        .await
        .unwrap();

    svc.start_quest_from_mission("ARIS - Daily Hunting 1: Weak Mortirex (Repeatable)")
        .await
        .unwrap();
    let quests = svc.get_quests(true).await.unwrap();
    assert_eq!(quests.len(), 1);
    let created = &quests[0];
    assert_eq!(
        created["name"],
        json!("ARIS - Daily Hunting 1: Weak Mortirex"),
        "named as the line reads, repeatable suffix stripped"
    );
    assert_eq!(
        created["planet"],
        json!("ARIS"),
        "inherits the family planet"
    );
    assert_eq!(created["family_id"], family["id"]);
    assert!(
        json_truthy(created.get("started_at")),
        "starts in the same motion"
    );

    // The second encounter is an exact match: no duplicate row.
    svc.start_quest_from_mission("ARIS - Daily Hunting 1: Weak Mortirex")
        .await
        .unwrap();
    assert_eq!(svc.get_quests(true).await.unwrap().len(), 1);

    // A line matching no quest and no family stays ignored, and the
    // bare umbrella line (no variant part) never creates a quest.
    svc.start_quest_from_mission("Some Other Mission")
        .await
        .unwrap();
    svc.start_quest_from_mission("ARIS - Daily Hunting 1: ")
        .await
        .unwrap();
    assert_eq!(svc.get_quests(true).await.unwrap().len(), 1);
    let quest_rows = count_rows(&db, "SELECT COUNT(*) FROM quests").await;
    assert_eq!(quest_rows, 1);
}

#[tokio::test]
async fn updating_a_soft_deleted_family_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let family = svc
        .create_family(&json!({"name": "Daily Hunting 1", "cooldown_hours": 20.0}))
        .await
        .unwrap();
    let fid = family["id"].as_i64().unwrap();
    let member = quest_id(
        &svc.create_quest(&json!({"name": "Daily Hunting 1: Weak Mortirex"}))
            .await
            .unwrap(),
    );

    assert!(svc.delete_family(fid).await.unwrap());
    // A soft-deleted family reads as absent: no mutation, and above all
    // no rename sweep re-pointing active quests at a dead family (the
    // delete detached them precisely to prevent that).
    assert_eq!(
        svc.update_family(fid, &json!({"name": "Daily Hunting 1"}))
            .await
            .unwrap(),
        None
    );
    let row = svc.get_quest(member).await.unwrap().unwrap();
    assert_eq!(row["family_id"], Value::Null, "the detach stands");
}
