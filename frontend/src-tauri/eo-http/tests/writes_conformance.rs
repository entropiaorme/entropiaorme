//! Write-route conformance through the PUBLIC PORT: the same write
//! sequence drives BOTH arms from identical starting states (the
//! substrate's native arm over its own backend's database, and a
//! second, independent backend as the comparison arm over its own),
//! comparing after every step:
//! - the RESPONSE, byte-for-byte where it carries no wall-clock field
//!   and value-compared with the clock fields masked where it does;
//! - the DATABASE STATE, captured through the snapshot catalogue with
//!   a fresh normaliser per arm (encounter-order symbolisation makes
//!   per-arm wall-clock stamps comparable when the write sequences
//!   match).
//!
//! Error legs (validation envelopes, not-found, the calibrate bound,
//! the deliberate storage-range 500s) mutate nothing and compare
//! response-only.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test writes_conformance
#![cfg(feature = "cross-language")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use eo_http::arms::ArmOverrides;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::RealClock;
use eo_services::game_data_store::GameDataStore;
use eo_wire::db_snapshot::{capture, serialize};
use eo_wire::normalizer::Normalizer;
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

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

fn spawn_sidecar() -> Sidecar {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
    let port = free_port();
    let mut command = Command::new(oracle_python());
    command
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
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
    request_raw(
        port,
        method,
        path,
        body.map(|payload| payload.as_bytes().to_vec()),
        body.map(|_| "application/json"),
    )
    .await
}

