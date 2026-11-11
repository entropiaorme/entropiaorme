//! Behavioural pins for the skill-scan family over the typed facade: the
//! status of each phase, starting and cancelling, the shell's bookkeeping
//! of the scan's window, a scan driven over drawn skills windows to a
//! held review, and the accept / reject / pending shapes. Every wait is
//! for a state the scan reaches; a capture the test must control is held
//! by the stand-in game window until the test lets it through.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eo_api::scan::{SkillScanHint, SkillScanRefusal, SkillScanStatus};
use eo_api::Api;
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::seams::{FrameUnavailable, GameWindowFrame};
use eo_services::game_ui::test_support::{
    bundled_profile, draw_skills_window, skills_drawing_vocabulary, skills_scene,
    standard_skills_layout, Lettering, SkillsLayout,
};
use eo_services::scan_completion::ScannedLevel;
use eo_services::skill_panel::BgrImage;
use eo_services::skill_scan::{SkillScanProviders, SkillScanService};
use eo_services::spacebar_capture_listener::SpacebarCaptureListener;
use serde_json::json;

use crate::common;

/// A stand-in game window: it shows one frame (or none), counts the
/// captures begun, and lets through only as many as the test allows.
struct Window {
    showing: Mutex<Option<BgrImage>>,
    captures: AtomicUsize,
    allowed: Mutex<Option<usize>>,
    changed: Condvar,
}

impl Window {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            showing: Mutex::new(None),
            captures: AtomicUsize::new(0),
            allowed: Mutex::new(None),
            changed: Condvar::new(),
        })
    }

    fn show(&self, frame: &BgrImage) {
        *self.showing.lock().unwrap() = Some(frame.clone());
    }

    /// Hold captures from now on; `allow` lets them through one by one.
    fn hold(&self) {
        *self.allowed.lock().unwrap() = Some(0);
    }

    fn allow(&self, n: usize) {
        let mut allowed = self.allowed.lock().unwrap();
        *allowed = allowed.map(|left| left + n);
        self.changed.notify_all();
    }

    fn release(&self) {
        *self.allowed.lock().unwrap() = None;
        self.changed.notify_all();
    }

    fn captures(&self) -> usize {
        self.captures.load(Ordering::SeqCst)
    }

    fn wait_for_captures(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.captures() < n {
            assert!(
                Instant::now() < deadline,
                "only {} captures",
                self.captures()
            );
            std::thread::yield_now();
        }
    }

    fn frame(self: &Arc<Self>) -> GameWindowFrame {
        let window = self.clone();
        Arc::new(move || {
            window.captures.fetch_add(1, Ordering::SeqCst);
            let mut allowed = window.allowed.lock().unwrap();
            while *allowed == Some(0) {
                allowed = window.changed.wait(allowed).unwrap();
            }
            if let Some(left) = allowed.as_mut() {
                *left -= 1;
            }
            drop(allowed);
            window
                .showing
                .lock()
                .unwrap()
                .clone()
                .ok_or(FrameUnavailable::WindowNotFound)
        })
    }
}

/// Two drawn pages whose whole points add up to the total they print.
struct Pages {
    frames: Vec<BgrImage>,
    lettering: Lettering,
}

fn page(pager: &'static str, rows: Vec<(Option<&'static str>, &'static str)>) -> SkillsLayout {
    SkillsLayout {
        pager,
        total: "Total: 4422",
        rows,
        ..standard_skills_layout()
    }
}

fn two_pages() -> Pages {
    let mut lettering = Lettering::new(skills_scene());
    let mut frames = Vec::new();
    for layout in [
        page(
            "1 / 2",
            vec![
                (None, "1"),
                (Some("Agility"), "73.71"),
                (Some("Aim"), "1,268.53"),
            ],
        ),
        page(
            "2 / 2",
            vec![(Some("Alertness"), "3,068.94"), (Some("Analysis"), "12.34")],
        ),
    ] {
        draw_skills_window(&mut lettering, &layout);
        frames.push(std::mem::replace(&mut lettering.image, skills_scene()));
    }
    Pages { frames, lettering }
}

