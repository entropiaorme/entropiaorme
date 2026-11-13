//! The IPC facade: the application boundary the typed Tauri commands
//! call into.
//!
//! Each operation the frontend invokes is one async method on [`Api`],
//! taking and returning the DTO types defined beside it (plain `serde`
//! structs with JSON-Schema derives). The shell wraps every method in a
//! thin `#[tauri::command]`; the TypeScript bindings for the DTOs and
//! the command signatures are generated from this crate by `cargo xtask
//! gen-ts`, so the wire contract has a single Rust source.
//!
//! The facade is built whole from the composed services once the
//! database has opened (construct-then-share): every handle is present
//! by value, there is no half-initialised state to observe, and the
//! shell publishes the finished value to the IPC layer in one step.
//! This crate owns the entire backend operation surface; it replaced
//! the in-process HTTP router the migration era ran on, since deleted
//! (ADR-0019).

// The tracking snapshot's `json!` literal expands one level per field
// and outgrew the default limit at 43; the literal stays declarative
// rather than assembling the map imperatively around a macro artefact.
#![recursion_limit = "256"]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eo_services::analytics::AnalyticsService;
use eo_services::attack_rate::WeaponPricing;
use eo_services::auction_fee_research::AuctionFeeResearchService;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::Clock;
use eo_services::codex::CodexService;
use eo_services::config_service::{load_config_readonly, ConfigService};
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::healing_review::HealingReviewService;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::market_service::MarketService;
use eo_services::protection::ProtectionService;
use eo_services::quests::QuestService;
use eo_services::repair_ocr::RepairOcrService;
use eo_services::sale_capture::SaleCaptureService;
use eo_services::skill_scan::SkillScanService;
use eo_services::skill_tracker::SkillTracker;
use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use eo_services::tracker::HuntTracker;
use eo_services::weapon_review::WeaponReviewService;

pub mod activities;
pub mod analytics;
pub mod character;
pub mod codex;
pub mod consumables;
pub mod dev;
pub mod equipment;
mod error;
pub mod healing;
pub mod held_tool;
pub mod loadout_comparison;
pub mod manifest;
pub mod maps;
pub mod market;
pub mod mob_detection;
mod nullable;
pub mod protection;
pub mod quests;
pub mod sale_capture;
pub mod scan;
pub mod session_definitions;
pub mod settings;
pub mod skilling;
pub mod tool_sighting;
pub mod tracking;
pub mod weapons;

pub use error::ApiError;
pub use nullable::Nullable;

