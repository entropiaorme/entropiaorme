//! The user-driven skill scan, ported from
//! `backend/services/skill_scan_manual.py`.
//!
//! The user docks the in-game skills panel, opens the scan overlay,
//! and captures once per page after flipping pages in-game. After the
//! final page, `process` runs the captures through the extraction
//! seam on a background thread and holds the result for the in-app
//! diff review, which then accepts (persists via the completion
//! callback) or rejects (discards).
//!
//! The capture and extraction primitives live behind injected
//! providers (the original composes them from the screen capturer and
//! the local OCR engine, which port with the OCR service); region
//! resolution is likewise a provider over the window lookup and the
//! committed geometry. A `scan.status.changed` envelope publishes on
//! the bus whenever the settled status moves, coalesced on the owned
//! state's projection so each discrete change emits exactly one
//! frame; the typed publish happens after the guard drops. Page
//! results merge in page order, later pages overwriting duplicate
//! names exactly as the original's dict update does. The original's
//! logging is omitted.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::clock::Clock;
use crate::event_bus::{EventBus, Topic};
use crate::tracker::{naive_to_epoch, to_iso_utc};
use eo_wire::domain_events::{
    DomainEvent, ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
};

/// The default page count; the user picks a different one per scan
/// in the overlay.
pub const PAGE_COUNT: i64 = 12;

/// The page-count ceiling a scan may request.
pub const MAX_PAGE_COUNT: i64 = 30;

/// One scan region: the panel's top-left and bottom-right corners.
pub type ScanRegion = ([i32; 2], [i32; 2]);

/// The capture observer (the recording controller's seam): called as
/// `tap(panel, region, png)` after each successful page grab.
pub type CaptureTap = Arc<dyn Fn(&str, &Value, &[u8]) + Send + Sync>;

/// The completion callback: persists an accepted result. An error
/// string surfaces on the status, exactly as the original's caught
/// exception does.
pub type CompletionCallback = Arc<dyn Fn(&[(String, f64)]) -> Result<(), String> + Send + Sync>;

/// The capture and extraction seams the composition root wires in.
pub struct ScanProviders {
    /// Whether the extraction engine can be loaded right now.
    pub engine_available: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The skill-panel region from the live game window, when found.
    pub skill_region: Arc<dyn Fn() -> Option<ScanRegion> + Send + Sync>,
    /// Capture the region as PNG bytes; None on any capture failure.
    pub capture_region: Arc<dyn Fn(ScanRegion) -> Option<Vec<u8>> + Send + Sync>,
    /// Extract `{canonical_name: level}` rows from one page.
    pub extract_page_levels: Arc<dyn Fn(&[u8]) -> Vec<(String, f64)> + Send + Sync>,
}

impl Default for ScanProviders {
    fn default() -> Self {
        Self {
            engine_available: Arc::new(|| false),
            skill_region: Arc::new(|| None),
            capture_region: Arc::new(|_| None),
            extract_page_levels: Arc::new(|_| Vec::new()),
        }
    }
}

/// The owned-state projection the coalescer compares: an emit fires
/// only when this moves, so each discrete status change publishes
/// exactly one frame. The environmental fields (engine availability,
/// window presence) stay out: no verb mutates them.
#[derive(Clone, PartialEq)]
struct StatusKey {
    phase: ScanPhase,
    captured: usize,
    expected: i64,
    progress: (i64, i64),
    has_pending: bool,
    error: Option<String>,
    last_scan_time: Option<f64>,
    last_skills_count: i64,
}

