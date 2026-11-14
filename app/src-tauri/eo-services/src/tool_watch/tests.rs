use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Mutex;
use std::time::Duration;

use super::*;
use crate::clock::MockClock;
use crate::eu_window::GameFocus;
use crate::game_ui::geometry::crop;
use crate::game_ui::mob_names::MobCatalogue;
use crate::game_ui::seams::GameFrame;
use crate::game_ui::test_support::{
    blank, bundled_profile, draw_window, fill, scenery, ToolBarDrawing, ToolBarScene, NAME_LEFT,
    NAME_TOP,
};
use crate::game_ui::tool_bar::{ToolBar, View};
use crate::game_ui::tool_names::{BundledTool, LibraryTool, ToolKind};
use crate::mob_watch::{MobPulse, MobWatch, MobWatchProviders, PulseSink};
use crate::screen_watch::{capture_gate, capture_is_gated};
use crate::time::clock_epoch_ms;

const OPALO: &str = "Sollomate Opalo";
const OPALA: &str = "Sollomate Opala";
const ARMAX: &str = "Armax ML-35";

/// Where the health bar's outline starts in the drawn scenes.
const AT: (i32, i32) = (100, 380);

/// Every wait for a worker below is bounded only so that a fault fails the
/// test instead of hanging it; nothing asserted depends on timing.
const BOUND: Duration = Duration::from_secs(60);

/// A scene with the bar drawn at `at`, naming `name` (nothing held when
/// `None`).
fn scene_at(at: (i32, i32), name: Option<&str>) -> BgrImage {
    let mut scene = ToolBarScene::new(scenery(1000, 500, 5));
    scene.draw(&ToolBarDrawing {
        name,
        ..ToolBarDrawing::named(at, "")
    });
    scene.image
}

fn scene(name: Option<&str>) -> BgrImage {
    scene_at(AT, name)
}

/// The scene with marks in the bar's name area that are not the client's
/// lettering: an unclear bar.
fn unclear_scene() -> BgrImage {
    let mut image = scene(None);
    let tool = (AT.0 + crate::game_ui::test_support::TOOL_PITCH, AT.1);
    let at = (tool.0 + NAME_LEFT, tool.1 + NAME_TOP);
    fill(
        &mut image,
        Rect::new(at.0, at.1, at.0 + 13, at.1 + 13),
        [60, 140, 220],
    );
    image
}

fn library(entries: &[(i64, &str)]) -> ToolCatalogue {
    ToolCatalogue::new(
        entries.iter().map(|(id, name)| LibraryTool {
            id: *id,
            name: name.to_string(),
            kind: ToolKind::Weapon,
            enabled: true,
        }),
        Vec::<BundledTool>::new(),
        Vec::<String>::new(),
    )
}

fn holding(id: i64) -> ToolSeen {
    ToolSeen::Holding(HeldTool::Library { id })
}

type Step = Box<dyn FnOnce() + Send>;

/// Run inside a capture, on the capture's number.
type OnCapture = Arc<Mutex<Option<Box<dyn FnMut(usize) + Send>>>>;

/// The watcher over stand-in seams: a frame seam that captures the scene
/// as it stands (the whole of it, or the part asked for, placed in the
/// client area as the app's seam places it) and records what it was asked
/// for and when, a recogniser that says what the test tells it to and
/// counts its calls, and a wait driven by the test.
struct Rig {
    clock: Arc<MockClock>,
    start_ms: i64,
    image: Arc<Mutex<BgrImage>>,
    /// What the recogniser reads, whatever the crop.
    answer: Arc<Mutex<String>>,
    reads: Arc<AtomicUsize>,
    /// Each capture: what was asked for, and when on the monotonic clock.
    asked: Arc<Mutex<Vec<(FrameArea, f64)>>>,
    seen: Arc<Mutex<Vec<ToolPulse>>>,
    catalogue: Arc<ToolCatalogueCell>,
    focus: Arc<Mutex<GameFocus>>,
    running: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
    /// Run inside a capture, on the capture's number.
    on_capture: OnCapture,
    /// What every capture fails with, while set.
    failing: Arc<Mutex<Option<FrameUnavailable>>>,
    /// Run before the look the wait of that number lets through.
    steps: Arc<Mutex<BTreeMap<usize, Step>>>,
    watch: Arc<ToolWatch>,
}

/// A wait that runs the step of its number, then lets a look through at
/// once if a hint is waiting, else moves the clock to the moment the look
/// is due (never back); past `max_waits` it gives up, so a run ends.
fn driven_wait(
    clock: Arc<MockClock>,
    steps: Arc<Mutex<BTreeMap<usize, Step>>>,
    max_waits: usize,
) -> PulseWait {
    let waits = AtomicUsize::new(0);
    Arc::new(move |due: f64, woken: &mpsc::Receiver<()>| {
        let n = waits.fetch_add(1, Ordering::SeqCst) + 1;
        if n > max_waits {
            return false;
        }
        let step = steps.lock().unwrap().remove(&n);
        if let Some(step) = step {
            step();
        }
        match woken.try_recv() {
            Ok(()) => return true,
            Err(TryRecvError::Disconnected) => return false,
            Err(TryRecvError::Empty) => {}
        }
        let now = clock.monotonic();
        if due > now {
            clock.advance(due - now).unwrap();
        }
        true
    })
}

