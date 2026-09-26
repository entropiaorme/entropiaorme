//! Behavioural pins for the analytics family over the typed facade: the
//! Overview / Activity aggregates bridged into their DTOs, the ledger
//! keyset page (the cursor folded into the return), the ledger / preset /
//! inventory CRUD, and the not-found / bad-request legs.
//!
//! The computation itself is pinned in `eo_services::analytics`; these
//! tests cover the facade's own contract: the DTO serialisation (the wire
//! bytes a typed command answers) and the service-error-to-`ApiError`
//! mapping. The one ratified movement is asserted here: the empty
//! Overview's `cycledBreakdown` zeros render as JSON floats (`0.0`) under
//! the `f64` DTO, where the transport passed the engine integer (`0`)
//! through its `Any`-typed field.

use std::path::Path;
use std::sync::Arc;

use eo_api::analytics::{
    AuctionConfirmInput, AuctionExpireInput, AuctionListingInput, InventorySellInput,
    LedgerEntryInput, LedgerPresetInput, Profession,
};
use eo_api::market::MarketHorizon;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot (analytics is catalogue-independent).
async fn analytics_api(dir: &Path) -> Api {
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    analytics_api_with_db(dir, db).await
}

async fn analytics_api_with_db(dir: &Path, db: Db) -> Api {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
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
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_hunting_facades_cannot_read_raw_fact_tables() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open_with_denied_reader_tables(
        &data_dir.join("entropia_orme.db"),
        &["kills", "kill_loot_items", "skill_gains"],
    )
    .await
    .expect("guarded database");
    db.with_writer(|connection| {
        connection.execute_batch(
            "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active) \
             VALUES ('settled', 1000.0, 2000.0, 0); \
             INSERT INTO kills \
                 (id, session_id, timestamp, mob_name, mob_species, mob_maturity, \
                  cost_ped, enhancer_cost, loot_total_ped) \
             VALUES ('kill', 'settled', 1500.0, 'Atrox Young', 'Atrox', 'Young', \
                     1.0, 0.1, 2.0); \
             INSERT INTO kill_loot_items \
                 (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) \
             VALUES ('kill', 'Animal Hide', 2, 2.0, 0); \
             INSERT INTO kill_tool_stats \
                 (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES ('kill', 'Test Rifle', 10, 0.1); \
             INSERT INTO skill_gains \
                 (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('settled', 1600.0, 'Rifle', 0.1, 0.01);",
        )?;
        eo_services::session_rollup::heal(connection)?;
        eo_services::session_summary::write_session_summary(connection, "settled")?;
        Ok(())
    })
    .await
    .expect("settled fixture");

    let api = analytics_api_with_db(dir.path(), db).await;
    assert_eq!(
        api.analytics_hunting_activity("all")
            .await
            .unwrap()
            .overall
            .returns,
        2.0
    );
    assert_eq!(
        api.activity_stock(Profession::Hunting).await.unwrap()[0].item_name,
        "Animal Hide"
    );
    assert_eq!(api.market_hunt_markups().await.unwrap().items.len(), 1);
    assert_eq!(
        api.market_mob_ranking(MarketHorizon::Week).await.unwrap()[0].mob_species,
        "Atrox"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_overview_serialises_to_the_float_typed_zeros() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    // Transport invariance (with the ratified movement): every numeric
    // field is `f64`, so the `cycledBreakdown` zeros the transport passed
    // through as engine integers now render as JSON floats.
    let overview = api.analytics_overview("all").await.unwrap();
    assert_eq!(
        serde_json::to_string(&overview).unwrap(),
        "{\"totalReturnRate\":0.0,\"trend\":\"stable\",\"returnsBreakdown\":{\"lootTt\":0.0,\
         \"questItemTt\":0.0,\"pes\":0.0,\"codexPes\":0.0,\"questPes\":0.0,\"ledger\":{}},\"lossesBreakdown\":\
         {\"trackingCost\":0.0,\"cycledBreakdown\":{\"weapon\":0.0,\"healing\":0.0,\
         \"enhancer\":0.0,\"armour\":0.0,\"dangling\":0.0,\"consumables\":0.0},\"ledger\":{}},\"totalGains\":0.0,\
         \"totalLosses\":0.0,\"timeline\":[],\"monthlyBreakdown\":[]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_hunting_serialises_to_two_empty_tables() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let hunting = api.analytics_hunting().await.unwrap();
    assert_eq!(
        serde_json::to_string(&hunting).unwrap(),
        "{\"mobComparisons\":[],\"nameComparisons\":[]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_harvest_serialises_to_an_empty_tier_table() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let harvest = api.analytics_harvest("all").await.unwrap();
    assert_eq!(
        serde_json::to_string(&harvest).unwrap(),
        "{\"tierComparisons\":[]}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ledger_create_reads_back_the_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let created = api
        .ledger_create(LedgerEntryInput {
            date: "2026-05-01".to_string(),
            kind: "expense".to_string(),
            description: "Ammo".to_string(),
            amount: 12.5,
            tag: "ammo".to_string(),
        })
        .await
        .unwrap();
    // Transport invariance: the `type` key stays `type`, its serialised
    // form stays the plain string, the amount coerces to its float form,
    // and the generated id rides along.
    assert_eq!(
        serde_json::to_value(created.kind).unwrap(),
        serde_json::json!("expense")
    );
    assert_eq!(created.date, "2026-05-01");
    assert_eq!(created.amount, 12.5);
    assert!(!created.id.is_empty());
    let created_bytes = serde_json::to_string(&created).unwrap();
    assert!(
        created_bytes.contains("\"type\":\"expense\"") && created_bytes.contains("\"amount\":12.5"),
        "wire shape: {created_bytes}"
    );

    // The page reads it back, the cursor folded into the return DTO.
    let page = api.ledger_list(None, None).await.unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].description, "Ammo");
    assert_eq!(page.entries[0].id, created.id);
    assert_eq!(page.next_cursor, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ledger_page_carries_the_next_cursor_when_more_remain() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    for day in ["01", "02", "03"] {
        api.ledger_create(LedgerEntryInput {
            date: format!("2026-05-{day}"),
            kind: "expense".to_string(),
            description: format!("e{day}"),
            amount: 1.0,
            tag: "t".to_string(),
        })
        .await
        .unwrap();
    }

    // A bounded first page carries a cursor; walking it to exhaustion
    // yields every entry once, newest first.
    let first = api.ledger_list(None, Some(2)).await.unwrap();
    assert_eq!(first.entries.len(), 2);
    assert_eq!(
        first.total, 3,
        "the count spans the whole table, not the page"
    );
    let cursor = first.next_cursor.0.clone().expect("a further page");
    let second = api.ledger_list(Some(cursor), Some(2)).await.unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.next_cursor, None);

    let seen: Vec<&str> = first
        .entries
        .iter()
        .chain(second.entries.iter())
        .map(|e| e.description.as_str())
        .collect();
    assert_eq!(seen, ["e03", "e02", "e01"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ledger_summary_counts_every_entry_beyond_the_page_window() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    // Five entries across two types and three tags, then a page smaller
    // than the table: the summary must aggregate the WHOLE ledger, not the
    // loaded page window.
    let entries = [
        ("2026-05-01", "expense", "ammo", 10.0),
        ("2026-05-02", "expense", "ammo", 5.0),
        ("2026-05-03", "expense", "equipment", 100.0),
        ("2026-05-04", "markup", "item_sale", 40.0),
        ("2026-05-05", "markup", "quest_reward", 2.5),
    ];
    for (date, kind, tag, amount) in entries {
        api.ledger_create(LedgerEntryInput {
            date: date.to_string(),
            kind: kind.to_string(),
            description: format!("{tag} {date}"),
            amount,
            tag: tag.to_string(),
        })
        .await
        .unwrap();
    }
    let page = api.ledger_list(None, Some(2)).await.unwrap();
    assert_eq!(page.entries.len(), 2, "the page window is a slice");

    let summary = api.ledger_summary("all").await.unwrap();
    assert_eq!(summary.losses.get("ammo"), Some(&15.0));
    assert_eq!(summary.losses.get("equipment"), Some(&100.0));
    assert_eq!(summary.gains.get("item_sale"), Some(&40.0));
    assert_eq!(summary.gains.get("quest_reward"), Some(&2.5));
    assert_eq!(summary.losses.len(), 2);
    assert_eq!(summary.gains.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_cursor_is_a_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    assert_eq!(
        api.ledger_list(Some("not a cursor!".to_string()), None)
            .await
            .unwrap_err(),
        ApiError::bad_request("Invalid cursor")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_positive_listing_quantity_is_a_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    // Listing nothing is not a no-op that should quietly succeed: it would
    // write a fee-bearing listing that can never resolve to anything.
    assert_eq!(
        api.auction_listing_create(AuctionListingInput {
            profession: Profession::Harvesting,
            item_name: "Long Moonleaf Board".to_string(),
            quantity: 0.0,
            starting_bid: 1.0,
            buyout: None,
            listing_fee: 0.5,
            listed_at: None,
            auction_days: None,
        })
        .await
        .unwrap_err(),
        ApiError::bad_request("a listing needs a positive quantity")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolving_an_unknown_listing_is_a_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    assert_eq!(
        api.auction_listing_confirm(AuctionConfirmInput {
            listing_id: "no-such-listing".to_string(),
            final_price: 5.0,
            sale_fee: 0.0,
            resolved_at: None,
        })
        .await
        .unwrap_err(),
        ApiError::not_found("no unresolved listing with that id")
    );
    assert_eq!(
        api.auction_listing_expire(AuctionExpireInput {
            listing_id: "no-such-listing".to_string(),
            resolved_at: None,
        })
        .await
        .unwrap_err(),
        ApiError::not_found("no unresolved listing with that id")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_preset_type_is_a_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    assert_eq!(
        api.ledger_preset_create(LedgerPresetInput {
            name: "Bad".to_string(),
            kind: "income".to_string(),
            description: "d".to_string(),
            amount: 1.0,
            tag: "t".to_string(),
        })
        .await
        .unwrap_err(),
        ApiError::bad_request("type must be 'expense' or 'markup'")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_not_found_legs_answer_the_typed_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    assert_eq!(
        api.ledger_delete("missing".to_string()).await.unwrap_err(),
        ApiError::not_found("Entry not found")
    );
    assert_eq!(
        api.ledger_preset_delete("missing".to_string())
            .await
            .unwrap_err(),
        ApiError::not_found("Preset not found")
    );
    assert_eq!(
        api.inventory_delete("missing".to_string())
            .await
            .unwrap_err(),
        ApiError::not_found("Inventory item not found")
    );
    assert_eq!(
        api.inventory_sell(
            "missing".to_string(),
            InventorySellInput {
                sale_price: 1.0,
                description: None,
                sold_at: None,
            },
        )
        .await
        .unwrap_err(),
        ApiError::not_found("Inventory item not found")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_profit_sale_emits_a_markup_ledger_entry() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let item = api
        .inventory_create(eo_api::analytics::InventoryItemInput {
            name: "Sword".to_string(),
            tt_value: 10.0,
            markup_paid: 2.0,
            notes: None,
            acquired_at: Some("2026-02-01".to_string()),
        })
        .await
        .unwrap();
    let sold = api
        .inventory_sell(
            item.id.clone(),
            InventorySellInput {
                sale_price: 20.0,
                description: None,
                sold_at: Some("2026-05-10".to_string()),
            },
        )
        .await
        .unwrap();
    let entry = sold
        .ledger_entry
        .0
        .expect("a profit sale emits a ledger row");
    assert_eq!(
        serde_json::to_value(entry.kind).unwrap(),
        serde_json::json!("markup")
    );
    assert_eq!(entry.amount, 8.0);
    assert_eq!(entry.tag, "inventory_sale");
    assert_eq!(entry.description, "Inventory Sale: Sword");
    assert_eq!(sold.sold_item.name, "Sword");
    // The sold item leaves current holdings; the emitted ledger row is the only one.
    assert!(api.inventory_list().await.unwrap().is_empty());
    assert_eq!(api.ledger_list(None, None).await.unwrap().entries.len(), 1);
}
