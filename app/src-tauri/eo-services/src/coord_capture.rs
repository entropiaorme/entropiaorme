//! Coordinate capture: reading the avatar's on-screen position (the
//! radar's longitude and latitude readout) for one-click map pins, plus
//! the guided two-point calibration that defines where that readout
//! sits on screen.
//!
//! The radar is freely movable and resizable, so the capture rectangle
//! is per-user state, not a constant. Calibration is a two-step flow:
//! the user hovers the readout's top-left corner and presses Enter,
//! then its bottom-right corner and presses Enter; the cursor position
//! at each press defines a corner. The completed rectangle persists
//! through an injected sink, and a validation scan runs immediately so
//! the user sees what the calibrated region reads.
//!
//! Two seams are deliberately narrow, in this service and its callers:
//!
//! - **The region provider** (`region`): how the capture rectangle is
//!   obtained is opaque to everything downstream of it. Today it reads
//!   the persisted manual calibration; an automatic UI-element locator
//!   can replace the provider wholesale, demoting manual calibration to
//!   an escape-hatch override, with no change to the scan path.
//! - **The frame reader** (`read_text`): the digit read is one closure
//!   over the shared OCR engine. A specialised recogniser replaces that
//!   single closure; capture, parsing, and the plausibility gate stay.
//!
//! Every scan validates before it answers: each half of the strip must
//! yield a digit run, and when the caller supplies the selected
//! planet's calibrated bounds the coordinates must fall inside them. An
//! implausible read is a typed refusal, never a silently-wrong pin.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::skill_panel::{digit_value, BgrImage};

/// The persisted capture rectangle, in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordRegion {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// The cursor-position seam (calibration corners come from it).
pub type CursorPosition = Arc<dyn Fn() -> Option<(i64, i64)> + Send + Sync>;
/// The region-provider seam: how the capture rectangle is obtained.
pub type RegionProvider = Arc<dyn Fn() -> Option<CoordRegion> + Send + Sync>;
/// The screen-capture seam: an `x/y/w/h` rectangle as BGR pixels.
pub type RegionCapture = Arc<dyn Fn(i64, i64, i64, i64) -> Option<BgrImage> + Send + Sync>;
/// The recognition seam: one frame to `(text, confidence)`.
pub type FrameReader = Arc<dyn Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync>;
/// The persistence sink a completed calibration writes through.
pub type RegionSink = Arc<dyn Fn(CoordRegion) -> Result<(), String> + Send + Sync>;
/// Where scan debug artefacts (the captured frame and what the
/// recogniser answered) should be written, or None to write nothing.
pub type DebugDir = Arc<dyn Fn() -> Option<std::path::PathBuf> + Send + Sync>;

/// The provider seams the composition root wires in.
pub struct CoordCaptureProviders {
    pub cursor_position: CursorPosition,
    pub region: RegionProvider,
    pub capture_region: RegionCapture,
    pub read_text: FrameReader,
    pub persist_region: RegionSink,
    pub debug_dir: DebugDir,
}

impl Default for CoordCaptureProviders {
    fn default() -> Self {
        Self {
            cursor_position: Arc::new(|| None),
            region: Arc::new(|| None),
            capture_region: Arc::new(|_, _, _, _| None),
            read_text: Arc::new(|_| None),
            persist_region: Arc::new(|_| Ok(())),
            debug_dir: Arc::new(|| None),
        }
    }
}

/// Where the calibration flow currently stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationPhase {
    Idle,
    AwaitTopLeft,
    AwaitBottomRight { top_left: (i64, i64) },
}

impl CalibrationPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            CalibrationPhase::Idle => "idle",
            CalibrationPhase::AwaitTopLeft => "awaitTopLeft",
            CalibrationPhase::AwaitBottomRight { .. } => "awaitBottomRight",
        }
    }
}

/// A successful coordinate read. The readout carries longitude and
/// latitude only; the client stopped showing an altitude alongside
/// them, so the reader cannot invent one.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordRead {
    pub lon: i64,
    pub lat: i64,
    pub raw_text: String,
    pub confidence: f64,
}

/// A scan's outcome: exactly one typed answer per failure leg, so the
/// UI can say precisely what went wrong (and a wrong read can never
/// masquerade as a pin).
#[derive(Debug, Clone, PartialEq)]
pub enum CoordScanOutcome {
    Read(CoordRead),
    /// No capture rectangle is available (never calibrated, or the
    /// provider stood down).
    NoRegion,
    /// The screen grab failed (capture unavailable on this host).
    CaptureFailed,
    /// The OCR engine is unavailable.
    EngineUnavailable,
    /// The captured text did not parse as a coordinate readout.
    Unreadable {
        raw_text: String,
        confidence: f64,
    },
    /// The parsed coordinates fall outside the selected planet's map
    /// bounds.
    Implausible {
        lon: i64,
        lat: i64,
        raw_text: String,
    },
}