/// The part of `image` that `area` asks for, as the app's seam captures
/// it: clipped to the image, its offset where it was taken from.
fn capture_of(image: &BgrImage, area: FrameArea) -> GameFrame {
    match area {
        FrameArea::Whole => GameFrame::whole(image.clone(), (0, 0)),
        FrameArea::Part(rect) => {
            let (x0, y0) = (rect.x0.max(0), rect.y0.max(0));
            GameFrame {
                image: crop(image, rect),
                origin: (i64::from(x0), i64::from(y0)),
                offset: (i64::from(x0), i64::from(y0)),
                age: Duration::ZERO,
            }
        }
    }
}

fn rig(name: Option<&str>, catalogue: ToolCatalogue, max_waits: usize) -> Rig {
    rig_with(name, catalogue, bundled_profile(), max_waits)
}

fn rig_with(
    name: Option<&str>,
    catalogue: ToolCatalogue,
    profile: UiProfile,
    max_waits: usize,
) -> Rig {
    let clock = Arc::new(MockClock::new(None, 100.0));
    let start_ms = clock_epoch_ms(clock.as_ref());
    let image = Arc::new(Mutex::new(scene(name)));
    let answer = Arc::new(Mutex::new(name.unwrap_or_default().to_string()));
    let reads = Arc::new(AtomicUsize::new(0));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let catalogue = Arc::new(ToolCatalogueCell::new(catalogue));
    let focus = Arc::new(Mutex::new(GameFocus::Focused));
    let running = Arc::new(AtomicBool::new(true));
    let starts = Arc::new(AtomicUsize::new(0));
    let on_capture: OnCapture = Arc::new(Mutex::new(None));
    let failing = Arc::new(Mutex::new(None));
    let steps = Arc::new(Mutex::new(BTreeMap::new()));
    let frame: GameWindowFrame = {
        let (clock, image, asked, running, on_capture, failing) = (
            clock.clone(),
            image.clone(),
            asked.clone(),
            running.clone(),
            on_capture.clone(),
            failing.clone(),
        );
        Arc::new(move |area| {
            if !running.load(Ordering::SeqCst) {
                return Err(FrameUnavailable::CaptureNotStarted);
            }
            assert!(capture_is_gated(), "a capture is taken under the lock");
            let n = {
                let mut asked = asked.lock().unwrap();
                asked.push((area, clock.monotonic()));
                asked.len()
            };
            // A hook that moves the clock is a slow grab of the pixels:
            // they are as old as it took.
            let before = clock.monotonic();
            if let Some(hook) = on_capture.lock().unwrap().as_mut() {
                hook(n);
            }
            let age = Duration::from_secs_f64(clock.monotonic() - before);
            if let Some(why) = *failing.lock().unwrap() {
                return Err(why);
            }
            Ok(capture_of(&image.lock().unwrap(), area).aged(age))
        })
    };
    let read_text: FrameReader = {
        let (answer, reads) = (answer.clone(), reads.clone());
        Arc::new(move |_: &BgrImage| {
            reads.fetch_add(1, Ordering::SeqCst);
            Some((answer.lock().unwrap().clone(), 0.99))
        })
    };
    let probe = focus.clone();
    let watch = ToolWatch::new(
        ToolWatchProviders {
            frame,
            capture_running: {
                let running = running.clone();
                Arc::new(move || running.load(Ordering::SeqCst))
            },
            start_capture: {
                let (running, starts) = (running.clone(), starts.clone());
                Arc::new(move || {
                    starts.fetch_add(1, Ordering::SeqCst);
                    running.store(true, Ordering::SeqCst);
                    true
                })
            },
            enabled: Arc::new(|| true),
            focus: Some(Arc::new(move || *probe.lock().unwrap())),
            profile: Some(Arc::new(profile)),
            read_text: Some(read_text),
            catalogue: catalogue.clone(),
            wait: Some(driven_wait(clock.clone(), steps.clone(), max_waits)),
        },
        clock.clone(),
    );
    Rig {
        clock,
        start_ms,
        image,
        answer,
        reads,
        asked,
        seen: Arc::new(Mutex::new(Vec::new())),
        catalogue,
        focus,
        running,
        starts,
        on_capture,
        failing,
        steps,
        watch,
    }
}

impl Rig {
    /// Before the look let through by wait `n`, do `step`.
    fn at(&self, n: usize, step: impl FnOnce() + Send + 'static) {
        self.steps.lock().unwrap().insert(n, Box::new(step));
    }

    /// Before look `n`, show `image` with the recogniser reading `answer`.
    fn show(&self, n: usize, image: BgrImage, answer: &str) {
        let (slot, said, answer) = (self.image.clone(), self.answer.clone(), answer.to_string());
        self.at(n, move || {
            *slot.lock().unwrap() = image;
            *said.lock().unwrap() = answer;
        });
    }

    fn sink(&self) -> ToolPulseSink {
        let seen = self.seen.clone();
        Arc::new(move |pulse| {
            seen.lock().unwrap().push(pulse);
            true
        })
    }

    /// Watch `s1` until the waits run out.
    fn run(&self) {
        self.watch.start("s1", self.sink());
        self.finish();
    }

    fn finish(&self) {
        assert!(self.watch.wait_for_workers(BOUND), "the worker ended");
    }

