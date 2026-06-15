//! Manual-scan / repair / spacebar / snapshot conformance through the PUBLIC
//! PORT: the natively-served scan surface proven byte-for-byte against the
//! running Python sidecar.
//!
//!   GET  /api/scan/skills/status            -> ScanManualStatus
//!   POST /api/scan/skills/{start,capture,cancel,undo,process,accept,reject}
//!   GET  /api/scan/skills/pending           -> 404 when none
//!   GET  /api/scan/skills/capture/{page}    -> 404 / 422
//!   POST /api/scan/spacebar-capture         -> SpacebarCaptureResult
//!   POST /api/tracking/session/{id}/repair-scan -> RepairScanResult
//!   GET  /api/tracking/snapshot             -> TrackingSnapshot (idle)
//!
//! TOPOLOGY (the producer-conformance two-arm form): a native substrate over
//! the upstream sidecar's data dir, plus a separate pure-Python comparison
//! sidecar, both seeded with an identical `settings.json`. Each case drives
//! BOTH arms and compares status + contract headers + body byte-for-byte.
//!
//! HOST-INDEPENDENCE. The native arm composes the scan with `engine_available
//! = || true` and `skill_region = || None`, mirroring the headless sidecar
//! whose lazily-loaded `local_ocr` engine is present but whose screen has no
//! game window: both arms then report `configured = true`,
//! `game_window_present = false`, and a `start` that fails with the same
//! "window not found". The repair reader's region lookup is likewise empty on
//! both, so a repair scan over an enabled config fails identically. The
//! state-machine guards (capture/undo/process/accept/reject from idle) settle
//! before any engine or window is consulted, so they are inherently
//! host-independent.
//!
//! EXCLUDED (documented gate edges, not silent gaps): the capture -> process
//! -> accept SUCCESS path needs a real game window and panel, so it is not
//! hermetically comparable (covered by the ratified OCR-equivalence spike and
//! the hermetic native-router state-machine test); the ACTIVE snapshot carries
//! a per-arm random session id, so only the deterministic IDLE snapshot is
//! byte-compared here (the active shape is covered by the hermetic test).
//!
//! Gated behind the `cross-language` feature: it needs the Python interpreter
//! and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test scan_conformance
#![cfg(feature = "cross-language")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use chrono::NaiveDateTime;
use eo_http::arms::ArmOverrides;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::MockClock;
use eo_services::config_service::ConfigService;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::keystroke_source::MockKeystrokeSource;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders};
use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
use eo_services::skill_tracker::SkillTracker;
use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use eo_services::tracker::{HuntTracker, Providers};
use http_body_util::BodyExt;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

const CLOCK: &str = "2026-06-01T12:00:00";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn oracle_python() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_ORACLE_PYTHON") {
        return PathBuf::from(explicit);
    }
    let root = repo_root();
    let windows = root.join(".venv/Scripts/python.exe");
    if windows.exists() {
        windows
    } else {
        root.join(".venv/bin/python")
    }
}

