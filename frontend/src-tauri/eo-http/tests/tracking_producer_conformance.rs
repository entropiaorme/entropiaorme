//! Tracking PRODUCER-route conformance through the PUBLIC PORT: the three
//! non-config-writing producer routes proven against the running Python
//! sidecar.
//!
//!   POST /api/tracking/start                  -> TrackingStartResult (plain)
//!   POST /api/tracking/stop                   -> TrackingStopResult (plain)
//!   GET  /api/tracking/manual-mob-suggestions -> list[ManualMobSuggestion]
//!        (ETag-scoped on the 200 legs; the tag-mode 409 legs are plain)
//!
//! TOPOLOGY: the producer routes touch LIVE state (the in-memory tracker
//! session, the DB-backed start/stop) and the bundled catalogue, so the
//! comparison cannot be a single byte-for-byte read like the session-read
//! battery. Instead:
//!
//! - manual-mob-suggestions is a deterministic READ over the catalogue +
//!   the live config: both arms read the SAME catalogue and an identical
//!   `settings.json`, so its 200/[]/409/422 legs ARE compared
//!   byte-for-byte (status, contract headers, body), including the 304 leg.
//!
//! - start/stop carry a random `session_id` (uuid4) and clock-derived
//!   `started_at` / `ended_at`, so they can never be byte-identical. They
//!   are compared STRUCTURALLY (the field set + value types) and by
//!   SESSION-ROW DB-STATE PARITY (the `tracking_sessions` row each arm
//!   writes: is_active flips 1->0, started_at/ended_at populate,
//!   mob_tracking_mode is captured), plus the guard codes/bodies
//!   (409 already-active, 409 no-active-session). The native arm writes to
//!   the upstream sidecar's shared database; the comparison Python arm
//!   writes to its own; each arm's row is read back and the structural
//!   state compared. Only the arm under test produces (the upstream
//!   sidecar's tracking routes are never driven), so the shared DB carries
//!   exactly the native arm's writes.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test tracking_producer_conformance
#![cfg(feature = "cross-language")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use chrono::NaiveDateTime;
use eo_http::arms::ArmOverrides;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::MockClock;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use eo_services::tracker::{HuntTracker, Providers};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

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
/// `settings.json`. The clock is frozen at `CLOCK` (so the native arm's
/// `MockClock` at the same instant keeps the topology robust); the
/// producer-idle gate stands its producers down so its own tracker never
/// races the arm under test for the chat log.
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
        // Stand the sidecar's own producers down: the routes still serve,
        // but no chat-log production runs, so the arm under test owns the
        // session lifecycle without a parallel producer mutating the DB.
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
    if_none_match: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        // Mutating methods need an allowed origin at the substrate guard;
        // reads tolerate it too, so send it unconditionally.
        .header("origin", "tauri://localhost");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(value) = if_none_match {
        builder = builder.header("if-none-match", value);
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

struct Arms {
    substrate_port: u16,
    comparison_port: u16,
    native_db: PathBuf,
    comparison_db: PathBuf,
}

impl Arms {
    /// Drive a request through both arms; assert status + contract headers
    /// match and the bodies are byte-identical (the suggestions surface
    /// carries no per-arm random ids or clock fields). Returns the native
    /// status so a call site can pin the absolute code too.
    async fn compare(&self, method: &str, path: &str, body: Option<&str>) -> http::StatusCode {
        let (native_status, native_headers, native_body) =
            request(self.substrate_port, method, path, body, None).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, method, path, body, None).await;
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

    /// The conditional-GET 304 leg on an ETag-scoped 200 read.
    async fn compare_conditional(&self, path: &str) {
        let (_, native_headers, _) = request(self.substrate_port, "GET", path, None, None).await;
        let (_, cmp_headers, _) = request(self.comparison_port, "GET", path, None, None).await;
        let native_etag = native_headers
            .get(http::header::ETAG)
            .expect("native etag")
            .to_str()
            .unwrap()
            .to_string();
        let cmp_etag = cmp_headers
            .get(http::header::ETAG)
            .expect("python etag")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(native_etag, cmp_etag, "etag diverged on {path}");

        let (native_status, native_headers, native_body) =
            request(self.substrate_port, "GET", path, None, Some(&native_etag)).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, "GET", path, None, Some(&cmp_etag)).await;
        assert_eq!(native_status, http::StatusCode::NOT_MODIFIED, "native 304");
        assert_eq!(cmp_status, http::StatusCode::NOT_MODIFIED, "python 304");
        assert!(native_body.is_empty(), "native 304 has no body");
        assert!(cmp_body.is_empty(), "python 304 has no body");
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&cmp_headers),
            "304 contract headers diverged on {path}"
        );
    }
}

