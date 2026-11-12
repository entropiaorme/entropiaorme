use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::clock::MockClock;
use crate::game_ui::mob_names::{CatalogueMob, Resolved, Spelling};
use crate::game_ui::seams::GameFrame;
use crate::game_ui::test_support::{blank, bundled_profile, PlateDrawing, RadarScene};

/// The watcher over stand-in seams: a frame seam counting its captures
/// and running `on_capture` first (a slow capture advances the clock), a
/// wait that moves the mock clock straight to the moment a look is due
/// and gives up after `max_waits`, and a sink that stops the watch after
/// `pulses` looks.
struct Rig {
    clock: Arc<MockClock>,
    captures: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<MobPulse>>>,
    watch: Arc<MobWatch>,
    focus: Arc<Mutex<GameFocus>>,
    /// Whether capture is running: the frame seam captures only then, as
    /// the app's running-only seam does.
    running: Arc<AtomicBool>,
    /// Whether starting capture succeeds (the player's consent), and how
    /// often it was asked.
    start_ok: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
    /// The live detection setting.
    enabled: Arc<AtomicBool>,
}

type OnCapture =
    Arc<dyn Fn(usize, &MockClock) -> Result<GameFrame, FrameUnavailable> + Send + Sync>;

/// Builds the rig's wait over its clock and its focus.
type WaitMaker = Box<dyn FnOnce(Arc<MockClock>, Arc<Mutex<GameFocus>>) -> PulseWait>;

fn driven() -> WaitMaker {
    Box::new(|clock, _| driven_wait(clock, 50))
}

fn blank_frame() -> Result<GameFrame, FrameUnavailable> {
    Ok(GameFrame::whole(blank(320, 200, [40, 60, 30]), (0, 0)))
}

fn rig_with(
    on_capture: OnCapture,
    wait: WaitMaker,
    read_text: Option<FrameReader>,
    catalogue: MobCatalogue,
) -> Rig {
    let clock = Arc::new(MockClock::new(None, 100.0));
    let captures = Arc::new(AtomicUsize::new(0));
    let focus = Arc::new(Mutex::new(GameFocus::Focused));
    let running = Arc::new(AtomicBool::new(true));
    let start_ok = Arc::new(AtomicBool::new(true));
    let starts = Arc::new(AtomicUsize::new(0));
    let enabled = Arc::new(AtomicBool::new(true));
    let frame: GameWindowFrame = {
        let clock = clock.clone();
        let captures = captures.clone();
        let running = running.clone();
        Arc::new(move |area| {
            if !running.load(Ordering::SeqCst) {
                return Err(FrameUnavailable::CaptureNotStarted);
            }
            assert_eq!(
                area,
                FrameArea::Whole,
                "the watcher captures the whole window"
            );
            let n = captures.fetch_add(1, Ordering::SeqCst) + 1;
            on_capture(n, &clock)
        })
    };
    let wait = wait(clock.clone(), focus.clone());
    let probe = focus.clone();
    let watch = MobWatch::new(
        MobWatchProviders {
            frame,
            focus: Some(Arc::new(move || *probe.lock().unwrap())),
            profile: Some(Arc::new(bundled_profile())),
            read_text: Some(read_text.unwrap_or_else(|| Arc::new(|_: &BgrImage| None))),
            catalogue: Arc::new(CatalogueCell::new(catalogue)),
            wait: Some(wait),
            capture_running: {
                let running = running.clone();
                Arc::new(move || running.load(Ordering::SeqCst))
            },
            start_capture: {
                let (running, start_ok, starts) =
                    (running.clone(), start_ok.clone(), starts.clone());
                Arc::new(move || {
                    starts.fetch_add(1, Ordering::SeqCst);
                    let ok = start_ok.load(Ordering::SeqCst);
                    if ok {
                        running.store(true, Ordering::SeqCst);
                    }
                    ok
                })
            },
            enabled: {
                let enabled = enabled.clone();
                Arc::new(move || enabled.load(Ordering::SeqCst))
            },
        },
        clock.clone(),
    );
    Rig {
        clock,
        captures,
        seen: Arc::new(Mutex::new(Vec::new())),
        watch,
        focus,
        running,
        start_ok,
        starts,
        enabled,
    }
}

