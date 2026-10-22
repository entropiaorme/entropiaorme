//! The curated catalogue, its sync, abandoning a run, and settling a reward
//! the chat log split from its completion.

use std::sync::Arc;

use rusqlite::params;
use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::db::Db;

use super::catalogue::{apply_catalogue, Catalogue, CatalogueEntry, SyncReport, CATALOGUE};
use super::{QuestError, QuestService};

async fn rig(dir: &std::path::Path) -> (Arc<QuestService>, Db, Arc<crate::clock::MockClock>) {
    let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        Some(
            chrono::NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        ),
        0.0,
    ));
    let bus = Arc::new(crate::event_bus::EventBus::new());
    let svc = QuestService::start(&bus, db.clone(), clock.clone(), Handle::current());
    (svc, db, clock)
}

/// A small catalogue in the shipped shape: one rotating family with an
/// aliased variant, one signal boss, and a plain mission on a second planet.
fn fixture() -> Catalogue {
    serde_json::from_value(json!({
        "version": 1,
        "planets": [
            {"key": "aris", "name": "ARIS", "categories": [
                {"key": "aris/daily", "name": "Daily missions", "entries": [
                    {"kind": "family", "key": "aris/h1", "name": "ARIS - Daily Hunting 1",
                     "label": "Hunting 1", "cooldownHours": 20, "cooldownAnchor": "pickup",
                     "reward": {"policy": "named_items", "items": ["Hyperion Daily Voucher"],
                                "label": "Hyperion voucher"},
                     "members": [
                        {"key": "aris/h1/mortirex", "name": "ARIS - Daily Hunting 1: Weak Mortirex",
                         "waypoint": "/wp [Aris, 1, 2, 3, Weak Mortirex]"},
                        {"key": "aris/h1/drones", "name": "ARIS - Daily Hunting 1: Defective Drones"}
                     ]}
                ]},
                {"key": "aris/bosses", "name": "Bosses", "entries": [
                    {"kind": "quest", "key": "aris/droktimus", "name": "ARIS - Robot Boss 1: Droktimus Prime",
                     "label": "Droktimus Prime", "detail": "Robot boss",
                     "completion": "signal_item", "signalItem": "Hyperion Daily Voucher",
                     "cooldownHours": 20, "cooldownAnchor": "completion",
                     "reward": {"policy": "named_items", "items": ["Hyperion Daily Voucher"],
                                "label": "Hyperion voucher"},
                     "aliases": ["ARIS - Robot Boss 1: Drokitmus Prime"]}
                ]}
            ]},
            {"key": "cyrene", "name": "Cyrene", "categories": [
                {"key": "cyrene/tempo", "name": "Tempo", "entries": [
                    {"kind": "quest", "key": "cyrene/birds", "name": "Zyn'Nix Tempo - Adult Horned Birds",
                     "completion": "mission_log", "cooldownHours": 21, "cooldownAnchor": "completion",
                     "reward": {"policy": "fixed_pes", "amount": 0.1, "label": "0.1 PED Inflict Melee Damage"}}
                ]}
            ]}
        ]
    }))
    .expect("fixture catalogue")
}

async fn sync(db: &Db, catalogue: Catalogue) -> SyncReport {
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        let report = apply_catalogue(&tx, &catalogue, 1_772_366_400.0)?;
        tx.commit()?;
        Ok(report)
    })
    .await
    .unwrap()
}

