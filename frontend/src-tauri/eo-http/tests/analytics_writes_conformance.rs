//! Analytics WRITE-route conformance through the PUBLIC PORT: the ledger,
//! ledger-preset and inventory CRUD surface (`backend/routers/analytics.py`,
//! the `~789-1129` block) driven through BOTH arms from identical starting
//! states and compared after every step.
//!
//! TOPOLOGY (writes_conformance's two-arm form, on analytics_conformance's
//! frozen clock): two sidecars, both with their clock frozen at `CLOCK` =
//! arm A (comparison) and arm B (native upstream); the native substrate
//! stands over arm B's database with a `MockClock` at the same naive
//! instant. The frozen clock makes the inventory / sell date-defaults
//! (`_utc_date_str(clock)`) deterministic and literal-equal across arms.
//!
//! THE UUID WRINKLE: ledger / preset / inventory ids are `str(uuid4())`,
//! independently random per arm. Two comparison strategies follow:
//!   - RESPONSE bodies normalise through a FRESH `Normalizer` per arm
//!     (`Normalizer::new().normalize(&value)`), which symbolises UUIDs by
//!     encounter order (`<UUID_1>` ...); the frozen clock makes the date
//!     fields literal-equal, so the two normalised `Value`s compare.
//!   - PATH-ID ops (DELETE / PATCH / sell) cannot drive one arm's random id
//!     through the other, so a row with a FIXED id is seeded directly into
//!     BOTH databases via sqlx, then the op runs that fixed id through both
//!     arms.
//!
//! DB-STATE after writes:
//!   - `ledger_entries` is in the snapshot catalogue: compare via
//!     `snapshot_of` (Normalizer-based), exactly as writes_conformance does.
//!   - `ledger_presets` / `inventory_items` are NOT catalogued: a direct
//!     read-back over the API-observable columns only, normalised through a
//!     fresh per-arm `Normalizer` (UUID symbolisation), excludes the
//!     wall-clock `updated_at` (non-API-observable, differs per arm).
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test analytics_writes_conformance
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
use sqlx::SqlitePool;

const CLOCK: &str = "2026-06-01T12:00:00";
/// `_utc_date_str(clock)` for `CLOCK`: the date-default both arms stamp.
const CLOCK_DATE: &str = "2026-06-01";

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

/// A backend sidecar with its clock frozen at `CLOCK` (so the inventory /
/// sell date-defaults are deterministic and identical across arms).
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

/// A fresh-normaliser canonical form of a `Value`: symbolises the per-arm
/// random UUIDs by encounter order so two arms' bodies compare. The frozen
/// clock keeps `YYYY-MM-DD` date fields literal (they are below the
/// normaliser's 19-char ISO-datetime floor, so they pass through untouched).
fn normalised(value: &Value) -> Value {
    Normalizer::new().normalize(value)
}

struct Arms {
    substrate_port: u16,
    comparison_port: u16,
    native_db: PathBuf,
    comparison_db: PathBuf,
}

