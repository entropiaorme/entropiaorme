use std::time::Instant;

use super::*;
use crate::clock::MockClock;

#[test]
fn a_hint_wakes_a_real_time_wait_and_a_dropped_sender_stops_it() {
    // The clock stands still, so a wait for a moment a minute ahead ends
    // only by what comes through the channel.
    let clock = MockClock::new(None, 0.0);
    let (wake, woken) = mpsc::channel();
    wake.send(()).unwrap();
    let started = Instant::now();
    assert!(wait_on_clock(&clock, 60.0, &woken), "a hint: go on");
    drop(wake);
    assert!(!wait_on_clock(&clock, 60.0, &woken), "stopped");
    assert!(started.elapsed() < Duration::from_secs(30));

    // A moment already passed: go on unless stopped, a hint or not.
    let (wake, woken) = mpsc::channel();
    assert!(wait_on_clock(&clock, 0.0, &woken));
    wake.send(()).unwrap();
    assert!(wait_on_clock(&clock, 0.0, &woken));
    drop(wake);
    assert!(!wait_on_clock(&clock, 0.0, &woken));
}

#[test]
fn a_replaced_value_is_what_the_next_reader_gets() {
    let cell = SwapCell::new(1);
    let before = cell.current();
    cell.replace(2);
    assert_eq!((*before, *cell.current()), (1, 2));
    assert!(!Arc::ptr_eq(&before, &cell.current()));
    assert_eq!(*SwapCell::<i32>::default().current(), 0);
}

#[test]
fn a_late_bound_detector_is_absent_until_bound_and_bound_once() {
    let slot: LateBound<dyn Fn() -> i32 + Send + Sync> = LateBound::default();
    assert!(slot.bound().is_none());
    slot.bind(Arc::new(|| 1));
    slot.bind(Arc::new(|| 2));
    assert_eq!(
        slot.bound().map(|f| f()),
        Some(1),
        "a second bind is ignored"
    );
}

/// One capture through `seam` on a mock clock the seam may move, with the
/// clock's readings before the capture.
fn capture_through(
    seam: impl Fn(&MockClock) -> Result<GameFrame, FrameUnavailable> + Send + Sync + 'static,
) -> (Capture, i64, f64) {
    let clock = Arc::new(MockClock::new(None, 50.0));
    let frame: GameWindowFrame = {
        let clock = clock.clone();
        Arc::new(move |_| seam(&clock))
    };
    let (at_ms, began) = (clock_epoch_ms(clock.as_ref()), clock.monotonic());
    let running = || true;
    let eyes = Eyes {
        frame: &frame,
        clock: clock.as_ref(),
        session_id: "s1",
        hinted_at: None,
        running: &running,
        notes: std::cell::Cell::default(),
    };
    (eyes.capture(FrameArea::Whole).unwrap(), at_ms, began)
}

fn one_pixel() -> GameFrame {
    let image = crate::game_ui::geometry::BgrImage {
        data: vec![0; 3],
        h: 1,
        w: 1,
    };
    GameFrame::whole(image, (0, 0))
}

#[test]
fn a_capture_is_stamped_when_its_pixels_were_taken_not_when_it_was_asked_for() {
    // Two seconds pass finding the game window, then the pixels are
    // grabbed in 100 ms: the look is of the screen 100 ms before it was
    // handed over, and only those 100 ms count as the capture's age.
    let (capture, asked_ms, asked) = capture_through(|clock| {
        clock.advance(2.1).unwrap();
        Ok(one_pixel().aged(Duration::from_millis(100)))
    });
    assert_eq!(capture.at_ms, asked_ms + 2_000);
    assert!((capture.began - (asked + 2.0)).abs() < 1e-9);
    assert_eq!(capture.age, Duration::from_millis(100));
}

#[test]
fn a_capture_that_got_no_frame_is_stamped_when_it_was_told_so() {
    let (capture, asked_ms, asked) = capture_through(|clock| {
        clock.advance(0.5).unwrap();
        Err(FrameUnavailable::CaptureFailed)
    });
    assert_eq!(capture.at_ms, asked_ms + 500);
    assert!((capture.began - (asked + 0.5)).abs() < 1e-9);
    assert_eq!(capture.age, Duration::ZERO);
}
