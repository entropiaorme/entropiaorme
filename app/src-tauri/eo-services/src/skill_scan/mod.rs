//! The skill scan: the player's skill points, read from the game's own
//! skills window.
//!
//! The player opens the scan's window, starts a scan, and pages through
//! the SKILLS tab in the game. While the scan is collecting, one worker
//! looks at the game window every tick, reads the page showing, and
//! offers it to a sweep that keeps each page once it is whole; the
//! player presses nothing per page. When every page is in, the sweep's
//! result is held for review and the worker stops. Accepting the review
//! persists it through the completion callback; rejecting it discards it.
//!
//! Every scan has its own number, its own sweep and its own worker. The
//! sweep lives in the worker, so nothing one scan read can reach another,
//! and a worker writes back only while its scan is still the one
//! collecting: one left reading when its scan ends drops what it read and
//! exits. Looks are taken one at a time across workers, so a scan started
//! just after another ended never reads beside it. Nothing is captured
//! unless a scan is collecting, and collecting stops when the scan's
//! window closes, when capture is not allowed, and when a while passes
//! with no new page.
//!
//! The phases are a typed lifecycle: idle, collecting, awaiting review,
//! and saving. While a review is being saved, every other verb is refused
//! at once, so no command waits on the save and nothing can discard a
//! review that is being written.
//!
//! A `scan.status.changed` envelope publishes on the bus whenever the
//! status moves, coalesced on the status itself so each change emits
//! exactly one frame.

mod hint;
mod review;
#[cfg(test)]
mod tests;

pub use hint::ScanHint;
pub use review::{GuessedSkill, NearMissSkill, SkillScanReview, TotalCheck, UnsavedSkill};

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::bus_events::BusEvent;
use crate::clock::Clock;
use crate::event_bus::EventBus;
use crate::game_ui::geometry::BgrImage;
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameReader, FrameUnavailable, GameWindowFrame};
use crate::game_ui::skills::{
    read_skills_page, Admission, SkillVocabulary, SkillsSweep, SkillsTableProfile, SweepResult,
};
use crate::scan_completion::ScannedLevel;
use crate::time::{naive_to_epoch, to_iso_utc};
use eo_wire::domain_events::{
    ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
};

use hint::{frame_hint, PageShortfall, SteadyHint};

/// The interface profile's key for the skills window.
const SKILLS_WINDOW: &str = "skills";

/// How often a collecting scan looks at the game window: about twice a
/// second, quick enough that a page flipped to is taken before the
/// player looks back, and slow enough to leave the game its machine.
pub const DEFAULT_TICK: Duration = Duration::from_millis(500);

/// How long a collecting scan goes on with no new page before it stops:
/// long enough to page through a long list slowly, short enough that a
/// scan left open is not capturing the screen for an evening.
pub const DEFAULT_IDLE_LIMIT: Duration = Duration::from_secs(3 * 60);

/// How long shutting down waits for a worker to finish its look. A worker
/// can be held inside a capture for as long as the desktop's consent
/// prompt is open; app exit does not wait on the player for that, and a
/// worker still out when this passes is left to end with the process.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// A completion failure: whatever error the wired persist path raises;
/// its rendered text is what the player is shown.
pub type CompletionError = Box<dyn std::error::Error + Send + Sync>;

/// The completion callback: persists an accepted scan's levels, each with
/// the moment its page was read.
pub type CompletionCallback =
    Arc<dyn Fn(&[ScannedLevel]) -> Result<(), CompletionError> + Send + Sync>;

/// The seams and settings the composition root wires in.
pub struct SkillScanProviders {
    /// The whole game window.
    pub frame: GameWindowFrame,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The recogniser; `None` when none was loaded. It may still fail on
    /// a particular crop, which is that crop not reading.
    pub read_text: Option<FrameReader>,
    /// The skills and categories a page is checked against.
    pub vocabulary: SkillVocabulary,
    /// How long a collecting scan waits between looks at the game window.
    pub tick: Duration,
    /// How long a collecting scan goes on with no new page before it stops.
    pub idle_limit: Duration,
}

