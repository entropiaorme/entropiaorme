//! The held-tool watcher: while a session is watched, it looks at the bar
//! above the hotbar that names the tool in hand (see
//! [`crate::game_ui::tool_bar`]) several times a second, and tells its
//! consumer what is in hand whenever that changes.
//!
//! The bar is first looked for in a capture of the whole window. Once
//! found, only the area round the health bar and the tool bar is captured,
//! and the bar is looked for there; when it is not there (the interface
//! was moved, or hidden), its place is forgotten and the whole window is
//! searched again, no more often than the profile allows, since a capture
//! of the whole window costs many times one of the bar's area. A window
//! over the bar may show too little of itself in the bar's area to be
//! told as a window: when that area shows no bar, a health bar alone, or
//! an outline broken where no window was found, the look says nothing and
//! the next captures the whole window to decide (at once the first time,
//! then no more often than the search interval while the doubt lasts).
//!
//! The name is read only when it is worth reading: when nothing is locked
//! yet, when the bar's lettering differs from the lettering locked at the
//! last read by more than one name differs from itself, when the profile's
//! interval has passed since the last read (the comparison cannot see
//! every change of one character, so the interval bounds how long such a
//! change goes unread), at every look for a while after a hint (a key
//! pressed in the game is usually seen before the client has drawn the
//! new name), when the last read was refused, or when the tools the name
//! is matched against were replaced. Otherwise a look ends with the
//! comparison and tells nothing new. Nothing is accepted on the
//! recogniser's confidence: a name is matched to a tool by
//! [`crate::game_ui::tool_names::resolve`].
//!
//! A change to a tool held, a name known not to be a tracked tool, or a
//! bar holding nothing is told at once. Anything else (an uncertain or
//! unseen bar, or a capture that came too late or failed while the game
//! window was there) is told only once such looks have run on for the
//! profile's settle time, so one unclear frame does not blank the tool in
//! hand, and is then told with the time of the first look of the run. A
//! run mixing an unseen bar with an uncertain one is told as uncertain:
//! an unseen bar keeps the tool in hand, which mixed evidence does not
//! support. What each look costs (the
//! capture, the search for the bar with the comparison of its lettering,
//! and the reading and matching of the name) is recorded in the
//! application's latency metrics.
//!
//! How the looks are run (the worker and its life, the cadence, hints,
//! focus, consent and the lock no two captures are taken beside) is the
//! shared [`crate::screen_watch`]; this module is what one look does.

use std::sync::Arc;
use std::time::Instant;

use crate::clock::Clock;
use crate::game_ui::geometry::{BgrImage, Rect};
use crate::game_ui::profile::UiProfile;
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameWindowFrame};
use crate::game_ui::tool_bar::{
    bar_state, find_tool_bar_in, lettering, read_tool_name, BarState, Lettering, View,
};
use crate::game_ui::tool_names::{resolve, Candidate, ToolCatalogue, ToolMatch};
use crate::hotbar_listener::GameFocusProbe;
use crate::screen_watch::{
    Eyes, LateBound, Look, Probe, PulseWait, Reader, ScreenWatch, Seams, Sink, SwapCell,
    WatchEnded, Watcher,
};

/// A tool held, as the bar names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldTool {
    /// An entry of the player's equipment library.
    Library { id: i64 },
    /// A tool of the bundled game data the library does not hold: known by
    /// its collection, its id there and its name.
    Bundled {
        endpoint: String,
        item_id: String,
        name: String,
    },
}

/// Why the bar does not say which tool is in hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// Enabled library entries the name read cannot tell apart. Sorted.
    Twins { ids: Vec<i64> },
    /// Library entries the name is, all of them disabled. Sorted.
    Disabled { ids: Vec<i64> },
    /// Tools the name read cannot tell apart, nearest first, and how many
    /// more there are.
    Contended {
        candidates: Vec<Candidate>,
        more: usize,
    },
    /// A name read that is near no tool well enough: the text as read.
    Unknown { text: String },
    /// The bar could not be read: its outline is broken, its name area
    /// holds marks that are not the client's lettering, a health bar shows
    /// with no tool panel beside it, or the name was refused.
    Unreadable,
}

