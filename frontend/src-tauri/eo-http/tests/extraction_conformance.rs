//! Extraction-layer conformance and registered-route A/B, through the
//! PUBLIC PORT: the substrate (native registrations + proxy fallback +
//! host guard) serves one arm, the running backend the other, over the
//! same database, and every probe compares byte-for-byte on status,
//! content-type, cache-control, etag, and body.
//!
//! Three claims are proven here that the handler-level fidelity test
//! cannot reach:
//! - REGISTRATION: each natively-registered route answers through the
//!   real router (route patterns, extraction, fallback interplay).
//! - VALIDATION CONFORMANCE: the extraction layer reproduces the
//!   backend's validation envelopes over the whole probed grid
//!   (missing / int_parsing / bounds / literal / multi-error /
//!   raw-input re-rendering / duplicate-parameter and decode rules).
//! - THE RUNTIME ARM OVERRIDE: flipping a registered route to the
//!   proxy arm and back changes which implementation answers (the
//!   sidecar's server header appears and disappears) while the
//!   response stays byte-identical.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test extraction_conformance
#![cfg(feature = "cross-language")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use eo_http::arms::ArmOverrides;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::RealClock;
use eo_services::game_data_store::GameDataStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
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
    let child = Command::new(oracle_python())
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn backend sidecar");
    Sidecar {
        child,
        port,
        data_dir,
    }
}

fn client() -> eo_http::proxy::ProxyClient {
    eo_http::proxy::build_client()
}

async fn get(
    port: u16,
    path: &str,
    if_none_match: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority);
    if let Some(tag) = if_none_match {
        builder = builder.header("if-none-match", tag);
    }
    let response = client()
        .request(builder.body(Body::empty()).unwrap())
        .await
        .expect("request succeeds");
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

