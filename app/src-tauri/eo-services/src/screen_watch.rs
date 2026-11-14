//! What every watcher of the game window is built from: a worker that,
//! while a session is watched, looks at the game window on a cadence and
//! hands what it sees to its consumer. A watcher supplies only what one
//! look does (see [`Watcher`]); everything about running the looks is
//! here, once.
//!
//! One worker does a watcher's looking, on a thread of its own, never on
//! the async runtime. Starting a watch for another session replaces the
//! running one; stopping returns at once and the worker ends within its
//! look; app exit waits for it only briefly. Each look is checked against
//! the watch it was started for, so a look finishing after its watch ended
//! is dropped, and a watch whose looks can no longer be delivered (its
//! consumer is gone) ends.
//!
//! A look is taken only while the game window holds focus (a platform
//! that cannot tell where focus is does not stop it; only a window known
//! to be in the background does). The looks keep to the watcher's
//! cadence: a look that runs past the moment the next was due does not
//! make the missed looks up afterwards; they are dropped. A hint asks for
//! a look as soon as the watcher allows instead of at the next interval;
//! hints are coalesced, and never make two looks overlap.
//!
//! No two captures of the game window are ever taken at once, by one
//! watcher or by several: a capture is taken under one lock the whole
//! process shares, held for the capture alone, so one watcher's reading of
//! what it captured never holds up another's capture. A look is stamped
//! with the moment its pixels were taken, so neither waiting behind
//! another watcher's capture nor finding the game window (slow on a busy
//! machine) makes its time early or counts against how late a capture may
//! be: only the grab of the pixels does. A watch stopped while it waited
//! captures nothing.
//!
//! What the looks came to is kept minute by minute (see [`LookMinute`]):
//! how many were taken, skipped, missed or given up as late, how many
//! found what they look for and could not read it, and the longest of
//! each of a look's stages. Whoever started the watch collects the
//! finished minutes and stores them, so a session can be judged afterwards
//! for how well the screen was read during it.
//!
//! Starting capture for the player's gesture is done under that lock too,
//! on purpose: on a desktop that asks the player's consent, the question
//! can stay open for as long as the player leaves it, and another watcher
//! capturing meanwhile would be told capture is not running and take
//! itself for refused. So a watcher may wait behind another's consent
//! question; at app exit each watcher's wait for its worker is bounded, so
//! exit takes at most two such bounded waits, never the player's time.
//!
//! No watcher starts screen capture by itself. Where the desktop asks the
//! player's consent before an application may see the screen (the
//! ScreenCast portal), the only gesture that may start capture is the
//! player turning the watcher's setting on: the next look after that asks
//! once. Every other look captures only from a capture already running,
//! so a watch never puts the consent question to the player and never
//! reopens a stream the player stopped. A session starting while capture
//! is not running starts no watch. When capture is refused, fails or is
//! stopped while watching, the watch ends and is not started again until
//! the player turns the setting off and on. The setting is also checked
//! where the capture happens, before every look.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::clock::Clock;
use crate::eu_window::GameFocus;
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameFrame, GameWindowFrame};
use crate::hotbar_listener::GameFocusProbe;
use crate::time::clock_epoch_ms;

/// How long shutting down waits for a worker to finish its look: a look
/// can be held inside a capture by the desktop's consent prompt, and app
/// exit does not wait on the player for that.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// Where a watcher hands what its looks tell: whether it was delivered. A
/// pulse that could not be (the consumer is gone) ends the watch.
pub type Sink<P> = Arc<dyn Fn(P) -> bool + Send + Sync>;

/// Told, with the session it watched for, when a watch ends by itself
/// (capture refused or stopped, the setting found off where the capture
/// happens, a look failing, its looks no longer deliverable), so whoever
/// started it knows the screen is no longer read. A watch stopped by its
/// owner tells nothing.
pub type WatchEnded = Arc<dyn Fn(&str) + Send + Sync>;

/// A yes-or-no seam the composition root wires in: whether capture is
/// running, start it (the player's gesture), or whether the watcher's
/// setting is on.
pub type Probe = Arc<dyn Fn() -> bool + Send + Sync>;

