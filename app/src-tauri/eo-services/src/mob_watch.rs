//! The creature watcher: while a hunt is tracked, it looks at the game
//! window about once a second and reads the plate of the creature the
//! player is locked on to, so the session can tell which creature each
//! kill was (see [`crate::game_ui::mob_history`] and the tracker's
//! detection).
//!
//! The tracker drives it: a session starting with detection on starts
//! it, and the session stopping, detection being turned off, or the app
//! closing stops it. One worker does the looking, so no two captures are
//! ever taken at once, and it runs on a thread of its own, never on the
//! async runtime. Each look is checked against the watch it was started
//! for, so a look finishing after its session ended is dropped.
//!
//! A look is taken only while the game window holds focus (a platform
//! that cannot tell where focus is does not stop it; only a window known
//! to be in the background does), and nothing is captured at all when the
//! window is not there to be captured. Nothing a look captures is kept:
//! the frame is searched, the locked plate read, and the frame dropped.
//!
//! The looks keep to the cadence the interface profile gives. A look that
//! runs past the moment the next was due does not make the missed looks
//! up afterwards: they are dropped, and the history simply sees the gap,
//! which it treats as the screen not having been watched then. What each
//! look costs (the capture, the search for the plate, and the reading of
//! the plate found) is recorded in the application's latency metrics.
//!
//! The watcher never starts screen capture by itself. Where the desktop
//! asks the player's consent before an application may see the screen
//! (the ScreenCast portal), the only gesture that may start capture is the
//! player turning detection on: the next look after that asks once. Every
//! other look captures only from a capture already running, so a watch
//! never puts the consent question to the player and never reopens a
//! stream the player stopped. A session starting while capture is not
//! running starts no watch, and the settings say why. When capture is
//! refused, fails or is stopped while watching, the watch ends and is not
//! started again until the player turns detection off and on.
//!
//! The setting is also checked where the capture happens, before every
//! look, and a watch whose looks can no longer be delivered (the tracker is
//! gone) ends.
//!
//! How the looks are run (the worker and its life, the cadence, focus,
//! consent and the lock no two captures are taken beside) is the shared
//! [`crate::screen_watch`]; this module is what one look does.

use std::sync::Arc;
use std::time::Instant;

use crate::clock::Clock;
use crate::game_ui::geometry::BgrImage;
use crate::game_ui::mob_history::{MobHistory, Sighting};
use crate::game_ui::mob_names::MobCatalogue;
use crate::game_ui::plates::{find_locked_plate, read_found_plate, PlateScratch};
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameWindowFrame};
use crate::hotbar_listener::GameFocusProbe;
use crate::screen_watch::{
    Eyes, LateBound, Look, Reader, ScreenWatch, Seams, Sink, SwapCell, Watcher,
};

pub use crate::screen_watch::{CaptureStatus, CaptureStatusProbe, Probe, PulseWait, WatchEnded};

/// One look at the game window, for the session it was taken for: when the
/// window was captured (epoch milliseconds on the injected clock), and the
/// locked plate read there with its name resolved, or `None` when no plate
/// was locked.
#[derive(Debug, Clone, PartialEq)]
pub struct MobPulse {
    pub session_id: String,
    pub at_ms: i64,
    pub sighting: Option<Sighting>,
}

/// Where the watcher hands each look: whether it was delivered. A look
/// that could not be (the tracker is gone) ends the watch.
pub type PulseSink = Sink<MobPulse>;

/// The creatures a name read off a plate is matched against: the bundled
/// list and the names the player has confirmed. Shared by the watcher,
/// which matches with it, and the review, which rebuilds it when the
/// player confirms a name.
pub type CatalogueCell = SwapCell<MobCatalogue>;

