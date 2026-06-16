//! The native-services composition root.
//!
//! Mirrors the backend's own startup composition (`backend/main.py`):
//! resolve the data directory, open the application database, load the
//! game-data snapshot, and construct the ported services over the real
//! clock. The substrate serves natively-registered routes through the
//! state composed here; when any step declines, the substrate runs
//! proxy-only and the sidecar serves everything, exactly as before the
//! first flip.
//!
//! Composition grows with the takeover: this skeleton carries the
//! hydration read surface and the producer spine; the OCR recogniser
//! with its ONNX Runtime obligations joins it here, ahead of the scan
//! routes that consume it (those flip later).
//!
//! ## The ONNX Runtime obligations
//!
//! The recogniser (`eo_services::ocr_engine`) binds the ONNX Runtime
//! dynamically (the `load-dynamic` feature), so the composition root
//! must discharge three runtime obligations before any session is built:
//!
//! 1. **Pin the dylib to an absolute path.** [`init_ort_runtime`] calls
//!    [`ort::init_from`] with the *absolute* path to the bundled
//!    `onnxruntime.dll` (the installed resource dir in a release build,
//!    the repo copy in dev), never a bare name. A bare name would let
//!    the OS loader resolve a stray `onnxruntime.dll` off `PATH`/CWD; the
//!    absolute path makes the shipped runtime authoritative. Its siblings
//!    `DirectML.dll` and `onnxruntime_providers_shared.dll` sit next to
//!    it in both layouts, where ONNX Runtime's own module-relative load
//!    finds them at session-creation time.
//! 2. **Init the global environment once.** `init_from` + `commit()` is
//!    process-global and once-only (a second commit is a silent no-op),
//!    so [`init_ort_runtime`] guards it with a [`std::sync::Once`]:
//!    re-entry (tests, a second compose) never tries to reconfigure a
//!    committed environment.
//! 3. **Select the execution provider, with a guaranteed CPU fallback.**
//!    The EP ladder lives per-session in
//!    [`eo_services::ocr_engine::OcrEngine::new_with_providers`]
//!    (DirectML preferred, CPU fallback), not on the global env, so the
//!    engine owns its full session config. The env here carries no EPs.
//!
//! Two deliberate divergences from the original (`local_ocr.py`),
//! recorded so a later reviewer does not read them as oversights:
//!
//! * **Eager warm-up at composition, not lazy on first use.** The
//!   original warms the engine on the first `get_engine()`; we warm it at
//!   startup so the first real scan never eats DirectML shader
//!   compilation. The warm-up runs a synchronous, potentially multi-
//!   second inference, so it is offloaded onto a blocking thread
//!   ([`tokio::task::spawn_blocking`]) rather than stalling the
//!   substrate's async runtime worker during startup.
//! * **No queried provider string.** The original records
//!   `session.get_providers()[0]`; this ort version has no per-session
//!   provider readout, so the engine derives the provider from its
//!   construction control flow (the DirectML-then-CPU attempt) instead.
//!   The behaviour (DirectML-preferred with CPU fallback) is faithful;
//!   only the readback mechanism differs.
//!
//! A failed ORT init or engine load never declines composition: OCR is
//! one optional faculty, so the read surface and producer spine compose
//! regardless and the engine sits `None` (exactly as
//! `local_ocr.get_engine()` returns `None`, with the consumer seams
//! defaulting to unavailable until the scan routes flip).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eo_http::hydration::HydrationState;
use eo_services::chatlog_watcher::{ChatlogWatcher, QuestRewardFilter};
use eo_services::clock::{Clock, RealClock};
use eo_services::config_service::{
    active_trifecta_preset, load_config_readonly, AppConfig, ConfigService,
};
use eo_services::cost_engine::{cost_per_shot_from_props, heal_cost_per_use, heal_reload_seconds};
use eo_services::db::{AdoptError, Db};
use eo_services::eu_window;
use eo_services::event_bus::{EventBus, Topic};
use eo_services::game_data_store::GameDataStore;
use eo_services::hotbar_listener::{HotbarListener, HotbarResolver, HOTBAR_SLOT_KEYS};
use eo_services::keystroke_source::{HookKeystrokeSource, KeystrokeSource, SharedKeystrokeSource};
use eo_services::ocr_engine::load_bgr_png;
pub use eo_services::ocr_engine::OcrEngine;
use eo_services::paths::{resolve_data_dir, DB_FILE_NAME};
use eo_services::quests::QuestService;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders};
use eo_services::scan_completion::{complete_skill_scan, hydrate_skill_scan_state};
use eo_services::scan_presets::ScanPresets;
use eo_services::screen_capture::{capture_region_bgr, capture_region_png};
use eo_services::skill_panel::{read_skill_panel, BgrImage};
pub use eo_services::skill_scan_manual::SkillScanManual;
use eo_services::skill_scan_manual::{ScanProviders, ScanRegion};
use eo_services::skill_tracker::SkillTracker;
pub use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use eo_services::tracker::{naive_to_epoch, EquipmentProfile, HuntTracker, Providers};
use eo_services::trifecta_service::{describe_trifecta, TrifectaPreset};
use eo_wire::domain_events::DomainEvent;
use eo_wire::sse::SseHub;
use serde_json::{Map, Value};

/// The repository root, compiled into dev builds (the manifest dir is
/// `frontend/src-tauri/entropia-orme`). Release builds never read it.
fn dev_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// Where the user's data lives, by the backend's own rules. Release
/// builds are "frozen" in the backend's sense (the installed app);
/// dev builds honour `ENTROPIAORME_DATA_DIR` and the repo default.
pub(crate) fn data_dir() -> PathBuf {
    let override_value = std::env::var("ENTROPIAORME_DATA_DIR").ok();
    let frozen = !cfg!(debug_assertions);
    let appdata_root = std::env::var("APPDATA")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    resolve_data_dir(
        override_value.as_deref(),
        &dev_project_root(),
        frozen,
        &appdata_root,
    )
}

/// The rolling-log directory: a `logs/` subdirectory of the resolved data
/// directory, so the structured logs sit beside the database under the same
/// OS app-data root the backend already owns. Resolved the same way as
/// [`data_dir`], so the shell can create it at startup before composition.
pub(crate) fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Where the game-data snapshot lives: the bundled resource directory
/// in an installed build, the repository copy in dev.
fn snapshot_dir(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("snapshot"),
        _ => dev_project_root()
            .join("backend")
            .join("data")
            .join("snapshot"),
    }
}

/// The ABSOLUTE path to the bundled `onnxruntime.dll`: the installed
/// resource dir (`<resource_dir>/ort/onnxruntime.dll`) in a release
/// build, the committed repo copy
/// (`frontend/src-tauri/entropia-orme/resources/ort/onnxruntime.dll`)
/// in dev. Its siblings `DirectML.dll` and
/// `onnxruntime_providers_shared.dll` live in the same directory in
/// both layouts, where ONNX Runtime resolves them module-relative at
/// session creation. Always absolute (the dev branch resolves through
/// the compiled-in [`dev_project_root`], the installed branch through
/// the OS-resolved `resource_dir`), so the runtime is never sought on
/// `PATH`/CWD.
fn ort_dylib_path(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("ort").join("onnxruntime.dll"),
        _ => dev_project_root()
            .join("frontend")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("ort")
            .join("onnxruntime.dll"),
    }
}

/// Where the recogniser's model + dict live: the bundled resource dir
/// (`<resource_dir>/models/`) in a release build, the repo assets
/// (`backend/assets/models/`) in dev. The asymmetry mirrors
/// `tauri.conf.json`'s bundle map (`backend/assets/models/` ->
/// `models/`): the model is `svtrv2_rec.onnx`, the dict
/// `ppocr_keys_v1.txt`.
fn models_dir(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("models"),
        _ => dev_project_root()
            .join("backend")
            .join("assets")
            .join("models"),
    }
}

/// Pin the ONNX Runtime dylib to its absolute path and commit the global
/// environment, exactly once for the process. `init_from` + `commit()`
/// is once-only (a second commit silently no-ops and a second load keeps
/// the first dylib), so the [`std::sync::Once`] guard keeps a second
/// `compose_native` (tests, re-entry) from re-attempting it. No execution
/// providers are set on the env: the EP ladder is a per-session concern
/// owned by [`OcrEngine::new_with_providers`]. A failed init is logged,
/// not fatal: a later `OcrEngine::new_with_providers` simply fails and
/// the engine sits `None`.
///
/// SECURITY (the load-bearing invariant): this MUST run before any other
/// ORT API call in the process. It pins the dylib two ways: it sets
/// `ORT_DYLIB_PATH` to the absolute bundled path AND calls `init_from`
/// with it. The env pin matters because if `init_from` ever fails (a
/// missing/corrupt/quarantined dylib), ort's lazy fallback loader would
/// otherwise resolve a BARE `onnxruntime.dll` off the OS search order
/// (exe dir / PATH / CWD): a DLL-planting vector. Pinning `ORT_DYLIB_PATH`
/// to the absolute path closes that fallback so the loader can only ever
/// resolve the trusted bundled library, never a planted one. Our value
/// overwrites any attacker-set one because it runs first.
/// Pin `ORT_DYLIB_PATH` to the absolute bundled dylib path and return it.
/// This is the fallback-closing half of [`init_ort_runtime`]: even if
/// `init_from` later fails, ort's lazy loader resolves THIS absolute path
/// rather than a bare `onnxruntime.dll` off the OS search order. Split out
/// so the pin is unit-testable without the process-global `Once`.
fn pin_ort_dylib_env(resource_dir: Option<&PathBuf>) -> PathBuf {
    let dylib = ort_dylib_path(resource_dir);
    std::env::set_var("ORT_DYLIB_PATH", &dylib);
    dylib
}