/// A rectangle a calibration may persist: both dimensions at least this
/// many pixels (a degenerate rectangle reads nothing).
const MIN_REGION_PX: i64 = 4;

/// The largest rectangle a scan will capture per axis. The readout is a
/// small strip; a stored region beyond any plausible screen (a
/// hand-edited config) reads as uncalibrated rather than driving an
/// enormous grab.
const MAX_REGION_PX: i64 = 4096;

/// The coordinate-capture service: the calibration state machine plus
/// the one-shot scan, over the injected seams.
pub struct CoordCaptureService {
    providers: CoordCaptureProviders,
    phase: Mutex<CalibrationPhase>,
    /// The validation read taken when a calibration completes, so the
    /// UI can echo "we read: X, Y; look right?".
    last_validation: Mutex<Option<CoordScanOutcome>>,
    /// The Enter listener's gate, attached post-construction (weak: the
    /// listener holds the service strongly). The service flips it so
    /// the listener is enabled exactly while a flow is live.
    confirm_listener: Mutex<Option<std::sync::Weak<CoordConfirmListener>>>,
}

impl CoordCaptureService {
    pub fn new(providers: CoordCaptureProviders) -> Arc<Self> {
        Arc::new(Self {
            providers,
            phase: Mutex::new(CalibrationPhase::Idle),
            last_validation: Mutex::new(None),
            confirm_listener: Mutex::new(None),
        })
    }

    /// Attach the Enter listener whose gate this service drives.
    pub fn attach_confirm_listener(&self, listener: &Arc<CoordConfirmListener>) {
        *self.confirm_listener.lock().expect("listener slot") = Some(Arc::downgrade(listener));
    }

    fn set_listener_enabled(&self, enabled: bool) {
        let listener = self
            .confirm_listener
            .lock()
            .expect("listener slot")
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        if let Some(listener) = listener {
            listener.set_enabled(enabled);
        }
    }

    /// Begin (or restart) the two-point calibration flow; the Enter
    /// listener arms with it.
    pub fn calibration_start(&self) -> CalibrationPhase {
        let phase = {
            let mut phase = self.phase.lock().expect("calibration phase");
            *phase = CalibrationPhase::AwaitTopLeft;
            *self.last_validation.lock().expect("validation slot") = None;
            *phase
        };
        self.set_listener_enabled(true);
        phase
    }

    /// Abandon an in-flight flow (the persisted region is untouched);
    /// the Enter listener disarms with it.
    pub fn calibration_cancel(&self) -> CalibrationPhase {
        let phase = {
            let mut phase = self.phase.lock().expect("calibration phase");
            *phase = CalibrationPhase::Idle;
            *phase
        };
        self.set_listener_enabled(false);
        phase
    }

    pub fn calibration_phase(&self) -> CalibrationPhase {
        *self.phase.lock().expect("calibration phase")
    }

    /// The persisted capture region, through the provider seam.
    pub fn region(&self) -> Option<CoordRegion> {
        (self.providers.region)()
    }

    /// Current physical cursor position through the same injected seam used
    /// by coordinate-boundary calibration. Other pointer calibrations reuse
    /// this provider rather than reaching around the established flow.
    pub fn cursor_position(&self) -> Option<(i64, i64)> {
        (self.providers.cursor_position)()
    }

    /// The validation read echoed after the last completed calibration.
    pub fn last_validation(&self) -> Option<CoordScanOutcome> {
        self.last_validation
            .lock()
            .expect("validation slot")
            .clone()
    }

    /// Whether an Enter press should currently be observed at all: the
    /// listener's gate, so the flow being idle means Enter presses are
    /// ignored entirely.
    pub fn calibration_active(&self) -> bool {
        !matches!(self.calibration_phase(), CalibrationPhase::Idle)
    }