    /// What was told, each with its time in milliseconds after the start.
    fn told(&self) -> Vec<(i64, ToolSeen)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|pulse| {
                assert_eq!(pulse.session_id, "s1");
                (pulse.at_ms - self.start_ms, pulse.seen.clone())
            })
            .collect()
    }

    fn areas(&self) -> Vec<FrameArea> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(area, _)| *area)
            .collect()
    }

    /// When each capture was taken, in milliseconds after the start.
    fn capture_times(&self) -> Vec<i64> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(_, at)| ((at - 100.0) * 1000.0).round() as i64)
            .collect()
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

/// Where the watcher captures once it has found the bar drawn at `at`.
fn place_of(at: (i32, i32)) -> Rect {
    let profile = bundled_profile();
    let image = scene_at(at, Some(OPALO));
    let bar = crate::game_ui::tool_bar::find_tool_bar(&image, &profile).unwrap();
    bar.pair(&profile)
        .unwrap()
        .padded(profile.tool_watch.margin)
}

#[test]
fn the_bar_is_found_on_a_whole_capture_then_only_its_area_is_captured() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 6);
    rig.run();
    let place = place_of(AT);
    assert_eq!(
        rig.areas(),
        [
            FrameArea::Whole,
            FrameArea::Part(place),
            FrameArea::Part(place),
            FrameArea::Part(place),
            FrameArea::Part(place),
            FrameArea::Part(place),
        ],
        "the place is kept in the client area's coordinates"
    );
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn a_moved_bar_is_found_again() {
    // Moved after the first whole search is long past: the look that no
    // longer finds it forgets its place, and the next searches the whole
    // window and finds it where it now stands.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 14);
    let moved = (150, 120);
    rig.show(11, scene_at(moved, Some(OPALO)), OPALO);
    rig.run();
    let areas = rig.areas();
    assert_eq!(areas[10], FrameArea::Part(place_of(AT)));
    assert_eq!(areas[11], FrameArea::Whole);
    assert_eq!(areas[12..], [FrameArea::Part(place_of(moved)); 2]);
    // Unseen for one look only: nothing changed for the player.
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn whole_window_searches_are_rate_limited() {
    // No bar anywhere: the whole window is searched every two seconds, and
    // no other look captures anything.
    let rig = rig(None, library(&[]), 17);
    *rig.image.lock().unwrap() = scenery(1000, 500, 5);
    rig.run();
    assert_eq!(rig.areas(), [FrameArea::Whole; 3]);
    assert_eq!(rig.capture_times(), [0, 2_000, 4_000]);
    // Seen unseen at the first and the second search: told once, with the
    // first one's time.
    assert_eq!(rig.told(), [(0, ToolSeen::NotVisible)]);
}

#[test]
fn nothing_is_recognised_while_the_lettering_holds_and_the_interval_has_not_passed() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 8);
    rig.run();
    assert_eq!(rig.areas().len(), 8);
    assert_eq!(rig.reads(), 1, "one read in the first 1.75 s");
}

#[test]
fn the_name_is_read_again_once_the_interval_has_passed() {
    // Looks at 0, 250, ... 2_000 ms: the ninth is two seconds after the
    // read.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 9);
    rig.run();
    assert_eq!(rig.reads(), 2);
}

#[test]
fn a_one_glyph_change_the_lettering_shows_is_read_at_once() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO), (2, OPALA)]), 4);
    rig.show(3, scene(Some(OPALA)), OPALA);
    rig.run();
    assert_eq!(rig.reads(), 2);
    assert_eq!(rig.told(), [(0, holding(1)), (500, holding(2))]);
}

#[test]
fn a_change_the_lettering_does_not_show_is_caught_by_the_interval() {
    // The recogniser reads another name over the same pixels: only the
    // interval's read sees it, and it is told with that look's time.
    let rig = rig(Some(OPALO), library(&[(1, OPALO), (2, OPALA)]), 10);
    let said = rig.answer.clone();
    rig.at(3, move || *said.lock().unwrap() = OPALA.to_string());
    rig.run();
    assert_eq!(rig.reads(), 2);
    assert_eq!(rig.told(), [(0, holding(1)), (2_000, holding(2))]);
}

#[test]
fn a_capture_slower_than_its_slack_is_not_used() {
    // The second capture lags its stamp by 400 ms, past the profile's
    // 300: the change it saw is told only from the next look, with that
    // look's time.
    let rig = rig(Some(OPALO), library(&[(1, OPALO), (2, OPALA)]), 3);
    rig.show(2, scene(Some(OPALA)), OPALA);
    let clock = rig.clock.clone();
    *rig.on_capture.lock().unwrap() = Some(Box::new(move |n| {
        if n == 2 {
            clock.advance(0.4).unwrap();
        }
    }));
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 750]);
    assert_eq!(rig.told(), [(0, holding(1)), (750, holding(2))]);
}

