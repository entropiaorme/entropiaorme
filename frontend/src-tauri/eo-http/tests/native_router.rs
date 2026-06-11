//! Hermetic router-level coverage for the native registrations: the
//! substrate serves a composed (temp-database) hydration state with NO
//! sidecar behind the proxy arm, so anything that reaches the fallback
//! answers 502 while natively-served routes answer themselves. This
//! pins registration, adapter extraction, route-level 404 semantics,
//! and the runtime arm override without a Python toolchain; the
//! cross-language battery proves the same surface byte-for-byte
//! against the running backend.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use eo_http::arms::ArmOverrides;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use http_body_util::BodyExt;
use serde_json::Value;

async fn serve_substrate() -> (u16, Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .expect("temp db opens");
    let game_data = Arc::new(GameDataStore::new(&dir.path().join("empty")).expect("empty store"));
    let hydration = Arc::new(HydrationState::new(
        db.pool().clone(),
        game_data,
        Arc::new(RealClock::new()),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    // Port 9 (discard) on loopback: nothing listens; the proxy arm
    // fails fast and visibly.
    let state = Arc::new(
        AppState::new("127.0.0.1:9".into(), port, ArmOverrides::empty())
            .with_hydration(hydration)
            .with_cors(CorsConfig::new(5173, None)),
    );
    let serve_state = state.clone();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, serve_state).await.expect("serve");
    });
    // The listener is already bound; one probe confirms the task runs.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if get(port, "/api/health").await.0 == http::StatusCode::OK {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "substrate never came up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (port, state, dir)
}

async fn get(port: u16, path: &str) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    request(port, "GET", path, &[]).await
}