async fn quest_row(db: &Db, key: &'static str) -> Value {
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT id, name, planet, category, cooldown_hours, cooldown_anchor, completion_mode, \
                    signal_loot_item, reward_policy, reward_ped, family_id, is_active, waypoint \
             FROM quests WHERE catalogue_key = ?",
            params![key],
            |row| {
                Ok(json!({
                    "id": row.get::<_, i64>(0)?,
                    "name": row.get::<_, String>(1)?,
                    "planet": row.get::<_, String>(2)?,
                    "category": row.get::<_, Option<String>>(3)?,
                    "cooldown_hours": row.get::<_, Option<f64>>(4)?,
                    "cooldown_anchor": row.get::<_, String>(5)?,
                    "completion_mode": row.get::<_, String>(6)?,
                    "signal": row.get::<_, Option<String>>(7)?,
                    "policy": row.get::<_, String>(8)?,
                    "reward_ped": row.get::<_, Option<f64>>(9)?,
                    "family_id": row.get::<_, Option<i64>>(10)?,
                    "is_active": row.get::<_, i64>(11)?,
                    "waypoint": row.get::<_, Option<String>>(12)?,
                }))
            },
        )?)
    })
    .await
    .unwrap()
}

async fn scalar(db: &Db, sql: &'static str) -> i64 {
    db.with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
        .await
        .unwrap()
}

#[test]
fn the_shipped_catalogue_is_well_formed() {
    CATALOGUE
        .validate()
        .expect("the shipped catalogue validates");
    let planet = |name: &str| {
        CATALOGUE
            .planets
            .iter()
            .find(|planet| planet.name == name)
            .unwrap_or_else(|| panic!("{name} ships"))
    };
    let families = planet("ARIS")
        .categories
        .iter()
        .flat_map(|category| &category.entries)
        .filter(|entry| matches!(entry, CatalogueEntry::Family(_)))
        .count();
    assert_eq!(families, 4, "three hunting slots and the samples slot");
    let cyrene_quests = planet("Cyrene")
        .categories
        .iter()
        .flat_map(|category| &category.entries)
        .count();
    assert_eq!(cyrene_quests, 31);
}

