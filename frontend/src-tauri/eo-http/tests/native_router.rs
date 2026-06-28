//! Hermetic router-level coverage for the native registrations. The test
//! harness composes a temp-database hydration state and drives the router
//! in-memory: each request is dispatched through `build_router(state).oneshot`
//! (the same router core the production binary serves in-process via the IPC
//! command), with no socket and no transport. A registered route answers
//! natively, an unmatched path is the framework 404, and an unported method
//! the framework 405. This pins registration, adapter extraction, the
//! validation envelopes, and the conditional-GET / CORS contracts without a
//! Python toolchain.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tower::ServiceExt;

async fn serve_substrate() -> (Arc<AppState>, tempfile::TempDir) {
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
    let state = Arc::new(
        AppState::new(0)
            .with_hydration(hydration)
            .with_cors(CorsConfig::new(5173, None)),
    );
    (state, dir)
}

async fn get(state: &Arc<AppState>, path: &str) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    request(state, "GET", path, &[]).await
}

async fn request(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    send(state, method, path, extra_headers, None).await
}

/// A mutating request: a JSON body plus the allowed origin the guard
/// demands of mutating methods.
async fn send_json(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    body: &str,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    send(
        state,
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

/// Dispatch one request through a freshly built router (`oneshot` consumes
/// the router, so each request gets its own). No socket and no Host default:
/// a request without a Host header is admitted exactly as the in-process IPC
/// transport's requests are, so the guard / CORS / observe stack is exercised
/// identically.
async fn send(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let mut builder = http::Request::builder().method(method).uri(path);
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
    let response = eo_http::build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
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
    let (state, _dir) = serve_substrate().await;
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
        // The tracking session reads (ETag-scoped; empty db -> []).
        ("/api/tracking/sessions", "[]"),
        ("/api/tracking/tag-suggestions?q=a", "[]"),
    ] {
        let (status, headers, body) = get(&state, path).await;
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
        let (status, headers, body) = get(&state, path).await;
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
    let (status, headers, body) = get(&state, "/api/codex/species/No%20Such/ranks").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Species 'No Such' not found\"}");
    assert!(!headers.contains_key(http::header::ETAG));

    // A missing tracking session: the handler's 404, no ETag.
    let (status, headers, body) = get(&state, "/api/tracking/session/no-such").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Session not found\"}");
    assert!(!headers.contains_key(http::header::ETAG));

    // The conditional-GET leg: the current validator earns a 304 with
    // an empty body; a stale one re-serves the representation.
    let (_, headers, _) = get(&state, "/api/quests").await;
    let etag = headers
        .get(http::header::ETAG)
        .expect("etag present")
        .to_str()
        .unwrap()
        .to_string();
    let (status, headers, body) = request(
        &state,
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
        &state,
        "GET",
        "/api/quests",
        &[("if-none-match", "\"stale\"")],
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
}

/// The analytics write adapters serve natively over the composed state:
/// the success paths, the validation envelopes (exercising required_f64),
/// and the handler error legs, all hermetically.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_analytics_write_routes_serve_natively() {
    let (state, _dir) = serve_substrate().await;
    let del = |state: &Arc<AppState>, path: &'static str| {
        let state = state.clone();
        async move { request(&state, "DELETE", path, &[("origin", "tauri://localhost")]).await }
    };

    // Ledger: create -> 200 with a generated id; missing required float ->
    // 422 (required_f64); delete-404.
    let (status, _, body) = send_json(
        &state,
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
        &state,
        "POST",
        "/api/analytics/ledger",
        r#"{"date":"2026-05-01","type":"expense","description":"x","tag":"t"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = del(&state, "/api/analytics/ledger/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Entry not found\"}");

    // Presets: create -> 200; bad type -> 400; delete-404.
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/analytics/ledger/presets",
        r#"{"name":"Decay","type":"expense","description":"d","amount":0.5,"tag":"decay"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/analytics/ledger/presets",
        r#"{"name":"Bad","type":"income","description":"d","amount":1.0,"tag":"t"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"type must be 'expense' or 'markup'\"}");
    let (status, _, body) = del(&state, "/api/analytics/ledger/presets/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Preset not found\"}");

    // Inventory: create (snake_case) -> 200 camelCase; missing name -> 422;
    // patch + delete + sell over the created id; the 404 legs.
    let (status, _, body) = send_json(
        &state,
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
        &state,
        "POST",
        "/api/analytics/inventory",
        r#"{"tt_value":1.0,"markup_paid":0.0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = send_json(
        &state,
        "PATCH",
        &format!("/api/analytics/inventory/{id}"),
        r#"{"name":"Renamed"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let patched: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(patched["name"], serde_json::json!("Renamed"));
    let (status, _, body) = request(
        &state,
        "PATCH",
        "/api/analytics/inventory/nope",
        &[("origin", "tauri://localhost")],
    )
    .await;
    // PATCH carries no body here, so the missing-body validation envelope
    // precedes the 404 item lookup.
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = del(&state, "/api/analytics/inventory/nope").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Inventory item not found\"}");

    // Sell the created item -> 200 with a markup ledger entry; sell-404.
    let (status, _, body) = send_json(
        &state,
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
        &state,
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
    let (state, _dir) = serve_substrate().await;
    // A passing preflight short-circuits ahead of routing (no upstream
    // exists, so a forwarded preflight would 502).
    let (status, headers, body) = request(
        &state,
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
        &state,
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
        &state,
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
        &state,
        "GET",
        "/api/quests",
        &[("origin", "http://evil.example")],
    )
    .await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    assert_eq!(body, b"{\"detail\":\"Invalid Origin header\"}");
    // Mutating methods require an allowed origin, enforced before any
    // upstream forward (a forwarded request would 502, not 403).
    let (status, _, body) = request(&state, "POST", "/api/quests", &[]).await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    assert_eq!(body, b"{\"detail\":\"Origin header required\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_router_validates_through_the_extraction_layer() {
    let (state, _dir) = serve_substrate().await;
    // Declaration-order multi-error.
    let (status, _, body) = get(&state, "/api/codex/recommend?rank=abc&target=xx").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        detail_types(&body),
        ["missing", "int_parsing", "literal_error"]
    );
    // Bounds re-render the raw text.
    let (status, _, body) = get(&state, "/api/codex/recommend?species_name=X&rank=-0").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let parsed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["detail"][0]["type"], "greater_than_equal");
    assert_eq!(parsed["detail"][0]["input"], "-0");
    // Duplicate parameter: the last occurrence validates.
    let (status, _, body) = get(
        &state,
        "/api/codex/recommend?species_name=X&rank=3&rank=abc",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing"]);
    // A decoded slash inside the path parameter de-matches the route.
    let (status, _, body) = get(&state, "/api/codex/species/A%2FB/ranks").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_framework_404s_unmatched_paths_and_405s_unported_methods() {
    let (state, _dir) = serve_substrate().await;
    // An encoded slash stays one raw segment, so it does not decode into the
    // registered `/api/quests/mobs` path: the framework 404 (nothing forwards
    // it upstream now).
    let (status, _, body) = get(&state, "/api/quests%2Fmobs").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
    // An unmatched path under /api is likewise the framework 404.
    let (status, _, _) = get(&state, "/api/no-such-route").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    // HEAD on a GET route is the framework 405: the backend hard-405s HEAD on
    // its GET routes, and the native router does not auto-serve HEAD from the
    // GET handler (it carries an explicit 405 method fallback).
    let (status, _, _) = request(&state, "HEAD", "/api/quests", &[]).await;
    assert_eq!(status, http::StatusCode::METHOD_NOT_ALLOWED);
    // A present-but-empty Host passes the guard (the backend's falsy check
    // skips it), so the route serves natively.
    let (status, _, _) = request(&state, "GET", "/api/quests", &[("host", "")]).await;
    assert_eq!(status, http::StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_surface_serves_natively_over_the_composed_state() {
    let (state, _dir) = serve_substrate().await;

    // Create: minimal, then lax-coerced fields; ids are deterministic
    // over the fresh database.
    let (status, headers, body) =
        send_json(&state, "POST", "/api/quests", r#"{"name": "Alpha"}"#).await;
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
        &state,
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
    let (status, headers, _) = get(&state, "/api/quests/1").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));

    // Update: exclude-unset (only sent fields move), declaration-order
    // multi-error, and present-null clears.
    let (status, _, body) = send_json(
        &state,
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
        &state,
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
        &state,
        "POST",
        "/api/quests",
        r#"{"name": "Cycle", "cooldown_hours": 0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(&state, "POST", "/api/quests/3/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let started: Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(started["startedAt"], Value::Null);
    let (status, _, body) = send_json(&state, "POST", "/api/quests/3/complete", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let completed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(completed["cooldownExpiresAt"], Value::Null);
    let (status, _, _) = send_json(&state, "POST", "/api/quests/3/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/quests/3/cancel",
        r#"{"undo_reward": false}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let cancelled: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(cancelled["startedAt"], Value::Null);
    // Cancel tolerates a top-level null body (no-body semantics).
    let (status, _, _) = send_json(&state, "POST", "/api/quests/3/cancel", "null").await;
    assert_eq!(status, http::StatusCode::OK);

    // Playlists: create with nested items, update, delete.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/quests/playlists",
        r#"{"name": "Run", "estimated_minutes": "45", "quest_ids": [1, "2"], "items": [{"quest_id": 3, "group_type": "long_horizon"}]}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let playlist: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(playlist["estimatedMinutes"], 45);
    // Provided items supersede the plain id list: the playlist's quest
    // set derives from them (the committed golden pins the same shape).
    assert_eq!(playlist["questIds"], serde_json::json!(["3"]));
    assert_eq!(playlist["items"][0]["groupType"], "long_horizon");
    let (status, _, body) = send_json(
        &state,
        "PUT",
        "/api/quests/playlists/1",
        r#"{"name": "Run 2"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let renamed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(renamed["name"], "Run 2");
    let (status, _, body) = send_json(&state, "DELETE", "/api/quests/playlists/1", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"ok\":true}");

    // Calibrate: the write, the bound, and the beyond-range rank.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/codex/calibrate",
        r#"{"species_name": "Sp", "rank": 7}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"speciesName\":\"Sp\",\"rank\":7}");
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/codex/calibrate",
        r#"{"species_name": "Sp", "rank": 26}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Rank must be 0-25\"}");
    let (status, _, _) = send_json(
        &state,
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
        let (status, _, reply) = send_json(&state, method, path, body).await;
        assert_eq!(status, http::StatusCode::NOT_FOUND, "{method} {path}");
        assert!(
            reply == b"{\"detail\":\"Quest not found\"}"
                || reply == b"{\"detail\":\"Playlist not found\"}",
            "{method} {path}"
        );
    }
    let (status, _, body) = send_json(&state, "DELETE", "/api/quests/2", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"{\"ok\":true}");

    // Path-parameter legs on the write routes.
    let (status, _, body) = send_json(&state, "PUT", "/api/quests/abc", r#"{"name": "Z"}"#).await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing"]);
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/quests/999999999999999999999999/start",
        "",
    )
    .await;
    // A beyond-i64 path id can never name a stored quest, so it is a
    // clean 404 (a missing resource), like the decoded-slash case below.
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
    let (status, _, body) = send_json(&state, "POST", "/api/quests/A%2FB/start", "").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Not Found\"}");
}

/// A5: an integer path id beyond i64 names no stored row, so every
/// int-id route answers a clean 404 (a missing resource) rather than
/// the old unhandled-overflow 500. The body-carrying routes resolve
/// the 404 only after the body validates clean (a deferred 404).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_path_id_is_not_found() {
    let (state, _dir) = serve_substrate().await;
    let big = "99999999999999999999999999"; // > i64::MAX

    // No-body int-id routes (via `path_id`): every method answers the
    // framework 404, like the decoded-slash case.
    let no_body: [(&str, String); 6] = [
        ("DELETE", format!("/api/quests/{big}")),
        ("POST", format!("/api/quests/{big}/start")),
        ("POST", format!("/api/quests/{big}/complete")),
        ("DELETE", format!("/api/quests/playlists/{big}")),
        ("GET", format!("/api/equipment/library/{big}/detail")),
        ("DELETE", format!("/api/equipment/library/{big}")),
    ];
    for (method, path) in &no_body {
        let (status, _, body) = send_json(&state, method, path, "").await;
        assert_eq!(status, http::StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(body, b"{\"detail\":\"Not Found\"}", "{method} {path}");
    }

    // Body-carrying int-id routes (via `path_param`): an overflow id on
    // an otherwise-clean body resolves to the deferred 404.
    let clean_body: [(&str, String, &str); 4] = [
        ("PUT", format!("/api/quests/{big}"), "{\"name\": \"X\"}"),
        ("POST", format!("/api/quests/{big}/cancel"), ""),
        (
            "PUT",
            format!("/api/quests/playlists/{big}"),
            "{\"name\": \"X\"}",
        ),
        (
            "PUT",
            format!("/api/equipment/library/{big}"),
            "{\"type\":\"weapon\",\"catalog_id\":\"x\"}",
        ),
    ];
    for (method, path, body) in &clean_body {
        let (status, _, _) = send_json(&state, method, path, body).await;
        assert_eq!(status, http::StatusCode::NOT_FOUND, "{method} {path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_failures_answer_the_backend_reply_classes() {
    let (state, _dir) = serve_substrate().await;

    // Missing and null bodies on a required-body route.
    for body in ["", "null"] {
        let (status, _, reply) = send_json(&state, "POST", "/api/quests", body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], "missing");
        assert_eq!(parsed["detail"][0]["loc"], serde_json::json!(["body"]));
    }

    // Malformed JSON carries the scanner's message and position.
    let (status, _, reply) = send_json(&state, "POST", "/api/quests", r#"{"name": "Q", }"#).await;
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
        let (status, _, reply) = send_json(&state, "POST", "/api/quests", &body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{value}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], kind, "{value}");
    }

    // Beyond-range floats into int fields answer the size 422 with
    // both exact bounds excluded; digit strings stay the storage 500.
    for value in ["1e30", "9223372036854775808.0", "-9223372036854775808.0"] {
        let body = format!(r#"{{"name": "I", "chain_position": {value}}}"#);
        let (status, _, reply) = send_json(&state, "POST", "/api/quests", &body).await;
        assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY, "{value}");
        let parsed: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(parsed["detail"][0]["type"], "int_parsing_size", "{value}");
    }
    let (status, _, body) = send_json(
        &state,
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
        let (status, _, reply) = send_json(&state, "POST", "/api/quests", body).await;
        assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(reply, b"Internal Server Error");
    }
    let deep = format!(r#"{{"name": {}{}}}"#, "[".repeat(990), "]".repeat(990));
    let (status, _, _) = send_json(&state, "POST", "/api/quests", &deep).await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let shallow = format!(r#"{{"name": {}{}}}"#, "[".repeat(100), "]".repeat(100));
    let (status, _, reply) = send_json(&state, "POST", "/api/quests", &shallow).await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&reply), ["string_type"]);

    // Beyond the parse cap: the generic body-parse 400.
    let too_deep = "[".repeat(50_000) + &"]".repeat(50_000);
    let (status, _, reply) = send_json(&state, "POST", "/api/quests", &too_deep).await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        reply,
        b"{\"detail\":\"There was an error parsing the body\"}"
    );

    // The content-type gate: a foreign maintype with a +json suffix is
    // not JSON (raw-string echo), application subtypes match
    // case-insensitively.
    let (status, _, reply) = send(
        &state,
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
        &state,
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
        &state,
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
        &state,
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

/// The settings/character/equipment surface serves natively over the
/// composed state: the settings reads, the character family over an
/// empty calibration table, and the equipment routes (a consumable
/// write needs no catalogue, so the full write path proves itself
/// against the temp database).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_character_and_equipment_surface_serves_natively() {
    let (state, _dir) = serve_substrate().await;

    // Settings assembly over the fresh data dir: defaults, the live
    // db path, and the workspace version stamp.
    let (status, headers, body) = get(&state, "/api/settings").await;
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

    let (status, _, body) = get(&state, "/api/settings/overlay-position").await;
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
        let (status, _, body) = get(&state, path).await;
        assert_eq!(status, http::StatusCode::OK, "{path}");
        assert_eq!(body, expected.as_bytes(), "{path}");
    }

    // The prospect family's validation ladder: the envelope first (in
    // signature order), then the handler's own 422 details.
    let (status, _, body) = get(&state, "/api/character/prospect").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing", "missing"]);
    let (status, _, body) = get(
        &state,
        "/api/character/prospect?profession=X&target_level=0",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body, b"{\"detail\":\"target_level must be positive\"}");
    let (status, _, body) = get(
        &state,
        "/api/character/prospect?profession=X&target_level=5&slice_type=banana",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        b"{\"detail\":\"slice_type must be global, tag, mob, or weapon\"}"
    );
    let (status, _, body) = get(
        &state,
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
    let (status, _, body) = get(
        &state,
        "/api/character/prospect?profession=X&target_level=5",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body,
        b"{\"error\":\"Profession 'X' not found\",\"rows\":[],\"warnings\":[]}"
    );
    let (status, _, body) = get(&state, "/api/character/profession-optimizer?profession=X").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body,
        b"{\"skills\":[],\"attributes\":[],\"error\":\"Profession 'X' not found\"}"
    );
    let (status, _, body) = get(&state, "/api/character/profession-optimizer").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = get(
        &state,
        "/api/character/profession-path-optimizer?profession=X&target_level=5&ped_budget=1",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        b"{\"detail\":\"Exactly one of target_level or ped_budget must be provided\"}"
    );
    let (status, _, body) = get(
        &state,
        "/api/character/profession-path-optimizer?profession=X&target_level=abc",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["float_parsing"]);

    // Equipment: the search type gate, the empty library, and the
    // catalogue-less validation ladder.
    let (status, _, body) = get(&state, "/api/equipment/search?q=op&type=banana").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Unknown type 'banana'\"}");
    let (status, _, body) = get(&state, "/api/equipment/search?q=o").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]", "short queries return empty before any lookup");
    let (status, _, body) = get(&state, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]");

    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"banana\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["literal_error"]);
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/equipment/library",
        "{\"type\":\"weapon\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"catalog_id required for weapon\"}");
    let (status, _, body) = send_json(
        &state,
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
        &state,
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
        &state,
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
    let (status, _, body) = get(&state, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed[0]["id"], "1");
    let (status, _, body) = send_json(
        &state,
        "PUT",
        "/api/equipment/library/1",
        "{\"type\":\"weapon\",\"catalog_id\":\"x\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body, b"{\"detail\":\"Cannot change equipment type\"}");
    let (status, _, body) = send_json(
        &state,
        "PUT",
        "/api/equipment/library/9",
        "{\"type\":\"consumable\",\"name\":\"X\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Equipment item 9 not found\"}");
    let (status, _, body) = get(&state, "/api/equipment/library/1/detail").await;
    assert_eq!(status, http::StatusCode::OK);
    let detail_shape: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail_shape["weapon"]["name"], "Nutrio Bar");
    assert_eq!(detail_shape["totalCostPerUse"], 0.0);
    let (status, _, body) = get(&state, "/api/equipment/library/9/detail").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body, b"{\"detail\":\"Equipment item 9 not found\"}");
    // Deletes are idempotent acknowledgements, present row or not.
    for item in ["1", "9"] {
        let (status, _, body) = send(
            &state,
            "DELETE",
            &format!("/api/equipment/library/{item}"),
            &[("origin", "tauri://localhost")],
            None,
        )
        .await;
        assert_eq!(status, http::StatusCode::OK);
        assert_eq!(body, b"{\"status\":\"deleted\"}");
    }
    let (status, _, body) = get(&state, "/api/equipment/library").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]");

    // Cost calculation validates in model order (catalog_id first).
    let (status, _, body) = send_json(&state, "POST", "/api/equipment/cost/calculate", "{}").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["missing"]);
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/equipment/cost/calculate",
        "{\"catalog_id\":\"x\",\"type\":\"consumable\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["literal_error"]);

    // The overlay-position write needs the composed config service, which the
    // read-only harness does not compose, so it hits the defensive 503 floor
    // here (the producer harness exercises the native success path).
    let (status, _, _) = send_json(
        &state,
        "PUT",
        "/api/settings/overlay-position",
        "{\"x\":1,\"y\":2}",
    )
    .await;
    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
}