/// A wait that moves the clock to the moment the look is due (never
/// back), and answers whether the watch still runs; past `max_waits` it
/// gives up, so a test that went wrong ends instead of spinning.
fn driven_wait(clock: Arc<MockClock>, max_waits: usize) -> PulseWait {
    let waits = AtomicUsize::new(0);
    Arc::new(move |due: f64, stopped: &mpsc::Receiver<()>| {
        if waits.fetch_add(1, Ordering::SeqCst) >= max_waits {
            return false;
        }
        let now = clock.monotonic();
        if due > now {
            clock.advance(due - now).unwrap();
        }
        matches!(stopped.try_recv(), Err(TryRecvError::Empty))
    })
}

impl Rig {
    fn start(&self, session: &str, pulses: usize) {
        let seen = self.seen.clone();
        let watch = self.watch.clone();
        let sink: PulseSink = Arc::new(move |pulse| {
            let mut seen = seen.lock().unwrap();
            seen.push(pulse);
            if seen.len() >= pulses {
                watch.stop();
            }
            true
        });
        self.watch.start(session, sink);
    }

    fn finish(&self) {
        assert!(
            self.watch.wait_for_workers(Duration::from_secs(10)),
            "the worker ended"
        );
    }

    /// Each look's time, in milliseconds after the clock's start.
    fn offsets(&self, start_ms: i64) -> Vec<i64> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|pulse| pulse.at_ms - start_ms)
            .collect()
    }
}

#[test]
fn the_next_look_is_the_first_still_to_come_on_the_cadence() {
    assert_eq!(next_due(10.0, 1.0, 10.4), 11.0, "on time");
    assert_eq!(
        next_due(10.0, 1.0, 11.0),
        12.0,
        "the look due as it ends is dropped"
    );
    assert_eq!(
        next_due(10.0, 1.0, 12.5),
        13.0,
        "two looks overrun, both dropped"
    );
    assert_eq!(next_due(10.0, 0.5, 10.2), 10.5);
}