fn init_ort_runtime(resource_dir: Option<&PathBuf>) {
    // The bundled ONNX Runtime is the Windows onnxruntime-directml build; on
    // any other platform there is no compatible runtime to pin (and loading
    // the Windows PE there hangs the loader), so OCR simply stays offline.
    if !cfg!(windows) {
        return;
    }
    static ORT_INIT: std::sync::Once = std::sync::Once::new();
    ORT_INIT.call_once(|| {
        // Pin the fallback loader to the absolute path before any ORT use
        // (see the SECURITY note above): even if `init_from` fails, the
        // lazy `setup_api` path resolves THIS path, never a bare name.
        let dylib = pin_ort_dylib_env(resource_dir);
        match ort::init_from(&dylib) {
            Ok(builder) => {
                // `commit()` returns false if an env was already
                // committed; the Once guard makes that unreachable here,
                // but the result is discarded either way (the env is now
                // configured).
                let _ = builder.with_name("entropiaorme").commit();
            }
            Err(err) => tracing::warn!(
                target: "eo::composition",
                "ONNX Runtime at {} unavailable ({err}); OCR will be offline until restart",
                dylib.display()
            ),
        }
    });
}

/// The live producer spine: the in-process event bus, the chat-log
/// watcher (tailing in its own thread), and the trackers subscribed to
/// the bus, all sharing the substrate's single-owner database pool and
/// one injected clock. Kept as a sibling of [`HydrationState`] so the
/// read surface stays a pure read surface and the producers are a
/// separate, stoppable concern.
///
/// The struct owns the producers for the substrate's lifetime; the
/// trackers and the quest service hold their own bus registrations and
/// stay alive only because this struct does. [`ProducerState::stop`]
/// (driven from the Tauri exit seam) stops the watcher's tail thread,
/// ends any open session, and drops the bus, so the OS-thread machinery
/// the watcher owns is torn down deterministically rather than left to
/// process-exit teardown.
pub struct ProducerState {
    watcher: ChatlogWatcher,
    tracker: Arc<HuntTracker>,
    // The SSE fan-out hub. The bus bridge (subscribed below) holds clones
    // of this through its subscriber closures, and the HTTP `/api/events`
    // handler serves over a clone handed off before this state moves into
    // the Tauri holder, so the publisher side and the stream side share
    // one hub.
    sse_hub: Arc<SseHub>,
    // The settings writer. Held here so the producer spine and the HTTP
    // write path share one service; a clone is handed to the app state at
    // composition. Mutex-guarded because `update`/`reset` take `&mut self`.
    config_service: Arc<Mutex<ConfigService>>,
    // The skill tracker. Held to keep its permanent bus subscription alive
    // for the substrate's lifetime, and exposed: the codex claim routes
    // call `suppress_next` on it.
    skill_tracker: Arc<SkillTracker>,
    // The in-process event bus the whole spine publishes on. Stored (rather
    // than left implicit on the subscriber handles) so the scan services
    // composed alongside the spine publish `scan.status.changed` on the SAME
    // bus the SSE bridge subscribes, and so the `/api/events` stream carries
    // scan-status frames once the scan routes flip.
    bus: Arc<EventBus>,
    // The hotbar key listener. A producer (it publishes tool-change events on
    // the bus), gated on the hotbar-hooks toggle and an active session; held
    // here so the snapshot route can read whether it is running and so the
    // exit seam stops it. Shares the one keystroke source below.
    hotbar: Arc<HotbarListener>,
    // The one OS keyboard hook the input listeners share (the hotbar listener
    // here and the spacebar listener composed alongside the scan services).
    // Held so the spacebar listener can be built over the SAME source; the
    // hook is single-instance, so two independent sources would have one
    // stand inert.
    keystroke_source: Arc<dyn KeystrokeSource>,
    // Held to keep its permanent bus subscription alive for the substrate's
    // lifetime; never read directly here.
    _quests: Arc<QuestService>,
}

impl ProducerState {
    /// Stop the producer spine: end any open session (so its stop
    /// events publish cleanly while the bus is still live), stop the
    /// watcher's tail thread, then drop the bus. Idempotent enough for
    /// the exit path: a second stop is a no-op on an already-stopped
    /// watcher and an already-idle tracker.
    pub fn stop(&self) {
        // Stop the input listener first (it detaches the shared OS hook), then
        // end any open session while the bus is live, then the watcher.
        self.hotbar.stop();
        if self.tracker.is_tracking() {
            let _ = self.tracker.stop_session();
        }
        self.watcher.stop();
    }

    /// The composed watcher, for tests driving a replay through it.
    #[cfg(test)]
    pub fn watcher(&self) -> &ChatlogWatcher {
        &self.watcher
    }

    /// The composed tracker, for tests asserting its readout.
    #[cfg(test)]
    pub fn tracker(&self) -> &Arc<HuntTracker> {
        &self.tracker
    }

    /// A handle to the composed tracker. The producer routes serve over
    /// this same `Arc<HuntTracker>`: the composition handoff clones it into
    /// the HTTP app state before this `ProducerState` moves into the
    /// Tauri-managed producer holder, so the routes and the exit-seam
    /// teardown share one tracker.
    pub fn tracker_handle(&self) -> Arc<HuntTracker> {
        self.tracker.clone()
    }

    /// A handle to the composed SSE hub. The `/api/events` stream serves
    /// over this same `Arc<SseHub>`: the composition handoff clones it into
    /// the HTTP app state before this `ProducerState` moves into the
    /// Tauri-managed producer holder, so the stream and the producer-bus
    /// bridge share one hub.
    pub fn sse_hub_handle(&self) -> Arc<SseHub> {
        self.sse_hub.clone()
    }

    /// A handle to the composed settings writer. The settings-write routes
    /// serve over this same `Arc<Mutex<ConfigService>>`: the composition
    /// handoff clones it into the HTTP app state before this `ProducerState`
    /// moves into the Tauri-managed holder, so the write path and the spine
    /// share one service (reads elsewhere stay file-based, coherent because
    /// every save reads-merges-before-write).
    pub fn config_service_handle(&self) -> Arc<Mutex<ConfigService>> {
        self.config_service.clone()
    }

    /// A handle to the composed skill tracker. The codex claim routes call
    /// `suppress_next` on this same `Arc<SkillTracker>`: cloned into the app
    /// state at the handoff, so the route side and the producer-bus
    /// subscription side share one tracker.
    pub fn skill_tracker_handle(&self) -> Arc<SkillTracker> {
        self.skill_tracker.clone()
    }

    /// A handle to the spine's event bus. The scan services compose on this
    /// same `Arc<EventBus>`, so their `scan.status.changed` envelopes reach
    /// the SSE bridge (subscribed in [`compose_producers`]) and the
    /// `/api/events` stream, exactly as the tracker's session frames do.
    pub fn bus_handle(&self) -> Arc<EventBus> {
        self.bus.clone()
    }

    /// A handle to the composed hotbar listener. The snapshot route reads its
    /// `is_running` flag; cloned into the app state at the handoff so the
    /// route side and the producer-spine side share one listener.
    pub fn hotbar_handle(&self) -> Arc<HotbarListener> {
        self.hotbar.clone()
    }

    /// A handle to the shared OS keyboard hook. The spacebar-capture listener
    /// composes over this SAME source, so both input listeners ride one hook
    /// (it is single-instance) while each gates independently.
    pub fn keystroke_source_handle(&self) -> Arc<dyn KeystrokeSource> {
        self.keystroke_source.clone()
    }
}

/// What a successful composition yields: the read surface, the producer
/// spine (sharing one pool and one clock), and the warmed OCR engine
/// when the runtime loaded.
///
/// `ocr_engine` is a sibling of `producers`, not a member of it: the
/// producer spine is the bus-subscribed, stoppable concern (watcher tail
/// thread, trackers, an exit-seam `stop()`), whereas the engine owns no
/// thread, no subscription, and no teardown obligation (its ONNX session
/// drops with the handle and the ORT env self-releases at process exit).
/// It is `Option` because OCR is an optional faculty: a failed runtime
/// load leaves it `None` while the rest of composition still succeeds.
/// `Arc` because the scan consumer seams will each capture a clone when
/// their routes flip.
pub struct Composed {
    pub hydration: Arc<HydrationState>,
    pub producers: ProducerState,
    pub ocr_engine: Option<Arc<OcrEngine>>,
    /// The manual skill-scan state machine, composed on the spine bus (its
    /// `scan.status.changed` envelopes reach the SSE stream) over the OCR
    /// extraction providers. Always constructed so the scan routes serve;
    /// its capture and extraction seams stand down to "engine unavailable"
    /// when the OCR runtime is absent, exactly as the sidecar reports.
    pub skill_scan: Arc<SkillScanManual>,
    /// The one-shot repair-cost OCR service, composed over the same capture
    /// and recogniser seams.
    pub repair_ocr: Arc<RepairOcrService>,
    /// The spacebar-capture listener, composed over the scan and the shared
    /// OS hook. Held for the spacebar-capture route (its toggle) and the exit
    /// seam (its teardown).
    pub spacebar_listener: Arc<SpacebarCaptureListener>,
}

/// The outcome of a composition attempt at the substrate's startup, which
/// the orchestration acts on: install the services, retry shortly, or stay
/// proxy-only for the session.
pub enum Composition {
    /// The native services are built and ready to install.
    Ready(Composed),
    /// The existing database is below the adoptable baseline: the sidecar
    /// has not finished migrating it forward yet (the first launch after an
    /// upgrade). Retrying composition once it has will adopt it, so the
    /// caller waits briefly and tries again rather than standing down.
    AwaitingMigration,
    /// A permanent decline (a missing/empty snapshot, a producer fault, or a
    /// database fault unrelated to the migration race). The substrate stays
    /// proxy-only for the rest of the session; retrying would not help.
    Declined,
}