struct ScanState {
    active: bool,
    region: Option<ScanRegion>,
    captures: Vec<Option<Vec<u8>>>,
    processing: bool,
    expected_pages: i64,
    pending_result: Option<Vec<(String, f64)>>,
    processing_progress: (i64, i64),
    error: Option<String>,
    last_scan_time: Option<f64>,
    last_skills_count: i64,
    last_emitted_key: StatusKey,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ScanState {
    fn captured_pages(&self) -> usize {
        self.captures.iter().filter(|c| c.is_some()).count()
    }

    fn derive_phase(&self) -> ScanPhase {
        if self.pending_result.is_some() {
            return ScanPhase::AwaitingReview;
        }
        if self.processing {
            return ScanPhase::Processing;
        }
        if self.active {
            return ScanPhase::Capturing;
        }
        ScanPhase::Idle
    }

    fn status_key(&self) -> StatusKey {
        StatusKey {
            phase: self.derive_phase(),
            captured: self.captured_pages(),
            expected: self.expected_pages,
            progress: self.processing_progress,
            has_pending: self.pending_result.is_some(),
            error: self.error.clone(),
            last_scan_time: self.last_scan_time,
            last_skills_count: self.last_skills_count,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.region = None;
        self.captures = Vec::new();
        self.pending_result = None;
        self.processing_progress = (0, 0);
    }
}

pub struct SkillScanManual {
    providers: ScanProviders,
    clock: Arc<dyn Clock>,
    bus: Option<Arc<EventBus>>,
    state: Mutex<ScanState>,
    on_complete: Mutex<Option<CompletionCallback>>,
    capture_tap: Mutex<Option<CaptureTap>>,
}

impl SkillScanManual {
    pub fn new(
        providers: ScanProviders,
        clock: Arc<dyn Clock>,
        bus: Option<Arc<EventBus>>,
        initial_scan_time: Option<f64>,
        initial_skills_count: i64,
    ) -> Arc<Self> {
        let resting = ScanState {
            active: false,
            region: None,
            captures: Vec::new(),
            processing: false,
            expected_pages: PAGE_COUNT,
            pending_result: None,
            processing_progress: (0, 0),
            error: None,
            last_scan_time: initial_scan_time,
            last_skills_count: initial_skills_count,
            // Baselined below at the construction-time status so the
            // first genuine change emits but a no-op publish on the
            // resting state does not (listeners hydrate idle via the
            // GET on mount).
            last_emitted_key: StatusKey {
                phase: ScanPhase::Idle,
                captured: 0,
                expected: PAGE_COUNT,
                progress: (0, 0),
                has_pending: false,
                error: None,
                last_scan_time: initial_scan_time,
                last_skills_count: initial_skills_count,
            },
            worker: None,
        };
        Arc::new(Self {
            providers,
            clock,
            bus,
            state: Mutex::new(resting),
            on_complete: Mutex::new(None),
            capture_tap: Mutex::new(None),
        })
    }

    /// Install a capture observer (called after each successful page
    /// grab).
    pub fn set_capture_tap(&self, tap: CaptureTap) {
        *self.capture_tap.lock().expect("capture tap") = Some(tap);
    }

    /// Remove the capture observer.
    pub fn clear_capture_tap(&self) {
        *self.capture_tap.lock().expect("capture tap") = None;
    }

    pub fn set_completion_callback(&self, callback: CompletionCallback) {
        *self.on_complete.lock().expect("completion callback") = Some(callback);
    }

    pub fn shutdown(&self) {
        self.lock_state().reset();
    }

    /// The state guard, tolerating poison like the other services.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, ScanState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get_status(&self) -> Value {
        let state = self.lock_state();
        self.status_locked(&state)
    }

    fn status_locked(&self, state: &ScanState) -> Value {
        let (done, total) = state.processing_progress;
        json!({
            "active": state.active,
            "processing": state.processing,
            "captured_pages": state.captured_pages(),
            "expected_pages": state.expected_pages,
            "last_scan_time": state.last_scan_time,
            "skills_count": state.last_skills_count,
            "configured": (self.providers.engine_available)(),
            "game_window_present": (self.providers.skill_region)().is_some(),
            "phase": phase_wire(state.derive_phase()),
            "processing_progress": {"done": done, "total": total},
            "has_pending_result": state.pending_result.is_some(),
            "error": state.error,
        })
    }

    /// Publish a `scan.status.changed` envelope iff the status moved.
    /// Call after releasing the guard at every settled mutation point;
    /// the key compare-and-advance happens under the lock so two
    /// threads cannot both emit the same transition.
    fn publish_status(&self) {
        let Some(bus) = &self.bus else {
            return;
        };
        let phase = {
            let mut state = self.lock_state();
            let key = state.status_key();
            if key == state.last_emitted_key {
                return;
            }
            let phase = key.phase;
            state.last_emitted_key = key;
            phase
        };
        let event = DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: to_iso_utc(naive_to_epoch(self.clock.now())),
            payload: ScanStatusChangedPayload { phase },
        });
        let value = serde_json::to_value(&event).expect("domain events always serialise");
        bus.publish(Topic::ScanStatusChanged, &value);
    }

    pub fn start(&self, page_count: Option<i64>) -> Value {
        if !(self.providers.engine_available)() {
            return json!({"error": "Local OCR engine is unavailable: check the backend log"});
        }
        let Some(region) = (self.providers.skill_region)() else {
            return json!({"error": "Entropia Universe window not found: start the game first"});
        };
        if let Some(count) = page_count {
            if !(1..=MAX_PAGE_COUNT).contains(&count) {
                return json!({
                    "error": format!("page_count must be between 1 and {MAX_PAGE_COUNT}")
                });
            }
        }
        let status = {
            let mut state = self.lock_state();
            if state.processing {
                return json!({"error": "Scan currently processing: wait for it to finish"});
            }
            if state.pending_result.is_some() {
                return json!({
                    "error": "Pending scan result awaiting review: accept or reject first"
                });
            }
            if let Some(count) = page_count {
                state.expected_pages = count;
            }
            state.active = true;
            state.region = Some(region);
            state.captures = Vec::new();
            state.error = None;
            state.processing_progress = (0, 0);
            self.status_locked(&state)
        };
        self.publish_status();
        status
    }

    pub fn capture_current_page(&self) -> Value {
        let region = {
            let state = self.lock_state();
            if !state.active {
                return json!({"error": "No active scan: call start first"});
            }
            let Some(region) = state.region else {
                return json!({"error": "Region not configured"});
            };
            region
        };
        let png = (self.providers.capture_region)(region);
        let (page_num, captured) = {
            let mut state = self.lock_state();
            let captured = png.is_some();
            state.captures.push(png.clone());
            (state.captures.len(), captured)
        };
        if captured {
            let tap = self.capture_tap.lock().expect("capture tap").clone();
            if let Some(tap) = tap {
                let (tl, br) = region;
                let region_value = json!({"tl": tl.to_vec(), "br": br.to_vec()});
                let bytes = png.expect("captured implies bytes");
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tap("skill", &region_value, &bytes)
                }));
            }
        }
        self.publish_status();
        let mut response = json!({"page": page_num, "captured": captured});
        merge_status(&mut response, self.get_status());
        response
    }

    pub fn cancel(&self) -> Value {
        {
            let mut state = self.lock_state();
            if state.processing {
                return json!({"error": "Cannot cancel while processing: wait for completion"});
            }
            state.reset();
        }
        self.publish_status();
        self.get_status()
    }

    /// Pop the most recent capture, returning the user one step back.
    /// Refused while processing or once review is pending; an empty
    /// stack errors rather than no-ops so the frontend can surface
    /// the nothing-to-undo state.
    pub fn undo_last_capture(&self) -> Value {
        let undone = {
            let mut state = self.lock_state();
            if !state.active {
                return json!({"error": "No active scan: call start first"});
            }
            if state.processing {
                return json!({"error": "Cannot undo while processing: wait for completion"});
            }
            if state.pending_result.is_some() {
                return json!({"error": "Pending result awaiting review: accept or reject first"});
            }
            if state.captures.is_empty() {
                return json!({"error": "No captures to undo"});
            }
            let popped = state.captures.len();
            state.captures.pop();
            popped
        };
        self.publish_status();
        let mut response = json!({"undone_page": undone});
        merge_status(&mut response, self.get_status());
        response
    }

    /// PNG bytes for a 1-indexed page, when present.
    pub fn get_capture_png(&self, page: i64) -> Option<Vec<u8>> {
        let state = self.lock_state();
        if page < 1 || page > state.captures.len() as i64 {
            return None;
        }
        state.captures[(page - 1) as usize].clone()
    }

    pub fn get_pending_result(&self) -> Option<Vec<(String, f64)>> {
        self.lock_state().pending_result.clone()
    }

    /// Kick off extraction on a background thread; the result holds
    /// for the diff review.
    pub fn process(self: &Arc<Self>) -> Value {
        let captures = {
            let mut state = self.lock_state();
            if state.processing {
                return json!({"error": "Scan currently processing: wait for it to finish"});
            }
            if state.pending_result.is_some() {
                return json!({"error": "Pending result awaiting review: accept or reject first"});
            }
            if !state.active {
                return json!({"error": "No active scan to process"});
            }
            let captures = state.captures.clone();
            let valid_count = captures.iter().filter(|c| c.is_some()).count() as i64;
            let expected = state.expected_pages;
            if valid_count < expected {
                return json!({
                    "error": format!(
                        "Need {expected} pages captured before processing (have {valid_count})"
                    )
                });
            }
            state.processing = true;
            state.active = false;
            state.error = None;
            state.processing_progress = (0, valid_count);
            captures
        };

        let worker_self = self.clone();
        let handle = std::thread::Builder::new()
            .name("skill-scan-process".into())
            .spawn(move || {
                // The original's worker catches everything: a crash
                // surfaces as the status error and the processing
                // flag always settles.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker_self.extract_levels(&captures)
                }))
                .unwrap_or_else(|_| Err("scan processing crashed".to_string()));
                {
                    let mut state = worker_self.lock_state();
                    match result {
                        Ok(skills) => state.pending_result = Some(skills),
                        Err(message) => state.error = Some(message),
                    }
                    state.processing = false;
                }
                worker_self.publish_status();
            })
            .expect("scan worker spawns");
        self.lock_state().worker = Some(handle);
        self.publish_status();
        self.get_status()
    }

    /// Persist the held scan result via the completion callback.
    pub fn accept(&self) -> Value {
        let skills = {
            let state = self.lock_state();
            let Some(pending) = &state.pending_result else {
                return json!({"error": "No pending result to accept"});
            };
            pending.clone()
        };

        let callback = self
            .on_complete
            .lock()
            .expect("completion callback")
            .clone();
        if let Some(callback) = callback {
            if let Err(message) = callback(&skills) {
                {
                    let mut state = self.lock_state();
                    state.error = Some(message.clone());
                }
                self.publish_status();
                return json!({"error": format!("Persist failed: {message}")});
            }
        }

        let persisted = skills.len();
        {
            let mut state = self.lock_state();
            state.last_scan_time = Some(naive_to_epoch(self.clock.now()));
            state.last_skills_count = persisted as i64;
            state.reset();
        }
        self.publish_status();
        json!({"ok": true, "skills_persisted": persisted})
    }

    /// Discard the held scan result.
    pub fn reject(&self) -> Value {
        {
            let mut state = self.lock_state();
            if state.pending_result.is_none() {
                return json!({"error": "No pending result to reject"});
            }
            state.reset();
        }
        self.publish_status();
        json!({"ok": true})
    }

    /// Block until a running extraction worker settles (test rigs and
    /// orderly shutdown; the original's daemon thread is simply
    /// abandoned).
    pub fn join_worker(&self) {
        let handle = self.lock_state().worker.take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Run extraction per page serially (the engine is
    /// single-threaded), advancing the progress and publishing one
    /// frame per page; pages merge in page order, later pages
    /// overwriting duplicate names.
    fn extract_levels(&self, captures: &[Option<Vec<u8>>]) -> Result<Vec<(String, f64)>, String> {
        let valid: Vec<&Vec<u8>> = captures.iter().flatten().collect();
        if valid.is_empty() {
            return Err("No successful captures to process".to_string());
        }
        {
            let mut state = self.lock_state();
            state.processing_progress = (0, valid.len() as i64);
        }
        let mut all_skills: Vec<(String, f64)> = Vec::new();
        for png in valid {
            let levels = (self.providers.extract_page_levels)(png);
            for (name, level) in levels {
                if let Some(entry) = all_skills.iter_mut().find(|(seen, _)| *seen == name) {
                    entry.1 = level;
                } else {
                    all_skills.push((name, level));
                }
            }
            {
                let mut state = self.lock_state();
                let (done, total) = state.processing_progress;
                state.processing_progress = (done + 1, total);
            }
            // Per-page settled boundary: one frame per page keeps the
            // overlay's progress live without a poll.
            self.publish_status();
        }
        if all_skills.is_empty() {
            return Err("No skills extracted from any page".to_string());
        }
        Ok(all_skills)
    }
}

fn phase_wire(phase: ScanPhase) -> &'static str {
    match phase {
        ScanPhase::Idle => "idle",
        ScanPhase::Capturing => "capturing",
        ScanPhase::Processing => "processing",
        ScanPhase::AwaitingReview => "awaiting_review",
    }
}