/// Path and body validation aggregate into one envelope (path issue
/// first); decode failures stand alone; deferred 500s (beyond-i64
/// integers, consumed surrogate taints) fire only on otherwise-clean
/// requests, each at its consumption point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_envelopes_aggregate_and_defer_the_backend_way() {
    let (state, _dir) = serve_substrate().await;

    // Path + body field issues, one envelope, path first.
    let (status, _, body) = send_json(
        &state,
        "PUT",
        "/api/equipment/library/abc",
        "{\"type\": \"banana\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing", "literal_error"]);
    let (status, _, body) = send_json(
        &state,
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
    let (status, _, body) = send_json(&state, "POST", "/api/quests/abc/cancel", "5").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        detail_types(&body),
        ["int_parsing", "model_attributes_type"]
    );
    // A decode failure stands alone, dropping the path issue.
    let (status, _, body) = send_json(&state, "PUT", "/api/quests/abc", "{bad").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["json_invalid"]);
    // A missing body aggregates as the missing-["body"] issue.
    let (status, _, body) = send(
        &state,
        "PUT",
        "/api/quests/playlists/abc",
        &[("origin", "tauri://localhost")],
        None,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing", "missing"]);

    // A beyond-i64 path id carries no validation issue, so a bad body
    // still renders its 422 envelope first (aggregation preserved)...
    let (status, _, body) = send_json(
        &state,
        "PUT",
        "/api/quests/99999999999999999999999999",
        "{\"cooldown_hours\": \"x\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["float_parsing"]);
    // ...and on an otherwise-clean request it resolves to a clean 404
    // (the id can never name a stored row), a deferred 404.
    let (status, _, _) = send_json(
        &state,
        "PUT",
        "/api/quests/99999999999999999999999999",
        "{\"name\": \"X\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);

    // Beyond-i64 BODY integers answer the deferred 500 across the
    // dump builders (quest update field, playlist quest_ids, playlist
    // item ids, equipment ints), only after validation passes.
    let (status, _, _) = send_json(&state, "POST", "/api/quests", "{\"name\": \"Q\"}").await;
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
        let (status, _, reply) = send_json(&state, method, path, body).await;
        assert_eq!(
            status,
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {path}: {}",
            String::from_utf8_lossy(&reply)
        );
    }
    // A playlist update whose only set field overflows must not slip
    // through as a no-op update.
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/quests/playlists",
        "{\"name\": \"Pl\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    for body in [
        "{\"estimated_minutes\": 99999999999999999999999999}",
        "{\"quest_ids\": [99999999999999999999999999]}",
        "{\"items\": [{\"quest_id\": 99999999999999999999999999}]}",
    ] {
        let (status, _, _) = send_json(&state, "PUT", "/api/quests/playlists/1", body).await;
        assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    }

    // A CONSUMED surrogate-tainted field answers the 500 at its
    // consumption point; an UNUSED one flows (the lookup miss answers
    // its renderable 404 on the empty store).
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/equipment/library",
        "{\"type\": \"weapon\", \"catalog_id\": \"ta\\ud800int\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let (status, _, body) = send_json(
        &state,
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
        &state,
        "POST",
        "/api/equipment/cost/calculate",
        "{\"catalog_id\": \"ta\\ud800int\", \"type\": \"healing\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/equipment/library",
        "{\"type\": \"consumable\", \"name\": \"ta\\ud800int\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::INTERNAL_SERVER_ERROR);

    // The calibrate codec message splits singular/plural on the
    // surrogate RUN length, with the exact position arithmetic.
    let (status, _, body) = send_json(
        &state,
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
        &state,
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
        &state,
        "/api/character/prospect?profession=X&target_level=5&markup_uplift=-0.1",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = get(
        &state,
        "/api/character/prospect?profession=X&target_level=5&markup_uplift=0",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
}

// ── Tracking session-edit write adapters, end-to-end and hermetic ──────
//
// The five edit adapters (`native.rs`) and the HydrationState method
// wrappers (`tracking_routes.rs`) were once driven end-to-end only by
// the now-retired cross-language battery, so the hermetic mutation
// campaign never exercised them. This test seeds an ended + an active
// session straight into the substrate's database and drives every edit
// through the public port, asserting the RESPONSE BODY fields (not just
// the status) so an adapter/wrapper degraded to `Default::default()`
// (an empty `Response`) is caught. Activate vs deactivate produce
// distinct results from the same wildcard registration, distinguishing
// the suffix dispatch and the path splitter.

const ENDED_MOB: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const ENDED_LOOT: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ACTIVE: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

async fn open_pool(path: std::path::PathBuf) -> SqlitePool {
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

/// Seed the substrate's own database (the schema was created by
/// `Db::open` in `serve_substrate`) with the fixtures every edit needs:
///   - `ENDED_MOB` (is_active=0): two "Atrox" kills (rename target) and
///     one "Argo" kill whose `original_mob_name` is "Wolf" (restore
///     target);
///   - `ENDED_LOOT` (is_active=0): a kill with an ACTIVE "AnimalOil"
///     loot row (deactivate target) and a kill with a DEACTIVATED "Old
///     Hide" row (activate target), plus an item name carrying a slash;
///   - `ACTIVE` (is_active=1): the 409 case.
async fn seed_edits(pool: &SqlitePool) {
    let base = 1_750_000_000.0_f64;
    for (id, active) in [(ENDED_MOB, 0_i64), (ENDED_LOOT, 0), (ACTIVE, 1)] {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(base)
        .bind(if active == 0 { Some(base + 3600.0) } else { None })
        .bind(active)
        .bind(1.0_f64)
        .bind(0.0_f64)
        .bind(0.0_f64)
        .bind("mob")
        .bind(base + 3600.0)
        .execute(pool)
        .await
        .expect("seed session");
    }

    // ENDED_MOB: two Atrox kills (rename) + one renamed Argo (restore).
    for i in 0..2 {
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(format!("k-mob-{i}")).bind(ENDED_MOB).bind("Atrox").bind("").bind("")
        .bind(base + i as f64).bind(10_i64).bind(50.0).bind(0.0).bind(0_i64)
        .bind(0.5).bind(0.0).bind(3.0).bind(0_i64).bind(0_i64).bind(Option::<String>::None)
        .execute(pool).await.expect("seed mob kill");
    }
    sqlx::query(
        "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind("k-mob-renamed").bind(ENDED_MOB).bind("Argo").bind("").bind("")
    .bind(base + 10.0).bind(10_i64).bind(50.0).bind(0.0).bind(0_i64)
    .bind(0.5).bind(0.0).bind(3.0).bind(0_i64).bind(0_i64).bind(Some("Wolf"))
    .execute(pool).await.expect("seed renamed kill");

    // ENDED_LOOT: K_LD carries an ACTIVE "AnimalOil" (deactivate
    // target, value 2.0, parent loot_total 5.0); K_LA carries a
    // DEACTIVATED "OldHide" (activate target, value 3.0, parent
    // loot_total 4.0) plus a slash-bearing item name.
    sqlx::query(
        "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind("k-ld").bind(ENDED_LOOT).bind("Atrox").bind("").bind("")
    .bind(base).bind(10_i64).bind(50.0).bind(0.0).bind(0_i64)
    .bind(0.5).bind(0.0).bind(5.0).bind(0_i64).bind(0_i64).bind(Option::<String>::None)
    .execute(pool).await.expect("seed ld kill");
    sqlx::query(
        "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind("k-la").bind(ENDED_LOOT).bind("Atrox").bind("").bind("")
    .bind(base + 1.0).bind(10_i64).bind(50.0).bind(0.0).bind(0_i64)
    .bind(0.5).bind(0.0).bind(4.0).bind(0_i64).bind(0_i64).bind(Option::<String>::None)
    .execute(pool).await.expect("seed la kill");
    sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
        .bind("k-ld").bind("AnimalOil").bind(1_i64).bind(2.0).bind(0_i64).bind(Option::<f64>::None)
        .execute(pool).await.expect("seed active loot");
    sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
        .bind("k-la").bind("OldHide").bind(1_i64).bind(3.0).bind(0_i64).bind(Some(base + 50.0))
        .execute(pool).await.expect("seed deactivated loot");
    // A slash-bearing item name: the `{item_name:path}` converter KEEPS
    // the decoded slash rather than 404-ing it (the session id, a single
    // segment, still de-matches a slash).
    sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
        .bind("k-ld").bind("Metal/Wire").bind(1_i64).bind(1.0).bind(0_i64).bind(Option::<f64>::None)
        .execute(pool).await.expect("seed slash loot");
}

fn body_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("response body is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_session_edits_drive_the_adapters_and_wrappers_end_to_end() {
    let (state, dir) = serve_substrate().await;
    let pool = open_pool(dir.path().join("entropia_orme.db")).await;
    seed_edits(&pool).await;

    // ── rename-mob: success body (sessionId / mobName / killCount) ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_MOB}/rename-mob"),
        "{\"fromMobName\":\"Atrox\",\"toMobName\":\"Daikiba\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], ENDED_MOB);
    assert_eq!(v["mobName"], "Daikiba");
    assert_eq!(v["killCount"], 2);

    // ── restore-mob: the "Argo" kill restores to its "Wolf" original ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_MOB}/restore-mob"),
        "{\"currentMobName\":\"Argo\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], ENDED_MOB);
    assert_eq!(v["mobName"], "Wolf");
    assert_eq!(v["killCount"], 1);

    // ── loot-item deactivate: full body, incl. signed delta + totals ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_LOOT}/loot-item/AnimalOil/deactivate"),
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], ENDED_LOOT);
    assert_eq!(v["itemName"], "AnimalOil");
    assert_eq!(v["affectedRows"], 1);
    assert_eq!(v["totalValueDelta"], -2.0);
    // K_LD 5.0 - 2.0 = 3.0; K_LA still 4.0 -> 7.0.
    assert_eq!(v["sessionTotalReturns"], 7.0);

    // ── loot-item activate on a DEACTIVATED row: the OTHER suffix arm,
    //    distinct result -> kills the wildcard split + suffix dispatch ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_LOOT}/loot-item/OldHide/activate"),
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], ENDED_LOOT);
    assert_eq!(v["itemName"], "OldHide");
    assert_eq!(v["affectedRows"], 1);
    assert_eq!(v["totalValueDelta"], 3.0);
    // K_LD now 3.0; K_LA 4.0 + 3.0 = 7.0 -> 10.0.
    assert_eq!(v["sessionTotalReturns"], 10.0);

    // ── loot-item with a slash in the {item_name:path} segment: the
    //    converter KEEPS the slash, so the item is found and flipped ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_LOOT}/loot-item/Metal/Wire/deactivate"),
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["itemName"], "Metal/Wire");
    assert_eq!(v["affectedRows"], 1);

    // ── armour-cost: echoes round(cost, 2), NOT the new total ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_LOOT}/armour-cost"),
        "{\"cost\":2.5}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], ENDED_LOOT);
    assert_eq!(v["armourCost"], 2.5);

    // ── 404: a missing session on every guarded edit ──
    let missing = "00000000-0000-4000-8000-000000000000";
    for (path, body) in [
        (
            format!("/api/tracking/session/{missing}/rename-mob"),
            "{\"fromMobName\":\"a\",\"toMobName\":\"b\"}",
        ),
        (
            format!("/api/tracking/session/{missing}/restore-mob"),
            "{\"currentMobName\":\"a\"}",
        ),
        (
            format!("/api/tracking/session/{missing}/loot-item/AnimalOil/deactivate"),
            "",
        ),
        (
            format!("/api/tracking/session/{missing}/armour-cost"),
            "{\"cost\":1.0}",
        ),
    ] {
        let (status, _, _) = send_json(&state, "POST", &path, body).await;
        assert_eq!(
            status,
            http::StatusCode::NOT_FOUND,
            "missing session 404: {path}"
        );
    }

    // ── 409: an ACTIVE session on the three guarded mob/loot edits
    //    (armour-cost deliberately omits the guard) ──
    for (path, body) in [
        (
            format!("/api/tracking/session/{ACTIVE}/rename-mob"),
            "{\"fromMobName\":\"a\",\"toMobName\":\"b\"}",
        ),
        (
            format!("/api/tracking/session/{ACTIVE}/restore-mob"),
            "{\"currentMobName\":\"a\"}",
        ),
        (
            format!("/api/tracking/session/{ACTIVE}/loot-item/AnimalOil/deactivate"),
            "",
        ),
    ] {
        let (status, _, _) = send_json(&state, "POST", &path, body).await;
        assert_eq!(
            status,
            http::StatusCode::CONFLICT,
            "active session 409: {path}"
        );
    }

    // ── 400: a blank mob name (the validated-then-trimmed empty leg) ──
    let (status, _, _) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_MOB}/rename-mob"),
        "{\"fromMobName\":\"   \",\"toMobName\":\"x\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);

    // ── 404: the wildcard tail matches NEITHER suffix ──
    let (status, _, _) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{ENDED_LOOT}/loot-item/Foo/bogus"),
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
}