    /// Advance the flow on an Enter press: capture the cursor position
    /// as the pending corner. On the second corner the rectangle is
    /// normalised (corners may be given in any order), size-checked,
    /// persisted, and a validation scan is taken. Returns the phase
    /// after the press.
    pub fn on_confirm(&self) -> CalibrationPhase {
        let cursor = (self.providers.cursor_position)();
        let mut phase = self.phase.lock().expect("calibration phase");
        let Some((cx, cy)) = cursor else {
            // No cursor position available: the flow cannot proceed on
            // this host; abandon rather than sit unfinishable.
            tracing::warn!(
                target: "eo::coord_capture",
                "cursor position unavailable; calibration abandoned"
            );
            *phase = CalibrationPhase::Idle;
            drop(phase);
            self.set_listener_enabled(false);
            return CalibrationPhase::Idle;
        };
        match *phase {
            CalibrationPhase::Idle => {}
            CalibrationPhase::AwaitTopLeft => {
                *phase = CalibrationPhase::AwaitBottomRight { top_left: (cx, cy) };
            }
            CalibrationPhase::AwaitBottomRight { top_left } => {
                let (tx, ty) = top_left;
                let region = CoordRegion {
                    x: tx.min(cx),
                    y: ty.min(cy),
                    w: (cx - tx).abs(),
                    h: (cy - ty).abs(),
                };
                if region.w < MIN_REGION_PX || region.h < MIN_REGION_PX {
                    // A degenerate rectangle re-arms the second corner
                    // rather than persisting something unreadable.
                    tracing::warn!(
                        target: "eo::coord_capture",
                        "calibration rectangle degenerate ({}x{}); second corner re-armed",
                        region.w, region.h
                    );
                    return *phase;
                }
                if let Err(err) = (self.providers.persist_region)(region) {
                    tracing::error!(
                        target: "eo::coord_capture",
                        "calibration region could not persist ({err}); flow abandoned"
                    );
                    *phase = CalibrationPhase::Idle;
                    drop(phase);
                    self.set_listener_enabled(false);
                    return CalibrationPhase::Idle;
                }
                *phase = CalibrationPhase::Idle;
                drop(phase);
                self.set_listener_enabled(false);
                // The validation echo: read the freshly calibrated
                // region once (no bounds gate; the echo shows the raw
                // read for the user to eyeball).
                let validation = self.scan(None);
                *self.last_validation.lock().expect("validation slot") = Some(validation);
                return CalibrationPhase::Idle;
            }
        }
        *phase
    }