/// The composed facade over a fresh migrated database, with the skill
/// scan built over `providers`; the scan is handed back too, so a test
/// can wait for its worker and install a completion.
async fn scan_api(dir: &Path, providers: SkillScanProviders) -> (Api, Arc<SkillScanService>) {
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
    let skill_scan = SkillScanService::new(providers, clock.clone(), None);
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
        SpacebarCaptureListener::new(None),
        handles.repair_ocr,
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
    );
    (api, skill_scan)
}

/// A scan reading drawn pages through the stand-in window.
fn reading(window: &Arc<Window>, pages: &Pages) -> SkillScanProviders {
    SkillScanProviders {
        frame: window.frame(),
        profile: Some(Arc::new(bundled_profile())),
        read_text: Some(Arc::new(pages.lettering.recogniser())),
        vocabulary: skills_drawing_vocabulary(),
        tick: Duration::from_millis(1),
        ..SkillScanProviders::default()
    }
}

fn idle(available: bool) -> SkillScanStatus {
    SkillScanStatus::Idle {
        available,
        stopped: None.into(),
    }
}

fn wait_for(api: &Api, what: &str, done: impl Fn(&SkillScanStatus) -> bool) -> SkillScanStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = api.scan_status().unwrap();
        if done(&status) {
            return status;
        }
        assert!(Instant::now() < deadline, "never {what}: {status:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_reader_the_scan_rests_unavailable_and_never_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _scan) = scan_api(dir.path(), SkillScanProviders::default()).await;
    api.skill_scan_window_shown_for_shell();
    assert_eq!(api.scan_status().unwrap(), idle(false));
    let reply = api.scan_start().unwrap();
    assert_eq!(reply.status, idle(false));
    assert!(reply.refused.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_starts_only_from_its_open_window() {
    let dir = tempfile::tempdir().unwrap();
    let window = Window::new();
    let (api, _scan) = scan_api(dir.path(), reading(&window, &two_pages())).await;
    let refused = api.scan_start().unwrap();
    assert_eq!(refused.status, idle(true));
    assert_eq!(*refused.refused, Some(SkillScanRefusal::SurfaceClosed));
    assert_eq!(window.captures(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_scan_says_what_stops_it_and_cancelling_ends_its_watching() {
    let dir = tempfile::tempdir().unwrap();
    let window = Window::new();
    let (api, scan) = scan_api(dir.path(), reading(&window, &two_pages())).await;
    api.skill_scan_window_shown_for_shell();
    window.hold();
    let started = api.scan_start().unwrap();
    assert!(matches!(started.status, SkillScanStatus::Collecting { .. }));
    assert!(started.refused.is_none());
    // Two looks at a missing game window, each written back before the
    // next capture begins: the hint shows once it has held for both.
    window.allow(2);
    window.wait_for_captures(3);
    let status = api.scan_status().unwrap();
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({
            "phase": "collecting",
            "pagesCollected": 0,
            "pageCount": null,
            "missingPages": [],
            "hint": "gameWindowNotFound",
        })
    );
    let SkillScanStatus::Collecting { hint, .. } = status else {
        unreachable!("pinned above")
    };
    assert_eq!(*hint, Some(SkillScanHint::GameWindowNotFound));

    // A second start leaves the one scan collecting.
    assert!(matches!(
        api.scan_start().unwrap().status,
        SkillScanStatus::Collecting { .. }
    ));
    let cancelled = api.scan_cancel().unwrap();
    assert_eq!(cancelled.status, idle(true));
    // The capture held when it was cancelled finishes; then no worker is
    // left, so nothing can capture again.
    window.release();
    assert!(scan.wait_for_workers(Duration::from_secs(30)));
    let captures = window.captures();
    assert_eq!(api.scan_status().unwrap(), idle(true));
    assert_eq!(window.captures(), captures);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_scan_window_stops_the_watching() {
    let dir = tempfile::tempdir().unwrap();
    let window = Window::new();
    let (api, scan) = scan_api(dir.path(), reading(&window, &two_pages())).await;
    api.skill_scan_window_shown_for_shell();
    window.hold();
    api.scan_start().unwrap();
    window.wait_for_captures(1);
    api.skill_scan_window_closed_for_shell();
    assert_eq!(api.scan_status().unwrap(), idle(true));
    window.release();
    assert!(scan.wait_for_workers(Duration::from_secs(30)));
    assert_eq!(window.captures(), 1, "no capture after the window closed");
    // Closed, it cannot be started again until it shows.
    assert_eq!(
        *api.scan_start().unwrap().refused,
        Some(SkillScanRefusal::SurfaceClosed)
    );
    assert_eq!(window.captures(), 1);
}

/// Drive a scan over the two drawn pages to a held review.
fn drive_to_review(api: &Api, window: &Window, pages: &Pages) {
    api.skill_scan_window_shown_for_shell();
    window.show(&pages.frames[0]);
    api.scan_start().unwrap();
    wait_for(api, "collected page 1", |s| {
        matches!(
            s,
            SkillScanStatus::Collecting {
                pages_collected: 1,
                ..
            }
        )
    });
    window.show(&pages.frames[1]);
    wait_for(api, "awaited review", |s| {
        *s == SkillScanStatus::AwaitingReview
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_driven_to_its_review_is_pending_and_accepted_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let window = Window::new();
    let pages = two_pages();
    let (api, scan) = scan_api(dir.path(), reading(&window, &pages)).await;
    let saved: Arc<Mutex<Vec<ScannedLevel>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = saved.clone();
    scan.set_completion_callback(Arc::new(move |levels| {
        sink.lock().unwrap().extend_from_slice(levels);
        Ok(())
    }));

    assert!(api.scan_pending().unwrap().is_none());
    drive_to_review(&api, &window, &pages);
    let pending = api.scan_pending().unwrap().expect("a held review");
    let id = pending.scan;
    assert_eq!(
        serde_json::to_value(&pending).unwrap(),
        json!({
            "scan": id,
            "skills": {
                "Agility": 73.71,
                "Aim": 1268.53,
                "Alertness": 3068.94,
                "Analysis": 12.34,
            },
            "total": {"check": "matches"},
            "nearMisses": [],
            "guesses": [],
            "unknown": [],
            "unplaced": [],
            "repeated": [],
        })
    );

    // A review that is not the one held is refused, and nothing is saved.
    let stale = api.scan_accept(id + 1, false).unwrap();
    assert_eq!(
        serde_json::to_value(&stale).unwrap(),
        json!({
            "refused": "staleReview",
            "error": "The review shown is no longer the one held",
        })
    );
    assert!(saved.lock().unwrap().is_empty());

    let accepted = api.scan_accept(id, false).unwrap();
    assert_eq!(
        serde_json::to_value(&accepted).unwrap(),
        json!({"ok": true, "skills_persisted": 4})
    );
    let saved: Vec<String> = saved
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.skill.clone())
        .collect();
    assert_eq!(saved, vec!["Agility", "Aim", "Alertness", "Analysis"]);
    assert_eq!(api.scan_status().unwrap(), idle(true));
    assert!(api.scan_pending().unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_review_is_rejected_by_name_and_refusals_carry_their_reason() {
    let dir = tempfile::tempdir().unwrap();
    let window = Window::new();
    let pages = two_pages();
    let (api, _scan) = scan_api(dir.path(), reading(&window, &pages)).await;

    let nothing = api.scan_reject(1).unwrap();
    assert_eq!(
        serde_json::to_value(&nothing).unwrap(),
        json!({"refused": "noReview", "error": "No pending result"})
    );
    let nothing = api.scan_accept(1, false).unwrap();
    assert_eq!(nothing.refused, Some(SkillScanRefusal::NoReview));

    drive_to_review(&api, &window, &pages);
    let id = api.scan_pending().unwrap().expect("a held review").scan;
    assert_eq!(
        api.scan_reject(id + 1).unwrap().refused,
        Some(SkillScanRefusal::StaleReview)
    );
    let rejected = api.scan_reject(id).unwrap();
    assert_eq!(
        serde_json::to_value(&rejected).unwrap(),
        json!({"ok": true})
    );
    assert!(api.scan_pending().unwrap().is_none());
    assert_eq!(api.scan_status().unwrap(), idle(true));
}
