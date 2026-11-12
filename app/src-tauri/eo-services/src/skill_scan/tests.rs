//! The scan over drawn skills windows: a stand-in screen the test pages
//! through, and a stand-in recogniser that reads each piece of drawn
//! lettering as the text it was drawn for. Every wait is for a state the
//! scan reaches, never for a length of time; a capture the test must
//! hold is held by the screen until the test releases it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex as StdMutex};
use std::time::Instant;

use chrono::NaiveDateTime;

use super::*;
use crate::clock::MockClock;
use crate::game_ui::geometry::Rect;
use crate::game_ui::test_support::{
    bundled_profile, draw_skills_window, draw_window, skills_drawing_vocabulary, skills_scene,
    standard_skills_layout, window_frame, Lettering, SkillsLayout, SKILLS_ROW_PITCH,
};

/// What the stand-in game window shows.
enum Showing {
    Frame(BgrImage),
    Nothing(FrameUnavailable),
}

/// A stand-in game window: the test sets what it shows, counts every
/// capture and how many run at once, and can hold captures until it
/// releases them.
struct Screen {
    showing: StdMutex<Showing>,
    captures: AtomicUsize,
    in_flight: AtomicUsize,
    most_at_once: AtomicUsize,
    /// `None` lets captures through; `Some(n)` lets `n` more through and
    /// holds the rest.
    held: StdMutex<Option<usize>>,
    released: Condvar,
}

impl Screen {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            showing: StdMutex::new(Showing::Nothing(FrameUnavailable::WindowNotFound)),
            captures: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            most_at_once: AtomicUsize::new(0),
            held: StdMutex::new(None),
            released: Condvar::new(),
        })
    }

    fn show(&self, frame: &BgrImage) {
        *self.showing.lock().unwrap() = Showing::Frame(frame.clone());
    }

    fn show_nothing(&self, why: FrameUnavailable) {
        *self.showing.lock().unwrap() = Showing::Nothing(why);
    }

    fn captures(&self) -> usize {
        self.captures.load(Ordering::SeqCst)
    }

    /// Make captures wait until [`Screen::release`] or [`Screen::allow`].
    fn hold(&self) {
        *self.held.lock().unwrap() = Some(0);
    }

    /// Let `n` more held captures through.
    fn allow(&self, n: usize) {
        let mut held = self.held.lock().unwrap();
        *held = held.map(|left| left + n);
        self.released.notify_all();
    }

    fn release(&self) {
        *self.held.lock().unwrap() = None;
        self.released.notify_all();
    }

    /// Wait until `n` captures have begun.
    fn wait_for_captures(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(60);
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
        let screen = self.clone();
        Arc::new(move |area| {
            screen.captures.fetch_add(1, Ordering::SeqCst);
            let now = screen.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            screen.most_at_once.fetch_max(now, Ordering::SeqCst);
            let mut held = screen.held.lock().unwrap();
            while *held == Some(0) {
                held = screen.released.wait(held).unwrap();
            }
            if let Some(left) = held.as_mut() {
                *left -= 1;
            }
            drop(held);
            let shown = match &*screen.showing.lock().unwrap() {
                Showing::Frame(frame) => window_frame(frame, (0, 0), area),
                Showing::Nothing(why) => Err(*why),
            };
            screen.in_flight.fetch_sub(1, Ordering::SeqCst);
            shown
        })
    }
}

/// Skills windows drawn with one lettering, so one recogniser reads
/// them all.
struct Pages {
    frames: Vec<BgrImage>,
    lettering: Lettering,
}

fn pages(layouts: &[SkillsLayout]) -> Pages {
    let mut lettering = Lettering::new(skills_scene());
    let mut frames = Vec::new();
    for layout in layouts {
        draw_skills_window(&mut lettering, layout);
        frames.push(std::mem::replace(&mut lettering.image, skills_scene()));
    }
    Pages { frames, lettering }
}