// ── Quest-link routes: hermetic body-asserting coverage ──

const QL_QUEST: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd"; // single_quest -> accept
const QL_DECLINE: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"; // decline

/// Seed the quest-link fixtures into the substrate's own database: one
/// quest (id 2), a single-quest completion for `QL_QUEST` (the accept
/// target) and a separate session `QL_DECLINE` with one completion (the
/// decline target). No playlists are needed for the single-quest path.
async fn seed_quest_link(pool: &SqlitePool) {
    for id in [QL_QUEST, QL_DECLINE] {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
             VALUES(?,1000.0,4600.0,0,0,0,0,'mob',4600.0)",
        )
        .bind(id)
        .execute(pool)
        .await
        .expect("seed quest-link session");
    }
    sqlx::query(
        "INSERT INTO quests(id,name,planet,is_active,created_at,category) VALUES(2,'Quest 2','Calypso',1,1000.0,'kill')",
    )
    .execute(pool)
    .await
    .expect("seed quest");
    for id in [QL_QUEST, QL_DECLINE] {
        sqlx::query(
            "INSERT INTO session_quest_completions(session_id,quest_id,completed_at) VALUES(?,2,2000.0)",
        )
        .bind(id)
        .execute(pool)
        .await
        .expect("seed completion");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quest_link_routes_drive_the_adapters_and_handlers_end_to_end() {
    let (state, dir) = serve_substrate().await;
    let pool = open_pool(dir.path().join("entropia_orme.db")).await;
    seed_quest_link(&pool).await;

    // ── GET suggestion for a single_quest session: the 7-field body ──
    let (status, headers, body) = get(
        &state,
        &format!("/api/tracking/session/{QL_QUEST}/quest-link-suggestion"),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    assert_eq!(v["sessionId"], QL_QUEST);
    assert_eq!(v["suggestionType"], "quest");
    assert_eq!(v["reason"], "single_quest");
    assert_eq!(v["questId"], "2");
    assert_eq!(v["questName"], "Quest 2");
    assert!(v["playlistId"].is_null());
    assert!(v["playlistName"].is_null());
    // ETag-scoped: the read carries a strong ETag for the 304 leg below.
    let etag = headers
        .get(http::header::ETAG)
        .expect("suggestion etag")
        .to_str()
        .unwrap()
        .to_string();

    // ── GET 304: re-fetch with the prior ETag -> empty 304 ──
    let (status, _, body) = request(
        &state,
        "GET",
        &format!("/api/tracking/session/{QL_QUEST}/quest-link-suggestion"),
        &[("if-none-match", etag.as_str())],
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // ── POST accept: persists; replies linked + linkType + questId ──
    let (status, headers, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{QL_QUEST}/quest-link"),
        "{\"action\":\"accept\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    // The write is a plain 200: no ETag (unlike the GET).
    assert!(!headers.contains_key(http::header::ETAG));
    let v = body_json(&body);
    assert_eq!(v["sessionId"], QL_QUEST);
    assert_eq!(v["status"], "linked");
    assert_eq!(v["linkType"], "quest");
    assert_eq!(v["questId"], "2");
    assert_eq!(v["questName"], "Quest 2");

    // ── POST decline on another session: EXACTLY {sessionId, status} ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{QL_DECLINE}/quest-link"),
        "{\"action\":\"decline\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    let v = body_json(&body);
    let object = v.as_object().expect("decline body is an object");
    assert_eq!(
        object.len(),
        2,
        "decline omits the link fields entirely (exactly sessionId + status), got {object:?}"
    );
    assert_eq!(v["sessionId"], QL_DECLINE);
    assert_eq!(v["status"], "declined");

    // ── 404: a missing session on BOTH routes ──
    let missing = "00000000-0000-4000-8000-000000000000";
    let (status, _, body) = get(
        &state,
        &format!("/api/tracking/session/{missing}/quest-link-suggestion"),
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body_json(&body)["detail"], "Session not found");
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{missing}/quest-link"),
        "{\"action\":\"decline\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body_json(&body)["detail"], "Session not found");

    // ── 400: an unrecognised action ──
    let (status, _, body) = send_json(
        &state,
        "POST",
        &format!("/api/tracking/session/{QL_QUEST}/quest-link"),
        "{\"action\":\"frobnicate\"}",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&body)["detail"],
        "Action must be 'accept' or 'decline'"
    );
}

// ── Tracking PRODUCER routes (start / stop / manual-mob-suggestions) ──
//
// These three reach the live `Arc<HuntTracker>` (and, for the
// suggestions, the bundled mobs catalogue) rather than the read-only
// database surface, so the hermetic harness above (no tracker) cannot
// drive them: it composes hydration only, so they answer the 503
// service-unavailable floor.
// This block boots a SEPARATE substrate that wires both the read surface
// AND a live tracker over a shared single-owner pool, plus a temp data
// dir carrying a `mobs.json` (the suggestions catalogue) and a
// `settings.json` (the attribution gate's config read), then drives the
// full leg set through the public port asserting RESPONSE BODY fields so
// a wrapper degraded to an empty `Response` is caught. The retired
// cross-language oracle proved this surface byte-identical; the
// committed goldens now hold it.

/// A substrate composed with BOTH the read surface and a live tracker
/// over a shared pool. `config_json` seeds `settings.json` (the
/// attribution gate + the idle tag-mode leg read it); a small mobs
/// catalogue seeds the suggestions lookup.
async fn serve_producer_substrate(config_json: &str) -> (Arc<AppState>, tempfile::TempDir) {
    use eo_services::event_bus::EventBus;
    use std::sync::Mutex;

    use eo_services::clock::Clock;
    use eo_services::config_service::ConfigService;
    use eo_services::hotbar_listener::HotbarListener;
    use eo_services::keystroke_source::MockKeystrokeSource;
    use eo_services::repair_ocr::{RepairOcrService, RepairProviders};
    use eo_services::skill_panel::BgrImage;
    use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
    use eo_services::skill_tracker::SkillTracker;
    use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
    use eo_services::tracker::{naive_to_epoch, HuntTracker, Providers};

    let dir = tempfile::tempdir().expect("temp dir");
    // The attribution gate and the idle tag-mode leg read settings.json
    // from the data dir; seed it before composition.
    std::fs::write(dir.path().join("settings.json"), config_json).expect("seed settings");
    // The suggestions lookup reads the mobs catalogue from the game-data
    // store directory.
    let store_dir = dir.path().join("snapshot");
    std::fs::create_dir_all(&store_dir).expect("store dir");
    std::fs::write(
        store_dir.join("mobs.json"),
        r#"[{"id":1,"species":{"name":"Atrox"},"maturities":[{"name":"Young"},{"name":"Old"}]}]"#,
    )
    .expect("seed mobs");

    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .expect("temp db opens");
    let game_data = Arc::new(GameDataStore::new(&store_dir).expect("mobs store"));
    let clock = Arc::new(RealClock::new());
    let bus = Arc::new(EventBus::new());
    // The tracker shares the substrate's single-owner pool (one
    // connection, serialised access), exactly as composition wires it.
    let tracker = HuntTracker::new(
        bus.clone(),
        db.pool().clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
        Providers::default(),
    )
    .expect("tracker builds over the fresh pool");
    // The skill tracker (codex suppress-next) and the settings writer (the
    // config-write routes) share the same pool, clock, and data dir, exactly
    // as composition wires them, so the write routes serve natively here.
    let skill_tracker = SkillTracker::new(
        &bus,
        db.pool().clone(),
        tokio::runtime::Handle::current(),
        clock.clone(),
    );
    let config_service = Arc::new(Mutex::new(
        ConfigService::new(dir.path()).expect("config service opens"),
    ));

    // The manual skill scan and repair-cost reader, composed over
    // deterministic test providers (no real screen capture or OCR): the
    // engine reads available, a fixed region is found, a capture yields fixed
    // PNG bytes, and extraction yields two skills. This exercises the full
    // HTTP state machine and the byte-faithful projection hermetically. The
    // completion callback persists through the shared pool exactly as
    // composition wires it (bridging onto this runtime from the handler).
    let skill_scan = SkillScanManual::new(
        ScanProviders {
            engine_available: Arc::new(|| true),
            skill_region: Arc::new(|| Some(([0, 0], [100, 200]))),
            capture_region: Arc::new(|_| Some(SCAN_CAPTURE_PNG.to_vec())),
            extract_page_levels: Arc::new(|_| {
                vec![("Anatomy".to_string(), 40.0), ("Rifle".to_string(), 100.5)]
            }),
        },
        clock.clone(),
        Some(bus.clone()),
        None,
        0,
    );
    let completion_pool = db.pool().clone();
    let completion_clock = clock.clone();
    let completion_runtime = tokio::runtime::Handle::current();
    skill_scan.set_completion_callback(Arc::new(move |levels: &[(String, f64)]| {
        let levels = levels.to_vec();
        let pool = completion_pool.clone();
        let scan_time = naive_to_epoch(completion_clock.now());
        let fut = async move {
            eo_services::scan_completion::complete_skill_scan(&pool, &levels, scan_time).await
        };
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| completion_runtime.block_on(fut))
        } else {
            completion_runtime.block_on(fut)
        };
        result.map(|_| ()).map_err(|err| err.to_string())
    }));
    let repair_ocr = Arc::new(RepairOcrService::new(RepairProviders {
        repair_region: Arc::new(|| Some(([10, 20], [110, 60]))),
        capture_region: Arc::new(|_, _, _, _| {
            Some(BgrImage {
                data: vec![0; 12],
                h: 2,
                w: 2,
            })
        }),
        read_text: Arc::new(|_| Some(("2,20 PED".to_string(), 0.97))),
    }));
    // The spacebar-capture listener over a mock source: the toggle route
    // drives its enabled state, which the mock source's start/stop honour.
    let spacebar = SpacebarCaptureListener::new(
        skill_scan.clone(),
        Some(Arc::new(MockKeystrokeSource::new())),
    );
    // The hotbar listener (the snapshot route reads its running state). A mock
    // source and no resolver, never enabled, so it reports not-running.
    let hotbar = HotbarListener::new(
        bus.clone(),
        Some(Arc::new(MockKeystrokeSource::new())),
        None,
    );

    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(db.pool().clone()),
        game_data,
        clock,
        dir.path().to_path_buf(),
    ));

    let state = Arc::new(
        AppState::new(0)
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
    (state, dir)
}