/// Compose the native services, or decline with a logged reason.
/// Declining is always safe: the substrate then proxies every route to
/// the sidecar. The ONNX Runtime dylib is pinned (once) before any
/// composition step, so the engine constructed inside `compose_with`
/// binds the bundled runtime, not a stray one off `PATH`.
pub async fn compose_native(resource_dir: Option<PathBuf>) -> Composition {
    init_ort_runtime(resource_dir.as_ref());
    compose_with(
        data_dir(),
        snapshot_dir(resource_dir.as_ref()),
        models_dir(resource_dir.as_ref()),
    )
    .await
}

/// Composition over already-resolved locations (separated from the
/// environment-reading resolution so the decline paths are testable).
/// `models` is the recogniser's model+dict directory; the engine is
/// constructed and warmed from it, but a failed engine load never
/// declines composition (OCR is optional).
async fn compose_with(data_dir: PathBuf, snapshot: PathBuf, models: PathBuf) -> Composition {
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        tracing::error!(
            target: "eo::composition",
            "data dir {} not creatable ({err}); native services stand down",
            data_dir.display()
        );
        return Composition::Declined;
    }
    let db_path = data_dir.join(DB_FILE_NAME);
    let db = match Db::open_adopted(&db_path).await {
        Ok(db) => db,
        Err(err) if err.is_below_baseline() => {
            // The existing database is still at the pre-upgrade schema: the
            // sidecar has not finished migrating it up to the baseline this
            // substrate adopts at. This is the first-launch-after-upgrade
            // race, not a fault, so stand down only for now and let the
            // caller retry once the sidecar has migrated it forward.
            return Composition::AwaitingMigration;
        }
        Err(err @ AdoptError::Quarantined { .. }) => {
            // An existing database we cannot adopt (for any other reason) is
            // surfaced loudly and left untouched; the sidecar (whose own
            // migration logic governs it as before) keeps serving.
            tracing::error!(target: "eo::composition", "{err}");
            return Composition::Declined;
        }
        Err(err) => {
            tracing::error!(
                target: "eo::composition",
                "database open failed ({err}); native services stand down"
            );
            return Composition::Declined;
        }
    };
    let game_data = match GameDataStore::new(&snapshot) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            tracing::error!(
                target: "eo::composition",
                "game-data snapshot at {} unreadable ({err}); native services stand down",
                snapshot.display()
            );
            return Composition::Declined;
        }
    };
    // The store tolerates a missing directory (the backend's
    // warn-and-continue, sensible for its own embedded copy), but an
    // empty store here means the bundled resources are absent or
    // broken: serving game-data-derived responses from it would
    // silently diverge from the sidecar's embedded copy. Stand down
    // and let the proxy serve instead.
    if game_data.total_entities() == 0 {
        tracing::error!(
            target: "eo::composition",
            "game-data snapshot at {} is empty; native services stand down",
            snapshot.display()
        );
        return Composition::Declined;
    }
    let clock: Arc<dyn Clock> = Arc::new(RealClock::new());

    // The producer spine shares the substrate's single-owner pool with
    // the read surface: one connection, one owner, serialised access
    // (WAL + busy_timeout, max_connections(1)), so producer writes and
    // HTTP reads queue through the single connection without deadlock.
    // A second handle over the SAME pool: `Db` is a thin clonable handle
    // around the connection pool, so the producers and the read surface
    // share one connection (one owner, serialised access) rather than
    // opening a second.
    let producer_db = Db::from_pool(db.pool().clone());
    let producers = match compose_producers(producer_db, clock.clone(), &data_dir, None) {
        Ok(producers) => producers,
        Err(err) => {
            tracing::error!(
                target: "eo::composition",
                "producer spine failed ({err}); native services stand down"
            );
            return Composition::Declined;
        }
    };

    // Construct the recogniser off the runtime worker (awaited; the engine
    // must exist for the handoff), with warm-up detached so the slow first
    // inference does not gate compose -> serve (see `build_ocr_engine`). A
    // failed load is logged and leaves the engine `None`; OCR is optional,
    // so composition still succeeds. The DirectML-then-CPU ladder and the
    // recorded provider live in `OcrEngine::new_with_providers`.
    let ocr_engine = build_ocr_engine(models).await;

    // The scan services compose on the spine bus (so their status frames
    // reach the SSE stream) over the OCR extraction providers and the
    // shared single-owner pool, before the read surface takes ownership of
    // `db`/`game_data`/`clock`/`data_dir`. The calibration artefact sits
    // beside the snapshot dir in both the dev and installed layouts, so it
    // resolves as the snapshot's sibling.
    let geometry_path = snapshot
        .parent()
        .map(|parent| parent.join("panel_geometry.json"))
        .unwrap_or_else(|| snapshot.join("panel_geometry.json"));
    let (skill_scan, repair_ocr, spacebar_listener) = compose_scan_services(
        producers.bus_handle(),
        ocr_engine.clone(),
        game_data.clone(),
        db.clone(),
        clock.clone(),
        geometry_path,
        producers.keystroke_source_handle(),
    )
    .await;

    let hydration = Arc::new(HydrationState::new(db, game_data, clock, data_dir));
    Composition::Ready(Composed {
        hydration,
        producers,
        ocr_engine,
        skill_scan,
        repair_ocr,
        spacebar_listener,
    })
}

/// Construct the OCR engine off the async runtime worker, then warm it up
/// DETACHED. Construction (a session commit) is awaited because the engine
/// must exist before the handoff to managed state, but it is quick. Warm-up
/// (a real inference; DirectML compiles shaders on first run, seconds) is
/// NOT awaited: stalling it ahead of `serve()` would make every request the
/// webview fires during startup hang, so it runs on a detached blocking
/// thread, concurrent with the server coming up. The reference warms lazily
/// on first scan, so deferring the cost off the startup path is, if
/// anything, more faithful. Returns `None` (logged) on any load failure:
/// OCR is an optional faculty and never declines composition.
async fn build_ocr_engine(models: PathBuf) -> Option<Arc<OcrEngine>> {
    // OCR ships only where a compatible ONNX Runtime is bundled, which today
    // is Windows (the onnxruntime-directml libraries). On other platforms the
    // engine stays absent rather than attempting to load the Windows runtime
    // (which hangs the loader), exactly as a failed load would leave it.
    if !cfg!(windows) {
        return None;
    }
    let model_path = models.join("svtrv2_rec.onnx");
    let dict_path = models.join("ppocr_keys_v1.txt");
    let constructed =
        tokio::task::spawn_blocking(move || OcrEngine::new_with_providers(&model_path, &dict_path))
            .await;
    let engine = match constructed {
        Ok(Ok(engine)) => Arc::new(engine),
        Ok(Err(err)) => {
            tracing::warn!(
                target: "eo::composition",
                "OCR engine unavailable ({err}); scan features offline until restart"
            );
            return None;
        }
        Err(err) => {
            tracing::warn!(
                target: "eo::composition",
                "OCR engine construction task failed ({err}); scan features offline"
            );
            return None;
        }
    };
    tracing::info!(
        target: "eo::composition",
        "OCR engine ready (provider={})",
        engine.provider()
    );
    // Warm up detached so the multi-second first inference never gates the
    // compose -> serve handoff. Best-effort: the result is discarded and a
    // failure cannot affect the already-constructed engine.
    let warming = engine.clone();
    tokio::task::spawn_blocking(move || warming.warm_up());
    Some(engine)
}

