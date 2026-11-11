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
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

use crate::common;

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
        handles.sale_capture,
        handles.quests.clone(),
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_overview_zeros_every_numeric_field() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    // Every numeric field is `f64`, so the cycled-breakdown zeros the
    // engine passes through as integers still render as JSON floats.
    let overview = api.analytics_overview("all").await.unwrap();
    assert_eq!(overview.total_return_rate, 0.0);
    assert_eq!(overview.total_gains, 0.0);
    assert_eq!(overview.total_losses, 0.0);
    assert!(overview.timeline.is_empty());
    assert!(overview.monthly_breakdown.is_empty());
    assert_eq!(overview.returns_breakdown.loot_tt, 0.0);
    assert_eq!(overview.losses_breakdown.tracking_cost, 0.0);
    assert_eq!(overview.losses_breakdown.cycled_breakdown.weapon, 0.0);
    assert!(serde_json::to_value(&overview).unwrap()["totalReturnRate"].is_f64());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cycled_breakdown_lines_sum_to_cycled() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    // One ended session carrying every cycled family: weapon 1.0,
    // enhancer 0.1, harvest decay 0.5, armour 0.4, healing 0.3,
    // consumables 0.2, and dangling 0.05.
    db.with_writer(|connection| {
        connection.execute_batch(
            "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, armour_cost, heal_cost, \
                  consumable_cost, dangling_cost) \
             VALUES ('mixed', 1000.0, 2000.0, 0, 0.4, 0.3, 0.2, 0.05); \
             INSERT INTO kills \
                 (id, session_id, timestamp, mob_name, mob_species, mob_maturity, \
                  cost_ped, enhancer_cost, loot_total_ped) \
             VALUES ('kill', 'mixed', 1500.0, 'Atrox Young', 'Atrox', 'Young', \
                     1.0, 0.1, 2.0); \
             INSERT INTO kill_tool_stats \
                 (kill_id, tool_name, shots_fired, cost_per_shot) \
             VALUES ('kill', 'Test Rifle', 10, 0.1); \
             INSERT INTO harvest_events \
                 (id, session_id, timestamp, success, tool_name, yield_tier, cost_ped, \
                  loot_total_ped) \
             VALUES ('swing', 'mixed', 1600.0, 1, 'Test Cutter', 'long', 0.5, 0.4);",
        )?;
        eo_services::session_rollup::heal(connection)?;
        eo_services::session_summary::write_session_summary(connection, "mixed")?;
        Ok(())
    })
    .await
    .expect("mixed-cost fixture");

    let api = analytics_api_with_db(dir.path(), db).await;
    let losses = api
        .analytics_overview("all")
        .await
        .unwrap()
        .losses_breakdown;
    let lines = &losses.cycled_breakdown;
    assert_eq!(lines.harvest, 0.5);
    let sum = lines.weapon
        + lines.enhancer
        + lines.harvest
        + lines.armour
        + lines.healing
        + lines.consumables
        + lines.dangling;
    assert!((losses.tracking_cost - 2.55).abs() < 1e-9);
    assert!((sum - losses.tracking_cost).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_hunting_has_no_comparisons() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let hunting = api.analytics_hunting().await.unwrap();
    assert!(hunting.mob_comparisons.is_empty());
    assert!(hunting.name_comparisons.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_empty_harvest_has_no_tier_comparisons() {
    let dir = tempfile::tempdir().unwrap();
    let api = analytics_api(dir.path()).await;

    let harvest = api.analytics_harvest("all").await.unwrap();
    assert!(harvest.tier_comparisons.is_empty());
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
            session_definition_id: None,
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
            session_definition_id: None,
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
            session_definition_id: None,
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
            field_sources: None,
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

/// The facade over a fresh database, its sell-queue capture reading
/// `drawn` (a sell window drawn as the client lays it out), with the
/// database handle kept for reading what a listing recorded.
async fn capture_api(
    dir: &Path,
    drawn: Option<&eo_services::game_ui::test_support::Lettering>,
) -> (Api, Db) {
    use eo_services::clock::MockClock;
    use eo_services::game_ui::sale_queue::ItemCatalogue;
    use eo_services::game_ui::test_support::{bundled_profile, window_frame};
    use eo_services::sale_capture::{SaleCaptureProviders, SaleCaptureService};

    let snapshot = dir.join("snapshot");
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let read_at = chrono::NaiveDate::from_ymd_opt(2026, 11, 3)
        .unwrap()
        .and_hms_opt(19, 42, 7)
        .unwrap();
    let sale_capture = match drawn {
        Some(drawn) => {
            let image = drawn.image.clone();
            Arc::new(SaleCaptureService::new(
                SaleCaptureProviders {
                    frame: Arc::new(move |area| window_frame(&image, (0, 0), area)),
                    profile: Some(Arc::new(bundled_profile())),
                    catalogue: Arc::new(ItemCatalogue::new([(
                        "Electric Attack Nanochip 3".to_string(),
                        Some(39.0),
                    )])),
                    read_text: Some(Arc::new(drawn.recogniser())),
                },
                Arc::new(MockClock::new(Some(read_at), 0.0)),
            ))
        }
        None => handles.sale_capture,
    };
    let api = Api::new(
        db.clone(),
        Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store")),
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
        sale_capture,
        handles.quests.clone(),
        None,
        None,
        None,
    );
    (api, db)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sell_queue_capture_answers_a_draft_per_queued_item() {
    use eo_services::game_ui::test_support::{sale_window, standard_sale_layout};

    let dir = tempfile::tempdir().unwrap();
    let drawn = sale_window(&standard_sale_layout());
    let (api, _db) = capture_api(dir.path(), Some(&drawn)).await;
    let capture = serde_json::to_value(api.inventory_sale_window_capture().unwrap()).unwrap();
    assert_eq!(capture["kind"], "read");
    assert_eq!(capture["readAt"], "2026-11-03T19:42:07");
    assert_eq!(capture["queued"], serde_json::json!({"kind": "all"}));
    assert_eq!(capture["unread"], serde_json::json!([]));
    let drafts = capture["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 3);
    let id = capture["captureId"].as_i64().unwrap();
    assert_eq!(drafts[0]["draftId"], format!("{id}-0"));
    assert_eq!(
        drafts[0]["itemName"],
        serde_json::json!({"text": "Electric Attack Nanochip 3", "found": "itemData"})
    );
    assert_eq!(
        drafts[0]["startingBid"],
        serde_json::json!({"value": 162.0, "text": "162", "check": false})
    );
    assert_eq!(
        drafts[0]["ttValue"],
        serde_json::json!({"value": 11.52, "text": "11.52", "check": true})
    );
    assert_eq!(drafts[0]["buyout"]["figure"]["text"], "172");
    assert_eq!(drafts[1]["buyout"], serde_json::json!({"kind": "off"}));
    assert_eq!(
        drafts[2]["itemName"],
        serde_json::json!({"text": "Skyripper Loyalist (L)", "found": "nowhere"})
    );
    // The form's own capture is answered to it, not held for collection.
    assert_eq!(
        serde_json::to_value(api.inventory_sale_window_take_capture()).unwrap(),
        serde_json::Value::Null
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_with_no_reader_is_refused_and_carries_no_draft() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = capture_api(dir.path(), None).await;
    let capture = serde_json::to_value(api.inventory_sale_window_capture().unwrap()).unwrap();
    assert_eq!(capture["kind"], "refused");
    assert_eq!(capture["reason"], "readerUnavailable");
    assert_eq!(capture.get("drafts"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_overlay_capture_waits_once_for_the_form_refusals_included() {
    use eo_api::sale_capture::SaleQueueCapture;

    let dir = tempfile::tempdir().unwrap();
    let (api, _db) = capture_api(dir.path(), None).await;
    let first = api.inventory_sale_window_capture_for_shell();
    let second = api.inventory_sale_window_capture_for_shell();
    let id = |capture: &SaleQueueCapture| match capture {
        SaleQueueCapture::Read { capture_id, .. }
        | SaleQueueCapture::Refused { capture_id, .. } => *capture_id,
    };
    assert!(id(&second) > id(&first));
    // The newer capture is the one waiting, a refusal as much as a read:
    // the form drops older drafts on it.
    let waiting = api.inventory_sale_window_take_capture().0.expect("held");
    assert_eq!(waiting, second);
    assert!(
        api.inventory_sale_window_take_capture().0.is_none(),
        "taken once"
    );
}

/// Where each figure of `listing_id` came from, as recorded.
async fn recorded_sources(db: &Db, listing_id: &str) -> Vec<(String, String, Option<String>)> {
    let id = listing_id.to_string();
    db.with_reader(move |conn| {
        let mut statement = conn.prepare(
            "SELECT field, source, read_text FROM auction_listing_field_sources \
             WHERE listing_id = ? ORDER BY field",
        )?;
        let rows = statement
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listing_drafted_from_a_capture_records_where_each_figure_came_from() {
    use eo_api::analytics::{EquipmentListingInput, InventoryItemInput};

    let dir = tempfile::tempdir().unwrap();
    let (api, db) = capture_api(dir.path(), None).await;
    let loot: AuctionListingInput = serde_json::from_value(serde_json::json!({
        "profession": "inventory",
        "itemName": "Output Amplifier Component",
        "quantity": 40.0,
        "startingBid": 42.0,
        "listingFee": 1.54,
        "listedAt": "2026-11-03T19:42:07",
        "auctionDays": 7,
        "fieldSources": [
            {"field": "itemName", "source": {"kind": "readUnchecked", "text": "Output Amplifier Component"}},
            {"field": "quantity", "source": {"kind": "typed"}},
            {"field": "startingBid", "source": {"kind": "readChecked", "text": "42"}},
            {"field": "buyout", "source": {"kind": "readChecked"}},
        ],
    }))
    .unwrap();
    let listing = api.auction_listing_create(loot).await.unwrap();
    assert_eq!(
        recorded_sources(&db, &listing.id).await,
        vec![
            ("buyout".into(), "read_checked".into(), None),
            (
                "item_name".into(),
                "read_unchecked".into(),
                Some("Output Amplifier Component".into())
            ),
            ("quantity".into(), "typed".into(), None),
            (
                "starting_bid".into(),
                "read_checked".into(),
                Some("42".into())
            ),
        ]
    );

    let asset = api
        .inventory_create(InventoryItemInput {
            name: "Electric Attack Nanochip 3".into(),
            tt_value: 11.52,
            markup_paid: 0.0,
            notes: None,
            acquired_at: None,
        })
        .await
        .unwrap();
    let equipment: EquipmentListingInput = serde_json::from_value(serde_json::json!({
        "itemId": asset.id,
        "startingBid": 162.0,
        "buyout": 172.0,
        "listingFee": 7.33,
        "fieldSources": [
            {"field": "itemName", "source": {"kind": "matched", "text": "Electric Attack Nanochip III"}},
            {"field": "listingFee", "source": {"kind": "readUnchecked", "text": "7.33"}},
            {"field": "ttValue", "source": {"kind": "readUnchecked", "text": "11.52"}},
        ],
    }))
    .unwrap();
    let listed = api
        .inventory_equipment_listing_create(equipment)
        .await
        .unwrap();
    assert_eq!(
        recorded_sources(&db, &listed.id).await,
        vec![
            (
                "item_name".into(),
                "matched".into(),
                Some("Electric Attack Nanochip III".into())
            ),
            (
                "listing_fee".into(),
                "read_unchecked".into(),
                Some("7.33".into())
            ),
            (
                "tt_value".into(),
                "read_unchecked".into(),
                Some("11.52".into())
            ),
        ]
    );

    // An asset's name is known only once its holding is read: a name read
    // and kept must be that name, or nothing is listed.
    let other = api
        .inventory_create(InventoryItemInput {
            name: "Output Amplifier Component".into(),
            tt_value: 20.8,
            markup_paid: 0.0,
            notes: None,
            acquired_at: None,
        })
        .await
        .unwrap();
    let misnamed: EquipmentListingInput = serde_json::from_value(serde_json::json!({
        "itemId": other.id,
        "startingBid": 42.0,
        "listingFee": 1.54,
        "fieldSources": [
            {"field": "itemName", "source": {"kind": "readChecked", "text": "Animal Hide"}},
        ],
    }))
    .unwrap();
    assert_eq!(
        api.inventory_equipment_listing_create(misnamed)
            .await
            .unwrap_err(),
        ApiError::bad_request("a name read and kept is the name recorded")
    );

    // A figure named twice is the caller's fault, said so, and nothing is
    // listed.
    let twice: AuctionListingInput = serde_json::from_value(serde_json::json!({
        "profession": "inventory",
        "itemName": "Output Amplifier Component",
        "quantity": 1.0,
        "startingBid": 42.0,
        "listingFee": 1.54,
        "fieldSources": [
            {"field": "quantity", "source": {"kind": "typed"}},
            {"field": "quantity", "source": {"kind": "readChecked", "text": "1"}},
        ],
    }))
    .unwrap();
    assert_eq!(
        api.auction_listing_create(twice).await.unwrap_err(),
        ApiError::bad_request("a listing figure's source is given twice")
    );
}
