//! Analytics overview/activity conformance through the PUBLIC PORT.
//!
//! READ topology: the substrate's native arm answers over its own upstream
//! backend's database (shared data dir), and every byte must match the
//! Python arm reading the same stored state. The analytics surface is
//! outside the ETag middleware's prefixes, so all responses are plain 200s
//! (no ETag / Cache-Control), which `contract_axes` checks alongside the
//! body.
//!
//! The overview reads the injected clock (the period filter and the
//! 30d-vs-prior-30d trend), so BOTH arms run a clock frozen at the same
//! instant: the sidecar via `ENTROPIA_TEST_CLOCK_START`, the native arm via
//! a `MockClock` at the matching naive instant. `naive_to_epoch` resolves a
//! naive instant through the local zone exactly as Python's
//! `datetime.timestamp()` does, so both arms compute an identical `now`, and
//! the seeded rows sit deterministically inside / outside the windows.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test analytics_conformance
#![cfg(feature = "cross-language")]

use std::path::PathBuf;
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
use eo_services::tracker::naive_to_epoch;
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

/// A backend sidecar with its clock frozen at `CLOCK` (the deterministic
/// seam the overview's period/trend reads depend on).
fn spawn_sidecar() -> Sidecar {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
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

fn ts(s: &str) -> f64 {
    naive_to_epoch(NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").expect("parse instant"))
}

/// Seed a representative cross-session economy: a recent session (inside the
/// 30d window) dominated by a real mob, a prior session (30-60d window)
/// dominated by a bare tag, a recent zero-cost zero-kill session that the
/// activity filter must drop, plus skill / codex / quest / ledger rows.
async fn seed_analytics(pool: &SqlitePool) {
    let recent = "2026-05-20T10:00:00";
    let prior = "2026-04-25T10:00:00";
    for (id, start, armour, heal, dangling) in [
        (id_a(), recent, 1.0, 2.0, 0.5),
        (id_b(), prior, 0.5, 1.0, 0.0),
        ("sess-z", recent, 0.0, 0.0, 0.0),
    ] {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(ts(start))
        .bind(ts(start) + 3600.0)
        .bind(0_i64)
        .bind(armour)
        .bind(heal)
        .bind(dangling)
        .bind("all")
        .bind(ts(start) + 3600.0)
        .execute(pool)
        .await
        .expect("seed session");
    }
    for i in 0..5 {
        let kid = format!("k-a-{i}");
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&kid).bind(id_a()).bind("Atrox").bind("Atrox").bind("Young")
        .bind(ts(recent) + i as f64).bind(50_i64).bind(0.55).bind(0.1).bind(10.0).bind(0_i64).bind(0_i64)
        .execute(pool).await.expect("seed kill");
        sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?,?,?,?)")
            .bind(&kid).bind("Opalo").bind(50_i64).bind(0.011)
            .execute(pool).await.expect("seed tool");
    }
    for i in 0..3 {
        let kid = format!("k-b-{i}");
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof) \
             VALUES(?,?,?,NULL,NULL,?,?,?,?,?,?,?)",
        )
        .bind(&kid).bind(id_b()).bind("Thing")
        .bind(ts(prior) + i as f64).bind(30_i64).bind(0.3).bind(0.0).bind(5.0).bind(0_i64).bind(0_i64)
        .execute(pool).await.expect("seed kill");
        sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,cost_per_shot) VALUES(?,?,?,?)")
            .bind(&kid).bind("Opalo").bind(30_i64).bind(0.01)
            .execute(pool).await.expect("seed tool");
    }
    sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
        .bind(id_a()).bind(ts(recent) + 1800.0).bind("Laser Weaponry Technology").bind(3.0).bind(3.0).bind(ts(recent) + 1800.0)
        .execute(pool).await.expect("seed skill");
    sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
        .bind(id_b()).bind(ts(prior) + 1800.0).bind("Anatomy").bind(1.0).bind(1.0).bind(ts(prior) + 1800.0)
        .execute(pool).await.expect("seed skill");
    sqlx::query("INSERT INTO codex_claims(species_name,rank,skill_name,ped_value,claimed_at,kind) VALUES(?,?,?,?,?,?)")
        .bind("Atrox").bind(3_i64).bind("Laser Weaponry Technology").bind(7.0).bind(ts(recent) + 2700.0).bind("skill")
        .execute(pool).await.expect("seed codex");
    sqlx::query(
        "INSERT INTO quest_claims(quest_id,quest_name,ped_value,claimed_at) VALUES(?,?,?,?)",
    )
    .bind(1_i64)
    .bind("Iron Challenge: Atrox")
    .bind(4.0)
    .bind(ts(recent) + 3000.0)
    .execute(pool)
    .await
    .expect("seed quest");
    sqlx::query(
        "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?,?,?,?,?,?)",
    )
    .bind("led-1")
    .bind("2026-05-20")
    .bind("markup")
    .bind("Sold hides")
    .bind(12.5)
    .bind("loot_sale")
    .execute(pool)
    .await
    .expect("seed ledger");
    sqlx::query(
        "INSERT INTO ledger_entries(id,date,type,description,amount,tag) VALUES(?,?,?,?,?,?)",
    )
    .bind("led-2")
    .bind("2026-04-25")
    .bind("expense")
    .bind("Deposit")
    .bind(8.0)
    .bind("deposit")
    .execute(pool)
    .await
    .expect("seed ledger");
}

