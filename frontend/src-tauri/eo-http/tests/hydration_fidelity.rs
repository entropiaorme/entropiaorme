//! A/B fidelity for the natively-served hydration handlers: every
//! quests/codex hydration GET answered by the native handlers over the
//! SAME database the running backend serves, compared byte-for-byte on
//! status, content-type, cache-control, etag, and body, plus the
//! conditional-GET (304), not-found, and rank-validation legs.
//!
//! The backend owns the database (it creates and migrates it at boot);
//! the test seeds quests and playlists through the backend's own API,
//! seeds the tracking economy directly in SQL, and then reads through
//! both implementations.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test hydration_fidelity
#![cfg(feature = "cross-language")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use chrono::NaiveDateTime;
use eo_http::hydration::HydrationState;
use eo_services::clock::MockClock;
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

async fn backend_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<Value>,
    if_none_match: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority);
    if let Some(tag) = if_none_match {
        builder = builder.header("if-none-match", tag);
    }
    let request = match body {
        Some(payload) => builder
            .header("content-type", "application/json")
            // Mutating requests pass the backend's origin guard with
            // the packaged app's origin.
            .header("origin", "tauri://localhost")
            .body(Body::from(payload.to_string()))
            .unwrap(),
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
    let authority = format!("127.0.0.1:{port}");
    loop {
        if Instant::now() > deadline {
            panic!("backend never became healthy on {authority}");
        }
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

async fn native_parts(
    response: http::Response<Body>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
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

fn header<'h>(headers: &'h http::HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Compare a native response against the backend's, byte for byte on
/// the load-bearing surface.
fn assert_matches(
    label: &str,
    native: &(http::StatusCode, http::HeaderMap, Vec<u8>),
    backend: &(http::StatusCode, http::HeaderMap, Vec<u8>),
) {
    assert_eq!(
        String::from_utf8_lossy(&native.2),
        String::from_utf8_lossy(&backend.2),
        "{label}: body diverges"
    );
    assert_eq!(native.0, backend.0, "{label}: status diverges");
    for name in ["etag", "cache-control", "content-type"] {
        assert_eq!(
            header(&native.1, name),
            header(&backend.1, name),
            "{label}: {name} diverges"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_native_hydration_handlers_match_the_backend() {
    let sidecar = spawn_sidecar();
    wait_healthy(sidecar.port).await;
    let port = sidecar.port;

    // ── Seed quests and playlists through the backend's own API ────
    let quest = |payload: Value| async move {
        let (status, _, body) =
            backend_request(port, "POST", "/api/quests", Some(payload), None).await;
        assert_eq!(status, http::StatusCode::OK, "quest seed failed");
        serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
    };
    let qa = quest(json!({
        "name": "Iron Challenge", "planet": "Foma", "waypoint": "/wp 1,2",
        "cooldown_hours": 24, "reward_ped": 2.5,
        "expected_reward_markup_percent": 150.0, "notes": "bring fap",
        "chain_name": "Cull", "chain_position": 1, "chain_total": 3,
        "category": "hunt", "reward_description": "ammo",
        "mobs": ["Atrox", "Atrax"],
    }))
    .await;
    let qb = quest(json!({
        "name": "Daily Hunt: Atrox", "reward_ped": 5.0, "reward_is_skill": true,
        "cooldown_hours": 1,
    }))
    .await;
    let qc = quest(json!({"name": "Géologist Survey"})).await;

    let playlist = |payload: Value| async move {
        let (status, _, body) =
            backend_request(port, "POST", "/api/quests/playlists", Some(payload), None).await;
        assert_eq!(status, http::StatusCode::OK, "playlist seed failed");
        serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
    };
    let p1 = playlist(json!({
        "name": "Mixed Run",
        "items": [
            {"quest_id": qa, "group_type": "immediate"},
            {"quest_id": qb, "group_type": "immediate"},
            {"quest_id": qc, "group_type": "long_horizon", "description": "later"},
        ],
    }))
    .await;
    let _p2 = playlist(json!({"name": "Solo", "quest_ids": [qa]})).await;

    // ── Seed the tracking economy directly in the shared database ──
    let db_path = sidecar.data_dir.path().join("entropia_orme.db");
    let seed_pool: SqlitePool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open the shared database");
    for statement in [
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
         VALUES ('ab-1', 1000.0, 4600.0, 0, 1.5, 0.25)",
        "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
         damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
         VALUES ('ab-k1', 'ab-1', 'Atrox', 1100.0, 10, 100.0, 5.0, 1, 0.3, 0.5, 12.75)",
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
         critical_hits, cost_per_shot) VALUES ('ab-k1', 'LR-32', 40, 50.0, 0, 0.05)",
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
         VALUES ('ab-1', 1100.0, 'Rifle', 1.0, 0.8)",
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
         VALUES ('Agility', 32.04, 'scan', 100.0)",
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
         VALUES ('Rifle', 48.25, 'scan', 100.0)",
    ] {
        sqlx::query(statement).execute(&seed_pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
         VALUES ('ab-1', ?, 1500.0)",
    )
    .bind(qa)
    .execute(&seed_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
         VALUES ('ab-2', 5000.0, 5030.5, 0, 0.0, 0.0)",
    )
    .execute(&seed_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
         VALUES ('ab-2', ?, 5020.0)",
    )
    .bind(qa)
    .execute(&seed_pool)
    .await
    .unwrap();
    for (session, link_type, quest_id, playlist_id) in [
        ("ab-1", "quest", Some(qa), None::<i64>),
        ("ab-2", "playlist", None, Some(p1)),
    ] {
        sqlx::query(
            "INSERT INTO session_quest_analytics_links \
             (session_id, link_type, quest_id, playlist_id, linked_at) \
             VALUES (?, ?, ?, ?, 9000.0)",
        )
        .bind(session)
        .bind(link_type)
        .bind(quest_id)
        .bind(playlist_id)
        .execute(&seed_pool)
        .await
        .unwrap();
    }
    // Codex progress through the backend's own API.
    let (status, _, _) = backend_request(
        port,
        "POST",
        "/api/codex/calibrate",
        Some(json!({"species_name": "Boar", "rank": 3})),
        None,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "codex calibrate failed");

    // ── The native side over the same database ──────────────────────
    let native = HydrationState::new(
        seed_pool.clone(),
        Arc::new(GameDataStore::new(&repo_root().join("backend/data/snapshot")).unwrap()),
        Arc::new(MockClock::new(
            Some(
                NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            ),
            0.0,
        )),
    );

    // ── The nine 200 legs, byte for byte ───────────────────────────
    let legs: Vec<(&str, http::Response<Body>)> = vec![
        ("/api/quests", native.list_quests(None).await),
        ("/api/quests/mobs", native.list_mob_names(None).await),
        ("/api/quests/analytics", native.quest_analytics(None).await),
        ("/api/quests/playlists", native.list_playlists(None).await),
        (
            "/api/quests/playlists/analytics",
            native.playlist_analytics(None).await,
        ),
        ("/api/codex/species", native.codex_species(None).await),
        (
            "/api/codex/species/Boar/ranks",
            native.codex_species_ranks("Boar", None).await,
        ),
        (
            "/api/codex/recommend?species_name=Boar&rank=4&profession=BLP%20Sniper%20(Hit)&target=profession",
            native
                .codex_recommend("Boar", 4, Some("BLP Sniper (Hit)"), "profession", None)
                .await,
        ),
        (
            "/api/codex/meta/attributes",
            native.codex_meta_attributes(None).await,
        ),
    ];
    let mut etags: Vec<(String, String)> = Vec::new();
    for (path, native_response) in legs {
        let native_parts = native_parts(native_response).await;
        let backend_parts = backend_request(port, "GET", path, None, None).await;
        assert_matches(path, &native_parts, &backend_parts);
        if let Some(tag) = header(&native_parts.1, "etag") {
            etags.push((path.to_string(), tag.to_string()));
        }
    }

    // ── The conditional-GET legs: a matching If-None-Match is a 304
    // with an empty body and the same validator headers ─────────────
    for (path, etag) in etags.iter().take(3) {
        let native_response = match path.as_str() {
            "/api/quests" => native.list_quests(Some(etag)).await,
            "/api/quests/mobs" => native.list_mob_names(Some(etag)).await,
            _ => native.quest_analytics(Some(etag)).await,
        };
        let native_parts = native_parts(native_response).await;
        let backend_parts = backend_request(port, "GET", path, None, Some(etag)).await;
        assert_eq!(
            native_parts.0,
            http::StatusCode::NOT_MODIFIED,
            "{path}: 304"
        );
        assert_matches(
            &format!("{path} (conditional)"),
            &native_parts,
            &backend_parts,
        );
        assert!(native_parts.2.is_empty(), "{path}: 304 body must be empty");
    }

    // ── The not-found and rank-validation legs ──────────────────────
    let native_404 = native_parts(native.codex_species_ranks("Nessie", None).await).await;
    let backend_404 =
        backend_request(port, "GET", "/api/codex/species/Nessie/ranks", None, None).await;
    assert_matches("/api/codex/species/Nessie/ranks", &native_404, &backend_404);

    for rank in [0i64, 26] {
        let native_422 = native_parts(
            native
                .codex_recommend("Boar", rank, None, "profession", None)
                .await,
        )
        .await;
        let backend_422 = backend_request(
            port,
            "GET",
            &format!("/api/codex/recommend?species_name=Boar&rank={rank}"),
            None,
            None,
        )
        .await;
        assert_matches(&format!("recommend rank={rank}"), &native_422, &backend_422);
    }
}
