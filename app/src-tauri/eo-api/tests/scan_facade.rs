//! Behavioural pins for the manual skill-scan family over the typed
//! facade, ported from the family's HTTP-era route behaviour: the status
//! read, the logical-refusal contract (a refusal returns the full status
//! carrying its `error`), the capture / undo status-plus-extra shapes, the
//! accept / reject polymorphic bodies, the pending read, the spacebar
//! toggle, and the capture-preview bytes. Each shape carries a
//! transport-invariance pin: the typed response serialises to the exact
//! bytes the HTTP route answered (for the status verbs, the success body;
//! the one ratified movement, a refusal's full-status-plus-error, is
//! pinned explicitly).

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use eo_api::Api;
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::skill_scan_manual::{ScanProviders, SkillScanManual};
use eo_services::spacebar_capture_listener::SpacebarCaptureListener;

mod common;

/// A configured scan: the OCR engine is available, the skill panel is
/// found at a fixed region, each grab yields bytes, and the extractor
/// serves the given per-page level rows in order.
fn configured_providers(pages: Vec<Vec<(String, f64)>>) -> ScanProviders {
    let served = Arc::new(StdMutex::new(0usize));
    ScanProviders {
        engine_available: Arc::new(|| true),
        skill_region: Arc::new(|| Some(([0, 0], [100, 200]))),
        capture_region: Arc::new(|_| Some(vec![1, 2, 3])),
        extract_page_levels: Arc::new(move |_| {
            let mut index = served.lock().unwrap();
            let page = pages.get(*index).cloned().unwrap_or_default();
            *index += 1;
            page
        }),
    }
}

