//! Dual-arm shadow-diff over the no-golden hydration read surface.
//!
//! The analytics / character / equipment / settings hydration GET routes
//! carry no committed golden (they are the oracle's conceded-blind read
//! surface), so a numeric divergence between the native arm and the Python
//! arm could ship unseen. This harness actively watches that surface: for
//! every enumerated no-golden GET route it
//!   1. verifies the route is SIDE-EFFECT-FREE on the Python arm (the DB
//!      snapshot is byte-identical across the request), the precondition
//!      for safely dual-issuing a read, and
//!   2. issues the SAME request to both arms over the SAME database and
//!      diffs the NORMALISED response (status, the contract headers, and the
//!      body), canonicalised through the shared Normalizer so per-run
//!      UUIDs/timestamps/float-forms cannot create a false divergence.
//!
//! A residual normalised divergence that is not on the explicit allow-list
//! fails the run; a real one is investigated as a genuine divergence.
//!
//! Both arms read a read-only COPY of the user's real database, not the live
//! file, so the diff is on logic against a stable
//! substrate. The two lazy-materialising character routes (prospect,
//! prospect-options) are warmed up once before the side-effect snapshot, as
//! their first read against an un-materialised DB writes the summary cache.
//!
//! Why offline and not the running hybrid: the substrate's arm selector is
//! route-keyed and process-global (one arm serves a route for all callers);
//! there is no per-request arm toggle, and adding one would be a production
//! change this work avoids. So the harness reconstructs the native read arm
//! in-process (HydrationState over the same DB) and spawns the Python arm as
//! a sidecar, exactly as the conformance batteries already do.
//!
//! Gated behind `cross-language` (needs the Python interpreter + backend
//! package). Run it with:
//!   cargo test -p eo-http --features cross-language \
//!     --test blind_surface_shadow_diff -- --nocapture
//!
//! The full real-data run only fires when a real DB is present at
//! `EO_SOAK_REAL_DB` (default `<repo>/data/entropia_orme.db`, the seeded
//! local database); absent it skips, so hermetic CI stays green while the local
//! soak instrument runs against real data.
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
use eo_services::game_data_store::GameDataStore;
use eo_wire::db_snapshot::{capture, serialize};
use eo_wire::normalizer::Normalizer;
use http_body_util::BodyExt;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

/// Frozen instant for BOTH arms: the analytics overview reads the injected
/// clock (period filter + 30d-vs-prior trend banding), so the native
/// `MockClock` and the sidecar's `ENTROPIA_TEST_CLOCK_START` must match for
/// the windows to fall identically.
const CLOCK: &str = "2026-06-01T12:00:00";

/// The enumerated no-golden hydration GET surface:
/// 5 analytics + 10 character + 3 equipment + 2 settings = 20 routes. Query
/// params pick a representative path; both arms get the identical request,
/// so even a shared 4xx is a valid agreement check.
const ROUTES: &[&str] = &[
    // analytics (5)
    "/api/analytics/overview?period=all",
    "/api/analytics/activity",
    "/api/analytics/ledger",
    "/api/analytics/ledger/presets",
    "/api/analytics/inventory",
    // character (10)
    "/api/character/calibration",
    "/api/character/stats",
    "/api/character/skills",
    "/api/character/professions",
    "/api/character/prospect-options",
    "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=10",
    "/api/character/profession-optimizer?profession=BLP%20Sniper%20(Hit)",
    "/api/character/profession-path-optimizer?profession=BLP%20Sniper%20(Hit)&target_level=10",
    "/api/character/hp-optimizer",
    "/api/character/codex",
    // equipment (3)
    "/api/equipment/search?q=opalo",
    "/api/equipment/library",
    "/api/equipment/library/1/detail",
    // settings (2)
    "/api/settings",
    "/api/settings/overlay-position",
];

/// Routes whose FIRST read against an un-materialised DB lazily writes the
/// session-summary cache (session_summary.py:288-291). Warmed up once before
/// the side-effect probe so the probe sees the converged, read-only state. A
/// `SUMMARY_VERSION` bump re-arms the write (re-walk these on a bump).
const LAZY_WRITE_ROUTES: &[&str] = &[
    "/api/character/prospect-options",
    "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=10",
];

/// Known, justified divergence classes (route, reason). A divergence on a
/// listed route is reported but does not fail the run; the bar is "zero
/// UNEXPLAINED divergences". Empty: the conformance
/// batteries already prove these arms equivalent on synthetic data, so a
/// real-data divergence is expected to be a genuine finding to triage, not
/// a pre-blessed one.
const ALLOW_LIST: &[(&str, &str)] = &[];

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

