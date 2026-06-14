//! Config-WRITE conformance through the PUBLIC PORT: the six side-effect-free
//! configuration writes proven against the running Python sidecar.
//!
//!   PUT  /api/settings/overlay-position -> {"ok": true}
//!   POST /api/tracking/release-mob       -> ReleaseMobResult
//!   POST /api/tracking/manual-mob-lock   -> ManualMobLockResult
//!   POST /api/tracking/tag-lock          -> TagLockResult
//!   POST /api/codex/claim                -> CodexClaimResult
//!   POST /api/codex/meta/claim           -> CodexMetaClaimResult
//!
//! TOPOLOGY (the producer-conformance two-arm form): a native substrate over
//! the upstream sidecar's data dir, plus a separate pure-Python comparison
//! sidecar, both seeded with an identical `settings.json`. Each case drives
//! BOTH arms and compares status + contract headers + body byte-for-byte
//! (none of these responses carry per-arm random / clock fields). The config
//! writes land in `settings.json`, so the two arms' files are compared after
//! each settings/tracking write (the substrate's `ConfigService` save is
//! byte-faithful to the sidecar's). The codex claim/meta writes land in the
//! codex tables (outside the db-snapshot catalogue), so they compare on the
//! response (incl. the 400 invalid-input + surrogate-input legs that settle
//! the encode-vs-bind envelope), with the suppress-next side effect carried
//! by the fidelity review and the skill-tracker unit suite.
//!
//! Gated behind the `cross-language` feature: it needs the Python interpreter
//! and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test settings_writes_conformance
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
use eo_services::skill_tracker::SkillTracker;
use eo_services::tracker::{HuntTracker, Providers};
use http_body_util::BodyExt;
use serde_json::Value;
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

/// Spawn a sidecar over a fresh data dir seeded with `config_json` as
/// `settings.json`; producers stood down so the arm under test owns the
/// write surface without a parallel producer mutating state.
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
    native_settings: PathBuf,
    comparison_settings: PathBuf,
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

    /// Compare the two arms' `settings.json` content (parsed, so benign
    /// whitespace never masks a value divergence) after a config write.
    fn compare_settings(&self, step: &str) {
        let native = read_settings(&self.native_settings);
        let comparison = read_settings(&self.comparison_settings);
        assert_eq!(
            native, comparison,
            "settings.json state diverged after {step}"
        );
    }
}

fn read_settings(path: &Path) -> Value {
    let raw = std::fs::read_to_string(path).expect("settings.json reads");
    serde_json::from_str(&raw).expect("settings.json parses")
}

async fn open_pool(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open shared database")
}

