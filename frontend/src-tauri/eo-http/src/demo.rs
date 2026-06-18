//! Guide-mode demo playback: the native `/api/demo/*` read namespace
//! (`backend/routers/demo.py`).
//!
//! The demo serves a curated, never-mutated dataset that drives the in-app
//! guide: a bundled demo database plus a synthetic "mid-hunt" active session.
//! It mirrors the reference's mechanism exactly:
//!
//! - The bundled demo DB ships as a Tauri resource. On first demo access it is
//!   copied to a per-process working file and opened read/write, so the demo's
//!   priming writes never touch the bundled file. A parallel [`HydrationState`]
//!   serves the analytics + session-read surface over that copy, and a parallel
//!   [`HuntTracker`] serves the live snapshot, both entirely separate from the
//!   live tracking state.
//! - The analytics / session-list reads need only the database; the tracker is
//!   primed lazily, and only by the snapshot endpoint (mirroring the
//!   reference's `_ensure_conn` vs `_ensure_svc` split). Priming writes the
//!   mid-hunt session into the shared demo copy, so analytics reads taken after
//!   the snapshot reflect it, exactly as the reference's shared in-memory
//!   connection does.
//! - The mid-hunt session is synthesised relative to "now" (`started_at =
//!   now - elapsed`): its curated kill stream rides a committed fixture
//!   (captured from the reference prime; see `resources/mid_hunt_fixture.json`),
//!   replayed with every timestamp rebased onto the live clock. The fixture
//!   pins the data; the clock keeps the readout fresh.
//! - Routes are GET-only and carry NO ETag (the `/api/demo` prefix is outside
//!   the conditional-GET middleware), so every reply is a plain JSON 200.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use chrono::TimeDelta;
use eo_services::clock::Clock;
use eo_services::config_service::{AppConfig, TrifectaPresetConfig};
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::game_data_store::GameDataStore;
use eo_services::tracker::{naive_to_epoch, HuntTracker, Providers};
use eo_services::tracking_models::{Kill, LootItem, ToolStats, TrackingSession};
use serde::Deserialize;
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use crate::hydration::{
    detail, error_response, internal_error, plain_json_response, HydrationState,
};
use crate::tracking_routes::{get_session_impl, list_sessions_impl};
use crate::AppState;

/// The curated mid-hunt session, captured once from the reference demo's
/// `mid_hunt` prime. Timestamps are offsets from `started_at`; every other
/// value is replayed verbatim.
const MID_HUNT_FIXTURE: &str = include_str!("../resources/mid_hunt_fixture.json");

/// The demo's fixed mob lock and trifecta preset, matching the reference stub
/// in `_ensure_svc` (`backend/routers/demo.py`).
const DEMO_MOB: (&str, &str, &str) = ("Caboria Old", "Caboria", "Old");
const DEMO_PRESET_ID: &str = "demo_default";
const DEMO_PRESET_NAME: &str = "Calypso";
const DEMO_SMALL_WEAPON: &str = "Jester D-1";
const DEMO_BIG_WEAPON: &str = "Korss H400";
const DEMO_HEAL_TOOL: &str = "Vivo T1";

#[derive(Debug)]
pub enum DemoError {
    Io(std::io::Error),
    Db(eo_services::db::DbError),
    Sql(sqlx::Error),
    Fixture(serde_json::Error),
    MissingEquipment(String),
}

impl From<std::io::Error> for DemoError {
    fn from(e: std::io::Error) -> Self {
        DemoError::Io(e)
    }
}
impl From<eo_services::db::DbError> for DemoError {
    fn from(e: eo_services::db::DbError) -> Self {
        DemoError::Db(e)
    }
}
impl From<sqlx::Error> for DemoError {
    fn from(e: sqlx::Error) -> Self {
        DemoError::Sql(e)
    }
}
impl From<serde_json::Error> for DemoError {
    fn from(e: serde_json::Error) -> Self {
        DemoError::Fixture(e)
    }
}

// ── The committed fixture shape ──

#[derive(Deserialize)]
struct Fixture {
    elapsed_seconds: f64,
    session: FixtureSession,
    kills: Vec<FixtureKill>,
    skill_gains: Vec<FixtureSkillGain>,
    notable_events: Vec<FixtureNotable>,
}