/// The fully-general form: raw body bytes and an explicit content
/// type (the encoding and content-type probes need both).
async fn request_raw(
    port: u16,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
    content_type: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost");
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let request = match body {
        Some(payload) => builder.body(Body::from(payload)).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
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

/// Mask the wall-clock response fields both arms stamp from their own
/// real clocks; nullness must still agree.
fn mask_clocks(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if (key == "startedAt" || key == "cooldownExpiresAt") && !entry.is_null() {
                    *entry = Value::String("<T>".into());
                } else {
                    mask_clocks(entry);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_clocks),
        _ => {}
    }
}

struct Arms {
    substrate_port: u16,
    comparison_port: u16,
    native_db: PathBuf,
    comparison_db: PathBuf,
}

impl Arms {
    /// Drive one request through both arms and compare the responses;
    /// `clocked` switches to value comparison with the wall-clock
    /// fields masked.
    async fn compare(&self, method: &str, path: &str, body: Option<&str>, clocked: bool) {
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
        if clocked {
            let mut native: Value = serde_json::from_slice(&native_body).expect("native parses");
            let mut python: Value = serde_json::from_slice(&cmp_body).expect("python parses");
            mask_clocks(&mut native);
            mask_clocks(&mut python);
            assert_eq!(native, python, "masked body diverged on {method} {path}");
        } else {
            assert_eq!(
                native_body,
                cmp_body,
                "body diverged on {method} {path}\n  native: {}\n  python: {}",
                String::from_utf8_lossy(&native_body),
                String::from_utf8_lossy(&cmp_body),
            );
        }
    }

    /// Capture and compare both databases through the snapshot
    /// catalogue, one fresh normaliser per arm.
    async fn compare_db_state(&self, step: &str) {
        let native = snapshot_of(&self.native_db).await;
        let comparison = snapshot_of(&self.comparison_db).await;
        assert_eq!(native, comparison, "database state diverged after {step}");
    }
}

async fn snapshot_of(db_path: &Path) -> String {
    let db = eo_services::db::Db::open(db_path)
        .await
        .expect("open db for snapshot");
    let rows = db.snapshot_rows().await.expect("snapshot rows");
    let mut normalizer = Normalizer::new();
    mask_manual_session_keys(&serialize(&capture(&rows, &mut normalizer)))
}

/// A manual quest completion keys its session as `manual-<uuid4>`,
/// random on both arms by design (the composite form is outside the
/// symboliser's whole-string uuid mapping); the comparison masks the
/// random tail.
fn mask_manual_session_keys(snapshot: &str) -> String {
    let mut out = String::with_capacity(snapshot.len());
    let mut rest = snapshot;
    while let Some(found) = rest.find("manual-") {
        let tail_start = found + "manual-".len();
        out.push_str(&rest[..tail_start]);
        rest = &rest[tail_start..];
        let uuid_len = rest
            .char_indices()
            .take_while(|(i, c)| *i < 36 && (c.is_ascii_hexdigit() || *c == '-'))
            .count();
        if uuid_len == 36 {
            out.push_str("<U>");
            rest = &rest[36..];
        }
    }
    out.push_str(rest);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_surface_conforms_through_the_public_port() {
    // Arm B: the backend behind the substrate (its database is the
    // native arm's database). Arm A: the independent comparison
    // backend.
    let upstream = spawn_sidecar();
    let comparison = spawn_sidecar();
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let native_db = upstream.data_dir.path().join("entropia_orme.db");
    let pool: SqlitePool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&native_db)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open the shared database");
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        Arc::new(RealClock::new()),
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
        native_db,
        comparison_db: comparison.data_dir.path().join("entropia_orme.db"),
    };

    // ── The write sequence, state-compared after each step ──
    // Creates: minimal, lax-coerced, and a zero-cooldown quest for the
    // lifecycle legs.
    arms.compare("POST", "/api/quests", Some("{\"name\": \"Alpha\"}"), false)
        .await;
    arms.compare_db_state("minimal create").await;
    arms.compare(
        "POST",
        "/api/quests",
        Some(
            "{\"name\": \"Beta\", \"planet\": \"Arkadia\", \"category\": \"hunt\", \
             \"cooldown_hours\": \"24.0\", \"reward_ped\": \"1_0.5\", \
             \"reward_is_skill\": \"yes\", \"expected_reward_markup_percent\": 150, \
             \"notes\": \"caf\u{e9}\", \"chain_position\": 2.0, \"mobs\": [\"Atrox\", \"Snable\"]}",
        ),
        false,
    )
    .await;
    arms.compare_db_state("lax-coerced create").await;
    arms.compare(
        "POST",
        "/api/quests",
        Some("{\"name\": \"Cycle\", \"cooldown_hours\": 0, \"reward_ped\": 2.5}"),
        false,
    )
    .await;
    arms.compare_db_state("zero-cooldown create").await;

    // Playlist with nested items over the created quests (ids 1-3 on
    // both arms: identical starting states).
    arms.compare(
        "POST",
        "/api/quests/playlists",
        Some(
            "{\"name\": \"Run\", \"estimated_minutes\": \"45\", \"quest_ids\": [1, \"2\"], \
             \"items\": [{\"quest_id\": 3, \"description\": \"finisher\", \
             \"group_type\": \"long_horizon\"}]}",
        ),
        false,
    )
    .await;
    arms.compare_db_state("playlist create").await;

    // Updates: subset, present-null, and the playlist counterpart.
    arms.compare(
        "PUT",
        "/api/quests/1",
        Some("{\"notes\": \"updated\", \"reward_ped\": null, \"mobs\": [\"Atrox\"]}"),
        false,
    )
    .await;
    arms.compare_db_state("quest update").await;
    arms.compare(
        "PUT",
        "/api/quests/playlists/1",
        Some("{\"name\": \"Run 2\", \"estimated_minutes\": 50}"),
        false,
    )
    .await;
    arms.compare_db_state("playlist update").await;

    // Lifecycle on the zero-cooldown quest: start stamps a wall clock
    // (masked), complete and cancel settle back to byte-comparable
    // shapes.
    arms.compare("POST", "/api/quests/3/start", None, true)
        .await;
    arms.compare_db_state("start").await;
    arms.compare("POST", "/api/quests/3/complete", None, false)
        .await;
    arms.compare_db_state("complete").await;
    arms.compare("POST", "/api/quests/3/start", None, true)
        .await;
    arms.compare(
        "POST",
        "/api/quests/3/cancel",
        Some("{\"undo_reward\": false}"),
        false,
    )
    .await;
    arms.compare_db_state("cancel").await;

    // Calibrate writes codex progress.
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"1\", \"rank\": 7}"),
        false,
    )
    .await;
    arms.compare_db_state("calibrate").await;

    // Deletes.
    arms.compare("DELETE", "/api/quests/playlists/1", None, false)
        .await;
    arms.compare_db_state("playlist delete").await;
    arms.compare("DELETE", "/api/quests/2", None, false).await;
    arms.compare_db_state("quest delete").await;

    // ── Error legs (no mutation; response-only comparison) ──
    for (method, path, body) in [
        // Body-validation envelopes through the real routes.
        ("POST", "/api/quests", Some("")),
        ("POST", "/api/quests", Some("{not json")),
        ("POST", "/api/quests", Some("{\"name\": \"Q\", }")),
        ("POST", "/api/quests", Some("{}")),
        ("POST", "/api/quests", Some("{\"name\": 7}")),
        ("POST", "/api/quests", Some("{\"name\": null}")),
        ("POST", "/api/quests", Some("[1, 2]")),
        ("POST", "/api/quests", Some("\"hello\"")),
        (
            "POST",
            "/api/quests",
            Some("{\"name\": 7, \"reward_ped\": \"x\", \"reward_is_skill\": \"enabled\", \"mobs\": 3}"),
        ),
        ("POST", "/api/quests", Some("{\"name\": \"Q\", \"chain_position\": 2.5}")),
        ("POST", "/api/quests", Some("{\"name\": \"Q\", \"mobs\": [\"A\", 5]}")),
        (
            "POST",
            "/api/quests/playlists",
            Some("{\"name\": \"P\", \"items\": [{\"description\": \"d\"}]}"),
        ),
        (
            "POST",
            "/api/quests/playlists",
            Some("{\"name\": \"P\", \"items\": [{\"quest_id\": \"x\"}]}"),
        ),
        ("POST", "/api/quests/playlists", Some("{\"name\": \"P\", \"items\": [7]}")),
        // Unknown ids and the calibrate bound.
        ("PUT", "/api/quests/424242", Some("{\"name\": \"Z\"}")),
        ("DELETE", "/api/quests/424242", None),
        ("GET", "/api/quests/424242", None),
        ("POST", "/api/quests/424242/start", None),
        ("POST", "/api/quests/424242/complete", None),
        ("POST", "/api/quests/424242/cancel", None),
        ("PUT", "/api/quests/playlists/424242", Some("{\"name\": \"Z\"}")),
        ("DELETE", "/api/quests/playlists/424242", None),
        (
            "POST",
            "/api/codex/calibrate",
            Some("{\"species_name\": \"1\", \"rank\": 26}"),
        ),
        (
            "POST",
            "/api/codex/calibrate",
            Some("{\"species_name\": \"1\", \"rank\": -2}"),
        ),
        (
            "POST",
            "/api/codex/calibrate",
            Some("{\"species_name\": \"1\", \"rank\": 999999999999999999999999}"),
        ),
        // Path-parameter legs.
        ("GET", "/api/quests/abc", None),
        ("PUT", "/api/quests/abc", Some("{\"name\": \"Z\"}")),
        ("GET", "/api/quests/999999999999999999999999", None),
        ("POST", "/api/quests/A%2FB/start", None),
        // The non-finite create reads back null end to end.
        ("POST", "/api/quests", Some("{\"name\": \"InfQ\", \"reward_ped\": \"inf\"}")),
        ("GET", "/api/quests", None),
    ] {
        // The last two probes mutate identically on both arms; the
        // sequenced comparison still holds because both arms see the
        // same request exactly once.
        arms.compare(method, path, body, false).await;
    }
    arms.compare_db_state("error legs + non-finite create")
        .await;

    // The adversarial-form grid (validation taxonomy, declaration
    // order, render-crash 500s; none of these mutate).
    let lone_surrogate_body = "{\"planet\": \"\\ud800\"}";
    for (method, path, body) in [
        // Multi-error issues list in model declaration order.
        (
            "PUT",
            "/api/quests/1",
            r#"{"reward_description": 5, "cooldown_hours": "x"}"#,
        ),
        (
            "PUT",
            "/api/quests/1",
            r#"{"expected_reward_markup_percent": "x", "reward_is_skill": "zz"}"#,
        ),
        // The bool taxonomy split.
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": null}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": 1.5}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": [1]}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": 2.0}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": 2}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "B", "reward_is_skill": 999999999999999999999999}"#,
        ),
        // Beyond-i64 floats into int fields: the size 422 (both exact
        // bounds excluded); digit strings stay the storage 500.
        (
            "POST",
            "/api/quests",
            r#"{"name": "I", "chain_position": 1e30}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "I", "chain_position": 9223372036854775808.0}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "I", "chain_position": -9223372036854775808.0}"#,
        ),
        (
            "POST",
            "/api/quests/playlists",
            r#"{"name": "P", "items": [{"quest_id": 1e19}]}"#,
        ),
        // The render-crash 500s: non-finite and lone-surrogate echoes.
        ("POST", "/api/quests", r#"{"name": Infinity}"#),
        ("POST", "/api/quests", r#"{"chain_position": Infinity}"#),
        ("POST", "/api/quests", lone_surrogate_body),
        // Float underscore gate: rejected forms.
        (
            "POST",
            "/api/quests",
            r#"{"name": "F", "reward_ped": "_1.5"}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "F", "reward_ped": "1.5_"}"#,
        ),
        (
            "POST",
            "/api/quests",
            r#"{"name": "F", "reward_ped": "1__0.5"}"#,
        ),
    ] {
        arms.compare(method, path, Some(body), false).await;
    }

    // Top-level null bodies: ABSENT to the model binding (missing on
    // required-body routes, no-body semantics on cancel).
    for (method, path) in [
        ("POST", "/api/quests"),
        ("PUT", "/api/quests/1"),
        ("POST", "/api/quests/424242/cancel"),
    ] {
        arms.compare(method, path, Some("null"), false).await;
    }

    // The deep-echo render crash (both arms 500 past the reference's
    // render limit) and the parity at its last passing depth.
    let deep_value =
        |depth: usize| format!(r#"{{"name": {}{}}}"#, "[".repeat(depth), "]".repeat(depth));
    arms.compare("POST", "/api/quests", Some(&deep_value(984)), false)
        .await;
    arms.compare("POST", "/api/quests", Some(&deep_value(990)), false)
        .await;

    // Content-type gating: a non-application maintype with a +json
    // suffix is NOT JSON to the backend (raw-string echo), while
    // application subtypes match case-insensitively (those two create,
    // identically on both arms).
    for (ct, body) in [
        ("text/whatever+json", r#"{"name": "TW"}"#),
        ("application/hal+JSON", r#"{"name": "HAL"}"#),
        ("Application/JSON", r#"{"name": "CASEY"}"#),
    ] {
        let (ns, _, nb) = request_raw(
            arms.substrate_port,
            "POST",
            "/api/quests",
            Some(body.as_bytes().to_vec()),
            Some(ct),
        )
        .await;
        let (cs, _, cb) = request_raw(
            arms.comparison_port,
            "POST",
            "/api/quests",
            Some(body.as_bytes().to_vec()),
            Some(ct),
        )
        .await;
        assert_eq!(ns, cs, "content-type {ct} status");
        assert_eq!(nb, cb, "content-type {ct} body");
    }

    // Body-encoding detection: UTF-16 and BOM-prefixed bodies parse
    // (and create, identically); invalid UTF-8 answers the generic
    // body-parse 400.
    let utf16: Vec<u8> = r#"{"name": "U16"}"#.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let bom = [vec![0xEF, 0xBB, 0xBF], br#"{"name": "BOM"}"#.to_vec()].concat();
    let bad_utf8 = [br#"{"name": ""#.to_vec(), vec![0xFF], br#""}"#.to_vec()].concat();
    for (bytes, label) in [
        (utf16, "utf-16-le"),
        (bom, "utf-8 bom"),
        (bad_utf8, "invalid utf-8"),
    ] {
        let (ns, _, nb) = request_raw(
            arms.substrate_port,
            "POST",
            "/api/quests",
            Some(bytes.clone()),
            Some("application/json"),
        )
        .await;
        let (cs, _, cb) = request_raw(
            arms.comparison_port,
            "POST",
            "/api/quests",
            Some(bytes),
            Some("application/json"),
        )
        .await;
        assert_eq!(ns, cs, "encoding {label} status");
        assert_eq!(nb, cb, "encoding {label} body");
    }

    // Accepted underscore floats create identically on both arms.
    for raw in ["1_.5", "1._5", "1e_5", "+_1"] {
        let body = format!(r#"{{"name": "UF{raw}", "reward_ped": "{raw}"}}"#);
        arms.compare("POST", "/api/quests", Some(&body), false)
            .await;
    }

    // The bool float-coercion window: integral floats beyond +/-2^63
    // answer the type error, not the parsing one.
    for value in ["1e30", "9223372036854775808.0"] {
        let body = format!(r#"{{"name": "B", "reward_is_skill": {value}}}"#);
        arms.compare("POST", "/api/quests", Some(&body), false)
            .await;
    }

    // Lone surrogates resolve at consumption on both arms: ignored
    // fields write through (200), validated fields answer the binding
    // 500 with nothing written, and the codex ValueError mapping
    // carries the codec message.
    arms.compare(
        "POST",
        "/api/quests",
        Some("{\"name\": \"SurOk\", \"bogus\": \"\\ud800\"}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/quests",
        Some("{\"name\": \"a\\ud800b\"}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"\\ud800\", \"rank\": 7}"),
        false,
    )
    .await;

    // BOM-less UTF-16 with a non-ASCII character: the byte-pair
    // detection must still pick the encoding.
    let unicode_utf16: Vec<u8> = "{\"name\": \"U16\u{3042}\"}"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let (ns, _, nb) = request_raw(
        arms.substrate_port,
        "POST",
        "/api/quests",
        Some(unicode_utf16.clone()),
        Some("application/json"),
    )
    .await;
    let (cs, _, cb) = request_raw(
        arms.comparison_port,
        "POST",
        "/api/quests",
        Some(unicode_utf16),
        Some("application/json"),
    )
    .await;
    assert_eq!(ns, cs, "bom-less unicode utf-16 status");
    assert_eq!(nb, cb, "bom-less unicode utf-16 body");

    // Surrogate consumption ORDER: another field's 422 wins over the
    // taint; calibrate resolves missing/parse 422s, then its rank
    // bound, then the codec message (singular and run forms).
    arms.compare(
        "POST",
        "/api/quests",
        Some("{\"name\": \"a\\ud800b\", \"reward_ped\": \"zz\"}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"\\ud800\", \"rank\": \"x\"}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"\\ud800\"}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"\\ud800\", \"rank\": 26}"),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/calibrate",
        Some("{\"species_name\": \"\\ud800\\ud801x\", \"rank\": 7}"),
        false,
    )
    .await;
    arms.compare_db_state("adversarial grid").await;

    // The multi-statement surrogate residual: both arms answer the
    // binding 500, but the reference leaves the parent quest row
    // PENDING in its connection's open transaction (its API shows the
    // row at once; the next commit on that connection ratifies it
    // durably) while the native arm writes nothing (the register's
    // recorded state divergence). The probe is response-only and runs
    // after the last state comparison because any later commit on the
    // reference arm materialises the row.
    arms.compare(
        "POST",
        "/api/quests",
        Some("{\"name\": \"MobSur\", \"mobs\": [\"a\\ud800\"]}"),
        false,
    )
    .await;

    // The deferral: codex claim and meta-claim stay proxied (the
    // sidecar's server header proves the arm) and answer identically.
    let (status, headers, _) = request_raw(
        arms.substrate_port,
        "POST",
        "/api/codex/claim",
        Some(br#"{"species_name": "1", "rank": 99, "skill_name": "X"}"#.to_vec()),
        Some("application/json"),
    )
    .await;
    assert!(
        headers.contains_key(http::header::SERVER),
        "codex claim stays on the proxy arm (deferred with the producer cutover)"
    );
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    arms.compare(
        "POST",
        "/api/codex/claim",
        Some(r#"{"species_name": "1", "rank": 99, "skill_name": "X"}"#),
        false,
    )
    .await;
    arms.compare(
        "POST",
        "/api/codex/meta/claim",
        Some(r#"{"attribute_name": "Nope"}"#),
        false,
    )
    .await;

    // Nesting beyond both parsers' limits answers the backend's
    // generic body-parse 400 on both arms.
    let deep = "[".repeat(50_000) + &"]".repeat(50_000);
    arms.compare("POST", "/api/quests", Some(&deep), false)
        .await;
}
