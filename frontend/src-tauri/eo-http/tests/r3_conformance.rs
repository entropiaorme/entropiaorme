//! Settings/character/equipment conformance through the PUBLIC PORT.
//!
//! Two comparison topologies, each matched to what can diverge:
//! - READS (settings, character, equipment lookups) compare the
//!   substrate's native arm against ITS OWN upstream backend over the
//!   same data directory and database: every byte must match,
//!   including the settings `dbPath` and the calibration timestamp,
//!   because both arms render the same stored state.
//! - WRITES (the equipment library) drive BOTH arms from identical
//!   starting states (the substrate's native arm over its backend's
//!   database, an independent comparison backend over its own) and
//!   compare the response AND the database state after every step,
//!   holding the stored `properties_json` bytes to the backend's bare
//!   `json.dumps` form.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test r3_conformance
#![cfg(feature = "cross-language")]

use std::path::PathBuf;
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
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

// Catalogue ids used by the probes (the bundled snapshot's stable ids).
const WEAPON: &str = "a7ac601143b3"; // Sollomate Opalo
const AMP: &str = "6d01aebd8d8a"; // Omegaton A105 Hypercharged
const SCOPE: &str = "75c4f4cd08b3"; // Alekz Precision Scope
const HEALER: &str = "b4d20baff055"; // Vivo Oxy2000 (L) Adapted
const STIM: &str = "d6bdf2a6acea"; // AccuStim 5mg

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
        Some(payload) => builder.body(Body::from(payload.to_string())).unwrap(),
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

struct Arms {
    substrate_port: u16,
    upstream_port: u16,
    comparison_port: u16,
    native_db: PathBuf,
    comparison_db: PathBuf,
}

impl Arms {
    /// Same-state read comparison: the native arm against its own
    /// upstream backend, byte-for-byte (shared data dir and database).
    async fn compare_read(&self, method: &str, path: &str, body: Option<&str>) {
        let (native_status, native_headers, native_body) =
            request(self.substrate_port, method, path, body).await;
        let (py_status, py_headers, py_body) =
            request(self.upstream_port, method, path, body).await;
        assert_eq!(
            native_status,
            py_status,
            "status diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&py_body),
        );
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&py_headers),
            "contract headers diverged on {method} {path}"
        );
        assert_eq!(
            native_body,
            py_body,
            "body diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&py_body),
        );
    }

    /// Two-arm write comparison: identical request against the
    /// substrate (native) and the independent comparison backend.
    async fn compare_write(&self, method: &str, path: &str, body: Option<&str>) {
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
            "body diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
    }

    /// Apply the same (still-proxied) write to both arms, asserting
    /// status agreement only: the response embeds each arm's own
    /// dbPath, so the byte comparison belongs to the same-state read.
    async fn apply_both(&self, method: &str, path: &str, body: Option<&str>) {
        let (native_status, _, native_body) =
            request(self.substrate_port, method, path, body).await;
        let (cmp_status, _, cmp_body) = request(self.comparison_port, method, path, body).await;
        assert_eq!(
            native_status,
            cmp_status,
            "status diverged on {method} {path}
  native: {}
  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
    }

    async fn compare_db_state(&self, step: &str) {
        let native = snapshot_of(&self.native_db).await;
        let comparison = snapshot_of(&self.comparison_db).await;
        assert_eq!(native, comparison, "database state diverged after {step}");
    }
}

async fn snapshot_of(db_path: &PathBuf) -> String {
    let db = eo_services::db::Db::open(db_path)
        .await
        .expect("open db for snapshot");
    let rows = db.snapshot_rows().await.expect("snapshot rows");
    let mut normalizer = Normalizer::new();
    serialize(&capture(&rows, &mut normalizer))
}

