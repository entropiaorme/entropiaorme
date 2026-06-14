//! Spacebar-capture listener, ported from
//! `backend/services/spacebar_capture_listener.py`: an optional hook for
//! hands-free capture during a manual skill scan.
//!
//! When enabled (the scan-overlay toggle), the listener consumes a
//! [`KeystrokeSource`] (production: the shared low-level keyboard hook
//! filtered to the space key at its boundary; tests: the mock) and, on a
//! press edge (auto-repeat suppressed via release tracking), dispatches
//! `capture_current_page` on the skill scan when it is in the `capturing`
//! phase. Idle: a no-op. Listening is pass-through (the press is not
//! consumed), so the game client still receives the keystroke. The capture
//! runs on a short-lived thread to keep the dispatch callback cheap, exactly
//! as the original offloads it. The original's logging is omitted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::keystroke_source::{KeystrokeEvent, KeystrokeKind, KeystrokeSource};
use crate::skill_scan_manual::SkillScanManual;

/// A keystroke observer (the recording controller's seam): called with
/// `(key, kind)` for each space press/release edge.
pub type KeyTap = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct Flags {
    enabled: AtomicBool,
    source_running: AtomicBool,
    space_down: AtomicBool,
}

pub struct SpacebarCaptureListener {
    skill_scan: Arc<SkillScanManual>,
    source: Option<Arc<dyn KeystrokeSource>>,
    flags: Flags,
    key_tap: Mutex<Option<KeyTap>>,
}

impl SpacebarCaptureListener {
    /// A `None` source leaves the listener inert, matching the original's
    /// missing-hook-library path. The listener subscribes to the source on
    /// construction; the source's strong handle through the subscriber
    /// closure keeps the listener alive, so only an explicit [`stop`] (or the
    /// source's own teardown) releases it.
    ///
    /// [`stop`]: SpacebarCaptureListener::stop
    pub fn new(
        skill_scan: Arc<SkillScanManual>,
        source: Option<Arc<dyn KeystrokeSource>>,
    ) -> Arc<Self> {
        let listener = Arc::new(Self {
            skill_scan,
            source: source.clone(),
            flags: Flags {
                enabled: AtomicBool::new(false),
                source_running: AtomicBool::new(false),
                space_down: AtomicBool::new(false),
            },
            key_tap: Mutex::new(None),
        });
        if let Some(source) = source {
            let dispatch = listener.clone();
            source.subscribe(Arc::new(move |event: &KeystrokeEvent| {
                dispatch.on_keystroke(event);
            }));
        }
        listener
    }

    /// True when the keystroke source is currently delivering events.
    pub fn is_running(&self) -> bool {
        self.flags.source_running.load(Ordering::SeqCst)
    }

    /// Whether the overlay toggle is on.
    pub fn is_enabled(&self) -> bool {
        self.flags.enabled.load(Ordering::SeqCst)
    }

    /// Install a keystroke observer (called for each space press/release edge).
    pub fn set_key_tap(&self, tap: KeyTap) {
        *self.key_tap.lock().expect("key tap") = Some(tap);
    }

    /// Remove the keystroke observer.
    pub fn clear_key_tap(&self) {
        *self.key_tap.lock().expect("key tap") = None;
    }

    /// Toggle the listener; idempotent. Enabling starts the source, disabling
    /// stops it (the source still only delivers while a listener wants it).
    pub fn set_enabled(&self, enabled: bool) {
        if self.flags.enabled.swap(enabled, Ordering::SeqCst) == enabled {
            return;
        }
        if enabled {
            self.start_source();
        } else {
            self.stop_source();
        }
    }

    /// Tear down at shutdown.
    pub fn stop(&self) {
        self.flags.enabled.store(false, Ordering::SeqCst);
        self.stop_source();
    }

    fn start_source(&self) {
        let Some(source) = &self.source else {
            return;
        };
        if self.flags.source_running.load(Ordering::SeqCst) {
            return;
        }
        // The source reports whether the underlying mechanism actually
        // attached; running honestly reflects whether events will come.
        let attached = source.start();
        self.flags.source_running.store(attached, Ordering::SeqCst);
    }

    fn stop_source(&self) {
        let Some(source) = &self.source else {
            return;
        };
        if !self.flags.source_running.load(Ordering::SeqCst) {
            return;
        }
        source.stop();
        self.flags.source_running.store(false, Ordering::SeqCst);
        self.flags.space_down.store(false, Ordering::SeqCst);
    }

    fn is_capturing(&self) -> bool {
        self.skill_scan.get_status()["phase"] == "capturing"
    }

    fn on_keystroke(&self, event: &KeystrokeEvent) {
        if !self.flags.source_running.load(Ordering::SeqCst) {
            return;
        }
        if event.key != "space" {
            return;
        }
        match event.kind {
            KeystrokeKind::Press => self.on_space_press(),
            KeystrokeKind::Release => self.on_space_release(),
        }
    }

