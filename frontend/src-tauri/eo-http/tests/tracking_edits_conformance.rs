//! Tracking session-EDIT conformance through the PUBLIC PORT: the five
//! post-hoc edits to ENDED sessions (`backend/routers/tracking.py`):
//! rename-mob, restore-mob, the bulk loot-item deactivate / activate
//! pair (`{item_name:path}`), and armour-cost. Driven through BOTH arms
//! from identical seeded state and compared after every step.
//!
//! TOPOLOGY (analytics_writes_conformance's two-arm form): two sidecars,
//! both clocks frozen at `CLOCK` = arm A (comparison) and arm B (native
//! upstream); the native substrate stands over arm B's database with a
//! `MockClock` at the same naive instant. These edits carry no
//! random ids and no clock-derived RESPONSE fields, so the comparison is
//! a direct byte-for-byte (status, contract headers, body).
//!
//! THE DEACTIVATED-AT WRINKLE: the loot flip stamps `deactivated_at =
//! unixepoch('now')`, the SQLite WALL clock (the test clock governs only
//! the Python clock, not SQLite's `now`), so the literal value differs
//! per arm and per run. It is not in the response and not in the
//! snapshot catalogue; the db-state read-back compares only the flip
//! STATE (null vs not-null), which is what the affordance observes.
//!
//! DB-STATE after edits, since the snapshot catalogue omits the
//! edit-touched columns (`kills.original_mob_name`, `kill_loot_items.
//! deactivated_at`, `tracking_sessions.armour_cost`, and
//! `session_summaries` entirely): a direct read-back over exactly those,
//! plus the catalogue snapshot for the columns it does cover
//! (`kills.mob_name`, `loot_total_ped`).
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test tracking_edits_conformance
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
use serde_json::{json, Value};
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
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = match body {
        Some(payload) => builder
            .body(Body::from(payload.as_bytes().to_vec()))
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
    /// Drive one request through both arms; assert status + contract
    /// headers match and the response bodies are byte-identical (no
    /// per-arm random ids or clock fields in these responses). Returns
    /// the native arm's parsed body for follow-up assertions.
    async fn compare_response(&self, method: &str, path: &str, body: Option<&str>) -> Value {
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
        assert_eq!(
            native_body,
            cmp_body,
            "response body diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        serde_json::from_slice(&native_body).unwrap_or(Value::Null)
    }

    /// Compare the catalogued database state (covers `kills.mob_name` and
    /// `loot_total_ped`, `tracking_sessions` core columns), one fresh
    /// normaliser per arm.
    async fn compare_catalogue(&self, step: &str) {
        let native = snapshot_of(&self.native_db).await;
        let comparison = snapshot_of(&self.comparison_db).await;
        assert_eq!(native, comparison, "catalogue state diverged after {step}");
    }

    /// Compare the edit-touched columns the catalogue omits:
    /// `kills.original_mob_name`, `kill_loot_items.deactivated_at` (as a
    /// flip-state boolean), `tracking_sessions.armour_cost`, and the
    /// `session_summaries` presence set.
    async fn compare_edit_columns(&self, step: &str) {
        let native = edit_columns_readback(&self.native_db).await;
        let comparison = edit_columns_readback(&self.comparison_db).await;
        assert_eq!(
            native, comparison,
            "edit-column state diverged after {step}"
        );
    }
}

async fn snapshot_of(db_path: &Path) -> String {
    let db = eo_services::db::Db::open(db_path)
        .await
        .expect("open db for snapshot");
    let rows = db.snapshot_rows().await.expect("snapshot rows");
    let mut normalizer = Normalizer::new();
    serialize(&capture(&rows, &mut normalizer))
}

/// A read-back over exactly the columns the snapshot catalogue does not
/// carry but these edits mutate. `deactivated_at` collapses to a flip
/// boolean (the literal `unixepoch('now')` is the wall clock, differing
/// per arm). Ordered by stable content keys so the comparison is
/// deterministic.
async fn edit_columns_readback(db_path: &Path) -> Value {
    let pool = open_pool(db_path).await;
    let kills = sqlx::query("SELECT id, mob_name, original_mob_name FROM kills ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("read kills")
        .into_iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>(0),
                "mobName": row.get::<Option<String>, _>(1),
                "originalMobName": row.get::<Option<String>, _>(2),
            })
        })
        .collect::<Vec<_>>();
    let loot = sqlx::query(
        "SELECT kli.kill_id, kli.item_name, kli.value_ped, kli.deactivated_at \
         FROM kill_loot_items kli ORDER BY kli.kill_id, kli.item_name",
    )
    .fetch_all(&pool)
    .await
    .expect("read loot")
    .into_iter()
    .map(|row| {
        json!({
            "killId": row.get::<String, _>(0),
            "itemName": row.get::<String, _>(1),
            "valuePed": row.get::<f64, _>(2),
            "deactivated": row.try_get::<Option<f64>, _>(3).ok().flatten().is_some(),
        })
    })
    .collect::<Vec<_>>();
    let sessions =
        sqlx::query("SELECT id, COALESCE(armour_cost, 0.0) FROM tracking_sessions ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("read sessions")
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>(0),
                    "armourCost": row.get::<f64, _>(1),
                })
            })
            .collect::<Vec<_>>();
    let summaries = sqlx::query("SELECT session_id FROM session_summaries ORDER BY session_id")
        .fetch_all(&pool)
        .await
        .expect("read summaries")
        .into_iter()
        .map(|row| Value::String(row.get::<String, _>(0)))
        .collect::<Vec<_>>();
    json!({
        "kills": kills,
        "loot": loot,
        "sessions": sessions,
        "summaries": summaries,
    })
}