fn layout(
    pager: &'static str,
    total: &'static str,
    rows: Vec<(Option<&'static str>, &'static str)>,
) -> SkillsLayout {
    SkillsLayout {
        pager,
        total,
        rows,
        ..standard_skills_layout()
    }
}

/// Two pages whose whole points add up to 4,427: the client's blank row
/// and a skill the vocabulary lacks count towards the total.
fn two_pages(total: &'static str) -> [SkillsLayout; 2] {
    [
        layout(
            "1 / 2",
            total,
            vec![
                (None, "1"),
                (Some("Agility"), "73.71"),
                (Some("Aim"), "1,268.53"),
            ],
        ),
        layout(
            "2 / 2",
            total,
            vec![
                (Some("Alertness"), "3,068.94"),
                (Some("Food Technology"), "5.25"),
                (Some("Analysis"), "12.34"),
            ],
        ),
    ]
}

struct Rig {
    scan: Arc<SkillScanService>,
    screen: Arc<Screen>,
    bus: Arc<EventBus>,
    clock: Arc<MockClock>,
}

fn rig(pages: &Pages) -> Rig {
    rig_with(Arc::new(pages.lettering.recogniser()), |_| {})
}

fn rig_with(read_text: FrameReader, adjust: impl FnOnce(&mut SkillScanProviders)) -> Rig {
    let screen = Screen::new();
    let bus = Arc::new(EventBus::new());
    let clock = Arc::new(MockClock::new(None, 0.0));
    let mut providers = SkillScanProviders {
        frame: screen.frame(),
        profile: Some(Arc::new(bundled_profile())),
        read_text: Some(read_text),
        vocabulary: skills_drawing_vocabulary(),
        tick: Duration::from_millis(1),
        idle_limit: DEFAULT_IDLE_LIMIT,
    };
    adjust(&mut providers);
    let scan = SkillScanService::new(providers, clock.clone(), Some(bus.clone()));
    scan.surface_shown();
    Rig {
        scan,
        screen,
        bus,
        clock,
    }
}

fn epoch(clock: &MockClock) -> f64 {
    naive_to_epoch(clock.now())
}

fn start(rig: &Rig) -> ScanStatus {
    rig.scan.start().expect("the scan starts")
}

/// Wait (bounded) until the status satisfies `done`.
fn wait_for(scan: &SkillScanService, what: &str, done: impl Fn(&ScanStatus) -> bool) -> ScanStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = scan.status();
        if done(&status) {
            return status;
        }
        assert!(Instant::now() < deadline, "never {what}: {status:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn collected(scan: &SkillScanService, pages: u32) {
    wait_for(
        scan,
        "collected",
        |s| matches!(s, ScanStatus::Collecting(p) if p.collected == pages),
    );
}

fn review(scan: &SkillScanService) -> SkillScanReview {
    wait_for(scan, "awaited review", |s| *s == ScanStatus::AwaitingReview);
    scan.pending().expect("the finished scan is held")
}

/// The status once the scan has finished `n` looks at the game window
/// begun after this call. The one worker looks one capture at a time and
/// writes each look back before its next capture, so a capture begun
/// after `n` more than were already counted marks the `n` before it as
/// written back.
fn after_looks(rig: &Rig, n: usize) -> ScanStatus {
    let from = rig.screen.captures();
    let deadline = Instant::now() + Duration::from_secs(60);
    while rig.screen.captures() < from + n + 1 {
        if !matches!(rig.scan.status(), ScanStatus::Collecting(_)) {
            break;
        }
        assert!(Instant::now() < deadline, "the scan stopped looking");
        std::thread::sleep(Duration::from_millis(2));
    }
    rig.scan.status()
}

/// The hint once the view has been the same for long enough to show.
fn hint_now(rig: &Rig) -> Option<ScanHint> {
    match after_looks(rig, 3) {
        ScanStatus::Collecting(progress) => progress.hint,
        other => panic!("not collecting: {other:?}"),
    }
}