/// The real DB to diff against: `EO_SOAK_REAL_DB` or the seeded local database.
fn real_db_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_SOAK_REAL_DB") {
        return PathBuf::from(explicit);
    }
    repo_root().join("data/entropia_orme.db")
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

/// Copy the real local database (entropia_orme.db plus its siblings) into a
/// throwaway data dir, leaving the real file untouched.
fn prepare_real_db_dir() -> tempfile::TempDir {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
    let src_dir = real_db_path()
        .parent()
        .expect("real db has a parent dir")
        .to_path_buf();
    for name in ["entropia_orme.db", "nexus_cache.db", "settings.json"] {
        let src = src_dir.join(name);
        if src.exists() {
            std::fs::copy(&src, data_dir.path().join(name)).expect("copy database file");
        }
    }
    data_dir
}

/// Bring the copied DB forward to the backend's CURRENT schema.
///
/// A real database may predate a column the current schema defines only in
/// its fresh `CREATE TABLE` rather than in a versioned migration: a DB can
/// stamp `db_metadata.version = 33` yet lack `codex_claims.kind`/
/// `attribute_name` (added to the create statement, never via an ALTER), so
/// the version-gated migrations never add them. Both arms expect those
/// columns. Adding them idempotently with the schema's own `DEFAULT 'rank'`
/// brings the copy to the current schema; a copy that already has them no-ops
/// on the duplicate.
async fn reconcile_to_current_schema(db_path: &Path) {
    let pool = open_pool(db_path.to_path_buf()).await;
    for stmt in [
        "ALTER TABLE codex_claims ADD COLUMN kind TEXT NOT NULL DEFAULT 'rank'",
        "ALTER TABLE codex_claims ADD COLUMN attribute_name TEXT",
    ] {
        // A duplicate-column error means the copy is already current-lineage;
        // any other failure surfaces loudly at the first snapshot/GET.
        let _ = sqlx::query(stmt).execute(&pool).await;
    }
    pool.close().await;
}

/// Boot a sidecar over a prepared data dir, clock frozen at `CLOCK`.
fn spawn_sidecar(data_dir: tempfile::TempDir) -> Sidecar {
    let port = free_port();
    let mut command = Command::new(oracle_python());
    command
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
        .env("ENTROPIA_TEST_CLOCK_START", CLOCK)
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

async fn request(port: u16, path: &str) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost")
        .body(Body::empty())
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

async fn open_pool(path: PathBuf) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open shared database")
}

/// The canonical db_state fingerprint of a SQLite file, used as the
/// side-effect probe: identical before/after a read means the read wrote
/// nothing.
async fn snapshot_of(db_path: &Path) -> String {
    let db = eo_services::db::Db::open(db_path)
        .await
        .expect("open db for snapshot");
    let rows = db.snapshot_rows().await.expect("snapshot rows");
    let mut normalizer = Normalizer::new();
    serialize(&capture(&rows, &mut normalizer))
}

/// Canonicalise a response body through the shared Normalizer (UUID /
/// timestamp symbolisation, ties-to-even 4dp float rounding, sorted keys),
/// so only a genuine semantic divergence survives. A fresh Normalizer per
/// body grows symbol tables identically for structurally-equal bodies.
/// Non-JSON bodies fall back to lossy text (an error body still diffs).
fn normalise_body(body: &[u8]) -> String {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => Normalizer::new().normalize_to_compact_json(&value),
        Err(_) => String::from_utf8_lossy(body).to_string(),
    }
}