/// The composed application facade the typed commands dispatch into.
pub struct Api {
    db: Db,
    game_data: Arc<GameDataStore>,
    /// The injectable wall clock: the calibration-staleness read compares
    /// the latest scan against it.
    clock: Arc<dyn Clock>,
    /// The resolved data directory: configuration read-through
    /// (`settings.json`) for the operations that consult it.
    data_dir: PathBuf,
    /// The sole settings writer: the settings-write operations lock and
    /// save through it, so there is no second writer to lose updates.
    config_service: Arc<Mutex<ConfigService>>,
    /// The live hunt tracker: a settings write re-signals it so an
    /// in-flight session re-reads its config, and a codex claim checks it
    /// before suppressing the claimed skill's next gain.
    tracker: Arc<HuntTracker>,
    /// The hotbar listener: a `hotbar_hooks_enabled` change flips its gate.
    hotbar: Arc<HotbarListener>,
    /// The chat-log watcher: a `chatlog_path` change restarts its tail.
    watcher: Arc<ChatlogWatcher>,
    /// The skill tracker: a codex claim suppresses the claimed skill's
    /// next gain on it while a session is live.
    skill_tracker: Arc<SkillTracker>,
    /// The skill scan: the scan family's verbs drive it and read its
    /// status.
    skill_scan: Arc<SkillScanService>,
    /// The spacebar-capture listener: the development auction-fee
    /// research capture arms it.
    spacebar: Arc<SpacebarCaptureListener>,
    /// The one-shot repair-cost OCR: the tracking family's repair-scan leg
    /// drives it (gated on the `repair_ocr_enabled` config flag).
    repair_ocr: Arc<RepairOcrService>,
    /// The one-shot capture of the auction's sell queue: the inventory
    /// family's listing capture drives it, filling drafts the player reviews
    /// before anything is recorded.
    sale_capture: Arc<SaleCaptureService>,
    /// Development-only auction-fee sampling over the production sell-queue
    /// capture. Its Space action is installed on the existing listener during
    /// construction, so there is no second OS hook or reading path.
    auction_fee_research: Arc<AuctionFeeResearchService>,
    /// The last capture taken from the floating capture button, waiting to
    /// be collected.
    ///
    /// Its whole purpose is to bridge the gap between the overlay's capture
    /// button and the main window's form: the form may not be on screen when
    /// the button is pressed, so the drafts wait here until it opens. Taken
    /// once and cleared, and never persisted, because a capture nobody came
    /// back for is one worth taking again.
    last_sale_capture: std::sync::Mutex<Option<sale_capture::SaleQueueCapture>>,
    /// How a stored weapon's props are prepared before pricing or showing
    /// them: the attack rate under the reload speed the declared passive
    /// effects put in force. Shared with the weapon review service, so
    /// Equipment, review, and live tracking price a weapon alike.
    weapon_pricing: WeaponPricing,
    /// The codex service (species / ranks / recommendations / claims),
    /// built over the facade's shared db, catalogue, and clock.
    codex: CodexService,
    /// The composed quest service (quest CRUD, lifecycle,
    /// analytics): the same instance whose owning task carries the
    /// bus-fed flows (session tracking, mission auto-start, reward
    /// suppression), so the command surface and the producer spine
    /// share one service.
    quests: Arc<QuestService>,
    /// The analytics service (Overview / Activity aggregates, ledger,
    /// presets, inventory), built over the facade's shared db and clock.
    analytics: AnalyticsService,
    /// The market service (markup-observation paste feed + reads), built
    /// over the facade's shared db and clock. An informational layer
    /// only: nothing here feeds the ledger or any realised P&L figure.
    market: MarketService,
    /// Protection catalogue, live default, and limited-layer
    /// reconciliation over the shared database and injected clock.
    protection: ProtectionService,
    /// Post-play healing corrections over the shared database and injected
    /// clock.
    healing_review: HealingReviewService,
    /// Post-play weapon review and the assignment of unpriced shots, over
    /// the shared database and injected clock.
    weapon_review: WeaponReviewService,
    /// The review of which creatures a session's kills were detected as,
    /// over the catalogue the creature watcher matches names with.
    mob_review: eo_services::mob_review::MobReviewService,
    /// The review of what the screen read as the tool in hand that is not
    /// in the library, over the shared database and injected clock.
    tool_review: eo_services::tool_review::ToolReviewService,
    /// Whether creature detection can capture the game window, for the
    /// settings; `None` without a watcher (detection then cannot run).
    mob_capture: Option<eo_services::mob_watch::CaptureStatusProbe>,
    /// How names read off the held-tool bar are told apart, from the
    /// interface profile: the library read groups the entries the bar
    /// cannot tell apart by it. `None` without a profile (the library then
    /// reports no groups).
    tool_names: Option<eo_services::game_ui::tool_names::ToolNameProfile>,
    /// The tools the held-tool watcher matches names against, rebuilt when
    /// the library changes; `None` without a watcher.
    tool_catalogues: Option<eo_services::tool_catalogue::ToolCatalogues>,
    /// Whether reading the held tool can capture the game window, for the
    /// settings; `None` without a watcher (the reading then cannot run).
    tool_capture: Option<eo_services::screen_watch::CaptureStatusProbe>,
    /// The session-definition service (definition + roster lifecycle), built
    /// over the facade's shared db and clock; the tracking family's
    /// selection verb validates against it.
    session_definitions: Arc<eo_services::session_definitions::SessionDefinitionService>,
    /// Serialises selection with definition lifecycle transitions so a
    /// selection validated as active cannot be written after Archive commits.
    definition_transition: tokio::sync::Mutex<()>,
    /// The cartography-pin service (pin CRUD), built over the facade's
    /// shared db and clock; the facade adds the map-bounds gate on top.
    map_pins: eo_services::map_pins::MapPinsService,
    /// The pin-configuration service (the per-preset palette; pins are
    /// instances of a configuration), built over the same db and clock.
    pin_configs: eo_services::pin_configs::PinConfigsService,
    /// The bundled planet-map catalogue (a shipped resource), or `None`
    /// on a facade built without it (the maps family then serves an
    /// empty catalogue and the raster fetch reports unavailable).
    planet_maps: Option<Arc<eo_services::planet_maps::PlanetMapStore>>,
    /// The coordinate-capture service (the maps calibration flow and
    /// the one-shot coordinate scan), or `None` on a facade built
    /// without the native capture seams (those commands then report
    /// unavailable).
    coord_capture: Option<Arc<eo_services::coord_capture::CoordCaptureService>>,
    /// Persisted route navigation and radar guidance, composed only when
    /// the native capture and producer seams are available.
    navigation: Option<Arc<eo_services::navigation::NavigationService>>,
    input: settings::InputAccess,
}