/// What the bar shows is in hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSeen {
    /// The bar names a tool, and it is this one.
    Holding(HeldTool),
    /// The bar names something the player said is not a tracked tool, as
    /// it was dismissed.
    NotTracked { name: String },
    /// The bar is in view, whole, and its name area is plain fill: nothing
    /// is held.
    Nothing,
    /// The bar is in view but does not say which tool is in hand.
    Uncertain(Why),
    /// The bar is not in view: a window lies over it, or it is not shown.
    NotVisible,
}

impl ToolSeen {
    /// What a bar in `state` shows, for every state but a lettered one,
    /// whose name must be read to say.
    ///
    /// A health bar with no tool panel beside it is uncertain, never
    /// nothing held: what the client draws when nothing is in hand has
    /// not been seen, so a bar that might be showing it is not taken to.
    /// Only a whole bar whose name area is plain fill holds nothing.
    pub fn of_bar(state: &BarState) -> Option<ToolSeen> {
        match state {
            BarState::Absent | BarState::Covered(_) => Some(ToolSeen::NotVisible),
            BarState::NoPanel(_) | BarState::Unclear(_) => {
                Some(ToolSeen::Uncertain(Why::Unreadable))
            }
            BarState::Empty(_) => Some(ToolSeen::Nothing),
            BarState::Lettered(_) => None,
        }
    }

    /// Whether a change to this is told at once: a tool held, a name known
    /// not to be one, or nothing held. Anything else waits for the settle
    /// time.
    fn told_at_once(&self) -> bool {
        matches!(
            self,
            ToolSeen::Holding(_) | ToolSeen::NotTracked { .. } | ToolSeen::Nothing
        )
    }
}

impl From<ToolMatch> for ToolSeen {
    fn from(found: ToolMatch) -> Self {
        match found {
            ToolMatch::Library { id } => ToolSeen::Holding(HeldTool::Library { id }),
            ToolMatch::Bundled {
                endpoint,
                item_id,
                name,
            } => ToolSeen::Holding(HeldTool::Bundled {
                endpoint,
                item_id,
                name,
            }),
            ToolMatch::NotTracked { name } => ToolSeen::NotTracked { name },
            ToolMatch::Twins { ids } => ToolSeen::Uncertain(Why::Twins { ids }),
            ToolMatch::Disabled { ids } => ToolSeen::Uncertain(Why::Disabled { ids }),
            ToolMatch::Contended { candidates, more } => {
                ToolSeen::Uncertain(Why::Contended { candidates, more })
            }
            ToolMatch::Unknown { text } => ToolSeen::Uncertain(Why::Unknown { text }),
        }
    }
}

/// A change in what is in hand, for the session it was seen in: what the
/// bar now shows, and when the frame that first showed it was captured
/// (epoch milliseconds on the injected clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPulse {
    pub session_id: String,
    pub at_ms: i64,
    pub seen: ToolSeen,
}

/// Where the watcher hands each change: whether it was delivered. A change
/// that could not be (the consumer is gone) ends the watch.
pub type ToolPulseSink = Sink<ToolPulse>;

/// The tools a name read off the bar is matched against: the player's
/// library, the names the player dismissed and the bundled game data.
/// Replaced in place when the library changes; the next look matches with
/// the new one, even if the bar did not change.
pub type ToolCatalogueCell = SwapCell<ToolCatalogue>;