/// The composed facade over a fresh migrated database, with the manual
/// scan built over the given providers. Returns the scan handle too, so a
/// test can set the completion callback and join the extraction worker.
async fn scan_api(dir: &Path, providers: ScanProviders) -> (Api, Arc<SkillScanManual>) {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    let skill_scan = SkillScanManual::new(providers, clock.clone(), None, None, 0);
    let spacebar = SpacebarCaptureListener::new(skill_scan.clone(), None);
    let api = Api::new(
        db,
        game_data,
        clock,
        data_dir,
        handles.config_service,
        handles.tracker,
        handles.hotbar,
        handles.watcher,
        handles.skill_tracker,
        skill_scan.clone(),
        spacebar,
        handles.repair_ocr,
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
    );
    (api, skill_scan)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_read_serialises_the_backend_way() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), configured_providers(Vec::new())).await;

    // The resting status over a configured, window-present scan: the full
    // field set in the HTTP response-model order, `error` null.
    let status = api.scan_status().unwrap();
    assert_eq!(
        serde_json::to_string(&status).unwrap(),
        "{\"active\":false,\"processing\":false,\"captured_pages\":0,\
         \"expected_pages\":12,\"last_scan_time\":null,\"skills_count\":0,\
         \"configured\":true,\"game_window_present\":true,\"phase\":\"idle\",\
         \"processing_progress\":{\"done\":0,\"total\":0},\
         \"has_pending_result\":false,\"error\":null}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_returns_the_full_status_carrying_the_error() {
    let dir = tempfile::tempdir().unwrap();
    // Engine unavailable, no window: start refuses.
    let mut providers = configured_providers(Vec::new());
    providers.engine_available = Arc::new(|| false);
    providers.skill_region = Arc::new(|| None);
    let (api, _scan) = scan_api(dir.path(), providers).await;

    // The ratified movement: where the HTTP body was the lone
    // `{"error": ...}`, the typed reply is the full current status with
    // `error` set (every consumer reads `.error` first).
    let status = api.scan_start(None).unwrap();
    assert_eq!(
        serde_json::to_string(&status).unwrap(),
        "{\"active\":false,\"processing\":false,\"captured_pages\":0,\
         \"expected_pages\":12,\"last_scan_time\":null,\"skills_count\":0,\
         \"configured\":false,\"game_window_present\":false,\"phase\":\"idle\",\
         \"processing_progress\":{\"done\":0,\"total\":0},\
         \"has_pending_result\":false,\
         \"error\":\"Local OCR engine is unavailable: check the backend log\"}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_flow_serialises_status_plus_page_and_captured() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), configured_providers(Vec::new())).await;

    api.scan_start(Some(1)).unwrap();
    // A capture success carries the status fields (in order) then `page`
    // and `captured`, byte-for-byte as the HTTP capture body.
    let captured = api.scan_capture().unwrap();
    assert_eq!(
        serde_json::to_string(&captured).unwrap(),
        "{\"active\":true,\"processing\":false,\"captured_pages\":1,\
         \"expected_pages\":1,\"last_scan_time\":null,\"skills_count\":0,\
         \"configured\":true,\"game_window_present\":true,\"phase\":\"capturing\",\
         \"processing_progress\":{\"done\":0,\"total\":0},\
         \"has_pending_result\":false,\"error\":null,\"page\":1,\"captured\":true}"
    );

    // A capture refusal (no active scan after a cancel) rides the status'
    // `error`; the page/captured extras are absent.
    api.scan_cancel().unwrap();
    let refused = api.scan_capture().unwrap();
    assert_eq!(refused.page, None);
    assert_eq!(refused.captured, None);
    assert_eq!(
        refused.status.error.as_deref(),
        Some("No active scan: call start first")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undo_returns_status_plus_undone_page() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), configured_providers(Vec::new())).await;

    api.scan_start(Some(2)).unwrap();
    api.scan_capture().unwrap();
    let undone = api.scan_undo().unwrap();
    assert_eq!(undone.undone_page, Some(1));
    assert_eq!(undone.status.captured_pages, 0);

    // The empty-stack refusal rides the status' error; no undone_page.
    let refused = api.scan_undo().unwrap();
    assert_eq!(refused.undone_page, None);
    assert_eq!(refused.status.error.as_deref(), Some("No captures to undo"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accept_and_reject_preserve_the_polymorphic_shape() {
    let dir = tempfile::tempdir().unwrap();
    let pages = vec![vec![("Rifle".to_string(), 100.0)]];
    let (api, scan) = scan_api(dir.path(), configured_providers(pages)).await;
    let persisted: Arc<StdMutex<usize>> = Arc::new(StdMutex::new(0));
    let sink = persisted.clone();
    scan.set_completion_callback(Arc::new(move |levels| {
        *sink.lock().unwrap() += levels.len();
        Ok(())
    }));

    // The lone-error refusals, byte-for-byte as the HTTP plain-200 bodies.
    assert_eq!(
        serde_json::to_string(&api.scan_accept().unwrap()).unwrap(),
        "{\"error\":\"No pending result to accept\"}"
    );
    assert_eq!(
        serde_json::to_string(&api.scan_reject().unwrap()).unwrap(),
        "{\"error\":\"No pending result to reject\"}"
    );

    // Drive one page to a held result, then accept: the success body is
    // `{ok, skills_persisted}` with no `error` key.
    api.scan_start(Some(1)).unwrap();
    api.scan_capture().unwrap();
    api.scan_process().unwrap();
    scan.join_worker();
    assert_eq!(
        serde_json::to_string(&api.scan_accept().unwrap()).unwrap(),
        "{\"ok\":true,\"skills_persisted\":1}"
    );
    assert_eq!(*persisted.lock().unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_returns_the_held_skills_or_none() {
    let dir = tempfile::tempdir().unwrap();
    let pages = vec![vec![("Rifle".to_string(), 100.0)]];
    let (api, scan) = scan_api(dir.path(), configured_providers(pages)).await;

    assert!(api.scan_pending().unwrap().is_none());

    api.scan_start(Some(1)).unwrap();
    api.scan_capture().unwrap();
    api.scan_process().unwrap();
    scan.join_worker();
    let pending = api.scan_pending().unwrap().expect("a held result");
    assert_eq!(
        serde_json::to_string(&pending).unwrap(),
        "{\"skills\":{\"Rifle\":100.0}}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spacebar_toggle_echoes_the_enabled_state() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), configured_providers(Vec::new())).await;

    assert_eq!(
        serde_json::to_string(&api.scan_set_spacebar_capture(true).unwrap()).unwrap(),
        "{\"ok\":true,\"enabled\":true}"
    );
    assert_eq!(
        serde_json::to_string(&api.scan_set_spacebar_capture(false).unwrap()).unwrap(),
        "{\"ok\":true,\"enabled\":false}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_png_returns_bytes_or_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), configured_providers(Vec::new())).await;

    // No capture yet: the not-found leg.
    let missing = api.scan_capture_png(1).unwrap_err();
    assert_eq!(serde_json::to_value(&missing).unwrap()["kind"], "notFound");

    api.scan_start(Some(1)).unwrap();
    api.scan_capture().unwrap();
    assert_eq!(api.scan_capture_png(1).unwrap(), vec![1, 2, 3]);
    assert!(api.scan_capture_png(2).is_err());
}