/// Weapon pricing over the live config and the running doses: the reload
/// speed in effect, read at each pricing. A poisoned config lock leaves the
/// doses alone in effect.
fn weapon_pricing(
    config_service: &Arc<Mutex<ConfigService>>,
    game_data: Arc<GameDataStore>,
    doses: eo_services::consumables::DoseBoard,
) -> WeaponPricing {
    let reader = config_service.lock().ok().map(|service| service.reader());
    WeaponPricing::new(
        Some(game_data),
        Arc::new(move || {
            let sources = reader
                .as_ref()
                .map(|reader| reader.current().passive_effect_sources.clone())
                .unwrap_or_default();
            doses.reload_speed_percent(&sources)
        }),
    )
}

impl Api {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Db,
        game_data: Arc<GameDataStore>,
        clock: Arc<dyn Clock>,
        data_dir: PathBuf,
        config_service: Arc<Mutex<ConfigService>>,
        tracker: Arc<HuntTracker>,
        hotbar: Arc<HotbarListener>,
        watcher: Arc<ChatlogWatcher>,
        skill_tracker: Arc<SkillTracker>,
        skill_scan: Arc<SkillScanService>,
        spacebar: Arc<SpacebarCaptureListener>,
        repair_ocr: Arc<RepairOcrService>,
        sale_capture: Arc<SaleCaptureService>,
        quests: Arc<QuestService>,
        planet_maps: Option<Arc<eo_services::planet_maps::PlanetMapStore>>,
        coord_capture: Option<Arc<eo_services::coord_capture::CoordCaptureService>>,
        navigation: Option<Arc<eo_services::navigation::NavigationService>>,
    ) -> Self {
        let codex = codex::build_codex_service(db.clone(), game_data.clone(), clock.clone());
        let analytics = AnalyticsService::new(db.clone(), clock.clone());
        let market = MarketService::new(db.clone(), clock.clone());
        let protection = ProtectionService::new(db.clone(), clock.clone());
        let healing_review = HealingReviewService::new(db.clone(), clock.clone());
        let weapon_pricing =
            weapon_pricing(&config_service, game_data.clone(), tracker.dose_board());
        // Until composition shares the watcher's catalogue and the name
        // profile, the review lists and puts back declarations, but refuses
        // a typed name it cannot check.
        let mob_review = eo_services::mob_review::MobReviewService::new(
            db.clone(),
            clock.clone(),
            Arc::new(eo_services::mob_watch::CatalogueCell::default()),
            Some(game_data.clone()),
            None,
        )
        .with_labels(tracker.mob_names());
        let weapon_review = WeaponReviewService::new(db.clone(), clock.clone())
            .with_pricing(weapon_pricing.clone());
        let tool_review =
            eo_services::tool_review::ToolReviewService::new(db.clone(), clock.clone());
        let session_definitions = eo_services::session_definitions::SessionDefinitionService::new(
            db.clone(),
            clock.clone(),
        );
        let map_pins = eo_services::map_pins::MapPinsService::new(db.clone(), clock.clone());
        let pin_configs =
            eo_services::pin_configs::PinConfigsService::new(db.clone(), clock.clone());
        let auction_fee_research =
            AuctionFeeResearchService::new(sale_capture.clone(), clock.clone(), &data_dir);
        let research = auction_fee_research.clone();
        let research_data_dir = data_dir.clone();
        let research_spacebar = Arc::downgrade(&spacebar);
        spacebar.set_research_capture(Arc::new(move || {
            let developer_mode = load_config_readonly(&research_data_dir)
                .map(|config| config.developer_mode_enabled)
                .unwrap_or(false);
            if !developer_mode {
                if let Some(spacebar) = research_spacebar.upgrade() {
                    spacebar.set_research_enabled(false);
                }
                research.stop();
                return;
            }
            let _ = research.capture();
        }));
        Self {
            db,
            game_data,
            clock,
            data_dir,
            config_service,
            tracker,
            hotbar,
            watcher,
            skill_tracker,
            skill_scan,
            spacebar,
            repair_ocr,
            sale_capture,
            auction_fee_research,
            last_sale_capture: std::sync::Mutex::new(None),
            codex,
            quests,
            analytics,
            market,
            protection,
            healing_review,
            weapon_review,
            mob_review,
            tool_review,
            mob_capture: None,
            tool_names: None,
            tool_catalogues: None,
            tool_capture: None,
            weapon_pricing,
            session_definitions,
            definition_transition: tokio::sync::Mutex::new(()),
            map_pins,
            pin_configs,
            planet_maps,
            coord_capture,
            navigation,
            input: settings::InputAccess::inert(),
        }
    }

    /// Review detected creatures over `catalogue`, the one the creature
    /// watcher matches names with, checking typed names by `names`: a name
    /// the player confirms is matched from the next look on. `capture`
    /// tells the settings whether the watcher can capture.
    pub fn with_mob_detection(
        mut self,
        catalogue: Arc<eo_services::mob_watch::CatalogueCell>,
        names: Option<eo_services::game_ui::mob_names::MobNameProfile>,
        capture: Option<eo_services::mob_watch::CaptureStatusProbe>,
    ) -> Self {
        self.mob_capture = capture;
        self.mob_review = eo_services::mob_review::MobReviewService::new(
            self.db.clone(),
            self.clock.clone(),
            catalogue,
            Some(self.game_data.clone()),
            names,
        )
        .with_labels(self.tracker.mob_names());
        self
    }

    /// Group the library entries the held-tool bar cannot tell apart by
    /// `names`, the interface profile's rules for reading tool names.
    pub fn with_tool_names(
        mut self,
        names: Option<eo_services::game_ui::tool_names::ToolNameProfile>,
    ) -> Self {
        self.tool_names = names;
        self
    }

    /// Rebuild `catalogues`, the tools the held-tool watcher matches names
    /// against, whenever the library changes. `capture` tells the settings
    /// whether the watcher can capture.
    pub fn with_tool_reading(
        mut self,
        catalogues: eo_services::tool_catalogue::ToolCatalogues,
        capture: Option<eo_services::screen_watch::CaptureStatusProbe>,
    ) -> Self {
        self.tool_catalogues = Some(catalogues);
        self.tool_capture = capture;
        self
    }

    /// Attach this machine's input access: its identity, which keyless
    /// tracking is held to, and the shared keyboard source the setting
    /// suppresses. Without it keyless tracking cannot be turned on.
    pub fn with_input_access(mut self, input: settings::InputAccess) -> Self {
        self.input = input;
        self
    }

    /// Announce every committed protection write through `changed`, so
    /// surfaces showing armour costs can re-read them.
    pub fn with_protection_changed(
        mut self,
        changed: eo_services::protection::ChangedSink,
    ) -> Self {
        self.protection = self.protection.with_changed(changed);
        self
    }

    /// Announce every committed healing correction through `changed`, so
    /// the tracker re-reads its live effect windows and surfaces showing
    /// heal costs re-read them.
    pub fn with_healing_changed(
        mut self,
        changed: eo_services::healing_review::ChangedSink,
    ) -> Self {
        self.healing_review = self.healing_review.with_changed(changed);
        self
    }

    /// Announce every committed market write through `changed`, so surfaces
    /// valuing loot at market markup can re-read.
    pub fn with_market_changed(
        mut self,
        changed: eo_services::market_service::ChangedSink,
    ) -> Self {
        self.market = self.market.with_changed(changed);
        self
    }

    /// Announce every committed weapon assignment through `changed`, so
    /// surfaces showing weapon costs re-read them.
    pub fn with_weapons_changed(
        mut self,
        changed: eo_services::weapon_review::ChangedSink,
    ) -> Self {
        self.weapon_review = self.weapon_review.with_changed(changed);
        self
    }
}