/// The tracker's handle on the held-tool watcher.
pub trait ToolDetector: Send + Sync {
    /// Start watching for `session_id`, handing each change to `sink`, and
    /// telling `ended` if the watch ends by itself. A watch already running
    /// for another session stops first.
    fn start(&self, session_id: &str, sink: ToolPulseSink, ended: WatchEnded);
    /// Stop watching. Returns at once; the worker ends within its look.
    fn stop(&self);
    /// Whether the bar is being watched for `session_id` now: a watch can
    /// fail to start (capture not running, refused) or end on its own.
    fn watches(&self, session_id: &str) -> bool;
    /// The player turned the reading on: a capture the desktop refused
    /// before may be asked for again.
    fn permit_capture(&self);
    /// Something the player did may have changed the tool in hand (a key
    /// pressed in the game): look, and read the name, as soon as the
    /// watcher allows. Returns at once, from any thread.
    fn hint(&self);
    /// Stop at app exit, waiting briefly for a look in progress.
    fn shutdown(&self) {
        self.stop();
    }
}

/// A watcher that never runs: what a tracker has without one.
pub struct NoToolDetector;

impl ToolDetector for NoToolDetector {
    fn start(&self, _session_id: &str, _sink: ToolPulseSink, _ended: WatchEnded) {}

    fn stop(&self) {}

    fn watches(&self, _session_id: &str) -> bool {
        false
    }

    fn permit_capture(&self) {}

    fn hint(&self) {}
}

/// A held-tool watcher bound after the tracker is built. Until it is
/// bound, nothing is watched.
pub type ToolDetectorSlot = LateBound<dyn ToolDetector>;

impl ToolDetector for ToolDetectorSlot {
    fn start(&self, session_id: &str, sink: ToolPulseSink, ended: WatchEnded) {
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

    fn hint(&self) {
        if let Some(detector) = self.bound() {
            detector.hint();
        }
    }

    fn shutdown(&self) {
        if let Some(detector) = self.bound() {
            detector.shutdown();
        }
    }
}

/// The seams the composition root wires in.
pub struct ToolWatchProviders {
    /// The game window, whole or a part of it, captured only if capture is
    /// already running: it must never start capture (see
    /// [`crate::eu_window::capture_game_window_running`]).
    pub frame: GameWindowFrame,
    /// Whether capture is running now, starting nothing.
    pub capture_running: Probe,
    /// Start capture, asking the desktop's consent where it must: called
    /// only for the player's turning the reading on.
    pub start_capture: Probe,
    /// Whether the reading is on, read live before every look.
    pub enabled: Probe,
    /// Where keyboard focus sits; `None` where it cannot be asked.
    pub focus: Option<GameFocusProbe>,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The recogniser; `None` when none was loaded.
    pub read_text: Option<FrameReader>,
    pub catalogue: Arc<ToolCatalogueCell>,
    /// How to wait for the next look; `None` waits in real time.
    pub wait: Option<PulseWait>,
}

impl Default for ToolWatchProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|_| Err(FrameUnavailable::WindowNotFound)),
            capture_running: Arc::new(|| true),
            start_capture: Arc::new(|| true),
            enabled: Arc::new(|| false),
            focus: None,
            profile: None,
            read_text: None,
            catalogue: Arc::new(ToolCatalogueCell::default()),
            wait: None,
        }
    }
}

/// What the held-tool watcher does at each look (see the module's notes).
pub struct BarLook {
    profile: Option<Arc<UiProfile>>,
    read_text: Option<FrameReader>,
    catalogue: Arc<ToolCatalogueCell>,
}

/// The lettering locked at the last good read, and what that read came
/// to.
struct Locked {
    lettering: Lettering,
    seen: ToolSeen,
    /// When the read was made (monotonic seconds).
    read_at: f64,
    /// The tools it was matched against.
    catalogue: Arc<ToolCatalogue>,
}

/// A run of looks whose answers are not told at once, waiting out the
/// settle time.
struct Run {
    /// When the run's first look was captured.
    at_ms: i64,
    /// The same moment on the monotonic clock.
    since: f64,
    /// The latest answer of the run.
    latest: ToolSeen,
    /// The latest uncertain answer of the run, if it had one.
    uncertain: Option<ToolSeen>,
}

