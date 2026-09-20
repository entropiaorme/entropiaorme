//! Behavioural pins for the maps family over the typed facade: the
//! bundled catalogue read (with and without the bundle composed), the
//! pin CRUD roundtrip with its DTO wire shape, the map-bounds gate on
//! create and move, the patch's double-option null semantics, and the
//! not-found / bad-request legs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_api::maps::{MapPinInput, MapPinPatch};
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::coord_capture::{CoordCaptureProviders, CoordCaptureService, CoordRegion};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::planet_maps::PlanetMapStore;
use eo_services::skill_panel::BgrImage;

mod common;

/// The repo's bundled maps directory (the dev-layout resolution).
fn bundled_maps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("entropia-orme")
        .join("resources")
        .join("maps")
}

/// A coordinate-capture service whose seams read a fixed OCR text.
fn coord_service(text: &'static str) -> Arc<CoordCaptureService> {
    CoordCaptureService::new(CoordCaptureProviders {
        cursor_position: Arc::new(|| Some((0, 0))),
        region: Arc::new(|| {
            Some(CoordRegion {
                x: 0,
                y: 0,
                w: 120,
                h: 24,
            })
        }),
        capture_region: Arc::new(|_, _, _, _| {
            Some(BgrImage {
                data: vec![0; 12],
                h: 2,
                w: 2,
            })
        }),
        // The split per-half reads answer empty (no digit runs), so the
        // scan falls through to the whole-strip single-line read, which
        // is where these tests inject their text. The halves are one
        // column wide; the whole strip is two.
        read_text: Arc::new(move |img| {
            if img.w == 1 {
                return Some((String::new(), 0.9));
            }
            Some((text.to_string(), 0.9))
        }),
        persist_region: Arc::new(|_| Ok(())),
        debug_dir: Arc::new(|| None),
    })
}

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot, with or without the planet-map bundle.
async fn maps_api(dir: &Path, with_bundle: bool, coord: Option<Arc<CoordCaptureService>>) -> Api {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let planet_maps = with_bundle
        .then(|| Arc::new(PlanetMapStore::new(&bundled_maps_dir()).expect("bundled maps load")));
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
        planet_maps,
        coord,
        None,
    )
}

