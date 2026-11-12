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

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::clock::Clock;
use crate::eu_window::GameFocus;
use crate::game_ui::geometry::BgrImage;
use crate::game_ui::mob_history::{MobHistory, Sighting};
use crate::game_ui::mob_names::MobCatalogue;
use crate::game_ui::plates::{find_locked_plate, read_found_plate, PlateScratch};
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameWindowFrame};
use crate::hotbar_listener::GameFocusProbe;
use crate::time::clock_epoch_ms;

/// How long shutting down waits for the worker to finish its look: a look
/// can be held inside a capture by the desktop's consent prompt, and app
/// exit does not wait on the player for that.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

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
pub type PulseSink = Arc<dyn Fn(MobPulse) -> bool + Send + Sync>;

/// Told, with the session it watched for, when a watch ends by itself
/// (capture refused or stopped, detection found off where the capture
/// happens, a look failing, its looks no longer deliverable), so whoever
/// started it knows the screen is no longer read. A watch stopped by its
/// owner tells nothing.
pub type WatchEnded = Arc<dyn Fn(&str) + Send + Sync>;

/// A yes-or-no seam the composition root wires in: whether capture is
/// running, start it (the player's gesture), or whether detection is on.
pub type Probe = Arc<dyn Fn() -> bool + Send + Sync>;

/// Whether the watcher can capture when a session wants it, in the
/// player's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStatus {
    /// Capture is running, or nothing has to be started on this desktop,
    /// or the player's turning detection on will start it at the next look.
    Ready,
    /// Capture needs starting and is not running: turning detection off and
    /// on starts it.
    NotRunning,
    /// The desktop refused capture, or it failed or was stopped while
    /// watching: turning detection off and on asks again.
    Refused,
    /// Detection cannot run here: the recogniser or the interface profile
    /// did not load.
    Unavailable,
}

/// How the settings ask the watcher's [`CaptureStatus`].
pub type CaptureStatusProbe = Arc<dyn Fn() -> CaptureStatus + Send + Sync>;

/// How the watcher waits for its next look: until the injected clock's
/// monotonic reading reaches the first argument, or until the receiver
/// says to stop (its sender dropped). Answers whether to go on. Left
/// unset, the watcher waits on the clock in real time; a test drives the
/// clock itself.
pub type PulseWait = Arc<dyn Fn(f64, &mpsc::Receiver<()>) -> bool + Send + Sync>;

/// The creatures a name read off a plate is matched against: the bundled
/// list and the names the player has confirmed. Shared by the watcher,
/// which matches with it, and the review, which rebuilds it when the
/// player confirms a name.
pub struct CatalogueCell(RwLock<Arc<MobCatalogue>>);

impl CatalogueCell {
    pub fn new(catalogue: MobCatalogue) -> Self {
        Self(RwLock::new(Arc::new(catalogue)))
    }

    /// The catalogue as it stands.
    pub fn current(&self) -> Arc<MobCatalogue> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Put a rebuilt catalogue in place: the next look matches with it.
    pub fn replace(&self, catalogue: MobCatalogue) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(catalogue);
    }
}

impl Default for CatalogueCell {
    fn default() -> Self {
        Self::new(MobCatalogue::default())
    }
}

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
#[derive(Default)]
pub struct DetectorSlot(OnceLock<Arc<dyn MobDetector>>);

impl DetectorSlot {
    /// Bind the detector; a second call is ignored.
    pub fn bind(&self, detector: Arc<dyn MobDetector>) {
        let _ = self.0.set(detector);
    }
}

impl MobDetector for DetectorSlot {
    fn new_history(&self) -> Option<MobHistory> {
        self.0.get()?.new_history()
    }

    fn start(&self, session_id: &str, sink: PulseSink, ended: WatchEnded) {
        if let Some(detector) = self.0.get() {
            detector.start(session_id, sink, ended);
        }
    }

    fn stop(&self) {
        if let Some(detector) = self.0.get() {
            detector.stop();
        }
    }

    fn watches(&self, session_id: &str) -> bool {
        self.0
            .get()
            .is_some_and(|detector| detector.watches(session_id))
    }

    fn permit_capture(&self) {
        if let Some(detector) = self.0.get() {
            detector.permit_capture();
        }
    }