    /// One coordinate scan through the seams: region -> capture ->
    /// read -> parse -> optional bounds gate.
    ///
    /// The game shows the readout as one horizontal strip, `LON <value>`
    /// on its left half and `LAT <value>` on its right, so the captured
    /// rectangle is split into two columns and each half reads
    /// separately, taking that half's trailing digit run (the value
    /// follows its label, so a label misread such as `L0N` cannot
    /// inject a spurious value). The split column is the dead space
    /// between the two halves rather than a fixed midpoint; see
    /// `split_columns`. A whole-rectangle single-line read remains the
    /// fallback for a strip the split could not read.
    pub fn scan(&self, bounds: Option<CoordBounds>) -> CoordScanOutcome {
        let Some(region) = (self.providers.region)() else {
            return CoordScanOutcome::NoRegion;
        };
        if region.w <= 0 || region.h <= 0 || region.w > MAX_REGION_PX || region.h > MAX_REGION_PX {
            return CoordScanOutcome::NoRegion;
        }
        let Some(frame) = (self.providers.capture_region)(region.x, region.y, region.w, region.h)
        else {
            return CoordScanOutcome::CaptureFailed;
        };

        // Every recogniser answer is recorded, so a debug dump (and the
        // scan log line) shows exactly what the model saw and said.
        let mut reads: Vec<(&'static str, String, f64)> = Vec::new();

        let outcome = (|| {
            let mut parsed: Option<(i64, i64, String, f64)> = None;
            if frame.w >= 2 {
                let (left, right) = split_columns(&frame);
                let Some((left_text, left_conf)) = (self.providers.read_text)(&left) else {
                    return CoordScanOutcome::EngineUnavailable;
                };
                let Some((right_text, right_conf)) = (self.providers.read_text)(&right) else {
                    return CoordScanOutcome::EngineUnavailable;
                };
                reads.push(("lon-half", left_text.clone(), left_conf));
                reads.push(("lat-half", right_text.clone(), right_conf));
                if let Some((lon, lat)) = trailing_run(&left_text).zip(trailing_run(&right_text)) {
                    let raw = format!("{left_text} | {right_text}");
                    parsed = Some((lon, lat, raw, left_conf.min(right_conf)));
                }
            }
            if parsed.is_none() {
                let Some((text, confidence)) = (self.providers.read_text)(&frame) else {
                    return CoordScanOutcome::EngineUnavailable;
                };
                reads.push(("whole", text.clone(), confidence));
                match parse_coordinates(&text) {
                    Some((lon, lat)) => {
                        parsed = Some((lon, lat, text, confidence));
                    }
                    None => {
                        return CoordScanOutcome::Unreadable {
                            raw_text: text,
                            confidence,
                        };
                    }
                }
            }
            let (lon, lat, raw_text, confidence) = parsed.expect("parsed set above");
            if let Some(bounds) = bounds {
                if !bounds.contains(lon, lat) {
                    return CoordScanOutcome::Implausible { lon, lat, raw_text };
                }
            }
            CoordScanOutcome::Read(CoordRead {
                lon,
                lat,
                raw_text,
                confidence,
            })
        })();

        tracing::info!(
            target: "eo::coord_capture",
            outcome = ?summarise(&outcome),
            reads = ?reads,
            region_w = region.w,
            region_h = region.h,
            "coordinate scan"
        );
        // Persist the debug crop only for a problematic read (unreadable or
        // implausible): those are the outcomes worth diagnosing. A clean read
        // writes nothing, so the high-frequency navigation auto-poll does not
        // leave a ~1 Hz screenshot of the player's live position in the data
        // directory; the escape hatch stays available exactly when a read fails.
        if !matches!(outcome, CoordScanOutcome::Read(_)) {
            if let Some(dir) = (self.providers.debug_dir)() {
                write_debug_artefacts(&dir, &frame, &reads, &outcome);
            }
        }
        outcome
    }
}

/// The shared Enter listener for pointer-based calibration flows, mirroring the
/// spacebar-capture listener's lifecycle over the SAME shared OS hook:
/// enabled only while a flow is live (the facade's start/cancel verbs
/// flip it, and completing the flow disables it from inside), starting
/// the shared source on enable and stopping it on disable, so outside a
/// calibration episode Enter presses are not observed at all.
/// Listening is pass-through: the game still receives the keystroke.
pub struct CoordConfirmListener {
    active: Arc<dyn Fn() -> bool + Send + Sync>,
    confirm: Arc<dyn Fn() + Send + Sync>,
    source: Option<Arc<dyn crate::keystroke_source::KeystrokeSource>>,
    enabled: std::sync::atomic::AtomicBool,
    source_running: std::sync::atomic::AtomicBool,
    return_down: std::sync::atomic::AtomicBool,
}

impl CoordConfirmListener {
    /// A `None` source leaves the listener inert. Subscription uses a
    /// weak handle so the source's callback cannot keep the listener
    /// alive past its owners (the spacebar listener's pattern).
    pub fn new(
        service: Arc<CoordCaptureService>,
        source: Option<Arc<dyn crate::keystroke_source::KeystrokeSource>>,
    ) -> Arc<Self> {
        let active_service = service.clone();
        Self::new_with_handler(
            Arc::new(move || active_service.calibration_active()),
            Arc::new(move || {
                service.on_confirm();
            }),
            source,
        )
    }

    /// Reuse the established calibration listener lifecycle for another
    /// two-point flow. The callbacks remain ignorant of the OS hook; the
    /// listener owns press-edge filtering, flow gating and source claims.
    pub fn new_with_handler(
        active: Arc<dyn Fn() -> bool + Send + Sync>,
        confirm: Arc<dyn Fn() + Send + Sync>,
        source: Option<Arc<dyn crate::keystroke_source::KeystrokeSource>>,
    ) -> Arc<Self> {
        use std::sync::atomic::AtomicBool;
        let listener = Arc::new(Self {
            active,
            confirm,
            source: source.clone(),
            enabled: AtomicBool::new(false),
            source_running: AtomicBool::new(false),
            return_down: AtomicBool::new(false),
        });
        if let Some(source) = source {
            let dispatch = Arc::downgrade(&listener);
            source.subscribe(Arc::new(
                move |event: &crate::keystroke_source::KeystrokeEvent| {
                    if let Some(listener) = dispatch.upgrade() {
                        listener.on_keystroke(event);
                    }
                },
            ));
        }
        listener
    }

