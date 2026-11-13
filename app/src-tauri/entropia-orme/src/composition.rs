//! The native-services composition root.
//!
//! Startup composition: resolve the data directory, open the
//! application database, load the game-data snapshot, and construct
//! the services over the real clock. The facade serves every typed
//! command through the state composed here; when any step declines, composition is terminal and the
//! backend does not come up for the session (there is no upstream to fall
//! back to).
//!
//! Composition assembles the full native surface: the hydration read
//! surface, the producer spine, and the OCR recogniser with its ONNX
//! Runtime obligations, all built here ahead of the operations that consume
//! them.
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
//!    (the platform's GPU provider preferred, CPU fallback), not on the
//!    global env, so the engine owns its full session config. The env
//!    here carries no EPs.
//!
//! Two deliberate divergences from the original (`local_ocr.py`),
//! recorded so a later reviewer does not read them as oversights:
//!
//! * **Eager warm-up at composition, not lazy on first use.** The
//!   original warms the engine on the first `get_engine()`; we warm it at
//!   startup so the first real scan never eats the GPU provider's shader
//!   compilation. The warm-up runs a synchronous, potentially multi-
//!   second inference, so it is offloaded onto a blocking thread
//!   ([`tokio::task::spawn_blocking`]) rather than stalling the
//!   substrate's async runtime worker during startup.
//! * **No queried provider string.** The original records
//!   `session.get_providers()[0]`; this ort version has no per-session
//!   provider readout, so the engine derives the provider from its
//!   construction control flow (the GPU-then-CPU attempt) instead.
//!   The behaviour (GPU-preferred with CPU fallback) is faithful;
//!   only the readback mechanism differs.
//!
//! A failed ORT init or engine load never declines composition: OCR is
//! one optional faculty, so the read surface and producer spine compose
//! regardless and the engine sits `None` (exactly as
//! `local_ocr.get_engine()` returns `None`, with the consumer seams
//! defaulting to unavailable until a rebuilt engine flips them).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eo_services::attack_rate::with_attack_rate;
use eo_services::bus_events::BusEvent;
use eo_services::bus_events::HotbarItemKind;
use eo_services::character_calc::all_profession_levels;
use eo_services::chatlog_time::ChatLogClock;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::{Clock, RealClock};
use eo_services::config_service::{load_config_readonly, AppConfig, ConfigReader, ConfigService};
use eo_services::consumables::{
    consumable_profile_from_props, on_use_effect_from_props, DoseBoard,
};
use eo_services::db::{AdoptError, Db};
use eo_services::equipment_pricing::{
    cost_per_shot_ped, heal_cost_from_props, healing_profile_from_props, hotbar_equipment_row_sync,
    lifesteal_percent_from_props, lifesteal_percent_from_props_with_catalog, weapon_cost_by_name,
};
use eo_services::eu_window;
use eo_services::event_bus::{EventBus, Topic};
use eo_services::expected_hunting::{with_current_offensive_efficiencies, HuntingLooterLevels};
use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::geometry::BgrImage;
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::sale_queue::ItemCatalogue;
use eo_services::game_ui::seams::{FrameReader, GameWindowFrame};
use eo_services::game_ui::skills::SkillVocabulary;
use eo_services::hotbar_listener::{
    GameFocusProbe, HotbarListener, HotbarResolver, ItemResolver, ResolvedHotbarItem,
    HOTBAR_SLOT_KEYS,
};
use eo_services::keystroke_source::{HookKeystrokeSource, KeystrokeSource, SharedKeystrokeSource};
use eo_services::mob_watch::{
    CatalogueCell, DetectorSlot, MobDetector, MobWatch, MobWatchProviders,
};
pub use eo_services::ocr_engine::OcrEngine;
use eo_services::passive_effects::reload_seconds_under;
use eo_services::paths::{resolve_data_dir, DB_FILE_NAME};
use eo_services::quests::QuestService;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders};
use eo_services::sale_capture::{SaleCaptureProviders, SaleCaptureService};
use eo_services::scan_completion::{complete_skill_scan, latest_skill_levels, ScannedLevel};
pub use eo_services::skill_scan::SkillScanService;
use eo_services::skill_scan::{
    CompletionCallback, SkillScanProviders, DEFAULT_IDLE_LIMIT, DEFAULT_TICK,
};
use eo_services::skill_tracker::SkillTracker;
pub use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use eo_services::time::naive_to_epoch;
use eo_services::tool_setup::ToolSetup;
use eo_services::tracker::{
    ActivityKey, CarriedWeapon, CarriedWeaponProfile, EquipmentLibrary, EquipmentProfile,
    GuardrailTool, HarvestGuardrailTools, HuntTracker, Providers, TrackingConfig,
};
use eo_wire::bus::DomainBus;
use eo_wire::domain_events::DomainEvent;
use serde_json::{Map, Value};

use crate::substrate::{Decline, DeclineReason};

/// The repository root, compiled into dev builds (the manifest dir is
/// `app/src-tauri/entropia-orme`). Release builds never read it.
fn dev_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// Where the user's data lives (see `eo_services::paths`). Release
/// builds resolve the installed app's per-user data directory; dev
/// builds honour `ENTROPIAORME_DATA_DIR` and the repo default.
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
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("snapshot"),
    }
}

/// The ABSOLUTE path to the platform's bundled ONNX Runtime dylib: the
/// installed resource dir in a release build, the committed repo copy
/// (`app/src-tauri/entropia-orme/resources/ort*/...`) in dev. On
/// Windows its siblings `DirectML.dll` and
/// `onnxruntime_providers_shared.dll` live in the same directory in
/// both layouts, where ONNX Runtime resolves them module-relative at
/// session creation; the Linux build is a single dylib with its GPU
/// provider (WebGPU) compiled in. Always absolute (the dev branch resolves through
/// the compiled-in [`dev_project_root`], the installed branch through
/// the OS-resolved `resource_dir`), so the runtime is never sought on
/// `PATH`/CWD.
fn ort_dylib_path(resource_dir: Option<&PathBuf>) -> PathBuf {
    // The runtime is platform-forked: the Windows onnxruntime-directml
    // build under `ort/`, the Linux WebGPU-enabled build under
    // `ort-linux/` (see each dir's PROVENANCE.txt). Both are bundled
    // beside the same `resources/` subtree; the release branch flattens
    // each to its own bundle target.
    #[cfg(target_os = "linux")]
    let (subdir, file) = ("ort-linux", "libonnxruntime.so");
    #[cfg(not(target_os = "linux"))]
    let (subdir, file) = ("ort", "onnxruntime.dll");
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join(subdir).join(file),
        _ => dev_project_root()
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join(subdir)
            .join(file),
    }
}

/// Where the bundled planet-map bundle lives (per-planet rasters plus
/// `calibration.json`): the bundled resource dir (`<resource_dir>/maps/`)
/// in a release build, the repo copy (`entropia-orme/resources/maps/`)
/// in dev, mirroring `tauri.conf.json`'s bundle map
/// (`resources/maps/` -> `maps/`).
fn maps_dir(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("maps"),
        _ => dev_project_root()
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("maps"),
    }
}