    fn shutdown(&self) {
        if let Some(detector) = self.0.get() {
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

/// The running watch.
struct Watching {
    watch: u64,
    session_id: String,
    /// Told if the watch ends by itself.
    ended: Option<WatchEnded>,
    /// Held only to be dropped: leaving this state disconnects the
    /// worker's receiver, which wakes it from its wait to stop.
    _stop: mpsc::Sender<()>,
}

struct State {
    watching: Option<Watching>,
    /// The last watch's number; each start takes the next.
    last_watch: u64,
    /// The desktop refused a capture, or capture stopped while watching:
    /// nothing is captured again until the player turns detection back on.
    capture_refused: bool,
    /// The player turned detection on: the next look may start capture.
    may_start: bool,
}

/// Counts the live workers, so shutting down can wait for them.
#[derive(Default)]
struct Workers {
    live: Mutex<usize>,
    done: Condvar,
}

/// One live worker; dropping it, however the worker ends, counts it out.
struct WorkerToken {
    service: Arc<MobWatch>,
}

impl Drop for WorkerToken {
    fn drop(&mut self) {
        let workers = &self.service.workers;
        let mut live = workers.live.lock().unwrap_or_else(PoisonError::into_inner);
        *live = live.saturating_sub(1);
        workers.done.notify_all();
    }
}

/// What a worker reads with, taken once when its watch starts.
struct Reader {
    profile: Arc<UiProfile>,
    read_text: FrameReader,
}

/// What one look came to.
enum Look {
    /// The window was captured: the plate locked then, if any.
    Seen {
        at_ms: i64,
        sighting: Option<Sighting>,
    },
    /// Nothing was captured (the game was in the background, or its
    /// window could not be had): the screen was not watched.
    Missed,
    /// Capture was refused, or is no longer running.
    Refused,
    /// Detection was turned off.
    Disabled,
}

pub struct MobWatch {
    providers: MobWatchProviders,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Held across each look, so a worker whose watch was just replaced
    /// never captures beside its successor.
    looking: Mutex<()>,
    workers: Workers,
}

impl MobWatch {
    pub fn new(providers: MobWatchProviders, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            providers,
            clock,
            state: Mutex::new(State {
                watching: None,
                last_watch: 0,
                capture_refused: false,
                may_start: false,
            }),
            looking: Mutex::new(()),
            workers: Workers::default(),
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn reader(&self) -> Option<Reader> {
        Some(Reader {
            profile: self.providers.profile.clone()?,
            read_text: self.providers.read_text.clone()?,
        })
    }

    /// The session being watched, if any.
    pub fn watching(&self) -> Option<String> {
        self.lock_state()
            .watching
            .as_ref()
            .map(|w| w.session_id.clone())
    }

    /// Whether a session starting now could be watched, in the player's
    /// terms.
    pub fn capture_status(&self) -> CaptureStatus {
        if self.reader().is_none() {
            return CaptureStatus::Unavailable;
        }
        let state = self.lock_state();
        if state.capture_refused {
            CaptureStatus::Refused
        } else if state.may_start || (self.providers.capture_running)() {
            CaptureStatus::Ready
        } else {
            CaptureStatus::NotRunning
        }
    }

    /// Start watching for `session_id`. A watch for the same session is
    /// left running; one for another session is replaced. Nothing starts
    /// without the profile and the recogniser, after capture was refused,
    /// or while capture is not running and the player has not just turned
    /// detection on.
    pub fn start(self: &Arc<Self>, session_id: &str, sink: PulseSink) {
        self.begin(session_id, sink, None);
    }

    /// Start watching as [`MobWatch::start`] does, telling `ended` if the
    /// watch ends by itself.
    pub fn start_reporting_end(
        self: &Arc<Self>,
        session_id: &str,
        sink: PulseSink,
        ended: WatchEnded,
    ) {
        self.begin(session_id, sink, Some(ended));
    }

    fn begin(self: &Arc<Self>, session_id: &str, sink: PulseSink, ended: Option<WatchEnded>) {
        let Some(reader) = self.reader() else {
            return;
        };
        let mut state = self.lock_state();
        if state.capture_refused || !(state.may_start || (self.providers.capture_running)()) {
            return;
        }
        if state
            .watching
            .as_ref()
            .is_some_and(|w| w.session_id == session_id)
        {
            return;
        }
        // Replacing the running watch drops its sender: that worker
        // wakes and ends.
        state.watching = None;
        let watch = state.last_watch + 1;
        let (stop, stopped) = mpsc::channel();
        *self
            .workers
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        let token = WorkerToken {
            service: self.clone(),
        };
        let worker = self.clone();
        let session = session_id.to_string();
        let spawned = std::thread::Builder::new()
            .name("mob-watch".into())
            .spawn(move || worker.watch(watch, &session, &reader, &sink, &stopped, token));
        // A worker that did not start dropped its token with the closure,
        // counting itself out; nothing is watched.
        if spawned.is_err() {
            tracing::warn!(target: "eo::mob_watch", "the creature watcher could not start");
            return;
        }
        state.last_watch = watch;
        state.watching = Some(Watching {
            watch,
            session_id: session_id.to_string(),
            ended,
            _stop: stop,
        });
    }

    /// Stop watching. Returns at once: the worker ends within its look.
    pub fn stop(&self) {
        self.lock_state().watching = None;
    }

    /// The player turned detection on: the next look may start capture,
    /// asking the desktop's consent once, even where it was refused before.
    pub fn permit_capture(&self) {
        let mut state = self.lock_state();
        state.capture_refused = false;
        state.may_start = true;
    }

    /// Whether this look may start capture: the player turned detection on
    /// and no look has asked since. Taken once.
    fn take_start(&self) -> bool {
        std::mem::take(&mut self.lock_state().may_start)
    }

    /// Stop watching and wait, for a short while, for the worker to finish
    /// its look: at app exit, where nothing may go on capturing behind the
    /// closing app. A worker still out after the wait is left to end with
    /// the process.
    pub fn shutdown(&self) {
        self.stop();
        if !self.wait_for_workers(SHUTDOWN_WAIT) {
            tracing::warn!(
                target: "eo::mob_watch",
                "the creature watcher was still capturing at exit; not waiting for it"
            );
        }
    }

    /// Wait until no worker is running; answers whether that happened
    /// within `timeout`.
    pub fn wait_for_workers(&self, timeout: Duration) -> bool {
        let live = self
            .workers
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (live, _) = self
            .workers
            .done
            .wait_timeout_while(live, timeout, |live| *live > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *live == 0
    }

    fn is_watching(&self, watch: u64) -> bool {
        self.lock_state()
            .watching
            .as_ref()
            .is_some_and(|w| w.watch == watch)
    }

    /// End `watch` because the desktop refused a capture, if it is still
    /// the one running.
    fn refused(&self, watch: u64) {
        let ended = {
            let mut state = self.lock_state();
            state.capture_refused = true;
            Self::take_ended(&mut state, watch)
        };
        Self::tell_ended(ended);
    }

    /// End `watch` because a look failed, if it is still the one running.
    fn failed(&self, watch: u64) {
        let ended = Self::take_ended(&mut self.lock_state(), watch);
        Self::tell_ended(ended);
    }

    /// Clear `watch` if it is still the one running, with whom to tell.
    fn take_ended(state: &mut State, watch: u64) -> Option<(WatchEnded, String)> {
        if state.watching.as_ref().is_none_or(|w| w.watch != watch) {
            return None;
        }
        let ended = state.watching.take()?;
        ended.ended.map(|tell| (tell, ended.session_id))
    }

    /// Tell, outside the state's lock, that a watch ended by itself.
    fn tell_ended(ended: Option<(WatchEnded, String)>) {
        if let Some((tell, session_id)) = ended {
            tell(&session_id);
        }
    }

    fn wait(&self, due: f64, stopped: &mpsc::Receiver<()>) -> bool {
        match &self.providers.wait {
            Some(wait) => wait(due, stopped),
            None => wait_on_clock(self.clock.as_ref(), due, stopped),
        }
    }

    /// The worker: wait for the next look, look, hand the look on, until
    /// the watch is no longer the running one.
    fn watch(
        &self,
        watch: u64,
        session_id: &str,
        reader: &Reader,
        sink: &PulseSink,
        stopped: &mpsc::Receiver<()>,
        _token: WorkerToken,
    ) {
        let interval = reader.profile.mob_history.pulse_ms.max(1) as f64 / 1000.0;
        // A capture taking longer than this is too late to stand for the
        // moment it was stamped with: a gap, not an observation.
        let slack = reader.profile.mob_history.capture_slack_ms as f64 / 1000.0;
        let mut scratch = PlateScratch::default();
        // The first look is taken at once.
        let mut due = self.clock.monotonic();
        loop {
            if !self.wait(due, stopped) {
                return;
            }
            let look = {
                let _looking = self.looking.lock().unwrap_or_else(PoisonError::into_inner);
                // Checked again after waiting for another worker's look.
                if !self.is_watching(watch) {
                    return;
                }
                if !(self.providers.enabled)() {
                    self.failed(watch);
                    return;
                }
                if self.take_start() && !(self.providers.start_capture)() {
                    self.refused(watch);
                    return;
                }
                catch_unwind(AssertUnwindSafe(|| self.look(reader, slack, &mut scratch)))
            };
            match look {
                Ok(Look::Seen { at_ms, sighting }) => {
                    // A look finishing after its watch ended is dropped.
                    if !self.is_watching(watch) {
                        return;
                    }
                    let delivered = sink(MobPulse {
                        session_id: session_id.to_string(),
                        at_ms,
                        sighting,
                    });
                    if !delivered {
                        self.failed(watch);
                        return;
                    }
                }
                Ok(Look::Missed) => {}
                Ok(Look::Disabled) => {
                    self.failed(watch);
                    return;
                }
                Ok(Look::Refused) => {
                    self.refused(watch);
                    return;
                }
                Err(_) => {
                    tracing::error!(
                        target: "eo::mob_watch",
                        "reading the locked plate failed; the creature watcher stops"
                    );
                    self.failed(watch);
                    return;
                }
            }
            due = next_due(due, interval, self.clock.monotonic());
        }
    }

    /// One look: capture the game window, find the locked plate, read it
    /// and match its name.
    fn look(&self, reader: &Reader, slack: f64, scratch: &mut PlateScratch) -> Look {
        if !(self.providers.enabled)() {
            return Look::Disabled;
        }
        if self
            .providers
            .focus
            .as_ref()
            .is_some_and(|focus| focus() == GameFocus::Unfocused)
        {
            return Look::Missed;
        }
        let metrics = eo_wire::metrics::metrics();
        // Stamped as the capture starts: the profile allows for how far the
        // capture lags this moment.
        let at_ms = clock_epoch_ms(self.clock.as_ref());
        let began = self.clock.monotonic();
        let started = Instant::now();
        let frame = (self.providers.frame)(FrameArea::Whole);
        let image = match frame {
            Ok(frame) => {
                // Only a capture that found the window is a measure of what
                // capturing costs.
                metrics.record_plate_capture(started.elapsed());
                if self.clock.monotonic() - began > slack {
                    return Look::Missed;
                }
                frame.image
            }
            Err(FrameUnavailable::CaptureNotStarted) => return Look::Refused,
            Err(
                FrameUnavailable::WindowNotFound
                | FrameUnavailable::OutsideCapturableArea
                | FrameUnavailable::CaptureFailed,
            ) => return Look::Missed,
        };
        let profile = reader.profile.as_ref();
        let started = Instant::now();
        let locked = find_locked_plate(&image, profile, scratch);
        metrics.record_plate_search(started.elapsed());
        let Some(locked) = locked else {
            return Look::Seen {
                at_ms,
                sighting: None,
            };
        };
        let started = Instant::now();
        // A crop the recogniser fails on reads as nothing, which no
        // grammar or name accepts.
        let read_text = &reader.read_text;
        let recogniser =
            |crop: &BgrImage| Some(read_text(crop).unwrap_or_else(|| (String::new(), 0.0)));
        let sighting = read_found_plate(&image, &locked, profile, &recogniser).map(|read| {
            Sighting::of(
                &read,
                &self.providers.catalogue.current(),
                &profile.mob_names,
            )
        });
        metrics.record_plate_read(started.elapsed());
        Look::Seen { at_ms, sighting }
    }
}

impl MobDetector for Arc<MobWatch> {
    fn new_history(&self) -> Option<MobHistory> {
        let reader = self.reader()?;
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

/// When the look after one due at `due` is due, `interval` apart, now that
/// the clock reads `now`: the next on the cadence that is still to come.
/// Looks that came due while the last one ran are dropped, never taken
/// late one after another.
pub(crate) fn next_due(due: f64, interval: f64, now: f64) -> f64 {
    let next = due + interval;
    if next > now {
        return next;
    }
    let missed = ((now - next) / interval).floor() + 1.0;
    next + missed * interval
}

/// Wait in real time until `clock`'s monotonic reading reaches `due`, or
/// until told to stop.
fn wait_on_clock(clock: &dyn Clock, due: f64, stopped: &mpsc::Receiver<()>) -> bool {
    loop {
        let left = due - clock.monotonic();
        if left <= 0.0 {
            return matches!(stopped.try_recv(), Err(TryRecvError::Empty));
        }
        match stopped.recv_timeout(Duration::from_secs_f64(left)) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

#[cfg(test)]
mod tests;