#[test]
fn each_bar_state_shows_what_it_means() {
    let bar = ToolBar {
        panel: Rect::new(0, 0, 371, 30),
        name: Rect::new(9, 5, 287, 25),
        counter: Rect::new(295, 5, 365, 25),
        scale: 0,
        view: View::Clear,
    };
    let unreadable = Some(ToolSeen::Uncertain(Why::Unreadable));
    assert_eq!(
        ToolSeen::of_bar(&BarState::Absent),
        Some(ToolSeen::NotVisible)
    );
    assert_eq!(
        ToolSeen::of_bar(&BarState::Covered(bar)),
        Some(ToolSeen::NotVisible)
    );
    assert_eq!(ToolSeen::of_bar(&BarState::Unclear(bar)), unreadable);
    // Nothing has been seen of what the client draws with nothing held: a
    // health bar alone is never taken for it.
    assert_eq!(ToolSeen::of_bar(&BarState::NoPanel(bar)), unreadable);
    assert_eq!(
        ToolSeen::of_bar(&BarState::Empty(bar)),
        Some(ToolSeen::Nothing)
    );
    assert_eq!(
        ToolSeen::of_bar(&BarState::Lettered(bar)),
        None,
        "to be read"
    );
}

#[test]
fn each_match_shows_what_it_means() {
    let bundled = BundledTool {
        endpoint: "weapons".into(),
        item_id: "77".into(),
        name: ARMAX.into(),
    };
    let candidates = vec![
        Candidate::Library {
            ids: vec![3],
            name: OPALO.into(),
        },
        Candidate::Bundled(bundled.clone()),
    ];
    let cases = [
        (ToolMatch::Library { id: 4 }, holding(4)),
        (
            ToolMatch::Bundled {
                endpoint: "weapons".into(),
                item_id: "77".into(),
                name: ARMAX.into(),
            },
            ToolSeen::Holding(HeldTool::Bundled {
                endpoint: "weapons".into(),
                item_id: "77".into(),
                name: ARMAX.into(),
            }),
        ),
        (
            ToolMatch::NotTracked {
                name: "Teleport Chip".into(),
            },
            ToolSeen::NotTracked {
                name: "Teleport Chip".into(),
            },
        ),
        (
            ToolMatch::Twins { ids: vec![1, 2] },
            ToolSeen::Uncertain(Why::Twins { ids: vec![1, 2] }),
        ),
        (
            ToolMatch::Disabled { ids: vec![5] },
            ToolSeen::Uncertain(Why::Disabled { ids: vec![5] }),
        ),
        (
            ToolMatch::Contended {
                candidates: candidates.clone(),
                more: 3,
            },
            ToolSeen::Uncertain(Why::Contended {
                candidates,
                more: 3,
            }),
        ),
        (
            ToolMatch::Unknown { text: "Qx".into() },
            ToolSeen::Uncertain(Why::Unknown { text: "Qx".into() }),
        ),
    ];
    for (found, seen) in cases {
        assert_eq!(ToolSeen::from(found.clone()), seen, "{found:?}");
    }
}

#[test]
fn an_empty_bar_holds_nothing_and_a_refused_read_is_unreadable() {
    // A whole bar of plain fill: nothing held, told at once.
    let rig = rig(None, library(&[]), 1);
    rig.run();
    assert_eq!(rig.told(), [(0, ToolSeen::Nothing)]);
    // The recogniser reading nothing: refused, and told once settled.
    let rig = rig_named_reading(OPALO, "", 3);
    rig.run();
    assert_eq!(rig.told(), [(0, ToolSeen::Uncertain(Why::Unreadable))]);
    // A refused read is not kept: every look reads again.
    assert_eq!(rig.reads(), 3);
}

/// A rig whose bar names `name` and whose recogniser reads `read`.
fn rig_named_reading(name: &str, read: &str, max_waits: usize) -> Rig {
    let rig = rig(Some(name), library(&[(1, name)]), max_waits);
    *rig.answer.lock().unwrap() = read.to_string();
    rig
}

#[test]
fn a_pulse_is_sent_only_on_a_change_and_a_re_read_of_the_same_tool_tells_nothing() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 12);
    rig.run();
    assert_eq!(rig.reads(), 2, "read again at two seconds");
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn the_first_conclusive_look_of_a_watch_tells() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 1);
    rig.run();
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn one_unclear_frame_between_two_good_ones_tells_nothing() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 6);
    rig.show(3, unclear_scene(), OPALO);
    rig.show(4, scene(Some(OPALO)), OPALO);
    rig.run();
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn an_unclear_bar_held_past_the_settle_time_tells_once_with_the_first_looks_time() {
    // Unclear from the third look (500 ms) on: held 500 ms by the fifth.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 9);
    rig.show(3, unclear_scene(), OPALO);
    rig.run();
    assert_eq!(
        rig.told(),
        [(0, holding(1)), (500, ToolSeen::Uncertain(Why::Unreadable))]
    );
}

#[test]
fn a_window_over_the_bar_is_not_visible() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 6);
    let mut covered = scene(Some(OPALO));
    let tool_x = AT.0 + crate::game_ui::test_support::TOOL_PITCH;
    draw_window(
        &mut covered,
        Rect::new(tool_x + 100, 300, tool_x + 300, 470),
    );
    rig.show(2, covered, OPALO);
    rig.run();
    assert_eq!(rig.told(), [(0, holding(1)), (250, ToolSeen::NotVisible)]);
}

#[test]
fn a_hint_looks_at_once_and_reads_again() {
    // A key pressed 100 ms after the second look: the third is taken then,
    // and reads the name though the lettering has not changed; the
    // cadence then runs from it.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 4);
    let (clock, watch) = (rig.clock.clone(), rig.watch.clone());
    rig.at(3, move || {
        clock.advance(0.1).unwrap();
        watch.hint();
    });
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 350, 600]);
    // Read at the hinted look, and at the next, within the hint's window.
    assert_eq!(rig.reads(), 3);
}