// ── Identical state seeding into BOTH arms' databases ──

/// Seed the ended-session fixture (and an active session for the 409
/// leg) into one database. Reset-then-insert so each macro step starts
/// from a known state on BOTH arms.
async fn seed_db(db_path: &Path) {
    let pool = open_pool(db_path).await;
    for stmt in [
        "DELETE FROM kill_loot_items",
        "DELETE FROM kills",
        "DELETE FROM session_summaries",
        "DELETE FROM tracking_sessions",
    ] {
        sqlx::query(stmt).execute(&pool).await.expect("clear table");
    }
    sqlx::query(
        "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
         dangling_cost,mob_tracking_mode,updated_at) VALUES('ended',1000.0,4600.0,0,5.0,0,0,'mob',4600.0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
         dangling_cost,mob_tracking_mode,updated_at) VALUES('act',1000.0,NULL,1,0,0,0,'mob',1000.0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (id, mob, ts, loot, orig) in [
        ("k1", "Atrox", 1001.0, 10.0, None),
        ("k2", "Atrox", 1002.0, 20.0, Some("Daikiba")),
        ("k3", "Foul", 1003.0, 5.0, None),
    ] {
        sqlx::query(
            "INSERT INTO kills(id,session_id,mob_name,mob_species,mob_maturity,timestamp,\
             shots_fired,damage_dealt,damage_taken,critical_hits,cost_ped,enhancer_cost,\
             loot_total_ped,is_global,is_hof,original_mob_name) \
             VALUES(?,?,?,'','',?,0,0,0,0,0,0,?,0,0,?)",
        )
        .bind(id)
        .bind("ended")
        .bind(mob)
        .bind(ts)
        .bind(loot)
        .bind(orig)
        .execute(&pool)
        .await
        .unwrap();
    }
    for (kid, item, qty, val) in [
        ("k1", "Animal Hide", 2_i64, 3.0),
        ("k2", "Animal Hide", 1, 1.5),
        ("k3", "Metal/Residue", 1, 2.25),
    ] {
        sqlx::query(
            "INSERT INTO kill_loot_items(kill_id,item_name,quantity,value_ped,\
             is_enhancer_shrapnel,deactivated_at) VALUES(?,?,?,?,0,NULL)",
        )
        .bind(kid)
        .bind(item)
        .bind(qty)
        .bind(val)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO session_summaries(session_id,summary_version,started_at,ended_at,\
         duration_hours,kills,loot_tt,weapon_cost,enhancer_cost,armour_cost,heal_cost,\
         dangling_cost,cycled_ped,regular_skill_ped_json,attribute_levels_json,\
         regular_skill_tt,attribute_levels_total) \
         VALUES('ended',1,1000,4600,1.0,3,36.75,0,0,5,0,0,0,'{}','{}',0,0)",
    )
    .execute(&pool)
    .await
    .unwrap();
}

async fn seed_both(arms: &Arms) {
    seed_db(&arms.native_db).await;
    seed_db(&arms.comparison_db).await;
}

async fn boot() -> (Sidecar, Sidecar, Arms) {
    let upstream = spawn_sidecar();
    let comparison = spawn_sidecar();
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let native_db = upstream.data_dir.path().join("entropia_orme.db");
    let pool = open_pool(&native_db).await;
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

    let comparison_db = comparison.data_dir.path().join("entropia_orme.db");
    let arms = Arms {
        substrate_port,
        comparison_port: comparison.port,
        native_db,
        comparison_db,
    };
    (upstream, comparison, arms)
}

const SESSION: &str = "/api/tracking/session";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tracking_edit_surface_conforms_through_the_public_port() {
    let (_upstream, _comparison, arms) = boot().await;

    // ── rename-mob ──
    // 404 (missing session), 409 (active), 409 (no-op), 400 (blank),
    // 409 (no match), 422 (missing field): all response-only.
    seed_both(&arms).await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/nope/rename-mob"),
        Some(r#"{"fromMobName": "Atrox", "toMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/act/rename-mob"),
        Some(r#"{"fromMobName": "Atrox", "toMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/rename-mob"),
        Some(r#"{"fromMobName": "Atrox", "toMobName": "Atrox"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/rename-mob"),
        Some(r#"{"fromMobName": "  ", "toMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/rename-mob"),
        Some(r#"{"fromMobName": "Zzz", "toMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/rename-mob"),
        Some(r#"{"fromMobName": "Atrox"}"#),
    )
    .await;
    // The failed legs above must have left state untouched.
    arms.compare_catalogue("rename error legs").await;
    arms.compare_edit_columns("rename error legs").await;

    // rename success: Atrox -> Argo (COALESCE preserves the first
    // original; the merged-original case sets up the ambiguous restore).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/rename-mob"),
        Some(r#"{"fromMobName": "Atrox", "toMobName": "Argo"}"#),
    )
    .await;
    arms.compare_catalogue("rename Atrox->Argo").await;
    arms.compare_edit_columns("rename Atrox->Argo").await;

    // restore-mob ambiguous (Argo carries two distinct originals) + the
    // no-restorable leg, response-only.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/restore-mob"),
        Some(r#"{"currentMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/restore-mob"),
        Some(r#"{"currentMobName": "Foul"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/nope/restore-mob"),
        Some(r#"{"currentMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/act/restore-mob"),
        Some(r#"{"currentMobName": "Argo"}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/restore-mob"),
        Some(r#"{"currentMobName": ""}"#),
    )
    .await;
    arms.compare_edit_columns("restore error legs").await;

    // ── restore-mob clean success (re-seeded with a single shared original) ──
    seed_both(&arms).await;
    for db in [&arms.native_db, &arms.comparison_db] {
        let pool = open_pool(db).await;
        sqlx::query(
            "UPDATE kills SET mob_name='Argo', original_mob_name='Wolf' WHERE mob_name='Atrox'",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/restore-mob"),
        Some(r#"{"currentMobName": "Argo"}"#),
    )
    .await;
    arms.compare_catalogue("restore clean").await;
    arms.compare_edit_columns("restore clean").await;

    // ── loot deactivate / activate ──
    seed_both(&arms).await;
    // deactivate Animal Hide (two rows; per-kill loot_total recompute).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Animal%20Hide/deactivate"),
        None,
    )
    .await;
    arms.compare_catalogue("loot deactivate Animal Hide").await;
    arms.compare_edit_columns("loot deactivate Animal Hide")
        .await;
    // already-deactivated -> 409.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Animal%20Hide/deactivate"),
        None,
    )
    .await;
    // activate back.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Animal%20Hide/activate"),
        None,
    )
    .await;
    arms.compare_catalogue("loot activate Animal Hide").await;
    arms.compare_edit_columns("loot activate Animal Hide").await;
    // 404 missing item, 404 missing session, 409 active session.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Nonexist/deactivate"),
        None,
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/nope/loot-item/Animal%20Hide/deactivate"),
        None,
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/act/loot-item/Animal%20Hide/deactivate"),
        None,
    )
    .await;

    // ── the {item_name:path} slash legs ──
    // RAW slash: `Metal/Residue` reaches the handler with the slash
    // intact (FastAPI's :path converter; the native adapter keeps the
    // decoded slash rather than 404ing as a single segment would).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Metal/Residue/deactivate"),
        None,
    )
    .await;
    arms.compare_catalogue("loot deactivate slash item (raw)")
        .await;
    arms.compare_edit_columns("loot deactivate slash item (raw)")
        .await;
    // PERCENT-ENCODED slash: `Metal%2FResidue` decodes to the same item,
    // already deactivated -> 409 (proves both encodings land identically).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Metal%2FResidue/deactivate"),
        None,
    )
    .await;
    // re-activate the slash item via the encoded form.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item/Metal%2FResidue/activate"),
        None,
    )
    .await;
    arms.compare_catalogue("loot activate slash item").await;
    arms.compare_edit_columns("loot activate slash item").await;
    // empty item name (`//`) -> 400 blank.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/loot-item//deactivate"),
        None,
    )
    .await;

    // ── armour-cost ──
    seed_both(&arms).await;
    // add to an ended session (accumulates; echoes the submitted value).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/armour-cost"),
        Some(r#"{"cost": 2.5}"#),
    )
    .await;
    arms.compare_edit_columns("armour-cost add").await;
    // integer coerces to float; banker's rounding on the echo.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/armour-cost"),
        Some(r#"{"cost": 3}"#),
    )
    .await;
    arms.compare_response(
        "POST",
        &format!("{SESSION}/ended/armour-cost"),
        Some(r#"{"cost": 2.675}"#),
    )
    .await;
    // NO active-session guard: succeeds on the active session.
    arms.compare_response(
        "POST",
        &format!("{SESSION}/act/armour-cost"),
        Some(r#"{"cost": 1.0}"#),
    )
    .await;
    arms.compare_edit_columns("armour-cost active + rounding")
        .await;
    // 404 (missing session) and 422 (missing cost).
    arms.compare_response(
        "POST",
        &format!("{SESSION}/nope/armour-cost"),
        Some(r#"{"cost": 1.0}"#),
    )
    .await;
    arms.compare_response("POST", &format!("{SESSION}/ended/armour-cost"), Some("{}"))
        .await;
}