/// Seed identical calibration and prospect-cache rows. The summaries
/// land in the materialised cache directly (their builder has its own
/// proof); the prospect maths and its response shaping are what this
/// battery holds.
async fn seed_character_state(pool: &SqlitePool, base_ts: f64) {
    for (name, level, source, offset) in [
        ("Rifle", 1200.0, "scan", -86400.0 * 3.0),
        ("Anatomy", 800.0, "scan", -86400.0 * 3.0),
        ("Agility", 30.0, "scan", -86400.0 * 3.0),
        ("Health", 142.7, "scan", -86400.0 * 3.0),
        ("Rifle", 1250.0, "chatlog", -3600.0),
        ("Courage", 500.0, "codex", -7200.0),
        ("BLP Weaponry Technology", 420.0, "chatlog", -3600.0),
    ] {
        sqlx::query(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(name)
        .bind(level)
        .bind(source)
        .bind(base_ts + offset)
        .execute(pool)
        .await
        .expect("seed calibration");
    }
    for (
        id,
        kills,
        loot,
        cycled,
        hours,
        mob,
        tag,
        weapon,
        skill_json,
        attr_json,
        skill_tt,
        attr_total,
    ) in [
        (
            "s-atrox-1",
            120_i64,
            95.5,
            100.0,
            1.5,
            "Atrox",
            "",
            "Sollomate Opalo",
            "{\"Rifle\": 1.8, \"Anatomy\": 0.6}",
            "{\"Agility\": 0.02}",
            2.4,
            0.02,
        ),
        (
            "s-snable",
            60_i64,
            41.25,
            50.0,
            0.75,
            "Snable",
            "team",
            "Omegaton A105",
            "{\"Rifle\": 0.7, \"BLP Weaponry Technology\": 0.4}",
            "{}",
            1.1,
            0.0,
        ),
        (
            "s-atrox-2",
            200_i64,
            180.0,
            175.0,
            2.25,
            "Atrox",
            "",
            "Sollomate Opalo",
            "{\"Rifle\": 3.0, \"Anatomy\": 1.1, \"Courage\": 0.2}",
            "{\"Agility\": 0.05}",
            4.3,
            0.05,
        ),
    ] {
        sqlx::query(
            "INSERT INTO session_summaries (session_id, summary_version, started_at, ended_at, \
             duration_hours, kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, \
             dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, \
             regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, \
             dominant_weapon, computed_at) \
             VALUES (?, 1, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(base_ts - 86400.0)
        .bind(base_ts - 86400.0 + hours * 3600.0)
        .bind(hours)
        .bind(kills)
        .bind(loot)
        .bind(cycled)
        .bind(skill_json)
        .bind(attr_json)
        .bind(skill_tt)
        .bind(attr_total)
        .bind(mob)
        .bind(tag)
        .bind(weapon)
        .bind(base_ts)
        .execute(pool)
        .await
        .expect("seed summary");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_r3_surface_conforms_through_the_public_port() {
    let upstream = spawn_sidecar();
    let comparison = spawn_sidecar();
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let native_db = upstream.data_dir.path().join("entropia_orme.db");
    let comparison_db = comparison.data_dir.path().join("entropia_orme.db");
    let open_pool = |path: PathBuf| async move {
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
    };
    let pool: SqlitePool = open_pool(native_db.clone()).await;
    let comparison_pool: SqlitePool = open_pool(comparison_db.clone()).await;

    let base_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock past the epoch")
        .as_secs_f64();
    seed_character_state(&pool, base_ts).await;
    seed_character_state(&comparison_pool, base_ts).await;

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
        upstream_port: upstream.port,
        comparison_port: comparison.port,
        native_db,
        comparison_db,
    };

    // ── Settings reads: byte-for-byte against the same stored state,
    //    dbPath and trifecta validation included ──
    arms.compare_read("GET", "/api/settings", None).await;
    arms.compare_read("GET", "/api/settings/overlay-position", None)
        .await;

    // ── Character reads over the seeded state ──
    arms.compare_read("GET", "/api/character/calibration", None)
        .await;
    arms.compare_read("GET", "/api/character/stats", None).await;
    arms.compare_read("GET", "/api/character/skills", None)
        .await;
    arms.compare_read("GET", "/api/character/professions", None)
        .await;
    arms.compare_read("GET", "/api/character/codex", None).await;
    arms.compare_read("GET", "/api/character/hp-optimizer", None)
        .await;
    arms.compare_read(
        "GET",
        "/api/character/profession-optimizer?profession=BLP%20Sniper%20(Hit)",
        None,
    )
    .await;
    arms.compare_read(
        "GET",
        "/api/character/profession-optimizer?profession=Nobody",
        None,
    )
    .await;
    arms.compare_read(
        "GET",
        "/api/character/profession-path-optimizer?profession=BLP%20Sniper%20(Hit)&target_level=6",
        None,
    )
    .await;
    arms.compare_read(
        "GET",
        "/api/character/profession-path-optimizer?profession=BLP%20Sniper%20(Hit)&ped_budget=25",
        None,
    )
    .await;

    // The prospect family: options, the live forecast (global and
    // sliced), the speculative-markup branch, and every error shape.
    arms.compare_read("GET", "/api/character/prospect-options", None)
        .await;
    for path in [
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6&markup_uplift=0.12",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6&slice_type=mob&slice_value=Atrox",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6&slice_type=weapon&slice_value=Sollomate%20Opalo",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6&slice_type=tag&slice_value=team",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=6&slice_type=mob&slice_value=Nothing",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=2",
        "/api/character/prospect?profession=Armor%20Engineer&target_level=20",
        "/api/character/prospect?profession=Nobody&target_level=5",
        "/api/character/prospect?profession=BLP%20Sniper%20(Hit)&target_level=1000",
    ] {
        arms.compare_read("GET", path, None).await;
    }

    // The character validation grid: envelope forms first (signature
    // order), then the handler's own 422 details.
    for path in [
        "/api/character/prospect",
        "/api/character/prospect?target_level=abc&markup_uplift=zz",
        "/api/character/prospect?profession=X&target_level=1_0",
        "/api/character/prospect?profession=X&target_level=0",
        "/api/character/prospect?profession=X&target_level=5&markup_uplift=-1",
        "/api/character/prospect?profession=X&target_level=5&slice_type=banana",
        "/api/character/prospect?profession=X&target_level=5&slice_type=tag",
        "/api/character/profession-optimizer",
        "/api/character/profession-path-optimizer?profession=X",
        "/api/character/profession-path-optimizer?profession=X&target_level=5&ped_budget=1",
        "/api/character/profession-path-optimizer?profession=X&target_level=abc",
    ] {
        arms.compare_read("GET", path, None).await;
    }

    // ── Equipment catalogue reads ──
    for path in [
        "/api/equipment/search?q=opalo",
        "/api/equipment/search?q=OPALO",
        "/api/equipment/search?q=o",
        "/api/equipment/search",
        "/api/equipment/search?q=vivo&type=healer",
        "/api/equipment/search?q=a105&type=amp",
        "/api/equipment/search?q=scope&type=scope",
        "/api/equipment/search?q=a&type=absorber",
        "/api/equipment/search?q=stim&type=consumable",
        "/api/equipment/search?q=x&type=banana",
        "/api/equipment/library",
    ] {
        arms.compare_read("GET", path, None).await;
    }

    // ── The equipment write sequence, state-compared after each step ──
    let weapon_full = format!(
        "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"amp_catalog_id\": \"{AMP}\", \
         \"scope_catalog_id\": \"{SCOPE}\", \"weapon_markup\": \"1_2_0\", \"amp_markup\": 105.0, \
         \"damage_enhancers\": 2}}"
    );
    arms.compare_write("POST", "/api/equipment/library", Some(&weapon_full))
        .await;
    arms.compare_db_state("full weapon add").await;
    arms.compare_write(
        "POST",
        "/api/equipment/library",
        Some(&format!(
            "{{\"type\": \"healing\", \"catalog_id\": \"{HEALER}\", \"weapon_markup\": 110}}"
        )),
    )
    .await;
    arms.compare_db_state("healing add").await;
    arms.compare_write(
        "POST",
        "/api/equipment/library",
        Some("{\"type\": \"consumable\", \"name\": \"  Caf\u{e9} Ration  \"}"),
    )
    .await;
    arms.compare_db_state("custom consumable add").await;
    arms.compare_write(
        "POST",
        "/api/equipment/library",
        Some(&format!(
            "{{\"type\": \"consumable\", \"catalog_id\": \"{STIM}\"}}"
        )),
    )
    .await;
    arms.compare_db_state("catalogue consumable add").await;

    // Library and detail reads over the populated rows.
    arms.compare_write("GET", "/api/equipment/library", None)
        .await;
    for item in 1..=4 {
        arms.compare_write(
            "GET",
            &format!("/api/equipment/library/{item}/detail"),
            None,
        )
        .await;
    }
    arms.compare_write("GET", "/api/equipment/library/99/detail", None)
        .await;

    // Updates: a reconfiguration, the type-change gate, the missing
    // row, and the malformed-id path legs.
    arms.compare_write(
        "PUT",
        "/api/equipment/library/1",
        Some(&format!(
            "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"weapon_markup\": 130}}"
        )),
    )
    .await;
    arms.compare_db_state("weapon reconfigure").await;
    arms.compare_write(
        "PUT",
        "/api/equipment/library/2",
        Some("{\"type\": \"weapon\", \"catalog_id\": \"x\"}"),
    )
    .await;
    arms.compare_write(
        "PUT",
        "/api/equipment/library/99",
        Some("{\"type\": \"consumable\", \"name\": \"X\"}"),
    )
    .await;
    arms.compare_write("DELETE", "/api/equipment/library/abc", None)
        .await;
    arms.compare_write(
        "GET",
        "/api/equipment/library/9999999999999999999999/detail",
        None,
    )
    .await;

    // The trifecta-reference guard: a preset claims items 1 and 2
    // through the (proxied) settings PATCH on each arm, the settings
    // read reflects the readiness against the live library, and the
    // delete answers 409 until the preset releases it.
    let preset_patch = "{\"trifecta_presets\": [{\"id\": \"main\", \"name\": \"Main\", \
                        \"small_weapon_id\": 1, \"big_weapon_id\": 1, \"heal_id\": 2}], \
                        \"active_trifecta_preset_id\": \"main\"}";
    arms.apply_both("PATCH", "/api/settings", Some(preset_patch))
        .await;
    arms.compare_read("GET", "/api/settings", None).await;
    arms.compare_write("DELETE", "/api/equipment/library/1", None)
        .await;
    arms.compare_write("DELETE", "/api/equipment/library/3", None)
        .await;
    arms.compare_db_state("guarded and free deletes").await;
    arms.apply_both("PATCH", "/api/settings", Some("{\"trifecta_presets\": []}"))
        .await;
    arms.compare_read("GET", "/api/settings", None).await;
    arms.compare_write("DELETE", "/api/equipment/library/1", None)
        .await;
    arms.compare_db_state("released delete").await;

    // ── Cost calculation (response-only: no state) ──
    let cost_full = format!(
        "{{\"catalog_id\": \"{WEAPON}\", \"amp_catalog_id\": \"{AMP}\", \
         \"scope_catalog_id\": \"{SCOPE}\", \"weapon_markup\": 120, \"amp_markup\": 105, \
         \"damage_enhancers\": 2}}"
    );
    arms.compare_write("POST", "/api/equipment/cost/calculate", Some(&cost_full))
        .await;
    arms.compare_write(
        "POST",
        "/api/equipment/cost/calculate",
        Some(&format!(
            "{{\"catalog_id\": \"{HEALER}\", \"type\": \"healing\", \"weapon_markup\": \"115\"}}"
        )),
    )
    .await;

    // ── The equipment validation grid ──
    for body in [
        // Literal violations, declaration-order multi-error, lax ints.
        "{\"type\": \"banana\"}",
        "{\"type\": 5}",
        "{\"type\": null}",
        "{\"type\": \"banana\", \"weapon_markup\": \"x\", \"damage_enhancers\": 1.5}",
        "{\"type\": \"weapon\"}",
        "{\"type\": \"weapon\", \"catalog_id\": \"\"}",
        "{\"type\": \"weapon\", \"catalog_id\": \"nope\"}",
        "{\"type\": \"weapon\", \"catalog_id\": 7}",
        &format!(
            "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"amp_catalog_id\": \"ghost\"}}"
        ),
        &format!(
            "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"absorber_catalog_id\": \"ghost\"}}"
        ),
        "{\"type\": \"healing\"}",
        "{\"type\": \"consumable\"}",
        "{\"type\": \"consumable\", \"name\": \"   \"}",
        "{\"type\": \"consumable\", \"catalog_id\": \"ghost\"}",
        "{}",
        "null",
        "[1, 2]",
        &format!(
            "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"weapon_markup\": true}}"
        ),
        &format!(
            "{{\"type\": \"weapon\", \"catalog_id\": \"{WEAPON}\", \"damage_enhancers\": -3}}"
        ),
    ] {
        arms.compare_write("POST", "/api/equipment/library", Some(body)).await;
    }
    arms.compare_db_state("validation grid leaves no writes")
        .await;
    for body in [
        "{}",
        "{\"catalog_id\": \"nope\"}",
        "{\"catalog_id\": 9, \"type\": \"consumable\", \"weapon_markup\": []}",
        &format!("{{\"catalog_id\": \"{WEAPON}\", \"type\": null}}"),
    ] {
        arms.compare_write("POST", "/api/equipment/cost/calculate", Some(body))
            .await;
    }

    // ── The R3 reads sit OUTSIDE the ETag middleware's prefixes:
    //    plain 200s, conditional validators ignored ──
    let (status, headers, _) = request(arms.substrate_port, "GET", "/api/settings", None).await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        !headers.contains_key(http::header::ETAG),
        "settings reads carry no conditional-GET contract"
    );
    let authority = format!("127.0.0.1:{}", arms.substrate_port);
    let conditional = http::Request::builder()
        .uri(format!("http://{authority}/api/character/stats"))
        .header("host", &authority)
        .header("if-none-match", "\"anything\"")
        .body(Body::empty())
        .unwrap();
    let response = client()
        .request(conditional)
        .await
        .expect("conditional GET");
    assert_eq!(
        response.status(),
        http::StatusCode::OK,
        "validators are ignored outside the ETag scope"
    );

    // The substrate proves which arm answered the native reads: the
    // server header is the substrate's own, not the proxied uvicorn's.
    let (_, headers, _) = request(arms.substrate_port, "GET", "/api/character/stats", None).await;
    let server = headers
        .get(http::header::SERVER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        !server.contains("uvicorn"),
        "native reads must not proxy (server: {server})"
    );
    // And the still-proxied settings PATCH reaches uvicorn through the
    // same substrate.
    let (_, headers, _) = request(
        arms.substrate_port,
        "PATCH",
        "/api/settings",
        Some("{\"player_name\": \"Probe\"}"),
    )
    .await;
    let server = headers
        .get(http::header::SERVER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        server.contains("uvicorn"),
        "settings writes stay proxied (server: {server})"
    );
}