fn is_allow_listed(route: &str) -> Option<&'static str> {
    ALLOW_LIST
        .iter()
        .find(|(r, _)| *r == route)
        .map(|(_, reason)| *reason)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_no_golden_hydration_surface_shadow_diffs_clean() {
    let real_db = real_db_path();
    if !real_db.exists() {
        eprintln!(
            "[shadow-diff] real database absent at {real_db:?}; skipping the real-data \
             shadow-diff (set EO_SOAK_REAL_DB to a copy of the real DB). The conformance \
             batteries cover these routes on synthetic data in hermetic CI."
        );
        return;
    }

    let data_dir = prepare_real_db_dir();
    let db_file = data_dir.path().join("entropia_orme.db");
    reconcile_to_current_schema(&db_file).await;
    let sidecar = spawn_sidecar(data_dir);
    wait_healthy(sidecar.port).await;

    // Native arm: HydrationState over the SAME copied DB, clock matched.
    let pool = open_pool(db_file.clone()).await;
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let naive = NaiveDateTime::parse_from_str(CLOCK, "%Y-%m-%dT%H:%M:%S").expect("clock parses");
    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        Arc::new(MockClock::new(Some(naive), 0.0)),
        sidecar.data_dir.path().to_path_buf(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind substrate");
    listener.set_nonblocking(true).expect("nonblocking");
    let substrate_port = listener.local_addr().expect("addr").port();
    let state = Arc::new(
        AppState::new(
            format!("127.0.0.1:{}", sidecar.port),
            substrate_port,
            ArmOverrides::empty(),
        )
        .with_hydration(hydration)
        .with_cors(CorsConfig::new(5173, None)),
    );
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, state).await.expect("serve");
    });
    wait_healthy(substrate_port).await;

    // Warm up the lazy-materialising routes against the Python arm so the
    // session-summary cache is converged before the side-effect probe.
    for path in LAZY_WRITE_ROUTES {
        let _ = request(sidecar.port, path).await;
    }

    let mut failures: Vec<String> = Vec::new();
    // The post-warm-up baseline: nothing in the read loop should change it
    // (the native arm is read-only; pure-read Python GETs write nothing).
    let mut prev_snapshot = snapshot_of(&db_file).await;

    for route in ROUTES {
        // Side-effect probe on the Python arm: snapshot AFTER this GET and
        // assert it equals the prior snapshot (the GET wrote nothing).
        let (py_status, py_headers, py_body) = request(sidecar.port, route).await;
        let after = snapshot_of(&db_file).await;
        let side_effect_free = after == prev_snapshot;
        prev_snapshot = after;

        // Dual-arm diff against the native arm over the same DB.
        let (nat_status, nat_headers, nat_body) = request(substrate_port, route).await;
        let py_norm = normalise_body(&py_body);
        let nat_norm = normalise_body(&nat_body);
        let status_match = py_status == nat_status;
        let header_match = contract_axes(&py_headers) == contract_axes(&nat_headers);
        let body_match = py_norm == nat_norm;
        let diverged = !(status_match && header_match && body_match);
        let allow = is_allow_listed(route);

        eprintln!(
            "[shadow-diff] {route}\n    side-effect-free: {side_effect_free}\n    \
             status: py={py_status} native={nat_status} (match={status_match})\n    \
             contract-headers-match: {header_match}\n    normalised-body-match: {body_match}{}",
            allow
                .map(|r| format!("\n    allow-listed: {r}"))
                .unwrap_or_default(),
        );

        if !side_effect_free {
            failures.push(format!(
                "{route}: SIDE EFFECT on the Python arm (db_state changed across the GET)"
            ));
        }
        if diverged && allow.is_none() {
            failures.push(format!(
                "{route}: UNEXPLAINED DIVERGENCE (status_match={status_match} \
                 header_match={header_match} body_match={body_match})\n  python: {py_norm}\n  \
                 native: {nat_norm}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "blind-surface shadow-diff found {} failure(s):\n{}",
        failures.len(),
        failures.join("\n"),
    );
    eprintln!(
        "[shadow-diff] all {} no-golden hydration routes: side-effect-free + \
         zero unexplained divergences",
        ROUTES.len()
    );
}

/// Detection-power negative control (no sidecar needed): the normalise-and-
/// diff must FLAG a genuine value divergence, and must NOT flag a pure
/// timestamp/UUID difference (which normalisation symbolises away). A harness
/// that cannot tell these apart would pass vacuously.
#[test]
fn the_shadow_diff_detects_a_value_divergence_but_tolerates_symbolised_fields() {
    // Genuine value divergence: must be detected.
    let a = br#"{"returnPct": 95.5, "kills": 120}"#;
    let b = br#"{"returnPct": 96.5, "kills": 120}"#;
    assert_ne!(
        normalise_body(a),
        normalise_body(b),
        "the diff must DETECT a real numeric divergence"
    );

    // Same structure, differing UUID + timestamp only: must compare EQUAL
    // after normalisation (encounter-order symbolisation), else live reads
    // would false-positive on per-run ids/instants.
    let c =
        br#"{"id": "11111111-1111-4111-8111-111111111111", "at": "2026-06-01T12:00:00Z", "n": 3}"#;
    let d =
        br#"{"id": "22222222-2222-4222-8222-222222222222", "at": "2025-01-15T08:30:00Z", "n": 3}"#;
    assert_eq!(
        normalise_body(c),
        normalise_body(d),
        "the diff must TOLERATE symbolised UUID/timestamp fields"
    );
}
