//! Behavioural pins for the maps family over the typed facade: the
//! bundled catalogue read (with and without the bundle composed), the
//! pin CRUD roundtrip with its DTO wire shape, the map-bounds gate on
//! create and move, the patch's double-option null semantics, the
//! not-found / bad-request legs, and the coordinate reads, the position
//! poll and radar guidance in their wire shapes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eo_api::maps::{MapPinInput, MapPinPatch, PinConfigInput};
use eo_api::{Api, ApiError};
use eo_services::clock::{Clock, MockClock, RealClock};
use eo_services::coord_capture::{
    CoordCaptureProviders, CoordCaptureService, CoordRegion, KnownPlanet,
};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::geometry::{BgrImage, Rect};
use eo_services::game_ui::radar::CoordinateBounds;
use eo_services::game_ui::seams::FrameReader;
use eo_services::game_ui::test_support::{
    blank, bundled_profile, draw_window, window_frame, Printed, RadarScene, RINGS, RING_LABELS,
};
use eo_services::navigation::{NavigationProviders, NavigationService, NavigationSettings};
use eo_services::planet_maps::PlanetMapStore;

use crate::common;

/// The repo's bundled maps directory (the dev-layout resolution).
fn bundled_maps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("entropia-orme")
        .join("resources")
        .join("maps")
}

/// A stand-in game window: what it shows, how its lettering reads, and
/// what the hand-drawn region's two halves read when one is drawn.
struct Screen {
    shown: Mutex<(BgrImage, FrameReader)>,
    region: Mutex<Option<(String, String)>>,
}

impl Screen {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            shown: Mutex::new((
                blank(64, 48, [120, 110, 90]),
                Arc::new(|_: &BgrImage| Some((String::new(), 0.9))),
            )),
            region: Mutex::new(None),
        })
    }

    fn show(&self, scene: &RadarScene) {
        *self.shown.lock().unwrap() = (scene.image.clone(), Arc::new(scene.recogniser()));
    }

    /// A two-column strip whose columns carry the bytes the region's
    /// reader tells its halves apart by.
    fn strip() -> BgrImage {
        BgrImage {
            data: [vec![1u8; 3], vec![2u8; 3], vec![1u8; 3], vec![2u8; 3]].concat(),
            h: 2,
            w: 2,
        }
    }

    fn capture(self: &Arc<Self>, clock: Arc<dyn Clock>) -> Arc<CoordCaptureService> {
        let (window, reading, halves, drawn) =
            (self.clone(), self.clone(), self.clone(), self.clone());
        CoordCaptureService::new(
            CoordCaptureProviders {
                frame: Arc::new(move |area| {
                    let image = window.shown.lock().unwrap().0.clone();
                    window_frame(&image, (0, 0), area)
                }),
                profile: Some(Arc::new(bundled_profile())),
                planets: ["Calypso", "Monria"]
                    .map(|name| KnownPlanet {
                        name: name.to_string(),
                        printed: vec![name.to_string()],
                    })
                    .to_vec(),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    if crop.w == 1 && matches!(crop.data.first(), Some(1 | 2)) {
                        let region = halves.region.lock().unwrap().clone()?;
                        let text = if crop.data[0] == 1 {
                            region.0
                        } else {
                            region.1
                        };
                        return Some((text, 0.9));
                    }
                    let reader = reading.shown.lock().unwrap().1.clone();
                    reader(crop)
                })),
                cursor_position: Arc::new(|| Some((0, 0))),
                region: Arc::new(move || {
                    drawn.region.lock().unwrap().as_ref().map(|_| CoordRegion {
                        x: 0,
                        y: 0,
                        w: 120,
                        h: 24,
                    })
                }),
                capture_region: Arc::new(|_, _, _, _| Some(Screen::strip())),
                ..CoordCaptureProviders::default()
            },
            clock,
        )
    }
}

/// A radar with both values printed under it.
fn radar_with(lon: &str, lat: &str) -> RadarScene {
    RadarScene::with_coordinates(
        (900, 200),
        113,
        Some((lon, Printed::of(5))),
        Some((lat, Printed::of(5))),
    )
}