fn merge_status(response: &mut Value, status: Value) {
    if let (Value::Object(target), Value::Object(source)) = (response, status) {
        for (key, value) in source {
            target.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    fn providers(pages: Vec<Vec<(String, f64)>>) -> ScanProviders {
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

    fn rig(providers: ScanProviders) -> (Arc<SkillScanManual>, Arc<EventBus>, Arc<MockClock>) {
        let bus = Arc::new(EventBus::new());
        let clock = Arc::new(MockClock::new(None, 0.0));
        let scan = SkillScanManual::new(providers, clock.clone(), Some(bus.clone()), None, 0);
        (scan, bus, clock)
    }

    fn capture_phases(bus: &EventBus) -> Arc<StdMutex<Vec<(String, Value)>>> {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let sink = seen.clone();
        bus.add_tap(move |topic, data| {
            if topic == Topic::ScanStatusChanged {
                sink.lock().unwrap().push((
                    data["payload"]["phase"].as_str().unwrap_or("?").to_string(),
                    data.clone(),
                ));
            }
        });
        seen
    }

    #[test]
    fn the_happy_path_scans_reviews_and_persists() {
        let pages = vec![
            vec![("Anatomy".to_string(), 40.0), ("Rifle".to_string(), 100.0)],
            vec![("Rifle".to_string(), 100.5), ("Sweat".to_string(), 10.0)],
        ];
        let (scan, bus, _clock) = rig(providers(pages));
        let phases = capture_phases(&bus);
        let persisted: Arc<StdMutex<Vec<(String, f64)>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = persisted.clone();
        scan.set_completion_callback(Arc::new(move |levels| {
            sink.lock().unwrap().extend_from_slice(levels);
            Ok(())
        }));

        let status = scan.start(Some(2));
        assert_eq!(status["phase"], "capturing");
        assert_eq!(status["expected_pages"], 2);
        let first = scan.capture_current_page();
        assert_eq!(first["page"], 1);
        assert_eq!(first["captured"], true);
        let second = scan.capture_current_page();
        assert_eq!(second["captured_pages"], 2);

        let status = scan.process();
        assert_eq!(status["phase"], "processing");
        scan.join_worker();
        // Page order merges with later pages overwriting duplicates,
        // first-seen positions kept.
        assert_eq!(
            scan.get_pending_result(),
            Some(vec![
                ("Anatomy".to_string(), 40.0),
                ("Rifle".to_string(), 100.5),
                ("Sweat".to_string(), 10.0),
            ])
        );
        assert_eq!(scan.get_status()["phase"], "awaiting_review");
        assert_eq!(
            scan.get_status()["processing_progress"],
            json!({"done": 2, "total": 2}),
            "the per-page progress advanced to completion"
        );

        let accepted = scan.accept();
        assert_eq!(accepted, json!({"ok": true, "skills_persisted": 3}));
        assert_eq!(persisted.lock().unwrap().len(), 3);
        let status = scan.get_status();
        assert_eq!(status["phase"], "idle");
        assert_eq!(status["skills_count"], 3);
        assert!(status["last_scan_time"].is_f64());

        // The coalesced frame sequence: one per settled transition.
        let observed: Vec<String> = phases
            .lock()
            .unwrap()
            .iter()
            .map(|(phase, _)| phase.clone())
            .collect();
        assert_eq!(
            observed,
            vec![
                "capturing",       // start
                "capturing",       // page 1 (captured count moved)
                "capturing",       // page 2
                "processing",      // process kickoff
                "processing",      // page 1 extracted
                "processing",      // page 2 extracted
                "awaiting_review", // worker settled
                "idle",            // accept
            ]
        );
        // The envelope is the typed wire shape.
        let (_, first_frame) = &phases.lock().unwrap()[0];
        assert_eq!(first_frame["type"], "scan.status.changed");
        assert_eq!(first_frame["event_version"], 1);
        assert!(first_frame["occurred_at"]
            .as_str()
            .unwrap()
            .ends_with("+00:00"));
    }

    #[test]
    fn start_refuses_each_precondition() {
        let mut unavailable = providers(Vec::new());
        unavailable.engine_available = Arc::new(|| false);
        let (scan, _bus, _clock) = rig(unavailable);
        assert_eq!(
            scan.start(None)["error"],
            "Local OCR engine is unavailable: check the backend log"
        );

        let mut windowless = providers(Vec::new());
        windowless.skill_region = Arc::new(|| None);
        let (scan, _bus, _clock) = rig(windowless);
        assert_eq!(
            scan.start(None)["error"],
            "Entropia Universe window not found: start the game first"
        );

        let (scan, _bus, _clock) = rig(providers(Vec::new()));
        assert_eq!(
            scan.start(Some(0))["error"],
            "page_count must be between 1 and 30"
        );
        assert_eq!(
            scan.start(Some(31))["error"],
            "page_count must be between 1 and 30"
        );
    }

    #[test]
    fn the_verbs_guard_their_states() {
        let (scan, _bus, _clock) = rig(providers(vec![vec![("Rifle".to_string(), 1.0)]]));
        assert_eq!(
            scan.capture_current_page()["error"],
            "No active scan: call start first"
        );
        assert_eq!(
            scan.undo_last_capture()["error"],
            "No active scan: call start first"
        );
        assert_eq!(scan.accept()["error"], "No pending result to accept");
        assert_eq!(scan.reject()["error"], "No pending result to reject");
        assert_eq!(scan.process()["error"], "No active scan to process");

        scan.start(Some(2));
        assert_eq!(scan.undo_last_capture()["error"], "No captures to undo");
        scan.capture_current_page();
        assert_eq!(
            scan.process()["error"],
            "Need 2 pages captured before processing (have 1)"
        );
        let undone = scan.undo_last_capture();
        assert_eq!(undone["undone_page"], 1);
        assert_eq!(undone["captured_pages"], 0);

        // A pending review blocks restarts and undo until dispositioned.
        scan.capture_current_page();
        let mut single = scan.start(Some(1));
        assert_eq!(
            single["error"],
            Value::Null,
            "restart before processing is allowed: {single}"
        );
        single = scan.capture_current_page();
        assert_eq!(single["captured"], true);
        scan.process();
        scan.join_worker();
        assert_eq!(
            scan.start(None)["error"],
            "Pending scan result awaiting review: accept or reject first"
        );
        assert_eq!(
            scan.undo_last_capture()["error"],
            "No active scan: call start first"
        );
        scan.reject();
        assert_eq!(scan.get_status()["phase"], "idle");

        // Cancel returns the settled status, and shutdown resets the
        // owned state wholesale.
        scan.start(Some(1));
        scan.capture_current_page();
        let cancelled = scan.cancel();
        assert_eq!(cancelled["phase"], "idle");
        assert_eq!(cancelled["captured_pages"], 0);
        scan.start(Some(1));
        scan.capture_current_page();
        scan.shutdown();
        let status = scan.get_status();
        assert_eq!(status["phase"], "idle");
        assert_eq!(status["captured_pages"], 0);
    }

    #[test]
    fn failed_captures_record_and_extraction_errors_surface() {
        let mut flaky = providers(Vec::new());
        let calls = Arc::new(StdMutex::new(0usize));
        let counter = calls.clone();
        flaky.capture_region = Arc::new(move |_| {
            let mut count = counter.lock().unwrap();
            *count += 1;
            if *count == 1 {
                None
            } else {
                Some(vec![9])
            }
        });
        flaky.extract_page_levels = Arc::new(|_| Vec::new());
        let (scan, _bus, _clock) = rig(flaky);
        scan.start(Some(1));
        let first = scan.capture_current_page();
        assert_eq!(first["captured"], false);
        assert_eq!(first["captured_pages"], 0);
        let second = scan.capture_current_page();
        assert_eq!(second["captured"], true);

        // One valid page, but the extractor finds nothing.
        scan.process();
        scan.join_worker();
        let status = scan.get_status();
        assert_eq!(status["error"], "No skills extracted from any page");
        assert_eq!(status["phase"], "idle", "the failed scan settles idle");
    }

    #[test]
    fn a_failing_completion_keeps_the_review_open() {
        let (scan, _bus, _clock) = rig(providers(vec![vec![("Rifle".to_string(), 1.0)]]));
        scan.set_completion_callback(Arc::new(|_| Err("disk full".to_string())));
        scan.start(Some(1));
        scan.capture_current_page();
        scan.process();
        scan.join_worker();
        let refused = scan.accept();
        assert_eq!(refused["error"], "Persist failed: disk full");
        assert_eq!(
            scan.get_status()["phase"],
            "awaiting_review",
            "the pending result survives a persist failure for a retry"
        );
        assert_eq!(scan.get_status()["error"], "disk full");

        scan.set_completion_callback(Arc::new(|_| Ok(())));
        assert_eq!(scan.accept()["ok"], true);
    }

    #[test]
    fn the_capture_tap_sees_successful_grabs_only() {
        let (scan, _bus, _clock) = rig(providers(vec![vec![("Rifle".to_string(), 1.0)]]));
        let taps: Arc<StdMutex<Vec<(String, Value, usize)>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink = taps.clone();
        scan.set_capture_tap(Arc::new(move |panel, region, png| {
            sink.lock()
                .unwrap()
                .push((panel.to_string(), region.clone(), png.len()));
        }));
        scan.start(Some(1));
        scan.capture_current_page();
        {
            let taps = taps.lock().unwrap();
            assert_eq!(taps.len(), 1);
            assert_eq!(taps[0].0, "skill");
            assert_eq!(taps[0].1, json!({"tl": [0, 0], "br": [100, 200]}));
            assert_eq!(taps[0].2, 3);
        }
        scan.clear_capture_tap();
        scan.undo_last_capture();
        scan.capture_current_page();
        assert_eq!(taps.lock().unwrap().len(), 1, "a cleared tap stays silent");

        assert_eq!(scan.get_capture_png(1), Some(vec![1, 2, 3]));
        assert_eq!(scan.get_capture_png(2), None);
        assert_eq!(scan.get_capture_png(0), None);
    }
}
