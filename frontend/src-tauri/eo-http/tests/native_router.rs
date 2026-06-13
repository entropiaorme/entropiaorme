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
        db,
        game_data,
        Arc::new(RealClock::new()),
        dir.path().to_path_buf(),
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
    send(port, method, path, extra_headers, None).await
}

/// A mutating request: a JSON body plus the allowed origin the guard
/// demands of mutating methods.
async fn send_json(
    port: u16,
    method: &str,
    path: &str,
    body: &str,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    send(
        port,
        method,
        path,
        &[
            ("origin", "tauri://localhost"),
            ("content-type", "application/json"),
        ],
        Some(body.as_bytes().to_vec()),
    )
    .await
}

async fn send(
    port: u16,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
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
    let request = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
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
    // Analytics serves natively but OUTSIDE the ETag middleware's prefixes,
    // so its reads are plain 200s (no ETag / Cache-Control). The overview's
    // cycledBreakdown keeps engine integer zeros (the Any field) while the
    // float-declared aggregates coerce; activity returns three empty tables.
    for (path, expected_body) in [
        (
            "/api/analytics/overview?period=all",
            "{\"totalReturnRate\":0.0,\"trend\":\"stable\",\"returnsBreakdown\":\
             {\"lootTt\":0.0,\"pes\":0.0,\"codexPes\":0.0,\"questPes\":0.0,\"ledger\":{}},\
             \"lossesBreakdown\":{\"trackingCost\":0.0,\"cycledBreakdown\":\
             {\"weapon\":0,\"healing\":0,\"enhancer\":0,\"armour\":0,\"dangling\":0},\
             \"ledger\":{}},\"totalGains\":0.0,\"totalLosses\":0.0,\"timeline\":[],\
             \"monthlyBreakdown\":[]}",
        ),
        (
            "/api/analytics/activity",
            "{\"mobComparisons\":[],\"tagComparisons\":[],\"weaponComparisons\":[]}",
        ),
        // The ledger / preset / inventory list reads (empty db -> []).
        ("/api/analytics/ledger", "[]"),
        ("/api/analytics/ledger/presets", "[]"),
        ("/api/analytics/inventory", "[]"),
    ] {
        let (status, headers, body) = get(port, path).await;
        assert_eq!(status, http::StatusCode::OK, "{path}");
        assert_eq!(body, expected_body.as_bytes(), "{path}");
        assert!(!headers.contains_key(http::header::ETAG), "{path}");
        assert_eq!(
            headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json"),
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

/// The analytics write adapters serve natively over the composed state:
/// the success paths, the validation envelopes (exercising required_f64),
/// and the handler error legs, all without the cross-language battery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_analytics_write_routes_serve_natively() {
    let (port, _state, _dir) = serve_substrate().await;
    let del = |port, path: &'static str| async move {
        request(port, "DELETE", path, &[("origin", "tauri://localhost")]).await
    };

    // Ledger: create -> 200 with a generated id; missing required float ->
    // 422 (required_f64); delete-404.
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/ledger",
        r#"{"date":"2026-05-01","type":"expense","description":"Ammo","amount":12.5,"tag":"ammo"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let entry: Value = serde_json::from_slice(&body).unwrap();
    assert!(entry["id"].as_str().is_some());
    assert_eq!(entry["amount"], serde_json::json!(12.5));
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/ledger",
        r#"{"date":"2026-05-01","type":"expense","description":"x","tag":"t"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = del(port, "/api/analytics/ledger/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Entry not found\"}");

    // Presets: create -> 200; bad type -> 400; delete-404.
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/analytics/ledger/presets",
        r#"{"name":"Decay","type":"expense","description":"d","amount":0.5,"tag":"decay"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/ledger/presets",
        r#"{"name":"Bad","type":"income","description":"d","amount":1.0,"tag":"t"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"type must be 'expense' or 'markup'\"}");
    let (status, _, body) = del(port, "/api/analytics/ledger/presets/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Preset not found\"}");

    // Inventory: create (snake_case) -> 200 camelCase; missing name -> 422;
    // patch + delete + sell over the created id; the 404 legs.
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/inventory",
        r#"{"name":"Sword","tt_value":10.0,"markup_paid":2.0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let item: Value = serde_json::from_slice(&body).unwrap();
    let id = item["id"].as_str().unwrap().to_string();
    assert_eq!(item["ttValue"], serde_json::json!(10.0));
    assert_eq!(item["markupPaid"], serde_json::json!(2.0));
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/inventory",
        r#"{"tt_value":1.0,"markup_paid":0.0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = send_json(
        port,
        "PATCH",
        &format!("/api/analytics/inventory/{id}"),
        r#"{"name":"Renamed"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let patched: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(patched["name"], serde_json::json!("Renamed"));
    let (status, _, _) = request(
        port,
        "PATCH",
        "/api/analytics/inventory/nope",
        &[("origin", "tauri://localhost")],
    )
    .await;
    // PATCH carries no body here, so the missing-body envelope precedes the
    // 404; either way it is not a 200.
    assert_ne!(status, http::StatusCode::OK);
    let (status, _, body) = del(port, "/api/analytics/inventory/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Inventory item not found\"}");

    // Sell the created item -> 200 with a markup ledger entry; sell-404.
    let (status, _, body) = send_json(
        port,
        "POST",
        &format!("/api/analytics/inventory/{id}/sell"),
        r#"{"sale_price":20.0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let sold: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sold["ledgerEntry"]["type"], serde_json::json!("markup"));
    assert_eq!(sold["ledgerEntry"]["amount"], serde_json::json!(8.0));
    // The item was renamed by the earlier PATCH leg.
    assert_eq!(sold["soldItem"]["name"], serde_json::json!("Renamed"));
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/analytics/inventory/nope/sell",
        r#"{"sale_price":1.0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Inventory item not found\"}");
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
    // dead upstream proves which arm answered. The tracking read surface
    // stays with the sidecar, while the analytics surface serves natively.
    let (status, _, _) = get(port, "/api/tracking/snapshot").await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
    // A registered path's unported method falls back too: the
    // settings PATCH stays with the sidecar until the producer
    // cutover, while its GET serves natively.
    let (status, _, _) = send_json(port, "PATCH", "/api/settings", "{}").await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_surface_serves_natively_over_the_composed_state() {
    let (port, _state, _dir) = serve_substrate().await;

    // Create: minimal, then lax-coerced fields; ids are deterministic
    // over the fresh database.
    let (status, headers, body) =
        send_json(port, "POST", "/api/quests", r#"{"name": "Alpha"}"#).await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "write replies carry no conditional-GET headers"
    );
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(created["id"], "1");
    assert_eq!(created["planet"], "Calypso");
    assert_eq!(created["rewardIsSkill"], false);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/quests",
        r#"{"name": "Beta", "reward_ped": "1_0.5", "reward_is_skill": "yes", "chain_position": 2.0, "mobs": ["Atrox"]}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(created["reward"], 10.5);
    assert_eq!(created["rewardIsSkill"], true);
    assert_eq!(created["chainPosition"], 2);
    assert_eq!(created["targetMobs"], serde_json::json!(["Atrox"]));

    // The single-quest read serves with the conditional-GET contract.
    let (status, headers, _) = get(port, "/api/quests/1").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));

    // Update: exclude-unset (only sent fields move), declaration-order
    // multi-error, and present-null clears.
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/quests/1",
        r#"{"notes": "updated", "reward_ped": null}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let updated: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(updated["notes"], "updated");
    assert_eq!(updated["reward"], Value::Null);
    assert_eq!(updated["name"], "Alpha", "unsent fields keep their values");
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/quests/1",
        r#"{"reward_description": 5, "cooldown_hours": "x"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let kinds = detail_types(&body);
    assert_eq!(
        kinds,
        ["float_parsing", "string_type"],
        "issues list in model declaration order (cooldown_hours before reward_description)"
    );

    // Lifecycle on a zero-cooldown quest.
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/quests",
        r#"{"name": "Cycle", "cooldown_hours": 0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(port, "POST", "/api/quests/3/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let started: Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(started["startedAt"], Value::Null);
    let (status, _, body) = send_json(port, "POST", "/api/quests/3/complete", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let completed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(completed["cooldownExpiresAt"], Value::Null);
    let (status, _, _) = send_json(port, "POST", "/api/quests/3/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/quests/3/cancel",
        r#"{"undo_reward": false}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let cancelled: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(cancelled["startedAt"], Value::Null);
    // Cancel tolerates a top-level null body (no-body semantics).
    let (status, _, _) = send_json(port, "POST", "/api/quests/3/cancel", "null").await;
    assert_eq!(status, http::StatusCode::OK);

    // Playlists: create with nested items, update, delete.
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/quests/playlists",
        r#"{"name": "Run", "estimated_minutes": "45", "quest_ids": [1, "2"], "items": [{"quest_id": 3, "group_type": "long_horizon"}]}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let playlist: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(playlist["estimatedMinutes"], 45);
    // Provided items supersede the plain id list: the playlist's quest
    // set derives from them (the cross-language battery pins the same
    // shape on both arms).
    assert_eq!(playlist["questIds"], serde_json::json!(["3"]));
    assert_eq!(playlist["items"][0]["groupType"], "long_horizon");
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/quests/playlists/1",
        r#"{"name": "Run 2"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let renamed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(renamed["name"], "Run 2");
    let (status, _, body) = send_json(port, "DELETE", "/api/quests/playlists/1", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"ok\":true}");

    // Calibrate: the write, the bound, and the beyond-range rank.
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/codex/calibrate",
        r#"{"species_name": "Sp", "rank": 7}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"speciesName\":\"Sp\",\"rank\":7}");
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/codex/calibrate",
        r#"{"species_name": "Sp", "rank": 26}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Rank must be 0-25\"}");
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/codex/calibrate",
        r#"{"species_name": "Sp", "rank": 999999999999999999999999}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);

    // Not-found legs and the delete reply.
    for (method, path, body) in [
        ("PUT", "/api/quests/424242", r#"{"name": "Z"}"#),
        ("DELETE", "/api/quests/424242", ""),
        ("POST", "/api/quests/424242/start", ""),
        ("PUT", "/api/quests/playlists/424242", r#"{"name": "Z"}"#),
        ("DELETE", "/api/quests/playlists/424242", ""),
    ] {
        let (status, _, reply) = send_json(port, method, path, body).await;
        assert_eq!(status, http::StatusCode::NOT_FOUND, "{method} {path}");
        assert!(
            reply == b"{\"detail\":\"Quest not found\"}"
                || reply == b"{\"detail\":\"Playlist not found\"}",
            "{method} {path}"
        );
    }
    let (status, _, body) = send_json(port, "DELETE", "/api/quests/2", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"ok\":true}");

    // Path-parameter legs on the write routes.
    let (status, _, body) = send_json(port, "PUT", "/api/quests/abc", r#"{"name": "Z"}"#).await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing"]);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/quests/999999999999999999999999/start",
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, b"Internal Server Error");
    let (status, _, body) = send_json(port, "POST", "/api/quests/A%2FB/start", "").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_failures_answer_the_backend_reply_classes() {
    let (port, _state, _dir) = serve_substrate().await;

    // Missing and null bodies on a required-body route.
    for body in ["", "null"] {
        let (status, _, reply) = send_json(port, "POST", "/api/quests", body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], "missing");
        assert_eq!(parsed["detail"][0]["loc"], serde_json::json!(["body"]));
    }

    // Malformed JSON carries the scanner's message and position.
    let (status, _, reply) = send_json(port, "POST", "/api/quests", r#"{"name": "Q", }"#).await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let parsed: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(parsed["detail"][0]["type"], "json_invalid");
    assert_eq!(parsed["detail"][0]["loc"], serde_json::json!(["body", 12]));

    // The bool taxonomy split.
    for (value, kind) in [
        ("null", "bool_type"),
        ("1.5", "bool_type"),
        ("[1]", "bool_type"),
        ("999999999999999999999999", "bool_type"),
        ("2.0", "bool_parsing"),
        ("2", "bool_parsing"),
        ("\"zz\"", "bool_parsing"),
    ] {
        let body = format!(r#"{{"name": "B", "reward_is_skill": {value}}}"#);
        let (status, _, reply) = send_json(port, "POST", "/api/quests", &body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{value}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], kind, "{value}");
    }

    // Beyond-range floats into int fields answer the size 422 with
    // both exact bounds excluded; digit strings stay the storage 500.
    for value in ["1e30", "9223372036854775808.0", "-9223372036854775808.0"] {
        let body = format!(r#"{{"name": "I", "chain_position": {value}}}"#);
        let (status, _, reply) = send_json(port, "POST", "/api/quests", &body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{value}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], "int_parsing_size", "{value}");
    }
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/quests",
        r#"{"name": "I", "chain_position": 999999999999999999999999}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, b"Internal Server Error");

    // Unrenderable echoes answer the plain-text 500: non-finite
    // values, lone surrogates, and over-deep echoed bodies.
    for body in [r#"{"name": Infinity}"#, "{\"planet\": \"\\ud800\"}"] {
        let (status, _, reply) = send_json(port, "POST", "/api/quests", body).await;
        assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(reply, b"Internal Server Error");
    }
    let deep = format!(r#"{{"name": {}{}}}"#, "[".repeat(990), "]".repeat(990));
    let (status, _, _) = send_json(port, "POST", "/api/quests", &deep).await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let shallow = format!(r#"{{"name": {}{}}}"#, "[".repeat(100), "]".repeat(100));
    let (status, _, reply) = send_json(port, "POST", "/api/quests", &shallow).await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&reply), ["string_type"]);

    // Beyond the parse cap: the generic body-parse 400.
    let too_deep = "[".repeat(50_000) + &"]".repeat(50_000);
    let (status, _, reply) = send_json(port, "POST", "/api/quests", &too_deep).await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        reply,
        b"{\"detail\":\"There was an error parsing the body\"}"
    );

    // The content-type gate: a foreign maintype with a +json suffix is
    // not JSON (raw-string echo), application subtypes match
    // case-insensitively.
    let (status, _, reply) = send(
        port,
        "POST",
        "/api/quests",
        &[
            ("origin", "tauri://localhost"),
            ("content-type", "text/whatever+json"),
        ],
        Some(br#"{"name": "TW"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let parsed: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(parsed["detail"][0]["type"], "model_attributes_type");
    let (status, _, _) = send(
        port,
        "POST",
        "/api/quests",
        &[
            ("origin", "tauri://localhost"),
            ("content-type", "Application/JSON"),
        ],
        Some(br#"{"name": "CASEY"}"#.to_vec()),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);

    // Encoding detection: UTF-16 bodies parse; invalid UTF-8 answers
    // the generic 400.
    let utf16: Vec<u8> = r#"{"name": "U16"}"#.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let (status, _, _) = send(
        port,
        "POST",
        "/api/quests",
        &[
            ("origin", "tauri://localhost"),
            ("content-type", "application/json"),
        ],
        Some(utf16),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let bad = [br#"{"name": ""#.to_vec(), vec![0xFF], br#""}"#.to_vec()].concat();
    let (status, _, reply) = send(
        port,
        "POST",
        "/api/quests",
        &[
            ("origin", "tauri://localhost"),
            ("content-type", "application/json"),
        ],
        Some(bad),
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        reply,
        b"{\"detail\":\"There was an error parsing the body\"}"
    );
}

/// A transport-level body read failure (the peer half-closes inside an
/// over-declared Content-Length) answers the unhandled-error 500 and
/// mutates nothing: the reference never reaches its handler on a failed
/// read, so a started quest must not cancel off a truncated payload.
#[tokio::test]
async fn failed_body_read_answers_500_and_writes_nothing() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (port, _state, _dir) = serve_substrate().await;
    let (status, _, _) = send_json(port, "POST", "/api/quests", r#"{"name": "Probe"}"#).await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, _) = send_json(port, "POST", "/api/quests/1/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let (_, _, before) = get(port, "/api/quests/1").await;

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let head = format!(
        "POST /api/quests/1/cancel HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\norigin: tauri://localhost\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{{\"undo_rew"
    );
    stream.write_all(head.as_bytes()).await.expect("write head");
    stream.shutdown().await.expect("half-close");
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.expect("read reply");
    let reply = String::from_utf8_lossy(&reply);
    assert!(reply.starts_with("HTTP/1.1 500 "), "got: {reply}");
    assert!(reply.ends_with("Internal Server Error"), "got: {reply}");

    let (status, _, after) = get(port, "/api/quests/1").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(after, before, "the failed read must not cancel the quest");
}

/// The settings/character/equipment surface serves natively over the
/// composed state: the settings reads, the character family over an
/// empty calibration table, and the equipment routes (a consumable
/// write needs no catalogue, so the full write path proves itself
/// against the temp database).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_character_and_equipment_surface_serves_natively() {
    let (port, _state, _dir) = serve_substrate().await;

    // Settings assembly over the fresh data dir: defaults, the live
    // db path, and the workspace version stamp.
    let (status, headers, body) = get(port, "/api/settings").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "the ETag middleware scopes to other prefixes; settings reads are plain"
    );
    let settings: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(settings["mobTrackingMode"], "mob");
    assert_eq!(
        settings["lootFilterBlacklist"],
        serde_json::json!(["Universal Ammo"])
    );
    assert_eq!(settings["trifecta"]["activePresetId"], "default");
    assert_eq!(settings["trifecta"]["presets"][0]["ready"], false);
    assert_eq!(
        settings["trifecta"]["message"],
        "Trifecta attribution requires a configured small weapon, big weapon, and healing tool"
    );
    assert_eq!(settings["appVersion"], env!("CARGO_PKG_VERSION"));
    assert!(settings["dbPath"]
        .as_str()
        .unwrap()
        .ends_with("entropia_orme.db"));
    let hotbar_keys: Vec<&str> = settings["hotbar"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        hotbar_keys,
        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
    );

    let (status, _, body) = get(port, "/api/settings/overlay-position").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"x\":null,\"y\":null}");

    // Character family over an empty calibration table (the skills
    // catalogue is empty in this harness, so HP sits at its base).
    for (path, expected) in [
        (
            "/api/character/calibration",
            "{\"calibrated\":false,\"lastCalibration\":null,\"stale\":true}",
        ),
        ("/api/character/stats", "{\"hp\":0,\"topProfessions\":[]}"),
        ("/api/character/skills", "[]"),
        ("/api/character/professions", "[]"),
        (
            "/api/character/prospect-options",
            "{\"tags\":[],\"mobs\":[],\"weapons\":[]}",
        ),
        ("/api/character/codex", "[]"),
        (
            "/api/character/hp-optimizer",
            "{\"currentHp\":80.0,\"skills\":[],\"attributes\":[]}",
        ),
    ] {
        let (status, _, body) = get(port, path).await;
        assert_eq!(status, http::StatusCode::OK, "{path}");
        assert_eq!(body, expected.as_bytes(), "{path}");
    }

    // The prospect family's validation ladder: the envelope first (in
    // signature order), then the handler's own 422 details.
    let (status, _, body) = get(port, "/api/character/prospect").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing", "missing"]);
    let (status, _, body) = get(port, "/api/character/prospect?profession=X&target_level=0").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body, b"{\"detail\":\"target_level must be positive\"}");
    let (status, _, body) = get(
        port,
        "/api/character/prospect?profession=X&target_level=5&slice_type=banana",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        b"{\"detail\":\"slice_type must be global, tag, mob, or weapon\"}"
    );
    let (status, _, body) = get(
        port,
        "/api/character/prospect?profession=X&target_level=5&slice_type=mob",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        b"{\"detail\":\"slice_value is required for non-global slices\"}"
    );
    // An unknown profession answers the error SHAPE (model order puts
    // error/rows/warnings first), not a 404.
    let (status, _, body) = get(port, "/api/character/prospect?profession=X&target_level=5").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body,
        b"{\"error\":\"Profession 'X' not found\",\"rows\":[],\"warnings\":[]}"
    );
    let (status, _, body) = get(port, "/api/character/profession-optimizer?profession=X").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body,
        b"{\"skills\":[],\"attributes\":[],\"error\":\"Profession 'X' not found\"}"
    );
    let (status, _, body) = get(port, "/api/character/profession-optimizer").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = get(
        port,
        "/api/character/profession-path-optimizer?profession=X&target_level=5&ped_budget=1",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        b"{\"detail\":\"Exactly one of target_level or ped_budget must be provided\"}"
    );
    let (status, _, body) = get(
        port,
        "/api/character/profession-path-optimizer?profession=X&target_level=abc",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["float_parsing"]);

    // Equipment: the search type gate, the empty library, and the
    // catalogue-less validation ladder.
    let (status, _, body) = get(port, "/api/equipment/search?q=op&type=banana").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Unknown type 'banana'\"}");
    let (status, _, body) = get(port, "/api/equipment/search?q=o").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]", "short queries return empty before any lookup");
    let (status, _, body) = get(port, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]");

    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"banana\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["literal_error"]);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"weapon\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"catalog_id required for weapon\"}");
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"weapon\",\"catalog_id\":\"nope\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        b"{\"detail\":\"Entity 'nope' not found in catalogue endpoint 'weapons'.\"}"
    );
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"consumable\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        b"{\"detail\":\"Consumable requires either catalog_id (catalogue pick) or name (custom)\"}"
    );

    // A custom consumable needs no catalogue: the full write path over
    // the temp database, then the list, update-type gate, and delete.
    let (status, headers, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"consumable\",\"name\":\"  Nutrio Bar  \"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "write replies carry no conditional-GET headers"
    );
    assert_eq!(
        body,
        b"{\"id\":\"1\",\"name\":\"Nutrio Bar\",\"type\":\"consumable\",\"amplifierName\":null,\
          \"costPerUse\":0.0,\"damageMin\":null,\"damageMax\":null,\"reloadSeconds\":null,\
          \"isLimited\":false,\"enrichmentLevel\":1}"
    );
    let (status, _, body) = get(port, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed[0]["id"], "1");
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/equipment/library/1",
        "{\"type\":\"weapon\",\"catalog_id\":\"x\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Cannot change equipment type\"}");
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/equipment/library/9",
        "{\"type\":\"consumable\",\"name\":\"X\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Equipment item 9 not found\"}");
    let (status, _, body) = get(port, "/api/equipment/library/1/detail").await;
    assert_eq!(status, http::StatusCode::OK);
    let detail_shape: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail_shape["weapon"]["name"], "Nutrio Bar");
    assert_eq!(detail_shape["totalCostPerUse"], 0.0);
    let (status, _, body) = get(port, "/api/equipment/library/9/detail").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Equipment item 9 not found\"}");
    // Deletes are idempotent acknowledgements, present row or not.
    for item in ["1", "9"] {
        let (status, _, body) = send(
            port,
            "DELETE",
            &format!("/api/equipment/library/{item}"),
            &[("origin", "tauri://localhost")],
            None,
        )
        .await;
        assert_eq!(status, http::StatusCode::OK);
        assert_eq!(body, b"{\"status\":\"deleted\"}");
    }
    let (status, _, body) = get(port, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]");

    // Cost calculation validates in model order (catalog_id first).
    let (status, _, body) = send_json(port, "POST", "/api/equipment/cost/calculate", "{}").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/cost/calculate",
        "{\"catalog_id\":\"x\",\"type\":\"consumable\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["literal_error"]);

    // The unported settings write stays with the sidecar: the PUT
    // falls to the path's proxy fallback (dead upstream → 502).
    let (status, _, _) = send_json(
        port,
        "PUT",
        "/api/settings/overlay-position",
        "{\"x\":1,\"y\":2}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_GATEWAY);
}