#[test]
fn validation_refuses_a_member_outside_its_family_and_a_repeated_key() {
    let mut catalogue = fixture();
    if let CatalogueEntry::Family(family) = &mut catalogue.planets[0].categories[0].entries[0] {
        family.members[1].name = "ARIS - Daily Hunting 2: Defective Drones".to_string();
    }
    assert!(catalogue
        .validate()
        .unwrap_err()
        .contains("does not belong"));

    let mut catalogue = fixture();
    if let CatalogueEntry::Quest(quest) = &mut catalogue.planets[1].categories[0].entries[0] {
        quest.key = "aris/droktimus".to_string();
    }
    assert!(catalogue.validate().unwrap_err().contains("repeated"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_database_receives_the_catalogue_once() {
    let dir = tempfile::tempdir().unwrap();
    let (_svc, db, _clock) = rig(dir.path()).await;

    let first = sync(&db, fixture()).await;
    assert_eq!(
        first,
        SyncReport {
            inserted: 5,
            ..Default::default()
        },
        "one family and four quests"
    );
    let again = sync(&db, fixture()).await;
    assert_eq!(
        again,
        SyncReport::default(),
        "an unchanged catalogue writes nothing"
    );

    let member = quest_row(&db, "aris/h1/mortirex").await;
    assert_eq!(member["planet"], "ARIS");
    assert_eq!(member["category"], "Daily missions");
    assert!(
        member["cooldown_hours"].is_null(),
        "the family carries the cooldown"
    );
    assert!(member["family_id"].is_i64());
    assert_eq!(member["policy"], "named_items");
    let boss = quest_row(&db, "aris/droktimus").await;
    assert_eq!(boss["completion_mode"], "signal_item");
    assert_eq!(boss["signal"], "Hyperion Daily Voucher");
    let birds = quest_row(&db, "cyrene/birds").await;
    assert_eq!(birds["policy"], "fixed_pes");
    assert_eq!(birds["reward_ped"], 0.1);
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM quest_families \
             WHERE catalogue_key = 'aris/h1' AND cooldown_hours = 20 AND cooldown_anchor = 'pickup'"
        )
        .await,
        1
    );
}

/// The player's own rows are adopted, so their history stays attached: by
/// name (whitespace and case aside), and by alias across a rename the
/// catalogue makes. An active row wins over an inactive duplicate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_rows_are_adopted_with_their_history() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;

    let family = svc
        .create_family(&json!({
            "name": "ARIS - Daily Hunting 1", "planet": "ARIS",
            "cooldown_hours": 21, "cooldown_anchor": "completion",
        }))
        .await
        .unwrap();
    let family_id = family["id"].as_i64().unwrap();
    let variant = svc
        .create_quest(&json!({
            "name": "ARIS - Daily Hunting 1: Weak Mortirex", "planet": "Calypso",
            "cooldown_hours": 24, "cooldown_anchor": "completion",
        }))
        .await
        .unwrap();
    let boss = svc
        .create_quest(&json!({
            "name": "ARIS - Robot Boss 1: Drokitmus Prime",
            "signal_loot_item": "Hyperion Daily Voucher",
        }))
        .await
        .unwrap();
    let stale = svc
        .create_quest(&json!({"name": "Zyn'Nix Tempo - Adult Horned Birds"}))
        .await
        .unwrap();
    svc.delete_quest(stale["id"].as_i64().unwrap())
        .await
        .unwrap();
    let live = svc
        .create_quest(&json!({"name": "Zyn'Nix Tempo - Adult Horned Birds\t"}))
        .await
        .unwrap();
    let boss_id = boss["id"].as_i64().unwrap();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions(session_id, quest_id, completed_at) \
             VALUES ('manual-1', ?, 1772300000.0)",
            params![boss_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let report = sync(&db, fixture()).await;
    assert_eq!(
        report.adopted, 4,
        "the family, a variant, the boss, the tempo"
    );
    assert_eq!(report.inserted, 1, "only the unseen variant is new");

    let adopted_variant = quest_row(&db, "aris/h1/mortirex").await;
    assert_eq!(adopted_variant["id"], variant["id"]);
    assert_eq!(
        adopted_variant["planet"], "ARIS",
        "a misfiled planet is corrected"
    );
    assert!(adopted_variant["cooldown_hours"].is_null());
    assert_eq!(adopted_variant["family_id"], family_id);
    let adopted_boss = quest_row(&db, "aris/droktimus").await;
    assert_eq!(adopted_boss["id"], boss_id);
    assert_eq!(adopted_boss["name"], "ARIS - Robot Boss 1: Droktimus Prime");
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM session_quest_completions c \
             JOIN quests q ON q.id = c.quest_id WHERE q.catalogue_key = 'aris/droktimus'"
        )
        .await,
        1,
        "the completion follows the adopted row"
    );
    let birds = quest_row(&db, "cyrene/birds").await;
    assert_eq!(
        birds["id"], live["id"],
        "the active duplicate is the one adopted"
    );
    assert_eq!(birds["name"], "Zyn'Nix Tempo - Adult Horned Birds");
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM quest_families \
             WHERE id = 1 AND cooldown_hours = 20 AND cooldown_anchor = 'pickup'"
        )
        .await,
        1,
        "the family takes the catalogue's cooldown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curated_rows_are_read_only_and_retire_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    sync(&db, fixture()).await;

    let boss = quest_row(&db, "aris/droktimus").await;
    let boss_id = boss["id"].as_i64().unwrap();
    let quest = svc.get_quest(boss_id).await.unwrap().unwrap();
    assert_eq!(quest["curated"], true);
    assert_eq!(quest["catalogue_key"], "aris/droktimus");
    let refused = svc
        .update_quest(boss_id, &json!({"name": "Renamed"}))
        .await
        .unwrap_err();
    assert!(matches!(refused, QuestError::Invalid(message) if message.contains("maintained")));
    assert!(svc.delete_quest(boss_id).await.is_err());
    let family_id = quest_row(&db, "aris/h1/mortirex").await["family_id"]
        .as_i64()
        .unwrap();
    assert!(svc.delete_family(family_id).await.is_err());
    assert!(svc
        .update_family(family_id, &json!({"cooldown_hours": 4}))
        .await
        .is_err());

    let mut trimmed = fixture();
    trimmed.planets[0].categories.remove(1);
    let report = sync(&db, trimmed).await;
    assert_eq!(report.retired, 1);
    assert_eq!(quest_row(&db, "aris/droktimus").await["is_active"], 0);
}