/// A coordinate-capture service over a screen showing `scene`.
fn coord_service(scene: &RadarScene) -> Arc<CoordCaptureService> {
    let screen = Screen::new();
    screen.show(scene);
    screen.capture(Arc::new(RealClock::new()))
}

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot, with or without the planet-map bundle.
async fn maps_api(dir: &Path, with_bundle: bool, coord: Option<Arc<CoordCaptureService>>) -> Api {
    let (db, data_dir) = open_db(dir).await;
    maps_api_over(dir, db, data_dir, with_bundle, coord, None).await
}

async fn open_db(dir: &Path) -> (Db, PathBuf) {
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    (db, data_dir)
}

async fn maps_api_over(
    dir: &Path,
    db: Db,
    data_dir: PathBuf,
    with_bundle: bool,
    coord: Option<Arc<CoordCaptureService>>,
    navigation: Option<Arc<NavigationService>>,
) -> Api {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
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
        handles.sale_capture,
        handles.quests.clone(),
        planet_maps,
        coord,
        navigation,
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

fn scan(api: &Api, planet: &str) -> serde_json::Value {
    serde_json::to_value(api.maps_scan_coordinates(planet.to_string()).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_position_read_under_the_radar_reaches_the_wire_with_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let scene = radar_with("LON 67739", "LAT 87740");
    let api = maps_api(dir.path(), true, Some(coord_service(&scene))).await;
    assert_eq!(
        scan(&api, "Calypso"),
        serde_json::json!({"kind": "read", "lon": 67739, "lat": 87740, "source": "radar"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_reaches_the_wire_with_each_values_reason_and_no_coordinate() {
    let dir = tempfile::tempdir().unwrap();
    let mut scene = RadarScene::with_coordinates(
        (900, 200),
        169,
        Some(("LON 68520", Printed::of(5))),
        Some(("LAT 87611", Printed::of(5))),
    );
    draw_window(&mut scene.image, Rect::new(300, 40, 900 - 169 + 80, 650));
    let api = maps_api(dir.path(), true, Some(coord_service(&scene))).await;
    assert_eq!(
        scan(&api, "Calypso"),
        serde_json::json!({
            "kind": "refused",
            "refusal": {"reason": "notRead", "lon": "covered", "lat": null},
        })
    );

    // Values that read, but lie outside the planet named.
    let scene = radar_with("LON 67739", "LAT 87740");
    let api = maps_api(&dir.path().join("other"), true, Some(coord_service(&scene))).await;
    assert_eq!(
        scan(&api, "Monria"),
        serde_json::json!({
            "kind": "refused",
            "refusal": {"reason": "notRead", "lon": "outOfBounds", "lat": "outOfBounds"},
        })
    );
    // A planet whose coordinates are not known reads nothing.
    for planet in ["Thule", "Nowhere"] {
        assert_eq!(
            scan(&api, planet),
            serde_json::json!({
                "kind": "refused",
                "refusal": {"reason": "planetUnknown", "lon": null, "lat": null},
            })
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_radar_naming_another_planet_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut scene = radar_with("LON 67739", "LAT 87740");
    scene.planet_label((900, 200), 113, "Monria");
    let api = maps_api(dir.path(), true, Some(coord_service(&scene))).await;
    assert_eq!(
        scan(&api, "Calypso"),
        serde_json::json!({
            "kind": "refused",
            "refusal": {"reason": "otherPlanet", "lon": null, "lat": null},
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_radar_only_a_hand_drawn_region_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let screen = Screen::new();
    let api = maps_api(
        dir.path(),
        true,
        Some(screen.capture(Arc::new(RealClock::new()))),
    )
    .await;
    assert_eq!(
        scan(&api, "Calypso"),
        serde_json::json!({
            "kind": "refused",
            "refusal": {"reason": "radarNotFound", "lon": null, "lat": null},
        })
    );
    *screen.region.lock().unwrap() = Some(("LON 61234".into(), "LAT 75456".into()));
    assert_eq!(
        scan(&api, "Calypso"),
        serde_json::json!({"kind": "read", "lon": 61234, "lat": 75456, "source": "region"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_calibration_verbs_walk_the_flow_and_report_status() {
    let dir = tempfile::tempdir().unwrap();
    let screen = Screen::new();
    *screen.region.lock().unwrap() = Some(("LON 61234".into(), "LAT 75456".into()));
    let api = maps_api(
        dir.path(),
        true,
        Some(screen.capture(Arc::new(RealClock::new()))),
    )
    .await;

    let status = serde_json::to_value(api.maps_calibration_status().unwrap()).unwrap();
    assert_eq!(
        status,
        serde_json::json!({
            "phase": "idle",
            "region": {"x": 0, "y": 0, "w": 120, "h": 24},
        })
    );

    let started = serde_json::to_value(api.maps_calibration_start().unwrap()).unwrap();
    assert_eq!(started["phase"], "awaitTopLeft");

    // A confirm takes the corner at the cursor, as Enter does.
    let first = serde_json::to_value(api.maps_calibration_confirm().unwrap()).unwrap();
    assert_eq!(first["phase"], "awaitBottomRight");
    // The scripted cursor never moves, so the second corner is degenerate
    // and stays armed rather than saving an unreadable region.
    let second = serde_json::to_value(api.maps_calibration_confirm().unwrap()).unwrap();
    assert_eq!(second["phase"], "awaitBottomRight");

    let cancelled = serde_json::to_value(api.maps_calibration_cancel().unwrap()).unwrap();
    assert_eq!(cancelled["phase"], "idle");
    // With no flow running a confirm captures nothing.
    let idle = serde_json::to_value(api.maps_calibration_confirm().unwrap()).unwrap();
    assert_eq!(idle["phase"], "idle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_facade_without_capture_seams_reports_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let api = maps_api(dir.path(), false, None).await;
    assert!(api.maps_calibration_status().is_err());
    assert!(api.maps_scan_coordinates("Calypso".to_string()).is_err());
    assert!(api.radar_guidance().is_err());
}

/// The facade with route navigation over `screen`, on a clock the test
/// moves, and a tree pin on Calypso to route to.
async fn navigation_api(
    dir: &Path,
    screen: &Arc<Screen>,
) -> (Api, Arc<MockClock>, Arc<NavigationService>) {
    let (db, data_dir) = open_db(dir).await;
    let clock = Arc::new(MockClock::new(None, 0.0));
    let shared: Arc<dyn Clock> = clock.clone();
    let capture = screen.capture(shared.clone());
    let navigation = NavigationService::new(
        db.clone(),
        shared,
        NavigationProviders {
            coord_capture: capture.clone(),
            bounds: Arc::new(|_| {
                Some(CoordinateBounds {
                    lon: (16384, 90112),
                    lat: (24576, 98304),
                })
            }),
            changed: Arc::new(|| {}),
            input: None,
            focus: None,
            settings: NavigationSettings {
                guidance_interval: Duration::from_secs(3600),
                ..NavigationSettings::default()
            },
        },
    )
    .await;
    let api = maps_api_over(
        dir,
        db,
        data_dir,
        true,
        Some(capture),
        Some(navigation.clone()),
    )
    .await;
    let tree = api
        .pin_config_create(PinConfigInput {
            planet: "Calypso".to_string(),
            map_view_id: None,
            label: "Tree".to_string(),
            category: "special".to_string(),
            special_kind: Some("tree".to_string()),
            icon: "tree".to_string(),
            radius_m: None,
            colour: "#22c55e".to_string(),
            cooldown_colour: Some("#f59e0b".to_string()),
        })
        .await
        .unwrap();
    api.map_pin_create(MapPinInput {
        pin_config_id: Some(tree.id),
        ..calypso_pin()
    })
    .await
    .unwrap();
    (api, clock, navigation)
}

async fn poll(api: &Api) -> serde_json::Value {
    serde_json::to_value(api.navigation_update_position().await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_position_poll_carries_a_position_only_when_it_was_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let screen = Screen::new();
    let (api, clock, _navigation) = navigation_api(dir.path(), &screen).await;
    api.navigation_start("Calypso".to_string(), None, 61000.0, 75000.0, None, false)
        .await
        .unwrap();

    // With the route overlays away, the poll reads nothing.
    assert_eq!(
        poll(&api).await,
        serde_json::json!({
            "status": "pollStopped",
            "run": null,
            "refusal": null,
            "stopped": "overlaysHidden",
        })
    );
    api.navigation_set_overlays_shown(true).unwrap();

    clock.advance(1.0).unwrap();
    assert_eq!(
        poll(&api).await,
        serde_json::json!({
            "status": "notRead",
            "run": null,
            "refusal": {"reason": "radarNotFound", "lon": null, "lat": null},
            "stopped": null,
        })
    );

    *screen.region.lock().unwrap() = Some(("L0N 61050".into(), "LAT 75000".into()));
    clock.advance(1.0).unwrap();
    assert_eq!(
        poll(&api).await,
        serde_json::json!({
            "status": "notRead",
            "run": null,
            "refusal": {"reason": "notRead", "lon": "unclear", "lat": null},
            "stopped": null,
        })
    );

    // The route's start was typed, not read: its first reading needs the
    // next one's agreement.
    *screen.region.lock().unwrap() = Some(("LON 61050".into(), "LAT 75000".into()));
    clock.advance(1.0).unwrap();
    assert_eq!(poll(&api).await["status"], "jumpRefused");
    clock.advance(1.0).unwrap();
    let updated = poll(&api).await;
    assert_eq!(updated["status"], "updated");
    assert_eq!(updated["refusal"], serde_json::Value::Null);
    assert_eq!(updated["run"]["currentLon"], 61050.0);
    assert_eq!(updated["run"]["currentLat"], 75000.0);
    assert_eq!(updated["run"]["positionAgeS"], 0.0);

    *screen.region.lock().unwrap() = Some(("LON 71050".into(), "LAT 75000".into()));
    clock.advance(1.0).unwrap();
    assert_eq!(
        poll(&api).await,
        serde_json::json!({"status": "jumpRefused", "run": null, "refusal": null, "stopped": null})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn radar_guidance_reaches_the_wire_with_its_placement_and_scale() {
    let dir = tempfile::tempdir().unwrap();
    let screen = Screen::new();
    let (api, _clock, navigation) = navigation_api(dir.path(), &screen).await;
    assert_eq!(
        serde_json::to_value(api.radar_guidance().unwrap()).unwrap(),
        serde_json::json!({"kind": "off", "stopped": null})
    );
    api.navigation_start("Calypso".to_string(), None, 61000.0, 75000.0, None, false)
        .await
        .unwrap();
    let mut scene = RadarScene::sized(1100, 700, 7);
    scene.radar((400, 300), 113, &RINGS);
    scene.ring_labels((400, 300), &RINGS, &RING_LABELS);
    screen.show(&scene);
    api.navigation_set_overlays_shown(true).unwrap();
    let looks = navigation.guidance_looks();
    api.radar_guidance_start().await.unwrap();
    tokio::task::block_in_place(|| {
        assert!(navigation.wait_for_guidance_looks(looks + 1, Duration::from_secs(120)));
    });
    let placed = serde_json::to_value(api.radar_guidance().unwrap()).unwrap();
    assert_eq!(placed["kind"], "placed", "{placed}");
    let number = |key: &str| placed[key].as_f64().unwrap();
    assert_eq!(placed["source"], "found");
    assert!((number("centreX") - 400.0).abs() <= 1.5, "{placed}");
    assert!((number("centreY") - 300.0).abs() <= 1.5, "{placed}");
    assert!((number("radiusPx") - 113.0).abs() <= 1.5, "{placed}");
    assert!(number("clearRightX") > number("centreX") + number("radiusPx"));
    let metres_per_pixel = placed["scale"]["metresPerPixel"].as_f64().unwrap();
    assert!(
        (metres_per_pixel / (200.0 / 103.0) - 1.0).abs() < 0.01,
        "{placed}"
    );
    assert_eq!(placed["scaleMissing"], serde_json::Value::Null);
    let profile = bundled_profile();
    assert_eq!(
        placed["labelStrip"],
        serde_json::json!({
            "from": profile.radar.rings.span[0],
            "to": 1.0,
            "halfHeight": profile.radar.rings.label_reach,
        })
    );
    let edge = placed["scale"]["edgeRangeM"].as_f64().unwrap();
    assert!(
        (edge / (number("radiusPx") * metres_per_pixel) - 1.0).abs() < 1e-9,
        "{placed}"
    );

    // Ending the route ends guidance.
    api.navigation_end().await.unwrap();
    assert_eq!(
        serde_json::to_value(api.radar_guidance().unwrap()).unwrap(),
        serde_json::json!({"kind": "off", "stopped": null})
    );
}