impl Run {
    fn new(seen: ToolSeen, at_ms: i64, since: f64) -> Self {
        let mut run = Run {
            at_ms,
            since,
            latest: seen.clone(),
            uncertain: None,
        };
        run.add(seen);
        run
    }

    fn add(&mut self, seen: ToolSeen) {
        if matches!(seen, ToolSeen::Uncertain(_)) {
            self.uncertain = Some(seen.clone());
        }
        self.latest = seen;
    }

    /// What the run tells: an unseen bar only when every look of it was
    /// unseen (an unseen bar keeps the tool in hand, so it is never told
    /// on mixed evidence), else the latest uncertain answer.
    fn verdict(self) -> ToolSeen {
        self.uncertain.unwrap_or(self.latest)
    }
}

/// What the held-tool watcher keeps from one look to the next.
#[derive(Default)]
pub struct BarMemory {
    /// The area captured to look at the bar again, in the client area's
    /// coordinates; `None` while the bar's place is not known.
    place: Option<Rect>,
    /// When the whole window was last captured to look for the bar.
    searched: Option<f64>,
    /// The next look captures the whole window whenever the last was: a
    /// capture of the bar's area could not say whether a window covers it.
    confirm: bool,
    /// The bar has not been seen whole since a capture of its area last
    /// could not say: a further such doubt waits for the search interval.
    doubted: bool,
    /// Kept while the bar shows lettering read well; let go when it shows
    /// anything else, or the read was refused, so it is read again.
    locked: Option<Locked>,
    /// After a hint, the name is read at every look until then (monotonic).
    read_until: Option<f64>,
    /// What was last told.
    told: Option<ToolSeen>,
    run: Option<Run>,
}

impl BarMemory {
    /// What to tell, if anything, now that a look captured at `at_ms`
    /// (`now` on the monotonic clock) shows `seen`: the change and the
    /// time it is told with.
    ///
    /// A tool held, a name known not to be one, or nothing held is told at
    /// once. Anything else starts or continues a run, told once the run
    /// has lasted `settle` with the time of its first look, whatever
    /// mixture of uncertain and unseen it held (see [`Run::verdict`]).
    /// Once something not told at once has been told, another such answer
    /// is told only after it has itself held for `settle` without a break.
    fn settle(
        &mut self,
        seen: ToolSeen,
        at_ms: i64,
        now: f64,
        settle: f64,
    ) -> Option<(i64, ToolSeen)> {
        if seen.told_at_once() {
            self.run = None;
            if self.told.as_ref() == Some(&seen) {
                return None;
            }
            self.told = Some(seen.clone());
            return Some((at_ms, seen));
        }
        if self.told.as_ref().is_some_and(|told| !told.told_at_once()) {
            if self.told.as_ref() == Some(&seen) {
                self.run = None;
                return None;
            }
            if self.run.as_ref().is_none_or(|run| run.latest != seen) {
                self.run = Some(Run::new(seen, at_ms, now));
            }
        } else {
            match &mut self.run {
                Some(run) => run.add(seen),
                None => self.run = Some(Run::new(seen, at_ms, now)),
            }
        }
        self.conclude(now, settle)
    }

    /// The run, told if it has lasted `settle` by `now`: what a look that
    /// saw nothing to contradict it, and nothing new, may tell.
    fn conclude(&mut self, now: f64, settle: f64) -> Option<(i64, ToolSeen)> {
        if !self
            .run
            .as_ref()
            .is_some_and(|run| now - run.since >= settle)
        {
            return None;
        }
        let run = self.run.take()?;
        let at_ms = run.at_ms;
        let seen = run.verdict();
        if self.told.as_ref() == Some(&seen) {
            return None;
        }
        self.told = Some(seen.clone());
        Some((at_ms, seen))
    }
}