impl Arms {
    /// Drive one request through both arms; assert status + contract headers
    /// match, and the response bodies match after per-arm UUID normalisation.
    /// Returns the native arm's parsed (un-normalised) body for follow-up
    /// assertions where useful.
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
        // Non-JSON bodies (the 500 leg's plain-text "Internal Server Error",
        // or any non-parseable body) compare byte-for-byte.
        match (
            serde_json::from_slice::<Value>(&native_body),
            serde_json::from_slice::<Value>(&cmp_body),
        ) {
            (Ok(native), Ok(python)) => {
                assert_eq!(
                    normalised(&native),
                    normalised(&python),
                    "normalised body diverged on {method} {path}\n  native: {}\n  python: {}",
                    String::from_utf8_lossy(&native_body),
                    String::from_utf8_lossy(&cmp_body),
                );
                native
            }
            _ => {
                assert_eq!(
                    native_body,
                    cmp_body,
                    "non-json body diverged on {method} {path}\n  native: {}\n  python: {}",
                    String::from_utf8_lossy(&native_body),
                    String::from_utf8_lossy(&cmp_body),
                );
                Value::Null
            }
        }
    }

    /// Compare a LIST response whose route order can tie non-deterministically
    /// across arms (the preset list's `created_at ASC, id ASC` ties on the
    /// wall-clock `created_at` default, then the random `id` tiebreak diverges).
    /// Both arrays are sorted by `key_field` before per-arm UUID normalisation,
    /// so equal sets compare equal regardless of physical order; status and
    /// contract headers still compare directly.
    async fn compare_list_unordered(&self, path: &str, key_field: &str) {
        let (native_status, native_headers, native_body) =
            request(self.substrate_port, "GET", path, None).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, "GET", path, None).await;
        assert_eq!(native_status, cmp_status, "status diverged on GET {path}");
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&cmp_headers),
            "contract headers diverged on GET {path}"
        );
        let sort_by_key = |bytes: &[u8]| -> Value {
            let mut value: Value = serde_json::from_slice(bytes).expect("list parses");
            if let Some(items) = value.as_array_mut() {
                items.sort_by(|a, b| {
                    a[key_field]
                        .as_str()
                        .unwrap_or_default()
                        .cmp(b[key_field].as_str().unwrap_or_default())
                });
            }
            normalised(&value)
        };
        assert_eq!(
            sort_by_key(&native_body),
            sort_by_key(&cmp_body),
            "unordered list diverged on GET {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
    }

    /// Compare the catalogued database state (covers `ledger_entries`), one
    /// fresh normaliser per arm.
    async fn compare_db_state(&self, step: &str) {
        let native = snapshot_of(&self.native_db).await;
        let comparison = snapshot_of(&self.comparison_db).await;
        assert_eq!(native, comparison, "database state diverged after {step}");
    }

    /// Compare the un-catalogued `ledger_presets` read-back (API-observable
    /// columns only; `created_at` / `updated_at` excluded).
    async fn compare_presets_state(&self, step: &str) {
        let native = presets_readback(&self.native_db).await;
        let comparison = presets_readback(&self.comparison_db).await;
        assert_eq!(
            native, comparison,
            "ledger_presets state diverged after {step}"
        );
    }

    /// Compare the un-catalogued `inventory_items` read-back (API-observable
    /// columns only; `updated_at` excluded).
    async fn compare_inventory_state(&self, step: &str) {
        let native = inventory_readback(&self.native_db).await;
        let comparison = inventory_readback(&self.comparison_db).await;
        assert_eq!(
            native, comparison,
            "inventory_items state diverged after {step}"
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

/// `ledger_presets` read-back over the API-observable columns, normalised
/// through a fresh per-arm normaliser so the random ids symbolise to
/// `<UUID_n>`. Ordered by `name` (a content key identical across arms): the
/// route's `created_at ASC, id ASC` ties on the wall-clock `created_at`
/// default (which the frozen TEST clock does NOT govern), and the random
/// `id` tiebreak then orders differently per arm. Naming the rows distinctly
/// and ordering by name makes the encounter-order UUID symbolisation
/// deterministic, so equal sets compare equal regardless of physical order.
async fn presets_readback(db_path: &Path) -> Value {
    let pool = open_pool(db_path).await;
    let rows = sqlx::query_as::<_, (String, String, String, String, f64, String)>(
        "SELECT id, name, type, description, amount, tag FROM ledger_presets \
         ORDER BY name ASC, id ASC",
    )
    .fetch_all(&pool)
    .await
    .expect("read presets");
    let value = Value::Array(
        rows.into_iter()
            .map(|(id, name, kind, description, amount, tag)| {
                json!({
                    "id": id, "name": name, "type": kind,
                    "description": description, "amount": amount, "tag": tag,
                })
            })
            .collect(),
    );
    normalised(&value)
}

/// `inventory_items` read-back over the API-observable columns, in the
/// list-route's order (`acquired_at DESC, id DESC`), normalised per arm.
/// `updated_at` (wall-clock `unixepoch('now')`) is excluded.
async fn inventory_readback(db_path: &Path) -> Value {
    let pool = open_pool(db_path).await;
    let rows = sqlx::query_as::<_, (String, String, f64, f64, Option<String>, String)>(
        "SELECT id, name, tt_value, markup_paid, notes, acquired_at FROM inventory_items \
         ORDER BY acquired_at DESC, id DESC",
    )
    .fetch_all(&pool)
    .await
    .expect("read inventory");
    let value = Value::Array(
        rows.into_iter()
            .map(|(id, name, tt_value, markup_paid, notes, acquired_at)| {
                json!({
                    "id": id, "name": name, "ttValue": tt_value,
                    "markupPaid": markup_paid, "notes": notes, "acquiredAt": acquired_at,
                })
            })
            .collect(),
    );
    normalised(&value)
}

// ── Fixed-id seeding (the path-id strategy: identical rows in both dbs) ──

async fn seed_ledger_entry(db_path: &Path, id: &str, date: &str, kind: &str, amount: f64) {
    let pool = open_pool(db_path).await;
    sqlx::query(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(date)
    .bind(kind)
    .bind("Seeded entry")
    .bind(amount)
    .bind("seed")
    .execute(&pool)
    .await
    .expect("seed ledger entry");
}

async fn seed_preset(db_path: &Path, id: &str) {
    let pool = open_pool(db_path).await;
    // created_at fixed so the list order is deterministic across arms.
    sqlx::query(
        "INSERT INTO ledger_presets (id, name, type, description, amount, tag, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind("Seed Preset")
    .bind("expense")
    .bind("Seeded preset")
    .bind(3.5_f64)
    .bind("seed")
    .bind(1_700_000_000_i64)
    .execute(&pool)
    .await
    .expect("seed preset");
}

async fn seed_inventory(
    db_path: &Path,
    id: &str,
    name: &str,
    tt_value: f64,
    markup_paid: f64,
    acquired_at: &str,
) {
    let pool = open_pool(db_path).await;
    sqlx::query(
        "INSERT INTO inventory_items (id, name, tt_value, markup_paid, notes, acquired_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(tt_value)
    .bind(markup_paid)
    .bind(Option::<String>::None)
    .bind(acquired_at)
    .bind(1_700_000_000_i64)
    .execute(&pool)
    .await
    .expect("seed inventory item");
}

/// Seed identical fixed-id rows into BOTH arms' databases.
async fn seed_both<F, Fut>(arms: &Arms, mut f: F)
where
    F: FnMut(PathBuf) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    f(arms.native_db.clone()).await;
    f(arms.comparison_db.clone()).await;
}

/// Stand the native substrate over arm B's database, both arms' clocks
/// frozen at `CLOCK`.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_analytics_write_surface_conforms_through_the_public_port() {
    let (_upstream, _comparison, arms) = boot().await;

    // ── Ledger ──
    // create (response + ledger_entries snapshot).
    arms.compare_response(
        "POST",
        "/api/analytics/ledger",
        Some(r#"{"date": "2026-05-01", "type": "expense", "description": "Ammo", "amount": 12.5, "tag": "ammo"}"#),
    )
    .await;
    arms.compare_db_state("ledger create").await;
    // any type string accepted (no validation).
    arms.compare_response(
        "POST",
        "/api/analytics/ledger",
        Some(r#"{"date": "2026-05-02", "type": "whatever", "description": "Odd", "amount": 1.0, "tag": "x"}"#),
    )
    .await;
    arms.compare_db_state("ledger create lax type").await;
    // list.
    arms.compare_response("GET", "/api/analytics/ledger", None)
        .await;
    // delete a fixed-seeded entry (response + snapshot).
    seed_both(&arms, |db| async move {
        seed_ledger_entry(
            &db,
            "aaaaaaaa-0000-4000-8000-000000000001",
            "2026-05-03",
            "markup",
            9.0,
        )
        .await;
    })
    .await;
    arms.compare_response(
        "DELETE",
        "/api/analytics/ledger/aaaaaaaa-0000-4000-8000-000000000001",
        None,
    )
    .await;
    arms.compare_db_state("ledger delete").await;
    // delete-404 (response-only).
    arms.compare_response("DELETE", "/api/analytics/ledger/no-such-id", None)
        .await;

    // ── Presets ──
    // create (response + presets read-back).
    arms.compare_response(
        "POST",
        "/api/analytics/ledger/presets",
        Some(r#"{"name": "Decay", "type": "expense", "description": "Tool decay", "amount": 0.5, "tag": "decay"}"#),
    )
    .await;
    arms.compare_presets_state("preset create").await;
    arms.compare_response(
        "POST",
        "/api/analytics/ledger/presets",
        Some(r#"{"name": "Hide markup", "type": "markup", "description": "Hides", "amount": 4.0, "tag": "hide"}"#),
    )
    .await;
    arms.compare_presets_state("preset create markup").await;
    // create bad-type -> 400 (response-only, no mutation).
    arms.compare_response(
        "POST",
        "/api/analytics/ledger/presets",
        Some(r#"{"name": "Bad", "type": "income", "description": "x", "amount": 1.0, "tag": "y"}"#),
    )
    .await;
    arms.compare_presets_state("preset bad-type rejected").await;
    // list. The route order (`created_at ASC, id ASC`) ties on the wall-clock
    // `created_at` default and then diverges on the random id tiebreak, so the
    // two presets are compared as an unordered set keyed by name.
    arms.compare_list_unordered("/api/analytics/ledger/presets", "name")
        .await;
    // delete fixed-seeded (response + read-back).
    seed_both(&arms, |db| async move {
        seed_preset(&db, "bbbbbbbb-0000-4000-8000-000000000001").await;
    })
    .await;
    arms.compare_response(
        "DELETE",
        "/api/analytics/ledger/presets/bbbbbbbb-0000-4000-8000-000000000001",
        None,
    )
    .await;
    arms.compare_presets_state("preset delete").await;
    // delete-404.
    arms.compare_response("DELETE", "/api/analytics/ledger/presets/no-such", None)
        .await;

    // ── Inventory ──
    // create with snake_case body INCLUDING optional notes/acquired_at.
    arms.compare_response(
        "POST",
        "/api/analytics/inventory",
        Some(r#"{"name": "Modified Opalo", "tt_value": 10.0, "markup_paid": 2.5, "notes": "spare", "acquired_at": "2026-04-10"}"#),
    )
    .await;
    arms.compare_inventory_state("inventory create full").await;
    // create EXCLUDING optional fields: notes null, acquired_at defaults to
    // the clock date (frozen, so literal-equal).
    let created = arms
        .compare_response(
            "POST",
            "/api/analytics/inventory",
            Some(r#"{"name": "Imk2", "tt_value": 50.0, "markup_paid": 5.0}"#),
        )
        .await;
    assert_eq!(
        created["acquiredAt"],
        json!(CLOCK_DATE),
        "absent acquired_at defaults to the frozen clock date"
    );
    assert_eq!(created["notes"], Value::Null, "absent notes is null");
    arms.compare_inventory_state("inventory create defaults")
        .await;
    // list.
    arms.compare_response("GET", "/api/analytics/inventory", None)
        .await;

    // patch a fixed-seeded item: partial update (only some fields).
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "cccccccc-0000-4000-8000-000000000001",
            "Patchee",
            20.0,
            3.0,
            "2026-03-01",
        )
        .await;
    })
    .await;
    arms.compare_response(
        "PATCH",
        "/api/analytics/inventory/cccccccc-0000-4000-8000-000000000001",
        Some(r#"{"name": "Patched", "tt_value": 25.0}"#),
    )
    .await;
    arms.compare_inventory_state("inventory patch partial")
        .await;
    // a null field (must NOT update) + a provided field that does.
    arms.compare_response(
        "PATCH",
        "/api/analytics/inventory/cccccccc-0000-4000-8000-000000000001",
        Some(r#"{"markup_paid": 7.0, "notes": null}"#),
    )
    .await;
    arms.compare_inventory_state("inventory patch null-field")
        .await;
    // absent-field body (no fields update; row re-read and returned).
    arms.compare_response(
        "PATCH",
        "/api/analytics/inventory/cccccccc-0000-4000-8000-000000000001",
        Some("{}"),
    )
    .await;
    arms.compare_inventory_state("inventory patch empty").await;
    // patch-404.
    arms.compare_response(
        "PATCH",
        "/api/analytics/inventory/no-such",
        Some(r#"{"name": "Z"}"#),
    )
    .await;
    // delete fixed-seeded (response + read-back).
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "cccccccc-0000-4000-8000-000000000002",
            "Deletee",
            1.0,
            1.0,
            "2026-03-02",
        )
        .await;
    })
    .await;
    arms.compare_response(
        "DELETE",
        "/api/analytics/inventory/cccccccc-0000-4000-8000-000000000002",
        None,
    )
    .await;
    arms.compare_inventory_state("inventory delete").await;
    // delete-404.
    arms.compare_response("DELETE", "/api/analytics/inventory/no-such", None)
        .await;

    // ── Sell (fixed-seeded items; response + ledger snapshot + inventory) ──
    // PROFIT (sale > cost -> markup ledger entry), explicit description.
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "dddddddd-0000-4000-8000-000000000001",
            "Profit Item",
            10.0,
            2.0,
            "2026-02-01",
        )
        .await;
    })
    .await;
    arms.compare_response(
        "POST",
        "/api/analytics/inventory/dddddddd-0000-4000-8000-000000000001/sell",
        Some(r#"{"sale_price": 20.0, "description": "Sold for profit", "sold_at": "2026-05-10"}"#),
    )
    .await;
    arms.compare_db_state("sell profit").await;
    arms.compare_inventory_state("sell profit (item gone)")
        .await;
    // LOSS (sale < cost -> expense), no explicit description (default form),
    // no sold_at (clock-date default).
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "dddddddd-0000-4000-8000-000000000002",
            "Loss Item",
            30.0,
            5.0,
            "2026-02-02",
        )
        .await;
    })
    .await;
    arms.compare_response(
        "POST",
        "/api/analytics/inventory/dddddddd-0000-4000-8000-000000000002/sell",
        Some(r#"{"sale_price": 10.0}"#),
    )
    .await;
    arms.compare_db_state("sell loss default-desc default-date")
        .await;
    arms.compare_inventory_state("sell loss (item gone)").await;
    // ZERO-DELTA (sale == cost -> ledgerEntry null, item still deleted).
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "dddddddd-0000-4000-8000-000000000003",
            "Even Item",
            8.0,
            2.0,
            "2026-02-03",
        )
        .await;
    })
    .await;
    let zero = arms
        .compare_response(
            "POST",
            "/api/analytics/inventory/dddddddd-0000-4000-8000-000000000003/sell",
            Some(r#"{"sale_price": 10.0}"#),
        )
        .await;
    assert_eq!(
        zero["ledgerEntry"],
        Value::Null,
        "zero-delta sale emits no ledger entry"
    );
    arms.compare_db_state("sell zero-delta").await;
    arms.compare_inventory_state("sell zero-delta (item gone)")
        .await;
    // profit WITHOUT description (default "Inventory Sale: {name}") and WITH
    // an explicit sold_at.
    seed_both(&arms, |db| async move {
        seed_inventory(
            &db,
            "dddddddd-0000-4000-8000-000000000004",
            "Default Desc Item",
            1.0,
            0.0,
            "2026-02-04",
        )
        .await;
    })
    .await;
    arms.compare_response(
        "POST",
        "/api/analytics/inventory/dddddddd-0000-4000-8000-000000000004/sell",
        Some(r#"{"sale_price": 5.0, "sold_at": "2026-05-11"}"#),
    )
    .await;
    arms.compare_db_state("sell profit default-desc explicit-date")
        .await;
    // sell-404.
    arms.compare_response(
        "POST",
        "/api/analytics/inventory/no-such/sell",
        Some(r#"{"sale_price": 1.0}"#),
    )
    .await;

    // ── 422 validation leg (missing required body field) ──
    arms.compare_response(
        "POST",
        "/api/analytics/ledger",
        Some(r#"{"type": "expense", "description": "no date", "amount": 1.0, "tag": "x"}"#),
    )
    .await;

    // ── Surrogate-taint 500 leg ──
    // A lone-surrogate escape in a required string binds at storage; the
    // reference answers an unhandled-exception 500. Drive it on create and
    // assert both arms 500 (response-only; nothing written). Run it LAST so
    // any partial-write divergence cannot taint an earlier state comparison.
    arms.compare_response(
        "POST",
        "/api/analytics/ledger",
        Some("{\"date\": \"2026-05-01\", \"type\": \"expense\", \"description\": \"a\\ud800b\", \"amount\": 1.0, \"tag\": \"x\"}"),
    )
    .await;
}