impl Default for SkillScanProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|| Err(FrameUnavailable::WindowNotFound)),
            profile: None,
            read_text: None,
            vocabulary: SkillVocabulary::default(),
            tick: DEFAULT_TICK,
            idle_limit: DEFAULT_IDLE_LIMIT,
        }
    }
}

/// How far a collecting scan has come.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanProgress {
    /// Pages collected.
    pub collected: u32,
    /// How many pages the listing has, once a page has said.
    pub page_count: Option<u32>,
    /// The page numbers still to collect, once the count is known.
    pub missing: Vec<u32>,
    /// Why the game window, as it has steadily looked, gives nothing to
    /// collect.
    pub hint: Option<ScanHint>,
}

/// Why a scan stopped collecting by itself, with no review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStop {
    /// Reading the skills window failed.
    ReadFailed,
    /// Screen capture was not allowed to start. The scan does not ask
    /// again by itself; starting it again asks once more.
    CaptureNotAllowed,
    /// The idle limit passed with no new page collected.
    NoNewPage,
}

/// The scan's state, as the scan surfaces show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanStatus {
    /// No scan is collecting or awaiting review.
    Idle {
        /// A scan can run: the recogniser, the interface profile and the
        /// skill list are all loaded.
        available: bool,
        /// Why the last scan stopped by itself, when it did.
        stopped: Option<ScanStop>,
    },
    Collecting(ScanProgress),
    /// Every page is in; the result waits for the player's review.
    AwaitingReview,
    /// The accepted review is being saved.
    Saving,
}

impl ScanStatus {
    fn phase(&self) -> ScanPhase {
        match self {
            ScanStatus::Idle { .. } => ScanPhase::Idle,
            ScanStatus::Collecting(_) => ScanPhase::Capturing,
            ScanStatus::AwaitingReview | ScanStatus::Saving => ScanPhase::AwaitingReview,
        }
    }
}

/// Why a verb was refused. A refused verb changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanRefusal {
    #[error("No pending result")]
    NoReview,
    /// The review named is not the one held: the one on screen is out of
    /// date.
    #[error("The review shown is no longer the one held")]
    StaleReview,
    /// The figures do not add up to the game's total, or could not be
    /// checked against it, and saving them anyway was not asked for.
    #[error("The figures do not add up to the game's total")]
    TotalNotConfirmed,
    #[error("The scan's results are being saved")]
    Saving,
    /// The scan's window is not open, so there is nothing to collect for.
    #[error("The scan window is not open")]
    SurfaceClosed,
    /// The worker could not be started.
    #[error("The scan could not start")]
    WorkerUnavailable,
    /// The completion callback failed; the review stays open for a retry.
    #[error("Persist failed: {0}")]
    SaveFailed(String),
}

enum Lifecycle {
    Idle {
        stopped: Option<ScanStop>,
    },
    Collecting {
        scan: u64,
        progress: ScanProgress,
        /// Held only to be dropped: leaving this state disconnects the
        /// worker's receiver, which wakes it from its wait to stop.
        _stop: mpsc::Sender<()>,
    },
    AwaitingReview {
        review: SkillScanReview,
    },
    /// The review is with the accepting caller, being saved.
    Saving,
}

struct State {
    lifecycle: Lifecycle,
    /// The last scan's number; each start takes the next.
    last_scan: u64,
    /// Whether the scan's window is showing. Collecting happens only for
    /// a showing window: closing it stops collecting, and a start while
    /// it is closed is refused, both under this one lock.
    surface_open: bool,
    last_emitted: ScanStatus,
}

/// What one look at the game window came to.
enum Outcome {
    Seen(Option<ScanHint>),
    Finished(SweepResult),
    Stop(ScanStop),
}

/// What a worker needs to read a page, taken once when its scan starts.
struct Reader {
    profile: Arc<UiProfile>,
    table: SkillsTableProfile,
    read_text: FrameReader,
}