/// The tracker's handle on creature detection.
pub trait MobDetector: Send + Sync {
    /// A fresh history for a session, when detection can run here (the
    /// interface profile and the recogniser are loaded).
    fn new_history(&self) -> Option<MobHistory>;
    /// Start looking for `session_id`, handing each look to `sink`, and
    /// telling `ended` if the watch ends by itself. A watch already running
    /// for another session stops first.
    fn start(&self, session_id: &str, sink: PulseSink, ended: WatchEnded);
    /// Stop looking. Returns at once; the worker ends within its look.
    fn stop(&self);
    /// Whether looks are being taken for `session_id` now: a watch can fail
    /// to start (capture not running, refused) or end on its own.
    fn watches(&self, session_id: &str) -> bool;
    /// The player turned detection on: a capture the desktop refused
    /// before may be asked for again.
    fn permit_capture(&self);
    /// Stop at app exit, waiting briefly for a look in progress.
    fn shutdown(&self) {
        self.stop();
    }
}

/// Detection that never runs: what a tracker has without a watcher (a
/// replayed log, a test that does not look at a screen).
pub struct NoDetector;

impl MobDetector for NoDetector {
    fn new_history(&self) -> Option<MobHistory> {
        None
    }

    fn start(&self, _session_id: &str, _sink: PulseSink, _ended: WatchEnded) {}

    fn stop(&self) {}

    fn watches(&self, _session_id: &str) -> bool {
        false
    }

    fn permit_capture(&self) {}
}

/// A detector bound after the tracker is built: the watcher reads with
/// the recogniser, which is loaded later in composition than the tracker
/// starts. Until it is bound, detection does not run.
pub type DetectorSlot = LateBound<dyn MobDetector>;

impl MobDetector for DetectorSlot {
    fn new_history(&self) -> Option<MobHistory> {
        self.bound()?.new_history()
    }

    fn start(&self, session_id: &str, sink: PulseSink, ended: WatchEnded) {
        if let Some(detector) = self.bound() {
            detector.start(session_id, sink, ended);
        }
    }

    fn stop(&self) {
        if let Some(detector) = self.bound() {
            detector.stop();
        }
    }

    fn watches(&self, session_id: &str) -> bool {
        self.bound()
            .is_some_and(|detector| detector.watches(session_id))
    }

    fn permit_capture(&self) {
        if let Some(detector) = self.bound() {
            detector.permit_capture();
        }
    }

    fn shutdown(&self) {
        if let Some(detector) = self.bound() {
            detector.shutdown();
        }
    }
}

/// The seams the composition root wires in.
pub struct MobWatchProviders {
    /// The whole game window, captured only if capture is already running:
    /// it must never start capture (see
    /// [`crate::eu_window::capture_game_window_running`]).
    pub frame: GameWindowFrame,
    /// Whether capture is running now, starting nothing.
    pub capture_running: Probe,
    /// Start capture, asking the desktop's consent where it must: called
    /// only for the player's turning detection on.
    pub start_capture: Probe,
    /// Whether detection is on, read live before every look.
    pub enabled: Probe,
    /// Where keyboard focus sits; `None` where it cannot be asked.
    pub focus: Option<GameFocusProbe>,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The recogniser; `None` when none was loaded.
    pub read_text: Option<FrameReader>,
    pub catalogue: Arc<CatalogueCell>,
    /// How to wait for the next look; `None` waits in real time.
    pub wait: Option<PulseWait>,
}

impl Default for MobWatchProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|_| Err(FrameUnavailable::WindowNotFound)),
            capture_running: Arc::new(|| true),
            start_capture: Arc::new(|| true),
            enabled: Arc::new(|| false),
            focus: None,
            profile: None,
            read_text: None,
            catalogue: Arc::new(CatalogueCell::default()),
            wait: None,
        }
    }
}

/// What the creature watcher does at each look: capture the game window,
/// find the locked plate, read it and match its name.
pub struct PlateLook {
    profile: Option<Arc<UiProfile>>,
    read_text: Option<FrameReader>,
    catalogue: Arc<CatalogueCell>,
}

/// What the creature watcher keeps from one look to the next.
pub struct PlateMemory {
    scratch: PlateScratch,
    /// A capture whose pixels took longer than this to grab, in seconds,
    /// cannot be placed in time well enough: a gap, not an observation.
    slack: f64,
}

impl Watcher for PlateLook {
    type Pulse = MobPulse;
    type Memory = PlateMemory;
    const THREAD: &'static str = "mob-watch";
    const WHAT: &'static str = "the creature watcher";