#[test]
fn hints_before_a_look_are_one_hint() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 4);
    let (clock, watch) = (rig.clock.clone(), rig.watch.clone());
    rig.at(3, move || {
        clock.advance(0.1).unwrap();
        for _ in 0..3 {
            watch.hint();
        }
    });
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 350, 600]);
    assert_eq!(rig.reads(), 3);
}

#[test]
fn a_hint_during_a_look_waits_for_it_and_for_the_least_gap() {
    // Hinted from inside the second look's capture: the look runs to its
    // end, and the hinted one is taken the profile's least gap after it
    // began, never beside it.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 5);
    let watch = rig.watch.clone();
    *rig.on_capture.lock().unwrap() = Some(Box::new(move |n| {
        if n == 2 {
            watch.hint();
        }
    }));
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 350, 600]);
    assert_eq!(rig.reads(), 3);
}

#[test]
fn hints_while_a_hinted_look_waits_out_the_gap_do_not_bring_it_forward() {
    // A key repeating every 33 ms after the second look: each hint wakes
    // the wait, but the one hinted look is taken at the gap, 100 ms after
    // the look before, and no other.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 8);
    for (n, delay) in [(3, 0.033), (4, 0.033), (5, 0.033)] {
        let (clock, watch) = (rig.clock.clone(), rig.watch.clone());
        rig.at(n, move || {
            clock.advance(delay).unwrap();
            watch.hint();
        });
    }
    rig.run();
    assert_eq!(rig.capture_times()[..4], [0, 250, 350, 600]);
}

#[test]
fn a_hint_keeps_the_name_read_for_a_while() {
    // The client has drawn the new name only by the second look after the
    // key, 500 ms on: with the same lettering, only the reads the hint
    // keeps up catch it, long before the re-read interval would.
    let rig = rig(Some(OPALO), library(&[(1, OPALO), (2, OPALA)]), 6);
    let (clock, watch) = (rig.clock.clone(), rig.watch.clone());
    rig.at(3, move || {
        clock.advance(0.1).unwrap();
        watch.hint();
    });
    let said = rig.answer.clone();
    rig.at(5, move || *said.lock().unwrap() = OPALA.to_string());
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 350, 600, 850, 1_100]);
    assert_eq!(rig.told(), [(0, holding(1)), (850, holding(2))]);
}

#[test]
fn a_hint_with_capture_not_running_starts_nothing() {
    // Capture stops while watching; a hint then makes the next look, which
    // finds capture not running and ends the watch without asking the
    // desktop to start it.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 4);
    let (running, watch) = (rig.running.clone(), rig.watch.clone());
    rig.at(2, move || {
        running.store(false, Ordering::SeqCst);
        watch.hint();
    });
    rig.run();
    assert_eq!(rig.watch.watching(), None);
    assert_eq!(rig.starts.load(Ordering::SeqCst), 0, "consent never asked");
    assert_eq!(rig.areas().len(), 1, "nothing captured after it stopped");
}

#[test]
fn a_replaced_catalogue_is_matched_again_without_a_change_on_the_bar() {
    // Not in the library at first: unknown, told once settled. The player
    // adds it: the next look matches again.
    let rig = rig(Some(OPALO), library(&[]), 5);
    let cell = rig.catalogue.clone();
    rig.at(4, move || cell.replace(library(&[(9, OPALO)])));
    rig.run();
    assert_eq!(rig.reads(), 2);
    assert_eq!(
        rig.told(),
        [
            (0, ToolSeen::Uncertain(Why::Unknown { text: OPALO.into() })),
            (750, holding(9)),
        ]
    );
}

#[test]
fn nothing_is_captured_or_told_while_the_game_is_in_the_background() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 5);
    *rig.focus.lock().unwrap() = GameFocus::Unfocused;
    let focus = rig.focus.clone();
    rig.at(4, move || *focus.lock().unwrap() = GameFocus::Focused);
    rig.run();
    assert_eq!(rig.capture_times(), [750, 1_000]);
    assert_eq!(rig.told(), [(750, holding(1))]);
}

#[test]
fn an_unknown_focus_does_not_stop_the_looks() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 2);
    *rig.focus.lock().unwrap() = GameFocus::Unknown;
    rig.run();
    assert_eq!(rig.areas().len(), 2);
    assert_eq!(rig.told(), [(0, holding(1))]);
}

/// Starts `s1` on `rig`, recording each session a self-ended watch tells
/// of.
fn start_reporting(rig: &Rig, sink: ToolPulseSink) -> Arc<Mutex<Vec<String>>> {
    let told = Arc::new(Mutex::new(Vec::new()));
    let tell = told.clone();
    ToolDetector::start(
        &rig.watch,
        "s1",
        sink,
        Arc::new(move |session: &str| tell.lock().unwrap().push(session.to_string())),
    );
    told
}