#[test]
fn looks_keep_the_cadence_and_a_slow_look_drops_the_ones_it_overran() {
    // The third capture takes two and a half seconds: the looks due at
    // three and four seconds came due while it ran, and are dropped
    // rather than taken late one after another; and a capture that slow no
    // longer stands for the moment it was stamped with, so it is a gap.
    let rig = rig_with(
        Arc::new(|n, clock| {
            if n == 3 {
                clock.advance(2.5).unwrap();
            }
            blank_frame()
        }),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let start = clock_epoch_ms(rig.clock.as_ref());
    rig.start("s1", 5);
    rig.finish();
    assert_eq!(rig.offsets(start), [0, 1_000, 5_000, 6_000, 7_000]);
    assert_eq!(
        rig.captures.load(Ordering::SeqCst),
        6,
        "one capture per look"
    );
    let seen = rig.seen.lock().unwrap();
    assert!(
        seen.iter()
            .all(|pulse| pulse.session_id == "s1" && pulse.sighting.is_none()),
        "a frame with no plate is a look with nothing locked"
    );
}

#[test]
fn the_cadence_comes_from_the_profile() {
    let mut profile = bundled_profile();
    profile.mob_history.pulse_ms = 250;
    let clock = Arc::new(MockClock::new(None, 0.0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let watch = MobWatch::new(
        MobWatchProviders {
            frame: Arc::new(|_| blank_frame()),
            profile: Some(Arc::new(profile)),
            read_text: Some(Arc::new(|_: &BgrImage| None)),
            wait: Some(driven_wait(clock.clone(), 50)),
            enabled: Arc::new(|| true),
            ..MobWatchProviders::default()
        },
        clock.clone(),
    );
    let start = clock_epoch_ms(clock.as_ref());
    let sink: PulseSink = {
        let seen = seen.clone();
        let watch = watch.clone();
        Arc::new(move |pulse: MobPulse| {
            let mut seen = seen.lock().unwrap();
            seen.push(pulse.at_ms - start);
            if seen.len() == 4 {
                watch.stop();
            }
            true
        })
    };
    watch.start("s1", sink);
    assert!(watch.wait_for_workers(Duration::from_secs(10)));
    assert_eq!(*seen.lock().unwrap(), [0, 250, 500, 750]);
}

#[test]
fn nothing_is_captured_while_another_window_holds_focus() {
    // Three looks pass with the game in the background: none captures and
    // none reaches the history (the screen was not watched then, which is
    // not the same as seeing nothing locked). The game coming forward
    // resumes the looks on the cadence.
    let comes_forward: WaitMaker = Box::new(|clock, focus| {
        let inner = driven_wait(clock, 50);
        let waits = AtomicUsize::new(0);
        Arc::new(move |due, stopped| {
            if waits.fetch_add(1, Ordering::SeqCst) == 3 {
                *focus.lock().unwrap() = GameFocus::Focused;
            }
            inner(due, stopped)
        })
    });
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        comes_forward,
        None,
        MobCatalogue::default(),
    );
    *rig.focus.lock().unwrap() = GameFocus::Unfocused;
    let start = clock_epoch_ms(rig.clock.as_ref());
    rig.start("s1", 2);
    rig.finish();
    assert_eq!(rig.offsets(start), [3_000, 4_000]);
    assert_eq!(
        rig.captures.load(Ordering::SeqCst),
        2,
        "no capture in the background"
    );
}

#[test]
fn an_unknown_focus_does_not_stop_the_looks() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    *rig.focus.lock().unwrap() = GameFocus::Unknown;
    rig.start("s1", 2);
    rig.finish();
    assert_eq!(rig.seen.lock().unwrap().len(), 2);
}

#[test]
fn a_window_that_cannot_be_captured_is_a_gap_not_an_empty_look() {
    let rig = rig_with(
        Arc::new(|n, _| match n {
            2 => Err(FrameUnavailable::WindowNotFound),
            3 => Err(FrameUnavailable::OutsideCapturableArea),
            4 => Err(FrameUnavailable::CaptureFailed),
            _ => blank_frame(),
        }),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let start = clock_epoch_ms(rig.clock.as_ref());
    rig.start("s1", 2);
    rig.finish();
    assert_eq!(rig.offsets(start), [0, 4_000]);
}

#[test]
fn a_refused_capture_stops_the_watch_until_the_player_turns_detection_back_on() {
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let gate = refuse.clone();
    let rig = rig_with(
        Arc::new(move |_, _| {
            if gate.load(Ordering::SeqCst) {
                Err(FrameUnavailable::CaptureNotStarted)
            } else {
                blank_frame()
            }
        }),
        driven(),
        None,
        MobCatalogue::default(),
    );
    rig.start("s1", 1);
    rig.finish();
    assert_eq!(rig.captures.load(Ordering::SeqCst), 1, "asked once");
    assert_eq!(rig.watch.watching(), None, "the watch ended");

    // A later session does not ask the desktop again by itself.
    rig.start("s2", 1);
    rig.finish();
    assert_eq!(rig.captures.load(Ordering::SeqCst), 1, "not asked again");

    refuse.store(false, Ordering::SeqCst);
    rig.watch.permit_capture();
    rig.start("s2", 1);
    rig.finish();
    assert_eq!(rig.captures.load(Ordering::SeqCst), 2);
    assert_eq!(rig.seen.lock().unwrap().len(), 1);
}

#[test]
fn stopping_ends_a_waiting_worker_at_once() {
    // The wait only ends when told to stop: the worker must be woken by
    // the stop itself, not by its next look coming due.
    let blocking: WaitMaker = Box::new(|_, _| Arc::new(|_, stopped| stopped.recv().is_ok()));
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        blocking,
        None,
        MobCatalogue::default(),
    );
    rig.start("s1", 1);
    assert_eq!(rig.watch.watching().as_deref(), Some("s1"));
    rig.watch.stop();
    rig.finish();
    assert_eq!(rig.captures.load(Ordering::SeqCst), 0);

    // A new session's watch replaces the running one, whose worker ends.
    rig.start("s1", 1);
    rig.start("s2", 1);
    assert_eq!(rig.watch.watching().as_deref(), Some("s2"));
    rig.watch.shutdown();
    rig.finish();
    assert_eq!(rig.watch.watching(), None);
}

#[test]
fn nothing_starts_without_the_profile_or_the_recogniser() {
    let captures = Arc::new(AtomicUsize::new(0));
    let counted = captures.clone();
    let watch = MobWatch::new(
        MobWatchProviders {
            frame: Arc::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                blank_frame()
            }),
            profile: Some(Arc::new(bundled_profile())),
            read_text: None,
            ..MobWatchProviders::default()
        },
        Arc::new(MockClock::new(None, 0.0)),
    );
    watch.start("s1", Arc::new(|_| true));
    assert_eq!(watch.watching(), None);
    assert!(watch.wait_for_workers(Duration::from_secs(1)));
    assert_eq!(captures.load(Ordering::SeqCst), 0);
    assert!(MobDetector::new_history(&watch).is_none());
}