#[derive(Deserialize)]
struct FixtureSession {
    id: String,
    is_active: i64,
    armour_cost: f64,
    heal_cost: f64,
    dangling_cost: f64,
}

#[derive(Deserialize)]
struct FixtureKill {
    id: String,
    mob_name: String,
    mob_species: String,
    mob_maturity: String,
    ts_offset: f64,
    shots_fired: i64,
    damage_dealt: f64,
    damage_taken: f64,
    critical_hits: i64,
    cost_ped: f64,
    enhancer_cost: f64,
    loot_total_ped: f64,
    is_global: i64,
    is_hof: i64,
    tool_stats: Vec<FixtureToolStat>,
    loot_items: Vec<FixtureLootItem>,
}

#[derive(Deserialize)]
struct FixtureToolStat {
    tool_name: String,
    shots_fired: i64,
    damage_dealt: f64,
    critical_hits: i64,
    cost_per_shot: f64,
}

#[derive(Deserialize)]
struct FixtureLootItem {
    item_name: String,
    quantity: i64,
    value_ped: f64,
    is_enhancer_shrapnel: i64,
}

#[derive(Deserialize)]
struct FixtureSkillGain {
    ts_offset: f64,
    skill_name: String,
    amount: f64,
    ped_value: f64,
}

#[derive(Deserialize)]
struct FixtureNotable {
    kill_id: String,
    event_type: String,
    mob_or_item: String,
    value_ped: f64,
    ts_offset: f64,
}

// ── The demo state ──

/// The parallel demo services over a writable clone of the bundled demo DB.
pub struct DemoState {
    db: Db,
    hydration: HydrationState,
    tracker: Arc<HuntTracker>,
    clock: Arc<dyn Clock>,
    fixture: Fixture,
    /// Snapshot-triggered, once: writes the mid-hunt session into the demo DB
    /// and primes the demo tracker (mirroring `_ensure_svc`).
    primed: OnceCell<()>,
}

impl DemoState {
    /// Build the demo services: copy the bundled demo DB to a per-process
    /// working file, open it, and stand up the parallel hydration + tracker.
    /// The tracker stays UNPRIMED until the first snapshot.
    pub async fn build(
        demo_db_path: &Path,
        game_data: Arc<GameDataStore>,
        clock: Arc<dyn Clock>,
        data_dir: PathBuf,
    ) -> Result<DemoState, DemoError> {
        let work = working_copy_path();
        // A stale copy from a prior run of the same pid (rare) must not be
        // adopted; start from the bundled file each launch.
        let _ = std::fs::remove_file(&work);
        std::fs::copy(demo_db_path, &work)?;
        let db = Db::open(&work).await?;
        let hydration = HydrationState::new(db.clone(), game_data, clock.clone(), data_dir);
        let bus = Arc::new(EventBus::new());
        let tracker = HuntTracker::new(
            bus,
            db.pool().clone(),
            Handle::current(),
            clock.clone(),
            Providers::default(),
        )?;
        let fixture: Fixture = serde_json::from_str(MID_HUNT_FIXTURE)?;
        Ok(DemoState {
            db,
            hydration,
            tracker,
            clock,
            fixture,
            primed: OnceCell::new(),
        })
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        self.db.pool()
    }

    fn now_epoch(&self) -> f64 {
        naive_to_epoch(self.clock.now())
    }

    // ── Analytics reads (delegated to the parallel hydration; no prime) ──

    pub async fn analytics_overview(&self, period: &str) -> Response<Body> {
        self.hydration.analytics_overview(period).await
    }

    pub async fn analytics_activity(&self) -> Response<Body> {
        self.hydration.analytics_activity(None).await
    }

    pub async fn list_ledger(&self) -> Response<Body> {
        self.hydration.list_ledger().await
    }

    pub async fn list_ledger_presets(&self) -> Response<Body> {
        self.hydration.list_ledger_presets().await
    }

    pub async fn list_inventory(&self) -> Response<Body> {
        self.hydration.list_inventory().await
    }