/// Path and body validation aggregate into one envelope (path issue
/// first); decode failures stand alone; deferred 500s (beyond-i64
/// integers, consumed surrogate taints) fire only on otherwise-clean
/// requests, each at its consumption point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_envelopes_aggregate_and_defer_the_backend_way() {
    let (port, _state, _dir) = serve_substrate().await;

    // Path + body field issues, one envelope, path first.
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/equipment/library/abc",
        "{\"type\": \"banana\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing", "literal_error"]);
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/quests/abc",
        "{\"cooldown_hours\": \"x\", \"chain_position\": 1.5}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        detail_types(&body),
        ["int_parsing", "float_parsing", "int_from_float"]
    );
    // A non-object cancel body aggregates with the path issue.
    let (status, _, body) = send_json(port, "POST", "/api/quests/abc/cancel", "5").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        detail_types(&body),
        ["int_parsing", "model_attributes_type"]
    );
    // A decode failure stands alone, dropping the path issue.
    let (status, _, body) = send_json(port, "PUT", "/api/quests/abc", "{bad").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["json_invalid"]);
    // A missing body aggregates as the missing-["body"] issue.
    let (status, _, body) = send(
        port,
        "PUT",
        "/api/quests/playlists/abc",
        &[("origin", "tauri://localhost")],
        None,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing", "missing"]);

    // Beyond-i64 path ids validate (Python's int is unbounded) and
    // crash at the handler's first binding AFTER the envelope.
    let (status, _, body) = send_json(
        port,
        "PUT",
        "/api/quests/99999999999999999999999999",
        "{\"cooldown_hours\": \"x\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["float_parsing"]);
    let (status, _, _) = send_json(
        port,
        "PUT",
        "/api/quests/99999999999999999999999999",
        "{\"name\": \"X\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);

    // Beyond-i64 BODY integers answer the deferred 500 across the
    // dump builders (quest update field, playlist quest_ids, playlist
    // item ids, equipment ints), only after validation passes.
    let (status, _, _) = send_json(port, "POST", "/api/quests", "{\"name\": \"Q\"}").await;
    assert_eq!(status, http::StatusCode::OK);
    for (method, path, body) in [
        (
            "PUT",
            "/api/quests/1",
            "{\"chain_position\": 99999999999999999999999999}",
        ),
        (
            "POST",
            "/api/quests/playlists",
            "{\"name\": \"P\", \"quest_ids\": [99999999999999999999999999]}",
        ),
        (
            "POST",
            "/api/quests/playlists",
            "{\"name\": \"P\", \"items\": [{\"quest_id\": 99999999999999999999999999}]}",
        ),
        (
            "POST",
            "/api/equipment/library",
            "{\"type\": \"consumable\", \"name\": \"X\", \"weapon_markup\": 99999999999999999999999999}",
        ),
        (
            "POST",
            "/api/equipment/library",
            "{\"type\": \"consumable\", \"name\": \"X\", \"damage_enhancers\": 99999999999999999999999999}",
        ),
        (
            "POST",
            "/api/equipment/cost/calculate",
            "{\"catalog_id\": \"x\", \"amp_markup\": 99999999999999999999999999}",
        ),
    ] {
        let (status, _, reply) = send_json(port, method, path, body).await;
        assert_eq!(
            status,
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {path}: {}",
            String::from_utf8_lossy(&reply)
        );
    }
    // A playlist update whose only set field overflows must not slip
    // through as a no-op update.
    let (status, _, _) =
        send_json(port, "POST", "/api/quests/playlists", "{\"name\": \"Pl\"}").await;
    assert_eq!(status, http::StatusCode::OK);
    for body in [
        "{\"estimated_minutes\": 99999999999999999999999999}",
        "{\"quest_ids\": [99999999999999999999999999]}",
        "{\"items\": [{\"quest_id\": 99999999999999999999999999}]}",
    ] {
        let (status, _, _) = send_json(port, "PUT", "/api/quests/playlists/1", body).await;
        assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    }

    // A CONSUMED surrogate-tainted field answers the 500 at its
    // consumption point; an UNUSED one flows (the lookup miss answers
    // its renderable 404 on the empty store).
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\": \"weapon\", \"catalog_id\": \"ta\\ud800int\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\": \"weapon\", \"catalog_id\": \"clean\", \"name\": \"ta\\ud800int\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND, "unused taint flows");
    assert_eq!(
        body,
        b"{\"detail\":\"Entity 'clean' not found in catalogue endpoint 'weapons'.\"}"
    );
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/equipment/cost/calculate",
        "{\"catalog_id\": \"ta\\ud800int\", \"type\": \"healing\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let (status, _, _) = send_json(
        port,
        "POST",
        "/api/equipment/library",
        "{\"type\": \"consumable\", \"name\": \"ta\\ud800int\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);

    // The calibrate codec message splits singular/plural on the
    // surrogate RUN length, with the exact position arithmetic.
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/codex/calibrate",
        "{\"species_name\": \"ab\\ud800cd\", \"rank\": 3}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        b"{\"detail\":\"'utf-8' codec can't encode character '\\\\ud800' in position 2: surrogates not allowed\"}"
    );
    let (status, _, body) = send_json(
        port,
        "POST",
        "/api/codex/calibrate",
        "{\"species_name\": \"ab\\ud800\\ud801cd\", \"rank\": 3}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        b"{\"detail\":\"'utf-8' codec can't encode characters in position 2-3: surrogates not allowed\"}"
    );

    // The prospect markup gate sits strictly below zero.
    let (status, _, _) = get(
        port,
        "/api/character/prospect?profession=X&target_level=5&markup_uplift=-0.1",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = get(
        port,
        "/api/character/prospect?profession=X&target_level=5&markup_uplift=0",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
}