#[test]
fn a_refused_capture_ends_the_watch_and_tells_the_starter() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 4);
    let running = rig.running.clone();
    rig.at(2, move || running.store(false, Ordering::SeqCst));
    let ended = start_reporting(&rig, rig.sink());
    rig.finish();
    assert_eq!(*ended.lock().unwrap(), ["s1"]);
    assert_eq!(rig.watch.watching(), None);
    assert_eq!(
        rig.watch.capture_status(),
        crate::screen_watch::CaptureStatus::Refused
    );
    assert_eq!(rig.starts.load(Ordering::SeqCst), 0, "never restarted");
}

#[test]
fn a_sink_that_cannot_deliver_ends_the_watch() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 6);
    let ended = start_reporting(&rig, Arc::new(|_| false));
    rig.finish();
    assert_eq!(*ended.lock().unwrap(), ["s1"]);
    assert_eq!(rig.areas().len(), 1, "one look, then the watch ended");
}

#[test]
fn stopping_ends_a_waiting_worker_and_a_newer_watch_replaces_an_older() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 0);
    let blocking: PulseWait = Arc::new(|_, woken| woken.recv().is_ok());
    let watch = ToolWatch::new(
        ToolWatchProviders {
            frame: Arc::new(|_| unreachable!("no look is let through")),
            enabled: Arc::new(|| true),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(|_| None)),
            wait: Some(blocking),
            ..ToolWatchProviders::default()
        },
        rig.clock.clone(),
    );
    watch.start("s1", rig.sink());
    assert!(ToolDetector::watches(&watch, "s1"));
    watch.stop();
    assert!(watch.wait_for_workers(BOUND));
    watch.start("s1", rig.sink());
    watch.start("s2", rig.sink());
    assert_eq!(watch.watching().as_deref(), Some("s2"));
    ToolDetector::shutdown(&watch);
    assert!(watch.wait_for_workers(BOUND));
    assert_eq!(watch.watching(), None);
}

#[test]
fn shutting_down_does_not_wait_on_a_capture_that_does_not_return() {
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    let (entered, inside) = mpsc::channel::<()>();
    let entered = Mutex::new(entered);
    let clock = Arc::new(MockClock::new(None, 0.0));
    let watch = ToolWatch::new(
        ToolWatchProviders {
            frame: Arc::new(move |_| {
                let _ = entered.lock().unwrap().send(());
                let _ = held.lock().unwrap().recv_timeout(BOUND);
                Err(FrameUnavailable::CaptureFailed)
            }),
            enabled: Arc::new(|| true),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(|_| None)),
            wait: Some(Arc::new(|_, woken| {
                !matches!(woken.try_recv(), Err(TryRecvError::Disconnected))
            })),
            ..ToolWatchProviders::default()
        },
        clock,
    );
    watch.start("s1", Arc::new(|_| true));
    inside.recv_timeout(BOUND).expect("the look is capturing");
    watch.shutdown();
    assert!(
        !watch.wait_for_workers(Duration::ZERO),
        "shutdown returned with the capture still out"
    );
    drop(release);
    assert!(watch.wait_for_workers(BOUND));
}

#[test]
fn a_look_finishing_after_its_watch_stopped_is_dropped() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 3);
    let watch = rig.watch.clone();
    *rig.on_capture.lock().unwrap() = Some(Box::new(move |_| watch.stop()));
    rig.run();
    assert_eq!(rig.areas().len(), 1);
    assert!(rig.told().is_empty(), "the stale look told nothing");
}

#[test]
fn a_detector_bound_late_serves_once_bound() {
    let slot = ToolDetectorSlot::default();
    slot.hint();
    assert!(!slot.watches("s1"), "unbound, nothing is watched");
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 1);
    slot.bind(Arc::new(rig.watch.clone()));
    slot.start("s1", rig.sink(), Arc::new(|_: &str| {}));
    rig.finish();
    assert_eq!(rig.told(), [(0, holding(1))]);
    assert!(!NoToolDetector.watches("s1"));
}