/// Whether a watcher can capture when a session wants it, in the
/// player's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStatus {
    /// Capture is running, or nothing has to be started on this desktop,
    /// or the player's turning the setting on will start it at the next
    /// look.
    Ready,
    /// Capture needs starting and is not running: turning the setting off
    /// and on starts it.
    NotRunning,
    /// The desktop refused capture, or it failed or was stopped while
    /// watching: turning the setting off and on asks again.
    Refused,
    /// The watcher cannot run here: the recogniser or the interface
    /// profile did not load.
    Unavailable,
}

/// How the settings ask a watcher's [`CaptureStatus`].
pub type CaptureStatusProbe = Arc<dyn Fn() -> CaptureStatus + Send + Sync>;

/// How a watcher waits for its next look: until the injected clock's
/// monotonic reading reaches the first argument, until a message on the
/// receiver asks for a look now (a hint), or until the receiver says to
/// stop (its sender dropped). Answers whether to go on. Left unset, the
/// watcher waits on the clock in real time; a test drives the clock
/// itself.
pub type PulseWait = Arc<dyn Fn(f64, &mpsc::Receiver<()>) -> bool + Send + Sync>;

/// A value a watcher's looks read as it stands and its owner replaces in
/// place: the next look uses the new one.
pub struct SwapCell<T>(RwLock<Arc<T>>);

impl<T> SwapCell<T> {
    pub fn new(value: T) -> Self {
        Self(RwLock::new(Arc::new(value)))
    }

    /// The value as it stands.
    pub fn current(&self) -> Arc<T> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Put a rebuilt value in place: the next look uses it.
    pub fn replace(&self, value: T) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(value);
    }
}

impl<T: Default> Default for SwapCell<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// A detector bound after its holder is built: a watcher reads with the
/// recogniser, which is loaded later in composition than the tracker
/// starts. Until it is bound, nothing is watched.
pub struct LateBound<D: ?Sized>(OnceLock<Arc<D>>);

impl<D: ?Sized> LateBound<D> {
    /// Bind the detector; a second call is ignored.
    pub fn bind(&self, detector: Arc<D>) {
        let _ = self.0.set(detector);
    }

    /// The detector, once bound.
    pub fn bound(&self) -> Option<&Arc<D>> {
        self.0.get()
    }
}

impl<D: ?Sized> Default for LateBound<D> {
    fn default() -> Self {
        Self(OnceLock::new())
    }
}

/// The one lock every capture of the game window is taken under, whichever
/// watcher takes it.
static CAPTURE: Mutex<()> = Mutex::new(());

pub(crate) fn capture_gate() -> MutexGuard<'static, ()> {
    CAPTURE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a capture is being taken now, by any watcher.
#[cfg(test)]
pub(crate) fn capture_is_gated() -> bool {
    CAPTURE.try_lock().is_err()
}

/// The seams every watcher is composed over.
pub struct Seams {
    /// The game window, captured only if capture is already running: it
    /// must never start capture (see
    /// [`crate::eu_window::capture_game_window_running`]).
    pub frame: GameWindowFrame,
    /// Whether capture is running now, starting nothing.
    pub capture_running: Probe,
    /// Start capture, asking the desktop's consent where it must: called
    /// only for the player's turning the setting on.
    pub start_capture: Probe,
    /// Whether the watcher's setting is on, read live before every look.
    pub enabled: Probe,
    /// Where keyboard focus sits; `None` where it cannot be asked.
    pub focus: Option<GameFocusProbe>,
    /// How to wait for the next look; `None` waits in real time.
    pub wait: Option<PulseWait>,
}

/// What a watcher reads with, taken once when its watch starts.
pub struct Reader {
    pub profile: Arc<UiProfile>,
    pub read_text: FrameReader,
}

/// What one look came to.
pub enum Look<P> {
    /// Something to hand the consumer.
    Tell(P),
    /// Nothing to hand on: nothing changed worth telling, or nothing was
    /// captured (the window could not be had).
    Quiet,
    /// Capture was refused, or is no longer running.
    Refused,
    /// The setting was turned off.
    Disabled,
}