/// Stop the scan and wait for its worker to be gone, after which nothing
/// can capture.
fn stop_and_settle(rig: &Rig) {
    rig.scan.cancel().expect("cancels");
    rig.screen.release();
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
}

fn phases(bus: &EventBus) -> Arc<StdMutex<Vec<String>>> {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let sink = seen.clone();
    bus.add_tap(move |event| {
        if event.topic() == crate::event_bus::Topic::ScanStatusChanged {
            let data = event.payload_value();
            sink.lock()
                .unwrap()
                .push(data["payload"]["phase"].as_str().unwrap_or("?").to_string());
        }
    });
    seen
}

fn saved_by(scan: &SkillScanService) -> Arc<StdMutex<Vec<ScannedLevel>>> {
    let saved = Arc::new(StdMutex::new(Vec::new()));
    let sink = saved.clone();
    scan.set_completion_callback(Arc::new(move |skills| {
        sink.lock().unwrap().extend_from_slice(skills);
        Ok(())
    }));
    saved
}

fn level(skill: &str, level: f64, scanned_at: f64) -> ScannedLevel {
    ScannedLevel {
        skill: skill.into(),
        level,
        scanned_at,
    }
}

fn idle(stopped: Option<ScanStop>) -> ScanStatus {
    ScanStatus::Idle {
        available: true,
        stopped,
    }
}

#[test]
fn pages_collected_in_any_order_are_saved_each_with_when_it_was_read() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    let events = phases(&rig.bus);
    let saved = saved_by(&rig.scan);

    let page_two_read = epoch(&rig.clock);
    rig.screen.show(&pages.frames[1]);
    assert!(matches!(start(&rig), ScanStatus::Collecting(_)));
    collected(&rig.scan, 1);
    // Hold the next capture, then turn to page 1 a minute on: the held
    // capture is of page 1, taken at the later time.
    rig.screen.hold();
    rig.screen.wait_for_captures(rig.screen.captures() + 1);
    assert_eq!(
        rig.scan.status(),
        ScanStatus::Collecting(ScanProgress {
            collected: 1,
            page_count: Some(2),
            missing: vec![1],
            hint: None,
        })
    );

    rig.screen.show(&pages.frames[0]);
    rig.clock.advance(60.0).unwrap();
    rig.screen.release();
    let review = review(&rig.scan);
    assert_eq!(
        review.skills,
        vec![
            level("Agility", 73.71, page_two_read + 60.0),
            level("Aim", 1268.53, page_two_read + 60.0),
            level("Alertness", 3068.94, page_two_read),
            level("Analysis", 12.34, page_two_read),
        ],
        "the blank row and the skill this version lacks are not saved"
    );
    assert_eq!(review.total, TotalCheck::Matches);
    assert_eq!(
        review.unknown,
        vec![UnsavedSkill {
            name: "Food Technology".into(),
            points_text: "5.25".into(),
        }]
    );
    assert!(review.near_misses.is_empty() && review.guesses.is_empty());

    // The watching stopped with the last page.
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
    let captures = rig.screen.captures();

    // The player accepts later still; the levels keep their read times.
    rig.clock.advance(600.0).unwrap();
    assert_eq!(rig.scan.accept(review.scan, false), Ok(4));
    assert_eq!(*saved.lock().unwrap(), review.skills);
    assert_eq!(rig.scan.status(), idle(None));
    assert_eq!(
        rig.screen.captures(),
        captures,
        "nothing captured in review"
    );

    let events = events.lock().unwrap().clone();
    assert_eq!(events.first().map(String::as_str), Some("capturing"));
    assert_eq!(events.last().map(String::as_str), Some("idle"));
    assert!(events.contains(&"awaiting_review".to_string()));
}