/// Milliseconds from the profile, in seconds.
fn seconds(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

/// A look that tells `told`, if anything.
fn tell(eyes: &Eyes, told: Option<(i64, ToolSeen)>) -> Look<ToolPulse> {
    match told {
        Some((at_ms, seen)) => Look::Tell(ToolPulse {
            session_id: eyes.session_id().to_string(),
            at_ms,
            seen,
        }),
        None => Look::Quiet,
    }
}

/// Whether a bar in `state`, seen in a capture of only the bar's area,
/// may be covered by a window the capture holds too little of to tell:
/// nothing placed there, a health bar with no panel beside it, or an
/// outline broken where no window was found. Marks in the name area of a
/// whole outline are not: a capture of the whole window would tell the
/// same.
fn may_be_covered(state: &BarState) -> bool {
    match state {
        BarState::Absent | BarState::NoPanel(_) => true,
        BarState::Unclear(bar) => bar.view == View::Broken,
        BarState::Covered(_) | BarState::Empty(_) | BarState::Lettered(_) => false,
    }
}

impl Watcher for BarLook {
    type Pulse = ToolPulse;
    type Memory = BarMemory;
    const THREAD: &'static str = "tool-watch";
    const WHAT: &'static str = "the held-tool watcher";

    fn reader(&self) -> Option<Reader> {
        Some(Reader {
            profile: self.profile.clone()?,
            read_text: self.read_text.clone()?,
        })
    }

    fn interval(reader: &Reader) -> f64 {
        seconds(reader.profile.tool_watch.look_ms)
    }

    fn hint_gap(reader: &Reader) -> f64 {
        seconds(reader.profile.tool_watch.hint_gap_ms)
    }

    fn memory(_reader: &Reader) -> BarMemory {
        BarMemory::default()
    }

    fn look(&self, reader: &Reader, memory: &mut BarMemory, eyes: &Eyes) -> Look<ToolPulse> {
        let profile = reader.profile.as_ref();
        let watch = &profile.tool_watch;
        let settle = seconds(watch.settle_ms);
        let clock = eyes.clock();
        let metrics = eo_wire::metrics::metrics();
        let now = clock.monotonic();
        if let Some(hinted_at) = eyes.hinted_at() {
            let until = hinted_at + seconds(watch.hint_window_ms);
            memory.read_until = Some(memory.read_until.map_or(until, |t| t.max(until)));
        }
        let part = memory.place;
        let area = match part {
            Some(place) => FrameArea::Part(place),
            None => {
                let confirm = std::mem::take(&mut memory.confirm);
                if !confirm
                    && memory
                        .searched
                        .is_some_and(|last| now - last < seconds(watch.search_ms))
                {
                    return tell(eyes, memory.conclude(now, settle));
                }
                FrameArea::Whole
            }
        };
        let Some(capture) = eyes.capture(area) else {
            return Look::Quiet;
        };
        let (at_ms, began) = (capture.at_ms, capture.began);
        if part.is_none() {
            memory.searched = Some(began);
        }
        let unreadable = ToolSeen::Uncertain(Why::Unreadable);
        let frame = match capture.frame {
            Ok(frame) => {
                metrics.record_tool_capture(capture.took);
                if clock.monotonic() - began > seconds(watch.capture_slack_ms) {
                    // Too late to stand for the moment it was stamped
                    // with: the bar was not seen then.
                    return tell(eyes, memory.settle(unreadable, at_ms, began, settle));
                }
                frame
            }
            Err(FrameUnavailable::CaptureNotStarted) => return Look::Refused,
            Err(FrameUnavailable::WindowNotFound) => {
                // No game window, so no shots: nothing to say.
                memory.place = None;
                return Look::Quiet;
            }
            Err(FrameUnavailable::OutsideCapturableArea | FrameUnavailable::CaptureFailed) => {
                // The window is there but the bar could not be seen; it is
                // looked for again.
                memory.place = None;
                return tell(eyes, memory.settle(unreadable, at_ms, began, settle));
            }
        };
        let image = &frame.image;
        let started = Instant::now();
        let all = Rect::new(0, 0, image.w as i32 - 1, image.h as i32 - 1);
        let found = find_tool_bar_in(image, all, profile);
        let state = bar_state(image, found, profile);
        let shown = lettering(image, &state, profile);
        metrics.record_tool_search(started.elapsed());
        if part.is_some() && may_be_covered(&state) {
            // Not told from the bar's area: the whole window is captured to
            // decide, at once the first time, then at the search interval.
            memory.place = None;
            memory.locked = None;
            if !memory.doubted {
                memory.confirm = true;
            }
            memory.doubted = true;
            return tell(eyes, memory.conclude(began, settle));
        }
        if matches!(state, BarState::Lettered(_) | BarState::Empty(_)) {
            memory.doubted = false;
        }
        memory.place = found.and_then(|bar| bar.pair(profile)).map(|pair| {
            let (dx, dy) = frame.offset;
            let place = pair.offset(dx as i32, dy as i32).padded(watch.margin);
            Rect::new(place.x0.max(0), place.y0.max(0), place.x1, place.y1)
        });
        let seen = match (ToolSeen::of_bar(&state), shown) {
            (Some(seen), _) => {
                memory.locked = None;
                seen
            }
            (None, None) => {
                memory.locked = None;
                unreadable
            }
            (None, Some(shown)) => {
                let catalogue = self.catalogue.current();
                let read = eyes.hinted()
                    || memory.read_until.is_some_and(|until| began < until)
                    || memory.locked.as_ref().is_none_or(|locked| {
                        shown.worth_rereading(&locked.lettering, profile)
                            || began - locked.read_at >= seconds(watch.reread_ms)
                            || !Arc::ptr_eq(&locked.catalogue, &catalogue)
                    });
                match &memory.locked {
                    Some(locked) if !read => locked.seen.clone(),
                    _ => {
                        let started = Instant::now();
                        let read_text = &reader.read_text;
                        let recogniser = |crop: &BgrImage| read_text(crop);
                        let name = read_tool_name(image, &state, profile, &recogniser);
                        let seen = match name {
                            Ok(name) => {
                                let seen =
                                    ToolSeen::from(resolve(&name, &catalogue, &profile.tool_names));
                                memory.locked = Some(Locked {
                                    lettering: shown,
                                    seen: seen.clone(),
                                    read_at: began,
                                    catalogue,
                                });
                                seen
                            }
                            // A refused read locks nothing: the next look
                            // reads again.
                            Err(_) => {
                                memory.locked = None;
                                unreadable
                            }
                        };
                        metrics.record_tool_read(started.elapsed());
                        seen
                    }
                }
            }
        };
        tell(eyes, memory.settle(seen, at_ms, began, settle))
    }
}

/// The held-tool watcher.
pub type ToolWatch = ScreenWatch<BarLook>;

impl ScreenWatch<BarLook> {
    pub fn new(providers: ToolWatchProviders, clock: Arc<dyn Clock>) -> Arc<Self> {
        ScreenWatch::from_parts(
            BarLook {
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

impl ToolDetector for Arc<ToolWatch> {
    fn start(&self, session_id: &str, sink: ToolPulseSink, ended: WatchEnded) {
        ToolWatch::start_reporting_end(self, session_id, sink, ended);
    }

    fn stop(&self) {
        ToolWatch::stop(self);
    }

    fn watches(&self, session_id: &str) -> bool {
        ToolWatch::watching(self).as_deref() == Some(session_id)
    }

    fn permit_capture(&self) {
        ToolWatch::permit_capture(self);
    }

    fn hint(&self) {
        ToolWatch::hint(self);
    }

    fn shutdown(&self) {
        ToolWatch::shutdown(self);
    }
}

#[cfg(test)]
mod tests;