/// A settings.json with hotbar mode and slot "1" bound: the attribution
/// gate passes (`_validate_hotbar`), so `/start` succeeds without a
/// configured trifecta.
const HOTBAR_BOUND_CONFIG: &str = r#"{"hotbar_hooks_enabled": true, "hotbar": {"1": 7}}"#;

/// The fixed bytes the test scan capturer returns; the capture-PNG route
/// serves them verbatim, so the route test asserts the body and the strong
/// ETag (the SHA-256 of these bytes) against them.
const SCAN_CAPTURE_PNG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_producer_lifecycle_and_suggestions_serve_natively() {
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;

    // ── start: 200 with the lifecycle acknowledgement (plain, no ETag) ──
    let (status, headers, body) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "start replies plain (POST is outside the ETag middleware)"
    );
    let started = body_json(&body);
    let session_id = started["session_id"]
        .as_str()
        .expect("session_id is a string")
        .to_string();
    assert!(!session_id.is_empty());
    assert_eq!(started["status"], "active");
    assert!(started["started_at"].as_str().is_some());

    // ── start again while active: 409 "Session already active" ──
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    assert_eq!(body_json(&body)["detail"], "Session already active");

    // ── stop: 200 with the stop acknowledgement, same session id ──
    let (status, headers, body) = send_json(&state, "POST", "/api/tracking/stop", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    let stopped = body_json(&body);
    assert_eq!(stopped["session_id"], session_id);
    assert!(stopped["started_at"].as_str().is_some());
    assert!(stopped["ended_at"].as_str().is_some());
    assert_eq!(stopped["kill_count"], 0);

    // ── stop with no active session: 409 "No active session" ──
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/stop", "").await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    assert_eq!(body_json(&body)["detail"], "No active session");

    // ── manual-mob-suggestions success: ETag-scoped 200 over the
    //    catalogue (mob mode -> no tag-mode gate) ──
    let (status, headers, body) = get(&state, "/api/tracking/manual-mob-suggestions?q=atrox").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        headers.contains_key(http::header::ETAG),
        "the 200 suggestions leg is ETag-scoped (a GET under /api/tracking)"
    );
    let suggestions = body_json(&body);
    let displays: Vec<&str> = suggestions
        .as_array()
        .expect("array")
        .iter()
        .map(|row| row["display"].as_str().unwrap())
        .collect();
    assert_eq!(displays, ["Old Atrox", "Young Atrox"]);
    assert_eq!(suggestions[0]["species"], "Atrox");
    assert_eq!(suggestions[0]["maturity"], "Old");

    // ── the empty-q short-circuit: 200 [], still ETag-scoped ──
    let (status, headers, body) = get(&state, "/api/tracking/manual-mob-suggestions?q=").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));
    assert_eq!(body, b"[]");
    // No `q` at all behaves the same.
    let (status, _, body) = get(&state, "/api/tracking/manual-mob-suggestions").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, b"[]");

    // ── the limit clamp: limit=99 clamps to 20 (here only 2 rows exist,
    //    so both surface); limit=0 clamps to 1 (one row) ──
    let (status, _, body) = get(
        &state,
        "/api/tracking/manual-mob-suggestions?q=atrox&limit=99",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body).as_array().unwrap().len(), 2);
    let (status, _, body) = get(
        &state,
        "/api/tracking/manual-mob-suggestions?q=atrox&limit=0",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body).as_array().unwrap().len(), 1);

    // ── 422: an unparseable limit (the adapter's int_parsing envelope) ──
    let (status, _, body) = get(&state, "/api/tracking/manual-mob-suggestions?q=a&limit=abc").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail_types(&body), ["int_parsing"]);

    // ── conditional GET: the suggestions 200 earns a 304 on its ETag ──
    let (_, headers, _) = get(&state, "/api/tracking/manual-mob-suggestions?q=atrox").await;
    let etag = headers
        .get(http::header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let (status, _, body) = request(
        &state,
        "GET",
        "/api/tracking/manual-mob-suggestions?q=atrox",
        &[("if-none-match", etag.as_str())],
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_start_rejects_an_unready_attribution() {
    // No hotbar, no configured trifecta (the default-preset slots are
    // null): the attribution gate fails with the trifecta 400.
    let (state, _dir) = serve_producer_substrate("{}").await;
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&body)["detail"],
        "Trifecta attribution requires a configured small weapon, big weapon, and healing tool"
    );
    // Hotbar mode with NO bound slot: the hotbar-specific 400.
    let (state, _dir) = serve_producer_substrate(r#"{"hotbar_hooks_enabled": true}"#).await;
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&body)["detail"],
        "Bind at least one hotbar slot in the Equipment page before tracking."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_mob_suggestions_tag_mode_409_precedes_the_empty_q_shortcut() {
    // Idle tag mode: the live config's `mob_tracking_mode == "tag"` gates
    // BEFORE the empty-q short-circuit, so even `q=` 409s (not []).
    let (state, _dir) =
        serve_producer_substrate(r#"{"mob_tracking_mode": "tag", "mob_tracking_tag": "Boss"}"#)
            .await;
    for path in [
        "/api/tracking/manual-mob-suggestions?q=atrox",
        "/api/tracking/manual-mob-suggestions?q=",
        "/api/tracking/manual-mob-suggestions",
    ] {
        let (status, headers, body) = get(&state, path).await;
        assert_eq!(status, http::StatusCode::CONFLICT, "{path}");
        assert_eq!(
            body_json(&body)["detail"],
            "Tag mode disables manual mob selection",
            "{path}"
        );
        assert!(
            !headers.contains_key(http::header::ETAG),
            "the 409 leg is non-2xx, so it carries no ETag: {path}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn producer_routes_answer_503_without_a_composed_tracker() {
    // The read-only harness composes hydration but NO tracker, so the three
    // producer routes hit the defensive service-unavailable floor (503),
    // proving the adapters require the live tracker. The production binary
    // publishes the router only once every service is composed, so this floor
    // is unreached on the normal startup path.
    let (state, _dir) = serve_substrate().await;
    let (status, _, _) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _) = send_json(&state, "POST", "/api/tracking/stop", "").await;
    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _) = get(&state, "/api/tracking/manual-mob-suggestions?q=a").await;
    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
}

/// Read the substrate's `settings.json` (the config-write target) as JSON.
fn read_settings(dir: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(dir.join("settings.json")).expect("settings.json reads");
    serde_json::from_str(&raw).expect("settings.json parses")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_write_routes_serve_natively_idle_mob_mode() {
    // Mob-mode config, no active session. Drives the native config-write
    // handlers (over the composed ConfigService + skill tracker) and pins
    // their responses + the settings.json they persist; the dead proxy
    // (port 9) means any handler that fell back would 502 instead.
    let (state, dir) = serve_producer_substrate("{}").await;

    // overlay-position: plain 200 {"ok": true}, exact coordinates persisted.
    let (status, headers, body) = send_json(
        &state,
        "PUT",
        "/api/settings/overlay-position",
        r#"{"x": 7, "y": 9}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    assert_eq!(body_json(&body), json!({"ok": true}));
    let cfg = read_settings(dir.path());
    assert_eq!(cfg["overlay_x"], 7);
    assert_eq!(cfg["overlay_y"], 9);

    // An unparseable coordinate is the 422 int_parsing envelope.
    let (status, _, _) = send_json(
        &state,
        "PUT",
        "/api/settings/overlay-position",
        r#"{"x": "nope", "y": 0}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);

    // manual-mob-lock: a catalogue match locks; the selection persists.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/manual-mob-lock",
        r#"{"species": "Atrox", "maturity": "Old"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body_json(&body),
        json!({"mobName": "Old Atrox", "species": "Atrox", "maturity": "Old"})
    );
    let cfg = read_settings(dir.path());
    assert_eq!(cfg["manual_mob_species"], "Atrox");
    assert_eq!(cfg["manual_mob_maturity"], "Old");

    // A mob absent from the catalogue is the 400.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/manual-mob-lock",
        r#"{"species": "Notamob", "maturity": ""}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&body)["detail"],
        "Mob is not present in the catalogue"
    );

    // tag-lock outside tag mode is the 409.
    let (status, _, body) =
        send_json(&state, "POST", "/api/tracking/tag-lock", r#"{"tag": "X"}"#).await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    assert_eq!(body_json(&body)["detail"], "Tag mode is not enabled");

    // release-mob in idle manual mode returns the stored display and clears it.
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/release-mob", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body), json!({"released": "Old Atrox"}));
    let cfg = read_settings(dir.path());
    assert_eq!(cfg["manual_mob_species"], "");
    assert_eq!(cfg["manual_mob_maturity"], "");

    // release-mob again with nothing stored: released is null.
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/release-mob", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body), json!({ "released": Value::Null }));

    // codex claim/meta over a catalogue with no codex data: the service's
    // not-found ValueError is the 400 (exercises the suppress-next handlers
    // and their skill-tracker dependency; idle, so no suppression fires).
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/codex/claim",
        r#"{"species_name": "Notaspecies", "rank": 1, "skill_name": "Anatomy"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    // unclaim over a species with no claimed rank: the service's
    // nothing-to-unclaim ValueError is the 400 (exercises the route's
    // registration and error mapping).
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/codex/unclaim",
        r#"{"species_name": "Notaspecies"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&body)["detail"],
        "No claimed rank to unclaim for 'Notaspecies'"
    );
    let (status, _, _) = send_json(
        &state,
        "POST",
        "/api/codex/meta/claim",
        r#"{"attribute_name": "Notanattribute"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_write_routes_serve_natively_idle_tag_mode() {
    // Tag-mode config, no active session: the tag-lock success/empty legs,
    // the manual-mob-lock tag-mode 409, and the idle-tag release branch.
    let (state, dir) = serve_producer_substrate(r#"{"mob_tracking_mode": "tag"}"#).await;

    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/tag-lock",
        r#"{"tag": "Daily Hunt"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body), json!({"tag": "Daily Hunt"}));
    assert_eq!(read_settings(dir.path())["mob_tracking_tag"], "Daily Hunt");

    // An all-whitespace tag is the 400.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/tag-lock",
        r#"{"tag": "   "}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&body)["detail"], "Tag cannot be empty");

    // manual-mob-lock is disabled in tag mode: the 409.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/manual-mob-lock",
        r#"{"species": "Atrox", "maturity": ""}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    assert_eq!(
        body_json(&body)["detail"],
        "Tag mode disables manual mob selection"
    );

    // release-mob in idle tag mode returns the trimmed tag and clears it.
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/release-mob", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body), json!({"released": "Daily Hunt"}));
    assert_eq!(read_settings(dir.path())["mob_tracking_tag"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_write_routes_serve_natively_active_session() {
    // An active mob-mode session: the tracker in-memory calls fire, the
    // tag-lock active-session 409 leg, and release-mob's active branch
    // (clears the manual selection, not the tag).
    let (state, dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;
    let (status, _, _) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::OK);

    // manual-mob-lock while tracking: 200, sets the live tracker + config.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/tracking/manual-mob-lock",
        r#"{"species": "Atrox", "maturity": "Young"}"#,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body)["mobName"], "Young Atrox");
    assert_eq!(read_settings(dir.path())["manual_mob_species"], "Atrox");

    // tag-lock against a mob-mode active session: the session-snapshot 409.
    let (status, _, body) =
        send_json(&state, "POST", "/api/tracking/tag-lock", r#"{"tag": "X"}"#).await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    assert_eq!(
        body_json(&body)["detail"],
        "Active session is not in tag mode"
    );

    // release-mob in an active non-tag session clears the MANUAL selection
    // (the active-non-tag branch), not the tag.
    let (status, _, _) = send_json(&state, "POST", "/api/tracking/release-mob", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        read_settings(dir.path())["manual_mob_species"],
        "",
        "an active mob-mode release clears the manual selection, not the tag"
    );
}

/// The full manual-scan state machine over the native arm: each verb's status
/// code, ETag scoping (GETs in the /api/scan ETag prefix carry it, the POST
/// verbs do not), the response-model field order, and the logical refusals
/// riding the plain-200 body exactly as the reference returns the dict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_skills_state_machine_serves_natively() {
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;

    // status: 200 + the conditional-GET contract, idle, full field set in the
    // ScanManualStatus declaration order.
    let (status, headers, body) = get(&state, "/api/scan/skills/status").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));
    assert_eq!(
        headers.get(http::header::CACHE_CONTROL).unwrap(),
        "no-cache"
    );
    let resting = body_json(&body);
    assert_eq!(resting["phase"], "idle");
    assert_eq!(resting["captured_pages"], 0);
    assert_eq!(resting["configured"], true);
    assert_eq!(resting["game_window_present"], true);
    let keys: Vec<&str> = resting
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "active",
            "processing",
            "captured_pages",
            "expected_pages",
            "last_scan_time",
            "skills_count",
            "configured",
            "game_window_present",
            "phase",
            "processing_progress",
            "has_pending_result",
            "error",
        ]
    );

    // A status re-read with the matching validator is a 304 (the conditional
    // GET the polling overlay relies on).
    let etag = headers
        .get(http::header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let (status, _, body) = request(
        &state,
        "GET",
        "/api/scan/skills/status",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // capture before start: a plain 200 carrying the refusal (no ETag: POST).
    let (status, headers, body) = send_json(&state, "POST", "/api/scan/skills/capture", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    assert_eq!(
        body_json(&body)["error"],
        "No active scan: call start first"
    );

    // start with 2 pages: capturing.
    let (status, headers, body) =
        send_json(&state, "POST", "/api/scan/skills/start?page_count=2", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    let started = body_json(&body);
    assert_eq!(started["phase"], "capturing");
    assert_eq!(started["expected_pages"], 2);

    // capture twice: page/captured present, AFTER the inherited status fields
    // (the ScanCaptureResult subclass order).
    let (_, _, body) = send_json(&state, "POST", "/api/scan/skills/capture", "").await;
    let first = body_json(&body);
    assert_eq!(first["page"], 1);
    assert_eq!(first["captured"], true);
    let keys: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys[0], "active", "the inherited status fields lead");
    assert_eq!(&keys[keys.len() - 2..], ["page", "captured"]);
    let (_, _, body) = send_json(&state, "POST", "/api/scan/skills/capture", "").await;
    assert_eq!(body_json(&body)["captured_pages"], 2);

    // pending before processing: the reference's 404 (a non-2xx, so no ETag).
    let (status, headers, body) = get(&state, "/api/scan/skills/pending").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert!(!headers.contains_key(http::header::ETAG));
    assert_eq!(body_json(&body)["detail"], "No pending skill scan result");

    // process: kicks extraction off on the worker thread.
    let (status, _, _) = send_json(&state, "POST", "/api/scan/skills/process", "").await;
    assert_eq!(status, http::StatusCode::OK);

    // The worker settles the held result; poll the status to review.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (_, _, body) = get(&state, "/api/scan/skills/status").await;
        if body_json(&body)["phase"] == "awaiting_review" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the scan never settled to review"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // pending: 200 + ETag, the held result as a {name: level} object in
    // first-seen order.
    let (status, headers, body) = get(&state, "/api/scan/skills/pending").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));
    let pending = body_json(&body);
    assert_eq!(pending["skills"]["Anatomy"], 40.0);
    assert_eq!(pending["skills"]["Rifle"], 100.5);

    // accept: 200 with the persisted count, fields in ScanAcceptResult order.
    let (status, headers, body) = send_json(&state, "POST", "/api/scan/skills/accept", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    let accepted = body_json(&body);
    assert_eq!(accepted["ok"], true);
    assert_eq!(accepted["skills_persisted"], 2);
    let keys: Vec<&str> = accepted
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["ok", "skills_persisted"]);

    // The accepted scan settled the resting status: idle, two skills.
    let (_, _, body) = get(&state, "/api/scan/skills/status").await;
    let settled = body_json(&body);
    assert_eq!(settled["phase"], "idle");
    assert_eq!(settled["skills_count"], 2);
}

/// The capture-PNG read serves the stored bytes under the same conditional-GET
/// contract the JSON reads carry (the ETag middleware covers any media type in
/// scope), 404s a missing page, and 422s an unparseable one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_capture_png_serves_with_etag_and_refuses() {
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;
    send_json(&state, "POST", "/api/scan/skills/start?page_count=2", "").await;
    send_json(&state, "POST", "/api/scan/skills/capture", "").await;

    // capture/1: 200 image/png, the strong ETag of the bytes, the bytes.
    let (status, headers, body) = get(&state, "/api/scan/skills/capture/1").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        headers.get(http::header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(
        headers.get(http::header::CACHE_CONTROL).unwrap(),
        "no-cache"
    );
    let etag = headers
        .get(http::header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        etag,
        eo_http::hydration::compute_strong_etag(SCAN_CAPTURE_PNG)
    );
    assert_eq!(body, SCAN_CAPTURE_PNG);

    // The matching validator: 304, empty body.
    let (status, _, body) = request(
        &state,
        "GET",
        "/api/scan/skills/capture/1",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());

    // A page with no capture: the reference's 404.
    let (status, _, body) = get(&state, "/api/scan/skills/capture/99").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    assert_eq!(body_json(&body)["detail"], "Capture not available");

    // An unparseable page: the framework's 422 int_parsing on the path param.
    let (status, _, body) = get(&state, "/api/scan/skills/capture/abc").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(&body)["detail"][0]["type"], "int_parsing");
    assert_eq!(
        body_json(&body)["detail"][0]["loc"],
        serde_json::json!(["path", "page"])
    );
}

/// The repair-cost read runs the OCR provider chain and gates on the live
/// `repair_ocr_enabled` flag (the reference's 400 when off). A plain 200
/// (POST, outside the ETag scope) carrying the declared fields in model order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_scan_serves_and_gates_on_the_config_flag() {
    let (state, _dir) = serve_producer_substrate(r#"{"repair_ocr_enabled": true}"#).await;
    let (status, headers, body) =
        send_json(&state, "POST", "/api/tracking/session/abc/repair-scan", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "POST is outside the ETag scope"
    );
    let result = body_json(&body);
    assert_eq!(result["cost_ped"], 2.2);
    assert_eq!(result["raw_text"], "2,20 PED");
    assert_eq!(result["confidence"], 0.97);
    let keys: Vec<&str> = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["cost_ped", "raw_text", "confidence"],
        "success carries the declared fields only, no null error key"
    );

    let (state, _dir) = serve_producer_substrate(r#"{"repair_ocr_enabled": false}"#).await;
    let (status, _, body) =
        send_json(&state, "POST", "/api/tracking/session/abc/repair-scan", "").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&body)["detail"], "Repair OCR is disabled");
}