/// The structural fingerprint of a start/stop response: which keys are
/// present and each value's JSON type (a uuid `session_id` and an ISO
/// `started_at`/`ended_at` are strings, `kill_count` an integer, `status`
/// a string), so the two arms' shapes compare without the random/clock
/// values themselves. Asserts the literal `status` too.
fn lifecycle_fingerprint(body: &[u8]) -> Value {
    let v: Value = serde_json::from_slice(body).expect("lifecycle body is JSON");
    let object = v.as_object().expect("lifecycle body is an object");
    let mut shape = serde_json::Map::new();
    for (key, value) in object {
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(n) if n.is_i64() || n.is_u64() => "int",
            Value::Number(_) => "float",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        shape.insert(key.clone(), Value::from(kind));
    }
    Value::Object(shape)
}

/// The session-row DB-state fingerprint: the columns start/stop mutate,
/// with the random id and the literal timestamps reduced to presence
/// booleans (each arm's id/clock differs, but the STATE transition is the
/// invariant). One row expected.
async fn session_row_state(db_path: &Path) -> Value {
    let pool = open_pool(db_path).await;
    let rows = sqlx::query(
        "SELECT id, started_at, ended_at, is_active, mob_tracking_mode \
         FROM tracking_sessions ORDER BY started_at",
    )
    .fetch_all(&pool)
    .await
    .expect("read sessions")
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "hasId": !row.get::<String, _>(0).is_empty(),
            "hasStartedAt": row.try_get::<Option<f64>, _>(1).ok().flatten().is_some(),
            "hasEndedAt": row.try_get::<Option<f64>, _>(2).ok().flatten().is_some(),
            "isActive": row.get::<i64, _>(3),
            "mobTrackingMode": row.get::<Option<String>, _>(4),
        })
    })
    .collect::<Vec<_>>();
    Value::Array(rows)
}

/// Build a native substrate over `upstream`'s database with a live
/// HuntTracker (the producer routes' dependency). The tracker shares the
/// substrate's single-owner pool, exactly as composition wires it. The
/// upstream sidecar's own tracker is never driven, so the shared DB
/// carries only the native arm's writes.
async fn boot(config_json: &str) -> (Sidecar, Sidecar, Arms) {
    let upstream = spawn_sidecar(config_json);
    let comparison = spawn_sidecar(config_json);
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let native_db = upstream.data_dir.path().join("entropia_orme.db");
    let pool = open_pool(&native_db).await;
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let naive = NaiveDateTime::parse_from_str(CLOCK, "%Y-%m-%dT%H:%M:%S").expect("clock parses");
    let clock = Arc::new(MockClock::new(Some(naive), 0.0));
    let tracker = HuntTracker::new(
        Arc::new(EventBus::new()),
        pool.clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
        Providers::default(),
    )
    .expect("native tracker builds over the shared pool");
    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        clock,
        upstream.data_dir.path().to_path_buf(),
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
        .with_cors(CorsConfig::new(5173, None)),
    );
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, state).await.expect("serve");
    });
    wait_healthy(substrate_port).await;

    let comparison_db = comparison.data_dir.path().join("entropia_orme.db");
    let arms = Arms {
        substrate_port,
        comparison_port: comparison.port,
        native_db,
        comparison_db,
    };
    (upstream, comparison, arms)
}