    fn reader(&self) -> Option<Reader> {
        Some(Reader {
            profile: self.profile.clone()?,
            read_text: self.read_text.clone()?,
        })
    }

    fn interval(reader: &Reader) -> f64 {
        reader.profile.mob_history.pulse_ms.max(1) as f64 / 1000.0
    }

    fn memory(reader: &Reader) -> PlateMemory {
        PlateMemory {
            scratch: PlateScratch::default(),
            slack: reader.profile.mob_history.capture_slack_ms as f64 / 1000.0,
        }
    }

    fn look(&self, reader: &Reader, memory: &mut PlateMemory, eyes: &Eyes) -> Look<MobPulse> {
        let metrics = eo_wire::metrics::metrics();
        // Stamped as the capture starts, once the capture lock is held: the
        // profile allows for how far the capture lags this moment.
        let Some(capture) = eyes.capture(FrameArea::Whole) else {
            return Look::Quiet;
        };
        let at_ms = capture.at_ms;
        let image = match capture.frame {
            Ok(frame) => {
                // Only a capture that found the window is a measure of what
                // capturing costs.
                metrics.record_plate_capture(capture.took);
                if capture.age.as_secs_f64() > memory.slack {
                    return Look::Quiet;
                }
                frame.image
            }
            Err(FrameUnavailable::CaptureNotStarted) => return Look::Refused,
            Err(
                FrameUnavailable::WindowNotFound
                | FrameUnavailable::OutsideCapturableArea
                | FrameUnavailable::CaptureFailed,
            ) => return Look::Quiet,
        };
        let profile = reader.profile.as_ref();
        let started = Instant::now();
        let locked = find_locked_plate(&image, profile, &mut memory.scratch);
        metrics.record_plate_search(started.elapsed());
        let pulse = |sighting| MobPulse {
            session_id: eyes.session_id().to_string(),
            at_ms,
            sighting,
        };
        let Some(locked) = locked else {
            return Look::Tell(pulse(None));
        };
        let started = Instant::now();
        // A crop the recogniser fails on reads as nothing, which no
        // grammar or name accepts.
        let read_text = &reader.read_text;
        let recogniser =
            |crop: &BgrImage| Some(read_text(crop).unwrap_or_else(|| (String::new(), 0.0)));
        let sighting = read_found_plate(&image, &locked, profile, &recogniser)
            .map(|read| Sighting::of(&read, &self.catalogue.current(), &profile.mob_names));
        metrics.record_plate_read(started.elapsed());
        Look::Tell(pulse(sighting))
    }
}

/// The creature watcher.
pub type MobWatch = ScreenWatch<PlateLook>;

impl ScreenWatch<PlateLook> {
    pub fn new(providers: MobWatchProviders, clock: Arc<dyn Clock>) -> Arc<Self> {
        ScreenWatch::from_parts(
            PlateLook {
                profile: providers.profile,
                read_text: providers.read_text,
                catalogue: providers.catalogue,
            },
            Seams {
                frame: providers.frame,
                capture_running: providers.capture_running,
                start_capture: providers.start_capture,
                enabled: providers.enabled,
                focus: providers.focus,
                wait: providers.wait,
            },
            clock,
        )
    }
}

impl MobDetector for Arc<MobWatch> {
    fn new_history(&self) -> Option<MobHistory> {
        let reader = self.watcher().reader()?;
        Some(MobHistory::new(
            reader.profile.mob_history.clone(),
            reader.profile.mob_names.clone(),
        ))
    }

    fn start(&self, session_id: &str, sink: PulseSink, ended: WatchEnded) {
        MobWatch::start_reporting_end(self, session_id, sink, ended);
    }

    fn stop(&self) {
        MobWatch::stop(self);
    }

    fn watches(&self, session_id: &str) -> bool {
        MobWatch::watching(self).as_deref() == Some(session_id)
    }

    fn permit_capture(&self) {
        MobWatch::permit_capture(self);
    }

    fn shutdown(&self) {
        MobWatch::shutdown(self);
    }
}

#[cfg(test)]
mod tests;