    fn on_space_press(&self) {
        // Auto-repeat suppression: only the first press edge fires.
        if self.flags.space_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let tap = self.key_tap.lock().expect("key tap").clone();
        if let Some(tap) = tap {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap("space", "press")));
        }
        if !self.is_capturing() {
            return;
        }
        // Off-thread to keep the dispatch callback cheap, exactly as the
        // original spawns a daemon thread for the capture.
        let scan = self.skill_scan.clone();
        let _ = std::thread::Builder::new()
            .name("spacebar-capture".into())
            .spawn(move || {
                let _ = scan.capture_current_page();
            });
    }

    fn on_space_release(&self) {
        self.flags.space_down.store(false, Ordering::SeqCst);
        let tap = self.key_tap.lock().expect("key tap").clone();
        if let Some(tap) = tap {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap("space", "release")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::keystroke_source::MockKeystrokeSource;
    use crate::skill_scan_manual::{ScanProviders, SkillScanManual};
    use chrono::{DateTime, Utc};
    use std::time::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn scan() -> Arc<SkillScanManual> {
        SkillScanManual::new(
            ScanProviders {
                engine_available: Arc::new(|| true),
                skill_region: Arc::new(|| Some(([0, 0], [100, 200]))),
                capture_region: Arc::new(|_| Some(vec![1, 2, 3])),
                extract_page_levels: Arc::new(|_| Vec::new()),
            },
            Arc::new(MockClock::new(None, 0.0)),
            None,
            None,
            0,
        )
    }

    fn captured(scan: &SkillScanManual) -> i64 {
        scan.get_status()["captured_pages"].as_i64().unwrap_or(-1)
    }

    /// Wait (bounded) for the off-thread capture to land the expected count.
    fn wait_for_captures(scan: &SkillScanManual, want: i64) {
        for _ in 0..100 {
            if captured(scan) == want {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(captured(scan), want, "capture count never settled");
    }

    #[test]
    fn the_toggle_starts_and_stops_the_source() {
        let source = Arc::new(MockKeystrokeSource::new());
        let listener = SpacebarCaptureListener::new(scan(), Some(source.clone()));
        assert!(!listener.is_running());
        assert!(!listener.is_enabled());

        listener.set_enabled(true);
        assert!(listener.is_running());
        assert!(listener.is_enabled());

        // Idempotent: a second enable is a no-op.
        listener.set_enabled(true);
        assert!(listener.is_running());

        listener.set_enabled(false);
        assert!(!listener.is_running());
        assert!(!listener.is_enabled());
    }

    #[test]
    fn space_fires_capture_only_while_capturing_with_auto_repeat_suppressed() {
        let source = Arc::new(MockKeystrokeSource::new());
        let scan = scan();
        let listener = SpacebarCaptureListener::new(scan.clone(), Some(source.clone()));
        listener.set_enabled(true);

        // Idle scan: a space press is a no-op (no active capture target).
        source.inject("space", now(), KeystrokeKind::Press);
        source.inject("space", now(), KeystrokeKind::Release);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(captured(&scan), 0, "no capture while idle");

        // Start the scan (capturing); a press fires one capture.
        scan.start(Some(3));
        source.inject("space", now(), KeystrokeKind::Press);
        wait_for_captures(&scan, 1);

        // A second press WITHOUT a release is auto-repeat: suppressed.
        source.inject("space", now(), KeystrokeKind::Press);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(captured(&scan), 1, "auto-repeat is suppressed");

        // Release then press fires again.
        source.inject("space", now(), KeystrokeKind::Release);
        source.inject("space", now(), KeystrokeKind::Press);
        wait_for_captures(&scan, 2);
    }

    #[test]
    fn a_stopped_listener_ignores_keys_and_non_space_is_ignored() {
        let source = Arc::new(MockKeystrokeSource::new());
        let scan = scan();
        let listener = SpacebarCaptureListener::new(scan.clone(), Some(source.clone()));
        listener.set_enabled(true);
        scan.start(Some(3));

        // A non-space key never fires a capture.
        source.inject("1", now(), KeystrokeKind::Press);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(captured(&scan), 0, "non-space keys are ignored");

        // After stop the source no longer delivers, so a space press is inert.
        listener.stop();
        assert!(!listener.is_running());
        source.inject("space", now(), KeystrokeKind::Press);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(captured(&scan), 0, "a stopped listener ignores space");
    }

    #[test]
    fn the_key_tap_observes_both_edges() {
        let source = Arc::new(MockKeystrokeSource::new());
        let listener = SpacebarCaptureListener::new(scan(), Some(source.clone()));
        let taps: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = taps.clone();
        listener.set_key_tap(Arc::new(move |key: &str, kind: &str| {
            sink.lock()
                .unwrap()
                .push((key.to_string(), kind.to_string()));
        }));
        listener.set_enabled(true);

        source.inject("space", now(), KeystrokeKind::Press);
        source.inject("space", now(), KeystrokeKind::Release);
        std::thread::sleep(Duration::from_millis(20));
        let observed = taps.lock().unwrap().clone();
        assert_eq!(
            observed,
            vec![
                ("space".to_string(), "press".to_string()),
                ("space".to_string(), "release".to_string()),
            ]
        );

        listener.clear_key_tap();
        source.inject("space", now(), KeystrokeKind::Press);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(taps.lock().unwrap().len(), 2, "a cleared tap stays silent");
    }
}