#[test]
fn a_total_that_does_not_add_up_is_saved_only_when_asked() {
    let pages = pages(&two_pages("Total: 4500"));
    let rig = rig(&pages);
    let saved = saved_by(&rig.scan);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    let review = review(&rig.scan);
    assert_eq!(
        review.total,
        TotalCheck::Mismatch {
            sum: 4427,
            total: 4500
        }
    );
    assert_eq!(
        rig.scan.accept(review.scan, false),
        Err(ScanRefusal::TotalNotConfirmed)
    );
    assert_eq!(rig.scan.status(), ScanStatus::AwaitingReview);
    assert!(saved.lock().unwrap().is_empty());
    assert_eq!(rig.scan.accept(review.scan, true), Ok(4));
    assert_eq!(saved.lock().unwrap().len(), 4);
}

#[test]
fn totals_that_differ_between_pages_leave_the_review_unchecked() {
    let [first, mut second] = two_pages("Total: 4427");
    second.total = "Total: 4428";
    let pages = pages(&[first, second]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    let review = review(&rig.scan);
    assert_eq!(review.total, TotalCheck::Unchecked);
    assert_eq!(
        rig.scan.accept(review.scan, false),
        Err(ScanRefusal::TotalNotConfirmed)
    );
}

#[test]
fn a_near_miss_on_its_page_is_saved_but_a_guess_at_the_end_is_not() {
    let [mut first, mut second] = two_pages("Total: 4427");
    // A slip near Alertness and no other skill: read on its page.
    second.rows[0] = (Some("Alertnes"), "3,068.94");
    // Near no skill on its page; the finished sweep guesses Aim, the one
    // skill it has not otherwise read.
    first.rows[2] = (Some("Alm"), "1,268.53");
    let pages = pages(&[first, second]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    let review = review(&rig.scan);
    assert_eq!(
        review.near_misses,
        vec![NearMissSkill {
            skill: "Alertness".into(),
            read_as: "Alertnes".into()
        }]
    );
    assert_eq!(
        review.guesses,
        vec![GuessedSkill {
            read_as: "Alm".into(),
            probably: "Aim".into(),
            points_text: "1,268.53".into(),
        }]
    );
    let names: Vec<&str> = review.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(names, vec!["Agility", "Alertness", "Analysis"]);
    assert_eq!(review.total, TotalCheck::Matches);
}

#[test]
fn a_skill_read_on_two_pages_is_shown_and_not_saved() {
    let [first, mut second] = two_pages("Total: 4427");
    second.rows[1] = (Some("Agility"), "73.71");
    let pages = pages(&[first, second]);
    let rig = rig(&pages);
    let saved = saved_by(&rig.scan);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    let review = review(&rig.scan);
    assert_eq!(review.repeated, vec!["Agility".to_string()]);
    assert_eq!(review.total, TotalCheck::Unchecked);
    assert_eq!(rig.scan.accept(review.scan, true), Ok(3));
    assert!(
        saved.lock().unwrap().iter().all(|s| s.skill != "Agility"),
        "{saved:?}"
    );
}

#[test]
fn a_window_too_short_for_a_whole_page_asks_to_be_dragged_taller() {
    let mut short = standard_skills_layout();
    short.pane = true;
    let track = 32 + 4 * SKILLS_ROW_PITCH;
    short.thumb = Some((0, track * 2 / 3));
    let pages = pages(&[short]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    assert_eq!(hint_now(&rig), Some(ScanHint::WindowTooShort));
    assert_eq!(
        rig.scan.status(),
        ScanStatus::Collecting(ScanProgress {
            hint: Some(ScanHint::WindowTooShort),
            ..ScanProgress::default()
        }),
        "the rows in view are not collected"
    );
    stop_and_settle(&rig);
}

#[test]
fn the_professions_tab_asks_for_the_skills_tab() {
    let mut professions = standard_skills_layout();
    professions.name_header = ("Profession Name", 100);
    professions.points_header = "Level";
    let pages = pages(&[professions]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    assert_eq!(hint_now(&rig), Some(ScanHint::NotSkillsTab));
    stop_and_settle(&rig);
}

#[test]
fn a_window_over_the_table_asks_for_it_to_be_uncovered() {
    let layout = standard_skills_layout();
    let mut pages = pages(&[layout]);
    let header_bottom = 30 + 182;
    draw_window(
        &mut pages.frames[0],
        Rect::new(
            40 + 790,
            header_bottom + 2 * SKILLS_ROW_PITCH + 20,
            1080,
            880,
        ),
    );
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    assert_eq!(hint_now(&rig), Some(ScanHint::SkillsWindowCovered));
    stop_and_settle(&rig);
}

#[test]
fn no_game_window_or_no_skills_window_says_which() {
    let pages = pages(&[]);
    let rig = rig(&pages);
    start(&rig);
    assert_eq!(hint_now(&rig), Some(ScanHint::GameWindowNotFound));
    rig.screen
        .show_nothing(FrameUnavailable::OutsideCapturableArea);
    assert_eq!(hint_now(&rig), Some(ScanHint::GameWindowOffScreen));
    rig.screen.show(&skills_scene());
    assert_eq!(hint_now(&rig), Some(ScanHint::SkillsWindowNotOpen));
    stop_and_settle(&rig);
}

#[test]
fn a_hint_is_shown_only_once_it_has_held_for_two_looks() {
    let pages = pages(&[]);
    let rig = rig(&pages);
    let events = phases(&rig.bus);
    rig.screen.hold();
    start(&rig);
    // The first look, at a missing game window, is written back before
    // the second capture begins: it shows nothing yet.
    rig.screen.allow(1);
    rig.screen.wait_for_captures(2);
    assert_eq!(
        rig.scan.status(),
        ScanStatus::Collecting(ScanProgress::default()),
        "a single look does not show a hint"
    );
    rig.screen.allow(1);
    rig.screen.wait_for_captures(3);
    assert_eq!(
        rig.scan.status(),
        ScanStatus::Collecting(ScanProgress {
            hint: Some(ScanHint::GameWindowNotFound),
            ..ScanProgress::default()
        }),
        "the second look alike shows it"
    );
    // Steady thereafter: no frame per look.
    let frames = events.lock().unwrap().len();
    rig.screen.allow(5);
    rig.screen.wait_for_captures(8);
    assert_eq!(events.lock().unwrap().len(), frames);
    stop_and_settle(&rig);
}

#[test]
fn capture_that_is_not_allowed_ends_the_scan_without_asking_again() {
    let pages = pages(&[]);
    let rig = rig(&pages);
    rig.screen.show_nothing(FrameUnavailable::CaptureNotStarted);
    start(&rig);
    let status = wait_for(&rig.scan, "stopped", |s| {
        matches!(s, ScanStatus::Idle { .. })
    });
    assert_eq!(status, idle(Some(ScanStop::CaptureNotAllowed)));
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
    assert_eq!(
        rig.screen.captures(),
        1,
        "asked once, never again by itself"
    );
    // Starting again asks once more.
    start(&rig);
    wait_for(&rig.scan, "stopped", |s| {
        matches!(s, ScanStatus::Idle { .. })
    });
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
    assert_eq!(rig.screen.captures(), 2);
}

#[test]
fn a_scan_with_no_new_page_for_a_while_stops() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig_with(Arc::new(pages.lettering.recogniser()), |providers| {
        providers.idle_limit = Duration::from_secs(120);
    });
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    collected(&rig.scan, 1);
    // The same page showing for most of the limit keeps it collecting.
    rig.clock.advance(119.0).unwrap();
    assert!(matches!(after_looks(&rig, 2), ScanStatus::Collecting(_)));
    rig.clock.advance(1.0).unwrap();
    let status = wait_for(&rig.scan, "stopped", |s| {
        matches!(s, ScanStatus::Idle { .. })
    });
    assert_eq!(status, idle(Some(ScanStop::NoNewPage)));
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
}

#[test]
fn a_skills_window_at_an_interface_size_the_profile_lacks_is_said_as_that_to_the_end() {
    // The table drawn with taller rows than any scale the bundled profile
    // lists, whatever it comes to list: no look can read it.
    let tallest_listed = crate::game_ui::test_support::bundled_profile().windows["skills"]
        .skills_table
        .as_ref()
        .expect("the bundled profile describes the skills table")
        .scales
        .iter()
        .map(|scale| scale.row_pitch[1])
        .max()
        .expect("a scale is listed");
    let larger = SkillsLayout {
        // The drawn pitch runs rule to rule, one more than the band a
        // claim is stated in; well clear of the tallest claim.
        row_pitch: tallest_listed + 1 + (SKILLS_ROW_PITCH - 44),
        ..layout(
            "1 / 2",
            "Total: 4427",
            vec![(None, "1"), (Some("Agility"), "73.71")],
        )
    };
    let pages = pages(&[larger]);
    let rig = rig_with(Arc::new(pages.lettering.recogniser()), |providers| {
        providers.idle_limit = Duration::from_secs(120);
    });
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    // Once two looks agree, the player is told why, not to show the page
    // again.
    wait_for(&rig.scan, "said the size is unreadable", |s| {
        matches!(s, ScanStatus::Collecting(p)
            if p.hint == Some(ScanHint::InterfaceSizeUnreadable) && p.collected == 0)
    });
    // When the scan gives up, it says the same, not that a wait passed
    // with no new page.
    rig.clock.advance(120.0).unwrap();
    let status = wait_for(&rig.scan, "stopped", |s| {
        matches!(s, ScanStatus::Idle { .. })
    });
    assert_eq!(status, idle(Some(ScanStop::InterfaceSizeUnreadable)));
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
}

#[test]
fn a_cancelled_scan_drops_the_page_it_was_reading_and_stops() {
    let pages = pages(&two_pages("Total: 4427")[..1]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    rig.screen.hold();
    start(&rig);
    rig.screen.wait_for_captures(1);
    assert_eq!(rig.scan.cancel(), Ok(idle(None)));
    stop_and_settle(&rig);
    assert_eq!(rig.screen.captures(), 1, "no capture after the cancel");
    assert_eq!(
        rig.scan.status(),
        idle(None),
        "the page read after the cancel was not written back"
    );
}

#[test]
fn a_scan_started_after_a_cancel_never_reads_beside_the_old_one() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    rig.screen.hold();
    start(&rig);
    rig.screen.wait_for_captures(1);
    rig.scan.cancel().unwrap();
    // The old worker is still inside its capture.
    rig.screen.show(&pages.frames[1]);
    start(&rig);
    rig.screen.release();
    collected(&rig.scan, 1);
    assert_eq!(
        rig.scan.status(),
        ScanStatus::Collecting(ScanProgress {
            collected: 1,
            page_count: Some(2),
            missing: vec![1],
            hint: None,
        }),
        "page 1 from the cancelled scan is not carried over"
    );
    assert_eq!(rig.screen.most_at_once.load(Ordering::SeqCst), 1);
    stop_and_settle(&rig);
}

#[test]
fn only_one_worker_watches_however_often_start_is_asked() {
    let pages = pages(&two_pages("Total: 4427")[..1]);
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    rig.screen.hold();
    start(&rig);
    rig.screen.wait_for_captures(1);
    assert!(matches!(start(&rig), ScanStatus::Collecting(_)));
    assert!(matches!(start(&rig), ScanStatus::Collecting(_)));
    assert_eq!(*rig.scan.workers.live.lock().unwrap(), 1);
    stop_and_settle(&rig);
}

#[test]
fn nothing_is_captured_unless_a_scan_is_collecting() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    for _ in 0..3 {
        rig.scan.status();
        rig.scan.pending();
    }
    assert_eq!(*rig.scan.workers.live.lock().unwrap(), 0);
    assert_eq!(rig.screen.captures(), 0, "nothing captured while idle");

    start(&rig);
    after_looks(&rig, 1);
    stop_and_settle(&rig);
    let captures = rig.screen.captures();
    assert_eq!(*rig.scan.workers.live.lock().unwrap(), 0);
    assert_eq!(rig.screen.captures(), captures, "no worker left to capture");
}

#[test]
fn collecting_happens_only_while_the_scan_window_shows() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    rig.screen.show(&pages.frames[0]);
    rig.scan.surface_closed();
    assert_eq!(rig.scan.start(), Err(ScanRefusal::SurfaceClosed));
    assert_eq!(rig.screen.captures(), 0);

    rig.scan.surface_shown();
    start(&rig);
    after_looks(&rig, 1);
    assert_eq!(rig.scan.surface_closed(), idle(None));
    assert!(rig.scan.wait_for_workers(Duration::from_secs(60)));
    let captures = rig.screen.captures();
    assert_eq!(rig.scan.start(), Err(ScanRefusal::SurfaceClosed));
    assert_eq!(
        rig.screen.captures(),
        captures,
        "the closed window stopped it"
    );

    // A review is the main window's: closing the scan window keeps it.
    rig.scan.surface_shown();
    start(&rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    review(&rig.scan);
    assert_eq!(rig.scan.surface_closed(), ScanStatus::AwaitingReview);
    assert!(rig.scan.pending().is_some());
}

#[test]
fn a_reader_that_fails_stops_the_scan_and_says_so() {
    let pages = pages(&two_pages("Total: 4427")[..1]);
    let rig = rig_with(
        Arc::new(|_: &BgrImage| -> Option<(String, f64)> { panic!("the recogniser failed") }),
        |_| {},
    );
    rig.screen.show(&pages.frames[0]);
    start(&rig);
    let status = wait_for(&rig.scan, "stopped", |s| {
        matches!(s, ScanStatus::Idle { .. })
    });
    assert_eq!(status, idle(Some(ScanStop::ReadFailed)));
    // The next scan starts clean.
    assert!(matches!(start(&rig), ScanStatus::Collecting(_)));
    stop_and_settle(&rig);
}

/// A clock that fails on its `n`th reading.
struct FailingClock {
    readings: AtomicUsize,
    fail_on: usize,
}

impl Clock for FailingClock {
    fn now(&self) -> NaiveDateTime {
        let reading = self.readings.fetch_add(1, Ordering::SeqCst) + 1;
        assert_ne!(reading, self.fail_on, "the clock failed");
        NaiveDateTime::parse_from_str("2026-01-01 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn monotonic(&self) -> f64 {
        0.0
    }
}

#[test]
fn a_failure_while_writing_a_look_back_stops_the_scan_too() {
    let screen = Screen::new();
    // With no bus, the clock is read when the worker starts (1), as each
    // look captures (2), and as the look is written back (3).
    let scan = SkillScanService::new(
        SkillScanProviders {
            frame: screen.frame(),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(|_: &BgrImage| Some((String::new(), 0.0)))),
            vocabulary: skills_drawing_vocabulary(),
            tick: Duration::from_millis(1),
            idle_limit: DEFAULT_IDLE_LIMIT,
        },
        Arc::new(FailingClock {
            readings: AtomicUsize::new(0),
            fail_on: 3,
        }),
        None,
    );
    scan.surface_shown();
    scan.start().unwrap();
    let status = wait_for(&scan, "stopped", |s| matches!(s, ScanStatus::Idle { .. }));
    assert_eq!(status, idle(Some(ScanStop::ReadFailed)));
    assert!(scan.wait_for_workers(Duration::from_secs(60)));
}

#[test]
fn without_a_reader_nothing_starts_and_nothing_is_captured() {
    let screen = Screen::new();
    let scan = SkillScanService::new(
        SkillScanProviders {
            frame: screen.frame(),
            profile: Some(Arc::new(bundled_profile())),
            read_text: None,
            vocabulary: skills_drawing_vocabulary(),
            ..SkillScanProviders::default()
        },
        Arc::new(MockClock::new(None, 0.0)),
        None,
    );
    scan.surface_shown();
    let unavailable = ScanStatus::Idle {
        available: false,
        stopped: None,
    };
    assert_eq!(scan.start(), Ok(unavailable.clone()));
    assert_eq!(scan.status(), unavailable);
    assert_eq!(*scan.workers.live.lock().unwrap(), 0);
    assert_eq!(screen.captures(), 0);
}

/// A finished, matching review held by a rig's scan.
fn held_review(rig: &Rig, pages: &Pages) -> SkillScanReview {
    rig.screen.show(&pages.frames[0]);
    start(rig);
    collected(&rig.scan, 1);
    rig.screen.show(&pages.frames[1]);
    review(&rig.scan)
}

#[test]
fn the_review_verbs_name_the_review_and_a_failed_save_keeps_it() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    assert_eq!(rig.scan.accept(1, false), Err(ScanRefusal::NoReview));
    assert_eq!(rig.scan.reject(1), Err(ScanRefusal::NoReview));

    rig.scan
        .set_completion_callback(Arc::new(|_| Err("disk full".into())));
    let review = held_review(&rig, &pages);
    assert_eq!(
        rig.scan.accept(review.scan + 1, false),
        Err(ScanRefusal::StaleReview)
    );
    assert_eq!(
        rig.scan.reject(review.scan + 1),
        Err(ScanRefusal::StaleReview)
    );
    assert_eq!(
        rig.scan.accept(review.scan, false),
        Err(ScanRefusal::SaveFailed("disk full".into()))
    );
    assert_eq!(rig.scan.status(), ScanStatus::AwaitingReview);
    assert_eq!(rig.scan.pending(), Some(review.clone()));
    assert_eq!(rig.scan.start(), Ok(ScanStatus::AwaitingReview));
    assert_eq!(rig.scan.cancel(), Ok(ScanStatus::AwaitingReview));

    assert_eq!(rig.scan.reject(review.scan), Ok(()));
    assert_eq!(rig.scan.pending(), None);
    assert_eq!(rig.scan.status(), idle(None));
}

#[test]
fn while_a_review_saves_every_other_verb_is_refused_at_once() {
    let pages = pages(&two_pages("Total: 4427"));
    let rig = rig(&pages);
    let review = held_review(&rig, &pages);
    let entered = Arc::new((StdMutex::new(false), Condvar::new()));
    let gate = Arc::new((StdMutex::new(false), Condvar::new()));
    let (entered_in, gate_in) = (entered.clone(), gate.clone());
    rig.scan.set_completion_callback(Arc::new(move |_| {
        *entered_in.0.lock().unwrap() = true;
        entered_in.1.notify_all();
        let mut open = gate_in.0.lock().unwrap();
        while !*open {
            open = gate_in.1.wait(open).unwrap();
        }
        Ok(())
    }));
    let scan = rig.scan.clone();
    let id = review.scan;
    let saving = std::thread::spawn(move || scan.accept(id, false));
    {
        let mut inside = entered.0.lock().unwrap();
        while !*inside {
            inside = entered.1.wait(inside).unwrap();
        }
    }
    assert_eq!(rig.scan.status(), ScanStatus::Saving);
    assert_eq!(rig.scan.reject(id), Err(ScanRefusal::Saving));
    assert_eq!(rig.scan.accept(id, false), Err(ScanRefusal::Saving));
    assert_eq!(rig.scan.cancel(), Err(ScanRefusal::Saving));
    assert_eq!(rig.scan.start(), Err(ScanRefusal::Saving));
    assert_eq!(rig.scan.pending(), None);

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert_eq!(saving.join().unwrap(), Ok(4), "the save completes");
    assert_eq!(rig.scan.status(), idle(None));
}