    // ── Session reads (the live `/api/tracking` versions carry ETag; the
    //    demo prefix does not, so these call the impls and reply plainly) ──

    pub async fn list_sessions(&self) -> Response<Body> {
        match list_sessions_impl(self.pool(), self.now_epoch()).await {
            Ok(value) => plain_json_response(&value),
            Err(_) => internal_error(),
        }
    }

    pub async fn get_session(&self, session_id: &str) -> Response<Body> {
        match get_session_impl(self.pool(), session_id, self.now_epoch()).await {
            Ok(Some(value)) => plain_json_response(&value),
            Ok(None) => error_response(StatusCode::NOT_FOUND, &detail("Session not found")),
            Err(_) => internal_error(),
        }
    }

    // ── The snapshot (primes on first access) ──

    pub async fn tracking_snapshot(&self) -> Response<Body> {
        if let Err(error) = self.ensure_primed().await {
            tracing::warn!("demo prime failed: {error:?}");
            return internal_error();
        }
        let config = match self.demo_config().await {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!("demo config failed: {error:?}");
                return internal_error();
            }
        };
        // hotbar_active is fixed `true` (the reference stub reports the listener
        // running); the demo reuses the live snapshot assembly verbatim.
        match self
            .hydration
            .build_snapshot_value(&self.tracker, &config, true)
            .await
        {
            Ok(value) => plain_json_response(&value),
            Err(_) => internal_error(),
        }
    }

    async fn ensure_primed(&self) -> Result<(), DemoError> {
        // `OnceCell` runs the prime exactly once even under concurrent first
        // snapshots; a failure is not cached, so a transient error can retry.
        if self.primed.get().is_some() {
            return Ok(());
        }
        let result = self.prime().await;
        if result.is_ok() {
            let _ = self.primed.set(());
        }
        result
    }

    /// Write the mid-hunt session into the demo DB and prime the demo tracker,
    /// rebasing every fixture timestamp onto the live clock. Mirrors
    /// `_write_demo_session_to_db` + `_prime_mid_hunt`.
    async fn prime(&self) -> Result<(), DemoError> {
        let started_naive = self.clock.now()
            - TimeDelta::milliseconds((self.fixture.elapsed_seconds * 1000.0).round() as i64);
        let started_epoch = naive_to_epoch(started_naive);
        let session = &self.fixture.session;

        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) \
             VALUES (?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(&session.id)
        .bind(started_epoch)
        .bind(session.is_active)
        .bind(session.armour_cost)
        .bind(session.heal_cost)
        .bind(session.dangling_cost)
        .execute(&mut *tx)
        .await?;

        for kill in &self.fixture.kills {
            sqlx::query(
                "INSERT INTO kills \
                 (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
                  shots_fired, damage_dealt, damage_taken, critical_hits, \
                  cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&kill.id)
            .bind(&session.id)
            .bind(&kill.mob_name)
            .bind(&kill.mob_species)
            .bind(&kill.mob_maturity)
            .bind(started_epoch + kill.ts_offset)
            .bind(kill.shots_fired)
            .bind(kill.damage_dealt)
            .bind(kill.damage_taken)
            .bind(kill.critical_hits)
            .bind(kill.cost_ped)
            .bind(kill.enhancer_cost)
            .bind(kill.loot_total_ped)
            .bind(kill.is_global)
            .bind(kill.is_hof)
            .execute(&mut *tx)
            .await?;

            for tool in &kill.tool_stats {
                sqlx::query(
                    "INSERT INTO kill_tool_stats \
                     (kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&kill.id)
                .bind(&tool.tool_name)
                .bind(tool.shots_fired)
                .bind(tool.damage_dealt)
                .bind(tool.critical_hits)
                .bind(tool.cost_per_shot)
                .execute(&mut *tx)
                .await?;
            }

            for item in &kill.loot_items {
                sqlx::query(
                    "INSERT INTO kill_loot_items \
                     (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&kill.id)
                .bind(&item.item_name)
                .bind(item.quantity)
                .bind(item.value_ped)
                .bind(item.is_enhancer_shrapnel)
                .execute(&mut *tx)
                .await?;
            }
        }

        for gain in &self.fixture.skill_gains {
            sqlx::query(
                "INSERT INTO skill_gains \
                 (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&session.id)
            .bind(started_epoch + gain.ts_offset)
            .bind(&gain.skill_name)
            .bind(gain.amount)
            .bind(gain.ped_value)
            .execute(&mut *tx)
            .await?;
        }

        for event in &self.fixture.notable_events {
            sqlx::query(
                "INSERT INTO notable_events \
                 (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&session.id)
            .bind(&event.kill_id)
            .bind(&event.event_type)
            .bind(&event.mob_or_item)
            .bind(event.value_ped)
            .bind(started_epoch + event.ts_offset)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        // Build the in-memory session and prime the parallel tracker (the
        // snapshot reads its computed readout). The kill values match the rows
        // just written, so the snapshot and the session-read surface agree.
        let kills: Vec<Kill> = self
            .fixture
            .kills
            .iter()
            .map(|kill| Kill {
                id: kill.id.clone(),
                session_id: session.id.clone(),
                mob_name: kill.mob_name.clone(),
                mob_species: kill.mob_species.clone(),
                mob_maturity: kill.mob_maturity.clone(),
                timestamp: started_epoch + kill.ts_offset,
                shots_fired: kill.shots_fired,
                damage_dealt: kill.damage_dealt,
                damage_taken: kill.damage_taken,
                critical_hits: kill.critical_hits,
                cost_ped: kill.cost_ped,
                enhancer_cost: kill.enhancer_cost,
                loot_total_ped: kill.loot_total_ped,
                loot_items: kill
                    .loot_items
                    .iter()
                    .map(|item| LootItem {
                        item_name: item.item_name.clone(),
                        quantity: item.quantity,
                        value_ped: item.value_ped,
                        is_enhancer_shrapnel: item.is_enhancer_shrapnel != 0,
                    })
                    .collect(),
                tool_stats: kill
                    .tool_stats
                    .iter()
                    .map(|tool| {
                        (
                            tool.tool_name.clone(),
                            ToolStats {
                                tool_name: tool.tool_name.clone(),
                                shots_fired: tool.shots_fired,
                                damage_dealt: tool.damage_dealt,
                                critical_hits: tool.critical_hits,
                                cost_per_shot: tool.cost_per_shot,
                            },
                        )
                    })
                    .collect(),
                is_global: kill.is_global != 0,
                is_hof: kill.is_hof != 0,
            })
            .collect();

        let demo_session = TrackingSession {
            id: session.id.clone(),
            start_time: started_naive,
            end_time: None,
            kills,
            dangling_cost: session.dangling_cost,
        };
        self.tracker.prime_demo(
            demo_session,
            (
                DEMO_MOB.0.to_string(),
                DEMO_MOB.1.to_string(),
                DEMO_MOB.2.to_string(),
            ),
            "manual",
            "mob",
        );
        Ok(())
    }

    /// The demo's config stub: trifecta mode with the curated "Calypso" preset,
    /// its weapon ids resolved by name from the demo equipment library (the
    /// reference's `_lookup_id`). Everything else is the default config.
    async fn demo_config(&self) -> Result<AppConfig, DemoError> {
        let preset = TrifectaPresetConfig {
            id: DEMO_PRESET_ID.to_string(),
            name: DEMO_PRESET_NAME.to_string(),
            small_weapon_id: Some(self.lookup_equipment_id(DEMO_SMALL_WEAPON).await?),
            big_weapon_id: Some(self.lookup_equipment_id(DEMO_BIG_WEAPON).await?),
            heal_id: Some(self.lookup_equipment_id(DEMO_HEAL_TOOL).await?),
        };
        Ok(AppConfig {
            hotbar_hooks_enabled: false,
            repair_ocr_enabled: false,
            end_of_session_armour_reminder_enabled: false,
            mob_tracking_mode: "mob".to_string(),
            mob_tracking_tag: String::new(),
            manual_mob_species: String::new(),
            manual_mob_maturity: String::new(),
            trifecta_presets: vec![preset],
            active_trifecta_preset_id: Some(DEMO_PRESET_ID.to_string()),
            ..AppConfig::default()
        })
    }

    async fn lookup_equipment_id(&self, name: &str) -> Result<i64, DemoError> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM equipment_library WHERE name = ?")
            .bind(name)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| DemoError::MissingEquipment(name.to_string()))
    }
}

/// The per-process working copy of the demo DB (writable, never the bundle).
fn working_copy_path() -> PathBuf {
    std::env::temp_dir().join(format!("entropiaorme-demo-{}.db", std::process::id()))
}

/// Resolve the lazily-built demo state, building it once on first demo access.
/// Returns `None` when the demo cannot be served (the native services are not
/// composed, no demo DB is bundled, or the build failed); the caller then falls
/// back to the proxy arm (hybrid) or a 503 (single-binary).
pub(crate) async fn ensure_demo(state: &Arc<AppState>) -> Option<Arc<DemoState>> {
    let cell = state.demo_cell();
    cell.get_or_init(|| async {
        let demo_db_path = state.demo_db_path()?;
        let hydration = state.hydration()?;
        match DemoState::build(
            &demo_db_path,
            hydration.game_data.clone(),
            hydration.clock.clone(),
            hydration.data_dir.clone(),
        )
        .await
        {
            Ok(demo) => Some(Arc::new(demo)),
            Err(error) => {
                tracing::warn!("demo state build failed: {error:?}");
                None
            }
        }
    })
    .await
    .clone()
}

// The goldens are the reference demo's exact HTTP bodies, captured under a
// frozen clock. The demo's now-relative session
// makes the absolute-datetime renderings (the snapshot `started_at` and
// `recentEvents[].timestamp`, the sessions-list `startTime`/`endTime`)
// clock/timezone-dependent; those are normalised before the comparison (the
// migration's frozen-ignore-list pattern), so this test pins the curated DATA
// and the native computation byte-for-byte while treating the live time
// rendering as the known non-deterministic surface. UTC-stable date buckets
// (the analytics timeline/monthly keys) are NOT datetime strings and stay
// asserted.
#[cfg(test)]
mod tests {
    use super::*;
    use eo_services::clock::MockClock;
    use http_body_util::BodyExt;
    use serde_json::Value;

    const G_OVERVIEW_ALL: &str =
        include_str!("../resources/demo_goldens/analytics_overview_all.txt");
    const G_OVERVIEW_30D: &str =
        include_str!("../resources/demo_goldens/analytics_overview_30d.txt");
    const G_ACTIVITY: &str = include_str!("../resources/demo_goldens/analytics_activity.txt");
    const G_LEDGER: &str = include_str!("../resources/demo_goldens/analytics_ledger.txt");
    const G_PRESETS: &str = include_str!("../resources/demo_goldens/analytics_ledger_presets.txt");
    const G_INVENTORY: &str = include_str!("../resources/demo_goldens/analytics_inventory.txt");
    const G_SESSIONS: &str = include_str!("../resources/demo_goldens/tracking_sessions.txt");
    const G_SESSION_DETAIL: &str =
        include_str!("../resources/demo_goldens/tracking_session_detail.txt");
    const G_SNAPSHOT: &str = include_str!("../resources/demo_goldens/tracking_snapshot.txt");

    /// The dev-tree bundled demo DB (the resource the app ships).
    fn demo_db_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/demo/entropia_orme.db")
    }

    /// A minimal game-data store: the demo routes never read the catalogue
    /// (analytics + the tracker snapshot are pure DB/state reads), but
    /// `HydrationState` requires one.
    fn empty_game_data(dir: &Path) -> Arc<GameDataStore> {
        let snapshot = dir.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("mobs.json"), "[]").unwrap();
        std::fs::write(snapshot.join("professions.json"), "[]").unwrap();
        std::fs::write(snapshot.join("skills.json"), "[]").unwrap();
        Arc::new(GameDataStore::new(&snapshot).unwrap())
    }

    async fn body_string(response: Response<Body>) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Recursively replace the now-relative surface with placeholders: ISO-8601
    /// datetime strings (`YYYY-MM-DDThh:...`) and the snapshot's `elapsed` count.
    /// Date-only (`YYYY-MM-DD`) and month (`YYYY-MM`) bucket keys lack the `T`
    /// and stay, so the analytics timeline is still asserted. The golden's
    /// `elapsed` is a wall-clock capture artifact (the reference froze the prime
    /// clock but not the snapshot clock); the native value is pinned to the
    /// deterministic 754 separately.
    fn normalise(value: &mut Value) {
        match value {
            Value::String(text) => {
                if is_iso_datetime(text) {
                    *text = "<TS>".to_string();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(normalise),
            Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if key == "elapsed" {
                        *child = Value::String("<ELAPSED>".to_string());
                    } else {
                        normalise(child);
                    }
                }
            }
            _ => {}
        }
    }

    fn is_iso_datetime(text: &str) -> bool {
        let b = text.as_bytes();
        b.len() >= 19
            && b[..4].iter().all(u8::is_ascii_digit)
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b'T'
    }

    fn assert_matches_golden(label: &str, body: &str, golden: &str) {
        let mut got: Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("{label}: native body is not JSON: {e}\n{body}"));
        let mut want: Value = serde_json::from_str(golden)
            .unwrap_or_else(|e| panic!("{label}: golden not JSON: {e}"));
        normalise(&mut got);
        normalise(&mut want);
        assert_eq!(
            got, want,
            "{label}: native demo output diverged from the golden"
        );
    }

    // The parallel tracker bridges DB work onto the runtime via `block_on`,
    // which requires the multi-threaded flavour (as in production).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn demo_routes_reproduce_the_reference_goldens() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(MockClock::new(
            Some(
                chrono::NaiveDateTime::parse_from_str("2026-06-18 12:00:00", "%Y-%m-%d %H:%M:%S")
                    .unwrap(),
            ),
            0.0,
        ));
        let demo = DemoState::build(
            &demo_db_path(),
            empty_game_data(dir.path()),
            clock,
            dir.path().to_path_buf(),
        )
        .await
        .expect("demo state builds over the bundled demo DB");

        // Snapshot FIRST: primes the mid-hunt session into the shared demo DB,
        // matching the capture order (analytics reads then reflect it).
        let snapshot = body_string(demo.tracking_snapshot().await).await;
        assert_matches_golden("snapshot", &snapshot, G_SNAPSHOT);
        // The now-relative readout: elapsed is the fixed mid-hunt window and the
        // session is active with the full kill stream.
        let snap: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snap["status"], "active");
        assert_eq!(snap["elapsed"], 754);
        assert_eq!(snap["kill_count"], 100);
        assert_eq!(snap["currentMob"], "Caboria Old");
        assert_eq!(snap["mobSource"], "manual");
        assert_eq!(snap["weaponAttribution"], "trifecta");
        assert!(is_iso_datetime(snap["started_at"].as_str().unwrap()));

        assert_matches_golden(
            "overview_all",
            &body_string(demo.analytics_overview("all").await).await,
            G_OVERVIEW_ALL,
        );
        assert_matches_golden(
            "overview_30d",
            &body_string(demo.analytics_overview("30d").await).await,
            G_OVERVIEW_30D,
        );
        assert_matches_golden(
            "activity",
            &body_string(demo.analytics_activity().await).await,
            G_ACTIVITY,
        );
        assert_matches_golden(
            "ledger",
            &body_string(demo.list_ledger().await).await,
            G_LEDGER,
        );
        assert_matches_golden(
            "presets",
            &body_string(demo.list_ledger_presets().await).await,
            G_PRESETS,
        );
        assert_matches_golden(
            "inventory",
            &body_string(demo.list_inventory().await).await,
            G_INVENTORY,
        );
        assert_matches_golden(
            "sessions",
            &body_string(demo.list_sessions().await).await,
            G_SESSIONS,
        );

        // Session detail for the primed mid-hunt session (the fixture's id, the
        // same one the golden was captured against).
        let fixture: Value = serde_json::from_str(MID_HUNT_FIXTURE).unwrap();
        let session_id = fixture["session"]["id"].as_str().unwrap();
        assert_matches_golden(
            "session_detail",
            &body_string(demo.get_session(session_id).await).await,
            G_SESSION_DETAIL,
        );
    }
}