    /// Toggle the listener; idempotent. Enabling starts the shared
    /// source, disabling stops this listener's claim on it.
    pub fn set_enabled(&self, enabled: bool) {
        use std::sync::atomic::Ordering;
        if self.enabled.swap(enabled, Ordering::SeqCst) == enabled {
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
        use std::sync::atomic::Ordering;
        self.enabled.store(false, Ordering::SeqCst);
        self.stop_source();
    }

    fn start_source(&self) {
        use std::sync::atomic::Ordering;
        let Some(source) = &self.source else {
            return;
        };
        if self.source_running.load(Ordering::SeqCst) {
            return;
        }
        let attached = source.start();
        self.source_running.store(attached, Ordering::SeqCst);
        tracing::info!(
            target: "eo::input",
            attached,
            "coordinate-calibration confirm source start requested"
        );
    }

    fn stop_source(&self) {
        use std::sync::atomic::Ordering;
        let Some(source) = &self.source else {
            return;
        };
        if !self.source_running.load(Ordering::SeqCst) {
            return;
        }
        source.stop();
        self.source_running.store(false, Ordering::SeqCst);
        self.return_down.store(false, Ordering::SeqCst);
    }

    fn on_keystroke(self: &Arc<Self>, event: &crate::keystroke_source::KeystrokeEvent) {
        use crate::keystroke_source::KeystrokeKind;
        use std::sync::atomic::Ordering;
        if !self.source_running.load(Ordering::SeqCst) || !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        if event.key != "return" {
            return;
        }
        match event.kind {
            KeystrokeKind::Release => {
                self.return_down.store(false, Ordering::SeqCst);
            }
            KeystrokeKind::Press => {
                // Press edge only: auto-repeat while held must not step
                // the flow through both corners in one hold.
                if self.return_down.swap(true, Ordering::SeqCst) {
                    return;
                }
                if !(self.active)() {
                    return;
                }
                // The completing press runs a capture + OCR validation
                // read; a short-lived thread keeps the dispatch cheap.
                // The service disarms this listener itself on every
                // Idle-reaching transition.
                let listener = self.clone();
                std::thread::spawn(move || {
                    (listener.confirm)();
                });
            }
        }
    }
}

/// A planet's coordinate window, supplied by the caller that knows the
/// selected map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordBounds {
    pub lon_min: i64,
    pub lon_max: i64,
    pub lat_min: i64,
    pub lat_max: i64,
}

impl CoordBounds {
    pub fn contains(&self, lon: i64, lat: i64) -> bool {
        (self.lon_min..=self.lon_max).contains(&lon) && (self.lat_min..=self.lat_max).contains(&lat)
    }
}

/// A scan outcome's one-word summary for the log line.
fn summarise(outcome: &CoordScanOutcome) -> &'static str {
    match outcome {
        CoordScanOutcome::Read(_) => "read",
        CoordScanOutcome::NoRegion => "no-region",
        CoordScanOutcome::CaptureFailed => "capture-failed",
        CoordScanOutcome::EngineUnavailable => "engine-unavailable",
        CoordScanOutcome::Unreadable { .. } => "unreadable",
        CoordScanOutcome::Implausible { .. } => "implausible",
    }
}