async fn post(port: u16, path: &str, payload: Value) -> Value {
    let authority = format!("127.0.0.1:{port}");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("content-type", "application/json")
        .header("origin", "tauri://localhost")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = client().request(request).await.expect("post succeeds");
    assert!(
        response.status().is_success(),
        "seed POST {path} failed: {}",
        response.status()
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes()
        .to_vec();
    serde_json::from_slice(&bytes).expect("seed response parses")
}

async fn wait_healthy(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            panic!("backend never became healthy on port {port}");
        }
        let authority = format!("127.0.0.1:{port}");
        let request = http::Request::builder()
            .uri(format!("http://{authority}/api/health"))
            .header("host", &authority)
            .body(Body::empty())
            .unwrap();
        if let Ok(response) = client().request(request).await {
            if response.status() == http::StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The compared contract axes; the proxy arm additionally carries the
/// sidecar's server/date headers, which are not part of the contract.
fn contract_axes(headers: &http::HeaderMap) -> (Option<String>, Option<String>, Option<String>) {
    let value = |name: http::header::HeaderName| {
        headers
            .get(name)
            .map(|v| v.to_str().unwrap_or("<non-utf8>").to_string())
    };
    (
        value(http::header::CONTENT_TYPE),
        value(http::header::CACHE_CONTROL),
        value(http::header::ETAG),
    )
}

async fn assert_substrate_matches_backend(
    substrate_port: u16,
    backend_port: u16,
    path: &str,
    if_none_match: Option<&str>,
) {
    let (native_status, native_headers, native_body) =
        get(substrate_port, path, if_none_match).await;
    let (backend_status, backend_headers, backend_body) =
        get(backend_port, path, if_none_match).await;
    assert_eq!(
        native_status, backend_status,
        "status diverged on {path} (if-none-match: {if_none_match:?})\n  substrate body: {}\n  backend body:   {}",
        String::from_utf8_lossy(&native_body),
        String::from_utf8_lossy(&backend_body),
    );
    assert_eq!(
        contract_axes(&native_headers),
        contract_axes(&backend_headers),
        "contract headers diverged on {path}"
    );
    assert_eq!(
        native_body,
        backend_body,
        "body diverged on {path} (if-none-match: {if_none_match:?})\n  substrate: {}\n  backend:   {}",
        String::from_utf8_lossy(&native_body),
        String::from_utf8_lossy(&backend_body),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_registered_surface_conforms_through_the_public_port() {
    let sidecar = spawn_sidecar();
    wait_healthy(sidecar.port).await;

    // Seed a little data through the backend's own API so the list
    // routes carry rows through the substrate path too.
    let quest = post(
        sidecar.port,
        "/api/quests",
        json!({"name": "Conformance Quest", "mobs": ["Atrox"], "reward_ped": 1.5}),
    )
    .await;
    let quest_id: i64 = quest["id"]
        .as_str()
        .expect("quest id is a string")
        .parse()
        .expect("quest id parses");
    post(
        sidecar.port,
        "/api/quests/playlists",
        json!({"name": "Conformance Playlist", "quest_ids": [quest_id]}),
    )
    .await;

    // The native arm reads the SAME database the backend serves.
    let db_path = sidecar.data_dir.path().join("entropia_orme.db");
    let pool: SqlitePool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open the shared database");
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let hydration = Arc::new(HydrationState::new(
        pool,
        game_data,
        Arc::new(RealClock::new()),
    ));

    // Serve the substrate on its own public port, proxying everything
    // unregistered to the backend.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind substrate");
    listener.set_nonblocking(true).expect("nonblocking");
    let substrate_port = listener.local_addr().expect("addr").port();
    let state = Arc::new(
        AppState::new(
            format!("127.0.0.1:{}", sidecar.port),
            substrate_port,
            ArmOverrides::empty(),
        )
        .with_hydration(hydration),
    );
    let serve_state = state.clone();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, serve_state).await.expect("serve");
    });
    wait_healthy(substrate_port).await;

    // ── The nine registered routes, happy path ──
    for path in [
        "/api/quests",
        "/api/quests/mobs",
        "/api/quests/analytics",
        "/api/quests/playlists",
        "/api/quests/playlists/analytics",
        "/api/codex/species",
        "/api/codex/species/1/ranks",
        "/api/codex/recommend?species_name=1&rank=4",
        "/api/codex/meta/attributes",
    ] {
        assert_substrate_matches_backend(substrate_port, sidecar.port, path, None).await;
    }

    // ── Conditional-GET legs (current and stale validators) ──
    let (_, headers, _) = get(sidecar.port, "/api/quests", None).await;
    let etag = headers
        .get(http::header::ETAG)
        .expect("etag present")
        .to_str()
        .unwrap()
        .to_string();
    for tag in [etag.as_str(), "\"stale\"", "*"] {
        assert_substrate_matches_backend(substrate_port, sidecar.port, "/api/quests", Some(tag))
            .await;
    }

    // ── Not-found legs: handler 404, decoded-space 404, UTF-8 404,
    //    and the route-level 404 a decoded slash produces ──
    for path in [
        "/api/codex/species/No%20Such%20Species/ranks",
        "/api/codex/species/Ber%C3%A7as/ranks",
        "/api/codex/species/Mind%20Essence%2FX/ranks",
    ] {
        assert_substrate_matches_backend(substrate_port, sidecar.port, path, None).await;
    }

    // An encoded slash that decodes INTO an existing route: unmatched
    // by the substrate's raw-path router, it falls back to the proxy,
    // where the backend's decode-then-match serves the real route.
    assert_substrate_matches_backend(substrate_port, sidecar.port, "/api/quests%2Fmobs", None)
        .await;

    // ── The validation grid (every form grounded against the live
    //    backend during authoring; re-proven here on both arms) ──
    let recommend = "/api/codex/recommend";
    let mut probes: Vec<String> = vec![
        // missing / multi-error / ordering
        recommend.to_string(),
        format!("{recommend}?species_name=1"),
        format!("{recommend}?rank=4"),
        format!("{recommend}?rank=abc&target=xx"),
        format!("{recommend}?target=hp"),
        // int_parsing rejections, raw input re-rendered
        format!("{recommend}?species_name=1&rank=abc"),
        format!("{recommend}?species_name=1&rank="),
        format!("{recommend}?species_name=1&rank=4.5"),
        format!("{recommend}?species_name=1&rank=4e0"),
        format!("{recommend}?species_name=1&rank=0x4"),
        format!("{recommend}?species_name=1&rank=%EF%BC%94"), // fullwidth 4
        format!("{recommend}?species_name=1&rank=4."),
        format!("{recommend}?species_name=1&rank=.4"),
        format!("{recommend}?species_name=1&rank=%2B%204"), // "+ 4"
        format!("{recommend}?species_name=1&rank=--4"),
        format!("{recommend}?species_name=1&rank=4_"),
        format!("{recommend}?species_name=1&rank=_4"),
        format!("{recommend}?species_name=1&rank=1__0"),
        format!("{recommend}?species_name=1&rank=4.000000001"),
        // lax acceptances
        format!("{recommend}?species_name=1&rank=05"),
        format!("{recommend}?species_name=1&rank=%2B5"),
        format!("{recommend}?species_name=1&rank=%204%20"),
        format!("{recommend}?species_name=1&rank=%C2%A04"), // NBSP-prefixed
        format!("{recommend}?species_name=1&rank=4.0000"),
        // bounds, raw input re-rendered (incl. beyond-i64 magnitudes)
        format!("{recommend}?species_name=1&rank=0"),
        format!("{recommend}?species_name=1&rank=-0"),
        format!("{recommend}?species_name=1&rank=-3"),
        format!("{recommend}?species_name=1&rank=26"),
        format!("{recommend}?species_name=1&rank=1_0_0"),
        format!("{recommend}?species_name=1&rank=999999999999999999999999"),
        format!("{recommend}?species_name=1&rank=-999999999999999999999999"),
        // literal target
        format!("{recommend}?species_name=1&rank=4&target=xx"),
        format!("{recommend}?species_name=1&rank=4&target=hp"),
        format!("{recommend}?species_name=1&rank=4&target=profession"),
        // duplicate parameters: the last occurrence validates
        format!("{recommend}?species_name=1&rank=3&rank=abc"),
        format!("{recommend}?species_name=1&rank=abc&rank=3"),
        // form decoding in query values: plus is a space
        format!("{recommend}?species_name=Some+Name&rank=4"),
        format!("{recommend}?species_name=Some%20Name&rank=4"),
        // profession is optional free text
        format!("{recommend}?species_name=1&rank=4&profession=Evade"),
    ];
    // Lax underscore grouping accepted by the backend's parser.
    probes.push(format!("{recommend}?species_name=1&rank=1_0"));
    for path in &probes {
        assert_substrate_matches_backend(substrate_port, sidecar.port, path, None).await;
    }

    // ── The runtime arm override on a registered route ──
    // Native arm first: the substrate answers itself (no sidecar
    // server header), byte-identical to the backend.
    let (_, native_headers, _) = get(substrate_port, "/api/quests", None).await;
    assert!(
        !native_headers.contains_key(http::header::SERVER),
        "the native arm answers without the sidecar's server header"
    );
    // Flip to the proxy arm: the sidecar answers (its server header
    // appears) and the response stays byte-identical.
    state.set_overrides(ArmOverrides::parse_env_value("/api/quests=proxy"));
    let (_, proxy_headers, _) = get(substrate_port, "/api/quests", None).await;
    assert!(
        proxy_headers.contains_key(http::header::SERVER),
        "the proxy arm carries the sidecar's server header"
    );
    assert_substrate_matches_backend(substrate_port, sidecar.port, "/api/quests", None).await;
    // Flip back to native.
    state.set_overrides(ArmOverrides::empty());
    let (_, restored_headers, _) = get(substrate_port, "/api/quests", None).await;
    assert!(
        !restored_headers.contains_key(http::header::SERVER),
        "the native arm resumes after the override clears"
    );
    assert_substrate_matches_backend(substrate_port, sidecar.port, "/api/quests", None).await;
}