const MMS: &str = "/api/tracking/manual-mob-suggestions";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_mob_suggestions_conforms_over_identical_state() {
    // Mob mode (no tag-mode gate): the success/[]/clamp/422 legs.
    let (_upstream, _comparison, arms) = boot("{}").await;

    // Success: a catalogue match (the snapshot is shared, the config is
    // mob mode) -> byte-identical 200 with the strong ETag.
    assert_eq!(
        arms.compare("GET", &format!("{MMS}?q=atrox"), None).await,
        http::StatusCode::OK
    );
    // The empty-q short-circuit -> 200 [] (still ETag-scoped).
    assert_eq!(
        arms.compare("GET", &format!("{MMS}?q="), None).await,
        http::StatusCode::OK
    );
    assert_eq!(arms.compare("GET", MMS, None).await, http::StatusCode::OK);
    // The limit clamp: 99 -> 20, 0/-5 -> 1, 1 -> 1.
    for query in [
        "q=atrox&limit=99",
        "q=atrox&limit=0",
        "q=atrox&limit=-5",
        "q=atrox&limit=1",
    ] {
        assert_eq!(
            arms.compare("GET", &format!("{MMS}?{query}"), None).await,
            http::StatusCode::OK,
            "{query}"
        );
    }
    // A no-match query -> 200 [].
    assert_eq!(
        arms.compare("GET", &format!("{MMS}?q=zzzzzz"), None).await,
        http::StatusCode::OK
    );
    // An unparseable limit -> the byte-identical 422 int_parsing envelope.
    assert_eq!(
        arms.compare("GET", &format!("{MMS}?q=a&limit=abc"), None)
            .await,
        http::StatusCode::UNPROCESSABLE_ENTITY
    );
    // The conditional-GET 304 leg on a 200-bearing read.
    arms.compare_conditional(&format!("{MMS}?q=atrox")).await;
    arms.compare_conditional(&format!("{MMS}?q=")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_mob_suggestions_tag_mode_409_conforms() {
    // Idle tag mode: the live config gates BEFORE the empty-q short-circuit,
    // so even q= 409s. Both legs share one body, neither carries an ETag.
    let (_upstream, _comparison, arms) =
        boot(r#"{"mob_tracking_mode": "tag", "mob_tracking_tag": "Boss"}"#).await;
    for query in ["q=atrox", "q=", ""] {
        let path = if query.is_empty() {
            MMS.to_string()
        } else {
            format!("{MMS}?{query}")
        };
        assert_eq!(
            arms.compare("GET", &path, None).await,
            http::StatusCode::CONFLICT,
            "{path}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_stop_lifecycle_conforms_structurally_and_in_db_state() {
    // Hotbar mode with a bound slot: the attribution gate passes on both
    // arms, so /start succeeds without a configured trifecta.
    let (_upstream, _comparison, arms) =
        boot(r#"{"hotbar_hooks_enabled": true, "hotbar": {"1": 7}}"#).await;

    // ── start: structural parity (session_id/started_at strings, status
    //    literal "active"), and an active row appears in each DB ──
    let (native_status, _, native_start) = request(
        arms.substrate_port,
        "POST",
        "/api/tracking/start",
        Some(""),
        None,
    )
    .await;
    let (cmp_status, _, cmp_start) = request(
        arms.comparison_port,
        "POST",
        "/api/tracking/start",
        Some(""),
        None,
    )
    .await;
    assert_eq!(native_status, http::StatusCode::OK, "native start 200");
    assert_eq!(cmp_status, http::StatusCode::OK, "python start 200");
    assert_eq!(
        lifecycle_fingerprint(&native_start),
        lifecycle_fingerprint(&cmp_start),
        "start response shape diverged\n  native: {}\n  python: {}",
        String::from_utf8_lossy(&native_start),
        String::from_utf8_lossy(&cmp_start),
    );
    let native_started: Value = serde_json::from_slice(&native_start).unwrap();
    assert_eq!(native_started["status"], "active");
    assert!(native_started["session_id"].as_str().is_some());
    // The DB-state transition after start matches.
    assert_eq!(
        session_row_state(&arms.native_db).await,
        session_row_state(&arms.comparison_db).await,
        "session-row state diverged after start"
    );

    // ── start again while active: the byte-identical 409 (no random/clock
    //    fields in the error body) ──
    assert_eq!(
        arms.compare("POST", "/api/tracking/start", Some("")).await,
        http::StatusCode::CONFLICT
    );

    // ── stop: structural parity (session_id/started_at/ended_at strings,
    //    kill_count int) + the is_active 1->0 DB transition ──
    let (native_status, _, native_stop) = request(
        arms.substrate_port,
        "POST",
        "/api/tracking/stop",
        Some(""),
        None,
    )
    .await;
    let (cmp_status, _, cmp_stop) = request(
        arms.comparison_port,
        "POST",
        "/api/tracking/stop",
        Some(""),
        None,
    )
    .await;
    assert_eq!(native_status, http::StatusCode::OK, "native stop 200");
    assert_eq!(cmp_status, http::StatusCode::OK, "python stop 200");
    assert_eq!(
        lifecycle_fingerprint(&native_stop),
        lifecycle_fingerprint(&cmp_stop),
        "stop response shape diverged\n  native: {}\n  python: {}",
        String::from_utf8_lossy(&native_stop),
        String::from_utf8_lossy(&cmp_stop),
    );
    let native_stopped: Value = serde_json::from_slice(&native_stop).unwrap();
    assert_eq!(native_stopped["kill_count"], 0);
    assert_eq!(
        native_stopped["session_id"], native_started["session_id"],
        "stop echoes the started session id"
    );
    assert_eq!(
        session_row_state(&arms.native_db).await,
        session_row_state(&arms.comparison_db).await,
        "session-row state diverged after stop"
    );

    // ── stop with no active session: the byte-identical 409 ──
    assert_eq!(
        arms.compare("POST", "/api/tracking/stop", Some("")).await,
        http::StatusCode::CONFLICT
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_rejects_an_unready_attribution_on_both_arms() {
    // No hotbar, no configured trifecta: the byte-identical trifecta 400.
    let (_upstream, _comparison, arms) = boot("{}").await;
    assert_eq!(
        arms.compare("POST", "/api/tracking/start", Some("")).await,
        http::StatusCode::BAD_REQUEST
    );
    // Hotbar mode, no bound slot: the byte-identical hotbar 400.
    let (_upstream, _comparison, arms) = boot(r#"{"hotbar_hooks_enabled": true}"#).await;
    assert_eq!(
        arms.compare("POST", "/api/tracking/start", Some("")).await,
        http::StatusCode::BAD_REQUEST
    );
}