/// The spacebar-capture toggle route serves its acknowledgement and validates
/// the required `enabled` boolean the framework's way (bool_parsing on an
/// uninterpretable value, missing when absent).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spacebar_capture_toggle_serves_and_validates() {
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;

    // enable: a plain 200 (POST), {ok, enabled} in model order.
    let (status, headers, body) = send_json(
        &state,
        "POST",
        "/api/scan/spacebar-capture?enabled=true",
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    let enabled = body_json(&body);
    assert_eq!(enabled["ok"], true);
    assert_eq!(enabled["enabled"], true);
    let keys: Vec<&str> = enabled
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["ok", "enabled"]);

    // disable.
    let (_, _, body) = send_json(
        &state,
        "POST",
        "/api/scan/spacebar-capture?enabled=false",
        "",
    )
    .await;
    assert_eq!(body_json(&body)["enabled"], false);

    // an uninterpretable value: 422 bool_parsing on the query param.
    let (status, _, body) = send_json(
        &state,
        "POST",
        "/api/scan/spacebar-capture?enabled=maybe",
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(&body)["detail"][0]["type"], "bool_parsing");
    assert_eq!(
        body_json(&body)["detail"][0]["loc"],
        serde_json::json!(["query", "enabled"])
    );

    // absent: the framework's 422 missing.
    let (status, _, body) = send_json(&state, "POST", "/api/scan/spacebar-capture", "").await;
    assert_eq!(status, http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body_json(&body)["detail"][0]["type"], "missing");
}

