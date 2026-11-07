//! Behavioural pins for the player's own market entries over the typed
//! facade: a manual markup's input shapes and refusals, the by-name lookup
//! that carries it beside the market reading, the live Nanocube mark, and
//! the change signal every write announces.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use eo_api::market::{MarketManualMarkupInput, MarketManualMarkupKind};
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database, with its market
/// change signal counted.
async fn make_api(dir: &Path) -> (Api, Arc<AtomicUsize>) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let announced = Arc::new(AtomicUsize::new(0));
    let counter = announced.clone();
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
    )
    .with_market_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    (api, announced)
}

fn entry(item: &str, kind: MarketManualMarkupKind, value: f64) -> MarketManualMarkupInput {
    MarketManualMarkupInput {
        item_name: item.to_string(),
        kind,
        value,
        unit_tt_ped: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manual_markup_rides_the_by_name_lookup_and_clears_back() {
    let dir = tempfile::tempdir().unwrap();
    let (api, announced) = make_api(dir.path()).await;

    let saved = api
        .market_manual_markup_set(MarketManualMarkupInput {
            unit_tt_ped: Some(4.0),
            ..entry("Robot Component", MarketManualMarkupKind::PedPerUnit, 1.0)
        })
        .await
        .unwrap();
    let saved = serde_json::to_value(saved).unwrap();
    assert_eq!(saved["kind"], "ped_per_unit");
    assert_eq!(saved["markupPct"], 125.0);
    assert_eq!(announced.load(Ordering::SeqCst), 1);

    let data = api
        .market_item_markups(vec!["Robot Component".to_string()])
        .await
        .unwrap();
    let item = serde_json::to_value(&data.items[0]).unwrap();
    assert_eq!(item["itemName"], "Robot Component");
    assert_eq!(item["preferred"], "manual");
    assert_eq!(item["markupPct"], serde_json::Value::Null);
    assert_eq!(item["manual"]["value"], 1.0);
    assert_eq!(item["nanocubeMarked"], false);

    assert!(api
        .market_manual_markup_clear("Robot Component".to_string())
        .await
        .unwrap());
    let data = api
        .market_item_markups(vec!["Robot Component".to_string()])
        .await
        .unwrap();
    let item = serde_json::to_value(&data.items[0]).unwrap();
    assert_eq!(item["manual"], serde_json::Value::Null);
    assert_eq!(item["preferred"], serde_json::Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_nanocube_mark_is_a_toggle_that_shrapnel_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let (api, announced) = make_api(dir.path()).await;
    assert!(api
        .market_nanocube_mark_set("Animal Hide".to_string(), true)
        .await
        .unwrap());
    let data = api
        .market_item_markups(vec!["Animal Hide".to_string()])
        .await
        .unwrap();
    assert!(data.items[0].nanocube_marked);
    assert!(!api
        .market_nanocube_mark_set("Animal Hide".to_string(), false)
        .await
        .unwrap());
    assert_eq!(announced.load(Ordering::SeqCst), 2);

    let refused = api
        .market_nanocube_mark_set("Shrapnel".to_string(), true)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(&refused).unwrap()["kind"], "badRequest");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn values_that_are_not_markups_are_bad_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (api, announced) = make_api(dir.path()).await;
    for input in [
        entry("Item", MarketManualMarkupKind::Percent, 0.0),
        entry("Item", MarketManualMarkupKind::PedPerUnit, -1.0),
        entry(" ", MarketManualMarkupKind::Percent, 120.0),
    ] {
        let error: ApiError = api.market_manual_markup_set(input).await.unwrap_err();
        assert_eq!(serde_json::to_value(&error).unwrap()["kind"], "badRequest");
    }
    assert_eq!(announced.load(Ordering::SeqCst), 0);
}