#[test]
fn a_look_reads_the_locked_plate_and_matches_its_name() {
    let mut scene = RadarScene::sized(1100, 600, 7);
    scene.plate(&PlateDrawing::locked((420, 300), 0.6), "L5", "Atrox Young");
    let image = scene.image.clone();
    let recogniser = scene.recogniser();
    let catalogue = MobCatalogue::new(
        [CatalogueMob {
            name: "Atrox".into(),
            species: Some("Atrox".into()),
            maturity: Some("Young".into()),
            level: Some(5),
        }],
        Vec::<String>::new(),
        &bundled_profile().mob_names,
    );
    let before = eo_wire::metrics::metrics().snapshot();
    let rig = rig_with(
        Arc::new(move |_, _| Ok(GameFrame::whole(image.clone(), (0, 0)))),
        driven(),
        Some(Arc::new(recogniser)),
        catalogue,
    );
    rig.start("s1", 1);
    rig.finish();
    let seen = rig.seen.lock().unwrap();
    let sighting = seen[0].sighting.as_ref().expect("a plate was locked");
    assert_eq!(sighting.raw_name.as_deref(), Some("Atrox Young"));
    assert_eq!(
        sighting.resolved,
        Resolved::Known {
            name: "Atrox".into(),
            species: Some("Atrox".into()),
            maturity: Some("Young".into()),
            spelling: Spelling::Exact,
        }
    );
    assert!(
        sighting.fill > 0.5 && sighting.fill < 0.7,
        "{}",
        sighting.fill
    );
    // Every stage of the look was measured (other tests share the
    // registry, so it has grown by at least this look).
    let after = eo_wire::metrics::metrics().snapshot();
    assert!(after.plate_capture_latency.count > before.plate_capture_latency.count);
    assert!(after.plate_search_latency.count > before.plate_search_latency.count);
    assert!(after.plate_read_latency.count > before.plate_read_latency.count);
}

#[test]
fn a_detector_bound_late_serves_once_bound() {
    let slot = DetectorSlot::default();
    assert!(
        slot.new_history().is_none(),
        "unbound, detection does not run"
    );
    slot.stop();
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    slot.bind(Arc::new(rig.watch.clone()));
    assert!(slot.new_history().is_some());
    let seen = rig.seen.clone();
    let watch = rig.watch.clone();
    slot.start(
        "s1",
        Arc::new(move |pulse| {
            seen.lock().unwrap().push(pulse);
            watch.stop();
            true
        }),
        Arc::new(|_: &str| {}),
    );
    rig.finish();
    assert_eq!(rig.seen.lock().unwrap().len(), 1);
}

#[test]
fn a_session_starting_while_capture_is_not_running_starts_no_watch_and_asks_nothing() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    rig.running.store(false, Ordering::SeqCst);
    assert_eq!(rig.watch.capture_status(), CaptureStatus::NotRunning);
    rig.start("s1", 1);
    assert_eq!(
        rig.watch.watching(),
        None,
        "no watch without capture running"
    );
    rig.finish();
    assert_eq!(
        rig.starts.load(Ordering::SeqCst),
        0,
        "the consent was never asked"
    );
    assert_eq!(rig.captures.load(Ordering::SeqCst), 0);

    // Turning detection on is the gesture: the next look asks, once.
    rig.watch.permit_capture();
    assert_eq!(rig.watch.capture_status(), CaptureStatus::Ready);
    rig.start("s1", 2);
    rig.finish();
    assert_eq!(
        rig.starts.load(Ordering::SeqCst),
        1,
        "asked once, not per look"
    );
    assert_eq!(rig.seen.lock().unwrap().len(), 2);
    assert_eq!(rig.watch.capture_status(), CaptureStatus::Ready);
}

#[test]
fn a_declined_consent_ends_the_watch_until_the_player_turns_detection_off_and_on() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    rig.running.store(false, Ordering::SeqCst);
    rig.start_ok.store(false, Ordering::SeqCst);
    rig.watch.permit_capture();
    rig.start("s1", 1);
    rig.finish();
    assert_eq!(rig.starts.load(Ordering::SeqCst), 1);
    assert_eq!(rig.captures.load(Ordering::SeqCst), 0);
    assert_eq!(rig.watch.capture_status(), CaptureStatus::Refused);
    rig.start("s2", 1);
    rig.finish();
    assert_eq!(
        rig.starts.load(Ordering::SeqCst),
        1,
        "not asked again by a session"
    );
}