/// The dashboard snapshot serves both states under the conditional-GET
/// contract, each keeping its own polymorphic shape in the model's
/// declaration order (the snake-case status trio among the camelCase numbers).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracking_snapshot_serves_idle_and_active() {
    // ── idle, trifecta-mode (the default preset exists, nothing bound) ──
    let (state, _dir) = serve_producer_substrate(r#"{"hotbar_hooks_enabled": false}"#).await;
    let (status, headers, body) = get(&state, "/api/tracking/snapshot").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        headers.contains_key(http::header::ETAG),
        "the snapshot GET carries the conditional-GET contract"
    );
    let idle = body_json(&body);
    assert_eq!(idle["status"], "idle");
    assert_eq!(idle["hotbarListenerActive"], false);
    assert_eq!(idle["weaponAttribution"], "trifecta");
    assert_eq!(idle["recentEvents"], serde_json::json!([]));
    let keys: Vec<&str> = idle
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "status",
            "hotbarListenerActive",
            "weaponAttribution",
            "repairOcrEnabled",
            "endOfSessionArmourReminderEnabled",
            "mobEntryMode",
            "currentMob",
            "mobSource",
            "currentTool",
            "trifectaAttribution",
            "recentEvents",
        ]
    );
    // The trifecta summary populates (a default preset exists) with nothing
    // bound, in its own insertion order.
    let summary = &idle["trifectaAttribution"];
    assert_eq!(summary["smallWeapon"], serde_json::Value::Null);
    assert!(summary["presets"].is_array());
    let summary_keys: Vec<&str> = summary
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        summary_keys,
        [
            "activePresetId",
            "presetName",
            "presets",
            "smallWeapon",
            "bigWeapon",
            "healTool",
        ]
    );

    // ── active, hotbar-mode (a started session) ──
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;
    let (status, _, body) = send_json(&state, "POST", "/api/tracking/start", "").await;
    assert_eq!(status, http::StatusCode::OK);
    let session_id = body_json(&body)["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let (status, headers, body) = get(&state, "/api/tracking/snapshot").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains_key(http::header::ETAG));
    let active = body_json(&body);
    assert_eq!(active["status"], "active");
    assert_eq!(active["session_id"], session_id);
    assert_eq!(active["kill_count"], 0);
    assert_eq!(active["hotbarListenerActive"], false);
    assert_eq!(active["weaponAttribution"], "hotbar");
    assert_eq!(active["trifectaAttribution"], serde_json::Value::Null);
    assert_eq!(active["recentEvents"], serde_json::json!([]));
    assert_eq!(active["warnings"], serde_json::json!([]));
    // The full polymorphic shape in the model's declaration order: status,
    // then the shared envelope, then the active-only block ending in warnings.
    let keys: Vec<&str> = active
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "status",
            "hotbarListenerActive",
            "weaponAttribution",
            "repairOcrEnabled",
            "endOfSessionArmourReminderEnabled",
            "mobEntryMode",
            "currentMob",
            "mobSource",
            "currentTool",
            "trifectaAttribution",
            "recentEvents",
            "session_id",
            "started_at",
            "kill_count",
            "elapsed",
            "cost",
            "returns",
            "pes",
            "net",
            "returnRate",
            "damageDealtTotal",
            "weaponDamageDealt",
            "weaponCost",
            "shotsFiredTotal",
            "criticalHitsTotal",
            "maxDamage",
            "globalsCount",
            "hofsCount",
            "latestKillLoot",
            "multiplierLast",
            "multiplierAvg",
            "multiplierMax",
            "multiplierHistory",
            "cumulativeNetHistory",
            "warnings",
        ]
    );
}

/// The cancel / undo / reject verbs respond with their service bodies (not an
/// empty default): cancel from idle returns the resting status, undo from idle
/// the no-active-scan refusal, reject the no-pending refusal. Plain 200s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_skills_cancel_undo_reject_respond() {
    let (state, _dir) = serve_producer_substrate(HOTBAR_BOUND_CONFIG).await;

    let (status, headers, body) = send_json(&state, "POST", "/api/scan/skills/cancel", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(!headers.contains_key(http::header::ETAG));
    assert_eq!(body_json(&body)["phase"], "idle");
    assert_eq!(body_json(&body)["captured_pages"], 0);

    let (status, _, body) = send_json(&state, "POST", "/api/scan/skills/undo", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        body_json(&body)["error"],
        "No active scan: call start first"
    );

    let (status, _, body) = send_json(&state, "POST", "/api/scan/skills/reject", "").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body_json(&body)["error"], "No pending result to reject");
}
