//! Tracking session-read conformance through the PUBLIC PORT.
//!
//! READ topology: the substrate's native arm answers over its own upstream
//! backend's database (shared data dir), and every byte must match the
//! Python arm reading the same stored state. The `/api/tracking` surface is
//! ETag-scoped, so each 200 carries a strong ETag + `Cache-Control:
//! no-cache`, both checked by `contract_axes`, and the conditional-GET 304
//! leg is exercised explicitly.
//!
//! The three reads under test:
//!   GET /api/tracking/sessions            -> list[TrackingSession]
//!   GET /api/tracking/session/{id}        -> SessionDetail (404 if absent)
//!   GET /api/tracking/tag-suggestions     -> list[str]
//!
//! Only an ACTIVE session's duration reads `clock.now()`; an ended session's
//! is the stored span. The seed uses ended sessions, but both arms still run
//! a clock frozen at the same instant (the sidecar via
//! `ENTROPIA_TEST_CLOCK_START`, the native arm via a matching `MockClock`) so
//! the topology stays robust to any clock-dependent field and matches the
//! analytics battery's precedent.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test tracking_reads_conformance
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

async fn request(
    port: u16,
    path: &str,
    if_none_match: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost");
    if let Some(value) = if_none_match {
        builder = builder.header("if-none-match", value);
    }
    let request = builder.body(Body::empty()).unwrap();
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

fn id_a() -> &'static str {
    "11111111-1111-4111-8111-111111111111"
}
fn id_b() -> &'static str {
    "22222222-2222-4222-8222-222222222222"
}
fn id_c() -> &'static str {
    "33333333-3333-4333-8333-333333333333"
}