/// Giving a mission up holds its slot: a completion-anchored timer runs from
/// the abandon, the run ends cancelled with no completion, and a reset
/// forgets the abandon rather than an older completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_run_holds_the_slot_until_reset() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    let quest = svc
        .create_quest(&json!({"name": "Getting Slimed!", "cooldown_hours": 21}))
        .await
        .unwrap();
    let quest_id = quest["id"].as_i64().unwrap();

    let refused = svc.abandon_quest(quest_id).await.unwrap_err();
    assert!(matches!(refused, QuestError::Invalid(message) if message.contains("in progress")));

    svc.start_quest(quest_id).await.unwrap();
    let abandoned = svc.abandon_quest(quest_id).await.unwrap().unwrap();
    assert!(abandoned["started_at"].is_null());
    assert!(abandoned["last_abandoned_at"].is_f64());
    assert!(
        abandoned["cooldown_expires_at"].is_string(),
        "the abandon starts the completion-anchored timer"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM quest_runs \
             WHERE status = 'cancelled' AND abandoned_at IS NOT NULL AND completion_id IS NULL"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM session_quest_completions").await,
        0,
        "nothing was completed"
    );
    let offer = svc
        .quest_offers()
        .await
        .unwrap()
        .into_iter()
        .find(|offer| offer.id == quest_id)
        .unwrap();
    assert!(offer.available_from.is_some(), "the offering agrees");

    let reset = svc.cancel_quest(quest_id, false).await.unwrap().unwrap();
    assert!(reset["cooldown_expires_at"].is_null());
    assert!(reset["last_abandoned_at"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoning_a_daily_keeps_its_family_cooling_from_pickup() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    sync(&db, fixture()).await;
    let variant_id = quest_row(&db, "aris/h1/mortirex").await["id"]
        .as_i64()
        .unwrap();

    svc.start_quest(variant_id).await.unwrap();
    let abandoned = svc.abandon_quest(variant_id).await.unwrap().unwrap();
    assert!(abandoned["started_at"].is_null());
    assert!(abandoned["family_cooldown_expires_at"].is_string());
    let sibling_id = quest_row(&db, "aris/h1/drones").await["id"]
        .as_i64()
        .unwrap();
    let sibling = svc.get_quest(sibling_id).await.unwrap().unwrap();
    assert_eq!(
        sibling["family_cooldown_expires_at"], abandoned["family_cooldown_expires_at"],
        "the whole slot is held"
    );
}

/// Seed one unresolved voucher completion whose voucher arrived as ordinary
/// loot in the same declared activity.
async fn seed_split_voucher(db: &Db, quest_id: i64, with_loot: bool) {
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active) \
             VALUES('s-split', 1772366300.0, 1772366500.0, 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO session_contexts(id, session_id, created_at) \
             VALUES(901, 's-split', 1772366300.0)",
            [],
        )?;
        if with_loot {
            conn.execute(
                "INSERT INTO kills(id, loot_source_id, session_id, mob_name, timestamp, \
                                   context_id, loot_total_ped) \
                 VALUES('k-split', 'split-source', 's-split', 'Weak Mortirex', 1772366399.0, \
                        901, 0.5)",
                [],
            )?;
            conn.execute(
                "INSERT INTO kill_loot_items(kill_id, item_name, quantity, value_ped) \
                 VALUES('k-split', 'Hyperion Daily Voucher', 1, 0.0), \
                       ('k-split', 'Shrapnel', 5000, 0.5)",
                [],
            )?;
        }
        conn.execute(
            "INSERT INTO session_quest_completions \
             (id, session_id, quest_id, completed_at, activity_context_id, reward_outcome, \
              reward_policy_snapshot, reward_unresolved_reason, reward_evidence_json) \
             VALUES(902, 's-split', ?, 1772366400.0, 901, 'unresolved', 'named_items', \
                    'One or more expected reward items were missing', \
                    '{\"loot\":[],\"skills\":[],\"isolated\":true}')",
            params![quest_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_split_voucher_settles_with_its_exact_loot_line() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    sync(&db, fixture()).await;
    let quest_id = quest_row(&db, "aris/h1/mortirex").await["id"]
        .as_i64()
        .unwrap();
    seed_split_voucher(&db, quest_id, true).await;

    let open = svc.unresolved_reward_reviews().await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["isolated"], true);
    assert_eq!(open[0]["declarable"], true);
    assert_eq!(open[0]["expected_items"], json!(["Hyperion Daily Voucher"]));
    let candidates = open[0]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "only the named item is a candidate");
    assert_eq!(candidates[0]["mob_name"], "Weak Mortirex");
    let line = candidates[0]["id"].as_i64().unwrap();

    let refused = svc.declare_reward_paid(902).await.unwrap_err();
    assert!(
        matches!(refused, QuestError::Invalid(message) if message.contains("recorded loot line")),
        "declaring beside a recorded line would count the voucher twice"
    );
    assert!(svc.settle_reward_with_loot(902, &[line + 1]).await.is_err());

    svc.settle_reward_with_loot(902, &[line]).await.unwrap();
    assert!(svc.unresolved_reward_reviews().await.unwrap().is_empty());
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM session_quest_completion_reward_items \
             WHERE completion_id = 902 AND item_name = 'Hyperion Daily Voucher' \
               AND evidence_basis = 'observed'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM kill_loot_items \
             WHERE item_name = 'Hyperion Daily Voucher' AND deactivated_at IS NULL"
        )
        .await,
        0,
        "the voucher left ordinary loot"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM quest_reward_reviews WHERE completion_id = 902 AND basis = 'observed'"
        )
        .await,
        1
    );
    assert!(svc.settle_reward_with_loot(902, &[line]).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unevidenced_named_reward_is_declared_paid_and_stays_marked() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    sync(&db, fixture()).await;
    let quest_id = quest_row(&db, "aris/h1/mortirex").await["id"]
        .as_i64()
        .unwrap();
    seed_split_voucher(&db, quest_id, false).await;

    let open = svc.unresolved_reward_reviews().await.unwrap();
    assert!(open[0]["candidates"].as_array().unwrap().is_empty());
    svc.declare_reward_paid(902).await.unwrap();
    assert!(svc.unresolved_reward_reviews().await.unwrap().is_empty());
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM session_quest_completion_reward_items \
             WHERE completion_id = 902 AND evidence_basis = 'declared' AND value_ped = 0"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM quest_reward_reviews WHERE completion_id = 902 AND basis = 'declared'"
        )
        .await,
        1
    );
    assert!(svc.declare_reward_paid(902).await.is_err(), "settled once");
}

/// A completion with no recorded evidence names no tick shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completion_without_evidence_names_no_tick_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db, _clock) = rig(dir.path()).await;
    let quest = svc
        .create_quest(&json!({"name": "AI Daily: Boris", "reward_policy": "completion_clump"}))
        .await
        .unwrap();
    let quest_id = quest["id"].as_i64().unwrap();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO session_quest_completions \
             (id, session_id, quest_id, completed_at, reward_outcome, reward_policy_snapshot, \
              reward_unresolved_reason) \
             VALUES(903, 'manual-x', ?, 1772366400.0, 'unresolved', 'completion_clump', \
                    'Historical completion')",
            params![quest_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let open = svc.unresolved_reward_reviews().await.unwrap();
    assert!(open[0]["isolated"].is_null());
    assert_eq!(open[0]["declarable"], false);
    assert!(svc.declare_reward_paid(903).await.is_err());
}