/// One capture of the game window, stamped with the moment its pixels
/// were taken, on both clocks: the moment the frame was handed over, less
/// the age the frame gives for its pixels. A capture that got no frame is
/// stamped with the moment it was told so.
pub struct Capture {
    /// Epoch milliseconds on the injected clock.
    pub at_ms: i64,
    /// The injected clock's monotonic reading, in seconds.
    pub began: f64,
    /// How long the frame seam took, the lock's wait not included.
    pub took: Duration,
    /// How long the grab of the pixels alone took: how far the stamp can
    /// be from the moment the pixels show. Zero when there is no frame.
    pub age: Duration,
    pub frame: Result<GameFrame, FrameUnavailable>,
}

/// How one watcher's looks went over about a minute of one watch: counts
/// of looks by what became of them, and the longest of each stage. A
/// diagnostic record: nothing reads it to decide anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookMinute {
    pub session_id: String,
    /// Which watcher: its [`Watcher::RECORD`].
    pub watcher: &'static str,
    /// When the minute's first look began, epoch milliseconds on the
    /// injected clock.
    pub started_ms: i64,
    /// Looks that came due and were begun.
    pub looks: u32,
    /// Of those, skipped because the game was in the background.
    pub unfocused: u32,
    /// Looks the cadence dropped because the look before overran them.
    pub missed: u32,
    /// Looks whose capture got a frame.
    pub frames: u32,
    /// Looks given up because the pixels took too long to grab.
    pub late: u32,
    /// Looks that found what the watcher looks for (a locked plate, the
    /// tool bar).
    pub found: u32,
    /// Of those, looks whose name was tried and did not read.
    pub unread: u32,
    /// The longest look, start to finish, in milliseconds on the injected
    /// clock.
    pub max_look_ms: u32,
    /// The longest capture, finding the game window included, in
    /// milliseconds of real time.
    pub max_capture_ms: u32,
    /// The longest grab of the pixels alone, in milliseconds.
    pub max_grab_ms: u32,
}

/// How long a [`LookMinute`] runs before the next look opens another.
const MINUTE_MS: i64 = 60_000;

/// The most finished minutes kept for collection; past it the oldest go.
const MINUTES_KEPT: usize = 240;

/// What one look noted of itself, for its [`LookMinute`].
#[derive(Debug, Clone, Copy, Default)]
struct Notes {
    unfocused: bool,
    frame: bool,
    late: bool,
    found: bool,
    unread: bool,
    capture_ms: u32,
    grab_ms: u32,
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

impl LookMinute {
    fn opened(session_id: &str, watcher: &'static str, started_ms: i64) -> Self {
        Self {
            session_id: session_id.to_string(),
            watcher,
            started_ms,
            looks: 0,
            unfocused: 0,
            missed: 0,
            frames: 0,
            late: 0,
            found: 0,
            unread: 0,
            max_look_ms: 0,
            max_capture_ms: 0,
            max_grab_ms: 0,
        }
    }

    fn add(&mut self, notes: Notes, look_ms: u32, missed: u32) {
        self.looks += 1;
        self.unfocused += u32::from(notes.unfocused);
        self.missed += missed;
        self.frames += u32::from(notes.frame);
        self.late += u32::from(notes.late);
        self.found += u32::from(notes.found);
        self.unread += u32::from(notes.unread);
        self.max_look_ms = self.max_look_ms.max(look_ms);
        self.max_capture_ms = self.max_capture_ms.max(notes.capture_ms);
        self.max_grab_ms = self.max_grab_ms.max(notes.grab_ms);
    }
}

/// What one look sees with: the capture, the clock, and what the look is
/// for.
pub struct Eyes<'a> {
    frame: &'a GameWindowFrame,
    clock: &'a dyn Clock,
    session_id: &'a str,
    /// When the latest hint this look answers was made (monotonic).
    hinted_at: Option<f64>,
    /// Whether the watch is still the running one.
    running: &'a dyn Fn() -> bool,
    /// What the look notes of itself as it goes.
    notes: std::cell::Cell<Notes>,
}