fn id_a() -> &'static str {
    "11111111-1111-4111-8111-111111111111"
}
fn id_b() -> &'static str {
    "22222222-2222-4222-8222-222222222222"
}

/// Stand the native substrate over the upstream's database, both clocks
/// frozen at `CLOCK`. Returns the live upstream (kept alive by the caller)
/// and the substrate port.
async fn boot(seeded: bool) -> (Sidecar, u16) {
    let upstream = spawn_sidecar();
    wait_healthy(upstream.port).await;

    let db_path = upstream.data_dir.path().join("entropia_orme.db");
    let pool = open_pool(db_path).await;
    if seeded {
        seed_analytics(&pool).await;
    }

    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let naive = NaiveDateTime::parse_from_str(CLOCK, "%Y-%m-%dT%H:%M:%S").expect("clock parses");
    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        Arc::new(MockClock::new(Some(naive), 0.0)),
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
    (upstream, substrate_port)
}

async fn compare(substrate: u16, upstream: u16, path: &str) {
    let (native_status, native_headers, native_body) = request(substrate, path).await;
    let (py_status, py_headers, py_body) = request(upstream, path).await;
    assert_eq!(
        native_status,
        py_status,
        "status diverged on {path}\n  native: {}\n  python: {}",
        String::from_utf8_lossy(&native_body),
        String::from_utf8_lossy(&py_body),
    );
    assert_eq!(
        contract_axes(&native_headers),
        contract_axes(&py_headers),
        "contract headers diverged on {path}"
    );
    assert_eq!(
        native_body,
        py_body,
        "body diverged on {path}\n  native: {}\n  python: {}",
        String::from_utf8_lossy(&native_body),
        String::from_utf8_lossy(&py_body),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_analytics_read_surface_conforms_over_seeded_state() {
    let (upstream, substrate) = boot(true).await;
    // Overview across every period: all-time, then the three named windows
    // (each exercising the period filter and the trend banding).
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=all",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=30d",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=90d",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=1y",
    )
    .await;
    // An unrecognised period falls through to all-time, byte-identically.
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=bogus",
    )
    .await;
    // No period at all defaults to "all".
    compare(substrate, upstream.port, "/api/analytics/overview").await;
    // Activity: mob / tag / weapon dominance and the session filters.
    compare(substrate, upstream.port, "/api/analytics/activity").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_analytics_read_surface_conforms_over_an_empty_database() {
    // The empty database is the engine-typed-zero negative control: the
    // cycledBreakdown integers and the float-coerced aggregates must match
    // the Python arm exactly.
    let (upstream, substrate) = boot(false).await;
    compare(
        substrate,
        upstream.port,
        "/api/analytics/overview?period=all",
    )
    .await;
    compare(substrate, upstream.port, "/api/analytics/activity").await;
}