fn calypso_pin() -> MapPinInput {
    MapPinInput {
        planet: "Calypso".to_string(),
        lon: 61400.0,
        lat: 75800.0,
        altitude: Some(103.0),
        name: "Port Atlantis TP".to_string(),
        icon: "teleporter".to_string(),
        kind: "travel".to_string(),
        radius_m: None,
        notes: Some("the sanity anchor".to_string()),
        session_id: None,
        map_view_id: None,
        pin_config_id: None,
        allow_nearby: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_catalogue_serialises_with_its_bundle_shape() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;

    let maps = api.planet_maps().unwrap();
    assert_eq!(maps.len(), 20);

    let calypso = serde_json::to_value(maps.iter().find(|m| m.name == "Calypso").unwrap()).unwrap();
    assert_eq!(calypso["imageMime"], "image/jpeg");
    assert_eq!(calypso["imageWidthPx"], 4608);
    assert_eq!(calypso["calibration"]["unitsPerPixelX"], 16.0);
    assert_eq!(calypso["calibration"]["bounds"]["lonMin"], 16384);

    // The view-only map: present, explicitly null-calibrated.
    let thule = serde_json::to_value(maps.iter().find(|m| m.name == "Thule").unwrap()).unwrap();
    assert_eq!(thule["calibration"], serde_json::Value::Null);
    assert_eq!(thule["technicalName"], serde_json::Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_facade_without_the_bundle_serves_an_empty_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), false, None).await;
    assert!(api.planet_maps().unwrap().is_empty());
    assert!(api.planet_map_image("Calypso").is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pin_roundtrips_with_its_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;

    let created = api.map_pin_create(calypso_pin()).await.unwrap();
    let listed = api
        .map_pins_list("Calypso".to_string(), None)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);

    let wire = serde_json::to_value(&listed[0]).unwrap();
    assert_eq!(wire["id"], created.id);
    assert_eq!(wire["planet"], "Calypso");
    assert_eq!(wire["lon"], 61400.0);
    assert_eq!(wire["radiusM"], serde_json::Value::Null);
    assert_eq!(wire["notes"], "the sanity anchor");
    assert_eq!(wire["sessionId"], serde_json::Value::Null);
    assert_eq!(wire["mapViewId"], serde_json::Value::Null);
    assert!(wire["createdAt"].is_f64());

    // Another planet's list stays empty (planet-scoped reads).
    assert!(api
        .map_pins_list("Arkadia".to_string(), None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nearby_pin_creation_is_advisory_and_requires_explicit_override() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;
    let first = api.map_pin_create(calypso_pin()).await.unwrap();

    let nearby = api
        .map_pin_nearby("Calypso".to_string(), None, 61402.0, 75800.0)
        .await
        .unwrap()
        .0
        .expect("two-unit boundary is included");
    assert_eq!(nearby.pin.id, first.id);
    assert_eq!(nearby.distance, 2.0);

    let mut duplicate = calypso_pin();
    duplicate.lon = 61402.0;
    duplicate.lat = 75800.0;
    duplicate.name = "Second tree".into();
    assert!(matches!(
        api.map_pin_create(duplicate).await,
        Err(ApiError::BadRequest { .. })
    ));

    let mut confirmed = calypso_pin();
    confirmed.lon = 61402.0;
    confirmed.lat = 75800.0;
    confirmed.name = "Second tree".into();
    confirmed.allow_nearby = true;
    assert!(api.map_pin_create(confirmed).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_map_views_isolate_and_cascade_their_pins() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;

    let view = api
        .map_view_create("Calypso".to_string(), "Tree map".to_string())
        .await
        .unwrap();
    assert_eq!(view.planet, "Calypso");
    assert_eq!(view.name, "Tree map");

    let default_pin = api.map_pin_create(calypso_pin()).await.unwrap();
    let mut tree_pin = calypso_pin();
    tree_pin.name = "Papplon tree".to_string();
    tree_pin.map_view_id = Some(view.id);
    let tree_pin = api.map_pin_create(tree_pin).await.unwrap();

    let default_pins = api
        .map_pins_list("Calypso".to_string(), None)
        .await
        .unwrap();
    assert_eq!(default_pins.len(), 1);
    assert_eq!(default_pins[0].id, default_pin.id);

    let tree_pins = api
        .map_pins_list("Calypso".to_string(), Some(view.id))
        .await
        .unwrap();
    assert_eq!(tree_pins.len(), 1);
    assert_eq!(tree_pins[0].id, tree_pin.id);
    assert_eq!(tree_pins[0].map_view_id, Some(view.id));

    let renamed = api
        .map_view_rename(view.id, "Trees".to_string())
        .await
        .unwrap();
    assert_eq!(renamed.name, "Trees");
    assert_eq!(
        api.map_views_list("Calypso".to_string())
            .await
            .unwrap()
            .len(),
        1
    );

    api.map_view_delete(view.id).await.unwrap();
    assert!(api
        .map_views_list("Calypso".to_string())
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        api.map_pins_list("Calypso".to_string(), Some(view.id))
            .await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.map_pin_update(tree_pin.id, MapPinPatch::default())
            .await,
        Err(ApiError::NotFound { .. })
    ));
    assert_eq!(
        api.map_pins_list("Calypso".to_string(), None)
            .await
            .unwrap()[0]
            .id,
        default_pin.id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn implausible_coordinates_are_refused_on_create_and_move() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;

    let mut outside = calypso_pin();
    outside.lat = 999_999.0;
    assert!(matches!(
        api.map_pin_create(outside).await,
        Err(ApiError::BadRequest { .. })
    ));

    let created = api.map_pin_create(calypso_pin()).await.unwrap();
    let move_outside = MapPinPatch {
        lon: Some(1.0),
        ..MapPinPatch::default()
    };
    assert!(matches!(
        api.map_pin_update(created.id, move_outside).await,
        Err(ApiError::BadRequest { .. })
    ));

    // Without the bundle there is nothing authoritative to gate against:
    // the same out-of-bounds pin is accepted rather than invented-refused.
    let bare = maps_api(&dir.path().join("bare"), false, None).await;
    let mut outside = calypso_pin();
    outside.lat = 999_999.0;
    assert!(bare.map_pin_create(outside).await.is_ok());

    let mut unknown = calypso_pin();
    unknown.planet = "Not a planet".to_string();
    assert!(matches!(
        api.map_pin_create(unknown).await,
        Err(ApiError::BadRequest { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_distinguishes_absent_from_explicit_null() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;
    let created = api.map_pin_create(calypso_pin()).await.unwrap();

    // The wire shape: notes explicitly nulled, name changed, the rest
    // absent. Deserialised through serde to exercise the double-option
    // semantics the typed transport carries.
    let patch: MapPinPatch =
        serde_json::from_value(serde_json::json!({"name": "PA", "notes": null})).unwrap();
    let updated = api.map_pin_update(created.id, patch).await.unwrap();
    assert_eq!(updated.name, "PA");
    assert_eq!(*updated.notes, None);
    // Untouched fields survived.
    assert_eq!(*updated.altitude, Some(103.0));
    assert_eq!(updated.lon, 61400.0);

    // An empty patch is a no-op update, not an error.
    let unchanged = api
        .map_pin_update(created.id, MapPinPatch::default())
        .await
        .unwrap();
    assert_eq!(unchanged.name, "PA");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_and_not_found_legs_answer_typed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, None).await;

    let mut nameless = calypso_pin();
    nameless.name = "   ".to_string();
    assert!(matches!(
        api.map_pin_create(nameless).await,
        Err(ApiError::BadRequest { .. })
    ));

    let mut degenerate = calypso_pin();
    degenerate.radius_m = Some(0.0);
    assert!(matches!(
        api.map_pin_create(degenerate).await,
        Err(ApiError::BadRequest { .. })
    ));

    assert!(matches!(
        api.map_pin_update(9999, MapPinPatch::default()).await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        api.map_pin_delete(9999).await,
        Err(ApiError::NotFound { .. })
    ));

    let created = api.map_pin_create(calypso_pin()).await.unwrap();
    api.map_pin_delete(created.id).await.unwrap();
    assert!(api
        .map_pins_list("Calypso".to_string(), None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_coordinate_scan_gates_against_the_selected_planet() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, Some(coord_service("61234, 75456"))).await;

    // A clean read inside Calypso's bounds, in its wire shape.
    let wire = serde_json::to_value(
        api.maps_scan_coordinates(Some("Calypso".to_string()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wire["status"], "read");
    assert_eq!(wire["lon"], 61234);
    assert_eq!(wire["lat"], 75456);
    // The readout carries no altitude, so the wire shape has no such field.
    assert!(wire.get("altitude").is_none());
    // The successful read's coordinates carry everything; the raw
    // capture text stays server-side (it only rides the unreadable leg).
    assert!(wire.get("rawText").is_none());

    // The same read is implausible against a small instance's window.
    let wire = serde_json::to_value(
        api.maps_scan_coordinates(Some("Monria".to_string()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wire["status"], "implausible");

    // No planet named: the raw read answers ungated.
    let wire = serde_json::to_value(api.maps_scan_coordinates(None).unwrap()).unwrap();
    assert_eq!(wire["status"], "read");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_capture_answers_its_typed_status() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, Some(coord_service("loading..."))).await;
    let wire = serde_json::to_value(api.maps_scan_coordinates(None).unwrap()).unwrap();
    assert_eq!(wire["status"], "unreadable");
    assert_eq!(wire["rawText"], "loading...");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_calibration_verbs_walk_the_flow_and_report_status() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), true, Some(coord_service("61234, 75456"))).await;

    let status = serde_json::to_value(api.maps_calibration_status().unwrap()).unwrap();
    assert_eq!(status["phase"], "idle");
    assert!(status["region"].is_object());

    let started = serde_json::to_value(api.maps_calibration_start().unwrap()).unwrap();
    assert_eq!(started["phase"], "awaitTopLeft");

    let cancelled = serde_json::to_value(api.maps_calibration_cancel().unwrap()).unwrap();
    assert_eq!(cancelled["phase"], "idle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_facade_without_capture_seams_reports_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), false, None).await;
    assert!(api.maps_calibration_status().is_err());
    assert!(api.maps_scan_coordinates(None).is_err());
}