impl Eyes<'_> {
    /// Capture `area` of the game window, under the lock no other capture
    /// is taken beside, stamped with the moment its pixels were taken (see
    /// [`Capture`]). `None` when the watch stopped while the capture
    /// waited for the lock: nothing is captured then.
    pub fn capture(&self, area: FrameArea) -> Option<Capture> {
        let _gate = capture_gate();
        if !(self.running)() {
            return None;
        }
        let started = Instant::now();
        let frame = (self.frame)(area);
        let took = started.elapsed();
        let age = frame.as_ref().map_or(Duration::ZERO, |frame| frame.age);
        self.note(|notes| {
            notes.frame = frame.is_ok();
            notes.capture_ms = millis(took);
            notes.grab_ms = millis(age);
        });
        Some(Capture {
            at_ms: clock_epoch_ms(self.clock) - age.as_millis() as i64,
            began: self.clock.monotonic() - age.as_secs_f64(),
            took,
            age,
            frame,
        })
    }

    fn note(&self, change: impl FnOnce(&mut Notes)) {
        let mut notes = self.notes.get();
        change(&mut notes);
        self.notes.set(notes);
    }

    /// The look gives its capture up: the pixels took too long to grab.
    pub fn mark_late(&self) {
        self.note(|notes| notes.late = true);
    }

    /// The look found what the watcher looks for.
    pub fn mark_found(&self) {
        self.note(|notes| notes.found = true);
    }

    /// The look tried the name of what it found and could not read it.
    pub fn mark_unread(&self) {
        self.note(|notes| notes.unread = true);
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock
    }

    /// The session the look is taken for.
    pub fn session_id(&self) -> &str {
        self.session_id
    }

    /// Whether a hint asked for this look.
    pub fn hinted(&self) -> bool {
        self.hinted_at.is_some()
    }

    /// When the latest hint this look answers was made, on the injected
    /// clock's monotonic reading.
    pub fn hinted_at(&self) -> Option<f64> {
        self.hinted_at
    }
}

/// What one watcher does at each look.
pub trait Watcher: Send + Sync + 'static {
    /// What the watcher's looks tell its consumer.
    type Pulse: Send + 'static;
    /// What a worker keeps from one look to the next, fresh for each watch.
    type Memory: Send + 'static;
    /// The worker thread's name.
    const THREAD: &'static str;
    /// The watcher, as the log names it.
    const WHAT: &'static str;
    /// The watcher, as its [`LookMinute`]s name it.
    const RECORD: &'static str;

    /// What the watcher reads with, when it can run here (the interface
    /// profile and the recogniser are loaded).
    fn reader(&self) -> Option<Reader>;

    /// How far apart the looks are, in seconds.
    fn interval(reader: &Reader) -> f64;

    /// How soon after a look began a hint may have the next taken, in
    /// seconds.
    fn hint_gap(reader: &Reader) -> f64 {
        Self::interval(reader)
    }

    /// The memory a watch starts with.
    fn memory(reader: &Reader) -> Self::Memory;

    /// One look, taken with the setting on and the game not known to be
    /// in the background.
    fn look(&self, reader: &Reader, memory: &mut Self::Memory, eyes: &Eyes) -> Look<Self::Pulse>;
}

/// The running watch.
struct Watching {
    watch: u64,
    session_id: String,
    /// Told if the watch ends by itself.
    ended: Option<WatchEnded>,
    /// When the latest hint waiting to be taken by the next look was made.
    hinted: Option<f64>,
    /// Wakes the worker for a hint; dropped with this state, it
    /// disconnects the worker's receiver, which wakes it to stop.
    wake: mpsc::Sender<()>,
}

struct State {
    watching: Option<Watching>,
    /// The last watch's number; each start takes the next.
    last_watch: u64,
    /// The desktop refused a capture, or capture stopped while watching:
    /// nothing is captured again until the player turns the setting back
    /// on.
    capture_refused: bool,
    /// The player turned the setting on: the next look may start capture.
    may_start: bool,
    /// The running watch's minute still being counted.
    minute: Option<LookMinute>,
    /// Finished minutes waiting to be collected, oldest first.
    minutes: std::collections::VecDeque<LookMinute>,
}

impl State {
    /// Finish the minute being counted: it waits to be collected.
    fn close_minute(&mut self) {
        if let Some(minute) = self.minute.take() {
            if self.minutes.len() == MINUTES_KEPT {
                self.minutes.pop_front();
            }
            self.minutes.push_back(minute);
        }
    }