/// Compose the manual skill scan and the repair-cost OCR over the OCR
/// engine, the live game-window region lookups, and the on-demand screen
/// capturer. The scan publishes `scan.status.changed` on the spine `bus`
/// (so its frames reach the SSE stream), persists accepted calibrations
/// through the shared `pool`, and hydrates its resting status from the
/// same. When the OCR runtime is absent the providers stand down to
/// "engine unavailable" rather than declining composition, so the routes
/// still serve (the scan reports offline), exactly as the sidecar does
/// without a loadable engine. Both services are always constructed.
async fn compose_scan_services(
    bus: Arc<EventBus>,
    ocr_engine: Option<Arc<OcrEngine>>,
    game_data: Arc<GameDataStore>,
    db: Db,
    clock: Arc<dyn Clock>,
    geometry_path: PathBuf,
    keystroke_source: Arc<dyn KeystrokeSource>,
) -> (
    Arc<SkillScanManual>,
    Arc<RepairOcrService>,
    Arc<SpacebarCaptureListener>,
) {
    let runtime = tokio::runtime::Handle::current();
    let pool = db.pool().clone();

    // The calibrated panel grid and the canonical skill vocabulary the
    // panel reader resolves names against; both read existing snapshot
    // assets (the geometry artefact beside the snapshot dir, the skill
    // names from the bundled `skills` endpoint), so this is port scope, not
    // new surface.
    let presets = Arc::new(ScanPresets::new(&geometry_path));
    let skill_geom = presets.skill.to_geom_value();
    let vocab: Vec<String> = game_data
        .get_entities("skills")
        .iter()
        .filter_map(|entity| {
            entity
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();

    // The skill-scan provider seams: engine availability is fixed at the
    // load attempt (the engine never reloads at runtime); the region lookup
    // reads the live game window through the calibrated anchor; the capturer
    // grabs the PNG on demand; the extractor decodes, slices, OCRs, and
    // filters to resolved (name, level) rows.
    let has_engine = ocr_engine.is_some();
    let region_presets = presets.clone();
    let skill_region: Arc<dyn Fn() -> Option<ScanRegion> + Send + Sync> =
        Arc::new(move || eu_window::skill_region(&region_presets));
    let capture_region: Arc<dyn Fn(ScanRegion) -> Option<Vec<u8>> + Send + Sync> =
        Arc::new(|(tl, br): ScanRegion| {
            capture_region_png(tl[0], tl[1], br[0] - tl[0], br[1] - tl[1])
        });
    let extract_engine = ocr_engine.clone();
    let extract_geom = skill_geom;
    let extract_vocab = vocab;
    let extract_page_levels: eo_services::skill_scan_manual::PageExtractor =
        Arc::new(move |png: &[u8]| {
            read_skill_page_levels(&extract_engine, &extract_geom, &extract_vocab, png)
        });

    let scan_providers = ScanProviders {
        engine_available: Arc::new(move || has_engine),
        skill_region,
        capture_region,
        extract_page_levels,
    };

    // Hydrate the resting status from the persisted calibration history,
    // exactly as the sidecar seeds initial scan time and skill count.
    let (initial_scan_time, initial_skills_count) =
        hydrate_skill_scan_state(&pool).await.unwrap_or((None, 0));

    let skill_scan = SkillScanManual::new(
        scan_providers,
        clock.clone(),
        Some(bus),
        initial_scan_time,
        initial_skills_count,
    );

    // The completion callback persists accepted calibrations through the
    // shared pool, bridging onto the runtime from the scan's worker thread
    // the same dual way the tracker's providers do; a persist error
    // surfaces on the scan status, exactly as the sidecar's caught
    // exception does.
    let completion_pool = pool;
    let completion_clock = clock.clone();
    let completion_runtime = runtime;
    skill_scan.set_completion_callback(Arc::new(move |levels: &[(String, f64)]| {
        let scan_time = naive_to_epoch(completion_clock.now());
        let levels = levels.to_vec();
        let pool = completion_pool.clone();
        block_on_pool(&completion_runtime, async move {
            complete_skill_scan(&pool, &levels, scan_time).await
        })
        .map(|_drift| ())
        .map_err(|err| err.to_string())
    }));

    // The repair-OCR provider seams: the same calibrated region lookup and
    // capturer (BGR pixels here), recognised by the shared engine.
    let repair_presets = presets;
    let repair_engine = ocr_engine;
    let repair_ocr = Arc::new(RepairOcrService::new(RepairProviders {
        repair_region: Arc::new(move || eu_window::repair_region(&repair_presets)),
        capture_region: Arc::new(capture_region_bgr),
        read_text: Arc::new(move |frame: &BgrImage| {
            repair_engine
                .as_ref()?
                .recognize_bgr(&frame.data, frame.h, frame.w)
                .ok()
        }),
    }));

    // The spacebar-capture listener fires a manual-scan capture on a space
    // press while the scan is capturing, over the SAME OS hook the hotbar
    // listener rides (the hook is single-instance). Off until the overlay
    // toggle enables it through the spacebar-capture route.
    let spacebar = SpacebarCaptureListener::new(skill_scan.clone(), Some(keystroke_source));

    (skill_scan, repair_ocr, spacebar)
}

/// Extract `{canonical_name: level}` rows from one captured skill-panel
/// PNG: decode to BGR, slice the calibrated grid, OCR each name/level cell,
/// estimate each bar fill, and keep only the rows that both resolved a name
/// and parsed a level. Returns no rows when the engine is absent, the PNG
/// is unreadable, or the grid is uncalibrated (the reader would otherwise
/// have no rows to slice and would panic), so an installed build missing
/// the calibration artefact degrades to an empty scan rather than crashing.
fn read_skill_page_levels(
    engine: &Option<Arc<OcrEngine>>,
    skill_geom: &Value,
    vocab: &[String],
    png: &[u8],
) -> Vec<(String, f64)> {
    let Some(engine) = engine.as_ref() else {
        return Vec::new();
    };
    if skill_geom.get("n_rows").and_then(Value::as_i64).is_none() {
        return Vec::new();
    }
    let Ok((data, h, w)) = load_bgr_png(png) else {
        return Vec::new();
    };
    let panel = BgrImage { data, h, w };
    let read = |crop: &BgrImage| -> (String, f64) {
        engine
            .recognize_bgr(&crop.data, crop.h, crop.w)
            .unwrap_or_default()
    };
    read_skill_panel(&read, &panel, skill_geom, vocab)
        .into_iter()
        .filter_map(|row| match (row.name, row.level) {
            (Some(name), Some(level)) => Some((name, level)),
            _ => None,
        })
        .collect()
}

/// Build and start the producer spine over the shared pool and clock.
/// The providers are wired faithfully to the backend's own composition
/// (`backend/main.py`): every lookup the tracker consults reads through
/// the same database or the same config read-through the sidecar wrote.
fn compose_producers(
    db: Db,
    clock: Arc<dyn Clock>,
    data_dir: &std::path::Path,
    chatlog_override: Option<PathBuf>,
) -> Result<ProducerState, eo_services::db::DbError> {
    // The producers run on the substrate's tokio runtime; the trackers
    // bridge their database work onto this handle from their own
    // (non-runtime) producer threads, exactly as the sidecar's tracker
    // bridges onto its event loop.
    let runtime = tokio::runtime::Handle::current();
    let bus = Arc::new(EventBus::new());

    // The SSE bridge: forward the frontend-facing domain topics off the
    // in-process bus to the `/api/events` fan-out hub, mirroring the
    // sidecar's `EventStreamHub`. Subscribed here, before the watcher's
    // tail thread starts, so an event published the instant a producer
    // ticks is never raced away from a connected stream. The hub is held
    // in `ProducerState` (and through these subscriber closures), and a
    // clone is handed to the HTTP layer at composition.
    let sse_hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
    subscribe_sse_bridge(&bus, &sse_hub);

    // The config read-through: producers read the live config the same
    // way the read surface does (`settings.json` stays sidecar-written
    // until a later cutover), so a wrong default never silently
    // corrupts tracker DB state. A read failure falls back to the typed
    // defaults, exactly as the backend's loader does.
    let config = load_config_readonly(data_dir).unwrap_or_default();

    // The settings writer the write routes serve over. A corrupt or
    // wrong-shape settings file fails composition loudly (declining native
    // composition so the surface proxies), exactly as the sidecar's loader
    // would crash rather than silently reset user settings.
    let config_service = Arc::new(Mutex::new(
        ConfigService::new(data_dir)
            .map_err(|e| eo_services::db::DbError::Driver(e.to_string()))?,
    ));

    // The quest service is bus-subscribed (session tracking + mission
    // auto-start) and supplies the watcher's quest-reward filter, so a
    // mission completion can suppress its reward echo just as the
    // sidecar's does.
    let quests = Arc::new(QuestService::new(db.pool().clone(), clock.clone()));
    quests.subscribe(&bus, runtime.clone());

    let watched_chatlog = chatlog_override.unwrap_or_else(|| PathBuf::from(&config.chatlog_path));
    let quest_reward_filter = quest_reward_filter_adapter(quests.clone(), runtime.clone());
    let watcher = ChatlogWatcher::new(bus.clone(), watched_chatlog, Some(quest_reward_filter));

    let skill_tracker = SkillTracker::new(&bus, db.pool().clone(), runtime.clone(), clock.clone());

    // The input listeners share ONE OS keyboard hook (it is single-instance):
    // a ref-counted source wraps the low-level hook, filtered at its boundary
    // to the hotbar digit keys and the space key. The hotbar listener is a
    // producer (it publishes tool-change events on the bus), gated on the
    // hotbar-hooks toggle AND an active session; the spacebar listener is
    // composed alongside the scan services over this same source. Off Windows
    // (or in a headless run) the hook is inert, so both listeners never run.
    let allowlist: std::collections::BTreeSet<String> = HOTBAR_SLOT_KEYS
        .iter()
        .map(|key| key.to_string())
        .chain(std::iter::once("space".to_string()))
        .collect();
    let keystroke_source: Arc<dyn KeystrokeSource> = Arc::new(SharedKeystrokeSource::new(
        Arc::new(HookKeystrokeSource::new(Some(allowlist))),
    ));
    let hotbar = HotbarListener::new(
        bus.clone(),
        Some(keystroke_source.clone()),
        Some(build_hotbar_resolver(db.clone(), data_dir, runtime.clone())),
    );
    // Apply the stored toggle; the source still only attaches while a session
    // is active (the listener reconciles on the session bus events).
    hotbar.set_hotbar_hooks_enabled(config.hotbar_hooks_enabled);

    let tracker = HuntTracker::new(
        bus.clone(),
        db.pool().clone(),
        runtime.clone(),
        clock.clone(),
        build_providers(db, data_dir, &config, runtime.clone()),
    )?;

    // Start the tail thread last, after every subscriber is registered,
    // so no published tick can land before the trackers can see it.
    watcher.start();

    Ok(ProducerState {
        watcher,
        tracker,
        sse_hub,
        config_service,
        skill_tracker,
        bus,
        hotbar,
        keystroke_source,
        _quests: quests,
    })
}

/// The hotbar slot resolver, mirroring the backend's `_hotbar_resolver`: the
/// live config maps the slot key to an equipment-library id; the row's item
/// type selects the outcome (a healing tool's per-use cost and reload from its
/// entity, a consumable's zero-cost one-off, or a weapon's per-shot cost
/// looked up by name fragment exactly as the cost provider does). An unbound
/// slot, an absent row, or a read failure yields None (no tool change).
fn build_hotbar_resolver(
    db: Db,
    data_dir: &std::path::Path,
    runtime: tokio::runtime::Handle,
) -> HotbarResolver {
    let data_dir = data_dir.to_path_buf();
    Arc::new(move |slot: &str| {
        let config = load_config_readonly(&data_dir).ok()?;
        let equip_id = config.hotbar.get(slot).and_then(Value::as_i64)?;
        let db = db.clone();
        block_on_pool(&runtime, async move {
            let (name, item_type, properties_json) =
                db.hotbar_equipment_row(equip_id).await.ok().flatten()?;
            let outcome = match item_type.as_str() {
                "healing" => {
                    let (cost_ped, reload_seconds) = heal_cost_from_props(&properties_json);
                    (name, cost_ped, "healing".to_string(), reload_seconds)
                }
                "consumable" => (name, 0.0, "consumable".to_string(), 0.0),
                _ => {
                    let cost = weapon_cost_by_name(&db, &name).await;
                    (name, cost, "weapon".to_string(), 0.0)
                }
            };
            Some(outcome)
        })
    })
}

/// The per-shot weapon cost in PED for a tool by name fragment, mirroring the
/// cost provider's `_equipment_cost_lookup`: `totalCostPerUse / 100`, or 0
/// when the tool is unknown.
async fn weapon_cost_by_name(db: &Db, name: &str) -> f64 {
    match db
        .weapon_properties_by_name_fragment(name)
        .await
        .ok()
        .flatten()
    {
        Some(properties_json) => {
            let props: Value = serde_json::from_str(&properties_json).unwrap_or(Value::Null);
            cost_per_shot_from_props(&props, None)["totalCostPerUse"]
                .as_f64()
                .unwrap_or(0.0)
                / 100.0
        }
        None => 0.0,
    }
}

/// The healing-tool per-use cost (PED) and reload (seconds) from a row's
/// properties, mirroring `_heal_tool_cost_lookup`: a missing or empty tool
/// entity falls back to `(0, 2.5)`; otherwise the cost engine's per-use cost
/// over the entity and its markup, in PED, and the entity's reload.
fn heal_cost_from_props(properties_json: &str) -> (f64, f64) {
    let props: Value = serde_json::from_str(properties_json).unwrap_or(Value::Null);
    let tool = props
        .get("tool_entity")
        .filter(|value| !value.is_null() && value.as_object().is_none_or(|map| !map.is_empty()));
    let Some(tool) = tool else {
        return (0.0, 2.5);
    };
    let markup = props.get("markup").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
    (
        heal_cost_per_use(tool, markup) / 100.0,
        heal_reload_seconds(tool),
    )
}

/// Bridge the producer bus's frontend-facing domain topics onto the SSE
/// fan-out hub, mirroring the sidecar's `EventStreamHub`: the same two
/// topics, the same drop-non-domain-payload contract. Each publish carries
/// the full `DomainEvent` serialised to a `Value` (see the tracker and
/// skill-scan publish sites); the bridge deserialises it back and hands the
/// typed envelope to the hub, which assigns the shared sequence number and
/// frames it. A payload that is not a `DomainEvent` on a domain topic would
/// be an upstream programming error, so it is dropped rather than forwarded
/// as an untyped frame. The subscriptions live in the bus (held alive by
/// the producer spine) and capture hub clones, so they need no separate
/// registration store; they drop with the bus when the spine tears down.
fn subscribe_sse_bridge(bus: &EventBus, hub: &Arc<SseHub>) {
    for topic in [Topic::TrackingSessionUpdated, Topic::ScanStatusChanged] {
        let hub = hub.clone();
        bus.subscribe(topic, move |value| {
            if let Ok(event) = serde_json::from_value::<DomainEvent>(value.clone()) {
                hub.dispatch(&event);
            }
        });
    }
}

/// Adapt the quest service's async reward filter to the watcher's
/// synchronous `QuestRewardFilter` closure: the watcher invokes it from
/// its tail thread, where there is no current runtime, so the closure
/// bridges onto the substrate runtime and parks. A filter error
/// surfaces as no suppression, exactly as the backend contains a filter
/// exception.
fn quest_reward_filter_adapter(
    quests: Arc<QuestService>,
    runtime: tokio::runtime::Handle,
) -> QuestRewardFilter {
    Arc::new(
        move |mission_name: &str, loot_items: &[Value], skill_gains: &[Value]| {
            let mission_name = mission_name.to_string();
            let loot_items = loot_items.to_vec();
            let skill_gains = skill_gains.to_vec();
            let quests = quests.clone();
            let result = if tokio::runtime::Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| {
                    runtime.block_on(async {
                        quests
                            .quest_reward_filter(&mission_name, &loot_items, &skill_gains)
                            .await
                    })
                })
            } else {
                runtime.block_on(async {
                    quests
                        .quest_reward_filter(&mission_name, &loot_items, &skill_gains)
                        .await
                })
            };
            result.unwrap_or(None)
        },
    )
}