/// Build a native substrate over the upstream sidecar's data dir with the
/// settings writer, the live tracker, and the skill tracker the config-write
/// routes depend on, all on the shared single-owner pool and the frozen
/// clock, exactly as composition wires them.
async fn boot(config_json: &str) -> (Sidecar, Sidecar, Arms) {
    let upstream = spawn_sidecar(config_json);
    let comparison = spawn_sidecar(config_json);
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let data_dir = upstream.data_dir.path().to_path_buf();
    let native_db = data_dir.join("entropia_orme.db");
    let pool = open_pool(&native_db).await;
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
        native_settings: data_dir.join("settings.json"),
        comparison_settings: comparison.data_dir.path().join("settings.json"),
    };
    (upstream, comparison, arms)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlay_position_write_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // Success: the write lands and both arms reply {"ok": true} with the
    // same settings.json overlay coordinates.
    assert_eq!(
        arms.compare(
            "PUT",
            "/api/settings/overlay-position",
            Some(r#"{"x": 137, "y": 42}"#),
        )
        .await,
        http::StatusCode::OK,
    );
    arms.compare_settings("overlay-position write");

    // 422: an unparseable coordinate is the FastAPI int_parsing envelope.
    assert_eq!(
        arms.compare(
            "PUT",
            "/api/settings/overlay-position",
            Some(r#"{"x": "nope", "y": 0}"#),
        )
        .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mob_mode_config_writes_conform() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // manual-mob-lock: a catalogue match locks (both arms over the shared
    // snapshot agree on present-or-absent); the settings.json manual-mob
    // selection matches afterwards.
    arms.compare(
        "POST",
        "/api/tracking/manual-mob-lock",
        Some(r#"{"species": "Atrox", "maturity": "Young"}"#),
    )
    .await;
    arms.compare_settings("manual-mob-lock");

    // A mob absent from the catalogue is the shared 400.
    assert_eq!(
        arms.compare(
            "POST",
            "/api/tracking/manual-mob-lock",
            Some(r#"{"species": "Zzzzzznotamob", "maturity": ""}"#),
        )
        .await,
        http::StatusCode::BAD_REQUEST,
    );

    // release-mob in manual mode clears the stored selection; both arms
    // return the released display (or null) and the cleared settings match.
    arms.compare("POST", "/api/tracking/release-mob", None)
        .await;
    arms.compare_settings("release-mob (manual mode)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tag_mode_config_writes_conform() {
    let (_upstream, _comparison, arms) = boot(r#"{"mob_tracking_mode": "tag"}"#).await;

    // tag-lock: success sets the tag; both arms' settings.json match.
    assert_eq!(
        arms.compare(
            "POST",
            "/api/tracking/tag-lock",
            Some(r#"{"tag": "Daily Hunt"}"#),
        )
        .await,
        http::StatusCode::OK,
    );
    arms.compare_settings("tag-lock");

    // An empty tag is the shared 400.
    assert_eq!(
        arms.compare("POST", "/api/tracking/tag-lock", Some(r#"{"tag": "   "}"#))
            .await,
        http::StatusCode::BAD_REQUEST,
    );

    // manual-mob-lock is disabled in tag mode: the shared 409.
    assert_eq!(
        arms.compare(
            "POST",
            "/api/tracking/manual-mob-lock",
            Some(r#"{"species": "Atrox", "maturity": ""}"#),
        )
        .await,
        http::StatusCode::CONFLICT,
    );

    // release-mob in tag mode returns the trimmed tag and clears it.
    arms.compare("POST", "/api/tracking/release-mob", None)
        .await;
    arms.compare_settings("release-mob (tag mode)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tag_lock_outside_tag_mode_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;
    // Idle config in mob mode: tag-lock is rejected with the shared 409.
    assert_eq!(
        arms.compare("POST", "/api/tracking/tag-lock", Some(r#"{"tag": "X"}"#))
            .await,
        http::StatusCode::CONFLICT,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_claim_invalid_input_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // A species absent from the catalogue is the shared 400 (the service's
    // ValueError mapped to the detail envelope).
    assert_eq!(
        arms.compare(
            "POST",
            "/api/codex/claim",
            Some(r#"{"species_name": "Zzzzznotaspecies", "rank": 1, "skill_name": "Anatomy"}"#),
        )
        .await,
        http::StatusCode::BAD_REQUEST,
    );

    // An invalid meta attribute is the shared 400.
    assert_eq!(
        arms.compare(
            "POST",
            "/api/codex/meta/claim",
            Some(r#"{"attribute_name": "NotAnAttribute"}"#),
        )
        .await,
        http::StatusCode::BAD_REQUEST,
    );

    // Missing required fields are the shared 422 (FastAPI validation).
    assert_eq!(
        arms.compare("POST", "/api/codex/claim", Some(r#"{"rank": 1}"#))
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_claim_surrogate_input_conforms() {
    let (_upstream, _comparison, arms) = boot("{}").await;

    // A lone-surrogate species name reaches the codex lookup, which is not
    // found; the reference's would-be 400 then cannot be rendered (the
    // surrogate is unencodable in the detail body), so it surfaces as a
    // plain-text 500. The native arm must produce the same envelope (its
    // binding-taint 500), which this byte-for-byte comparison pins.
    arms.compare(
        "POST",
        "/api/codex/claim",
        Some(r#"{"species_name": "\ud800", "rank": 1, "skill_name": "Anatomy"}"#),
    )
    .await;

    // A lone-surrogate meta attribute, likewise.
    arms.compare(
        "POST",
        "/api/codex/meta/claim",
        Some(r#"{"attribute_name": "\ud800"}"#),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_config_write_surrogate_input_conforms() {
    // tag-lock in tag mode: a surrogate tag passes the gate and the empty
    // check, then reaches the settings.json write the reference cannot
    // encode. The native arm must produce the same envelope.
    let (_u, _c, arms) = boot(r#"{"mob_tracking_mode": "tag"}"#).await;
    arms.compare(
        "POST",
        "/api/tracking/tag-lock",
        Some(r#"{"tag": "\ud800"}"#),
    )
    .await;

    // manual-mob-lock in mob mode: a surrogate species reaches the catalogue
    // lookup; the arms must agree on its envelope.
    let (_u2, _c2, arms2) = boot("{}").await;
    arms2
        .compare(
            "POST",
            "/api/tracking/manual-mob-lock",
            Some(r#"{"species": "\ud800", "maturity": ""}"#),
        )
        .await;
}