/// Seed two representative ended sessions plus the full child-row spread the
/// session detail reads: tool stats, active + deactivated loot (a partial
/// shrapnel-excluded item), per-mob (renamed) breakdown, ordered notable
/// events (global + hof), skill gains (a real skill + an excluded attribute),
/// and a calibration for a non-zero level. Session A is dominated by a
/// species-bearing mob (excluded from tag-suggestions) plus a species-less
/// tag mob (the tag-suggestions match).
async fn seed_tracking(pool: &SqlitePool) {
    let recent = "2026-05-20T10:00:00";
    let prior = "2026-04-25T10:00:00";
    for (id, start, armour, heal, dangling) in [
        (id_a(), recent, 1.0, 2.0, 0.5),
        (id_b(), prior, 0.5, 1.0, 0.0),
    ] {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(ts(start))
        .bind(ts(start) + 3661.0)
        .bind(0_i64)
        .bind(armour)
        .bind(heal)
        .bind(dangling)
        .bind("mob")
        .bind(ts(start) + 3661.0)
        .execute(pool)
        .await
        .expect("seed session");
    }
    // Session A: 5 Atrox kills (first global, first renamed) + 2 tag kills.
    for i in 0..5 {
        let kid = format!("k-a-{i}");
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&kid).bind(id_a()).bind("Atrox").bind("Atrox").bind("Young")
        .bind(ts(recent) + i as f64).bind(50_i64).bind(500.0).bind(10.0).bind(3_i64)
        .bind(0.55).bind(0.1).bind(10.0)
        .bind(if i == 0 { 1_i64 } else { 0 }).bind(0_i64)
        .bind(if i == 0 { Some("Atroxx") } else { None })
        .execute(pool).await.expect("seed kill");
        sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,damage_dealt,critical_hits,cost_per_shot) VALUES(?,?,?,?,?,?)")
            .bind(&kid).bind("Opalo").bind(50_i64).bind(500.0).bind(3_i64).bind(0.011)
            .execute(pool).await.expect("seed tool");
        sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
            .bind(&kid).bind("Animal Hide").bind(2_i64).bind(3.0).bind(0_i64).bind(Option::<f64>::None)
            .execute(pool).await.expect("seed loot");
        // A non-shrapnel item that is deactivated on the last kill: it lands
        // in deactivatedLootBreakdown, exercising the partial-state split.
        sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
            .bind(&kid).bind("Animal Oil").bind(1_i64).bind(2.0).bind(0_i64)
            .bind(if i == 4 { Some(ts(recent) + 100.0) } else { None })
            .execute(pool).await.expect("seed loot2");
        // Enhancer shrapnel: excluded from both loot aggregates.
        sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,?,?)")
            .bind(&kid).bind("Shrapnel").bind(100_i64).bind(1.0).bind(1_i64).bind(Option::<f64>::None)
            .execute(pool).await.expect("seed shrapnel");
    }
    for i in 0..2 {
        let kid = format!("k-tag-{i}");
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&kid).bind(id_a()).bind("Atrocious Tag").bind("").bind("")
        .bind(ts(recent) + 100.0 + i as f64).bind(10_i64).bind(50.0).bind(0.0).bind(0_i64)
        .bind(0.1).bind(0.0).bind(1.0).bind(0_i64).bind(0_i64).bind(Option::<String>::None)
        .execute(pool).await.expect("seed tag kill");
    }
    // Session B: 3 kills of a bare-name mob.
    for i in 0..3 {
        let kid = format!("k-b-{i}");
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,NULL,NULL,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&kid).bind(id_b()).bind("Thing")
        .bind(ts(prior) + i as f64).bind(30_i64).bind(300.0).bind(5.0).bind(1_i64)
        .bind(0.3).bind(0.0).bind(5.0).bind(0_i64).bind(0_i64).bind(Option::<String>::None)
        .execute(pool).await.expect("seed kill b");
        sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,damage_dealt,critical_hits,cost_per_shot) VALUES(?,?,?,?,?,?)")
            .bind(&kid).bind("Sollomate").bind(30_i64).bind(300.0).bind(1_i64).bind(0.01)
            .execute(pool).await.expect("seed tool b");
    }
    // Notable events on A: a global_kill then a hof_item (timestamp order).
    sqlx::query("INSERT INTO notable_events(session_id,kill_id,event_type,mob_or_item,value_ped,timestamp) VALUES(?,?,?,?,?,?)")
        .bind(id_a()).bind("k-a-0").bind("global_kill").bind("Atrox").bind(55.0).bind(ts(recent) + 1.0)
        .execute(pool).await.expect("seed notable 1");
    sqlx::query("INSERT INTO notable_events(session_id,kill_id,event_type,mob_or_item,value_ped,timestamp) VALUES(?,?,?,?,?,?)")
        .bind(id_a()).bind("k-a-1").bind("hof_item").bind("Rare Sword").bind(1500.0).bind(ts(recent) + 2.0)
        .execute(pool).await.expect("seed notable 2");
    // Skill gains on A: a real skill + an attribute (Agility, list-excluded).
    sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
        .bind(id_a()).bind(ts(recent) + 1800.0).bind("Laser Weaponry Technology").bind(3.0).bind(3.0).bind(ts(recent) + 1800.0)
        .execute(pool).await.expect("seed skill");
    sqlx::query("INSERT INTO skill_gains(session_id,timestamp,skill_name,amount,ped_value,created_at) VALUES(?,?,?,?,?,?)")
        .bind(id_a()).bind(ts(recent) + 1801.0).bind("Agility").bind(1.0).bind(1.0).bind(ts(recent) + 1801.0)
        .execute(pool).await.expect("seed attr skill");
    // Two calibrations for the same skill: the latest (MAX id) wins.
    sqlx::query(
        "INSERT INTO skill_calibrations(skill_name,level,source,scanned_at) VALUES(?,?,?,?)",
    )
    .bind("Laser Weaponry Technology")
    .bind(40.0)
    .bind("manual")
    .bind(ts(recent) + 900.0)
    .execute(pool)
    .await
    .expect("seed cal 1");
    sqlx::query(
        "INSERT INTO skill_calibrations(skill_name,level,source,scanned_at) VALUES(?,?,?,?)",
    )
    .bind("Laser Weaponry Technology")
    .bind(42.5)
    .bind("manual")
    .bind(ts(recent) + 1000.0)
    .execute(pool)
    .await
    .expect("seed cal 2");

    // Session C: deliberate TIES on every GROUP-BY/ORDER-BY key the reference
    // leaves without an explicit tie-break (two mobs at equal kills, two tools
    // at equal shots, two loot items at equal value). The reference relies on
    // SQLite's group order here; this leg proves the native arm's bundled
    // SQLite and the sidecar's sqlite3 resolve those ties identically, so the
    // verbatim-SQL port stays byte-faithful without inventing a tie-break.
    let cstart = "2026-03-10T10:00:00";
    sqlx::query(
        "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,dangling_cost,mob_tracking_mode,updated_at) \
         VALUES(?,?,?,0,0.0,0.0,0.0,'mob',?)",
    )
    .bind(id_c()).bind(ts(cstart)).bind(ts(cstart) + 3600.0).bind(ts(cstart) + 3600.0)
    .execute(pool).await.expect("seed session c");
    for (i, mob) in ["Mob Alpha", "Mob Beta"].iter().enumerate() {
        for j in 0..3 {
            let kid = format!("k-c-{i}-{j}");
            sqlx::query(
                "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,loot_total_ped,is_global,is_hof,original_mob_name) \
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,0,0,NULL)",
            )
            .bind(&kid).bind(id_c()).bind(*mob).bind("Spec").bind("Old")
            .bind(ts(cstart) + (i * 3 + j) as f64).bind(20_i64).bind(200.0).bind(0.0).bind(0_i64)
            .bind(0.2).bind(0.0).bind(4.0)
            .execute(pool).await.expect("seed kill c");
            // Tools alternate so both reach an equal total of 60 shots.
            let tool = if j % 2 == 0 { "Tool One" } else { "Tool Two" };
            sqlx::query("INSERT INTO kill_tool_stats(kill_id,tool_name,shots_fired,damage_dealt,critical_hits,cost_per_shot) VALUES(?,?,?,?,?,?)")
                .bind(&kid).bind(tool).bind(10_i64).bind(100.0).bind(0_i64).bind(0.01)
                .execute(pool).await.expect("seed tool c");
            // Two loot items at equal aggregate value.
            for item in ["Loot X", "Loot Y"] {
                sqlx::query("INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,0,NULL)")
                    .bind(&kid).bind(item).bind(1_i64).bind(2.0)
                    .execute(pool).await.expect("seed loot c");
            }
        }
    }
}