#[test]
fn two_watchers_never_capture_at_once_and_one_slow_read_does_not_hold_up_the_other() {
    // The held-tool watcher's read blocks until released; meanwhile the
    // creature watcher captures, which it could not if the read held the
    // capture lock. Every capture of either checks it holds the lock alone.
    // A capture holds `entered` for as long as it runs; entering while it
    // is held is an overlap. The held-tool watcher's first capture lingers
    // until the creature watcher enters a capture, or half a second, so a
    // capture lock that did not hold would be caught inside it.
    let entered = Arc::new(AtomicBool::new(false));
    let overlapped = Arc::new(AtomicBool::new(false));
    let (inside, tool_inside) = mpsc::channel::<()>();
    let inside = Arc::new(Mutex::new(inside));
    let (mob_entered, mob_in) = mpsc::channel::<()>();
    let mob_entered = Arc::new(Mutex::new(mob_entered));
    let mob_in = Arc::new(Mutex::new(mob_in));
    let capture = |image: BgrImage, lingers: bool| -> GameWindowFrame {
        let (entered, overlapped) = (entered.clone(), overlapped.clone());
        let (inside, mob_entered, mob_in) = (inside.clone(), mob_entered.clone(), mob_in.clone());
        let first = AtomicBool::new(true);
        Arc::new(move |area| {
            if entered.swap(true, Ordering::SeqCst) || !capture_is_gated() {
                overlapped.store(true, Ordering::SeqCst);
            }
            if lingers && first.swap(false, Ordering::SeqCst) {
                let _ = inside.lock().unwrap().send(());
                let _ = mob_in
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_millis(500));
            } else if !lingers {
                let _ = mob_entered.lock().unwrap().send(());
            }
            entered.store(false, Ordering::SeqCst);
            Ok(capture_of(&image, area))
        })
    };
    let (reading, in_read) = mpsc::channel::<()>();
    let reading = Mutex::new(reading);
    let (release, released) = mpsc::channel::<()>();
    let released = Mutex::new(released);
    let tool = ToolWatch::new(
        ToolWatchProviders {
            frame: capture(scene(Some(OPALO)), true),
            enabled: Arc::new(|| true),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(move |_| {
                let _ = reading.lock().unwrap().send(());
                let _ = released.lock().unwrap().recv_timeout(BOUND);
                Some((OPALO.to_string(), 0.99))
            })),
            catalogue: Arc::new(ToolCatalogueCell::new(library(&[(1, OPALO)]))),
            // Its first look is let through at once; later ones wait to
            // be stopped.
            wait: Some({
                let first = AtomicBool::new(true);
                Arc::new(move |_, woken| {
                    first.swap(false, Ordering::SeqCst) || woken.recv().is_ok()
                })
            }),
            ..ToolWatchProviders::default()
        },
        Arc::new(MockClock::new(None, 0.0)),
    );
    tool.start("s1", Arc::new(|_| true));
    tool_inside
        .recv_timeout(BOUND)
        .expect("the held-tool capture began");

    let mob_clock = Arc::new(MockClock::new(None, 0.0));
    let (pulsed, pulses) = mpsc::channel::<()>();
    let pulsed = Mutex::new(pulsed);
    let mob = MobWatch::new(
        MobWatchProviders {
            frame: capture(blank(320, 200, [40, 60, 30]), false),
            enabled: Arc::new(|| true),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(Arc::new(|_| None)),
            catalogue: Arc::new(crate::mob_watch::CatalogueCell::new(MobCatalogue::default())),
            wait: Some({
                let clock = mob_clock.clone();
                Arc::new(move |due, woken| {
                    let now = clock.monotonic();
                    if due > now {
                        clock.advance(due - now).unwrap();
                    }
                    !matches!(woken.try_recv(), Err(TryRecvError::Disconnected))
                })
            }),
            ..MobWatchProviders::default()
        },
        mob_clock,
    );
    let sink: PulseSink = Arc::new(move |_: MobPulse| {
        let _ = pulsed.lock().unwrap().send(());
        true
    });
    mob.start("s1", sink);
    in_read
        .recv_timeout(BOUND)
        .expect("the held-tool read began");
    for _ in 0..3 {
        pulses
            .recv_timeout(BOUND)
            .expect("the creature watcher captures while the tool is being read");
    }
    mob.stop();
    drop(release);
    tool.stop();
    assert!(mob.wait_for_workers(BOUND) && tool.wait_for_workers(BOUND));
    assert!(
        !overlapped.load(Ordering::SeqCst),
        "no two captures at once"
    );
}

#[test]
fn the_metrics_grow_by_what_was_done() {
    // Three looks: a whole capture and two of the bar's area, each searched,
    // and one read.
    let before = eo_wire::metrics::metrics().snapshot();
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 3);
    rig.run();
    let after = eo_wire::metrics::metrics().snapshot();
    let grew = |a: &eo_wire::metrics::HistogramSnapshot,
                b: &eo_wire::metrics::HistogramSnapshot| { a.count - b.count };
    // Other tests share the registry, so it has grown by at least this.
    assert!(grew(&after.tool_capture_latency, &before.tool_capture_latency) >= 3);
    assert!(grew(&after.tool_search_latency, &before.tool_search_latency) >= 3);
    assert!(grew(&after.tool_read_latency, &before.tool_read_latency) >= 1);
}

/// What `memory` tells for each look of `looks` (its answer, and when it
/// was captured in milliseconds), with the bundled profile's settle time.
fn settled(memory: &mut BarMemory, looks: &[(i64, ToolSeen)]) -> Vec<(i64, ToolSeen)> {
    let settle = seconds(bundled_profile().tool_watch.settle_ms);
    looks
        .iter()
        .filter_map(|(ms, seen)| memory.settle(seen.clone(), *ms, *ms as f64 / 1000.0, settle))
        .collect()
}

fn unknown(text: &str) -> ToolSeen {
    ToolSeen::Uncertain(Why::Unknown { text: text.into() })
}

#[test]
fn an_unclear_and_a_covered_bar_alternating_tell_uncertain_never_the_old_tool() {
    let unreadable = ToolSeen::Uncertain(Why::Unreadable);
    let mut looks = vec![(0, holding(1))];
    for k in 1..13 {
        let seen = if k % 2 == 1 {
            unreadable.clone()
        } else {
            ToolSeen::NotVisible
        };
        looks.push((k * 250, seen));
    }
    let mut memory = BarMemory::default();
    // Mixed evidence never claims an unseen bar, which would keep the tool.
    assert_eq!(
        settled(&mut memory, &looks),
        [(0, holding(1)), (250, unreadable)]
    );
}