struct Sidecar {
    child: Child,
    port: u16,
    data_dir: tempfile::TempDir,
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Spawn a sidecar over a fresh data dir seeded with `config_json`; producers
/// stood down (idle) so the scan/snapshot reads serve without a parallel
/// producer mutating state, and the keystroke sources fall back to mocks.
fn spawn_sidecar(config_json: &str) -> Sidecar {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
    std::fs::write(data_dir.path().join("settings.json"), config_json).expect("seed settings");
    let port = free_port();
    let mut command = Command::new(oracle_python());
    command
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
        .env("ENTROPIA_TEST_CLOCK_START", CLOCK)
        .env("ENTROPIAORME_PRODUCERS_IDLE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn().expect("spawn backend sidecar");
    Sidecar {
        child,
        port,
        data_dir,
    }
}

fn client() -> eo_http::proxy::ProxyClient {
    eo_http::proxy::build_client()
}

async fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(
            body.map(|b| Body::from(b.to_owned()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = client().request(request).await.expect("request succeeds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

async fn wait_healthy(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            panic!("backend never became healthy on port {port}");
        }
        let authority = format!("127.0.0.1:{port}");
        let probe = http::Request::builder()
            .uri(format!("http://{authority}/api/health"))
            .header("host", &authority)
            .body(Body::empty())
            .unwrap();
        if let Ok(response) = client().request(probe).await {
            if response.status() == http::StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn contract_axes(headers: &http::HeaderMap) -> Vec<Option<String>> {
    let value = |name: http::header::HeaderName| {
        headers
            .get(name)
            .map(|v| v.to_str().unwrap_or("<non-utf8>").to_string())
    };
    vec![
        value(http::header::CONTENT_TYPE),
        value(http::header::CACHE_CONTROL),
        value(http::header::ETAG),
    ]
}

struct Arms {
    substrate_port: u16,
    comparison_port: u16,
}

impl Arms {
    /// Drive a request through both arms; assert status + contract headers +
    /// body match byte-for-byte. Returns the native status so a call site can
    /// pin the absolute code too.
    async fn compare(&self, method: &str, path: &str, body: Option<&str>) -> http::StatusCode {
        let (native_status, native_headers, native_body) =
            request(self.substrate_port, method, path, body).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, method, path, body).await;
        assert_eq!(
            native_status,
            cmp_status,
            "status diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&cmp_headers),
            "contract headers diverged on {method} {path}"
        );
        assert_eq!(
            native_body,
            cmp_body,
            "body diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        native_status
    }
}

async fn open_pool(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .expect("native pool opens over the sidecar db")
}

/// Build a native substrate over the upstream sidecar's data dir with the
/// scan services, the live tracker, and the input listeners the scan and
/// snapshot routes depend on, mirroring composition with engine-present /
/// no-window providers so the headless arm matches the sidecar's.
async fn boot(config_json: &str) -> (Sidecar, Sidecar, Arms) {
    let upstream = spawn_sidecar(config_json);
    let comparison = spawn_sidecar(config_json);
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let data_dir = upstream.data_dir.path().to_path_buf();
    let pool = open_pool(&data_dir.join("entropia_orme.db")).await;
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let naive = NaiveDateTime::parse_from_str(CLOCK, "%Y-%m-%dT%H:%M:%S").expect("clock parses");
    let clock = Arc::new(MockClock::new(Some(naive), 0.0));
    let bus = Arc::new(EventBus::new());
    let tracker = HuntTracker::new(
        bus.clone(),
        pool.clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
        Providers::default(),
    )
    .expect("native tracker builds over the shared pool");
    let skill_tracker = SkillTracker::new(
        &bus,
        pool.clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
    );
    let config_service = Arc::new(Mutex::new(
        ConfigService::new(&data_dir).expect("config service opens"),
    ));

    // The scan composes with the engine reported available (the sidecar's
    // lazily-loaded engine is present on the conformance hosts) and no game
    // window (headless), so both arms report the same status and the same
    // "window not found" on a start; the extraction seam is inert (never
    // reached without a captured page).
    let skill_scan = SkillScanManual::new(
        ScanProviders {
            engine_available: Arc::new(|| true),
            skill_region: Arc::new(|| None),
            capture_region: Arc::new(|_| None),
            extract_page_levels: Arc::new(|_: &[u8]| Vec::<(String, f64)>::new()),
        },
        clock.clone(),
        Some(bus.clone()),
        None,
        0,
    );
    // The repair reader's region lookup is empty (no window), matching the
    // sidecar, so an enabled repair scan fails identically.
    let repair_ocr = Arc::new(RepairOcrService::new(RepairProviders::default()));
    let spacebar = SpacebarCaptureListener::new(
        skill_scan.clone(),
        Some(Arc::new(MockKeystrokeSource::new())),
    );
    let hotbar = HotbarListener::new(
        bus.clone(),
        Some(Arc::new(MockKeystrokeSource::new())),
        None,
    );

    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        clock,
        data_dir.clone(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind substrate");
    listener.set_nonblocking(true).expect("nonblocking");
    let substrate_port = listener.local_addr().expect("addr").port();
    let state = Arc::new(
        AppState::new(
            format!("127.0.0.1:{}", upstream.port),
            substrate_port,
            ArmOverrides::empty(),
        )
        .with_hydration(hydration)
        .with_tracker(tracker)
        .with_skill_tracker(skill_tracker)
        .with_config_service(config_service)
        .with_skill_scan(skill_scan)
        .with_repair_ocr(repair_ocr)
        .with_spacebar_listener(spacebar)
        .with_hotbar_listener(hotbar)
        .with_cors(CorsConfig::new(5173, None)),
    );
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, state).await.expect("serve");
    });
    wait_healthy(substrate_port).await;

    let arms = Arms {
        substrate_port,
        comparison_port: comparison.port,
    };
    (upstream, comparison, arms)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_skills_state_machine_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // The resting status (GET, ETag-scoped): both report idle with the engine
    // present and no game window.
    assert_eq!(
        arms.compare("GET", "/api/scan/skills/status", None).await,
        http::StatusCode::OK,
    );

    // start without a window: the same "window not found" refusal (a 200 body).
    assert_eq!(
        arms.compare("POST", "/api/scan/skills/start", Some(""))
            .await,
        http::StatusCode::OK,
    );
    // start with an unparseable page count: the framework's 422 int_parsing.
    assert_eq!(
        arms.compare("POST", "/api/scan/skills/start?page_count=abc", Some(""))
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );

    // The state-machine guards from idle, each settling before any engine or
    // window is consulted (so host-independent), all plain 200s.
    for path in [
        "/api/scan/skills/capture",
        "/api/scan/skills/process",
        "/api/scan/skills/accept",
        "/api/scan/skills/reject",
        "/api/scan/skills/undo",
        "/api/scan/skills/cancel",
    ] {
        assert_eq!(
            arms.compare("POST", path, Some("")).await,
            http::StatusCode::OK,
            "{path} guard"
        );
    }

    // pending with nothing held: the reference's 404.
    assert_eq!(
        arms.compare("GET", "/api/scan/skills/pending", None).await,
        http::StatusCode::NOT_FOUND,
    );
    // capture PNG for a page never captured: 404; an unparseable page: 422;
    // a percent-encoded slash de-matches the route (the framework 404).
    assert_eq!(
        arms.compare("GET", "/api/scan/skills/capture/1", None)
            .await,
        http::StatusCode::NOT_FOUND,
    );
    assert_eq!(
        arms.compare("GET", "/api/scan/skills/capture/abc", None)
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(
        arms.compare("GET", "/api/scan/skills/capture/1%2F2", None)
            .await,
        http::StatusCode::NOT_FOUND,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spacebar_capture_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // The toggle acknowledgement (a plain 200, mock source on both arms).
    assert_eq!(
        arms.compare("POST", "/api/scan/spacebar-capture?enabled=true", Some(""))
            .await,
        http::StatusCode::OK,
    );
    assert_eq!(
        arms.compare("POST", "/api/scan/spacebar-capture?enabled=false", Some(""))
            .await,
        http::StatusCode::OK,
    );
    // An uninterpretable boolean: 422 bool_parsing; absent: 422 missing.
    assert_eq!(
        arms.compare("POST", "/api/scan/spacebar-capture?enabled=maybe", Some(""))
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(
        arms.compare("POST", "/api/scan/spacebar-capture", Some(""))
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_scan_conforms() {
    // Disabled: both refuse with the same 400.
    let (_u, _c, arms) = boot(r#"{"repair_ocr_enabled": false}"#).await;
    assert_eq!(
        arms.compare("POST", "/api/tracking/session/abc/repair-scan", Some(""))
            .await,
        http::StatusCode::BAD_REQUEST,
    );

    // Enabled: the region lookup is empty on both, so the scan fails with the
    // same "window not found" (a 200 body carrying the failure legs).
    let (_u, _c, arms) = boot(r#"{"repair_ocr_enabled": true}"#).await;
    assert_eq!(
        arms.compare("POST", "/api/tracking/session/abc/repair-scan", Some(""))
            .await,
        http::StatusCode::OK,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_snapshot_idle_conforms() {
    // The idle dashboard is deterministic across arms (no per-arm session id,
    // no engine/window dependence): the full polymorphic projection, the
    // envelope, and the trifecta summary over the default config.
    let (_upstream, _comparison, arms) = boot("{}").await;
    assert_eq!(
        arms.compare("GET", "/api/tracking/snapshot", None).await,
        http::StatusCode::OK,
    );
}