/// Wire the hunt-tracker providers to the same sources the backend's
/// composition uses. Lookups that read the equipment library bridge
/// onto the runtime from inside the (synchronous) provider callback;
/// config-derived providers read the live config read-through so they
/// follow sidecar writes. `enhancer_tt_lookup` is intentionally absent:
/// the Rust tracker never reads it (see `tracker.rs`).
fn build_providers(
    db: Db,
    data_dir: &std::path::Path,
    initial_config: &AppConfig,
    runtime: tokio::runtime::Handle,
) -> Providers {
    let data_dir = data_dir.to_path_buf();

    // equipment_profile_lookup: the weapon row whose name contains the
    // tool fragment, as parsed JSON properties.
    let profile_db = db.clone();
    let profile_runtime = runtime.clone();
    let equipment_profile_lookup: Arc<dyn Fn(&str) -> EquipmentProfile + Send + Sync> =
        Arc::new(move |tool_name: &str| {
            let tool_name = tool_name.to_string();
            let db = profile_db.clone();
            let json = block_on_pool(&profile_runtime, async move {
                db.weapon_properties_by_name_fragment(&tool_name)
                    .await
                    .ok()
                    .flatten()
            })?;
            match serde_json::from_str::<Value>(&json) {
                Ok(Value::Object(map)) => Some(map),
                _ => None,
            }
        });

    // equipment_cost_lookup: the per-shot cost in PED derived from the
    // profile, `totalCostPerUse / 100`, or 0.0 when the tool is unknown.
    let cost_lookup_profile = equipment_profile_lookup.clone();
    let equipment_cost_lookup: Arc<dyn Fn(&str) -> f64 + Send + Sync> = Arc::new(
        move |tool_name: &str| match cost_lookup_profile(tool_name) {
            Some(props) => {
                let cost = cost_per_shot_from_props(&Value::Object(props), None);
                cost["totalCostPerUse"].as_f64().unwrap_or(0.0) / 100.0
            }
            None => 0.0,
        },
    );

    // The config-derived providers read the live read-through so they
    // follow the sidecar's writes between sessions.
    let mode_dir = data_dir.clone();
    let mob_tracking_mode: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
        load_config_readonly(&mode_dir)
            .map(|c| c.mob_tracking_mode)
            .unwrap_or_else(|_| "mob".to_string())
    });
    let tag_dir = data_dir.clone();
    let mob_tracking_tag: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
        load_config_readonly(&tag_dir)
            .map(|c| c.mob_tracking_tag)
            .unwrap_or_default()
    });
    let manual_enabled_dir = data_dir.clone();
    let manual_mob_entry_enabled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        load_config_readonly(&manual_enabled_dir)
            .map(|c| c.mob_tracking_mode != "tag")
            .unwrap_or(true)
    });
    let manual_mob_dir = data_dir.clone();
    let manual_mob: Arc<dyn Fn() -> Option<(String, String)> + Send + Sync> = Arc::new(move || {
        let config = load_config_readonly(&manual_mob_dir).ok()?;
        let species = config.manual_mob_species.trim().to_string();
        let maturity = config.manual_mob_maturity.trim().to_string();
        if species.is_empty() {
            return None;
        }
        Some((species, maturity))
    });
    let trifecta_mode_dir = data_dir.clone();
    let weapon_attribution_trifecta: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        // `not hotbar_hooks_enabled`, exactly as the backend's
        // `_is_weapon_attribution_trifecta`.
        load_config_readonly(&trifecta_mode_dir)
            .map(|c| !c.hotbar_hooks_enabled)
            .unwrap_or(false)
    });
    let blacklist_dir = data_dir.clone();
    let loot_filter_blacklist_provider: Arc<dyn Fn() -> Vec<String> + Send + Sync> =
        Arc::new(move || {
            load_config_readonly(&blacklist_dir)
                .map(|c| c.loot_filter_blacklist)
                .unwrap_or_default()
        });

    // trifecta_resolver: resolve the active preset's attribution map
    // off the live config and the equipment library (the backend's
    // `_resolve_trifecta`); the resolver discards the validation reason
    // and yields just the data, as the backend does.
    let resolver_db = db;
    let resolver_dir = data_dir;
    let resolver_runtime = runtime;
    let trifecta_resolver: Arc<dyn Fn() -> Option<Map<String, Value>> + Send + Sync> =
        Arc::new(move || {
            let config = load_config_readonly(&resolver_dir).ok()?;
            let preset = active_trifecta_preset(&config).map(|p| TrifectaPreset {
                small_weapon_id: p.small_weapon_id,
                big_weapon_id: p.big_weapon_id,
                heal_id: p.heal_id,
            });
            let db = resolver_db.clone();
            block_on_pool(&resolver_runtime, async move {
                describe_trifecta(&db, preset.as_ref())
                    .await
                    .ok()
                    .and_then(|(data, _error)| data)
            })
        });

    Providers {
        equipment_cost_lookup,
        equipment_profile_lookup,
        player_name: initial_config.player_name.clone(),
        loot_filter_blacklist: initial_config.loot_filter_blacklist.clone(),
        loot_filter_blacklist_provider: Some(loot_filter_blacklist_provider),
        weapon_attribution_trifecta,
        mob_tracking_mode,
        mob_tracking_tag,
        manual_mob_entry_enabled,
        manual_mob,
        trifecta_resolver,
    }
}