/// Where the recogniser's model + dict live: the bundled resource dir
/// (`<resource_dir>/models/`) in a release build, the repo copy
/// (`entropia-orme/resources/models/`) in dev. The asymmetry mirrors
/// `tauri.conf.json`'s bundle map (`resources/models/` -> `models/`):
/// the model is `svtrv2_rec.onnx`, the dict `ppocr_keys_v1.txt`.
fn models_dir(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("models"),
        _ => dev_project_root()
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
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
    // A compatible runtime ships for Windows (onnxruntime-directml) and
    // Linux (onnxruntime CPU); macOS and the rest have no bundled runtime,
    // so OCR stays offline there rather than pinning a foreign binary
    // (loading a wrong-platform library hangs or aborts the loader).
    if !cfg!(any(windows, target_os = "linux")) {
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
/// one injected clock. Kept separate from the typed-command facade so the
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
    // The chat-log watcher (tailing in its own thread). Held as an `Arc` so a
    // clone reaches the facade: the settings-write operation restarts it when
    // the watched `chatlog_path` changes. The exit seam still stops it via
    // [`ProducerState::stop`]; `restart`/`stop` take `&self`, so sharing is safe.
    watcher: Arc<ChatlogWatcher>,
    tracker: Arc<HuntTracker>,
    // The substrate runtime handle, kept for the synchronous exit seam's
    // one bridge onto the async tracker.
    runtime: tokio::runtime::Handle,
    // The typed domain-event channel. The bus bridge (subscribed below)
    // holds clones of this through its subscriber closures, and the shell's
    // Tauri-emit bridge subscribes over a clone handed off before this
    // state moves into the Tauri holder, so the publisher side and the
    // emit side share one channel.
    domain_bus: Arc<DomainBus>,
    // The settings writer. Held here so the producer spine and the
    // facade's write path share one service; a clone is handed to the
    // facade at composition. Mutex-guarded because `update`/`reset` take `&mut self`.
    config_service: Arc<Mutex<ConfigService>>,
    // The skill tracker. Held to keep its permanent bus subscription alive
    // for the substrate's lifetime, and exposed: the codex claim
    // operations call `suppress_next` on it.
    skill_tracker: Arc<SkillTracker>,
    // The in-process event bus the whole spine publishes on. Stored (rather
    // than left implicit on the subscriber handles) so the scan services
    // composed alongside the spine publish `scan.status.changed` on the SAME
    // bus the domain bridge subscribes, and so the shell's Tauri-emit
    // bridge carries scan-status envelopes exactly as session ones.
    bus: Arc<EventBus>,
    // The hotbar key listener. A producer (it publishes tool-change events on
    // the bus), gated on the hotbar-hooks toggle and an active session; held
    // here so the snapshot read can see whether it is running and so the
    // exit seam stops it. Shares the one keystroke source below.
    hotbar: Arc<HotbarListener>,
    // The one OS keyboard hook the input listeners share (the hotbar listener
    // here and the spacebar listener composed alongside the scan services).
    // Held so the spacebar listener can be built over the SAME source; the
    // hook is single-instance, so two independent sources would have one
    // stand inert.
    keystroke_source: Arc<dyn KeystrokeSource>,
    // This machine's identity, read once: keyless tracking is held to it.
    machine_id: Option<String>,
    // The quest service: its owning task holds the permanent bus
    // subscriptions for the substrate's lifetime, and the typed-command
    // facade serves the quest families over this same instance.
    quests: Arc<QuestService>,
    // Creature detection, bound once the recogniser has loaded: the tracker
    // starts and stops it with its sessions, and the exit path waits for
    // its look in progress.
    mob_detection: Arc<DetectorSlot>,
    // Reading the held tool, bound once the recogniser has loaded: the
    // tracker starts and stops it with its sessions while the setting is
    // on, and the exit path waits for its look in progress.
    tool_detection: Arc<eo_services::tool_watch::ToolDetectorSlot>,
}

impl ProducerState {
    /// Stop the producer spine: end any open session (so its stop
    /// events publish cleanly while the bus is still live), stop the
    /// watcher's tail thread, then drop the bus. Idempotent enough for
    /// the exit path: a second stop is a no-op on an already-stopped
    /// watcher and an already-idle tracker.
    pub fn stop(&self) {
        // Stop the input listener first (it detaches the shared OS hook), then
        // end any open session while the bus is live, then the watcher. The
        // exit seam is the one synchronous caller of the async tracker, so it
        // bridges onto the substrate runtime here.
        self.hotbar.stop();
        if self.tracker.is_tracking() {
            let _ = block_on_pool(&self.runtime, self.tracker.stop_session());
        }
        // Stopping the session stopped the creature and held-tool watchers;
        // wait, briefly, for a look still in progress, so nothing captures
        // behind the closing app.
        self.mob_detection.shutdown();
        eo_services::tool_watch::ToolDetector::shutdown(self.tool_detection.as_ref());
        self.watcher.stop();
    }

    /// Where creature detection is bound once the recogniser has loaded.
    pub fn mob_detection_handle(&self) -> Arc<DetectorSlot> {
        self.mob_detection.clone()
    }

    /// Where reading the held tool is bound once the recogniser has loaded.
    pub fn tool_detection_handle(&self) -> Arc<eo_services::tool_watch::ToolDetectorSlot> {
        self.tool_detection.clone()
    }

    /// The composed watcher, for tests driving a replay through it.
    #[cfg(test)]
    pub fn watcher(&self) -> &ChatlogWatcher {
        self.watcher.as_ref()
    }

    /// A handle to the composed chat-log watcher. The settings-write
    /// operation restarts it on a `chatlog_path` change; cloned into the
    /// facade at the composition handoff so the facade side and the
    /// producer-spine side share one watcher.
    pub fn watcher_handle(&self) -> Arc<ChatlogWatcher> {
        self.watcher.clone()
    }

    /// The composed tracker, for tests asserting its readout.
    #[cfg(test)]
    pub fn tracker(&self) -> &Arc<HuntTracker> {
        &self.tracker
    }

    /// A handle to the composed tracker. The tracking operations serve over
    /// this same `Arc<HuntTracker>`: the composition handoff clones it into
    /// the facade before this `ProducerState` moves into the
    /// Tauri-managed producer holder, so the facade and the exit-seam
    /// teardown share one tracker.
    pub fn tracker_handle(&self) -> Arc<HuntTracker> {
        self.tracker.clone()
    }

    /// A handle to the composed domain-event channel. The shell's
    /// Tauri-emit bridge subscribes over this same `Arc<DomainBus>`: the
    /// composition handoff clones it before this `ProducerState` moves into
    /// the Tauri-managed producer holder, so the emit side and the
    /// producer-bus bridge share one channel.
    pub fn domain_bus_handle(&self) -> Arc<DomainBus> {
        self.domain_bus.clone()
    }

    /// A handle to the composed settings writer. The settings-write
    /// operations serve over this same `Arc<Mutex<ConfigService>>`: the
    /// composition handoff clones it into the facade before this
    /// `ProducerState`
    /// moves into the Tauri-managed holder, so the write path and the spine
    /// share one service (reads elsewhere stay file-based, coherent because
    /// every save reads-merges-before-write).
    pub fn config_service_handle(&self) -> Arc<Mutex<ConfigService>> {
        self.config_service.clone()
    }

    /// A handle to the composed skill tracker. The codex claim operations
    /// call `suppress_next` on this same `Arc<SkillTracker>`: cloned into
    /// the facade at the handoff, so the facade side and the producer-bus
    /// subscription side share one tracker.
    pub fn skill_tracker_handle(&self) -> Arc<SkillTracker> {
        self.skill_tracker.clone()
    }

    /// A handle to the composed quest service. The typed-command facade
    /// serves the quest family over this same
    /// `Arc<QuestService>`: cloned into the facade at the composition
    /// handoff, so the bus-fed flows (session tracking, mission
    /// auto-start, reward suppression) and the command surface share one
    /// instance.
    pub fn quests_handle(&self) -> Arc<QuestService> {
        self.quests.clone()
    }

    /// A handle to the spine's event bus. The scan services compose on this
    /// same `Arc<EventBus>`, so their `scan.status.changed` envelopes reach
    /// the domain bridge (subscribed in [`compose_producers`]) and the
    /// shell's Tauri-emit bridge, exactly as the tracker's session events do.
    pub fn bus_handle(&self) -> Arc<EventBus> {
        self.bus.clone()
    }

    /// A handle to the composed hotbar listener. The tracking snapshot reads
    /// its `is_running` flag; cloned into the facade at the handoff so the
    /// facade side and the producer-spine side share one listener.
    pub fn hotbar_handle(&self) -> Arc<HotbarListener> {
        self.hotbar.clone()
    }

    /// A handle to the shared OS keyboard hook. The spacebar-capture listener
    /// composes over this SAME source, so both input listeners ride one hook
    /// (it is single-instance) while each gates independently.
    pub fn keystroke_source_handle(&self) -> Arc<dyn KeystrokeSource> {
        self.keystroke_source.clone()
    }

    /// This machine's input access for the facade: its identity and the
    /// shared keyboard source keyless tracking suppresses.
    pub fn input_access(&self) -> eo_api::settings::InputAccess {
        eo_api::settings::InputAccess::new(self.machine_id.clone(), self.keystroke_source.clone())
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
/// `Arc` because the scan consumer seams each hold their own clone,
/// captured at compose time.
pub struct Composed {
    /// The composed database handle, held so the exit seam can run the
    /// once-per-lifecycle `PRAGMA optimize` at shutdown (the last live user
    /// of the composed `Db` outside the typed-command facade).
    pub db: Db,
    /// The typed-command facade (the application boundary the typed
    /// Tauri commands dispatch into), sharing the database and catalogue
    /// handles.
    pub api: Arc<eo_api::Api>,
    pub producers: ProducerState,
    /// The warmed OCR engine when the runtime loaded. Production
    /// consumers (the scan services) hold their own clones captured at
    /// compose time, so nothing re-reads this copy outside the
    /// composition tests, which drive the warmed engine directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub ocr_engine: Option<Arc<OcrEngine>>,
    /// The skill scan, composed on the spine bus (its `scan.status.changed`
    /// envelopes reach the shell's event bridge) over the whole game
    /// window, the interface profile and the shared engine. Always
    /// constructed so the scan operations serve; without the engine or
    /// the profile it rests unavailable and never captures.
    pub skill_scan: Arc<SkillScanService>,
    /// The spacebar-capture listener, composed over the shared OS hook for
    /// the development research capture. Held for the exit seam (its
    /// teardown).
    pub spacebar_listener: Arc<SpacebarCaptureListener>,
    /// The coordinate-calibration Enter listener, on the same shared OS
    /// hook; enabled only while a calibration flow is live. Held for the
    /// exit seam (its teardown).
    pub coord_confirm: Arc<eo_services::coord_capture::CoordConfirmListener>,
    /// The radar flow's instance of the same gated Enter listener, held for
    /// the listener lifetime and the exit seam.
    pub radar_confirm: Arc<eo_services::coord_capture::CoordConfirmListener>,
}

/// The outcome of a composition attempt at the substrate's startup: the
/// native services are built and ready to install, or composition declined
/// with a logged reason. With the Python sidecar decommissioned (the
/// backend now collapsed into the shell process) there is no proxy fallback
/// and nothing to migrate an
/// existing database forward, so a decline is terminal for the session (the
/// backend cannot serve); there is no longer an interim "awaiting migration"
/// state to retry.
pub enum Composition {
    /// The native services are built and ready to install.
    Ready(Composed),
    /// A terminal decline (a missing/empty snapshot, a producer fault, or a
    /// database that cannot be opened or adopted, including one whose schema
    /// predates the supported baseline, which the retired sidecar used to
    /// migrate forward). Logged loudly; the backend does not come up. The
    /// reason is what the startup failure surface tells the user.
    Declined(Decline),
}

/// Compose the native services, or decline with a logged reason. The ONNX
/// Runtime dylib is pinned (once) before any composition step, so the engine
/// constructed inside `compose_with` binds the bundled runtime, not a stray
/// one off `PATH`.
pub async fn compose_native(resource_dir: Option<PathBuf>) -> Composition {
    init_ort_runtime(resource_dir.as_ref());
    // The single shared OS keyboard hook, built at this production site and
    // injected down the compose chain. The allowlist filters at the hook
    // boundary to the hotbar digit keys, the space key, Enter (the
    // coordinate-calibration confirm key), and F (the game's interact key, on
    // which a route can record its trees visited), so out-of-scope keystrokes
    // never enter the event stream.
    //
    // SECURITY (deliberate): admission is static for every allowlisted key,
    // not just the digits and spacebar. While the hook runs for any consumer,
    // an allowlisted edge enters the in-process stream and is dropped by every
    // listener unless a flow armed its consumer (calibration for Enter, a live
    // route planned to visit on F; neither logs nor persists the keystroke).
    // F is the one letter admitted, and a re-affirmed acceptance rather than
    // an oversight: a single letter's edges carry no recoverable text, just as
    // the hotbar digits already admitted carry none, and the route listener
    // additionally drops a press made while another window holds focus.
    // Dynamic admission (membership only while a flow is live) would scope
    // tighter but adds mutable shared state to a hook callback deliberately
    // kept to filter-and-enqueue; it remains the documented upgrade path if a
    // future key is genuinely sensitive. Tests inject a hook-free
    // `MockKeystrokeSource` through the same parameter instead, so a generic
    // test run never installs the OS hook (whose attach/detach lifecycle can
    // intermittently wedge a headless run). The game-focus probe is the other
    // half of that input boundary (the hotbar listener asks it whether a slot
    // press reached the game), so it is injected alongside, and tests pass
    // none rather than reading the live desktop's focus.
    let allowlist: std::collections::BTreeSet<String> = HOTBAR_SLOT_KEYS
        .iter()
        .map(|key| key.to_string())
        .chain(["space", "return", "f"].map(str::to_string))
        .collect();
    let keystroke_source: Arc<dyn KeystrokeSource> =
        Arc::new(HookKeystrokeSource::new(Some(allowlist)));
    compose_with(
        data_dir(),
        snapshot_dir(resource_dir.as_ref()),
        models_dir(resource_dir.as_ref()),
        maps_dir(resource_dir.as_ref()),
        keystroke_source,
        Some(Arc::new(eu_window::game_focus)),
    )
    .await
}

/// Composition over already-resolved locations (separated from the
/// environment-reading resolution so the decline paths are testable).
/// `models` is the recogniser's model+dict directory; the engine is
/// constructed and warmed from it, but a failed engine load never
/// declines composition (OCR is optional).
async fn compose_with(
    data_dir: PathBuf,
    snapshot: PathBuf,
    models: PathBuf,
    maps: PathBuf,
    keystroke_source: Arc<dyn KeystrokeSource>,
    game_focus: Option<GameFocusProbe>,
) -> Composition {
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        let detail = format!("data dir {} not creatable ({err})", data_dir.display());
        tracing::error!(target: "eo::composition", "{detail}; native services stand down");
        return Composition::Declined(Decline::new(DeclineReason::DataDirUnavailable, detail));
    }
    let db_path = data_dir.join(DB_FILE_NAME);
    let db = match Db::open_adopted(&db_path).await {
        Ok(db) => db,
        Err(err) if err.is_below_baseline() => {
            // The existing database is below the supported baseline AND below
            // the single rung the native first-launch upgrade bridges
            // (v32 -> v33, the version every in-the-wild v0.1.0-lineage
            // database sits at). A v32 database is upgraded in place and
            // adopted; only older schemas reach here, and none exist in the
            // wild, so this is a deliberate terminal decline, not a missing
            // capability.
            tracing::error!(
                target: "eo::composition",
                "{err}; the backend cannot serve until the database is at the supported baseline"
            );
            return Composition::Declined(Decline::new(
                DeclineReason::DatabaseBelowBaseline,
                err.to_string(),
            ));
        }
        Err(err @ AdoptError::Quarantined { .. }) => {
            // An existing database we cannot adopt (for any other reason) is
            // surfaced loudly and left untouched for diagnosis.
            tracing::error!(target: "eo::composition", "{err}");
            return Composition::Declined(Decline::new(
                DeclineReason::DatabaseUnreadable,
                err.to_string(),
            ));
        }
        Err(err) => {
            let detail = format!("database open failed ({err})");
            tracing::error!(target: "eo::composition", "{detail}; native services stand down");
            return Composition::Declined(Decline::new(DeclineReason::DatabaseUnreadable, detail));
        }
    };
    // A bounded corruption probe: PRAGMA quick_check on a reader connection,
    // time-boxed and run off the launch path so it never stalls startup. A
    // problem, or a budget overrun on a very large database, is logged and
    // the app keeps serving (degrade, never crash).
    {
        let probe_db = db.clone();
        tokio::spawn(async move {
            use eo_services::db::QuickCheckOutcome;
            match probe_db
                .quick_check_budgeted(eo_services::db::STARTUP_QUICK_CHECK_BUDGET)
                .await
            {
                QuickCheckOutcome::Ok => {}
                QuickCheckOutcome::Corrupt(report) => tracing::error!(
                    target: "eo::composition",
                    "startup quick_check reported database problems: {report}"
                ),
                QuickCheckOutcome::OverBudget => tracing::warn!(
                    target: "eo::composition",
                    "startup quick_check exceeded its budget and was skipped; \
                     the database is left unprobed this launch"
                ),
                QuickCheckOutcome::Error(err) => tracing::warn!(
                    target: "eo::composition",
                    "startup quick_check could not run: {err}"
                ),
            }
        });
    }

    let game_data = match GameDataStore::new(&snapshot) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            let detail = format!(
                "game-data snapshot at {} unreadable ({err})",
                snapshot.display()
            );
            tracing::error!(target: "eo::composition", "{detail}; native services stand down");
            return Composition::Declined(Decline::new(DeclineReason::GameDataUnavailable, detail));
        }
    };
    // The store tolerates a missing directory (warn-and-continue), but an
    // empty store here means the bundled resources are absent or
    // broken: serving game-data-derived responses from it would
    // silently diverge from the reference's embedded copy. Stand down
    // (a terminal decline) rather than serve divergent data.
    if game_data.total_entities() == 0 {
        let detail = format!("game-data snapshot at {} is empty", snapshot.display());
        tracing::error!(target: "eo::composition", "{detail}; native services stand down");
        return Composition::Declined(Decline::new(DeclineReason::GameDataUnavailable, detail));
    }
    let clock: Arc<dyn Clock> = Arc::new(RealClock::new());

    // The curated quest catalogue ships with the binary and is materialised
    // before any service reads quests. A failure leaves the catalogue as the
    // last launch left it and the app keeps serving (degrade, never crash).
    match eo_services::quests::catalogue::sync_catalogue(&db, clock.as_ref()).await {
        Ok(report) if report != Default::default() => tracing::info!(
            target: "eo::composition",
            "quest catalogue synced: {} inserted, {} adopted, {} updated, {} retired",
            report.inserted,
            report.adopted,
            report.updated,
            report.retired
        ),
        Ok(_) => {}
        Err(err) => tracing::error!(
            target: "eo::composition",
            "quest catalogue sync failed ({err}); the previous catalogue stays in place"
        ),
    }

    // The producer spine shares the substrate's single-owner pool with
    // the read surface: one connection, one owner, serialised access
    // (WAL + busy_timeout, max_connections(1)), so producer writes and
    // facade reads queue through the single connection without deadlock.
    // A second handle over the SAME pool: `Db` is a thin clonable handle
    // around the connection pool, so the producers and the read surface
    // share one connection (one owner, serialised access) rather than
    // opening a second.
    let producer_db = db.clone();
    let navigation_focus = game_focus.clone();
    let producers = match compose_producers(
        producer_db,
        clock.clone(),
        &data_dir,
        None,
        keystroke_source,
        game_focus,
        Some(game_data.clone()),
    ) {
        Ok(producers) => producers,
        Err(err) => {
            let detail = format!("producer spine failed ({err})");
            tracing::error!(target: "eo::composition", "{detail}; native services stand down");
            return Composition::Declined(Decline::new(DeclineReason::TrackingUnavailable, detail));
        }
    };

    // Construct the recogniser off the runtime worker (awaited; the engine
    // must exist for the handoff), with warm-up detached so the slow first
    // inference does not gate compose -> serve (see `build_ocr_engine`). A
    // failed load is logged and leaves the engine `None`; OCR is optional,
    // so composition still succeeds. The GPU-then-CPU ladder and the
    // recorded provider live in `OcrEngine::new_with_providers`.
    let ocr_engine = build_ocr_engine(models).await;

    // The scan services compose on the spine bus (so their status frames
    // reach the shell's event bridge) over the OCR extraction providers
    // and the shared single-owner pool, before the read surface takes
    // ownership of `db`/`game_data`/`clock`/`data_dir`. The interface
    // profile sits beside the snapshot dir in both the dev and installed
    // layouts, so it resolves as the snapshot's sibling.
    let resources = resource_root(&snapshot);

    // The readers that find what they want in the game window themselves
    // are composed over one capture seam for the whole window and one
    // interface profile (bundled beside the snapshot). A profile that will
    // not load is logged and leaves those readers unavailable; composition
    // goes on without them.
    let game_ui = GameUi {
        frame: Arc::new(eu_window::capture_game_window),
        profile: load_ui_profile(&resources.join(UI_PROFILE_FILE)),
    };

    // Creature detection: the whole game window, the interface profile and
    // the shared engine, matching names against the bundled creatures and
    // the ones the player has confirmed. Bound into the slot the tracker
    // was built with, so sessions started from now on can watch.
    let mob_catalogue = Arc::new(match &game_ui.profile {
        Some(profile) => {
            match eo_services::mob_review::build_catalogue(&db, &game_data, &profile.mob_names)
                .await
            {
                Ok(catalogue) => CatalogueCell::new(catalogue),
                Err(err) => {
                    tracing::warn!(
                        target: "eo::composition",
                        "confirmed creature names unreadable ({err}); matching the bundled list only"
                    );
                    CatalogueCell::new(eo_services::game_ui::mob_names::MobCatalogue::from_store(
                        &game_data,
                        Vec::<String>::new(),
                        &profile.mob_names,
                    ))
                }
            }
        }
        None => CatalogueCell::default(),
    });
    let mob_watch = MobWatch::new(
        MobWatchProviders {
            // Never the shared seam, which may start capture: the watcher
            // captures only from a capture already running, and starts one
            // only for the player's turning detection on.
            frame: Arc::new(eu_window::capture_game_window_running),
            capture_running: Arc::new(eo_services::screen_capture::capture_running),
            start_capture: Arc::new(eo_services::screen_capture::start_capture),
            enabled: {
                let reader = producers
                    .config_service_handle()
                    .lock()
                    .expect("config service")
                    .reader();
                Arc::new(move || reader.current().mob_detection_on())
            },
            focus: navigation_focus.clone(),
            profile: game_ui.profile.clone(),
            read_text: frame_reader(ocr_engine.clone()),
            catalogue: mob_catalogue.clone(),
            wait: None,
        },
        clock.clone(),
    );
    let mob_capture: eo_services::mob_watch::CaptureStatusProbe = {
        let mob_watch = mob_watch.clone();
        Arc::new(move || mob_watch.capture_status())
    };
    let mob_detector: Arc<dyn MobDetector> = Arc::new(mob_watch);
    producers.mob_detection_handle().bind(mob_detector);

    // Reading the held tool: the game window, the interface profile and the
    // shared engine, matching names against the player's library, the names
    // they dismissed and the bundled tools. Composed exactly as creature
    // detection is, and bound into the slot the tracker was built with.
    // The catalogue is built only while the reading is on: here, if it is on
    // already, before the watcher is bound (so before any session can start
    // it); otherwise when the player turns it on, before the tracker
    // re-reads its config and starts the watch.
    let tool_reading_on: eo_services::tool_catalogue::ReadingOn = {
        let reader = producers
            .config_service_handle()
            .lock()
            .expect("config service")
            .reader();
        Arc::new(move || reader.current().tool_reading_on())
    };
    let tool_catalogues = eo_services::tool_catalogue::ToolCatalogues::new(
        db.clone(),
        game_data.clone(),
        tool_reading_on.clone(),
    );
    if tool_reading_on() {
        tool_catalogues.ensure_current().await;
    }
    let tool_capture = bind_tool_watch(
        &producers,
        ToolWatchInputs {
            focus: navigation_focus.clone(),
            profile: game_ui.profile.clone(),
            read_text: frame_reader(ocr_engine.clone()),
            catalogue: tool_catalogues.cell(),
            clock: clock.clone(),
        },
    );

    let (skill_scan, sale_capture, spacebar_listener) = compose_scan_services(ScanInputs {
        bus: producers.bus_handle(),
        ocr_engine: ocr_engine.clone(),
        game_ui: &game_ui,
        game_data: game_data.clone(),
        db: db.clone(),
        clock: clock.clone(),
        keystroke_source: producers.keystroke_source_handle(),
    });

    // The terminal reads (the Repair Terminal's cost, the Trade Terminal's
    // total): the whole game window, the interface profile, and the shared
    // engine. When the engine did not load they are told so here, and
    // refuse as unavailable without capturing anything.
    let repair_ocr = Arc::new(RepairOcrService::new(RepairProviders {
        frame: game_ui.frame.clone(),
        profile: game_ui.profile.clone(),
        read_text: frame_reader(ocr_engine.clone()),
    }));

    // The planet-map bundle is optional: a missing or broken bundle stands
    // the maps surface down (an empty catalogue) with a logged reason,
    // never declines composition.
    let planet_maps = match eo_services::planet_maps::PlanetMapStore::new(&maps) {
        Ok(store) if !store.is_empty() => Some(Arc::new(store)),
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(
                target: "eo::composition",
                "planet-map bundle at {} unusable ({err}); the maps surface stands down",
                maps.display()
            );
            None
        }
    };

    // Coordinate capture: the player's position read from under the radar
    // wherever it is in the game window, through the same capture seam,
    // interface profile and engine as the other readers. The hand-drawn
    // region (the two-point calibration, persisted in config) is read only
    // when the radar cannot be found.
    let coord_capture = {
        use eo_services::coord_capture::{CoordCaptureProviders, CoordCaptureService};
        let region_reader = {
            let config = producers.config_service_handle();
            let reader = config.lock().expect("config service").reader();
            move || reader.current().map_coord_region
        };
        let persist_config = producers.config_service_handle();
        // In a debug build, a failed read leaves its crop and what was read
        // there under the data dir's debug/ (tiny, overwritten, local-only):
        // the standing instrument for diagnosing coordinates that will not
        // read. A clean read leaves nothing, and a release build never keeps
        // a capture of the screen.
        let coord_debug_dir = data_dir.join("debug");
        CoordCaptureService::new(
            CoordCaptureProviders {
                frame: game_ui.frame.clone(),
                profile: game_ui.profile.clone(),
                read_text: frame_reader(ocr_engine.clone()),
                // The names a radar may print, from the bundled maps: each
                // map's name and the game's own name for its planet.
                planets: planet_maps
                    .as_ref()
                    .map(|store| {
                        store
                            .records()
                            .iter()
                            .map(|record| eo_services::coord_capture::KnownPlanet {
                                name: record.name.clone(),
                                printed: std::iter::once(record.name.clone())
                                    .chain(record.technical_name.clone())
                                    .collect(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                cursor_position: Arc::new(eu_window::cursor_position),
                region: Arc::new(region_reader),
                persist_region: Arc::new(move |region| {
                    let mut updates = serde_json::Map::new();
                    updates.insert(
                        "map_coord_region".to_string(),
                        serde_json::to_value(region).map_err(|err| err.to_string())?,
                    );
                    persist_config
                        .lock()
                        .map_err(|_| "config service lock poisoned".to_string())?
                        .update(&updates)
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                }),
                debug_dir: Arc::new(move || {
                    cfg!(debug_assertions).then(|| coord_debug_dir.clone())
                }),
            },
            clock.clone(),
        )
    };
    let coord_confirm = eo_services::coord_capture::CoordConfirmListener::new(
        coord_capture.clone(),
        Some(producers.keystroke_source_handle()),
    );
    coord_capture.attach_confirm_listener(&coord_confirm);

    let navigation = {
        let bus = producers.bus_handle();
        let changed_bus = bus.clone();
        let changed_clock = clock.clone();
        let changed: eo_services::navigation::ChangedSink = Arc::new(move || {
            use eo_wire::domain_events::{
                NavigationUpdated, NavigationUpdatedPayload, NavigationUpdatedTag,
            };
            changed_bus.publish(&BusEvent::NavigationUpdated(NavigationUpdated {
                topic: NavigationUpdatedTag,
                event_version: 1,
                occurred_at: eo_services::time::to_iso_utc(naive_to_epoch(changed_clock.now())),
                payload: NavigationUpdatedPayload {},
            }));
        });
        let bounds_store = planet_maps.clone();
        let bounds: eo_services::navigation::BoundsProvider = Arc::new(move |planet| {
            let bounds = bounds_store
                .as_ref()?
                .record(planet)?
                .calibration
                .as_ref()?
                .bounds;
            Some(eo_services::game_ui::radar::CoordinateBounds {
                lon: (bounds.lon_min, bounds.lon_max),
                lat: (bounds.lat_min, bounds.lat_max),
            })
        });
        let service = eo_services::navigation::NavigationService::new(
            db.clone(),
            clock.clone(),
            eo_services::navigation::NavigationProviders {
                coord_capture: coord_capture.clone(),
                bounds,
                changed,
                input: Some(producers.keystroke_source_handle()),
                focus: navigation_focus,
                settings: eo_services::navigation::NavigationSettings::default(),
            },
        )
        .await;
        let radar_confirm = service.attach_radar_confirm_listener(
            Some(producers.keystroke_source_handle()),
            tokio::runtime::Handle::current(),
        );
        let harvest_navigation = service.clone();
        bus.subscribe(Topic::HarvestRecorded, move |event| {
            let BusEvent::HarvestRecorded(envelope) = event else {
                return;
            };
            let success = envelope.payload.success;
            let navigation = harvest_navigation.clone();
            tokio::spawn(async move { navigation.on_harvest(success).await });
        });
        (service, radar_confirm)
    };

    // The typed-command facade shares the read surface's handles plus the
    // producers the write families signal (the config writer, the hunt
    // tracker, the hotbar gate, the chat-log watcher, the skill tracker
    // a codex claim suppresses on, the skill scan the scan family drives,
    // and the spacebar listener the research capture arms), so the facade and the
    // producer spine serve over the same live instances.
    let api = Arc::new(
        eo_api::Api::new(
            db.clone(),
            game_data.clone(),
            clock.clone(),
            data_dir.clone(),
            producers.config_service_handle(),
            producers.tracker_handle(),
            producers.hotbar_handle(),
            producers.watcher_handle(),
            producers.skill_tracker_handle(),
            skill_scan.clone(),
            spacebar_listener.clone(),
            repair_ocr.clone(),
            sale_capture.clone(),
            producers.quests_handle(),
            planet_maps,
            Some(coord_capture.clone()),
            Some(navigation.0),
        )
        .with_input_access(producers.input_access())
        .with_mob_detection(
            mob_catalogue,
            game_ui
                .profile
                .as_ref()
                .map(|profile| profile.mob_names.clone()),
            Some(mob_capture),
        )
        // The library groups the entries the held-tool bar cannot tell
        // apart by the same profile the readers load; without one it
        // reports none and the library still reads.
        .with_tool_names(
            game_ui
                .profile
                .as_ref()
                .map(|profile| profile.tool_names.clone()),
        )
        // An edit to the library rebuilds the tools the held-tool watcher
        // matches against, so its next look matches with the library as it
        // now stands.
        .with_tool_reading(tool_catalogues, Some(tool_capture))
        .with_protection_changed({
            // Recording happens in the overlay's Cost popup while Equipment,
            // History, and the dashboard may be open elsewhere: each re-reads
            // on this signal rather than holding a stale armour cost.
            let bus = producers.bus_handle();
            let clock = clock.clone();
            Arc::new(move || {
                use eo_wire::domain_events::{
                    ProtectionUpdated, ProtectionUpdatedPayload, ProtectionUpdatedTag,
                };
                bus.publish(&BusEvent::ProtectionUpdated(ProtectionUpdated {
                    topic: ProtectionUpdatedTag,
                    event_version: 1,
                    occurred_at: eo_services::time::to_iso_utc(naive_to_epoch(clock.now())),
                    payload: ProtectionUpdatedPayload {},
                }));
            })
        })
        .with_healing_changed({
            // A correction in History moves a session's heal cost while the
            // review list may be open, and may take back or restore an effect
            // window a running session is still matching ticks against: the
            // tracker and every open surface re-read on this signal.
            let bus = producers.bus_handle();
            let clock = clock.clone();
            Arc::new(move || {
                use eo_wire::domain_events::{
                    HealingUpdated, HealingUpdatedPayload, HealingUpdatedTag,
                };
                bus.publish(&BusEvent::HealingUpdated(HealingUpdated {
                    topic: HealingUpdatedTag,
                    event_version: 1,
                    occurred_at: eo_services::time::to_iso_utc(naive_to_epoch(clock.now())),
                    payload: HealingUpdatedPayload {},
                }));
            })
        })
        .with_market_changed({
            // A paste, a quote, a manual markup, or a Nanocube mark moves the
            // markup loot is valued at while the dashboard, the overlay, and
            // Inventory may all be open: each re-reads on this signal.
            let bus = producers.bus_handle();
            let clock = clock.clone();
            Arc::new(move || {
                use eo_wire::domain_events::{
                    MarketUpdated, MarketUpdatedPayload, MarketUpdatedTag,
                };
                bus.publish(&BusEvent::MarketUpdated(MarketUpdated {
                    topic: MarketUpdatedTag,
                    event_version: 1,
                    occurred_at: eo_services::time::to_iso_utc(naive_to_epoch(clock.now())),
                    payload: MarketUpdatedPayload {},
                }));
            })
        })
        .with_weapons_changed({
            // Assigning an unpriced shot in History moves an ended session's
            // weapon cost while its detail and the session list may be open:
            // every open surface re-reads on this signal.
            let bus = producers.bus_handle();
            let clock = clock.clone();
            Arc::new(move || {
                use eo_wire::domain_events::{
                    WeaponsUpdated, WeaponsUpdatedPayload, WeaponsUpdatedTag,
                };
                bus.publish(&BusEvent::WeaponsUpdated(WeaponsUpdated {
                    topic: WeaponsUpdatedTag,
                    event_version: 1,
                    occurred_at: eo_services::time::to_iso_utc(naive_to_epoch(clock.now())),
                    payload: WeaponsUpdatedPayload {},
                }));
            })
        }),
    );
    Composition::Ready(Composed {
        db,
        api,
        producers,
        ocr_engine,
        skill_scan,
        spacebar_listener,
        coord_confirm,
        radar_confirm: navigation.1,
    })
}

/// What the held-tool watcher reads with, beside the producers it is bound
/// into.
struct ToolWatchInputs {
    focus: Option<GameFocusProbe>,
    profile: Option<Arc<UiProfile>>,
    read_text: Option<FrameReader>,
    catalogue: Arc<eo_services::tool_watch::ToolCatalogueCell>,
    clock: Arc<dyn Clock>,
}

/// The held-tool watcher's seams. It captures only from a capture already
/// running (never the shared seam, which may start capture), starts one only
/// for the player's turning the reading on, and is on only while the
/// player's setting is, read live from `reader` before every look.
fn tool_watch_providers(
    reader: ConfigReader,
    inputs: ToolWatchInputs,
) -> eo_services::tool_watch::ToolWatchProviders {
    eo_services::tool_watch::ToolWatchProviders {
        frame: Arc::new(eu_window::capture_game_window_running),
        capture_running: Arc::new(eo_services::screen_capture::capture_running),
        start_capture: Arc::new(eo_services::screen_capture::start_capture),
        enabled: Arc::new(move || reader.current().tool_reading_on()),
        focus: inputs.focus,
        profile: inputs.profile,
        read_text: inputs.read_text,
        catalogue: inputs.catalogue,
        wait: None,
    }
}

/// Build the held-tool watcher and bind it into the tracker's slot; its
/// capture status, for the settings. It captures under the one lock every
/// watcher of the game window shares.
fn bind_tool_watch(
    producers: &ProducerState,
    inputs: ToolWatchInputs,
) -> eo_services::screen_watch::CaptureStatusProbe {
    use eo_services::tool_watch::{ToolDetector, ToolWatch};
    let reader = producers
        .config_service_handle()
        .lock()
        .expect("config service")
        .reader();
    let clock = inputs.clock.clone();
    let tool_watch = ToolWatch::new(tool_watch_providers(reader, inputs), clock);
    let capture: eo_services::screen_watch::CaptureStatusProbe = {
        let tool_watch = tool_watch.clone();
        Arc::new(move || tool_watch.capture_status())
    };
    let detector: Arc<dyn ToolDetector> = Arc::new(tool_watch);
    producers.tool_detection_handle().bind(detector);
    capture
}

/// Construct the OCR engine off the async runtime worker, then warm it up
/// DETACHED. Construction (a session commit) is awaited because the engine
/// must exist before the handoff to managed state, but it is quick. Warm-up
/// (a real inference; the GPU providers compile shaders on first run, seconds) is
/// NOT awaited: stalling it ahead of `serve()` would make every request the
/// webview fires during startup hang, so it runs on a detached blocking
/// thread, concurrent with the server coming up. The reference warms lazily
/// on first scan, so deferring the cost off the startup path is, if
/// anything, more faithful. Returns `None` (logged) on any load failure:
/// OCR is an optional faculty and never declines composition.
async fn build_ocr_engine(models: PathBuf) -> Option<Arc<OcrEngine>> {
    // OCR ships only where a compatible ONNX Runtime is bundled: Windows
    // (the onnxruntime-directml libraries) and Linux (the WebGPU-enabled
    // build). Elsewhere the engine stays absent rather than attempting to
    // load a foreign-platform runtime (which hangs the loader), exactly as
    // a failed load would leave it. Keep this gate aligned with
    // `init_ort_runtime`'s platform check.
    if !cfg!(any(windows, target_os = "linux")) {
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

/// What the scan services are composed over.
struct ScanInputs<'a> {
    /// The spine bus their status frames publish on.
    bus: Arc<EventBus>,
    ocr_engine: Option<Arc<OcrEngine>>,
    game_ui: &'a GameUi,
    game_data: Arc<GameDataStore>,
    db: Db,
    clock: Arc<dyn Clock>,
    /// The shared OS keyboard hook the spacebar listener rides.
    keystroke_source: Arc<dyn KeystrokeSource>,
}

/// What the readers of the game's interface are composed over: the
/// whole game window, captured on demand, and the interface profile.
struct GameUi {
    frame: GameWindowFrame,
    profile: Option<Arc<UiProfile>>,
}

/// The interface profile's file name, bundled beside the snapshot dir
/// (`tauri.conf.json` maps `resources/ui_profile.json` there).
const UI_PROFILE_FILE: &str = "ui_profile.json";

/// The directory the bundled data files sit in: the snapshot dir's parent,
/// in the dev and installed layouts alike.
fn resource_root(snapshot: &std::path::Path) -> PathBuf {
    snapshot.parent().unwrap_or(snapshot).to_path_buf()
}

/// The bundled interface profile, or None (logged) when it cannot be
/// used, which leaves the readers that need it unavailable.
fn load_ui_profile(path: &std::path::Path) -> Option<Arc<UiProfile>> {
    match UiProfile::load(path) {
        Ok(profile) => Some(Arc::new(profile)),
        Err(err) => {
            tracing::warn!(
                target: "eo::composition",
                "interface profile at {} unusable ({err}); the readers of the game window stand down",
                path.display()
            );
            None
        }
    }
}

/// The skill scan's completion callback: it persists an accepted scan's
/// levels through the shared database handle, each stamped with when its
/// page was read, bridging onto the runtime from the calling thread the
/// same dual way the tracker's providers do. A persist error is answered
/// to the player rather than panicking.
fn skill_scan_completion(db: Db, runtime: tokio::runtime::Handle) -> CompletionCallback {
    Arc::new(move |levels: &[ScannedLevel]| {
        let levels = levels.to_vec();
        let db = db.clone();
        block_on_pool(
            &runtime,
            async move { complete_skill_scan(&db, &levels).await },
        )
        .map(|_drift| ())
        .map_err(Into::into)
    })
}

/// The engine as a recogniser of one crop; `None` without an engine.
fn frame_reader(engine: Option<Arc<OcrEngine>>) -> Option<FrameReader> {
    engine.map(|engine| -> FrameReader {
        Arc::new(move |frame: &BgrImage| engine.recognize_bgr(&frame.data, frame.h, frame.w).ok())
    })
}

/// Compose the skill scan, the sell-queue capture, and the spacebar
/// listener. The skill scan reads the whole game window through the
/// interface profile and the shared engine, checking what it reads
/// against the bundled skill list; it publishes `scan.status.changed` on
/// the spine `bus` (so its frames reach the shell's event bridge) and
/// persists an accepted scan through the shared database. Without the
/// engine or the profile it rests unavailable rather than declining
/// composition, so the scan operations still serve. The sell-queue capture
/// reads the whole game window through the same profile and engine,
/// confirming queued names against the bundled item data. All three
/// services are always constructed.
fn compose_scan_services(
    inputs: ScanInputs<'_>,
) -> (
    Arc<SkillScanService>,
    Arc<SaleCaptureService>,
    Arc<SpacebarCaptureListener>,
) {
    let ScanInputs {
        bus,
        ocr_engine,
        game_ui,
        game_data,
        db,
        clock,
        keystroke_source,
    } = inputs;

    let skill_scan = SkillScanService::new(
        SkillScanProviders {
            frame: game_ui.frame.clone(),
            profile: game_ui.profile.clone(),
            read_text: frame_reader(ocr_engine.clone()),
            vocabulary: SkillVocabulary::from_entities(game_data.get_entities("skills")),
            tick: DEFAULT_TICK,
            idle_limit: DEFAULT_IDLE_LIMIT,
        },
        clock.clone(),
        Some(bus),
    );
    skill_scan
        .set_completion_callback(skill_scan_completion(db, tokio::runtime::Handle::current()));

    // The sell-queue capture: the whole game window, the interface profile
    // and the shared engine, with the bundled item data built once into
    // the catalogue that confirms a queued name.
    let sale_capture = Arc::new(SaleCaptureService::new(
        SaleCaptureProviders {
            frame: game_ui.frame.clone(),
            profile: game_ui.profile.clone(),
            catalogue: Arc::new(ItemCatalogue::from_store(&game_data)),
            read_text: frame_reader(ocr_engine),
        },
        clock,
    ));

    // The spacebar-capture listener, over the SAME OS hook the hotbar
    // listener rides (the hook is single-instance). Unarmed until the
    // development research capture arms it.
    let spacebar = SpacebarCaptureListener::new(Some(keystroke_source));

    (skill_scan, sale_capture, spacebar)
}

/// Subscriber backlog bound for the typed domain channel (the shell's
/// emit bridge is the one live subscriber; a receiver that falls behind
/// observes a lag error and skips ahead rather than stalling publishers).
const DOMAIN_BUS_CAPACITY: usize = 256;

/// Why the producer spine declined to compose: a database fault, or a
/// settings store that refused to load (a corrupt or wrong-shape
/// `settings.json` fails composition loudly rather than silently
/// resetting user settings). The caller logs it and stands the native
/// services down.
#[derive(Debug, thiserror::Error)]
enum ComposeError {
    #[error(transparent)]
    Db(#[from] eo_services::db::DbError),
    #[error("settings store: {0}")]
    Config(#[from] std::io::Error),
}

/// Build and start the producer spine over the shared pool and clock.
/// Every lookup the tracker consults reads through the same database or
/// the same config read-through the `ConfigService` writes.
fn compose_producers(
    db: Db,
    clock: Arc<dyn Clock>,
    data_dir: &std::path::Path,
    chatlog_override: Option<PathBuf>,
    keystroke_source: Arc<dyn KeystrokeSource>,
    game_focus: Option<GameFocusProbe>,
    game_data: Option<Arc<GameDataStore>>,
) -> Result<ProducerState, ComposeError> {
    // The producers run on the substrate's tokio runtime; the trackers
    // bridge their database work onto this handle from their own
    // (non-runtime) producer threads.
    let runtime = tokio::runtime::Handle::current();
    let bus = Arc::new(EventBus::new());

    // The domain bridge: forward the frontend-facing domain topics off the
    // in-process bus onto the typed broadcast channel the shell's
    // Tauri-emit bridge consumes. Subscribed here, before the watcher's
    // tail thread starts, so an event published the instant a producer
    // ticks is never raced away from a connected subscriber. The channel is
    // held in `ProducerState` (and through these subscriber closures), and
    // a clone is handed to the shell at composition.
    let domain_bus = Arc::new(DomainBus::new(DOMAIN_BUS_CAPACITY));
    subscribe_domain_bridge(&bus, &domain_bus);

    // The config read-through: producers read the live config the same
    // way the read surface does (`settings.json`, now written solely by
    // the native `ConfigService`), so a wrong default never silently
    // corrupts tracker DB state. A read failure falls back to the typed
    // defaults.
    let config = load_config_readonly(data_dir).unwrap_or_default();

    // The settings writer the write operations serve over. A corrupt or
    // wrong-shape settings file fails composition loudly (a terminal
    // decline) rather than silently resetting user settings.
    let config_service = Arc::new(Mutex::new(ConfigService::new(data_dir)?));

    // The quest service is bus-subscribed (session tracking + mission
    // auto-start) and supplies the watcher's quest-reward filter, so a
    // mission completion can suppress its reward echo. Its owning task
    // serialises those flows;
    // the filter closure is a rendezvous into it.
    let quests = QuestService::start(&bus, db.clone(), clock.clone(), runtime.clone());

    let watched_chatlog = chatlog_override.unwrap_or_else(|| PathBuf::from(&config.chatlog_path));
    // One chat-log time base, shared by the tail that derives it from
    // the live file and by every service that resolves the readings it
    // publishes. The log is stamped in the game server's zone, so this
    // is what keeps a logged instant comparable with one the app
    // stamped itself; see `eo_services::chatlog_time`.
    let chatlog_clock = ChatLogClock::observed();
    let watcher = Arc::new(ChatlogWatcher::new(
        bus.clone(),
        watched_chatlog,
        Some(quests.watcher_filter()),
        chatlog_clock.clone(),
    ));
    watcher.set_signal_reward_filter(quests.watcher_signal_filter());
    // A loot line's arrival is stamped on the application's clock, the one
    // the creature watcher stamps its looks on, so the two compare.
    watcher.set_arrival_clock(clock.clone());
    let mob_detection = Arc::new(DetectorSlot::default());
    let tool_detection = Arc::new(eo_services::tool_watch::ToolDetectorSlot::default());

    let skill_tracker = SkillTracker::new(&bus, db.clone(), clock.clone(), chatlog_clock.clone());

    // The input listeners share ONE keyboard source (the OS hook is
    // single-instance): a ref-counted wrapper makes the injected source
    // shareable. The hotbar listener is a producer (it publishes tool-change
    // events on the bus), gated on the hotbar-hooks toggle AND an active
    // session; the spacebar listener is composed alongside the scan services
    // over this same source. The underlying source is injected by the caller
    // (`compose_native` builds the real boundary-filtered OS hook; tests pass a
    // hook-free mock), so a generic test run cannot wedge on the hook's
    // attach/detach lifecycle. Off Windows (or in a headless run) the real hook
    // is inert, so both listeners never run.
    let keystroke_source: Arc<dyn KeystrokeSource> =
        Arc::new(SharedKeystrokeSource::new(keystroke_source));
    // Keyless tracking is held to the machine it was turned on for. Apply it
    // before any consumer can start the keyboard, so a keyless machine never
    // opens it at all.
    let machine_id = eo_services::machine_identity::current();
    keystroke_source.set_suppressed(config.keyless_input_on(machine_id.as_deref()));
    // The running doses, published by the tracker and read by every other
    // surface that prices under the reload speed in effect.
    let doses = DoseBoard::new(clock.clone());
    let items = build_item_resolver(db.clone(), data_dir, game_data.clone(), doses.clone());
    let hotbar = HotbarListener::new(
        bus.clone(),
        Some(keystroke_source.clone()),
        Some(build_hotbar_resolver(data_dir, items.clone())),
        game_focus,
    );
    hotbar.set_item_resolver(items.clone());
    // Apply the stored toggle; the source still only attaches while a session
    // is active (the listener reconciles on the session bus events).
    hotbar.set_hotbar_hooks_enabled(config.hotbar_keys_observed(machine_id.as_deref()));

    let config_reader = config_service
        .lock()
        .expect("config service lock at composition")
        .reader();
    let tracker = block_on_pool(
        &runtime,
        HuntTracker::new(
            bus.clone(),
            db.clone(),
            clock.clone(),
            chatlog_clock,
            Providers {
                detector: mob_detection.clone(),
                tool_detector: tool_detection.clone(),
                // A library entry read off the screen resolves as a press
                // or a declaration of it does; a tool the library does not
                // hold prices bare from the game data.
                items: Some(items.clone()),
                bundled_tools: game_data.clone().map(|data| {
                    let reader = config_reader.clone();
                    let doses = doses.clone();
                    eo_services::bundled_tools::resolver(
                        data,
                        Arc::new(move || {
                            doses.reload_speed_percent(&reader.current().passive_effect_sources)
                        }),
                    )
                }),
                ..build_providers(
                    db,
                    config_reader,
                    machine_id.clone(),
                    &config,
                    runtime.clone(),
                    game_data,
                    doses,
                )
            },
        ),
    )?;

    // Wire the quest service's interval sink now that the tracker owning
    // the interval state exists (the quest service is built first, so this
    // cannot be a constructor argument). A completion closes the quest's
    // declared stretch, when the user declared one on the running session;
    // opening a stretch is the user's own declaration, never the
    // lifecycle's (the mission log only witnesses pickup and hand-in,
    // which bulk play separates from the effort between them).
    //
    // Deliberately a direct call rather than a bus topic: the corpus
    // fingerprints capture the published event stream, and the banked
    // port-equivalence captures must stay byte-identical.
    {
        let tracker_sink = tracker.clone();
        quests.set_stretch_closer(Arc::new(move |quest_id| {
            let tracker = tracker_sink.clone();
            Box::pin(async move {
                // Errors are swallowed on purpose: with no session
                // running (or no stretch declared) there is nothing to
                // close, and that is not a failure of the completion.
                let _ = tracker
                    .deactivate_activity(ActivityKey::Quest(quest_id))
                    .await;
            })
        }));
    }

    {
        let tracker_sink = tracker.clone();
        quests.set_loot_reconciler(Arc::new(move |source_id| {
            let tracker = tracker_sink.clone();
            Box::pin(async move {
                let _ = tracker.reconcile_loot_source(&source_id).await;
            })
        }));
    }

    // Wire the mission-completion probe: a tick's completions land
    // strictly after its publishes, so the tick's own loot (the final
    // objective kill, the payout) stamps into the declared stretch
    // before the completion closes it. Fire-and-forget onto the
    // runtime, because the probe is called from the tail thread and
    // must never block it.
    {
        let quests_probe = quests.clone();
        let probe_runtime = runtime.clone();
        watcher.set_mission_complete_probe(Arc::new(move |completions| {
            let quests = quests_probe.clone();
            probe_runtime.spawn(async move {
                // Errors are contained: a failed check must not take the
                // tail loop's attention, and the quest's own state is
                // re-derivable from the next matching tick.
                let _ = quests.mission_complete_check(&completions).await;
            });
        }));
    }

    // Wire the signal-loot probe: a loot tick with no mission completion
    // may complete a signal quest (the instance-boss pattern). Fire-and-
    // forget onto the runtime, because the probe is called from the tail
    // thread and must never block it; the quest service serialises the
    // completion itself.
    {
        let quests_probe = quests.clone();
        let probe_runtime = runtime.clone();
        watcher.set_signal_loot_probe(Arc::new(move |clump| {
            let quests = quests_probe.clone();
            probe_runtime.spawn(async move {
                // Errors are contained: a failed check must not take the
                // tail loop's attention, and the quest's own state is
                // re-derivable from the next matching tick.
                let _ = quests.raw_loot_clump_check(&clump).await;
            });
        }));
    }

    // Start the tail thread last, after every subscriber is registered,
    // so no published tick can land before the trackers can see it.
    watcher.start();

    Ok(ProducerState {
        watcher,
        tracker,
        runtime,
        domain_bus,
        config_service,
        skill_tracker,
        bus,
        hotbar,
        keystroke_source,
        machine_id,
        quests,
        mob_detection,
        tool_detection,
    })
}

/// The hotbar slot resolver: the
/// live config maps the slot key to an equipment-library id; the row's item
/// type selects the outcome (a healing tool's per-use cost and reload from its
/// entity, a consumable's dose, or a weapon's per-shot cost looked up by name
/// fragment exactly as the cost provider does). Reloads and attack rates are
/// under the reload speed in effect at the press: the equipped sources and
/// the running doses. An unbound slot, an absent row, or a read failure
/// yields None (no tool change).
/// Resolve a hotbar slot to the item bound there, through the item resolver
/// an overlay declaration uses too, so a press and a declaration price alike.
fn build_hotbar_resolver(data_dir: &std::path::Path, items: ItemResolver) -> HotbarResolver {
    let data_dir = data_dir.to_path_buf();
    Arc::new(move |slot: &str| {
        let config = load_config_readonly(&data_dir).ok()?;
        let equip_id = config.hotbar.get(slot).and_then(Value::as_i64)?;
        items(equip_id)
    })
}

/// Resolve an equipment-library item to its intent snapshot: kind, per-use
/// cost and reload under the reload speed in effect, and its healing,
/// lifesteal, or dose profile.
fn build_item_resolver(
    db: Db,
    data_dir: &std::path::Path,
    game_data: Option<Arc<GameDataStore>>,
    doses: DoseBoard,
) -> ItemResolver {
    let data_dir = data_dir.to_path_buf();
    Arc::new(move |equip_id: i64| {
        let config = load_config_readonly(&data_dir).ok()?;
        let db = db.clone();
        let game_data = game_data.clone();
        let reload_speed = doses.reload_speed_percent(&config.passive_effect_sources);
        // The hotbar listener runs on the input listener's plain OS key
        // thread (no async runtime), so the slot lookup reads through the
        // synchronous reader core rather than bridging an async query onto
        // the runtime. A read failure yields None (no tool change), exactly
        // as the swallowed async error did.
        db.with_reader_blocking(move |conn| {
            let Some((name, item_type, properties_json)) =
                hotbar_equipment_row_sync(conn, equip_id)?
            else {
                return Ok(None);
            };
            let outcome = match item_type.as_str() {
                "healing" => {
                    let (cost_ped, base_reload_seconds) = heal_cost_from_props(&properties_json);
                    let reload_seconds = reload_seconds_under(base_reload_seconds, reload_speed);
                    let mut healing_profile = healing_profile_from_props(&properties_json);
                    healing_profile.base_reload_seconds = Some(base_reload_seconds);
                    healing_profile.reload_speed_percent = Some(reload_speed);
                    healing_profile.effective_reload_seconds = Some(reload_seconds);
                    let tt_per_use_ped =
                        eo_services::equipment_pricing::heal_tt_from_props(&properties_json);
                    healing_profile.tt_per_use_ped = Some(tt_per_use_ped);
                    healing_profile.on_use = serde_json::from_str::<Value>(&properties_json)
                        .ok()
                        .and_then(|props| on_use_effect_from_props(&props, game_data.as_deref()));
                    ResolvedHotbarItem {
                        equipment_id: equip_id,
                        name,
                        kind: HotbarItemKind::Healing,
                        cost_per_use_ped: cost_ped,
                        reload_seconds,
                        healing_profile: Some(healing_profile),
                        lifesteal_percent: None,
                        consumable_profile: None,
                        setup: ToolSetup::from_props_json(&properties_json, Some(tt_per_use_ped)),
                    }
                }
                "consumable" => {
                    // One dose as the item stands at the press, over the
                    // current catalogue.
                    let props =
                        serde_json::from_str::<Value>(&properties_json).unwrap_or(Value::Null);
                    let profile =
                        consumable_profile_from_props(&props, game_data.as_deref()).profile;
                    ResolvedHotbarItem {
                        equipment_id: equip_id,
                        name,
                        kind: HotbarItemKind::Consumable,
                        cost_per_use_ped: profile.booked_cost_ped(),
                        reload_seconds: 0.0,
                        healing_profile: None,
                        lifesteal_percent: None,
                        consumable_profile: Some(profile),
                        setup: ToolSetup::default(),
                    }
                }
                // A harvesting tool stores the same single-entity props
                // shape as a healing tool (tool_entity + markup), so the
                // heal per-use recipe prices it; no reload semantics.
                "tool" => {
                    let (cost_ped, _) = heal_cost_from_props(&properties_json);
                    let tt_per_use_ped =
                        eo_services::equipment_pricing::heal_tt_from_props(&properties_json);
                    ResolvedHotbarItem {
                        equipment_id: equip_id,
                        name,
                        kind: HotbarItemKind::Harvesting,
                        cost_per_use_ped: cost_ped,
                        reload_seconds: 0.0,
                        healing_profile: None,
                        lifesteal_percent: None,
                        consumable_profile: None,
                        setup: ToolSetup::from_props_json(&properties_json, Some(tt_per_use_ped)),
                    }
                }
                _ => {
                    let cost = weapon_cost_by_name(conn, &name, &|props| {
                        with_attack_rate(props, game_data.as_deref(), reload_speed)
                    });
                    ResolvedHotbarItem {
                        equipment_id: equip_id,
                        name,
                        kind: HotbarItemKind::Weapon,
                        cost_per_use_ped: cost,
                        reload_seconds: 0.0,
                        healing_profile: None,
                        lifesteal_percent: game_data
                            .as_deref()
                            .and_then(|catalogue| {
                                lifesteal_percent_from_props_with_catalog(
                                    &properties_json,
                                    catalogue,
                                )
                            })
                            .or_else(|| lifesteal_percent_from_props(&properties_json)),
                        consumable_profile: None,
                        // A weapon's price and its TT follow the tracker's
                        // own pricing of it (its enhancers wear down), so only
                        // its parts are taken here.
                        setup: ToolSetup::from_props_json(&properties_json, None),
                    }
                }
            };
            Ok(Some(outcome))
        })
        .ok()
        .flatten()
    })
}

/// Bridge the producer bus's frontend-facing domain topics onto the typed
/// broadcast channel: the same two topics, now carried as typed envelopes
/// end to end (the tracker and skill-scan publish sites construct them
/// directly, so there is no serialise-and-reparse step left on the path).
/// The subscriptions live in the bus (held alive by the producer spine)
/// and capture channel clones, so they need no separate registration
/// store; they drop with the bus when the spine tears down.
fn subscribe_domain_bridge(bus: &EventBus, domain_bus: &Arc<DomainBus>) {
    for topic in [
        Topic::TrackingSessionUpdated,
        Topic::ScanStatusChanged,
        Topic::HarvestRecorded,
        Topic::NavigationUpdated,
        Topic::ProtectionUpdated,
        Topic::HealingUpdated,
        Topic::WeaponsUpdated,
        Topic::ConsumablesUpdated,
        Topic::MarketUpdated,
    ] {
        let domain_bus = domain_bus.clone();
        bus.subscribe(topic, move |event| match event {
            BusEvent::TrackingSessionUpdated(envelope) => {
                domain_bus.publish(DomainEvent::TrackingSessionUpdated(envelope.clone()));
            }
            BusEvent::ScanStatusChanged(envelope) => {
                domain_bus.publish(DomainEvent::ScanStatusChanged(envelope.clone()));
            }
            BusEvent::HarvestRecorded(envelope) => {
                domain_bus.publish(DomainEvent::HarvestRecorded(envelope.clone()));
            }
            BusEvent::NavigationUpdated(envelope) => {
                domain_bus.publish(DomainEvent::NavigationUpdated(envelope.clone()));
            }
            BusEvent::ProtectionUpdated(envelope) => {
                domain_bus.publish(DomainEvent::ProtectionUpdated(envelope.clone()));
            }
            BusEvent::HealingUpdated(envelope) => {
                domain_bus.publish(DomainEvent::HealingUpdated(envelope.clone()));
            }
            BusEvent::WeaponsUpdated(envelope) => {
                domain_bus.publish(DomainEvent::WeaponsUpdated(envelope.clone()));
            }
            BusEvent::ConsumablesUpdated(envelope) => {
                domain_bus.publish(DomainEvent::ConsumablesUpdated(envelope.clone()));
            }
            BusEvent::MarketUpdated(envelope) => {
                domain_bus.publish(DomainEvent::MarketUpdated(envelope.clone()));
            }
            // A foreign event on a domain topic is unrepresentable at the
            // publish site; nothing to forward.
            _ => {}
        });
    }
}

/// The live equipment library behind the tracker's equipment seam:
/// profile and cost lookups over the equipment tables, and the carried
/// weapons resolved over the live config plus those tables. The
/// lookups bridge onto the runtime from the synchronous trait calls
/// (the tracker invokes them inline on its own task).
struct LiveEquipmentLibrary {
    db: Db,
    reader: ConfigReader,
    runtime: tokio::runtime::Handle,
    game_data: Option<Arc<GameDataStore>>,
    doses: DoseBoard,
}

impl LiveEquipmentLibrary {
    /// A stored weapon's props as tracking prices and attributes them:
    /// offensive efficiencies from the current catalogue, and the attack
    /// rate under the reload speed in effect (the declared passive effects
    /// and the running doses).
    fn current_weapon_props(&self, props: &Value) -> Value {
        let game_data = self.game_data.as_deref();
        let props = game_data.map_or_else(
            || props.clone(),
            |game_data| with_current_offensive_efficiencies(props, game_data),
        );
        let reload_speed = self
            .doses
            .reload_speed_percent(&self.reader.current().passive_effect_sources);
        with_attack_rate(&props, game_data, reload_speed)
    }
}

impl EquipmentLibrary for LiveEquipmentLibrary {
    fn weapon_profile(&self, tool_name: &str) -> EquipmentProfile {
        let tool_name = tool_name.to_string();
        let db = self.db.clone();
        let json = block_on_pool(&self.runtime, async move {
            db.weapon_properties_by_name_fragment(&tool_name)
                .await
                .ok()
                .flatten()
        })?;
        match serde_json::from_str::<Value>(&json) {
            Ok(props @ Value::Object(_)) => self.current_weapon_props(&props).as_object().cloned(),
            _ => None,
        }
    }

    fn cost_per_shot(&self, tool_name: &str) -> f64 {
        // The per-shot cost in PED derived from the profile, or 0.0 when
        // the tool is unknown.
        match self.weapon_profile(tool_name) {
            Some(props) => cost_per_shot_ped(&Value::Object(props)),
            None => 0.0,
        }
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        // Every preset's weapons (the live preset's first, in slot order),
        // then the weapons carried without a slot, each once. A slot holding
        // a healer, harvesting tool, or consumable, and an id no longer in
        // the library, carry nothing. Each weapon's props are prepared as
        // the profile lookup's are, so its band carries the same attack rate
        // its cost does.
        let config = self.reader.current();
        let mut ids = config.hotbar_ids_live_first();
        for id in config.carried_weapon_ids.iter().copied() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return Vec::new();
        }
        let db = self.db.clone();
        let rows = block_on_pool(&self.runtime, async move {
            db.with_reader(move |conn| {
                let mut rows = Vec::with_capacity(ids.len());
                for id in ids {
                    if let Some((name, item_type, properties_json)) =
                        hotbar_equipment_row_sync(conn, id)?
                    {
                        if item_type == "weapon" {
                            rows.push((id, name, properties_json));
                        }
                    }
                }
                Ok(rows)
            })
            .await
            .ok()
        })
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|(id, name, properties_json)| {
                let props = serde_json::from_str::<Value>(&properties_json).ok()?;
                let props = self.current_weapon_props(&props);
                Some(CarriedWeaponProfile {
                    weapon: CarriedWeapon::from_props(id, name, &props),
                    props: props.as_object().cloned().unwrap_or_default(),
                })
            })
            .collect()
    }

    fn enabled_weapons(&self) -> Vec<CarriedWeaponProfile> {
        // Every weapon entry the player has on, in the library's order, each
        // prepared as the carried weapons' props are.
        let db = self.db.clone();
        let rows = block_on_pool(&self.runtime, async move {
            db.equipment_library_rows().await.ok()
        })
        .unwrap_or_default();
        rows.into_iter()
            .filter(|(_, _, item_type, _, enabled)| *enabled && item_type == "weapon")
            .filter_map(|(id, name, _, properties_json, _)| {
                let props = serde_json::from_str::<Value>(&properties_json).ok()?;
                let props = self.current_weapon_props(&props);
                Some(CarriedWeaponProfile {
                    weapon: CarriedWeapon::from_props(id, name, &props),
                    props: props.as_object().cloned().unwrap_or_default(),
                })
            })
            .collect()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        // Resolve the configured intent per board-output class off the live
        // config and the equipment library. A class whose id is unset,
        // unknown, or not a harvesting tool resolves to None; a fully
        // empty resolution reads as no guardrail at all. The per-use
        // cost recipe matches the hotbar resolver's "tool" branch, so
        // a guardrail attribution and a hotbar attribution of the same
        // tool always price identically.
        let config = self.reader.current();
        if !config.harvest_guardrail.enabled {
            return None;
        }
        let ids = [
            config.harvest_guardrail.short_tool_id,
            config.harvest_guardrail.long_tool_id,
            config.harvest_guardrail.huge_tool_id,
        ];
        let db = self.db.clone();
        let resolved = block_on_pool(&self.runtime, async move {
            db.with_reader(move |conn| {
                let mut tools: Vec<Option<GuardrailTool>> = Vec::with_capacity(ids.len());
                for id in ids {
                    let tool = match id {
                        None => None,
                        Some(equip_id) => hotbar_equipment_row_sync(conn, equip_id)?.and_then(
                            |(name, item_type, properties_json)| {
                                (item_type == "tool").then(|| {
                                    let (cost_ped, _) = heal_cost_from_props(&properties_json);
                                    GuardrailTool {
                                        name,
                                        cost_per_use_ped: cost_ped,
                                    }
                                })
                            },
                        ),
                    };
                    tools.push(tool);
                }
                Ok(tools)
            })
            .await
            .ok()
        })?;
        let mut sizes = resolved.into_iter();
        let tools = HarvestGuardrailTools {
            short: sizes.next().flatten(),
            long: sizes.next().flatten(),
            huge: sizes.next().flatten(),
        };
        (tools != HarvestGuardrailTools::default()).then_some(tools)
    }

    fn hunting_looter_levels(&self) -> HuntingLooterLevels {
        let Some(game_data) = self.game_data.as_ref() else {
            return HuntingLooterLevels {
                animal: 0.0,
                mutant: 0.0,
                robot: 0.0,
            };
        };
        let db = self.db.clone();
        let levels: Map<String, Value> = block_on_pool(&self.runtime, latest_skill_levels(&db))
            .unwrap_or_default()
            .into_iter()
            .map(|(name, level)| (name, Value::from(level)))
            .collect();
        let professions = all_profession_levels(&levels, game_data.get_entities("professions"));
        let level = |name: &str| professions.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        HuntingLooterLevels {
            animal: level("Animal Looter"),
            mutant: level("Mutant Looter"),
            robot: level("Robot Looter"),
        }
    }
}

/// The live session-capture configuration behind the tracker's config
/// seam: every read borrows the config service's published snapshot,
/// so settings writes are visible immediately and no call touches the
/// filesystem.
struct LiveTrackingConfig {
    reader: ConfigReader,
    machine_id: Option<String>,
}

impl TrackingConfig for LiveTrackingConfig {
    fn session_name(&self) -> String {
        self.reader.current().session_name.clone()
    }

    fn session_definition_id(&self) -> Option<i64> {
        self.reader.current().session_definition_id
    }

    fn declared_skill_boost_percent(&self) -> Option<i64> {
        self.reader.current().declared_skill_boost_percent
    }

    fn manual_mob(&self) -> Option<(String, String)> {
        let config = self.reader.current();
        let species = config.manual_mob_species.trim().to_string();
        let maturity = config.manual_mob_maturity.trim().to_string();
        if species.is_empty() {
            return None;
        }
        Some((species, maturity))
    }

    fn loot_filter_blacklist(&self) -> Vec<String> {
        self.reader.current().loot_filter_blacklist.clone()
    }

    fn passive_effect_sources(&self) -> Vec<eo_services::passive_effects::PassiveEffectSource> {
        self.reader.current().passive_effect_sources.clone()
    }

    fn hotbar_hooks_enabled(&self) -> bool {
        self.reader
            .current()
            .hotbar_keys_observed(self.machine_id.as_deref())
    }

    fn keyless_input(&self) -> bool {
        self.reader
            .current()
            .keyless_input_on(self.machine_id.as_deref())
    }

    fn mob_detection_enabled(&self) -> bool {
        self.reader.current().mob_detection_on()
    }

    fn tool_reading_enabled(&self) -> bool {
        self.reader.current().tool_reading_on()
    }
}

/// Wire the hunt-tracker seams to their live sources: the equipment
/// library over the database, and the
/// session-capture config over the config service's live snapshot.
fn build_providers(
    db: Db,
    reader: ConfigReader,
    machine_id: Option<String>,
    initial_config: &AppConfig,
    runtime: tokio::runtime::Handle,
    game_data: Option<Arc<GameDataStore>>,
    doses: DoseBoard,
) -> Providers {
    Providers {
        equipment: Arc::new(LiveEquipmentLibrary {
            db,
            reader: reader.clone(),
            runtime,
            game_data,
            doses: doses.clone(),
        }),
        config: Arc::new(LiveTrackingConfig { reader, machine_id }),
        player_name: initial_config.player_name.clone(),
        doses,
        // Bound by `compose_producers`, which owns the detector slot.
        detector: Arc::new(eo_services::mob_watch::NoDetector),
        // Filled by the tracker from the database as it starts.
        mob_names: Arc::default(),
        // Bound by `compose_producers`, which owns the held-tool slot.
        tool_detector: Arc::new(eo_services::tool_watch::NoToolDetector),
        items: None,
        bundled_tools: None,
    }
}

/// Run a database future from inside a synchronous provider callback,
/// from either calling context: a runtime worker thread (a facade-driven
/// reload) yields its slot, while a plain producer thread parks. The
/// tracker's own `block_on` uses this exact dual shape.
fn block_on_pool<F: std::future::Future>(handle: &tokio::runtime::Handle, future: F) -> F::Output {
    // Never `Handle::current()`: the provider callbacks run on the
    // chat-log watcher's plain OS thread (no current runtime), so the
    // handle is the one captured at composition time. A runtime worker
    // thread (a facade-driven reload) yields its slot via `block_in_place`;
    // a plain producer thread parks directly. The tracker's and quest
    // service's bus forwarders wait on their reply channels in this same
    // dual shape.
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        handle.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The hook-free keystroke source every compose test injects in place of the
    // real OS hook, so a generic test run never installs the shared hook (whose
    // attach/detach lifecycle can intermittently wedge a headless run).
    use eo_services::keystroke_source::MockKeystrokeSource;

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
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("snapshot")
    }

    /// The repo's planet-map bundle directory (the dev `maps_dir`).
    fn repo_maps() -> PathBuf {
        dev_project_root()
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("maps")
    }

    /// The repo's recogniser model+dict directory (the dev `models_dir`).
    fn repo_models() -> PathBuf {
        dev_project_root()
            .join("app")
            .join("src-tauri")
            .join("entropia-orme")
            .join("resources")
            .join("models")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn composes_over_a_fresh_data_dir_and_the_repo_snapshot() {
        let _ort = ORT_TEST_LOCK.lock().await;
        let dylib = ort_dylib_path(None);
        if dylib.is_file() {
            unsafe {
                std::env::set_var("ORT_DYLIB_PATH", &dylib);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let Composition::Ready(composed) = compose_with(
            dir.path().join("data"),
            repo_snapshot(),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await
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
        let composed = compose_with(
            data_dir,
            repo_snapshot(),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await;
        assert!(
            matches!(
                composed,
                Composition::Declined(Decline {
                    reason: DeclineReason::DatabaseUnreadable,
                    ..
                })
            ),
            "quarantine declines composition"
        );
        assert_eq!(std::fs::read(&db_path).unwrap(), b"not a database");
    }

    #[tokio::test]
    async fn declines_on_a_below_baseline_database() {
        // A database the backend created but never migrated up to the baseline
        // (the hybrid-era first-launch-after-upgrade state). The Python
        // sidecar used to migrate it forward; with it decommissioned there is
        // nothing to do so, so composition now declines cleanly rather than
        // awaiting a migration that will never arrive.
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join(DB_FILE_NAME);
        {
            let db = eo_services::db::Db::open(&db_path).await.unwrap();
            db.with_writer(|conn| {
                conn.execute(
                    "UPDATE db_metadata SET value = '28' WHERE key = 'version'",
                    [],
                )?;
                conn.execute("DROP TABLE _sqlx_migrations", [])?;
                Ok(())
            })
            .await
            .unwrap();
        }
        let composed = compose_with(
            data_dir,
            repo_snapshot(),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await;
        assert!(
            matches!(
                composed,
                Composition::Declined(Decline {
                    reason: DeclineReason::DatabaseBelowBaseline,
                    ..
                })
            ),
            "a below-baseline database declines (nothing migrates it without the sidecar)"
        );
    }

    #[tokio::test]
    async fn declines_on_a_missing_snapshot_dir() {
        let dir = tempfile::tempdir().unwrap();
        let composed = compose_with(
            dir.path().join("data"),
            dir.path().join("no-such-snapshot"),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await;
        assert!(
            matches!(
                composed,
                Composition::Declined(Decline {
                    reason: DeclineReason::GameDataUnavailable,
                    ..
                })
            ),
            "missing snapshot declines composition"
        );
    }

    #[test]
    fn maps_dir_prefers_the_repo_copy_in_dev_builds() {
        let resolved = maps_dir(Some(&PathBuf::from("X:/resources")));
        if cfg!(debug_assertions) {
            assert_eq!(resolved, repo_maps());
        } else {
            assert_eq!(resolved, PathBuf::from("X:/resources").join("maps"));
        }
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
        // The expected leaf mirrors the resolver's platform fork: the
        // Windows onnxruntime-directml build under `ort/`, the Linux
        // build under `ort-linux/`.
        #[cfg(target_os = "linux")]
        let (subdir, file) = ("ort-linux", "libonnxruntime.so");
        #[cfg(not(target_os = "linux"))]
        let (subdir, file) = ("ort", "onnxruntime.dll");
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
            Some(file),
            "the pinned path points at the platform's bundled ONNX Runtime dylib"
        );

        // The expected installed-vs-dev location.
        if cfg!(debug_assertions) {
            assert_eq!(
                resolved,
                dev_project_root()
                    .join("app")
                    .join("src-tauri")
                    .join("entropia-orme")
                    .join("resources")
                    .join(subdir)
                    .join(file),
                "dev resolves to the committed repo dylib"
            );
        } else {
            assert_eq!(
                resolved,
                resource.join(subdir).join(file),
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
        let Composition::Ready(composed) = compose_with(
            dir.path().join("data"),
            repo_snapshot(),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await
        else {
            panic!("composition succeeds regardless of OCR availability");
        };

        match &composed.ocr_engine {
            Some(engine) => {
                let provider = engine.provider();
                #[cfg(target_os = "linux")]
                let gpu_provider = "WebGpuExecutionProvider";
                #[cfg(not(target_os = "linux"))]
                let gpu_provider = "DmlExecutionProvider";
                assert!(
                    provider == gpu_provider || provider == "CPUExecutionProvider",
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
                // On the platforms that bundle a runtime the dylib was
                // verified present above, so a missing engine is a real
                // regression in the composition path (the gate that once
                // skipped OCR off-Windows hid exactly this), not an
                // acceptable optional-faculty outcome.
                if cfg!(any(windows, target_os = "linux")) {
                    panic!(
                        "the committed ONNX Runtime is present on this platform, \
                         yet composition produced no OCR engine"
                    );
                }
                eprintln!(
                    "OCR engine did not load on this host (runtime/model unavailable); \
                     composition still succeeded, which is the optional-faculty contract"
                );
            }
        }

        composed.producers.stop();
    }

    /// The composed scan services are reachable and wired over the LIVE
    /// providers, not the inert defaults: the skill scan rests idle, able
    /// to run exactly when the OCR engine loaded (the inert default has no
    /// recogniser, profile or skill list, so a host where the engine loads
    /// proves the real providers), and the terminal reader is composed
    /// with its profile.
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
        let Composition::Ready(composed) = compose_with(
            dir.path().join("data"),
            repo_snapshot(),
            repo_models(),
            repo_maps(),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await
        else {
            panic!("composition succeeds");
        };

        // Available means the engine, the bundled profile and the bundled
        // skill list all reached the scan.
        assert_eq!(
            composed.skill_scan.status(),
            eo_services::skill_scan::ScanStatus::Idle {
                available: composed.ocr_engine.is_some(),
                stopped: None,
            }
        );
        // The scan composed on the spine bus (its status frames reach the
        // domain bridge); the forwarding is covered by
        // `the_domain_bridge_forwards_typed_envelopes_to_a_subscriber`.

        // With an engine and the bundled profile, the terminal reader is
        // available: whatever it answers, it is not that the reader is
        // unavailable. (A profile at the wrong path would leave it so.)
        if composed.ocr_engine.is_some() {
            let read = composed.api.protection_trade_terminal_scan().unwrap();
            assert_ne!(
                read,
                eo_api::protection::TerminalReadResult::Refused {
                    reason: eo_api::protection::TerminalReadRefusal::ReaderUnavailable,
                },
                "the composed terminal reader has no profile"
            );
        }

        composed.producers.stop();
    }

    /// The interface profile is where composition looks for it, in the
    /// dev layout (beside the repository snapshot) and the installed one
    /// (the bundle map puts it beside the bundled snapshot dir).
    #[test]
    fn the_interface_profile_loads_from_where_composition_looks() {
        let dev = resource_root(&repo_snapshot()).join(UI_PROFILE_FILE);
        assert!(load_ui_profile(&dev).is_some(), "{}", dev.display());

        // Installed, the snapshot dir is `<resources>/snapshot`.
        assert_eq!(
            resource_root(&PathBuf::from("/installed/resources/snapshot")),
            PathBuf::from("/installed/resources")
        );
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            conf["bundle"]["resources"][format!("resources/{UI_PROFILE_FILE}")],
            serde_json::json!(UI_PROFILE_FILE),
            "the installed build bundles the profile beside the snapshot dir"
        );
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
        compose_producers(
            db,
            clock,
            data_dir,
            Some(chatlog),
            Arc::new(MockKeystrokeSource::new()),
            None,
            None,
        )
        .expect("producer spine composes")
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

        // A frozen, plan-advanced clock, exactly the corpus oracle's
        // protocol: the watcher guards its own drain timeout against a
        // frozen clock internally.
        let start =
            chrono::NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Some(start), 0.0));

        let chatlog = data_dir.join("chat_replay.log");
        std::fs::File::create(&chatlog).expect("empty chatlog exists before the watcher starts");

        let producers =
            compose_producers_for_test(db.clone(), clock.clone(), &data_dir, chatlog.clone());
        producers
            .tracker()
            .start_session()
            .await
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
        let readout = producers.tracker().snapshot().await.expect("snapshot");
        let active = readout.active.expect("a session is active");
        assert_eq!(active.kill_count, 2, "two loot groups, two kills");

        producers
            .tracker()
            .stop_session()
            .await
            .expect("session stops");
        producers.stop();

        // The persisted rows match: one session, two kills.
        let session_count: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 0",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(session_count, 1, "one closed session persisted");
        let kill_count: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM kills", [], |row| row.get::<_, i64>(0))?)
            })
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

        let start =
            chrono::NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Some(start), 0.0));

        let chatlog = data_dir.join("chat_replay.log");
        std::fs::File::create(&chatlog).expect("empty chatlog exists before the watcher starts");

        let producers =
            compose_producers_for_test(db.clone(), clock.clone(), &data_dir, chatlog.clone());
        producers
            .tracker()
            .start_session()
            .await
            .expect("composed session starts");

        // Before stop: exactly one active session row exists.
        let active_before: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
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

        let active_after: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(active_after, 0, "stop() ended the open session");
        let ended_set: i64 = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM tracking_sessions WHERE is_active = 0 AND ended_at IS NOT NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(ended_set, 1, "the closed session carries an ended_at stamp");
    }

    /// A spine over a fresh database, for the held-tool watcher's tests.
    async fn held_tool_spine(dir: &std::path::Path) -> (ProducerState, Arc<dyn Clock>) {
        let db = Db::open_adopted(&dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let chatlog = dir.join("chat_replay.log");
        std::fs::File::create(&chatlog).expect("empty chatlog exists before the watcher starts");
        let producers = compose_producers_for_test(db, clock.clone(), dir, chatlog);
        (producers, clock)
    }

    fn turn_tool_reading(producers: &ProducerState, on: bool) {
        let mut updates = serde_json::Map::new();
        updates.insert("tool_reading".into(), Value::Bool(on));
        producers
            .config_service_handle()
            .lock()
            .unwrap()
            .update(&updates)
            .unwrap();
    }

    fn bare_inputs(clock: Arc<dyn Clock>) -> ToolWatchInputs {
        ToolWatchInputs {
            focus: None,
            profile: None,
            read_text: None,
            catalogue: Arc::new(eo_services::tool_watch::ToolCatalogueCell::default()),
            clock,
        }
    }

    /// The held-tool watcher is bound into the slot the tracker was built
    /// with, and says it cannot run without the recogniser and the profile.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_held_tool_watcher_is_bound_into_the_trackers_slot() {
        let dir = tempfile::tempdir().unwrap();
        let (producers, clock) = held_tool_spine(dir.path()).await;
        assert!(producers.tool_detection_handle().bound().is_none());
        let capture = bind_tool_watch(&producers, bare_inputs(clock));
        assert!(
            producers.tool_detection_handle().bound().is_some(),
            "bound into the slot the tracker was built with"
        );
        assert_eq!(
            capture(),
            eo_services::screen_watch::CaptureStatus::Unavailable,
            "without the recogniser and the profile it cannot run"
        );
        producers.stop();
    }

    /// The watcher's own setting probe, and the tracker's, both follow a
    /// live write of the player's setting.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_held_tool_watcher_and_the_tracker_follow_the_live_setting() {
        let dir = tempfile::tempdir().unwrap();
        let (producers, clock) = held_tool_spine(dir.path()).await;
        let reader = producers.config_service_handle().lock().unwrap().reader();
        let providers = tool_watch_providers(reader.clone(), bare_inputs(clock));
        let config = LiveTrackingConfig {
            reader,
            machine_id: None,
        };
        assert!(!(providers.enabled)(), "the watcher is off until chosen");
        assert!(!config.tool_reading_enabled(), "and so is the tracker");
        turn_tool_reading(&producers, true);
        assert!(
            (providers.enabled)(),
            "the watcher follows the player's word"
        );
        assert!(config.tool_reading_enabled());
        turn_tool_reading(&producers, false);
        assert!(!(providers.enabled)(), "and off again");
        assert!(!config.tool_reading_enabled());
        producers.stop();
    }

    /// A held-tool watcher that records what is asked of it.
    #[derive(Default)]
    struct RecordingToolWatch {
        watching: Mutex<Option<String>>,
        shutdowns: Mutex<usize>,
    }

    impl eo_services::tool_watch::ToolDetector for RecordingToolWatch {
        fn start(
            &self,
            session_id: &str,
            _sink: eo_services::tool_watch::ToolPulseSink,
            _ended: eo_services::screen_watch::WatchEnded,
        ) {
            *self.watching.lock().unwrap() = Some(session_id.to_string());
        }

        fn stop(&self) {
            *self.watching.lock().unwrap() = None;
        }

        fn watches(&self, session_id: &str) -> bool {
            self.watching.lock().unwrap().as_deref() == Some(session_id)
        }

        fn permit_capture(&self) {}

        fn hint(&self) {}

        fn shutdown(&self) {
            *self.shutdowns.lock().unwrap() += 1;
            self.stop();
        }
    }

    /// A watch running for a session at exit is stopped, and the exit path
    /// reaches the watcher's shutdown, which waits for a look in progress.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_held_tool_watch_is_stopped_and_shut_down_with_the_spine() {
        let dir = tempfile::tempdir().unwrap();
        let (producers, _clock) = held_tool_spine(dir.path()).await;
        let watch = Arc::new(RecordingToolWatch::default());
        let detector: Arc<dyn eo_services::tool_watch::ToolDetector> = watch.clone();
        producers.tool_detection_handle().bind(detector);
        turn_tool_reading(&producers, true);
        producers.tracker().reload_config().await;
        producers
            .tracker()
            .start_session()
            .await
            .expect("composed session starts");
        assert!(
            watch.watching.lock().unwrap().is_some(),
            "with the setting on, a session starts the watch"
        );

        producers.stop();
        assert_eq!(watch.watching.lock().unwrap().as_deref(), None, "stopped");
        assert_eq!(*watch.shutdowns.lock().unwrap(), 1, "shut down at exit");
    }

    /// Seed a weapon row directly through the shared handle, the same
    /// shape `Db::weapon_properties_by_name_fragment` reads (item_type
    /// 'weapon', a name carrying the lookup fragment, a JSON-object
    /// properties blob).
    async fn seed_weapon(db: &Db, id: i64, name: &str, properties_json: &str) {
        let name = name.to_string();
        let properties_json = properties_json.to_string();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO equipment_library (id, name, item_type, properties_json) \
                 VALUES (?1, ?2, 'weapon', ?3)",
                rusqlite::params![id, name, properties_json],
            )?;
            Ok(())
        })
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
    /// - equipment.weapon_profile returns the parsed property object for a
    ///   weapon matched by name fragment (kills the deleted `Ok(Object)`
    ///   match arm and the `Default::default()` whole-function replacement,
    ///   both of which yield `None`).
    /// - equipment.cost_per_shot returns `totalCostPerUse / 100`; the seeded
    ///   props make totalCostPerUse == 250 so the expected 2.5 differs from
    ///   both `250 % 100` (50.0) and `250 * 100` (25000.0): kills the `/`->`%`
    ///   and `/`->`*` mutants.
    /// - the facet reads carry the stored values through unchanged
    ///   (session_name verbatim, declared_skill_boost_percent as stored).
    /// - carried_weapons reads the hotbar's weapons in slot order, then the
    ///   carried ones, each once, skipping non-weapons and absent rows, with
    ///   each weapon's damage band from its stored props.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_providers_transforms_pin_their_exact_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");

        // A weapon whose name carries the fragment "Korss", with a
        // property object whose economy yields a known totalCostPerUse.
        // ammo_burn 25000 -> 250 PEC ammo at markup 1.0, decay 0 ->
        // totalCostPerUse == 250, so equipment.cost_per_shot == 250/100 == 2.5.
        let props = serde_json::json!({
            "weapon_entity": {"economy": {"decay": 0.0, "ammo_burn": 25000}},
            "weapon_markup": 100,
        });
        seed_weapon(
            &db,
            1,
            "Korss H400 (L)",
            &serde_json::to_string(&props).unwrap(),
        )
        .await;

        // A second weapon with a damage figure, and a healer.
        seed_weapon(
            &db,
            2,
            "Opalo",
            &serde_json::json!({"weapon_entity": {"damage": {"impact": 20.0}}}).to_string(),
        )
        .await;
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO equipment_library (id, name, item_type, properties_json) \
                 VALUES (3, 'FAP', 'healing', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("healer seeds");

        // First on-disk config: a named session with a boost, hotbar
        // hooks ENABLED, the Korss and the healer bound, and the Opalo,
        // the Korss again, and a sold weapon carried without a slot.
        write_settings(
            &data_dir,
            &serde_json::json!({
                "session_name": "ARIS Dailies",
                "declared_skill_boost_percent": 50,
                "hotbar_hooks_enabled": true,
                "hotbar": {"2": 1, "3": 3},
                "carried_weapon_ids": [2, 1, 99],
            }),
        );
        let config = load_config_readonly(&data_dir).expect("config reads");
        let mut config_service = ConfigService::new(&data_dir).expect("config service");
        let providers = build_providers(
            db.clone(),
            config_service.reader(),
            None,
            &config,
            tokio::runtime::Handle::current(),
            None,
            DoseBoard::new(Arc::new(eo_services::clock::RealClock::new())),
        );

        // The equipment seam: the parsed property object, by fragment.
        let profile = providers
            .equipment
            .weapon_profile("Korss")
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

        // cost_per_shot: totalCostPerUse (250) / 100 == 2.5.
        let cost = providers.equipment.cost_per_shot("Korss");
        assert!(
            (cost - 2.5).abs() < 1e-9,
            "per-shot cost is totalCostPerUse/100 == 2.5, not {cost} \
             (% would be 50.0, * would be 25000.0)"
        );

        // The carried weapons: the slotted Korss, then the carried Opalo;
        // the healer, the repeat, and the sold weapon carry nothing.
        let carried: Vec<(i64, String, Option<eo_services::tracker::DamageBand>)> = providers
            .equipment
            .carried_weapons()
            .into_iter()
            .map(|profile| {
                (
                    profile.weapon.equipment_id,
                    profile.weapon.name,
                    profile.weapon.band,
                )
            })
            .collect();
        assert_eq!(
            carried,
            vec![
                (1, "Korss H400 (L)".to_string(), None),
                (
                    2,
                    "Opalo".to_string(),
                    Some(eo_services::tracker::DamageBand {
                        min: 10.0,
                        max: 20.0
                    })
                ),
            ]
        );

        // The config seam reads the live snapshot: the facets come
        // through verbatim.
        assert_eq!(providers.config.session_name(), "ARIS Dailies");
        assert_eq!(providers.config.declared_skill_boost_percent(), Some(50));

        // A config-service write publishes a new snapshot, and the SAME
        // seams follow it.
        let mut updates = serde_json::Map::new();
        updates.insert("session_name".into(), serde_json::json!("Solo Run"));
        updates.insert(
            "declared_skill_boost_percent".into(),
            serde_json::json!(100),
        );
        updates.insert("carried_weapon_ids".into(), serde_json::json!([]));
        config_service.update(&updates).expect("settings write");
        assert_eq!(providers.config.session_name(), "Solo Run");
        assert_eq!(providers.config.declared_skill_boost_percent(), Some(100));
        assert_eq!(providers.equipment.carried_weapons().len(), 1);

        // Switching to an empty preset parks the Korss, and a weapon on a
        // parked preset is still carried.
        let mut updates = serde_json::Map::new();
        updates.insert("active_hotbar_preset".into(), serde_json::json!(2));
        config_service.update(&updates).expect("preset switch");
        let carried: Vec<i64> = providers
            .equipment
            .carried_weapons()
            .into_iter()
            .map(|profile| profile.weapon.equipment_id)
            .collect();
        assert_eq!(carried, vec![1]);
    }

    /// REGRESSION: the equipment seam must resolve from a plain OS
    /// thread with NO current tokio runtime. Its production caller is
    /// now the tracker's actor task (a runtime context, bridged via
    /// `block_in_place`), but the seam's `block_on_pool` keeps the
    /// composition-time handle rather than `Handle::current()` (which
    /// panics off-runtime), so it stays robust from either context.
    /// Build the providers inside the runtime, then invoke the lookup
    /// from a plain `std::thread` and assert it resolves.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn equipment_provider_resolves_from_a_non_runtime_thread() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        let db = Db::open_adopted(&data_dir.join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");
        let props = serde_json::json!({
            "weapon_entity": {"economy": {"decay": 0.0, "ammo_burn": 25000}},
            "weapon_markup": 100,
        });
        seed_weapon(
            &db,
            1,
            "Korss H400 (L)",
            &serde_json::to_string(&props).unwrap(),
        )
        .await;
        write_settings(&data_dir, &serde_json::json!({}));
        let config = load_config_readonly(&data_dir).expect("config reads");
        let config_service = ConfigService::new(&data_dir).expect("config service");
        let providers = build_providers(
            db.clone(),
            config_service.reader(),
            None,
            &config,
            tokio::runtime::Handle::current(),
            None,
            DoseBoard::new(Arc::new(eo_services::clock::RealClock::new())),
        );

        // Invoke the lookup AND the derived cost from a plain OS thread
        // (no current runtime).
        let equipment = providers.equipment.clone();
        let outcome = std::thread::spawn(move || {
            let resolved = equipment.weapon_profile("Korss").is_some();
            let cost = equipment.cost_per_shot("Korss");
            (resolved, cost)
        })
        .join()
        .expect("the provider must not panic off-runtime");
        assert!(
            outcome.0,
            "equipment.weapon_profile resolves from a non-runtime thread"
        );
        assert!(
            (outcome.1 - 2.5).abs() < 1e-9,
            "equipment.cost_per_shot resolves off-runtime to 2.5, got {}",
            outcome.1
        );
    }

    /// The single-owner core tolerates a facade-shaped read concurrent
    /// with a producer-shaped write without deadlock and within a bounded
    /// latency: the writer thread serialises the write, while a reader
    /// thread serves the concurrent read against the WAL, so the read
    /// simply queues behind the write rather than locking.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_read_during_a_producer_write_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_adopted(&dir.path().join(DB_FILE_NAME))
            .await
            .expect("fresh database adopts");

        // A producer-shaped write: a short transaction holding the writer
        // connection briefly, exactly the shape of a tracker persistence
        // write.
        let writer = {
            let db = db.clone();
            tokio::spawn(async move {
                db.with_writer(|conn| {
                    let tx = conn.transaction()?;
                    tx.execute(
                        "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('cc-test', 0, 0)",
                        [],
                    )?;
                    // Hold the writer connection a moment so the read genuinely contends.
                    std::thread::sleep(Duration::from_millis(50));
                    tx.commit()?;
                    Ok(())
                })
                .await
                .expect("write under transaction");
            })
        };

        // A facade-shaped read on the same handle, bounded by a generous
        // deadline: if the single connection deadlocked, this would
        // time out.
        let read = tokio::time::timeout(Duration::from_secs(5), async {
            db.with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .expect("read query")
        })
        .await;
        assert!(
            read.is_ok(),
            "the concurrent read completed without deadlock"
        );

        writer.await.expect("writer task joins");
        let final_count = db
            .with_reader(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM tracking_sessions", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(final_count, 1, "the write committed");
    }

    #[tokio::test]
    async fn the_domain_bridge_forwards_typed_envelopes_to_a_subscriber() {
        use eo_services::bus_events::BusEvent;
        use eo_wire::domain_events::{
            ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
            TrackingReason, TrackingSessionUpdated, TrackingSessionUpdatedPayload,
            TrackingSessionUpdatedTag, TrackingStatus,
        };

        let bus = EventBus::new();
        let domain_bus = Arc::new(DomainBus::new(DOMAIN_BUS_CAPACITY));
        subscribe_domain_bridge(&bus, &domain_bus);
        let mut receiver = domain_bus.subscribe();

        // A tracking envelope published on the bus (typed end to end,
        // exactly as the tracker publishes it) is forwarded to the
        // channel's subscriber unchanged.
        let tracking = TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: "2026-01-01T00:00:00Z".to_string(),
            payload: TrackingSessionUpdatedPayload {
                session_id: Some("s1".to_string()),
                status: TrackingStatus::Active,
                reason: TrackingReason::Started,
            },
        };
        bus.publish(&BusEvent::TrackingSessionUpdated(tracking.clone()));
        assert_eq!(
            receiver.recv().await.expect("the envelope arrives"),
            DomainEvent::TrackingSessionUpdated(tracking)
        );

        // The other bridged topic is delivered too, on the same channel.
        let scan = ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2026-01-01T00:00:01Z".to_string(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Capturing,
            },
        };
        bus.publish(&BusEvent::ScanStatusChanged(scan.clone()));
        assert_eq!(
            receiver.recv().await.expect("the envelope arrives"),
            DomainEvent::ScanStatusChanged(scan)
        );
    }

    /// The LIVE scan-status path now the scan composes on the spine bus: a
    /// status-moving verb on a `SkillScanService` built over the same bus the
    /// bridge subscribes reaches a domain-channel subscriber as a typed
    /// `scan.status.changed` envelope, end to end (the bridge-forwards test
    /// publishes the event directly; this proves the scan's own publish
    /// flows through it).
    #[tokio::test]
    async fn the_composed_scan_delivers_a_status_envelope_to_a_subscriber() {
        use eo_wire::domain_events::ScanPhase;

        let bus = Arc::new(EventBus::new());
        let domain_bus = Arc::new(DomainBus::new(DOMAIN_BUS_CAPACITY));
        subscribe_domain_bridge(&bus, &domain_bus);
        let mut receiver = domain_bus.subscribe();

        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let scan = SkillScanService::new(
            SkillScanProviders {
                frame: Arc::new(|_| {
                    Err(eo_services::game_ui::seams::FrameUnavailable::WindowNotFound)
                }),
                profile: load_ui_profile(&resource_root(&repo_snapshot()).join(UI_PROFILE_FILE)),
                read_text: Some(Arc::new(|_: &BgrImage| Some((String::new(), 0.0)))),
                vocabulary: SkillVocabulary::new(vec!["Agility".into()], Vec::new()),
                tick: DEFAULT_TICK,
                idle_limit: DEFAULT_IDLE_LIMIT,
            },
            clock,
            Some(bus.clone()),
        );
        // `start` moves the status idle -> collecting, publishing one envelope.
        scan.surface_shown();
        scan.start().expect("the scan starts");
        let event = receiver.recv().await.expect("the envelope arrives");
        let DomainEvent::ScanStatusChanged(envelope) = event else {
            panic!("the scan publish routed to the wrong variant: {event:?}");
        };
        assert_eq!(envelope.payload.phase, ScanPhase::Capturing);
        scan.shutdown();
    }

    /// The skill scan's composed completion writes each accepted level to
    /// the database stamped with when its page was read, not when the
    /// review was accepted: a gain the chat log recorded after a page was
    /// read stays the current level.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_composed_scan_completion_writes_each_level_at_its_read_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join(DB_FILE_NAME)).await.unwrap();
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                 VALUES ('Rifle', 4000.60, 'chatlog', 160.0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let complete = skill_scan_completion(db.clone(), tokio::runtime::Handle::current());
        let levels = vec![
            ScannedLevel {
                skill: "Rifle".into(),
                level: 4000.10,
                scanned_at: 100.0,
            },
            ScannedLevel {
                skill: "Aim".into(),
                level: 1268.53,
                scanned_at: 220.0,
            },
        ];
        tokio::task::spawn_blocking(move || complete(&levels))
            .await
            .unwrap()
            .expect("the levels are written");
        let rows: Vec<(String, f64, f64)> = db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT skill_name, level, scanned_at FROM skill_calibrations \
                     WHERE source = 'scan' ORDER BY scanned_at",
                )?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("Rifle".to_string(), 4000.10, 100.0),
                ("Aim".to_string(), 1268.53, 220.0),
            ]
        );
        let mut current = latest_skill_levels(&db).await.unwrap();
        current.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            current,
            vec![("Aim".to_string(), 1268.53), ("Rifle".to_string(), 4000.60)]
        );
    }
}