/// Write the last scan's debug artefacts: the captured frame as
/// `coord-scan-last.png` (exactly what the recogniser saw) and
/// `coord-scan-last.txt` (what it answered per line, and the outcome).
/// Best-effort: a failed write only logs.
fn write_debug_artefacts(
    dir: &Path,
    frame: &BgrImage,
    reads: &[(&'static str, String, f64)],
    outcome: &CoordScanOutcome,
) {
    crate::screen_capture::write_debug_frame(dir, "coord-scan-last.png", frame);
    let mut report = format!("outcome: {outcome:?}\nframe: {}x{}\n", frame.w, frame.h);
    for (label, text, confidence) in reads {
        report.push_str(&format!("{label}: {text:?} (confidence {confidence:.3})\n"));
    }
    if let Err(err) = std::fs::write(dir.join("coord-scan-last.txt"), report) {
        tracing::warn!(target: "eo::coord_capture", %err, "debug report not writable");
    }
}

/// The column the strip splits at: the centre of the widest blank run
/// in its middle third, or the exact midpoint when the strip shows no
/// such gap.
///
/// The two values sit in the strip's left and right halves with empty
/// space between them, and cutting through that space rather than at a
/// fixed midpoint keeps the boundary off the digits: the values are
/// unpadded, so one can be much wider than the other. The search is
/// confined to the middle third so the gap between a label and its own
/// value cannot win, and a strip with no discernible gap (a blank
/// capture, a uniform crop) falls back to the midpoint rather than
/// inventing a boundary.
fn split_column(frame: &BgrImage) -> usize {
    let midpoint = frame.w / 2;
    if frame.w < 6 || frame.h == 0 {
        return midpoint;
    }
    let mut column_means = vec![0.0f64; frame.w];
    for (x, mean) in column_means.iter_mut().enumerate() {
        let mut total = 0.0;
        for y in 0..frame.h {
            let idx = (y * frame.w + x) * 3;
            total += f64::from(frame.data[idx])
                + f64::from(frame.data[idx + 1])
                + f64::from(frame.data[idx + 2]);
        }
        *mean = total / (frame.h as f64 * 3.0);
    }
    let lo = column_means.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = column_means
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    if hi - lo <= f64::EPSILON {
        return midpoint;
    }
    let threshold = (lo + hi) / 2.0;

    let start = frame.w / 3;
    let end = frame.w - frame.w / 3;
    let mut best: Option<(usize, usize)> = None;
    let mut run_start: Option<usize> = None;
    let close = |run_start: &mut Option<usize>, at: usize, best: &mut Option<(usize, usize)>| {
        if let Some(from) = run_start.take() {
            let len = at - from;
            if best.is_none_or(|(_, longest)| len > longest) {
                *best = Some((from, len));
            }
        }
    };
    for (offset, mean) in column_means[start..end].iter().enumerate() {
        let x = start + offset;
        if *mean < threshold {
            run_start.get_or_insert(x);
        } else {
            close(&mut run_start, x, &mut best);
        }
    }
    close(&mut run_start, end, &mut best);

    best.map_or(midpoint, |(from, len)| from + len / 2)
}

/// Split a frame into its two side-by-side readout halves. The caller
/// guarantees a frame at least two columns wide, so both halves are
/// non-empty.
fn split_columns(frame: &BgrImage) -> (BgrImage, BgrImage) {
    let split = split_column(frame).clamp(1, frame.w - 1);
    let stride = frame.w * 3;
    let cut = split * 3;
    let mut left = Vec::with_capacity(frame.h * cut);
    let mut right = Vec::with_capacity(frame.h * (stride - cut));
    for row in frame.data.chunks_exact(stride) {
        left.extend_from_slice(&row[..cut]);
        right.extend_from_slice(&row[cut..]);
    }
    (
        BgrImage {
            data: left,
            h: frame.h,
            w: split,
        },
        BgrImage {
            data: right,
            h: frame.h,
            w: frame.w - split,
        },
    )
}

/// One readout line's value: the trailing digit run (the value follows
/// its label, so digits misread inside the label never win).
pub fn trailing_run(text: &str) -> Option<i64> {
    let mut last: Option<i64> = None;
    let mut current: Option<i64> = None;
    for ch in text.chars() {
        if let Some(digit) = digit_value(ch) {
            let digit = i64::from(digit);
            current = match current {
                Some(value) if value <= (i64::MAX - digit) / 10 => Some(value * 10 + digit),
                Some(_) => return None,
                None => Some(digit),
            };
        } else if let Some(done) = current.take() {
            last = Some(done);
        }
    }
    current.or(last)
}

/// Parse a whole-strip readout: exactly two integer runs in order
/// (longitude, latitude), with everything between runs treated as
/// separator noise. Fullwidth digits fold by value (the recogniser's
/// alphabet carries both forms). Any other run count is unreadable,
/// not guessable: a label misread into a digit (`L0N`) shows up as a
/// third run, and refusing it is what keeps the fallback honest now
/// that no altitude run can legitimately appear.
pub fn parse_coordinates(text: &str) -> Option<(i64, i64)> {
    let mut runs: Vec<i64> = Vec::new();
    let mut current: Option<i64> = None;
    for ch in text.chars() {
        if let Some(digit) = digit_value(ch) {
            let digit = i64::from(digit);
            current = Some(match current {
                Some(value) if value <= (i64::MAX - digit) / 10 => value * 10 + digit,
                Some(_) => return None,
                None => digit,
            });
        } else if let Some(done) = current.take() {
            runs.push(done);
        }
    }
    if let Some(done) = current {
        runs.push(done);
    }
    match runs.as_slice() {
        [lon, lat] => Some((*lon, *lat)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A 2x2 capture frame whose columns carry distinct bytes, so a mock
    /// reader can tell the left half (w 1, byte 1), the right half
    /// (w 1, byte 2), and the whole frame (w 2) apart.
    fn frame() -> BgrImage {
        BgrImage {
            data: [vec![1u8; 3], vec![2u8; 3], vec![1u8; 3], vec![2u8; 3]].concat(),
            h: 2,
            w: 2,
        }
    }

    /// A one-row strip whose columns take the given brightnesses: the
    /// column-gap geometry `split_column` reads, without any pixels
    /// standing for glyphs.
    fn strip(columns: &[u8]) -> BgrImage {
        BgrImage {
            data: columns.iter().flat_map(|b| [*b, *b, *b]).collect(),
            h: 1,
            w: columns.len(),
        }
    }

    /// Providers whose reader answers per readout half: `left`/`right`
    /// for the split columns, `whole` for the whole-frame fallback.
    fn providers_halves(
        left: &'static str,
        right: &'static str,
        whole: &'static str,
    ) -> CoordCaptureProviders {
        CoordCaptureProviders {
            cursor_position: Arc::new(|| Some((10, 20))),
            region: Arc::new(|| {
                Some(CoordRegion {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 20,
                })
            }),
            capture_region: Arc::new(|_, _, _, _| Some(frame())),
            read_text: Arc::new(move |img| {
                let text = match (img.w, img.data[0]) {
                    (1, 1) => left,
                    (1, 2) => right,
                    _ => whole,
                };
                Some((text.to_string(), 0.93))
            }),
            persist_region: Arc::new(|_| Ok(())),
            debug_dir: Arc::new(|| None),
        }
    }

    /// Providers where only the whole-frame single-line read carries
    /// text (the split halves read empty), exercising the fallback.
    fn providers_reading(text: &'static str) -> CoordCaptureProviders {
        providers_halves("", "", text)
    }

    #[test]
    fn parses_a_two_run_strip() {
        assert_eq!(parse_coordinates("61234, 75456"), Some((61234, 75456)));
        // OCR noise between the runs is separator, not failure.
        assert_eq!(
            parse_coordinates(" LON 61234 . LAT 75456 "),
            Some((61234, 75456))
        );
        // Fullwidth digits fold by value.
        assert_eq!(
            parse_coordinates("６１２３４, ７５４５６"),
            Some((61234, 75456))
        );
    }

    #[test]
    fn a_side_by_side_readout_reads_per_half() {
        // The client's layout: `LON <value>` on the strip's left half,
        // `LAT <value>` on its right, labels inside the rectangle.
        let service = CoordCaptureService::new(providers_halves("LON 31915", "LAT 19999", "junk"));
        assert!(matches!(
            service.scan(None),
            CoordScanOutcome::Read(CoordRead {
                lon: 31915,
                lat: 19999,
                ..
            })
        ));
    }

    #[test]
    fn a_label_misread_cannot_inject_a_value() {
        // OCR reading the label's O as a zero: the trailing run wins.
        let service = CoordCaptureService::new(providers_halves("L0N 31915", "LAT 19999", ""));
        assert!(matches!(
            service.scan(None),
            CoordScanOutcome::Read(CoordRead {
                lon: 31915,
                lat: 19999,
                ..
            })
        ));
    }

    #[test]
    fn trailing_run_takes_the_last_digit_run() {
        assert_eq!(trailing_run("LON 31915"), Some(31915));
        assert_eq!(trailing_run("L0N 31915"), Some(31915));
        assert_eq!(trailing_run("31915"), Some(31915));
        assert_eq!(trailing_run("no digits"), None);
        assert_eq!(trailing_run(""), None);
    }

    #[test]
    fn refuses_run_counts_that_are_not_a_readout() {
        assert_eq!(parse_coordinates(""), None);
        assert_eq!(parse_coordinates("no digits"), None);
        assert_eq!(parse_coordinates("61234"), None);
        // A label misread into a digit shows up as a third run, and the
        // whole-strip fallback refuses rather than guessing which two
        // of the three are the coordinates.
        assert_eq!(parse_coordinates("L0N 61234 LAT 75456"), None);
        assert_eq!(parse_coordinates("1, 2, 3, 4"), None);
    }

    #[test]
    fn the_split_cuts_the_gap_between_the_halves_not_the_midpoint() {
        // A wide longitude and a narrow latitude: bright through column
        // 7, a two-column gap, then bright again. The midpoint (7) sits
        // inside the left value's digits; the gap's centre is 9.
        let mut columns = [200u8; 15];
        columns[8] = 10;
        columns[9] = 10;
        assert_eq!(split_column(&strip(&columns)), 9);
    }

    #[test]
    fn a_gap_outside_the_middle_third_cannot_win() {
        // A wider blank run at the strip's left edge (outside the
        // search window) must not drag the split off the real gap.
        let mut columns = [200u8; 15];
        for column in columns.iter_mut().take(5) {
            *column = 10;
        }
        columns[8] = 10;
        columns[9] = 10;
        assert_eq!(split_column(&strip(&columns)), 9);
    }

    #[test]
    fn a_strip_with_no_discernible_gap_falls_back_to_the_midpoint() {
        // Uniform pixels: no contrast to read a boundary from.
        assert_eq!(split_column(&strip(&[128u8; 15])), 7);
        // Too narrow to search at all.
        assert_eq!(split_column(&strip(&[10, 200, 10, 200])), 2);
    }

    #[test]
    fn the_halves_carry_the_pixels_either_side_of_the_split() {
        let mut columns = [200u8; 15];
        columns[8] = 10;
        columns[9] = 10;
        let (left, right) = split_columns(&strip(&columns));
        assert_eq!((left.w, right.w), (9, 6));
        assert_eq!(left.w + right.w, 15, "no column is dropped or duplicated");
        assert_eq!(left.data.len(), left.w * left.h * 3);
        assert_eq!(right.data.len(), right.w * right.h * 3);
        // The gap's first column lands in the left half, its second in
        // the right: the cut runs through the dead space.
        assert_eq!(left.data[8 * 3], 10);
        assert_eq!(right.data[0], 10);
    }

    #[test]
    fn the_two_point_flow_normalises_and_persists() {
        let persisted = Arc::new(Mutex::new(None));
        let sink = persisted.clone();
        let cursor_calls = Arc::new(AtomicUsize::new(0));
        let counter = cursor_calls.clone();
        let mut providers = providers_reading("61234, 75456");
        // Second corner arrives up-left of the first: normalisation duty.
        providers.cursor_position = Arc::new(move || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            Some(if call == 0 { (200, 100) } else { (50, 40) })
        });
        providers.persist_region = Arc::new(move |region| {
            *sink.lock().unwrap() = Some(region);
            Ok(())
        });
        let service = CoordCaptureService::new(providers);

        assert_eq!(service.calibration_start(), CalibrationPhase::AwaitTopLeft);
        assert!(matches!(
            service.on_confirm(),
            CalibrationPhase::AwaitBottomRight { .. }
        ));
        assert_eq!(service.on_confirm(), CalibrationPhase::Idle);

        assert_eq!(
            *persisted.lock().unwrap(),
            Some(CoordRegion {
                x: 50,
                y: 40,
                w: 150,
                h: 60,
            })
        );
        // The completion took a validation read and stored the echo.
        assert!(matches!(
            service.last_validation(),
            Some(CoordScanOutcome::Read(CoordRead { lon: 61234, .. }))
        ));
    }

    #[test]
    fn a_degenerate_rectangle_rearms_the_second_corner() {
        let mut providers = providers_reading("1, 2");
        providers.cursor_position = Arc::new(|| Some((100, 100)));
        let service = CoordCaptureService::new(providers);
        service.calibration_start();
        service.on_confirm();
        // Same cursor position for the second corner: zero-size rect.
        assert!(matches!(
            service.on_confirm(),
            CalibrationPhase::AwaitBottomRight { .. }
        ));
    }

    #[test]
    fn enter_outside_a_flow_is_ignored() {
        let service = CoordCaptureService::new(providers_reading("1, 2"));
        assert_eq!(service.on_confirm(), CalibrationPhase::Idle);
        assert!(!service.calibration_active());
    }

    #[test]
    fn scan_answers_each_failure_leg_typed() {
        // No region.
        let service = CoordCaptureService::new(CoordCaptureProviders {
            cursor_position: Arc::new(|| Some((0, 0))),
            ..Default::default()
        });
        assert_eq!(service.scan(None), CoordScanOutcome::NoRegion);

        // Capture fails.
        let mut providers = providers_reading("x");
        providers.capture_region = Arc::new(|_, _, _, _| None);
        let service = CoordCaptureService::new(providers);
        assert_eq!(service.scan(None), CoordScanOutcome::CaptureFailed);

        // Engine unavailable.
        let mut providers = providers_reading("x");
        providers.read_text = Arc::new(|_| None);
        let service = CoordCaptureService::new(providers);
        assert_eq!(service.scan(None), CoordScanOutcome::EngineUnavailable);

        // Unreadable text.
        let service = CoordCaptureService::new(providers_reading("loading..."));
        assert!(matches!(
            service.scan(None),
            CoordScanOutcome::Unreadable { .. }
        ));

        // Implausible against bounds.
        let service = CoordCaptureService::new(providers_reading("999999, 75456"));
        let bounds = CoordBounds {
            lon_min: 16384,
            lon_max: 90112,
            lat_min: 24576,
            lat_max: 98304,
        };
        assert!(matches!(
            service.scan(Some(bounds)),
            CoordScanOutcome::Implausible { lon: 999999, .. }
        ));

        // A clean read inside bounds.
        let service = CoordCaptureService::new(providers_reading("61234, 75456"));
        assert!(matches!(
            service.scan(Some(bounds)),
            CoordScanOutcome::Read(CoordRead {
                lon: 61234,
                lat: 75456,
                ..
            })
        ));
    }
}