async fn boot(seeded: bool) -> (Sidecar, u16) {
    let upstream = spawn_sidecar();
    wait_healthy(upstream.port).await;

    let db_path = upstream.data_dir.path().join("entropia_orme.db");
    let pool = open_pool(db_path).await;
    if seeded {
        seed_tracking(&pool).await;
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

/// Compare a plain GET: status, the three contract-header axes, body bytes.
async fn compare(substrate: u16, upstream: u16, path: &str) {
    let (native_status, native_headers, native_body) = request(substrate, path, None).await;
    let (py_status, py_headers, py_body) = request(upstream, path, None).await;
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

/// The conditional-GET 304 leg: fetch once to learn the ETag, then re-fetch
/// with `If-None-Match`. Both arms must answer 304 with no body and the same
/// ETag + Cache-Control, and the two arms' ETags for the 200 must be equal
/// (the bodies are byte-identical, so the SHA-256 ETag is too).
async fn compare_conditional(substrate: u16, upstream: u16, path: &str) {
    let (_, native_headers, _) = request(substrate, path, None).await;
    let (_, py_headers, _) = request(upstream, path, None).await;
    let native_etag = native_headers
        .get(http::header::ETAG)
        .expect("native etag")
        .to_str()
        .unwrap()
        .to_string();
    let py_etag = py_headers
        .get(http::header::ETAG)
        .expect("python etag")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(native_etag, py_etag, "etag diverged on {path}");

    let (native_status, native_headers, native_body) =
        request(substrate, path, Some(&native_etag)).await;
    let (py_status, py_headers, py_body) = request(upstream, path, Some(&py_etag)).await;
    assert_eq!(native_status, http::StatusCode::NOT_MODIFIED, "native 304");
    assert_eq!(py_status, http::StatusCode::NOT_MODIFIED, "python 304");
    assert!(native_body.is_empty(), "native 304 has no body");
    assert!(py_body.is_empty(), "python 304 has no body");
    assert_eq!(
        contract_axes(&native_headers),
        contract_axes(&py_headers),
        "304 contract headers diverged on {path}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tracking_read_surface_conforms_over_seeded_state() {
    let (upstream, substrate) = boot(true).await;

    compare(substrate, upstream.port, "/api/tracking/sessions").await;
    compare(
        substrate,
        upstream.port,
        &format!("/api/tracking/session/{}", id_a()),
    )
    .await;
    compare(
        substrate,
        upstream.port,
        &format!("/api/tracking/session/{}", id_b()),
    )
    .await;
    // Session C: the tie-resolution leg. Its detail (mobBreakdown, toolStats,
    // lootBreakdown) and its /sessions row (primaryMobs/primaryWeapons) all
    // turn on group keys the reference does not tie-break, so a byte-identical
    // result here proves the two SQLite engines order the ties the same way.
    compare(
        substrate,
        upstream.port,
        &format!("/api/tracking/session/{}", id_c()),
    )
    .await;
    // A nonexistent session: the byte-identical 404 envelope.
    compare(
        substrate,
        upstream.port,
        "/api/tracking/session/does-not-exist",
    )
    .await;
    // tag-suggestions: a species-bearing prefix (Atrox is filtered out, the
    // bare tag matches), a lowercase contains match, the empty-q [], a
    // clamped limit, and a no-match query.
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=At",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=atro",
    )
    .await;
    compare(substrate, upstream.port, "/api/tracking/tag-suggestions?q=").await;
    compare(substrate, upstream.port, "/api/tracking/tag-suggestions").await;
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=a&limit=1",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=a&limit=99",
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=zzz",
    )
    .await;
    // An unparseable limit: the byte-identical 422 int_parsing envelope.
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=a&limit=abc",
    )
    .await;

    // The conditional-GET 304 leg on each ETag-scoped read.
    compare_conditional(substrate, upstream.port, "/api/tracking/sessions").await;
    compare_conditional(
        substrate,
        upstream.port,
        &format!("/api/tracking/session/{}", id_a()),
    )
    .await;
    compare_conditional(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=At",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tracking_read_surface_conforms_over_an_empty_database() {
    // The empty database is the negative control: an empty session list, the
    // 404 for any id, and the empty-q [] must all match byte-for-byte.
    let (upstream, substrate) = boot(false).await;
    compare(substrate, upstream.port, "/api/tracking/sessions").await;
    compare(
        substrate,
        upstream.port,
        &format!("/api/tracking/session/{}", id_a()),
    )
    .await;
    compare(
        substrate,
        upstream.port,
        "/api/tracking/tag-suggestions?q=anything",
    )
    .await;
    compare_conditional(substrate, upstream.port, "/api/tracking/sessions").await;
}