    /// Stop watching, whoever asks: the watch's last minute is finished
    /// with it.
    fn end_watch(&mut self) -> Option<Watching> {
        self.close_minute();
        self.watching.take()
    }
}

/// Counts the live workers, so shutting down can wait for them.
#[derive(Default)]
struct Workers {
    live: Mutex<usize>,
    done: Condvar,
}

/// One live worker; dropping it, however the worker ends, counts it out.
struct WorkerToken<W: Watcher> {
    service: Arc<ScreenWatch<W>>,
}

impl<W: Watcher> Drop for WorkerToken<W> {
    fn drop(&mut self) {
        let workers = &self.service.workers;
        let mut live = workers.live.lock().unwrap_or_else(PoisonError::into_inner);
        *live = live.saturating_sub(1);
        workers.done.notify_all();
    }
}

/// A watcher of the game window, running `W`'s looks.
pub struct ScreenWatch<W: Watcher> {
    watcher: W,
    seams: Seams,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Held across each look, so a worker whose watch was just replaced
    /// never looks beside its successor.
    looking: Mutex<()>,
    workers: Workers,
}

impl<W: Watcher> ScreenWatch<W> {
    /// A watcher running `watcher`'s looks over `seams`.
    pub fn from_parts(watcher: W, seams: Seams, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            watcher,
            seams,
            clock,
            state: Mutex::new(State {
                watching: None,
                last_watch: 0,
                capture_refused: false,
                may_start: false,
                minute: None,
                minutes: std::collections::VecDeque::new(),
            }),
            looking: Mutex::new(()),
            workers: Workers::default(),
        })
    }

    /// What one look does.
    pub fn watcher(&self) -> &W {
        &self.watcher
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
        if self.watcher.reader().is_none() {
            return CaptureStatus::Unavailable;
        }
        let state = self.lock_state();
        if state.capture_refused {
            CaptureStatus::Refused
        } else if state.may_start || (self.seams.capture_running)() {
            CaptureStatus::Ready
        } else {
            CaptureStatus::NotRunning
        }
    }

    /// Start watching for `session_id`. A watch for the same session is
    /// left running; one for another session is replaced. Nothing starts
    /// without the profile and the recogniser, after capture was refused,
    /// or while capture is not running and the player has not just turned
    /// the setting on.
    pub fn start(self: &Arc<Self>, session_id: &str, sink: Sink<W::Pulse>) {
        self.begin(session_id, sink, None);
    }

    /// Start watching as [`ScreenWatch::start`] does, telling `ended` if
    /// the watch ends by itself.
    pub fn start_reporting_end(
        self: &Arc<Self>,
        session_id: &str,
        sink: Sink<W::Pulse>,
        ended: WatchEnded,
    ) {
        self.begin(session_id, sink, Some(ended));
    }

    fn begin(self: &Arc<Self>, session_id: &str, sink: Sink<W::Pulse>, ended: Option<WatchEnded>) {
        let Some(reader) = self.watcher.reader() else {
            return;
        };
        let mut state = self.lock_state();
        if state.capture_refused || !(state.may_start || (self.seams.capture_running)()) {
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
        state.end_watch();
        let watch = state.last_watch + 1;
        let (wake, woken) = mpsc::channel();
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
            .name(W::THREAD.into())
            .spawn(move || worker.watch(watch, &session, &reader, &sink, &woken, token));
        // A worker that did not start dropped its token with the closure,
        // counting itself out; nothing is watched.
        if spawned.is_err() {
            tracing::warn!(target: "eo::screen_watch", "{} could not start", W::WHAT);
            return;
        }
        state.last_watch = watch;
        state.watching = Some(Watching {
            watch,
            session_id: session_id.to_string(),
            ended,
            hinted: None,
            wake,
        });
    }

    /// Stop watching. Returns at once: the worker ends within its look.
    pub fn stop(&self) {
        self.lock_state().end_watch();
    }

    /// The finished minutes of how the looks went, oldest first, handed
    /// over once: of the running watch up to its last full minute, and of
    /// every watch that ended since they were last taken.
    pub fn take_look_minutes(&self) -> Vec<LookMinute> {
        self.lock_state().minutes.drain(..).collect()
    }

    /// Count one look of `watch` into its minute, opening the next minute
    /// when this one has run its length. A look of a watch no longer
    /// running is not counted.
    fn tally(&self, watch: u64, began_ms: i64, notes: Notes, look_ms: u32, missed: u32) {
        let mut state = self.lock_state();
        let Some(session_id) = state
            .watching
            .as_ref()
            .filter(|w| w.watch == watch)
            .map(|w| w.session_id.clone())
        else {
            return;
        };
        if state
            .minute
            .as_ref()
            .is_some_and(|minute| began_ms - minute.started_ms >= MINUTE_MS)
        {
            state.close_minute();
        }
        state
            .minute
            .get_or_insert_with(|| LookMinute::opened(&session_id, W::RECORD, began_ms))
            .add(notes, look_ms, missed);
    }

    /// Ask for a look as soon as the watcher allows, instead of at the
    /// next interval. Returns at once, from any thread. Hints made before
    /// the look they ask for are one hint; a hint while nothing is watched
    /// does nothing, and never starts capture.
    pub fn hint(&self) {
        let now = self.clock.monotonic();
        let mut state = self.lock_state();
        if let Some(watching) = state.watching.as_mut() {
            if watching.hinted.replace(now).is_none() {
                let _ = watching.wake.send(());
            }
        }
    }

    /// The player turned the setting on: the next look may start capture,
    /// asking the desktop's consent once, even where it was refused before.
    pub fn permit_capture(&self) {
        let mut state = self.lock_state();
        state.capture_refused = false;
        state.may_start = true;
    }

    /// Whether this look may start capture: the player turned the setting
    /// on and no look has asked since. Taken once.
    fn take_start(&self) -> bool {
        std::mem::take(&mut self.lock_state().may_start)
    }

    /// When the latest hint waiting for `watch`'s next look was made, if
    /// one is. Taken once, with the wake-ups it sent, under the lock the
    /// hint is made under, so a hint made after this is never lost.
    fn take_hint(&self, watch: u64, woken: &mpsc::Receiver<()>) -> Option<f64> {
        let mut state = self.lock_state();
        while woken.try_recv().is_ok() {}
        state
            .watching
            .as_mut()
            .filter(|w| w.watch == watch)
            .and_then(|w| w.hinted.take())
    }

    /// Stop watching and wait, for a short while, for the worker to finish
    /// its look: at app exit, where nothing may go on capturing behind the
    /// closing app. A worker still out after the wait is left to end with
    /// the process.
    pub fn shutdown(&self) {
        self.stop();
        if !self.wait_for_workers(SHUTDOWN_WAIT) {
            tracing::warn!(
                target: "eo::screen_watch",
                "{} was still capturing at exit; not waiting for it",
                W::WHAT
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
        let ended = state.end_watch()?;
        ended.ended.map(|tell| (tell, ended.session_id))
    }

    /// Tell, outside the state's lock, that a watch ended by itself.
    fn tell_ended(ended: Option<(WatchEnded, String)>) {
        if let Some((tell, session_id)) = ended {
            tell(&session_id);
        }
    }

    fn wait(&self, due: f64, woken: &mpsc::Receiver<()>) -> bool {
        match &self.seams.wait {
            Some(wait) => wait(due, woken),
            None => wait_on_clock(self.clock.as_ref(), due, woken),
        }
    }

    /// The worker: wait for the next look, look, hand the look on, until
    /// the watch is no longer the running one.
    fn watch(
        &self,
        watch: u64,
        session_id: &str,
        reader: &Reader,
        sink: &Sink<W::Pulse>,
        woken: &mpsc::Receiver<()>,
        _token: WorkerToken<W>,
    ) {
        let interval = W::interval(reader);
        let gap = W::hint_gap(reader);
        let mut memory = W::memory(reader);
        // When the last look began.
        let mut last: Option<f64> = None;
        // The first look is taken at once.
        let mut due = self.clock.monotonic();
        loop {
            if !self.wait(due, woken) {
                return;
            }
            let mut hinted_at = self.take_hint(watch, woken);
            // A hint is honoured no sooner than the watcher allows after
            // the look before; hints made meanwhile wake the wait, and are
            // taken with it, but never bring the look forward.
            if let Some(soonest) = last.map(|last| last + gap) {
                while hinted_at.is_some() && self.clock.monotonic() < soonest {
                    if !self.wait(soonest, woken) {
                        return;
                    }
                    hinted_at = self.take_hint(watch, woken).or(hinted_at);
                }
            }
            let hinted = hinted_at.is_some();
            let began = self.clock.monotonic();
            let began_ms = clock_epoch_ms(self.clock.as_ref());
            let (look, notes) = {
                let _looking = self.looking.lock().unwrap_or_else(PoisonError::into_inner);
                // Checked again after waiting for another worker's look.
                if !self.is_watching(watch) {
                    return;
                }
                if !(self.seams.enabled)() {
                    self.failed(watch);
                    return;
                }
                if self.take_start() {
                    match self.start_capture(watch) {
                        Some(true) => {}
                        Some(false) => {
                            self.refused(watch);
                            return;
                        }
                        None => return,
                    }
                }
                let running = || self.is_watching(watch);
                let eyes = Eyes {
                    frame: &self.seams.frame,
                    clock: self.clock.as_ref(),
                    session_id,
                    hinted_at,
                    running: &running,
                    notes: std::cell::Cell::default(),
                };
                let look = catch_unwind(AssertUnwindSafe(|| self.look(reader, &mut memory, &eyes)));
                (look, eyes.notes.get())
            };
            let look_ms = millis(Duration::from_secs_f64(
                (self.clock.monotonic() - began).max(0.0),
            ));
            last = Some(began);
            // A hinted look starts the cadence again from itself.
            let from = if hinted { began } else { due };
            due = next_due(from, interval, self.clock.monotonic());
            // Every interval between this look's place on the cadence and
            // the next, past the first, is a look the cadence dropped.
            let missed = (((due - from) / interval).round() as u32).saturating_sub(1);
            // Counted before the look is handed on, so a watch its own
            // consumer stops on this look has counted it.
            self.tally(watch, began_ms, notes, look_ms, missed);
            match look {
                Ok(Look::Tell(pulse)) => {
                    // A look finishing after its watch ended is dropped.
                    if !self.is_watching(watch) {
                        return;
                    }
                    if !sink(pulse) {
                        self.failed(watch);
                        return;
                    }
                }
                Ok(Look::Quiet) => {}
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
                        target: "eo::screen_watch",
                        "a look at the game window failed; {} stops",
                        W::WHAT
                    );
                    self.failed(watch);
                    return;
                }
            }
        }
    }

    /// Start capture for the player's gesture, under the capture lock: no
    /// other watcher's capture runs beside the consent question (see the
    /// module's notes). Whether it started; `None` when the watch stopped
    /// while this waited for the lock, and nothing was asked.
    fn start_capture(&self, watch: u64) -> Option<bool> {
        let _gate = capture_gate();
        if !self.is_watching(watch) {
            return None;
        }
        Some((self.seams.start_capture)())
    }

    /// One look, unless the setting is off or the game is known to be in
    /// the background.
    fn look(&self, reader: &Reader, memory: &mut W::Memory, eyes: &Eyes) -> Look<W::Pulse> {
        if !(self.seams.enabled)() {
            return Look::Disabled;
        }
        if self
            .seams
            .focus
            .as_ref()
            .is_some_and(|focus| !GameFocus::game_in_view(focus()))
        {
            eyes.note(|notes| notes.unfocused = true);
            return Look::Quiet;
        }
        self.watcher.look(reader, memory, eyes)
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
/// until a hint asks for a look now, or until told to stop.
fn wait_on_clock(clock: &dyn Clock, due: f64, woken: &mpsc::Receiver<()>) -> bool {
    loop {
        let left = due - clock.monotonic();
        if left <= 0.0 {
            return !matches!(woken.try_recv(), Err(TryRecvError::Disconnected));
        }
        match woken.recv_timeout(Duration::from_secs_f64(left)) {
            Err(RecvTimeoutError::Timeout) => {}
            // A hint: the worker takes it before looking.
            Ok(()) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

#[cfg(test)]
mod tests;
