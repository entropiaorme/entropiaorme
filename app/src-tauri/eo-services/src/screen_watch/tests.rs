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