async fn request(
    port: u16,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"));
    // An explicit host in the probe replaces the default, so bad-Host
    // probes carry exactly one Host header.
    if !extra_headers.iter().any(|(name, _)| *name == "host") {
        builder = builder.header("host", &authority);
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).unwrap();
    let response = eo_http::proxy::build_client()
        .request(request)
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

fn detail_types(body: &[u8]) -> Vec<String> {
    let parsed: Value = serde_json::from_slice(body).expect("envelope parses");
    parsed["detail"]
        .as_array()
        .expect("detail is a list")
        .iter()
        .map(|issue| issue["type"].as_str().expect("typed issue").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_route_serves_natively_over_the_composed_state() {
    let (port, _state, _dir) = serve_substrate().await;
    // List routes over a fresh database: empty collections, served
    // natively (a proxy fallback would 502 against the dead upstream).
    for (path, expected_body) in [
        ("/api/quests", "[]"),
        ("/api/quests/mobs", "[]"),
        ("/api/quests/analytics", "[]"),
        ("/api/quests/playlists", "[]"),
        ("/api/quests/playlists/analytics", "[]"),
        ("/api/codex/species", "[]"),
        // The attribute set is fixed; levels hydrate from the (empty)
        // calibration tables.
        (
            "/api/codex/meta/attributes",
            "[{\"name\":\"Agility\",\"currentLevel\":null},\
             {\"name\":\"Health\",\"currentLevel\":null},\
             {\"name\":\"Intelligence\",\"currentLevel\":null},\
             {\"name\":\"Psyche\",\"currentLevel\":null},\
             {\"name\":\"Stamina\",\"currentLevel\":null},\
             {\"name\":\"Strength\",\"currentLevel\":null}]",
        ),
        ("/api/codex/recommend?species_name=X&rank=4", "[]"),
    ] {
        let (status, headers, body) = get(port, path).await;
        assert_eq!(status, http::StatusCode::OK, "{path}");
        assert_eq!(body, expected_body.as_bytes(), "{path}");
        assert!(headers.contains_key(http::header::ETAG), "{path}");
        assert_eq!(
            headers
                .get(http::header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-cache"),
            "{path}"
        );
    }
    // The path-parameter route: decoded lookup misses on the empty
    // catalogue with the handler's message, errors carry no ETag.
    let (status, headers, body) = get(port, "/api/codex/species/No%20Such/ranks").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Species 'No Such' not found\"}");
    assert!(!headers.contains_key(http::header::ETAG));

    // The conditional-GET leg: the current validator earns a 304 with
    // an empty body; a stale one re-serves the representation.
    let (_, headers, _) = get(port, "/api/quests").await;
    let etag = headers
        .get(http::header::ETAG)
        .expect("etag present")
        .to_str()
        .unwrap()
        .to_string();
    let (status, headers, body) = request(
        port,
        "GET",
        "/api/quests",
        &[("if-none-match", etag.as_str())],
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    assert_eq!(
        headers.get(http::header::ETAG).unwrap().to_str().unwrap(),
        etag
    );
    let (status, _, _) = request(
        port,
        "GET",
        "/api/quests",
        &[("if-none-match", "\"stale\"")],
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_browser_surface_is_answered_at_the_substrate() {
    let (port, _state, _dir) = serve_substrate().await;
    // A passing preflight short-circuits ahead of routing (the proxy
    // arm is dead, so a forwarded preflight would 502).
    let (status, headers, body) = request(
        port,
        "OPTIONS",
        "/api/quests",
        &[
            ("origin", "tauri://localhost"),
            ("access-control-request-method", "GET"),
        ],
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"OK");
    assert_eq!(
        headers
            .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "tauri://localhost"
    );
    // A failing preflight names its failure.
    let (status, _, body) = request(
        port,
        "OPTIONS",
        "/api/quests",
        &[
            ("origin", "http://evil.example"),
            ("access-control-request-method", "GET"),
        ],
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"Disallowed CORS origin");
    // Natively-served responses decorate for an allowed origin.
    let (status, headers, _) = request(
        port,
        "GET",
        "/api/quests",
        &[("origin", "tauri://localhost")],
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        headers
            .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "tauri://localhost"
    );
    assert_eq!(headers.get(http::header::VARY).unwrap(), "Origin");
    // Reads reject a present-but-disallowed origin ahead of routing.
    let (status, _, body) = request(
        port,
        "GET",
        "/api/quests",
        &[("origin", "http://evil.example")],
    )
    .await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    assert_eq!(body, b"{\"detail\":\"Invalid Origin header\"}");
    // Mutating methods require an allowed origin, enforced before the
    // proxy arm (a forwarded request would 502, not 403).
    let (status, _, body) = request(port, "POST", "/api/quests", &[]).await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    assert_eq!(body, b"{\"detail\":\"Origin header required\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_router_validates_through_the_extraction_layer() {
    let (port, _state, _dir) = serve_substrate().await;
    // Declaration-order multi-error.
    let (status, _, body) = get(port, "/api/codex/recommend?rank=abc&target=xx").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        detail_types(&body),
        ["missing", "int_parsing", "literal_error"]
    );
    // Bounds re-render the raw text.
    let (status, _, body) = get(port, "/api/codex/recommend?species_name=X&rank=-0").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let parsed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["detail"][0]["type"], "greater_than_equal");
    assert_eq!(parsed["detail"][0]["input"], "-0");
    // Duplicate parameter: the last occurrence validates.
    let (status, _, body) = get(port, "/api/codex/recommend?species_name=X&rank=3&rank=abc").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing"]);
    // A decoded slash inside the path parameter de-matches the route.
    let (status, _, body) = get(port, "/api/codex/species/A%2FB/ranks").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_paths_and_proxied_arms_reach_the_fallback() {
    let (port, state, _dir) = serve_substrate().await;
    // An unregistered route falls back to the proxy: 502 against the
    // dead upstream proves which arm answered.
    let (status, _, _) = get(port, "/api/settings").await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    // An encoded slash that would decode into a registered path stays
    // one raw segment here, so it reaches the fallback too.
    let (status, _, _) = get(port, "/api/quests%2Fmobs").await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    // HEAD belongs to the sidecar (the backend hard-405s HEAD on its
    // GET routes); the explicit proxy leg keeps it off the native GET
    // handler, proven by the dead upstream.
    let (status, _, _) = request(port, "HEAD", "/api/quests", &[]).await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    // A present-but-empty Host passes the guard, as the backend's
    // falsy check skips it (served natively: 200, not 403).
    let (status, _, _) = request(port, "GET", "/api/quests", &[("host", "")]).await;
    assert_eq!(status, http::StatusCode::OK);
    // The guard scopes to API paths and skips OPTIONS outright, as the
    // backend's middleware does: a bad Host on a non-API path, or on a
    // bare OPTIONS, reaches the proxy (502 here) instead of a 403.
    let (status, _, _) = request(port, "GET", "/health", &[("host", "evil:1")]).await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    let (status, _, _) = request(port, "OPTIONS", "/api/quests", &[("host", "evil:1")]).await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    // The runtime arm override steers a registered route to the proxy
    // and back without a rebuild.
    let (status, _, _) = get(port, "/api/quests").await;
    assert_eq!(status, http::StatusCode::OK);
    state.set_overrides(ArmOverrides::parse_env_value("/api/quests=proxy"));
    let (status, _, _) = get(port, "/api/quests").await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    state.set_overrides(ArmOverrides::empty());
    let (status, _, _) = get(port, "/api/quests").await;
    assert_eq!(status, http::StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_composed_services_registered_routes_fall_back() {
    // A substrate with no hydration state (composition declined)
    // proxies even its registered routes: the safe degraded mode.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let state = Arc::new(AppState::new(
        "127.0.0.1:9".into(),
        port,
        ArmOverrides::empty(),
    ));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, state).await.expect("serve");
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        // /api/health is served natively without composed services.
        if get(port, "/api/health").await.0 == http::StatusCode::OK {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "substrate never came up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, _, _) = get(port, "/api/quests").await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
}
