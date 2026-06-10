//! Injectable clock, ported from `backend/testing/clock.py`.
//!
//! Services that read wall-clock or monotonic time go through a
//! `Clock` so replay scenarios can freeze and advance time
//! deterministically. Wall-clock instants are naive (matching the
//! original's default `datetime.now()` callers); the monotonic stream
//! is seconds since an arbitrary epoch where only deltas mean
//! anything. The mock's two streams advance in lockstep, and the
//! wall-clock stream can re-freeze independently so monotonic counts
//! survive a scenario walking several frozen instants.

use std::sync::Mutex;
use std::time::Instant;

use chrono::{Duration, Local, NaiveDateTime};

pub trait Clock: Send + Sync {
    /// The current wall-clock instant, naive.
    fn now(&self) -> NaiveDateTime;
    /// Monotonic seconds since an arbitrary epoch (deltas only).
    fn monotonic(&self) -> f64;
}

/// The production clock: the system's local wall clock and a process
/// monotonic stream.
pub struct RealClock {
    epoch: Instant,
}

impl Default for RealClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl RealClock {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clock for RealClock {
    fn now(&self) -> NaiveDateTime {
        Local::now().naive_local()
    }

    fn monotonic(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

struct MockState {
    now: NaiveDateTime,
    monotonic: f64,
}

/// The test clock: frozen by default, advanced explicitly.
pub struct MockClock {
    state: Mutex<MockState>,
}

impl MockClock {
    /// Frozen at `start` (the original defaults to 2026-01-01) with
    /// the monotonic stream at `monotonic_start`.
    pub fn new(start: Option<NaiveDateTime>, monotonic_start: f64) -> Self {
        let default_start =
            NaiveDateTime::parse_from_str("2026-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
                .expect("the default start parses");
        Self {
            state: Mutex::new(MockState {
                now: start.unwrap_or(default_start),
                monotonic: monotonic_start,
            }),
        }
    }

    /// Advance both streams together; negative deltas are refused to
    /// preserve the monotonic invariant.
    pub fn advance(&self, seconds: f64) -> Result<(), String> {
        if seconds < 0.0 {
            return Err(format!(
                "advance rejects negative deltas (got {seconds}); freeze the \
                 wall clock instead of moving the monotonic stream backwards"
            ));
        }
        let mut state = self.state.lock().expect("mock clock state");
        state.now += Duration::microseconds((seconds * 1_000_000.0).round() as i64);
        state.monotonic += seconds;
        Ok(())
    }

    /// Reset the wall-clock stream only; the monotonic stream is
    /// preserved.
    pub fn freeze_at(&self, instant: NaiveDateTime) {
        self.state.lock().expect("mock clock state").now = instant;
    }
}

impl Clock for MockClock {
    fn now(&self) -> NaiveDateTime {
        self.state.lock().expect("mock clock state").now
    }

    fn monotonic(&self) -> f64 {
        self.state.lock().expect("mock clock state").monotonic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_freezes_advances_and_refreezes() {
        let clock = MockClock::new(None, 10.0);
        assert_eq!(
            clock.now().format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-01-01 00:00:00"
        );
        assert_eq!(clock.monotonic(), 10.0);

        clock.advance(2.5).unwrap();
        assert_eq!(clock.monotonic(), 12.5);
        let expected =
            NaiveDateTime::parse_from_str("2026-01-01 00:00:02.500", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        assert_eq!(clock.now(), expected);

        assert!(clock.advance(-1.0).is_err());

        let instant =
            NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        clock.freeze_at(instant);
        assert_eq!(clock.now(), instant);
        assert_eq!(clock.monotonic(), 12.5, "the monotonic stream survives");
    }

    #[test]
    fn the_real_clock_streams_are_sane() {
        let clock = RealClock::new();
        let first = clock.monotonic();
        let second = clock.monotonic();
        assert!(second >= first);
        assert!(clock.now().and_utc().timestamp() > 1_700_000_000);
    }
}