/// What one worker keeps across its looks. Its own, never shared.
struct Looking {
    sweep: SkillsSweep,
    /// When each page held was read, in epoch seconds.
    read_at: BTreeMap<u32, f64>,
    steady: SteadyHint,
    collected: u32,
    /// When the last new page was collected (the start, before any).
    last_new_page: f64,
}

/// Counts the live workers, so shutting down can wait for them.
#[derive(Default)]
struct Workers {
    live: Mutex<usize>,
    done: Condvar,
}

/// One live worker; dropping it, however the worker ends, counts it out.
struct WorkerToken {
    service: Arc<SkillScanService>,
}

impl Drop for WorkerToken {
    fn drop(&mut self) {
        let workers = &self.service.workers;
        let mut live = workers.live.lock().unwrap_or_else(PoisonError::into_inner);
        *live = live.saturating_sub(1);
        workers.done.notify_all();
    }
}

pub struct SkillScanService {
    providers: SkillScanProviders,
    clock: Arc<dyn Clock>,
    bus: Option<Arc<EventBus>>,
    state: Mutex<State>,
    /// Held across each look, so two workers never capture or recognise
    /// at once.
    reading: Mutex<()>,
    workers: Workers,
    on_complete: Mutex<Option<CompletionCallback>>,
}

impl SkillScanService {
    pub fn new(
        providers: SkillScanProviders,
        clock: Arc<dyn Clock>,
        bus: Option<Arc<EventBus>>,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            providers,
            clock,
            bus,
            state: Mutex::new(State {
                lifecycle: Lifecycle::Idle { stopped: None },
                last_scan: 0,
                surface_open: false,
                last_emitted: ScanStatus::Saving,
            }),
            reading: Mutex::new(()),
            workers: Workers::default(),
            on_complete: Mutex::new(None),
        });
        // Baselined at the resting status, so the first genuine change
        // emits but nothing announces the resting state itself (the
        // surfaces read it on mount).
        let resting = service.status();
        service.lock_state().last_emitted = resting;
        service
    }

    pub fn set_completion_callback(&self, callback: CompletionCallback) {
        *self
            .on_complete
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(callback);
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a scan reads with, when everything it needs is loaded.
    fn reader(&self) -> Option<Reader> {
        let profile = self.providers.profile.clone()?;
        let table = profile.windows.get(SKILLS_WINDOW)?.skills_table.clone()?;
        let read_text = self.providers.read_text.clone()?;
        (!self.providers.vocabulary.names().is_empty()).then_some(Reader {
            profile,
            table,
            read_text,
        })
    }

    /// Whether a scan can run: everything it reads with is loaded.
    fn available(&self) -> bool {
        self.providers.read_text.is_some()
            && !self.providers.vocabulary.names().is_empty()
            && self.providers.profile.as_ref().is_some_and(|profile| {
                profile
                    .windows
                    .get(SKILLS_WINDOW)
                    .is_some_and(|window| window.skills_table.is_some())
            })
    }

    pub fn status(&self) -> ScanStatus {
        let state = self.lock_state();
        self.status_of(&state)
    }

    fn status_of(&self, state: &State) -> ScanStatus {
        match &state.lifecycle {
            Lifecycle::Idle { stopped } => ScanStatus::Idle {
                available: self.available(),
                stopped: *stopped,
            },
            Lifecycle::Collecting { progress, .. } => ScanStatus::Collecting(progress.clone()),
            Lifecycle::AwaitingReview { .. } => ScanStatus::AwaitingReview,
            Lifecycle::Saving => ScanStatus::Saving,
        }
    }

    /// Publish a `scan.status.changed` envelope iff the status moved.
    /// Called after releasing the guard at every settled change; the
    /// compare-and-advance happens under the lock, so two threads cannot
    /// both emit the same transition.
    fn publish_status(&self) {
        let Some(bus) = &self.bus else {
            return;
        };
        let phase = {
            let mut state = self.lock_state();
            let status = self.status_of(&state);
            if status == state.last_emitted {
                return;
            }
            let phase = status.phase();
            state.last_emitted = status;
            phase
        };
        bus.publish(&BusEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: to_iso_utc(naive_to_epoch(self.clock.now())),
            payload: ScanStatusChangedPayload { phase },
        }));
    }

    /// The scan's window is showing: a scan may now be started from it.
    pub fn surface_shown(&self) {
        self.lock_state().surface_open = true;
    }

    /// The scan's window has closed or been hidden: collecting stops with
    /// it, under the same lock a start checks, so no start can slip in
    /// between and collect for a window no one can see. A review is kept,
    /// since it is shown in the main window.
    pub fn surface_closed(&self) -> ScanStatus {
        {
            let mut state = self.lock_state();
            state.surface_open = false;
            if matches!(state.lifecycle, Lifecycle::Collecting { .. }) {
                state.lifecycle = Lifecycle::Idle { stopped: None };
            }
        }
        self.publish_status();
        self.status()
    }

    /// Start collecting. A scan already collecting or awaiting review is
    /// left as it is, and nothing starts when the reader is not loaded;
    /// either way the answer is the status that results. Refused while a
    /// review is being saved, while the scan's window is closed, and when
    /// no worker can be started.
    pub fn start(self: &Arc<Self>) -> Result<ScanStatus, ScanRefusal> {
        let Some(reader) = self.reader() else {
            return Ok(self.status());
        };
        let status = {
            let mut state = self.lock_state();
            match state.lifecycle {
                Lifecycle::Saving => return Err(ScanRefusal::Saving),
                Lifecycle::Collecting { .. } | Lifecycle::AwaitingReview { .. } => {
                    return Ok(self.status_of(&state));
                }
                Lifecycle::Idle { .. } if !state.surface_open => {
                    return Err(ScanRefusal::SurfaceClosed);
                }
                Lifecycle::Idle { .. } => {}
            }
            let scan = state.last_scan + 1;
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
            let spawned = std::thread::Builder::new()
                .name("skill-scan".into())
                .spawn(move || worker.watch(scan, &reader, &stopped, token));
            // A worker that did not start dropped its token with the
            // closure, counting itself out; the scan stays idle.
            if spawned.is_err() {
                return Err(ScanRefusal::WorkerUnavailable);
            }
            state.last_scan = scan;
            state.lifecycle = Lifecycle::Collecting {
                scan,
                progress: ScanProgress::default(),
                _stop: stop,
            };
            self.status_of(&state)
        };
        self.publish_status();
        Ok(status)
    }

    /// Stop collecting. Nothing else is cancelled: a review is settled by
    /// accepting or rejecting it. Refused while a review is being saved.
    pub fn cancel(&self) -> Result<ScanStatus, ScanRefusal> {
        {
            let mut state = self.lock_state();
            match state.lifecycle {
                Lifecycle::Saving => return Err(ScanRefusal::Saving),
                Lifecycle::Collecting { .. } => {
                    state.lifecycle = Lifecycle::Idle { stopped: None };
                }
                Lifecycle::Idle { .. } | Lifecycle::AwaitingReview { .. } => {}
            }
        }
        self.publish_status();
        Ok(self.status())
    }

    /// The finished scan awaiting review.
    pub fn pending(&self) -> Option<SkillScanReview> {
        match &self.lock_state().lifecycle {
            Lifecycle::AwaitingReview { review } => Some(review.clone()),
            _ => None,
        }
    }

    /// Save the review of scan `scan` through the completion callback,
    /// and answer how many skills it saved. A review whose figures do not
    /// add up to the game's total, or could not be checked against it, is
    /// saved only with `save_anyway`. While the save runs the scan is
    /// saving, and every other verb is refused at once; a failed save
    /// puts the review back for a retry.
    pub fn accept(&self, scan: u64, save_anyway: bool) -> Result<usize, ScanRefusal> {
        let review = {
            let mut state = self.lock_state();
            let held = std::mem::replace(&mut state.lifecycle, Lifecycle::Saving);
            match held {
                Lifecycle::AwaitingReview { review }
                    if review.scan == scan && (save_anyway || !review.needs_confirmation()) =>
                {
                    review
                }
                other => {
                    let refusal = match &other {
                        Lifecycle::Saving => ScanRefusal::Saving,
                        Lifecycle::AwaitingReview { review } if review.scan != scan => {
                            ScanRefusal::StaleReview
                        }
                        Lifecycle::AwaitingReview { .. } => ScanRefusal::TotalNotConfirmed,
                        Lifecycle::Idle { .. } | Lifecycle::Collecting { .. } => {
                            ScanRefusal::NoReview
                        }
                    };
                    state.lifecycle = other;
                    return Err(refusal);
                }
            }
        };
        self.publish_status();
        let callback = self
            .on_complete
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let saved = match callback {
            Some(callback) => catch_unwind(AssertUnwindSafe(|| callback(&review.skills)))
                .unwrap_or_else(|_| Err("the save stopped unexpectedly".into())),
            None => Ok(()),
        };
        let answer = {
            let mut state = self.lock_state();
            match saved {
                Ok(()) => {
                    let persisted = review.skills.len();
                    state.lifecycle = Lifecycle::Idle { stopped: None };
                    Ok(persisted)
                }
                Err(error) => {
                    state.lifecycle = Lifecycle::AwaitingReview { review };
                    Err(ScanRefusal::SaveFailed(error.to_string()))
                }
            }
        };
        self.publish_status();
        answer
    }

    /// Discard the review of scan `scan`.
    pub fn reject(&self, scan: u64) -> Result<(), ScanRefusal> {
        {
            let mut state = self.lock_state();
            match &state.lifecycle {
                Lifecycle::Saving => return Err(ScanRefusal::Saving),
                Lifecycle::AwaitingReview { review } if review.scan == scan => {
                    state.lifecycle = Lifecycle::Idle { stopped: None };
                }
                Lifecycle::AwaitingReview { .. } => return Err(ScanRefusal::StaleReview),
                Lifecycle::Idle { .. } | Lifecycle::Collecting { .. } => {
                    return Err(ScanRefusal::NoReview)
                }
            }
        }
        self.publish_status();
        Ok(())
    }

    /// Stop collecting and wait, for a short while, for every worker to
    /// finish its look: at app exit, where nothing may go on capturing
    /// behind the closing app. The wait is bounded because a worker can be
    /// held inside a capture by the desktop's consent prompt; one still out
    /// after [`SHUTDOWN_WAIT`] is left to end with the process.
    pub fn shutdown(&self) {
        self.surface_closed();
        if !self.wait_for_workers(SHUTDOWN_WAIT) {
            tracing::warn!(
                target: "eo::skill_scan",
                "a skill scan worker was still capturing at exit; not waiting for it"
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

    fn is_collecting(&self, scan: u64) -> bool {
        matches!(
            self.lock_state().lifecycle,
            Lifecycle::Collecting { scan: current, .. } if current == scan
        )
    }

    /// The worker: look, write back, wait a tick, until the scan is no
    /// longer collecting. Whatever goes wrong in a look or its write-back
    /// ends the scan, never leaves it showing as collecting.
    fn watch(&self, scan: u64, reader: &Reader, stopped: &mpsc::Receiver<()>, _token: WorkerToken) {
        let mut looking = Looking {
            sweep: SkillsSweep::new(&reader.table, &self.providers.vocabulary),
            read_at: BTreeMap::new(),
            steady: SteadyHint::default(),
            collected: 0,
            last_new_page: naive_to_epoch(self.clock.now()),
        };
        loop {
            let step = catch_unwind(AssertUnwindSafe(|| self.step(scan, reader, &mut looking)));
            let carry_on = step.unwrap_or_else(|_| {
                tracing::error!(
                    target: "eo::skill_scan",
                    "reading the skills window failed; the scan stops"
                );
                self.end(scan, ScanStop::ReadFailed);
                false
            });
            if !carry_on {
                return;
            }
            // Leaving the collecting state drops the sender, which ends
            // the wait at once.
            match stopped.recv_timeout(self.providers.tick) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// One look and its write-back; answers whether to keep watching.
    fn step(&self, scan: u64, reader: &Reader, looking: &mut Looking) -> bool {
        let outcome = {
            let _reading = self.reading.lock().unwrap_or_else(PoisonError::into_inner);
            // A capture is taken only for the scan that is collecting,
            // checked again after waiting for another worker's look.
            if !self.is_collecting(scan) {
                return false;
            }
            self.look(reader, looking)
        };
        self.write_back(scan, looking, outcome)
    }

    /// Capture the game window, read the page showing, and offer it.
    fn look(&self, reader: &Reader, looking: &mut Looking) -> Outcome {
        let frame = match (self.providers.frame)() {
            Ok(frame) => frame,
            Err(unavailable) => {
                return match frame_hint(unavailable) {
                    Ok(hint) => Outcome::Seen(Some(hint)),
                    Err(stop) => Outcome::Stop(stop),
                }
            }
        };
        // The levels on this frame are as of the moment it was captured.
        let read_at = naive_to_epoch(self.clock.now());
        // A crop the recogniser fails on reads as nothing, which no
        // grammar or vocabulary accepts.
        let read_text = &reader.read_text;
        let recogniser =
            |image: &BgrImage| Some(read_text(image).unwrap_or_else(|| (String::new(), 0.0)));
        let page = match read_skills_page(
            &frame,
            &reader.profile,
            &self.providers.vocabulary,
            &recogniser,
        ) {
            Ok(page) => page,
            Err(error) => return Outcome::Seen(Some(error.into())),
        };
        let index = page.page.map(|(index, _)| index);
        let shortfall = PageShortfall::of(&page);
        let admission = looking.sweep.accept(page);
        if let (Admission::Kept, Some(index)) = (admission, index) {
            // The sweep now holds this reading of the page: its levels are
            // as of this capture.
            looking.read_at.insert(index, read_at);
        }
        let hint = shortfall.hint(admission);
        match looking.sweep.finish() {
            Some(result) => Outcome::Finished(result),
            None => Outcome::Seen(hint),
        }
    }

    /// Record what a look came to, if `scan` is still collecting; answer
    /// whether to keep watching.
    fn write_back(&self, scan: u64, looking: &mut Looking, outcome: Outcome) -> bool {
        let progress = match outcome {
            Outcome::Stop(stop) => {
                self.end(scan, stop);
                return false;
            }
            Outcome::Finished(result) => {
                let review = SkillScanReview::from_sweep(scan, &result, &looking.read_at);
                {
                    let mut state = self.lock_state();
                    if matches!(state.lifecycle, Lifecycle::Collecting { scan: current, .. } if current == scan)
                    {
                        state.lifecycle = Lifecycle::AwaitingReview { review };
                    }
                }
                self.publish_status();
                return false;
            }
            Outcome::Seen(hint) => {
                let page_count = looking.sweep.page_count();
                let missing = looking.sweep.missing().unwrap_or_default();
                let collected = page_count.map_or(0, |count| count - missing.len() as u32);
                let now = naive_to_epoch(self.clock.now());
                if collected > looking.collected {
                    looking.collected = collected;
                    looking.last_new_page = now;
                } else if now - looking.last_new_page >= self.providers.idle_limit.as_secs_f64() {
                    self.end(scan, ScanStop::NoNewPage);
                    return false;
                }
                ScanProgress {
                    collected,
                    page_count,
                    missing,
                    hint: looking.steady.observe(hint),
                }
            }
        };
        {
            let mut state = self.lock_state();
            match &mut state.lifecycle {
                Lifecycle::Collecting {
                    scan: current,
                    progress: held,
                    ..
                } if *current == scan => *held = progress,
                _ => return false,
            }
        }
        self.publish_status();
        true
    }

    /// End `scan` because of `stop`, if it is still collecting.
    fn end(&self, scan: u64, stop: ScanStop) {
        {
            let mut state = self.lock_state();
            if matches!(state.lifecycle, Lifecycle::Collecting { scan: current, .. } if current == scan)
            {
                state.lifecycle = Lifecycle::Idle {
                    stopped: Some(stop),
                };
            }
        }
        self.publish_status();
    }
}