#[test]
fn two_unknown_readings_alternating_tell_the_latest_once_settled() {
    let looks = [
        (0, holding(1)),
        (250, unknown("Opalx")),
        (500, unknown("Opaly")),
        (750, unknown("Opalx")),
        (1_000, unknown("Opaly")),
    ];
    assert_eq!(
        settled(&mut BarMemory::default(), &looks),
        [(0, holding(1)), (250, unknown("Opalx"))]
    );
}

#[test]
fn a_bar_covered_throughout_its_run_tells_not_visible() {
    let looks = [
        (0, holding(1)),
        (250, ToolSeen::NotVisible),
        (500, ToolSeen::NotVisible),
        (750, ToolSeen::NotVisible),
    ];
    assert_eq!(
        settled(&mut BarMemory::default(), &looks),
        [(0, holding(1)), (250, ToolSeen::NotVisible)]
    );
}

#[test]
fn after_an_uncertain_bar_is_told_another_is_told_only_once_it_holds_unbroken() {
    let unreadable = ToolSeen::Uncertain(Why::Unreadable);
    let looks = [
        (0, unreadable.clone()),
        (500, unreadable.clone()),
        // Covered, broken by one unreadable look, then covered unbroken.
        (750, ToolSeen::NotVisible),
        (1_000, unreadable.clone()),
        (1_250, ToolSeen::NotVisible),
        (1_500, ToolSeen::NotVisible),
        (1_750, ToolSeen::NotVisible),
    ];
    assert_eq!(
        settled(&mut BarMemory::default(), &looks),
        [(0, unreadable), (1_250, ToolSeen::NotVisible)]
    );
}

#[test]
fn a_run_of_failed_captures_after_a_tool_tells_unreadable() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 5);
    let failing = rig.failing.clone();
    rig.at(2, move || {
        *failing.lock().unwrap() = Some(FrameUnavailable::CaptureFailed)
    });
    rig.run();
    assert_eq!(
        rig.told(),
        [(0, holding(1)), (250, ToolSeen::Uncertain(Why::Unreadable))]
    );
}

#[test]
fn no_game_window_tells_nothing() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 9);
    let failing = rig.failing.clone();
    rig.at(2, move || {
        *failing.lock().unwrap() = Some(FrameUnavailable::WindowNotFound)
    });
    rig.run();
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn a_run_of_slow_captures_after_a_tool_tells_unreadable() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 3);
    let clock = rig.clock.clone();
    *rig.on_capture.lock().unwrap() = Some(Box::new(move |n| {
        if n >= 2 {
            clock.advance(0.4).unwrap();
        }
    }));
    rig.run();
    assert_eq!(rig.capture_times(), [0, 250, 750]);
    assert_eq!(
        rig.told(),
        [(0, holding(1)), (250, ToolSeen::Uncertain(Why::Unreadable))]
    );
}

#[test]
fn one_refused_read_between_two_good_reads_of_the_same_tool_tells_nothing() {
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 4);
    // Other lettering, read as nothing; then the tool again.
    rig.show(2, scene(Some(OPALA)), "");
    rig.show(3, scene(Some(OPALO)), OPALO);
    rig.run();
    assert_eq!(rig.reads(), 3);
    assert_eq!(rig.told(), [(0, holding(1))]);
}

#[test]
fn a_name_not_tracked_is_told_at_once() {
    const CHIP: &str = "Teleport Chip";
    let catalogue = ToolCatalogue::new(
        [LibraryTool {
            id: 1,
            name: OPALO.into(),
            kind: ToolKind::Weapon,
            enabled: true,
        }],
        Vec::<BundledTool>::new(),
        [CHIP.to_string()],
    );
    let rig = rig(Some(OPALO), catalogue, 3);
    rig.show(3, scene(Some(CHIP)), CHIP);
    rig.run();
    assert_eq!(
        rig.told(),
        [
            (0, holding(1)),
            (500, ToolSeen::NotTracked { name: CHIP.into() })
        ]
    );
}

#[test]
fn a_window_larger_than_the_captured_area_is_not_visible_and_costs_one_whole_capture_a_search() {
    // A window over all but the health bar's left end, reaching far past
    // the bar's area on every side but the left.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 19);
    let mut covered = scene(Some(OPALO));
    draw_window(&mut covered, Rect::new(AT.0 + 30, 120, 995, 495));
    rig.show(2, covered, OPALO);
    rig.run();
    let told = rig.told();
    assert_eq!(told, [(0, holding(1)), (500, ToolSeen::NotVisible)]);
    // Within the settle time and one look of the window's coming.
    let wholes: Vec<i64> = rig
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|(area, _)| *area == FrameArea::Whole)
        .map(|(_, at)| ((at - 100.0) * 1000.0).round() as i64)
        .collect();
    assert_eq!(wholes, [0, 500, 2_500, 4_500]);
}

#[test]
fn a_capture_waiting_on_the_lock_is_stamped_when_it_takes_it() {
    // Another capture holds the lock for half a second of the clock: the
    // look waiting behind it is stamped when it gets the lock, and is not
    // taken for a capture too slow to use.
    let rig = rig(Some(OPALO), library(&[(1, OPALO)]), 1);
    let held = capture_gate();
    rig.watch.start("s1", rig.sink());
    std::thread::sleep(Duration::from_millis(100));
    rig.clock.advance(0.5).unwrap();
    drop(held);
    rig.finish();
    assert_eq!(rig.told(), [(500, holding(1))]);
}