/// Run a database future from inside a synchronous provider callback,
/// from either calling context: a runtime worker thread (an HTTP-driven
/// reload) yields its slot, while a plain producer thread parks. The
/// tracker's own `block_on` uses this exact dual shape.
fn block_on_pool<F: std::future::Future>(handle: &tokio::runtime::Handle, future: F) -> F::Output {
    // Never `Handle::current()`: the provider callbacks run on the
    // chat-log watcher's plain OS thread (no current runtime), so the
    // handle is the one captured at composition time. A runtime worker
    // thread (an HTTP-driven reload) yields its slot via `block_in_place`;
    // a plain producer thread parks directly. This mirrors the tracker's
    // own `block_on` and the quest-reward-filter adapter.
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        handle.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that mutate process-global, not-thread-safe
    /// state: `ORT_DYLIB_PATH` (the env var ORT reads via `getenv`) and the
    /// current working directory (`set_current_dir`). Async-aware so the
    /// guard can be held across the `compose_with(...).await` that builds
    /// the engine and reads the env, without the `await_holding_lock`
    /// hazard a `std::sync::Mutex` would raise. Only ever contended across
    /// distinct tests, so no intra-test deadlock.
    static ORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn repo_snapshot() -> PathBuf {
        dev_project_root()
            .join("backend")
            .join("data")
            .join("snapshot")
    }

    /// The repo's recogniser model+dict directory (the dev `models_dir`).
    fn repo_models() -> PathBuf {
        dev_project_root()
            .join("backend")
            .join("assets")
            .join("models")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn composes_over_a_fresh_data_dir_and_the_repo_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let Composition::Ready(composed) =
            compose_with(dir.path().join("data"), repo_snapshot(), repo_models()).await
        else {
            panic!("fresh-dir composition succeeds");
        };
        assert!(
            dir.path().join("data").join(DB_FILE_NAME).exists(),
            "the database file is created at the resolved location"
        );
        // The producer spine composed alongside the read surface; tear
        // it down so no tail thread outlives the test.
        composed.producers.stop();
    }

    #[tokio::test]
    async fn declines_on_a_quarantined_database_leaving_the_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join(DB_FILE_NAME);
        std::fs::write(&db_path, b"not a database").unwrap();
        let composed = compose_with(data_dir, repo_snapshot(), repo_models()).await;
        assert!(
            matches!(composed, Composition::Declined),
            "quarantine declines composition"
        );
        assert_eq!(std::fs::read(&db_path).unwrap(), b"not a database");
    }

    #[tokio::test]
    async fn awaits_migration_on_a_below_baseline_database() {
        // Seed a database the backend created but has not yet migrated up to
        // the baseline (the first-launch-after-upgrade state). Composition
        // must report AwaitingMigration (retry once the sidecar migrates it)
        // rather than Declined (give up proxy-only for the whole session).
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join(DB_FILE_NAME);
        {
            let db = eo_services::db::Db::open(&db_path).await.unwrap();
            sqlx::query("UPDATE db_metadata SET value = '28' WHERE key = 'version'")
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query("DROP TABLE _sqlx_migrations")
                .execute(db.pool())
                .await
                .unwrap();
        }
        let composed = compose_with(data_dir, repo_snapshot(), repo_models()).await;
        assert!(
            matches!(composed, Composition::AwaitingMigration),
            "a below-baseline database awaits the sidecar's migration"
        );
    }

    #[tokio::test]
    async fn declines_on_a_missing_snapshot_dir() {
        let dir = tempfile::tempdir().unwrap();
        let composed = compose_with(
            dir.path().join("data"),
            dir.path().join("no-such-snapshot"),
            repo_models(),
        )
        .await;
        assert!(
            matches!(composed, Composition::Declined),
            "missing snapshot declines composition"
        );
    }

    #[test]
    fn snapshot_dir_prefers_the_repo_copy_in_dev_builds() {
        let resolved = snapshot_dir(Some(&PathBuf::from("X:/resources")));
        if cfg!(debug_assertions) {
            assert_eq!(resolved, repo_snapshot());
        } else {
            assert_eq!(resolved, PathBuf::from("X:/resources").join("snapshot"));
        }
    }

    /// DYLIB RESOLUTION: the pinned `onnxruntime.dll` path is always
    /// ABSOLUTE (never a bare filename the OS loader would resolve off
    /// `PATH`/CWD), resolves to the expected installed-vs-dev location,
    /// and does NOT depend on the process working directory.
    ///
    /// The CWD-independence is the load-bearing property: a bare name or a
    /// relative path would let a stray system `onnxruntime.dll` win, so
    /// the resolver must return the same absolute path regardless of where
    /// the process runs. We assert it directly (the resolver composes only
    /// the installed `resource_dir` and the compiled-in `dev_project_root`,
    /// neither of which reads CWD) and prove it by resolving, changing the
    /// CWD, resolving again, and asserting the two are byte-identical.
    #[tokio::test]
    async fn ort_dylib_path_is_absolute_and_cwd_independent() {
        // Serialised: this test mutates the process-global CWD, which would
        // race any concurrent test reading the CWD or a relative path.
        let _ort = ORT_TEST_LOCK.lock().await;
        let resource = PathBuf::from("X:/resources");
        let resolved = ort_dylib_path(Some(&resource));

        // Always absolute, never a bare filename.
        assert!(
            resolved.is_absolute(),
            "the pinned dylib path must be absolute, got {}",
            resolved.display()
        );
        assert!(
            resolved.components().count() > 1,
            "the pinned dylib path is never a bare filename, got {}",
            resolved.display()
        );
        assert_eq!(
            resolved.file_name().and_then(|n| n.to_str()),
            Some("onnxruntime.dll"),
            "the pinned path points at onnxruntime.dll"
        );

        // The expected installed-vs-dev location.
        if cfg!(debug_assertions) {
            assert_eq!(
                resolved,
                dev_project_root()
                    .join("frontend")
                    .join("src-tauri")
                    .join("entropia-orme")
                    .join("resources")
                    .join("ort")
                    .join("onnxruntime.dll"),
                "dev resolves to the committed repo dylib"
            );
        } else {
            assert_eq!(
                resolved,
                resource.join("ort").join("onnxruntime.dll"),
                "installed resolves under the OS-given resource dir"
            );
        }

        // CWD-independence: resolving from a different working directory
        // yields the identical absolute path. (The dev branch ignores
        // `resource` and resolves through the compiled-in project root;
        // both branches are pure-path joins, so neither reads CWD.)
        let original_cwd = std::env::current_dir().expect("a current dir");
        let elsewhere = tempfile::tempdir().expect("temp dir");
        std::env::set_current_dir(elsewhere.path()).expect("chdir");
        let resolved_elsewhere = ort_dylib_path(Some(&resource));
        // Restore before any assertion so a failure never leaves the
        // process in the temp dir for the next test.
        std::env::set_current_dir(&original_cwd).expect("restore chdir");
        assert_eq!(
            resolved, resolved_elsewhere,
            "the resolved dylib path is identical regardless of the working directory"
        );
    }

    /// DYLIB PIN (the BLOCKER guard, on the PRODUCTION init path): pinning
    /// `ORT_DYLIB_PATH` to the absolute bundled path is what stops ort's
    /// fallback loader from ever resolving a bare `onnxruntime.dll` off the
    /// OS search order (a DLL-planting vector) when `init_from` fails. The
    /// production path (`compose_native` -> `init_ort_runtime`) does this;
    /// the engine tests use the env shortcut and never exercised it, the
    /// same production-vs-test-path gap that hid the original defect. Assert
    /// the pin sets the env to the absolute path, never a bare name. (Every
    /// writer in the suite pins the same dev path, so the shared-env set is
    /// idempotent.)
    #[tokio::test]
    async fn pin_writes_the_absolute_dylib_path_to_the_env_never_a_bare_name() {
        let _ort = ORT_TEST_LOCK.lock().await;
        let pinned = pin_ort_dylib_env(None);
        assert!(
            pinned.is_absolute(),
            "the pinned dylib path must be absolute, got {}",
            pinned.display()
        );
        assert_ne!(
            pinned.as_os_str(),
            std::ffi::OsStr::new("onnxruntime.dll"),
            "the pin must never be a bare filename the OS loader resolves off PATH/CWD"
        );
        assert_eq!(
            std::env::var("ORT_DYLIB_PATH").ok(),
            Some(pinned.to_string_lossy().into_owned()),
            "init pins ORT_DYLIB_PATH to the resolved absolute dylib"
        );
    }

    /// The models directory resolves the installed-vs-dev way, the same
    /// shape `snapshot_dir` and the dylib resolver use, so the engine
    /// reads the bundled model+dict and never a stray copy.
    #[test]
    fn models_dir_prefers_the_repo_copy_in_dev_builds() {
        let resolved = models_dir(Some(&PathBuf::from("X:/resources")));
        if cfg!(debug_assertions) {
            assert_eq!(resolved, repo_models());
        } else {
            assert_eq!(resolved, PathBuf::from("X:/resources").join("models"));
        }
    }

    /// COMPOSITION WITH A WARMED ENGINE: composing over the repo model
    /// yields a warmed engine reachable from `Composed.ocr_engine` when
    /// the ONNX Runtime is loadable on this host. Host-gated: the engine
    /// binds the runtime dynamically, so on a host without the dylib
    /// loadable the engine is `None` and the rest of composition still
    /// succeeds (OCR is optional) - we assert composition succeeded
    /// either way and that, WHEN the engine is present, it recorded a real
    /// provider and recognises a warm-up-shaped cell.
    ///
    /// The dylib is pinned via `ORT_DYLIB_PATH` (the committed repo copy)
    /// so the test can load the runtime without a system install; if the
    /// dylib is absent on this host the test skips with its reason.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn composes_with_a_warmed_ocr_engine_when_the_runtime_loads() {
        let _ort = ORT_TEST_LOCK.lock().await;
        let dylib = ort_dylib_path(None);
        if !dylib.is_file() {
            eprintln!(
                "committed ONNX Runtime dylib absent at {}; skipping",
                dylib.display()
            );
            return;
        }
        // SAFETY: set before any ORT use in this process. Process-global
        // and once-only, matching the production pin.
        unsafe {
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
        }

        let dir = tempfile::tempdir().unwrap();
        let Composition::Ready(composed) =
            compose_with(dir.path().join("data"), repo_snapshot(), repo_models()).await
        else {
            panic!("composition succeeds regardless of OCR availability");
        };

        match &composed.ocr_engine {
            Some(engine) => {
                let provider = engine.provider();
                assert!(
                    provider == "DmlExecutionProvider" || provider == "CPUExecutionProvider",
                    "the composed engine recorded a real provider, got {provider:?}"
                );
                // The engine is genuinely live (warmed at composition):
                // a white cell recognises without panicking.
                let white = vec![255u8; 48 * 200 * 3];
                let (_text, score) = engine
                    .recognize_bgr(&white, 48, 200)
                    .expect("the composed engine recognises a warm-up-shaped cell");
                assert!(score.is_finite(), "the score is finite, got {score}");
                eprintln!("composed OCR engine provider={provider}");
            }
            None => {
                eprintln!(
                    "OCR engine did not load on this host (runtime/model unavailable); \
                     composition still succeeded, which is the optional-faculty contract"
                );
            }
        }

        composed.producers.stop();
    }

    /// The composed scan services are reachable and wired over the LIVE
    /// providers, not the inert defaults: the manual scan reports a resting
    /// idle status whose `configured` flag mirrors whether the OCR engine
    /// loaded (the inert default's `engine_available` is always false, so a
    /// host where the engine loads proves the real provider), and the repair
    /// reader runs its provider chain to the window-not-found leg. The scan
    /// also publishes onto the spine bus: a status-moving verb dispatches one
    /// `scan.status.changed` frame through the SSE bridge composed alongside.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn composes_the_scan_services_over_live_providers() {
        let _ort = ORT_TEST_LOCK.lock().await;
        // Pin the repo dylib when present so the engine can load on a capable
        // host; absent, the services still compose with the engine `None`.
        let dylib = ort_dylib_path(None);
        if dylib.is_file() {
            unsafe {
                std::env::set_var("ORT_DYLIB_PATH", &dylib);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let Composition::Ready(composed) =
            compose_with(dir.path().join("data"), repo_snapshot(), repo_models()).await
        else {
            panic!("composition succeeds");
        };

        let status = composed.skill_scan.get_status();
        assert_eq!(status["phase"], "idle");
        assert_eq!(status["captured_pages"], 0);
        // `configured` mirrors whether the engine loaded (the real provider);
        // the game window is never present on a headless host.
        assert_eq!(status["configured"], composed.ocr_engine.is_some());
        assert_eq!(status["game_window_present"], false);

        // The repair reader runs its composed provider chain to the
        // no-window leg (its region lookup reads the live game window).
        let repair = composed.repair_ocr.scan_repair_cost();
        assert_eq!(
            repair["error"],
            "Entropia Universe window not found: start the game first"
        );
        // The scan composed on the spine bus (its status frames reach the SSE
        // stream); the bridge forwarding is covered by `sse_bridge_*`.

        composed.producers.stop();
    }

    use std::io::Write as _;
    use std::time::Duration;

    use eo_services::clock::MockClock;

    /// Build the producer spine alone over an injected clock, pool, and
    /// explicit chat-log, mirroring `compose_with`'s producer step. The
    /// integration tests drive a deterministic replay through this spine
    /// without the data-dir/snapshot resolution `compose_native` does.
    fn compose_producers_for_test(
        db: Db,
        clock: Arc<dyn Clock>,
        data_dir: &std::path::Path,
        chatlog: PathBuf,
    ) -> ProducerState {
        compose_producers(db, clock, data_dir, Some(chatlog)).expect("producer spine composes")
    }

    /// A composed-spine replay: feed a recorded-shape chat-log through
    /// the *composed* watcher (shared bus, shared single-owner pool,
    /// injected clock) and assert the composed hunt tracker persisted
    /// the expected session and kill rows. This proves the real-provider
    /// wiring and the shared bus/clock/single-Db composition preserve the
    /// pipeline; it does not claim byte-identical parity with the
    /// default-provider corpus goldens (the real providers stamp mobs and
    /// blacklist loot, which the inert defaults do not).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn composed_spine_replays_a_scenario_into_the_expected_db_rows() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        // Open the database the same single-owner way the substrate does.
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let pool = db.pool().clone();

        // A frozen, plan-advanced clock, exactly the corpus oracle's
        // protocol: the watcher guards its own drain timeout against a
        // frozen clock internally.
        let start =
            chrono::NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Some(start), 0.0));

        let chatlog = data_dir.join("chat_replay.log");
        std::fs::File::create(&chatlog).expect("empty chatlog exists before the watcher starts");

        let producers = compose_producers_for_test(
            Db::from_pool(pool.clone()),
            clock.clone(),
            &data_dir,
            chatlog.clone(),
        );
        producers
            .tracker()
            .start_session()
            .expect("composed session starts");

        // Three lines across three ticks: a combat tick, then two loot
        // ticks that each close a kill.
        let appended = 3u64;
        {
            let mut sink = std::fs::OpenOptions::new()
                .append(true)
                .open(&chatlog)
                .expect("chatlog append");
            // One flush per tick so the tail never sees EOF mid-tick.
            sink.write_all(
                b"2026-05-19 10:00:01 [System] [] You inflicted 12.0 points of damage\n",
            )
            .unwrap();
            sink.flush().unwrap();
            sink.write_all(
                b"2026-05-19 10:00:02 [System] [] You received Shrapnel x (500) Value: 5.00 PED\n",
            )
            .unwrap();
            sink.flush().unwrap();
            sink.write_all(b"2026-05-19 10:00:03 [System] [] You received Wool Value: 1.50 PED\n")
                .unwrap();
            sink.flush().unwrap();
        }
        producers
            .watcher()
            .wait_until_drained(appended, Duration::from_secs(10))
            .expect("composed watcher drains the scenario");

        // A snapshot proves the live tracker accumulated through the
        // composed bus.
        let readout = producers.tracker().snapshot().expect("snapshot");
        let active = readout.active.expect("a session is active");
        assert_eq!(active.kill_count, 2, "two loot groups, two kills");

        producers.tracker().stop_session().expect("session stops");
        producers.stop();

        // The persisted rows match: one session, two kills.
        let session_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(session_count, 1, "one closed session persisted");
        let kill_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kills")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kill_count, 2, "two kills persisted by the composed tracker");
    }

    /// `ProducerState::stop` must end any session left open: it is the
    /// exit seam's one chance to close the session cleanly while the bus
    /// is still live. Start a session through the composed tracker, then
    /// call `stop()` WITHOUT a prior `stop_session()`, and assert the
    /// open session was ended (its `tracking_sessions` row flipped to
    /// `is_active = 0` with an `ended_at` stamp). Replacing `stop`'s body
    /// with `()` would leave the session open, so this assertion fails:
    /// the mutant is killed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_ends_an_open_session_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let pool = db.pool().clone();

        let start =
            chrono::NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Some(start), 0.0));

        let chatlog = data_dir.join("chat_replay.log");
        std::fs::File::create(&chatlog).expect("empty chatlog exists before the watcher starts");

        let producers = compose_producers_for_test(
            Db::from_pool(pool.clone()),
            clock.clone(),
            &data_dir,
            chatlog.clone(),
        );
        producers
            .tracker()
            .start_session()
            .expect("composed session starts");

        // Before stop: exactly one active session row exists.
        let active_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(active_before, 1, "a session is open before stop");
        assert!(
            producers.tracker().is_tracking(),
            "the tracker reports an active session before stop"
        );

        // Stop the spine WITHOUT a prior stop_session: stop() itself must
        // end the open session. A stop body replaced with () leaves the
        // session open and fails the assertions below.
        producers.stop();

        let active_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(active_after, 0, "stop() ended the open session");
        let ended_set: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 0 AND ended_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ended_set, 1, "the closed session carries an ended_at stamp");
    }

    /// Seed a weapon row directly through the shared pool, the same shape
    /// `Db::weapon_properties_by_name_fragment` reads (item_type 'weapon',
    /// a name carrying the lookup fragment, a JSON-object properties blob).
    async fn seed_weapon(pool: &sqlx::SqlitePool, id: i64, name: &str, properties_json: &str) {
        sqlx::query(
            "INSERT INTO equipment_library (id, name, item_type, properties_json) \
             VALUES (?, ?, 'weapon', ?)",
        )
        .bind(id)
        .bind(name)
        .bind(properties_json)
        .execute(pool)
        .await
        .expect("weapon row seeds");
    }

    /// Write a minimal `settings.json` carrying just the keys a test
    /// pins; the read-through loader reads any JSON object and defaults
    /// the rest, so a partial object is enough to exercise the
    /// config-derived providers' live reads.
    fn write_settings(data_dir: &std::path::Path, settings: &Value) {
        std::fs::write(
            data_dir.join("settings.json"),
            serde_json::to_string(settings).unwrap(),
        )
        .expect("settings.json writes");
    }

    /// The `build_providers` transforms, each invoked against on-disk
    /// fixtures so a mutation to any one transform is observable:
    ///
    /// - equipment_profile_lookup returns the parsed property object for a
    ///   weapon matched by name fragment (kills the deleted `Ok(Object)`
    ///   match arm and the `Default::default()` whole-function replacement,
    ///   both of which yield `None`).
    /// - equipment_cost_lookup returns `totalCostPerUse / 100`; the seeded
    ///   props make totalCostPerUse == 250 so the expected 2.5 differs from
    ///   both `250 % 100` (50.0) and `250 * 100` (25000.0): kills the `/`->`%`
    ///   and `/`->`*` mutants.
    /// - manual_mob_entry_enabled is `mob_tracking_mode != "tag"`: false
    ///   under "tag", true under "mob" (kills `!=`->`==`, which flips both).
    /// - weapon_attribution_trifecta is `!hotbar_hooks_enabled`: false when
    ///   hooks are on, true when off (kills the deleted `!`, which flips both).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_providers_transforms_pin_their_exact_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let pool = db.pool().clone();

        // A weapon whose name carries the fragment "Korss", with a
        // property object whose economy yields a known totalCostPerUse.
        // ammo_burn 25000 -> 250 PEC ammo at markup 1.0, decay 0 ->
        // totalCostPerUse == 250, so equipment_cost_lookup == 250/100 == 2.5.
        let props = serde_json::json!({
            "weapon_entity": {"economy": {"decay": 0.0, "ammo_burn": 25000}},
            "weapon_markup": 100,
        });
        seed_weapon(
            &pool,
            1,
            "Korss H400 (L)",
            &serde_json::to_string(&props).unwrap(),
        )
        .await;

        // First on-disk config: tag mode, hotbar hooks ENABLED.
        write_settings(
            &data_dir,
            &serde_json::json!({
                "mob_tracking_mode": "tag",
                "hotbar_hooks_enabled": true,
            }),
        );
        let config = load_config_readonly(&data_dir).expect("config reads");
        let providers = build_providers(
            Db::from_pool(pool.clone()),
            &data_dir,
            &config,
            tokio::runtime::Handle::current(),
        );

        // equipment_profile_lookup: the parsed property object, by fragment.
        let profile = (providers.equipment_profile_lookup)("Korss")
            .expect("the seeded weapon resolves to a property object");
        assert_eq!(
            profile.get("weapon_markup").and_then(Value::as_f64),
            Some(100.0),
            "the profile carries the seeded keys"
        );
        assert!(
            profile.contains_key("weapon_entity"),
            "the profile carries the weapon entity"
        );

        // equipment_cost_lookup: totalCostPerUse (250) / 100 == 2.5.
        let cost = (providers.equipment_cost_lookup)("Korss");
        assert!(
            (cost - 2.5).abs() < 1e-9,
            "per-shot cost is totalCostPerUse/100 == 2.5, not {cost} \
             (% would be 50.0, * would be 25000.0)"
        );

        // Config-derived providers read live: under tag mode + hooks-on,
        // manual entry is disabled and trifecta attribution is off.
        assert!(
            !(providers.manual_mob_entry_enabled)(),
            "manual mob entry is disabled in tag mode"
        );
        assert!(
            !(providers.weapon_attribution_trifecta)(),
            "trifecta attribution is off when hotbar hooks are enabled"
        );

        // Rewrite settings.json and re-invoke the SAME closures: they read
        // live, so the flipped config flips both booleans. mob mode +
        // hooks-off -> manual entry enabled, trifecta attribution on.
        write_settings(
            &data_dir,
            &serde_json::json!({
                "mob_tracking_mode": "mob",
                "hotbar_hooks_enabled": false,
            }),
        );
        assert!(
            (providers.manual_mob_entry_enabled)(),
            "manual mob entry is enabled outside tag mode"
        );
        assert!(
            (providers.weapon_attribution_trifecta)(),
            "trifecta attribution is on when hotbar hooks are disabled"
        );
    }

    /// REGRESSION: the equipment provider runs on the chat-log watcher's
    /// plain OS thread, which has NO current tokio runtime (the bus
    /// dispatches synchronously on the watcher's tail thread). The
    /// provider's `block_on_pool` must therefore use the runtime handle
    /// captured at composition time, never `Handle::current()` (which
    /// panics off-runtime). Build the providers inside the runtime, then
    /// invoke the lookup from a plain `std::thread` with no runtime
    /// context and assert it resolves rather than panicking.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn equipment_provider_resolves_from_a_non_runtime_thread() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let pool = db.pool().clone();
        let props = serde_json::json!({
            "weapon_entity": {"economy": {"decay": 0.0, "ammo_burn": 25000}},
            "weapon_markup": 100,
        });
        seed_weapon(
            &pool,
            1,
            "Korss H400 (L)",
            &serde_json::to_string(&props).unwrap(),
        )
        .await;
        write_settings(&data_dir, &serde_json::json!({}));
        let config = load_config_readonly(&data_dir).expect("config reads");
        let providers = build_providers(
            Db::from_pool(pool),
            &data_dir,
            &config,
            tokio::runtime::Handle::current(),
        );

        // Invoke the lookup AND the derived cost from a plain OS thread
        // (no current runtime), exactly the watcher tail-thread context.
        let profile_lookup = providers.equipment_profile_lookup.clone();
        let cost_lookup = providers.equipment_cost_lookup.clone();
        let outcome = std::thread::spawn(move || {
            let resolved = profile_lookup("Korss").is_some();
            let cost = cost_lookup("Korss");
            (resolved, cost)
        })
        .join()
        .expect("the provider must not panic off-runtime");
        assert!(
            outcome.0,
            "equipment_profile_lookup resolves from a non-runtime thread"
        );
        assert!(
            (outcome.1 - 2.5).abs() < 1e-9,
            "equipment_cost_lookup resolves off-runtime to 2.5, got {}",
            outcome.1
        );
    }

    /// The single-owner pool tolerates an HTTP-shaped read concurrent
    /// with a producer-shaped write without deadlock and within a bounded
    /// latency: sqlx serialises both through the one connection, so the
    /// read simply queues behind the write rather than locking.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_read_during_a_producer_write_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_adopted(&dir.path().join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let pool = db.pool().clone();

        // A producer-shaped write: a short transaction holding the single
        // connection briefly, exactly the shape of a tracker persistence
        // write.
        let write_pool = pool.clone();
        let writer = tokio::spawn(async move {
            let mut tx = write_pool.begin().await.expect("begin");
            sqlx::query(
                "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('cc-test', 0, 0)",
            )
            .execute(&mut *tx)
            .await
            .expect("insert under tx");
            // Hold the connection a moment so the read genuinely contends.
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.commit().await.expect("commit");
        });

        // An HTTP-shaped read on the same pool, bounded by a generous
        // deadline: if the single connection deadlocked, this would
        // time out.
        let read = tokio::time::timeout(Duration::from_secs(5), async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracking_sessions")
                .fetch_one(&pool)
                .await
                .expect("read query")
        })
        .await;
        assert!(
            read.is_ok(),
            "the concurrent read completed without deadlock"
        );

        writer.await.expect("writer task joins");
        let final_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracking_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(final_count, 1, "the write committed");
    }

    #[tokio::test]
    async fn sse_bridge_forwards_domain_events_to_a_hub_client() {
        use eo_wire::domain_events::{
            ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
            TrackingReason, TrackingSessionUpdated, TrackingSessionUpdatedPayload,
            TrackingSessionUpdatedTag, TrackingStatus,
        };

        let bus = EventBus::new();
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        subscribe_sse_bridge(&bus, &hub);
        let client = hub.register();

        // A tracking event published on the bus (the full DomainEvent
        // serialised, exactly as the tracker publishes it) is reframed by
        // the hub and delivered to the client.
        let tracking = DomainEvent::TrackingSessionUpdated(TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: "2026-01-01T00:00:00Z".to_string(),
            payload: TrackingSessionUpdatedPayload {
                session_id: Some("s1".to_string()),
                status: TrackingStatus::Active,
                reason: TrackingReason::Started,
            },
        });
        bus.publish(
            Topic::TrackingSessionUpdated,
            &serde_json::to_value(&tracking).unwrap(),
        );
        assert_eq!(
            client.next_frame().await,
            format!(
                "id: 1\nevent: tracking.session.updated\ndata: {}\n\n",
                tracking.to_wire_json()
            )
        );

        // The other bridged topic is delivered too, sharing the hub's
        // process-monotonic sequence.
        let scan = DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2026-01-01T00:00:01Z".to_string(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Capturing,
            },
        });
        bus.publish(
            Topic::ScanStatusChanged,
            &serde_json::to_value(&scan).unwrap(),
        );
        assert_eq!(
            client.next_frame().await,
            format!(
                "id: 2\nevent: scan.status.changed\ndata: {}\n\n",
                scan.to_wire_json()
            )
        );
    }

    #[tokio::test]
    async fn sse_bridge_drops_a_non_domain_payload() {
        let bus = EventBus::new();
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        subscribe_sse_bridge(&bus, &hub);
        let client = hub.register();

        // A payload that does not deserialise to a DomainEvent on a domain
        // topic is an upstream programming error: the bridge drops it rather
        // than forward an untyped frame, so no frame ever reaches the client.
        bus.publish(
            Topic::TrackingSessionUpdated,
            &serde_json::json!({"unexpected": "shape"}),
        );
        let delivered = tokio::time::timeout(Duration::from_millis(100), client.next_frame()).await;
        assert!(delivered.is_err(), "a non-domain payload yields no frame");
    }

    /// The LIVE scan-status path now the scan composes on the spine bus: a
    /// status-moving verb on a `SkillScanManual` built over the same bus the
    /// bridge subscribes reaches an SSE client as a `scan.status.changed`
    /// frame, end to end (the bridge-forwards test publishes the event
    /// directly; this proves the scan's own publish flows through it).
    #[tokio::test]
    async fn the_composed_scan_delivers_a_status_frame_to_an_sse_client() {
        let bus = Arc::new(EventBus::new());
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        subscribe_sse_bridge(&bus, &hub);
        let client = hub.register();

        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let scan = SkillScanManual::new(
            ScanProviders {
                engine_available: Arc::new(|| true),
                skill_region: Arc::new(|| Some(([0, 0], [100, 200]))),
                capture_region: Arc::new(|_| Some(vec![1, 2, 3])),
                extract_page_levels: Arc::new(|_: &[u8]| Vec::new()),
            },
            clock,
            Some(bus.clone()),
            None,
            0,
        );
        // `start` moves the status idle -> capturing, publishing one frame.
        scan.start(Some(2));
        let frame = client.next_frame().await;
        assert!(
            frame.contains("event: scan.status.changed"),
            "the scan's status publish reaches the stream: {frame}"
        );
        assert!(
            frame.contains("capturing"),
            "the frame carries the moved phase: {frame}"
        );
    }
}