#[test]
fn a_stream_stopped_while_watching_ends_the_watch_and_is_not_reopened() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let stop_after_two = {
        let flag = rig.running.clone();
        let seen = rig.seen.clone();
        move || {
            if seen.lock().unwrap().len() >= 2 {
                flag.store(false, Ordering::SeqCst);
            }
        }
    };
    let seen = rig.seen.clone();
    let sink: PulseSink = Arc::new(move |pulse| {
        seen.lock().unwrap().push(pulse);
        stop_after_two();
        true
    });
    rig.watch.start("s1", sink);
    rig.finish();
    assert_eq!(rig.seen.lock().unwrap().len(), 2);
    assert_eq!(rig.watch.watching(), None);
    assert_eq!(rig.watch.capture_status(), CaptureStatus::Refused);
    assert_eq!(
        rig.starts.load(Ordering::SeqCst),
        0,
        "never restarted by itself"
    );
}

#[test]
fn the_watch_ends_when_its_looks_can_no_longer_be_delivered() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let delivered = Arc::new(AtomicUsize::new(0));
    let counted = delivered.clone();
    rig.watch.start(
        "s1",
        Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            false
        }),
    );
    rig.finish();
    assert_eq!(
        delivered.load(Ordering::SeqCst),
        1,
        "one look, then the watch ended"
    );
    assert_eq!(rig.watch.watching(), None);
}

#[test]
fn the_setting_is_checked_where_the_capture_happens() {
    let rig = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    rig.enabled.store(false, Ordering::SeqCst);
    rig.start("s1", 1);
    rig.finish();
    assert_eq!(
        rig.captures.load(Ordering::SeqCst),
        0,
        "turned off: nothing captured"
    );
    assert_eq!(rig.watch.watching(), None);
}

#[test]
fn a_capture_slower_than_its_slack_is_a_gap() {
    // The profile allows a capture 300 ms behind its stamp.
    let rig = rig_with(
        Arc::new(|n, clock| {
            clock.advance(if n == 2 { 0.4 } else { 0.2 }).unwrap();
            blank_frame()
        }),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let start = clock_epoch_ms(rig.clock.as_ref());
    rig.start("s1", 2);
    rig.finish();
    assert_eq!(rig.offsets(start), [0, 2_000]);
}

/// Starts `session` on `rig`, recording each session a self-ended watch
/// tells of.
fn start_reporting(rig: &Rig, session: &str) -> Arc<Mutex<Vec<String>>> {
    let told = Arc::new(Mutex::new(Vec::new()));
    let tell = told.clone();
    rig.watch.start_reporting_end(
        session,
        Arc::new(|_| true),
        Arc::new(move |session_id: &str| tell.lock().unwrap().push(session_id.to_string())),
    );
    told
}

#[test]
fn a_watch_that_ends_by_itself_says_so_once_and_one_stopped_says_nothing() {
    // Refused at its first look.
    let refused = rig_with(
        Arc::new(|_, _| Err(FrameUnavailable::CaptureNotStarted)),
        driven(),
        None,
        MobCatalogue::default(),
    );
    let told = start_reporting(&refused, "s1");
    refused.finish();
    assert_eq!(*told.lock().unwrap(), ["s1"]);

    // Detection found off where the capture happens.
    let off = rig_with(
        Arc::new(|_, _| blank_frame()),
        driven(),
        None,
        MobCatalogue::default(),
    );
    off.enabled.store(false, Ordering::SeqCst);
    let told = start_reporting(&off, "s2");
    off.finish();
    assert_eq!(*told.lock().unwrap(), ["s2"]);

    // Stopped by its owner while waiting: nothing is told.
    let blocking: WaitMaker = Box::new(|_, _| Arc::new(|_, stopped| stopped.recv().is_ok()));
    let stopped = rig_with(
        Arc::new(|_, _| blank_frame()),
        blocking,
        None,
        MobCatalogue::default(),
    );
    let told = start_reporting(&stopped, "s3");
    stopped.watch.stop();
    stopped.finish();
    assert!(told.lock().unwrap().is_empty());
}
