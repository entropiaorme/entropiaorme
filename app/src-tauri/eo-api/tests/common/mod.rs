//! Shared test scaffolding for the facade integration tests.
//!
//! The facade constructor takes the migrated write families' producer
//! handles (the config writer, the hunt tracker, the hotbar gate, the
//! chat-log watcher, the skill tracker) by value under the
//! construct-then-share invariant. Families that do not exercise those
//! surfaces still need them present, so this builds a minimal, inert set
//! over a fresh event bus: the listeners spawn no threads until started,
//! and the trackers are constructed but never driven.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eo_services::chatlog_time::ChatLogClock;
use eo_services::chatlog_watcher::ChatlogWatcher;
use eo_services::clock::RealClock;
use eo_services::config_service::ConfigService;
use eo_services::db::Db;
use eo_services::event_bus::EventBus;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::quests::QuestService;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders};
use eo_services::sale_window_ocr::{SaleWindowOcrService, SaleWindowProviders};
use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
use eo_services::skill_tracker::SkillTracker;
use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use eo_services::tracker::{HuntTracker, Providers};

/// The write-family producer handles a facade under test takes by value,
/// plus the manual-scan state machine and its spacebar listener (inert:
/// default providers report no engine and no game window, so the scan
/// answers its resting status the way the app does before any scan).
pub struct ProducerHandles {
    pub config_service: Arc<Mutex<ConfigService>>,
    pub tracker: Arc<HuntTracker>,
    pub hotbar: Arc<HotbarListener>,
    pub watcher: Arc<ChatlogWatcher>,
    pub skill_tracker: Arc<SkillTracker>,
    // The scan family's own facade test builds a scan over controllable
    // providers instead of these inert defaults, so it alone leaves the
    // two fields unread; every other facade test binary passes them to
    // `Api::new`.
    #[allow(dead_code)]
    pub skill_scan: Arc<SkillScanManual>,
    #[allow(dead_code)]
    pub spacebar: Arc<SpacebarCaptureListener>,
    // The tracking family's own facade test exercises repair-scan through
    // this handle; every other facade test binary passes it inertly to
    // `Api::new` and never reads it.
    #[allow(dead_code)]
    pub repair_ocr: Arc<RepairOcrService>,
    pub sale_window_ocr: Arc<SaleWindowOcrService>,
    // The composed quest service (its owning task subscribed on this
    // module's bus); the facade serves the quest families over it.
    pub quests: Arc<QuestService>,
}

/// Build the write-family producer handles for a facade under test.
/// `handle` is the runtime the trackers schedule onto (`Handle::current()`
/// inside a `#[tokio::test]`, or a built runtime's handle in a sync
/// harness). The trackers share the caller's real database handle (clones
/// of the one core), so their write spine runs on the synchronous core.
pub async fn producer_handles(
    db: &Db,
    data_dir: &Path,
    handle: tokio::runtime::Handle,
) -> ProducerHandles {
    producer_handles_with_tracker(db, data_dir, handle, Providers::default()).await
}

/// [`producer_handles`] with the tracker's providers supplied, for tests
/// whose sessions must snapshot real config facets at start (the default
/// harness composes the inert config, so its sessions never carry a
/// name). Unread by the test binaries that keep the inert default.
#[allow(dead_code)]
pub async fn producer_handles_with_tracker(
    db: &Db,
    data_dir: &Path,
    handle: tokio::runtime::Handle,
    providers: Providers,
) -> ProducerHandles {
    let bus = Arc::new(EventBus::new());
    let config_service = Arc::new(Mutex::new(
        ConfigService::new(data_dir).expect("config service"),
    ));
    let chatlog_clock = ChatLogClock::host_local();
    let tracker = HuntTracker::new(
        bus.clone(),
        db.clone(),
        Arc::new(RealClock::new()),
        chatlog_clock.clone(),
        providers,
    )
    .await
    .expect("tracker");
    let hotbar = HotbarListener::new(bus.clone(), None, None, None);
    let watcher = Arc::new(ChatlogWatcher::new(
        bus.clone(),
        data_dir.join("chat.log"),
        None,
        chatlog_clock.clone(),
    ));
    let skill_tracker =
        SkillTracker::new(&bus, db.clone(), Arc::new(RealClock::new()), chatlog_clock);
    let skill_scan = SkillScanManual::new(
        ScanProviders::default(),
        Arc::new(RealClock::new()),
        None,
        None,
        0,
    );
    let spacebar = SpacebarCaptureListener::new(skill_scan.clone(), None);
    let repair_ocr = Arc::new(RepairOcrService::new(RepairProviders::default()));
    let sale_window_ocr = Arc::new(SaleWindowOcrService::new(SaleWindowProviders::default()));
    let quests = QuestService::start(&bus, db.clone(), Arc::new(RealClock::new()), handle);
    ProducerHandles {
        config_service,
        tracker,
        hotbar,
        watcher,
        skill_tracker,
        skill_scan,
        spacebar,
        repair_ocr,
        sale_window_ocr,
        quests,
    }
}
