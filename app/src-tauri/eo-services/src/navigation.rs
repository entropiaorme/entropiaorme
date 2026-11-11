//! Persisted map-route navigation and radar guidance.
//!
//! Pins are immutable route inputs. Operational progress belongs to a
//! navigation run, so reaching a tree never mutates the pin itself. All
//! state-changing operations serialise through `operation`, while the
//! synchronous database core remains behind its async reader/writer seam.
//! Positions are read off the screen before `operation` is taken, so a
//! slow read never holds up the route's other verbs, and a reading is
//! applied only to the route it was started for.
//!
//! Radar guidance places two click-through overlays over and beside the
//! radar. One worker finds the radar when guidance starts and looks again
//! at a slow interval while it runs, so a radar the player moves or
//! resizes is followed; it captures nothing once guidance stops, however
//! it stops.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::OptionalExtension;

use crate::clock::Clock;
use crate::coord_capture::{
    CoordCaptureService, CoordConfirmListener, CoordReading, CoordRefusal, LabelStrip, RadarScale,
    ScaleReading, SightingFailure, Trigger,
};
use crate::db::{Db, DbError};
use crate::eu_window::GameFocus;
use crate::game_ui::radar::CoordinateBounds;
use crate::game_ui::seams::FrameUnavailable;
use crate::hotbar_listener::GameFocusProbe;
use crate::keystroke_source::{KeystrokeKind, KeystrokeSource};
use crate::time::naive_to_epoch;

/// Arrival radius, in game units (metres): a harvest or a manual Visited
/// within this of a route tree counts as reaching it. Set to fifteen because
/// Entropia Universe lets a player start cutting a tree from a wide radius, so
/// arrival should register anywhere within that reach, not only right on the
/// surveyed pin. Distinct from the pin-drop duplicate radius below.
pub const ARRIVAL_TOLERANCE_UNITS: f64 = 15.0;
/// Radius, in game units (metres), within which a new pin is flagged as a
/// possible duplicate of an existing one (advisory only).
pub const DUPLICATE_TOLERANCE_UNITS: f64 = 5.0;
/// A confirmed visit puts its tree on cooldown for this long, so a freshly
/// regenerated route excludes trees that were just harvested. Two hours is
/// the initial default; a configurable per-species respawn is deferred work.
pub const COOLDOWN_SECONDS: f64 = 2.0 * 60.0 * 60.0;
/// The game's interact key, which starts cutting a tree. A route planned to
/// visit on this key records the active tree the moment it is pressed.
pub const VISIT_KEY: &str = "f";
/// A second harvest this soon after the last one, from within the arrival
/// radius of where that one was observed, is the same tree swung at again.
/// The interact key is exempt: each press is the player declaring a cut, which
/// is what lets a dense stand be walked tree by tree.
const REPEAT_VISIT_WINDOW_SECONDS: f64 = 30.0;

/// How long shutting down waits for the guidance worker to finish its
/// look. A worker can be held inside a capture for as long as the
/// desktop's consent prompt is open; app exit does not wait on the player
/// for that, and a worker still out when this passes ends with the
/// process.
const GUIDANCE_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// How long starting guidance waits for its first look, so the overlays
/// open where the radar is rather than jumping there a moment later. A
/// first look still out after this (the desktop asking for consent to
/// capture, say) places the overlays when it lands.
const GUIDANCE_FIRST_LOOK_WAIT: Duration = Duration::from_secs(3);

/// The planet's coordinates, by planet name; `None` for a planet whose
/// coordinates are not known.
pub type BoundsProvider = Arc<dyn Fn(&str) -> Option<CoordinateBounds> + Send + Sync>;
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

/// How a reading of the position is told from a jump the game cannot make.
///
/// Every reading the app records or acts on without the player confirming
/// it (the position poll, a harvest, the interact key, and a Visited
/// outside the arrival radius) passes this check against the route's last
/// recorded position. A teleport does carry the player that far, so a
/// refused position is accepted once the reading after it agrees with it.
///
/// What this cannot do: recognition is deterministic, so a player standing
/// still gives identical pixels, and a misreading repeats. The next reading
/// then agrees with the wrong one and it is accepted. The check stops
/// passing errors only: a reading wrong on one look and right on the next.
/// A misread digit that keeps its width and stays inside the planet's
/// coordinates is stopped by nothing in the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JumpLimit {
    /// The fastest the game carries a player across a planet's surface,
    /// in metres per second of time between the two readings. Set well
    /// above the top speed of any vehicle a player can drive or fly over
    /// a planet, while a value misread in its leading digits moves the
    /// position by thousands of metres.
    pub speed_m_per_s: f64,
    /// Metres allowed whatever the time between the readings: the values
    /// are whole metres, and two readings can be taken moments apart.
    pub allowance_m: f64,
    /// The most time between two readings the allowance may grow with, in
    /// seconds. After a long gap (the coordinates covered for a minute,
    /// say) the allowance would otherwise cover kilometres and admit the
    /// first misreading at once; beyond this, a reading further than the
    /// capped allowance needs the next reading's agreement like any jump.
    pub max_gap_s: f64,
    /// How many readings in a row must agree with each other, the first
    /// of them the refused one, before it is accepted.
    pub confirmations: u32,
}

impl Default for JumpLimit {
    fn default() -> Self {
        Self {
            speed_m_per_s: 100.0,
            allowance_m: 20.0,
            max_gap_s: 5.0,
            confirmations: 2,
        }
    }
}

/// How route navigation reads the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavigationSettings {
    pub jump: JumpLimit,
    /// How often guidance looks for the radar again while it runs: slow,
    /// because a player moves or resizes the radar rarely, and each look
    /// captures the game window.
    pub guidance_interval: Duration,
    /// How long guidance runs on with the radar not found before it stops:
    /// long enough for a window opened over the radar for a while, short
    /// enough that guidance left running with the radar hidden is not
    /// capturing the screen for an evening. The hand calibration standing
    /// in for the radar does not count as finding it.
    pub guidance_idle_limit: Duration,
    /// How many looks in a row must miss a radar that was found before
    /// guidance shows it lost: a window dragged across it for a moment
    /// leaves the overlay where it was.
    pub guidance_loss_looks: u32,
    /// How long the position poll goes on with every read refused before
    /// it stops and says so: a few minutes of the coordinates covered or
    /// unreadable means the poll is no longer serving the player.
    pub poll_refusal_limit: Duration,
}

impl Default for NavigationSettings {
    fn default() -> Self {
        Self {
            jump: JumpLimit::default(),
            guidance_interval: Duration::from_secs(5),
            guidance_idle_limit: Duration::from_secs(3 * 60),
            guidance_loss_looks: 2,
            poll_refusal_limit: Duration::from_secs(3 * 60),
        }
    }
}

/// The seams and settings the composition root wires in.
pub struct NavigationProviders {
    pub coord_capture: Arc<CoordCaptureService>,
    pub bounds: BoundsProvider,
    /// Called whenever the route changes, to publish it.
    pub changed: ChangedSink,
    pub input: Option<Arc<dyn KeystrokeSource>>,
    pub focus: Option<GameFocusProbe>,
    pub settings: NavigationSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Active,
    // No live code path ever sets a run to Paused: pause/resume was removed. The
    // variant survives only to parse a legacy `paused` row (the frozen migration
    // 0009 CHECK still permits it), which `NavigationService::new` ends at
    // startup. Kept for round-trip completeness with that persisted schema, not
    // as an observable runtime state.
    Paused,
    Completed,
    Ended,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Ended => "ended",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopStatus {
    Pending,
    Active,
    Visited,
    Skipped,
}

impl StopStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Visited => "visited",
            Self::Skipped => "skipped",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "visited" => Some(Self::Visited),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationStop {
    pub id: i64,
    pub pin_id: i64,
    pub ordinal: i64,
    pub status: StopStatus,
    pub name: String,
    pub icon: String,
    pub lon: f64,
    pub lat: f64,
    pub completed_at: Option<f64>,
    pub completion_source: Option<String>,
}

/// A harvest swing detected outside the arrival radius of every route stop.
/// EU trees are cuttable from well beyond that radius, so rather than dropping
/// the swing the actor stashes this proposal for the player to accept or
/// dismiss in the overlay. It is ephemeral (never persisted) and always names
/// the currently-active stop.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingHarvest {
    pub stop_id: i64,
    pub pin_id: i64,
    pub name: String,
    /// Where the player was read to be at the harvest, and how far that is
    /// from the proposed tree; `None` when the position did not read or did
    /// not pass the position checks: the harvest still proposes the tree.
    pub observed: Option<Observed>,
    /// When the harvest happened: the reading's time when there is one.
    pub at: f64,
    pub outcome: String,
}

/// A position read for a visit, and its distance from the visited tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observed {
    pub lon: f64,
    pub lat: f64,
    pub distance: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationRun {
    pub id: i64,
    pub planet: String,
    pub map_view_id: Option<i64>,
    pub map_view_name: Option<String>,
    pub status: RunStatus,
    pub start_lon: f64,
    pub start_lat: f64,
    pub current_lon: f64,
    pub current_lat: f64,
    pub last_position_at: Option<f64>,
    pub hop_count: i64,
    /// Visits are recorded on the interact key rather than detected from
    /// harvests.
    pub visit_on_key: bool,
    pub updated_at: f64,
    pub stops: Vec<NavigationStop>,
    /// Set only on the snapshot read, and only while its stop is still active.
    pub pending_harvest: Option<PendingHarvest>,
}

impl NavigationRun {
    pub fn active_stop(&self) -> Option<&NavigationStop> {
        self.stops
            .iter()
            .find(|stop| stop.status == StopStatus::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarGeometry {
    pub centre_x: i64,
    pub centre_y: i64,
    pub north_x: i64,
    pub north_y: i64,
    pub radius_px: f64,
    pub display_scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadarCalibrationPhase {
    Idle,
    AwaitCentre,
    AwaitNorthEdge { centre: (i64, i64) },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PositionUpdate {
    Updated(NavigationRun),
    NoActiveRun,
    Paused(NavigationRun),
    /// The position was not read; the refusal says why, and carries no
    /// number. Nothing is recorded.
    NotRead(CoordRefusal),
    /// The position read lies further from the route's last position than
    /// the game can carry a player in the time between the two readings.
    /// Nothing is recorded; a teleport is accepted once the following
    /// polls keep reading the new position.
    JumpRefused,
    /// The route ended, or another took its place, while the position was
    /// being read; the reading belonged to the route before and is
    /// dropped.
    Superseded,
    /// The position poll is not running; the reason says why.
    PollStopped(PollStop),
    Ambiguous(NavigationRun),
    /// A manual `Visited` whose observed position is outside the arrival
    /// tolerance. The run position is updated so distance/bearing are fresh,
    /// but no visit is recorded until the user confirms a forced visit.
    OutOfTolerance(NavigationRun),
}

/// Why the position poll does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollStop {
    /// The route overlays are not showing: nobody is steered by the poll.
    OverlaysHidden,
    /// Every read was refused for the poll's refusal limit.
    NotReading,
    /// Screen capture was not permitted. Asking again waits for the player.
    CaptureNotAllowed,
}

/// Why a placement has no scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMissing {
    /// The rings are there but their distances did not read: the client
    /// can hide them.
    LabelsUnread,
    /// No rings were found to measure.
    NoRings,
    /// No recogniser is loaded to read the distances with.
    NoReader,
}

/// Where the guidance overlays go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarPlacement {
    /// The radar's centre, in screen coordinates.
    pub centre_x: f64,
    pub centre_y: f64,
    /// The map disc's radius, in screen pixels.
    pub radius_px: f64,
    /// The screen column right of which a window beside the radar covers
    /// neither the disc nor the coordinates printed under it.
    pub clear_right_x: f64,
    pub source: PlacementSource,
    /// The radar's scale, read from its ring distances on the same look
    /// as the placement; `None` when they are hidden or did not read, or
    /// the placement is the hand calibration's.
    pub scale: Option<RadarScale>,
    /// Why there is no scale, once that has held for two looks; `None`
    /// while there is a scale, for the hand calibration, and while a
    /// missing scale may yet be a passing one.
    pub scale_missing: Option<ScaleMissing>,
    /// Where the ring distances are printed: nothing drawn over the radar
    /// may enter it, or the scale is lost.
    pub label_strip: Option<LabelStrip>,
}

/// Where a placement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementSource {
    /// The radar was found on the screen.
    Found,
    /// The radar was not found, and the player's hand calibration places
    /// it.
    Calibrated,
}

/// Why guidance, while it runs, has no placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceLoss {
    /// The radar is not on the screen, and there is no hand calibration.
    RadarNotFound,
    GameWindowNotFound,
    GameWindowOffScreen,
    CaptureFailed,
}

/// Why guidance stopped by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceStop {
    /// Screen capture was not allowed to start. Guidance does not ask
    /// again by itself; starting it again asks once more.
    CaptureNotAllowed,
    /// The radar was not seen for the idle limit.
    RadarNotSeen,
    /// No interface profile is loaded and there is no hand calibration,
    /// so the radar cannot be placed at all.
    ReaderUnavailable,
    /// Looking for the radar failed unexpectedly.
    Failed,
}

/// Radar guidance, as the overlays show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RadarGuidance {
    /// Not running; when it stopped by itself, why.
    Off {
        stopped: Option<GuidanceStop>,
    },
    /// Running, its first look not yet in.
    Searching,
    Placed(RadarPlacement),
    Lost(GuidanceLoss),
}

#[derive(Debug, thiserror::Error)]
pub enum NavigationError {
    #[error("no active navigation route")]
    NoActiveRun,
    #[error("no eligible pins are available for this route")]
    NoPins,
    #[error("a custom route selection must contain at least one pin")]
    EmptyPinSelection,
    #[error("radar calibration needs a radius of at least 8 pixels")]
    InvalidRadarRadius,
    #[error(transparent)]
    Db(#[from] DbError),
}

/// A position refused as a jump, held to see whether the readings after
/// it agree with it.
#[derive(Debug, Clone, Copy)]
struct HeldJump {
    run: i64,
    at: Fix,
    seen: u32,
}

/// A position read, and when on the clock's monotonic reading.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fix {
    lon: f64,
    lat: f64,
    mono: f64,
}

impl Fix {
    fn of(reading: &CoordReading) -> Self {
        Self {
            lon: reading.lon as f64,
            lat: reading.lat as f64,
            mono: reading.read_mono,
        }
    }

    /// Whether the game can carry a player from `self` to `to` in the
    /// time between the two readings, that time capped (see
    /// [`JumpLimit::max_gap_s`]).
    fn reaches(&self, to: &Fix, limit: &JumpLimit) -> bool {
        let travelled = distance((self.lon, self.lat), (to.lon, to.lat));
        let gap = (to.mono - self.mono).abs().min(limit.max_gap_s);
        travelled <= limit.speed_m_per_s * gap + limit.allowance_m
    }
}

/// The position check every recorded reading passes (see [`JumpLimit`]),
/// and the route's last recorded position it measures from.
#[derive(Debug, Default)]
struct JumpGate {
    /// The route's last recorded position, when it was read on this run
    /// of the app.
    last: Option<(i64, Fix)>,
    held: Option<HeldJump>,
}

impl JumpGate {
    /// Whether `fix` may be recorded on route `run`. A route with no
    /// recorded reading yet has nothing to measure from: its first reading
    /// needs the next one's agreement.
    fn admit(&mut self, run: i64, fix: Fix, limit: &JumpLimit) -> bool {
        if self.last(run).is_some_and(|last| last.reaches(&fix, limit)) {
            self.record(run, fix);
            return true;
        }
        let seen = match self.held {
            Some(held) if held.run == run && held.at.reaches(&fix, limit) => held.seen + 1,
            _ => 1,
        };
        if seen >= limit.confirmations {
            self.record(run, fix);
            return true;
        }
        self.held = Some(HeldJump { run, at: fix, seen });
        false
    }

    /// `fix` is the route's position now.
    fn record(&mut self, run: i64, fix: Fix) {
        self.last = Some((run, fix));
        self.held = None;
    }

    fn last(&self, run: i64) -> Option<Fix> {
        self.last
            .filter(|(last_run, _)| *last_run == run)
            .map(|(_, fix)| fix)
    }

    /// A read that came to nothing: the readings either side of it are
    /// not consecutive.
    fn interrupt(&mut self) {
        self.held = None;
    }
}

/// The position poll's own state.
#[derive(Debug, Default)]
struct PollState {
    stopped: Option<PollStop>,
    /// When, on the monotonic clock, the current run of refused reads
    /// began.
    failing_since: Option<f64>,
}

/// What became of a read for a route, once checked.
enum Checked {
    /// The route ended or changed while it was read.
    Superseded,
    /// The route as it is now, and the position: read and passed the
    /// checks, or why not.
    Current(Box<NavigationRun>, Result<CoordReading, Unread>),
}

/// Why a read gives no position to record.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Unread {
    Refused(CoordRefusal),
    /// The position check held it back as a jump.
    Jump,
    /// A newer position was recorded while this one was read.
    Stale,
}

/// The guidance worker's lifecycle.
enum Guiding {
    Off {
        stopped: Option<GuidanceStop>,
    },
    Running {
        session: u64,
        view: Option<Look>,
        /// Held to wake the worker for a look now, and to be dropped:
        /// leaving this state disconnects the worker's receiver, which
        /// ends its wait at once.
        wake: mpsc::Sender<()>,
    },
}

/// What one look for the radar came to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Placed(RadarPlacement),
    Lost(GuidanceLoss),
}

struct GuidanceState {
    lifecycle: Guiding,
    last_session: u64,
    /// The status last announced through the sink.
    last_announced: RadarGuidance,
    /// Looks written back, for whoever must know a look has landed.
    looks: u64,
}

/// Counts the live guidance workers, so shutting down can wait for them.
#[derive(Default)]
struct Workers {
    live: Mutex<usize>,
    done: Condvar,
}

/// One live guidance worker; dropping it, however the worker ends, counts
/// it out.
struct WorkerToken {
    service: Arc<NavigationService>,
}

impl Drop for WorkerToken {
    fn drop(&mut self) {
        let workers = &self.service.guidance_workers;
        let mut live = workers.live.lock().unwrap_or_else(PoisonError::into_inner);
        *live = live.saturating_sub(1);
        workers.done.notify_all();
    }
}

pub struct NavigationService {
    db: Db,
    clock: Arc<dyn Clock>,
    coord_capture: Arc<CoordCaptureService>,
    bounds: BoundsProvider,
    changed: ChangedSink,
    settings: NavigationSettings,
    operation: tokio::sync::Mutex<()>,
    jump_gate: Mutex<JumpGate>,
    poll: Mutex<PollState>,
    /// Whether the route overlays are showing: the poll and guidance run
    /// only for overlays someone can see.
    overlays_shown: std::sync::atomic::AtomicBool,
    radar_phase: Mutex<RadarCalibrationPhase>,
    radar_confirm_listener: Mutex<Option<std::sync::Weak<CoordConfirmListener>>>,
    /// The hand calibration, read when guidance starts and replaced when
    /// the player calibrates again.
    calibration: Mutex<Option<RadarGeometry>>,
    guidance: Mutex<GuidanceState>,
    /// Told whenever a look is written back.
    guidance_looked: Condvar,
    guidance_workers: Workers,
    guidance_changed: Mutex<Option<ChangedSink>>,
    input: Option<Arc<dyn KeystrokeSource>>,
    input_claimed: std::sync::atomic::AtomicBool,
    route_live: std::sync::atomic::AtomicBool,
    // Where keyboard focus sits: an interact-key press typed into another
    // window is not a tree being cut.
    focus: Option<GameFocusProbe>,
    // Held between the interact key's press and release, so the autorepeat
    // presses of a held key record one visit, not a stream of them.
    visit_key_down: std::sync::atomic::AtomicBool,
    // A harvest swing detected beyond the arrival radius, awaiting the player's
    // confirm/dismiss in the overlay. Ephemeral; the snapshot self-heals it away
    // once the active stop moves on.
    pending_harvest: Mutex<Option<PendingHarvest>>,
}

impl NavigationService {
    pub async fn new(db: Db, clock: Arc<dyn Clock>, providers: NavigationProviders) -> Arc<Self> {
        let NavigationProviders {
            coord_capture,
            bounds,
            changed,
            input,
            focus,
            settings,
        } = providers;
        // An interrupted route is not restored as a unit: recovery is by
        // regenerating a route that excludes cooled-down trees (the per-tree
        // visit records are the recovery point). Any run left live by a crash
        // or a hard close is ended at startup rather than hydrated.
        let now = naive_to_epoch(clock.now());
        let _ = end_lingering_runs(&db, now).await;
        let service = Arc::new(Self {
            db,
            clock,
            coord_capture,
            bounds,
            changed,
            settings,
            operation: tokio::sync::Mutex::new(()),
            jump_gate: Mutex::new(JumpGate::default()),
            poll: Mutex::new(PollState::default()),
            overlays_shown: std::sync::atomic::AtomicBool::new(false),
            radar_phase: Mutex::new(RadarCalibrationPhase::Idle),
            radar_confirm_listener: Mutex::new(None),
            calibration: Mutex::new(None),
            guidance: Mutex::new(GuidanceState {
                lifecycle: Guiding::Off { stopped: None },
                last_session: 0,
                last_announced: RadarGuidance::Off { stopped: None },
                looks: 0,
            }),
            guidance_looked: Condvar::new(),
            guidance_workers: Workers::default(),
            guidance_changed: Mutex::new(None),
            input: input.clone(),
            input_claimed: std::sync::atomic::AtomicBool::new(false),
            route_live: std::sync::atomic::AtomicBool::new(false),
            focus,
            visit_key_down: std::sync::atomic::AtomicBool::new(false),
            pending_harvest: Mutex::new(None),
        });
        if let Some(input) = input {
            let weak = Arc::downgrade(&service);
            let runtime = tokio::runtime::Handle::current();
            input.subscribe(Arc::new(move |event| {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                use std::sync::atomic::Ordering;
                if event.key != VISIT_KEY {
                    return;
                }
                if event.kind == KeystrokeKind::Release {
                    service.visit_key_down.store(false, Ordering::SeqCst);
                    return;
                }
                if service.visit_key_down.swap(true, Ordering::SeqCst) {
                    return;
                }
                // The keyboard is claimed only while a route that visits on
                // the key is live; the shared hook may still deliver the key
                // for another consumer, and those presses are dropped here.
                if !service.input_claimed.load(Ordering::SeqCst)
                    || !service.route_live.load(Ordering::SeqCst)
                {
                    return;
                }
                // Only a definite Unfocused drops the press, matching the
                // hotbar listener: a platform that cannot tell admits it.
                if service
                    .focus
                    .as_ref()
                    .is_some_and(|focus| focus() == GameFocus::Unfocused)
                {
                    return;
                }
                let service = service.clone();
                runtime.spawn(async move {
                    service.on_visit_key().await;
                });
            }));
        }
        service
    }

    /// The route is no longer live (ended or completed): the keyboard is
    /// released and guidance stops with it.
    fn route_ended(&self) {
        self.route_live
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.release_input_if_idle();
        self.guidance_stop();
    }

    fn claim_input(&self) {
        use std::sync::atomic::Ordering;
        if self.input_claimed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(input) = &self.input {
            let _ = input.start();
        }
    }

    fn release_input_if_idle(&self) {
        if self.route_live.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.release_input();
    }

    fn release_input(&self) {
        use std::sync::atomic::Ordering;
        if !self.input_claimed.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(input) = &self.input {
            input.stop();
        }
    }

    pub async fn snapshot(&self) -> Result<Option<NavigationRun>, DbError> {
        let mut run = load_current_run(&self.db).await?;
        if let Some(run) = run.as_mut() {
            self.attach_pending_harvest(run);
        }
        Ok(run)
    }

    /// Decorate a freshly-loaded run with any pending far-harvest confirmation,
    /// but only while it still names the active stop; otherwise clear the stale
    /// proposal. Shared by the snapshot read and the observe path so the overlay
    /// prompt survives continuous automatic position updates.
    fn attach_pending_harvest(&self, run: &mut NavigationRun) {
        let pending = self
            .pending_harvest
            .lock()
            .expect("pending harvest")
            .clone();
        if let Some(pending) = pending {
            let still_active = run.status == RunStatus::Active
                && run.active_stop().map(|stop| stop.id) == Some(pending.stop_id);
            if still_active {
                run.pending_harvest = Some(pending);
            } else {
                *self.pending_harvest.lock().expect("pending harvest") = None;
            }
        }
    }

    pub async fn start(
        &self,
        planet: String,
        map_view_id: Option<i64>,
        start_lon: f64,
        start_lat: f64,
        selected_pin_ids: Option<Vec<i64>>,
        visit_on_key: bool,
    ) -> Result<NavigationRun, NavigationError> {
        if selected_pin_ids.as_ref().is_some_and(Vec::is_empty) {
            return Err(NavigationError::EmptyPinSelection);
        }
        let _guard = self.operation.lock().await;
        let now = naive_to_epoch(self.clock.now());
        let mut candidates = load_candidates(&self.db, planet.clone(), map_view_id, now).await?;
        if let Some(selected_pin_ids) = selected_pin_ids {
            let selected: std::collections::HashSet<_> = selected_pin_ids.into_iter().collect();
            candidates.retain(|candidate| selected.contains(&candidate.id));
        }
        let route = optimise_open_route((start_lon, start_lat), &candidates, candidates.len());
        if route.is_empty() {
            return Err(NavigationError::NoPins);
        }
        // A start read off the screen for this planet is the route's first
        // position, as of when it was read; a start from anywhere else gives
        // the route no position until one is read.
        let start_read = self
            .coord_capture
            .last_reading(&planet)
            .filter(|reading| reading.lon as f64 == start_lon && reading.lat as f64 == start_lat);
        let start_read_at = start_read.map(|reading| reading.read_at);
        let run_id = self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_runs SET status = 'ended', updated_at = ?1 WHERE status IN ('active', 'paused', 'completed')",
                [now],
            )?;
            tx.execute(
                "INSERT INTO navigation_runs (planet, map_view_id, status, start_lon, start_lat, current_lon, current_lat, last_position_at, hop_count, visit_on_key, created_at, updated_at) VALUES (?1, ?2, 'active', ?3, ?4, ?3, ?4, ?8, ?5, ?6, ?7, ?7)",
                rusqlite::params![planet, map_view_id, start_lon, start_lat, route.len() as i64, visit_on_key, now, start_read_at],
            )?;
            let run_id = tx.last_insert_rowid();
            for (index, pin) in route.iter().enumerate() {
                tx.execute(
                    "INSERT INTO navigation_stops (run_id, pin_id, ordinal, status) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![run_id, pin.id, index as i64, if index == 0 { "active" } else { "pending" }],
                )?;
            }
            tx.commit()?;
            Ok(run_id)
        }).await?;
        {
            let mut gate = self
                .jump_gate
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match start_read {
                Some(reading) => gate.record(run_id, Fix::of(&reading)),
                None => gate.interrupt(),
            }
        }
        *self.poll.lock().unwrap_or_else(PoisonError::into_inner) = PollState::default();
        self.route_live
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // The keyboard is claimed only for a route that listens to it, and
        // released when the route ends or completes.
        if visit_on_key {
            // A release missed while the keyboard was unclaimed must not leave
            // the new route's first press swallowed as a held key.
            self.visit_key_down
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.claim_input();
        } else {
            self.release_input();
        }
        let run = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        (self.changed)();
        Ok(run)
    }

    /// Read the player's position off the screen for `planet`, off the
    /// async runtime: a capture and its read take a while, and may wait
    /// on the desktop's consent to capture.
    async fn read_position(
        &self,
        planet: &str,
        trigger: Trigger,
    ) -> Result<CoordReading, CoordRefusal> {
        let bounds = (self.bounds)(planet);
        let capture = self.coord_capture.clone();
        let planet = planet.to_string();
        tokio::task::spawn_blocking(move || capture.scan(&planet, bounds, trigger))
            .await
            .unwrap_or_else(|_| {
                tracing::error!(target: "eo::navigation", "the position read stopped unexpectedly");
                Err(CoordRefusal::CaptureFailed)
            })
    }

    /// Read the position for route `run` and check it, the operation lock
    /// held by the caller once the read is in. A read for a route that has
    /// ended or been replaced is superseded. A reading older than the
    /// route's last recorded position, or one the position check holds
    /// back as a jump (see [`JumpLimit`]), gives no position; one that
    /// passes is the route's position now.
    async fn checked(
        &self,
        run: &NavigationRun,
        read: Result<CoordReading, CoordRefusal>,
    ) -> Result<Checked, NavigationError> {
        let Some(current) = load_live_run(&self.db)
            .await?
            .filter(|live| live.id == run.id)
        else {
            return Ok(Checked::Superseded);
        };
        let mut gate = self
            .jump_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let checked = match read {
            Err(refusal) => {
                gate.interrupt();
                Err(Unread::Refused(refusal))
            }
            Ok(reading)
                if gate
                    .last(current.id)
                    .is_some_and(|last| last.mono > reading.read_mono) =>
            {
                Err(Unread::Stale)
            }
            Ok(reading) if gate.admit(current.id, Fix::of(&reading), &self.settings.jump) => {
                Ok(reading)
            }
            Ok(_) => Err(Unread::Jump),
        };
        drop(gate);
        Ok(Checked::Current(Box::new(current), checked))
    }

    /// How long ago, on the monotonic clock, route `run`'s position was
    /// read; `None` when no position has been read for it on this run of
    /// the app. Steering from a position this old is the caller's call.
    pub fn position_age(&self, run: i64) -> Option<f64> {
        let last = self
            .jump_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last(run)?;
        Some((self.clock.monotonic() - last.mono).max(0.0))
    }

    /// The route overlays have been shown or taken away. The poll and
    /// guidance serve only overlays someone can see: taking them away stops
    /// guidance, and the poll refuses until they show again.
    pub fn set_overlays_shown(&self, shown: bool) {
        self.overlays_shown
            .store(shown, std::sync::atomic::Ordering::SeqCst);
        if !shown {
            self.guidance_stop();
        }
    }

    pub fn overlays_shown(&self) -> bool {
        self.overlays_shown
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The player asks the poll to start again: after it stopped with
    /// every read refused, or with capture not permitted. Capture may ask
    /// the desktop's consent once more.
    pub fn resume_polling(&self) {
        *self.poll.lock().unwrap_or_else(PoisonError::into_inner) = PollState::default();
        self.coord_capture.allow_capture_again();
    }

    /// Why the poll may not read now, if it may not.
    fn poll_stopped(&self) -> Option<PollStop> {
        if !self.overlays_shown() {
            return Some(PollStop::OverlaysHidden);
        }
        self.poll
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped
    }

    /// Note what a poll came to: an accepted position ends a run of
    /// refused reads; a run of refusals lasting the refusal limit, or a
    /// capture not permitted, stops the poll.
    fn note_poll(&self, accepted: bool, refusal: Option<CoordRefusal>) -> Option<PollStop> {
        let now = self.clock.monotonic();
        let mut poll = self.poll.lock().unwrap_or_else(PoisonError::into_inner);
        if accepted {
            *poll = PollState::default();
            return None;
        }
        if refusal == Some(CoordRefusal::CaptureNotAllowed) {
            poll.stopped = Some(PollStop::CaptureNotAllowed);
            return poll.stopped;
        }
        let since = *poll.failing_since.get_or_insert(now);
        if now - since >= self.settings.poll_refusal_limit.as_secs_f64() {
            poll.stopped = Some(PollStop::NotReading);
        }
        poll.stopped
    }

    /// Observe the current position. Strictly reads the coordinates and
    /// refreshes the run's position so distance and bearing to the active
    /// tree recompute; it never records a visit or advances the route. The
    /// overlay's automatic position poll rides this path, so it reads only
    /// while the route overlays show and the poll has not stopped, and a
    /// position is recorded and announced only when it passes the checks.
    pub async fn update_position(&self) -> Result<PositionUpdate, NavigationError> {
        if let Some(stop) = self.poll_stopped() {
            return Ok(PositionUpdate::PollStopped(stop));
        }
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        let read = self.read_position(&run.planet, Trigger::Automatic).await;
        let _guard = self.operation.lock().await;
        let Checked::Current(_, checked) = self.checked(&run, read).await? else {
            return Ok(PositionUpdate::Superseded);
        };
        let reading = match checked {
            Ok(reading) => reading,
            Err(unread) => {
                let refusal = match unread {
                    Unread::Refused(refusal) => Some(refusal),
                    Unread::Jump | Unread::Stale => None,
                };
                if let Some(stop) = self.note_poll(false, refusal) {
                    return Ok(PositionUpdate::PollStopped(stop));
                }
                return Ok(match unread {
                    Unread::Refused(refusal) => PositionUpdate::NotRead(refusal),
                    Unread::Jump => PositionUpdate::JumpRefused,
                    Unread::Stale => PositionUpdate::Superseded,
                });
            }
        };
        self.note_poll(true, None);
        update_run_position(
            &self.db,
            run.id,
            reading.lon as f64,
            reading.lat as f64,
            reading.read_at,
        )
        .await?;
        let mut refreshed = load_run(&self.db, run.id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        // An observe update never resolves a pending harvest, so keep surfacing
        // it (the automatic updater polls this path every interval).
        self.attach_pending_harvest(&mut refreshed);
        (self.changed)();
        Ok(PositionUpdate::Updated(refreshed))
    }

    /// Record the active tree as visited. When the observed position is
    /// within the arrival tolerance (or `force` is set after the caller
    /// confirms), the active stop is completed, a durable per-pin visit is
    /// written (starting its cooldown), and the route advances. Outside the
    /// tolerance without `force`, the position is refreshed but no visit is
    /// recorded, and `OutOfTolerance` asks the caller to confirm. A
    /// position the checks hold back records nothing: `JumpRefused` tells
    /// the player the position did not confirm.
    pub async fn mark_visited(&self, force: bool) -> Result<PositionUpdate, NavigationError> {
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        let read = self.read_position(&run.planet, Trigger::Player).await;
        let _guard = self.operation.lock().await;
        let Checked::Current(run, checked) = self.checked(&run, read).await? else {
            return Ok(PositionUpdate::Superseded);
        };
        let reading = match checked {
            Ok(reading) => reading,
            Err(Unread::Refused(refusal)) => return Ok(PositionUpdate::NotRead(refusal)),
            Err(Unread::Jump) => return Ok(PositionUpdate::JumpRefused),
            Err(Unread::Stale) => return Ok(PositionUpdate::Superseded),
        };
        // The player read the screen themselves: the poll may read again.
        self.note_poll(true, None);
        let lon = reading.lon as f64;
        let lat = reading.lat as f64;
        let at = reading.read_at;
        let Some(active) = run.active_stop() else {
            update_run_position(&self.db, run.id, lon, lat, at).await?;
            (self.changed)();
            return Ok(PositionUpdate::NoActiveRun);
        };
        let observed_distance = distance((lon, lat), (active.lon, active.lat));
        if observed_distance > ARRIVAL_TOLERANCE_UNITS && !force {
            update_run_position(&self.db, run.id, lon, lat, at).await?;
            let refreshed = load_run(&self.db, run.id)
                .await?
                .ok_or(NavigationError::NoActiveRun)?;
            (self.changed)();
            return Ok(PositionUpdate::OutOfTolerance(refreshed));
        }
        let refreshed = self
            .record_active_visit(run.id, active, Some((lon, lat)), "manual", at)
            .await?;
        Ok(PositionUpdate::Updated(refreshed))
    }

    /// The interact key was pressed on a route planned to visit on it: the
    /// player has started cutting the tree the route points at, so record the
    /// active tree. No harvest is waited for and no arrival radius applies.
    /// The press itself is the evidence of the visit: when the position
    /// does not read, does not pass the checks, or is older than one
    /// recorded meanwhile, the visit is recorded without a position rather
    /// than with a remembered one, so a press is never lost to the read. A
    /// press for a route that ended while its position was read is
    /// dropped with the route. Every distinct press counts, however close in
    /// time and space to the last: in a dense stand the next tree can be a
    /// step away. Only a held key's autorepeat is folded into one press, at
    /// the dispatch.
    pub async fn on_visit_key(&self) {
        let _ = self.key_visit().await;
    }

    async fn key_visit(&self) -> Result<Option<NavigationRun>, NavigationError> {
        let pressed_at = naive_to_epoch(self.clock.now());
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(None);
        };
        if !run.visit_on_key || run.active_stop().is_none() {
            return Ok(None);
        }
        let read = self.read_position(&run.planet, Trigger::Automatic).await;
        let _guard = self.operation.lock().await;
        let Checked::Current(run, checked) = self.checked(&run, read).await? else {
            tracing::info!(
                target: "eo::navigation",
                "an interact-key press landed after its route ended; not recorded"
            );
            return Ok(None);
        };
        let Some(active) = run.active_stop() else {
            return Ok(None);
        };
        let (observed, at) = match checked {
            Ok(reading) => (
                Some((reading.lon as f64, reading.lat as f64)),
                reading.read_at,
            ),
            Err(_) => (None, pressed_at),
        };
        self.record_active_visit(run.id, active, observed, "key", at)
            .await
            .map(Some)
    }

    /// Whether the run's latest visit from `source` with a position read was
    /// recorded moments ago within the arrival radius of `observed`: the
    /// same tree signalled again.
    async fn repeats_recent_visit(
        &self,
        run_id: i64,
        source: &'static str,
        observed: (f64, f64),
        now: f64,
    ) -> Result<bool, NavigationError> {
        let recent = self
            .db
            .with_reader(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT visited_at, observed_lon, observed_lat FROM map_pin_visits WHERE run_id = ?1 AND source = ?2 AND position_read = 1 ORDER BY visited_at DESC, id DESC LIMIT 1",
                        rusqlite::params![run_id, source],
                        |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?, row.get::<_, f64>(2)?)),
                    )
                    .optional()?)
            })
            .await?;
        Ok(
            recent.is_some_and(|(visited_at, previous_lon, previous_lat)| {
                now - visited_at <= REPEAT_VISIT_WINDOW_SECONDS
                    && distance(observed, (previous_lon, previous_lat)) <= ARRIVAL_TOLERANCE_UNITS
            }),
        )
    }

    /// Complete the active stop at `at`: the stop is marked visited, a
    /// durable per-pin visit is written (starting its cooldown), and the
    /// route advances to the next stop or completes. A position read for
    /// this visit (`observed`, already through the checks) also becomes
    /// the route's position; without one, no position is claimed (see
    /// [`write_visit`]).
    async fn record_active_visit(
        &self,
        run_id: i64,
        stop: &NavigationStop,
        observed: Option<(f64, f64)>,
        source: &'static str,
        at: f64,
    ) -> Result<NavigationRun, NavigationError> {
        let visit = Visit {
            stop_id: stop.id,
            pin_id: stop.pin_id,
            pin: (stop.lon, stop.lat),
            observed: observed.map(|(lon, lat)| Observed {
                lon,
                lat,
                distance: distance((lon, lat), (stop.lon, stop.lat)),
            }),
            source: source.to_string(),
            outcome: source.to_string(),
            at,
        };
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Some(observed) = visit.observed {
                    tx.execute(
                        "UPDATE navigation_runs SET current_lon = ?2, current_lat = ?3, last_position_at = ?4, updated_at = ?4 WHERE id = ?1",
                        rusqlite::params![run_id, observed.lon, observed.lat, visit.at],
                    )?;
                }
                write_visit(&tx, run_id, &visit)?;
                activate_next_or_complete(&tx, run_id, visit.at)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_ended();
        }
        (self.changed)();
        Ok(refreshed)
    }

    async fn apply_position(
        &self,
        run: NavigationRun,
        (lon, lat): (f64, f64),
        now: f64,
        source: &str,
        outcome: &str,
    ) -> Result<PositionUpdate, NavigationError> {
        if source == "harvest"
            && self
                .repeats_recent_visit(run.id, "harvest", (lon, lat), now)
                .await?
        {
            update_run_position(&self.db, run.id, lon, lat, now).await?;
            let refreshed = load_run(&self.db, run.id)
                .await?
                .ok_or(NavigationError::NoActiveRun)?;
            return Ok(PositionUpdate::Updated(refreshed));
        }
        let matches: Vec<_> = run
            .stops
            .iter()
            .filter(|stop| matches!(stop.status, StopStatus::Active | StopStatus::Pending))
            .filter_map(|stop| {
                let distance = distance((lon, lat), (stop.lon, stop.lat));
                (distance <= ARRIVAL_TOLERANCE_UNITS).then_some((stop.id, stop.pin_id, distance))
            })
            .collect();
        let active_stop_id = run.active_stop().map(|stop| stop.id);
        // A harvest proves arrival at a tree. When several route trees are
        // within the arrival radius, assume the one the route currently points
        // at (the active stop); a single unambiguous non-active match is an
        // out-of-order arrival. Several non-active matches with no active among
        // them stay ambiguous and touch nothing.
        let matched = active_stop_id
            .and_then(|id| {
                matches
                    .iter()
                    .find(|(stop_id, _, _)| *stop_id == id)
                    .copied()
            })
            .or_else(|| (matches.len() == 1).then(|| matches[0]));
        if matched.is_none() && matches.len() > 1 {
            update_run_position(&self.db, run.id, lon, lat, now).await?;
            let refreshed = load_run(&self.db, run.id)
                .await?
                .ok_or(NavigationError::NoActiveRun)?;
            return Ok(PositionUpdate::Ambiguous(refreshed));
        }
        // A harvest with nothing inside the arrival radius is not noise: EU trees
        // cut from well beyond it. Rather than silently dropping the swing, stash
        // a confirmation proposing the active tree for the overlay to resolve.
        if source == "harvest" && matched.is_none() {
            if let Some(active) = run.active_stop() {
                let stop_id = active.id;
                let pin_id = active.pin_id;
                let name = active.name.clone();
                let observed = Observed {
                    lon,
                    lat,
                    distance: distance((lon, lat), (active.lon, active.lat)),
                };
                update_run_position(&self.db, run.id, lon, lat, now).await?;
                *self.pending_harvest.lock().expect("pending harvest") = Some(PendingHarvest {
                    stop_id,
                    pin_id,
                    name,
                    observed: Some(observed),
                    at: now,
                    outcome: outcome.to_string(),
                });
                let refreshed = load_run(&self.db, run.id)
                    .await?
                    .ok_or(NavigationError::NoActiveRun)?;
                return Ok(PositionUpdate::Updated(refreshed));
            }
        }
        let source = source.to_string();
        let outcome = outcome.to_string();
        let arrived_out_of_order = matched
            .map(|(stop_id, _, _)| Some(stop_id) != active_stop_id)
            .unwrap_or(false);
        let run_id = run.id;
        self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_runs SET current_lon = ?2, current_lat = ?3, last_position_at = ?4, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![run_id, lon, lat, now],
            )?;
            if let Some((stop_id, pin_id, observed_distance)) = matched {
                tx.execute(
                    "UPDATE navigation_stops SET status = 'visited', completed_at = ?2, completion_source = ?3, observed_lon = ?4, observed_lat = ?5, observed_distance = ?6 WHERE id = ?1",
                    rusqlite::params![stop_id, now, source, lon, lat, observed_distance],
                )?;
                tx.execute(
                    "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![pin_id, run_id, now, source, outcome, lon, lat, observed_distance],
                )?;
                activate_next_or_complete(&tx, run_id, now)?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        if arrived_out_of_order {
            return Ok(PositionUpdate::Updated(self.replan_locked(run_id).await?));
        }
        let refreshed = load_run(&self.db, run.id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_ended();
        }
        Ok(PositionUpdate::Updated(refreshed))
    }

    pub async fn skip(&self) -> Result<NavigationRun, NavigationError> {
        self.complete_active("skipped", "manual", "unavailable")
            .await
    }

    /// Manually put a tree on cooldown from the map, whether or not a route is
    /// live. Records a cooldown visit at the pin's own position (so the next
    /// planned route excludes it), and if the live route holds the pin as an
    /// active or pending stop, skips that stop and replans the remainder.
    pub async fn cooldown_pin(&self, pin_id: i64) -> Result<(), NavigationError> {
        let _guard = self.operation.lock().await;
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    // Put on cooldown from the map: no position was read,
                    // so the pin's own stands in, marked as not read.
                    "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance, position_read) \
                     SELECT id, NULL, ?2, 'manual', 'manual', lon, lat, 0, 0 FROM map_pins WHERE id = ?1",
                    rusqlite::params![pin_id, now],
                )?)
            })
            .await?;
        if let Some(run) = load_live_run(&self.db).await? {
            let stop = run.stops.iter().find(|stop| {
                stop.pin_id == pin_id
                    && matches!(stop.status, StopStatus::Active | StopStatus::Pending)
            });
            if let Some(stop) = stop {
                let was_active = stop.status == StopStatus::Active;
                let stop_id = stop.id;
                let run_id = run.id;
                self.db
                    .with_writer(move |conn| {
                        let tx = conn.transaction()?;
                        tx.execute(
                            "UPDATE navigation_stops SET status = 'skipped', completed_at = ?2, completion_source = 'manual' WHERE id = ?1",
                            rusqlite::params![stop_id, now],
                        )?;
                        if was_active {
                            activate_next_or_complete(&tx, run_id, now)?;
                        }
                        tx.commit()?;
                        Ok(())
                    })
                    .await?;
                self.replan_or_complete(run_id, now).await?;
            }
        }
        (self.changed)();
        Ok(())
    }

    /// After a pin was deleted from the map, drop its orphaned active/pending
    /// stop from the live route (foreign keys are off, so the row lingers) and
    /// replan the remainder. A no-op when the pin was not a live route stop.
    pub async fn replan_after_pin_removed(&self, pin_id: i64) -> Result<(), NavigationError> {
        let _guard = self.operation.lock().await;
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(());
        };
        let run_id = run.id;
        let removed = self
            .db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    "DELETE FROM navigation_stops WHERE run_id = ?1 AND pin_id = ?2 AND status IN ('active', 'pending')",
                    rusqlite::params![run_id, pin_id],
                )?)
            })
            .await?;
        if removed == 0 {
            return Ok(());
        }
        let now = naive_to_epoch(self.clock.now());
        self.replan_or_complete(run_id, now).await?;
        (self.changed)();
        Ok(())
    }

    /// Replan the live route from the current position, or complete it when no
    /// active/pending stop remains. The caller holds the operation lock.
    async fn replan_or_complete(&self, run_id: i64, now: f64) -> Result<(), NavigationError> {
        let run = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        let has_target = run
            .stops
            .iter()
            .any(|stop| matches!(stop.status, StopStatus::Active | StopStatus::Pending));
        if has_target {
            self.replan_locked(run_id).await?;
        } else {
            self.db
                .with_writer(move |conn| {
                    Ok(conn.execute(
                        "UPDATE navigation_runs SET status = 'completed', updated_at = ?2 WHERE id = ?1",
                        rusqlite::params![run_id, now],
                    )?)
                })
                .await?;
            self.route_ended();
        }
        Ok(())
    }

    async fn complete_active(
        &self,
        status: &str,
        source: &str,
        _outcome: &str,
    ) -> Result<NavigationRun, NavigationError> {
        let _guard = self.operation.lock().await;
        let run = load_live_run(&self.db)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        let active = run.active_stop().ok_or(NavigationError::NoActiveRun)?;
        let now = naive_to_epoch(self.clock.now());
        let run_id = run.id;
        let stop_id = active.id;
        let status = status.to_string();
        let source = source.to_string();
        self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_stops SET status = ?2, completed_at = ?3, completion_source = ?4 WHERE id = ?1",
                rusqlite::params![stop_id, status, now, source],
            )?;
            activate_next_or_complete(&tx, run_id, now)?;
            tx.commit()?;
            Ok(())
        }).await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_ended();
        }
        (self.changed)();
        Ok(refreshed)
    }

    pub async fn undo(&self) -> Result<NavigationRun, NavigationError> {
        let _guard = self.operation.lock().await;
        let run = load_live_run_or_latest(&self.db)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        let run_id = run.id;
        let now = naive_to_epoch(self.clock.now());
        self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            let completed: Option<i64> = tx.query_row(
                "SELECT id FROM navigation_stops WHERE run_id = ?1 AND status IN ('visited', 'skipped') ORDER BY completed_at DESC, ordinal DESC LIMIT 1",
                [run_id], |row| row.get(0)).optional()?;
            let Some(stop_id) = completed else { return Ok(()) };
            tx.execute("UPDATE navigation_stops SET status = 'pending' WHERE run_id = ?1 AND status = 'active'", [run_id])?;
            tx.execute("UPDATE navigation_stops SET status = 'active', completed_at = NULL, completion_source = NULL, observed_lon = NULL, observed_lat = NULL, observed_distance = NULL WHERE id = ?1", [stop_id])?;
            tx.execute("DELETE FROM map_pin_visits WHERE id = (SELECT id FROM map_pin_visits WHERE run_id = ?1 ORDER BY visited_at DESC, id DESC LIMIT 1)", [run_id])?;
            tx.execute("UPDATE navigation_runs SET status = 'active', updated_at = ?2 WHERE id = ?1", rusqlite::params![run_id, now])?;
            tx.commit()?;
            Ok(())
        }).await?;
        self.route_live
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.claim_input();
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        (self.changed)();
        Ok(refreshed)
    }

    async fn replan_locked(&self, run_id: i64) -> Result<NavigationRun, NavigationError> {
        let run = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        let remaining: Vec<Candidate> = run
            .stops
            .iter()
            .filter(|stop| matches!(stop.status, StopStatus::Active | StopStatus::Pending))
            .map(|stop| Candidate {
                id: stop.pin_id,
                lon: stop.lon,
                lat: stop.lat,
            })
            .collect();
        let route = optimise_open_route(
            (run.current_lon, run.current_lat),
            &remaining,
            remaining.len(),
        );
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let completed_ids = {
                    let mut statement = tx.prepare(
                        "SELECT id FROM navigation_stops WHERE run_id = ?1 AND status IN ('visited', 'skipped') ORDER BY completed_at, id",
                    )?;
                    let rows = statement.query_map([run_id], |row| row.get::<_, i64>(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                tx.execute(
                    "UPDATE navigation_stops SET ordinal = ordinal + 10000 WHERE run_id = ?1",
                    [run_id],
                )?;
                for (index, stop_id) in completed_ids.iter().enumerate() {
                    tx.execute(
                        "UPDATE navigation_stops SET ordinal = ?2 WHERE id = ?1",
                        rusqlite::params![stop_id, index as i64],
                    )?;
                }
                for (index, candidate) in route.iter().enumerate() {
                    tx.execute(
                        "UPDATE navigation_stops SET ordinal = ?3, status = ?4 WHERE run_id = ?1 AND pin_id = ?2",
                        rusqlite::params![run_id, candidate.id, (completed_ids.len() + index) as i64, if index == 0 { "active" } else { "pending" }],
                    )?;
                }
                tx.execute(
                    "UPDATE navigation_runs SET updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![run_id, now],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        Ok(refreshed)
    }

    pub async fn end(&self) -> Result<(), NavigationError> {
        let _guard = self.operation.lock().await;
        let now = naive_to_epoch(self.clock.now());
        self.db.with_writer(move |conn| {
            conn.execute("UPDATE navigation_runs SET status = 'ended', updated_at = ?1 WHERE status IN ('active', 'paused', 'completed')", [now])?;
            Ok(())
        }).await?;
        self.route_ended();
        (self.changed)();
        Ok(())
    }

    pub async fn on_harvest(&self, success: bool) {
        if !self.route_live.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let _ = self
            .harvest_position(if success { "success" } else { "failed" })
            .await;
    }

    /// Resolve a pending far-harvest confirmation. `confirm` records the proposed
    /// (active) tree as harvested using the stashed observation and advances the
    /// route; a dismissal leaves the route untouched. Either way the pending
    /// proposal is cleared.
    pub async fn resolve_harvest(&self, confirm: bool) -> Result<NavigationRun, NavigationError> {
        let _guard = self.operation.lock().await;
        let pending = self.pending_harvest.lock().expect("pending harvest").take();
        let Some(run) = load_live_run(&self.db).await? else {
            return Err(NavigationError::NoActiveRun);
        };
        // Dismissed, nothing pending, or the proposal no longer names the active
        // stop: leave the route as it is.
        let record = pending.filter(|pending| {
            confirm && run.active_stop().map(|stop| stop.id) == Some(pending.stop_id)
        });
        let Some(pending) = record else {
            let refreshed = load_run(&self.db, run.id)
                .await?
                .ok_or(NavigationError::NoActiveRun)?;
            (self.changed)();
            return Ok(refreshed);
        };
        // The visit is as of the harvest; the route's position was recorded
        // then, when it read, and a poll may have moved it on since.
        let run_id = run.id;
        let pin = run
            .stops
            .iter()
            .find(|stop| stop.id == pending.stop_id)
            .map_or((0.0, 0.0), |stop| (stop.lon, stop.lat));
        let visit = Visit {
            stop_id: pending.stop_id,
            pin_id: pending.pin_id,
            pin,
            observed: pending.observed,
            source: "harvest".to_string(),
            outcome: pending.outcome,
            at: pending.at,
        };
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                write_visit(&tx, run_id, &visit)?;
                activate_next_or_complete(&tx, run_id, visit.at)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_ended();
        }
        (self.changed)();
        Ok(refreshed)
    }

    /// The automatic arrival path: a harvest swing proves arrival. Reads
    /// the current position and runs full arrival matching (debounce,
    /// ambiguity safety, out-of-order replan) so a matched pending stop is
    /// recorded and the route advances without blocking tracker processing.
    /// A harvest whose position does not read or does not pass the checks
    /// keeps its event: it proposes the active tree for the player to
    /// confirm, without a position.
    async fn harvest_position(&self, outcome: &str) -> Result<PositionUpdate, NavigationError> {
        let harvested_at = naive_to_epoch(self.clock.now());
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        // A route that visits on the interact key takes the key as the only
        // visit signal; harvests leave it untouched.
        if run.visit_on_key {
            return Ok(PositionUpdate::Updated(run));
        }
        let read = self.read_position(&run.planet, Trigger::Automatic).await;
        let _guard = self.operation.lock().await;
        let Checked::Current(run, checked) = self.checked(&run, read).await? else {
            return Ok(PositionUpdate::Superseded);
        };
        let updated = match checked {
            Ok(reading) => {
                self.apply_position(
                    *run,
                    (reading.lon as f64, reading.lat as f64),
                    reading.read_at,
                    "harvest",
                    outcome,
                )
                .await?
            }
            Err(unread) => {
                if let Some(active) = run.active_stop() {
                    *self.pending_harvest.lock().expect("pending harvest") = Some(PendingHarvest {
                        stop_id: active.id,
                        pin_id: active.pin_id,
                        name: active.name.clone(),
                        observed: None,
                        at: harvested_at,
                        outcome: outcome.to_string(),
                    });
                }
                match unread {
                    Unread::Refused(refusal) => PositionUpdate::NotRead(refusal),
                    Unread::Jump => PositionUpdate::JumpRefused,
                    Unread::Stale => PositionUpdate::Superseded,
                }
            }
        };
        (self.changed)();
        Ok(updated)
    }

    pub fn radar_calibration_start(&self) -> RadarCalibrationPhase {
        *self.radar_phase.lock().expect("radar phase") = RadarCalibrationPhase::AwaitCentre;
        self.set_radar_listener_enabled(true);
        RadarCalibrationPhase::AwaitCentre
    }

    pub fn radar_calibration_cancel(&self) {
        *self.radar_phase.lock().expect("radar phase") = RadarCalibrationPhase::Idle;
        self.set_radar_listener_enabled(false);
    }

    pub fn radar_calibration_phase(&self) -> RadarCalibrationPhase {
        *self.radar_phase.lock().expect("radar phase")
    }

    fn radar_calibration_active(&self) -> bool {
        !matches!(self.radar_calibration_phase(), RadarCalibrationPhase::Idle)
    }

    /// Attach radar calibration to the same gated, press-edge Enter listener
    /// used by coordinate-boundary calibration. Its callback explicitly
    /// re-enters the composition-time runtime from the plain dispatch thread.
    pub fn attach_radar_confirm_listener(
        self: &Arc<Self>,
        source: Option<Arc<dyn KeystrokeSource>>,
        runtime: tokio::runtime::Handle,
    ) -> Arc<CoordConfirmListener> {
        let active = Arc::downgrade(self);
        let confirm = Arc::downgrade(self);
        let listener = CoordConfirmListener::new_with_handler(
            Arc::new(move || {
                active
                    .upgrade()
                    .is_some_and(|service| service.radar_calibration_active())
            }),
            Arc::new(move || {
                let Some(service) = confirm.upgrade() else {
                    return;
                };
                runtime.spawn(async move { service.radar_confirm().await });
            }),
            source,
        );
        *self
            .radar_confirm_listener
            .lock()
            .expect("radar listener slot") = Some(Arc::downgrade(&listener));
        listener
    }

    fn set_radar_listener_enabled(&self, enabled: bool) {
        let listener = self
            .radar_confirm_listener
            .lock()
            .expect("radar listener slot")
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        if let Some(listener) = listener {
            listener.set_enabled(enabled);
        }
    }

    pub async fn radar_confirm(&self) {
        let Some(cursor) = self.coord_capture.cursor_position() else {
            self.radar_calibration_cancel();
            return;
        };
        let phase = self.radar_calibration_phase();
        match phase {
            RadarCalibrationPhase::Idle => {}
            RadarCalibrationPhase::AwaitCentre => {
                *self.radar_phase.lock().expect("radar phase") =
                    RadarCalibrationPhase::AwaitNorthEdge { centre: cursor };
            }
            RadarCalibrationPhase::AwaitNorthEdge { centre } => {
                let radius = distance(
                    (centre.0 as f64, centre.1 as f64),
                    (cursor.0 as f64, cursor.1 as f64),
                );
                if radius < 8.0 {
                    return;
                }
                let now = naive_to_epoch(self.clock.now());
                let saved = self.db.with_writer(move |conn| {
                    conn.execute(
                        "INSERT INTO radar_calibration (singleton, centre_x, centre_y, north_x, north_y, radius_px, display_scale, updated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5, 1.0, ?6) ON CONFLICT(singleton) DO UPDATE SET centre_x = excluded.centre_x, centre_y = excluded.centre_y, north_x = excluded.north_x, north_y = excluded.north_y, radius_px = excluded.radius_px, display_scale = excluded.display_scale, updated_at = excluded.updated_at",
                        rusqlite::params![centre.0, centre.1, cursor.0, cursor.1, radius, now],
                    )?;
                    Ok(())
                }).await;
                *self.radar_phase.lock().expect("radar phase") = RadarCalibrationPhase::Idle;
                self.set_radar_listener_enabled(false);
                if saved.is_ok() {
                    *self
                        .calibration
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(RadarGeometry {
                        centre_x: centre.0,
                        centre_y: centre.1,
                        north_x: cursor.0,
                        north_y: cursor.1,
                        radius_px: radius,
                        display_scale: 1.0,
                    });
                    self.guidance_check_now();
                }
            }
        }
    }

    pub async fn radar_geometry(&self) -> Result<Option<RadarGeometry>, DbError> {
        self.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT centre_x, centre_y, north_x, north_y, radius_px, display_scale FROM radar_calibration WHERE singleton = 1",
                [], |row| Ok(RadarGeometry { centre_x: row.get(0)?, centre_y: row.get(1)?, north_x: row.get(2)?, north_y: row.get(3)?, radius_px: row.get(4)?, display_scale: row.get(5)? }))
                .optional()?)
        }).await
    }

    // ── Radar guidance ─────────────────────────────────────────────────

    /// Where guidance stands.
    pub fn radar_guidance(&self) -> RadarGuidance {
        let state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
        guidance_of(&state.lifecycle)
    }

    /// The sink told whenever guidance's status changes, so the overlays
    /// can follow the radar.
    pub fn set_guidance_changed(&self, sink: ChangedSink) {
        *self
            .guidance_changed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Tell the sink iff the status moved materially since it was last
    /// told: a placement within a pixel of the last one announced is the
    /// same placement. The compare-and-advance happens under the lock, so
    /// two threads cannot both announce the same change.
    fn announce_guidance(&self) {
        {
            let mut state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
            let status = guidance_of(&state.lifecycle);
            if !moved(&state.last_announced, &status) {
                return;
            }
            state.last_announced = status;
        }
        let sink = self
            .guidance_changed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            sink();
        }
    }

    /// Start radar guidance for the live route: one worker finds the radar
    /// now and looks again at the configured interval. Already running, it
    /// looks again at once. Nothing starts without a live route, without the
    /// route overlays showing, or when the radar could not be placed at all
    /// (no interface profile and no hand calibration). Waits, briefly, for
    /// the first look, so the overlays open where the radar is.
    pub async fn guidance_start(self: &Arc<Self>) -> RadarGuidance {
        if !self.route_live.load(std::sync::atomic::Ordering::SeqCst) || !self.overlays_shown() {
            return self.radar_guidance();
        }
        if matches!(
            self.guidance
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .lifecycle,
            Guiding::Running { .. }
        ) {
            self.guidance_check_now();
            return self.radar_guidance();
        }
        let calibration = self.radar_geometry().await.ok().flatten();
        *self
            .calibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = calibration;
        let (first_look, looked) = tokio::sync::oneshot::channel();
        let started = {
            let mut state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(state.lifecycle, Guiding::Running { .. })
                || !self.route_live.load(std::sync::atomic::Ordering::SeqCst)
                || !self.overlays_shown()
            {
                // Started by another caller, or the route ended or the
                // overlays went away, while the calibration was read.
                false
            } else if !self.coord_capture.can_sight_radar() && calibration.is_none() {
                state.lifecycle = Guiding::Off {
                    stopped: Some(GuidanceStop::ReaderUnavailable),
                };
                false
            } else {
                let session = state.last_session + 1;
                let (wake, woken) = mpsc::channel();
                *self
                    .guidance_workers
                    .live
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) += 1;
                let token = WorkerToken {
                    service: self.clone(),
                };
                let worker = self.clone();
                let spawned = std::thread::Builder::new()
                    .name("radar-guidance".into())
                    .spawn(move || worker.guide(session, &woken, first_look, token));
                // A worker that did not start dropped its token with the
                // closure, counting itself out; guidance stays off.
                if spawned.is_ok() {
                    state.last_session = session;
                    state.lifecycle = Guiding::Running {
                        session,
                        view: None,
                        wake,
                    };
                }
                spawned.is_ok()
            }
        };
        self.announce_guidance();
        if started {
            let _ = tokio::time::timeout(GUIDANCE_FIRST_LOOK_WAIT, looked).await;
        }
        self.radar_guidance()
    }

    /// Look for the radar now, if guidance is running.
    pub fn guidance_check_now(&self) {
        if let Guiding::Running { wake, .. } = &self
            .guidance
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lifecycle
        {
            let _ = wake.send(());
        }
    }

    /// Stop guidance: the worker captures nothing more and exits.
    pub fn guidance_stop(&self) {
        {
            let mut state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(state.lifecycle, Guiding::Running { .. }) {
                state.lifecycle = Guiding::Off { stopped: None };
            }
        }
        self.announce_guidance();
    }

    /// Stop guidance and wait, for a short while, for its worker to finish
    /// its look: at app exit, where nothing may go on capturing behind the
    /// closing app. The wait is bounded because a worker can be held inside
    /// a capture by the desktop's consent prompt.
    pub fn guidance_shutdown(&self) {
        self.guidance_stop();
        if !self.wait_for_guidance_workers(GUIDANCE_SHUTDOWN_WAIT) {
            tracing::warn!(
                target: "eo::navigation",
                "the radar guidance worker was still capturing at exit; not waiting for it"
            );
        }
    }

    /// Wait until no guidance worker is running; answers whether that
    /// happened within `timeout`.
    pub fn wait_for_guidance_workers(&self, timeout: Duration) -> bool {
        let live = self
            .guidance_workers
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (live, _) = self
            .guidance_workers
            .done
            .wait_timeout_while(live, timeout, |live| *live > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *live == 0
    }

    /// How many looks have been written back since the service started.
    pub fn guidance_looks(&self) -> u64 {
        self.guidance
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .looks
    }

    /// Wait until `looks` looks have been written back, or guidance is no
    /// longer running; answers whether the looks landed within `timeout`.
    /// Woken by each write-back, never by the passing of time.
    pub fn wait_for_guidance_looks(&self, looks: u64, timeout: Duration) -> bool {
        let state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .guidance_looked
            .wait_timeout_while(state, timeout, |state| {
                state.looks < looks && matches!(state.lifecycle, Guiding::Running { .. })
            })
            .unwrap_or_else(PoisonError::into_inner);
        state.looks >= looks
    }

    fn guiding(&self, session: u64) -> bool {
        matches!(
            self.guidance.lock().unwrap_or_else(PoisonError::into_inner).lifecycle,
            Guiding::Running { session: current, .. } if current == session
        )
    }

    /// The worker: look, write back, wait for the interval or a wake, until
    /// guidance for `session` is no longer running. It looks only while
    /// the route is live, and a radar not found for the idle limit ends
    /// it. A look that fails unexpectedly ends guidance with a reason,
    /// never leaves it showing as running.
    fn guide(
        &self,
        session: u64,
        woken: &mpsc::Receiver<()>,
        first_look: tokio::sync::oneshot::Sender<()>,
        _token: WorkerToken,
    ) {
        let mut first_look = Some(first_look);
        let mut sight = Sight::new(self.clock.monotonic());
        loop {
            let step = catch_unwind(AssertUnwindSafe(|| self.guide_step(session, &mut sight)));
            let carry_on = step.unwrap_or_else(|_| {
                tracing::error!(
                    target: "eo::navigation",
                    "looking for the radar failed; guidance stops"
                );
                self.end_guidance(session, Some(GuidanceStop::Failed));
                false
            });
            if let Some(first_look) = first_look.take() {
                let _ = first_look.send(());
            }
            if !carry_on {
                return;
            }
            match woken.recv_timeout(self.settings.guidance_interval) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// One look and its write-back; answers whether to keep looking.
    fn guide_step(&self, session: u64, sight: &mut Sight) -> bool {
        // A capture is taken only for the guidance that is running, for a
        // route that is live: checked here, and again once the capture's
        // turn has come (another read may hold it).
        if !self.guiding(session) {
            return false;
        }
        if !self.route_live.load(std::sync::atomic::Ordering::SeqCst) {
            self.end_guidance(session, None);
            return false;
        }
        let still_wanted =
            || self.guiding(session) && self.route_live.load(std::sync::atomic::Ordering::SeqCst);
        let looked = self.coord_capture.sight_radar(&still_wanted);
        let now = self.clock.monotonic();
        let view = match sight.look(looked, now, self) {
            Ok(view) => view,
            Err(stop) => {
                self.end_guidance(session, stop);
                return false;
            }
        };
        {
            let mut state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
            match &mut state.lifecycle {
                Guiding::Running {
                    session: current,
                    view: held,
                    ..
                } if *current == session => *held = Some(view),
                _ => return false,
            }
        }
        // The look counts as done only once its change has been told:
        // whoever waits on the count then finds the telling already made.
        self.announce_guidance();
        self.guidance
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .looks += 1;
        self.guidance_looked.notify_all();
        true
    }

    /// The hand calibration's placement, if there is one.
    fn calibrated_placement(&self) -> Option<RadarPlacement> {
        let geometry = *self
            .calibration
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        geometry.map(|geometry| {
            let (x, y) = (geometry.centre_x as f64, geometry.centre_y as f64);
            RadarPlacement {
                centre_x: x,
                centre_y: y,
                radius_px: geometry.radius_px,
                clear_right_x: x + self.coord_capture.reach_right(geometry.radius_px),
                source: PlacementSource::Calibrated,
                scale: None,
                scale_missing: None,
                label_strip: self.coord_capture.label_strip(),
            }
        })
    }

    /// End guidance `session`, if it is still running, because of `stop`.
    fn end_guidance(&self, session: u64, stop: Option<GuidanceStop>) {
        {
            let mut state = self.guidance.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(state.lifecycle, Guiding::Running { session: current, .. } if current == session)
            {
                state.lifecycle = Guiding::Off { stopped: stop };
            }
        }
        self.announce_guidance();
        self.guidance_looked.notify_all();
    }
}

/// What one guidance run has seen of the radar, across its looks.
struct Sight {
    /// The last placement of the radar found on the screen; once there is
    /// one, the hand calibration no longer stands in.
    found: Option<RadarPlacement>,
    /// Looks in a row that missed a radar that was found.
    misses: u32,
    /// When the radar was last found, or guidance started, on the
    /// monotonic clock.
    last_seen: f64,
    /// Looks in a row that found the radar without a scale, and why.
    unscaled: (u32, Option<ScaleMissing>),
}

impl Sight {
    fn new(now: f64) -> Self {
        Self {
            found: None,
            misses: 0,
            last_seen: now,
            unscaled: (0, None),
        }
    }

    /// What `looked` comes to, given what was seen before; `Err` when
    /// guidance must stop, with the reason it stops for.
    fn look(
        &mut self,
        looked: Result<crate::coord_capture::RadarSighting, SightingFailure>,
        now: f64,
        service: &NavigationService,
    ) -> Result<Look, Option<GuidanceStop>> {
        let loss = match looked {
            Ok(sighting) => {
                let (x, y) = sighting.centre;
                let (scale, missing) = match sighting.scale {
                    ScaleReading::Read(scale) => (Some(scale), None),
                    ScaleReading::LabelsUnread => (None, Some(ScaleMissing::LabelsUnread)),
                    ScaleReading::NoRings => (None, Some(ScaleMissing::NoRings)),
                    ScaleReading::NoReader => (None, Some(ScaleMissing::NoReader)),
                };
                // Why the scale is missing is said only once two looks in a
                // row agree on it.
                self.unscaled = match (missing, self.unscaled) {
                    (None, _) => (0, None),
                    (Some(why), (count, Some(was))) if was == why => (count + 1, Some(why)),
                    (Some(why), _) => (1, Some(why)),
                };
                let placement = RadarPlacement {
                    centre_x: x,
                    centre_y: y,
                    radius_px: sighting.radius,
                    clear_right_x: x + service.coord_capture.reach_right(sighting.radius),
                    source: PlacementSource::Found,
                    scale,
                    scale_missing: (self.unscaled.0 >= 2).then_some(self.unscaled.1).flatten(),
                    label_strip: service.coord_capture.label_strip(),
                };
                self.found = Some(placement);
                self.misses = 0;
                self.last_seen = now;
                return Ok(Look::Placed(placement));
            }
            Err(SightingFailure::Withdrawn) => return Err(None),
            Err(SightingFailure::Frame(FrameUnavailable::CaptureNotStarted)) => {
                return Err(Some(GuidanceStop::CaptureNotAllowed))
            }
            Err(SightingFailure::RadarNotFound) => GuidanceLoss::RadarNotFound,
            Err(SightingFailure::ReaderUnavailable) => GuidanceLoss::RadarNotFound,
            Err(SightingFailure::Frame(unavailable)) => match unavailable {
                FrameUnavailable::OutsideCapturableArea => GuidanceLoss::GameWindowOffScreen,
                FrameUnavailable::CaptureFailed => GuidanceLoss::CaptureFailed,
                FrameUnavailable::WindowNotFound | FrameUnavailable::CaptureNotStarted => {
                    GuidanceLoss::GameWindowNotFound
                }
            },
        };
        if now - self.last_seen >= service.settings.guidance_idle_limit.as_secs_f64() {
            return Err(Some(GuidanceStop::RadarNotSeen));
        }
        match self.found {
            // A radar that was found keeps its place until the loss holds.
            Some(found) => {
                self.misses += 1;
                if self.misses < service.settings.guidance_loss_looks {
                    Ok(Look::Placed(found))
                } else {
                    Ok(Look::Lost(loss))
                }
            }
            // Never found on this run: the hand calibration stands in, but
            // it is not the radar seen.
            None => match service.calibrated_placement() {
                Some(calibrated) if loss == GuidanceLoss::RadarNotFound => {
                    Ok(Look::Placed(calibrated))
                }
                _ if service.coord_capture.can_sight_radar() => Ok(Look::Lost(loss)),
                _ => Err(Some(GuidanceStop::ReaderUnavailable)),
            },
        }
    }
}

/// Whether `next` differs from `last` enough to tell the overlays: any
/// change of state or reason, or a placement moved or resized by more than
/// a pixel, or with another scale.
fn moved(last: &RadarGuidance, next: &RadarGuidance) -> bool {
    match (last, next) {
        (RadarGuidance::Placed(a), RadarGuidance::Placed(b)) => {
            (a.centre_x - b.centre_x).abs() > 1.0
                || (a.centre_y - b.centre_y).abs() > 1.0
                || (a.radius_px - b.radius_px).abs() > 1.0
                || a.source != b.source
                || !same_scale(a.scale, b.scale)
                || a.scale_missing != b.scale_missing
        }
        _ => last != next,
    }
}

/// Whether two scales are the same to the overlay: both absent, or edge
/// ranges within a hundredth of each other.
fn same_scale(a: Option<RadarScale>, b: Option<RadarScale>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a.edge_range_m / b.edge_range_m - 1.0).abs() <= 0.01,
        _ => false,
    }
}

/// Guidance's status, from its lifecycle.
fn guidance_of(lifecycle: &Guiding) -> RadarGuidance {
    match lifecycle {
        Guiding::Off { stopped } => RadarGuidance::Off { stopped: *stopped },
        Guiding::Running { view: None, .. } => RadarGuidance::Searching,
        Guiding::Running {
            view: Some(Look::Placed(placement)),
            ..
        } => RadarGuidance::Placed(*placement),
        Guiding::Running {
            view: Some(Look::Lost(loss)),
            ..
        } => RadarGuidance::Lost(*loss),
    }
}

/// One visit to record.
struct Visit {
    stop_id: i64,
    pin_id: i64,
    /// The visited pin's own position.
    pin: (f64, f64),
    /// Where the player was read to be; `None` when no position was read.
    observed: Option<Observed>,
    source: String,
    outcome: String,
    at: f64,
}

/// Mark a visit's stop visited and write its durable visit row. Whether a
/// position was read under the radar or from the hand-drawn region is not
/// kept. A visit without a position read claims none: the stop's observed position is
/// left empty, and the visit row, whose observed columns cannot be empty,
/// stores the pin's own position and a distance of 0 as a placeholder, not
/// an observation, with `position_read = 0`; every reader of those columns
/// uses only rows with `position_read = 1`.
fn write_visit(
    conn: &rusqlite::Connection,
    run_id: i64,
    visit: &Visit,
) -> Result<(), rusqlite::Error> {
    let (observed, placeholder) = match visit.observed {
        Some(observed) => (
            Some(observed),
            (observed.lon, observed.lat, observed.distance),
        ),
        None => (None, (visit.pin.0, visit.pin.1, 0.0)),
    };
    conn.execute(
        "UPDATE navigation_stops SET status = 'visited', completed_at = ?2, completion_source = ?3, observed_lon = ?4, observed_lat = ?5, observed_distance = ?6 WHERE id = ?1",
        rusqlite::params![
            visit.stop_id,
            visit.at,
            visit.source,
            observed.map(|o| o.lon),
            observed.map(|o| o.lat),
            observed.map(|o| o.distance)
        ],
    )?;
    conn.execute(
        "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance, position_read) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            visit.pin_id,
            run_id,
            visit.at,
            visit.source,
            visit.outcome,
            placeholder.0,
            placeholder.1,
            placeholder.2,
            observed.is_some()
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
struct Candidate {
    id: i64,
    lon: f64,
    lat: f64,
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn optimise_open_route(start: (f64, f64), pins: &[Candidate], hop_count: usize) -> Vec<Candidate> {
    let mut remaining = pins.to_vec();
    remaining.sort_by_key(|pin| pin.id);
    let mut route: Vec<Candidate> = Vec::with_capacity(hop_count.min(remaining.len()));
    let mut current = start;
    while route.len() < hop_count && !remaining.is_empty() {
        let candidate_index = remaining
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                distance(current, (left.lon, left.lat))
                    .total_cmp(&distance(current, (right.lon, right.lat)))
                    .then_with(|| left.id.cmp(&right.id))
            })
            .map(|(index, _)| index)
            .expect("remaining is non-empty");
        let next = remaining.remove(candidate_index);
        current = (next.lon, next.lat);
        route.push(next);
    }

    // Bounded best-improvement 2-opt. Reversing a segment preserves all
    // internal edge lengths, so only its two boundaries need evaluation.
    // The fixed start and open end are handled explicitly. Thirty-two passes
    // keep a 500-stop plan predictably interactive while admitting successive
    // improvements that a single pass would miss.
    for _ in 0..32 {
        let mut best: Option<(usize, usize, f64)> = None;
        for left in 0..route.len() {
            let previous = if left == 0 {
                start
            } else {
                (route[left - 1].lon, route[left - 1].lat)
            };
            for right in left + 1..route.len() {
                let old_left = distance(previous, (route[left].lon, route[left].lat));
                let new_left = distance(previous, (route[right].lon, route[right].lat));
                let (old_right, new_right) = if right + 1 < route.len() {
                    let next = (route[right + 1].lon, route[right + 1].lat);
                    (
                        distance((route[right].lon, route[right].lat), next),
                        distance((route[left].lon, route[left].lat), next),
                    )
                } else {
                    (0.0, 0.0)
                };
                let gain = old_left + old_right - new_left - new_right;
                if gain > 1e-9 && best.is_none_or(|(_, _, best_gain)| gain > best_gain) {
                    best = Some((left, right, gain));
                }
            }
        }
        let Some((left, right, _)) = best else { break };
        route[left..=right].reverse();
    }
    route
}

async fn load_candidates(
    db: &Db,
    planet: String,
    map_view_id: Option<i64>,
    now: f64,
) -> Result<Vec<Candidate>, DbError> {
    // A freshly regenerated route excludes trees whose latest confirmed visit
    // is still within its cooldown, so the durable per-tree visits are the
    // recovery point after an interruption.
    let cutoff = now - COOLDOWN_SECONDS;
    db.with_reader(move |conn| {
        // Only special-tree pins are route stops: generic markers (vendors,
        // bosses) are not harvestable targets. The inner join on the tree
        // configuration also excludes config-less pins.
        let mut stmt = conn.prepare("SELECT mp.id, mp.lon, mp.lat FROM map_pins mp JOIN pin_configs pc ON pc.id = mp.pin_config_id AND pc.special_kind = 'tree' WHERE mp.planet = ?1 AND ((?2 IS NULL AND mp.map_view_id IS NULL) OR mp.map_view_id = ?2) AND NOT EXISTS (SELECT 1 FROM map_pin_visits v WHERE v.pin_id = mp.id AND v.visited_at >= ?3) ORDER BY mp.id")?;
        let rows = stmt.query_map(rusqlite::params![planet, map_view_id, cutoff], |row| Ok(Candidate { id: row.get(0)?, lon: row.get(1)?, lat: row.get(2)? }))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }).await
}

async fn end_lingering_runs(db: &Db, now: f64) -> Result<(), DbError> {
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE navigation_runs SET status = 'ended', updated_at = ?1 WHERE status IN ('active', 'paused', 'completed')",
            [now],
        )?;
        Ok(())
    })
    .await
}

/// Record the route's position as read at `now`. Whether it was read under
/// the radar or from the player's hand-drawn region is not kept once
/// stored; both passed the same grammar, bounds and position checks.
async fn update_run_position(
    db: &Db,
    id: i64,
    lon: f64,
    lat: f64,
    now: f64,
) -> Result<(), DbError> {
    db.with_writer(move |conn| { conn.execute("UPDATE navigation_runs SET current_lon = ?2, current_lat = ?3, last_position_at = ?4, updated_at = ?4 WHERE id = ?1", rusqlite::params![id, lon, lat, now])?; Ok(()) }).await
}

fn activate_next_or_complete(
    conn: &rusqlite::Connection,
    run_id: i64,
    now: f64,
) -> Result<(), rusqlite::Error> {
    let next: Option<i64> = conn.query_row("SELECT id FROM navigation_stops WHERE run_id = ?1 AND status = 'pending' ORDER BY ordinal LIMIT 1", [run_id], |row| row.get(0)).optional()?;
    if let Some(id) = next {
        conn.execute(
            "UPDATE navigation_stops SET status = 'active' WHERE id = ?1",
            [id],
        )?;
        conn.execute(
            "UPDATE navigation_runs SET updated_at = ?2 WHERE id = ?1",
            rusqlite::params![run_id, now],
        )?;
    } else {
        conn.execute(
            "UPDATE navigation_runs SET status = 'completed', updated_at = ?2 WHERE id = ?1",
            rusqlite::params![run_id, now],
        )?;
    }
    Ok(())
}

async fn load_live_run(db: &Db) -> Result<Option<NavigationRun>, DbError> {
    load_run_where(
        db,
        "WHERE status IN ('active', 'paused') ORDER BY updated_at DESC LIMIT 1",
    )
    .await
}

async fn load_current_run(db: &Db) -> Result<Option<NavigationRun>, DbError> {
    load_run_where(
        db,
        "WHERE status IN ('active', 'paused', 'completed') ORDER BY updated_at DESC LIMIT 1",
    )
    .await
}

async fn load_live_run_or_latest(db: &Db) -> Result<Option<NavigationRun>, DbError> {
    load_run_where(db, "ORDER BY updated_at DESC LIMIT 1").await
}

async fn load_run(db: &Db, id: i64) -> Result<Option<NavigationRun>, DbError> {
    load_run_where(db, &format!("WHERE id = {id}")).await
}

async fn load_run_where(db: &Db, clause: &str) -> Result<Option<NavigationRun>, DbError> {
    let clause = clause.to_string();
    let header = db.with_reader(move |conn| {
        Ok(conn.query_row(&format!("SELECT id, planet, map_view_id, (SELECT name FROM map_views WHERE id = navigation_runs.map_view_id), status, start_lon, start_lat, current_lon, current_lat, last_position_at, hop_count, visit_on_key, updated_at FROM navigation_runs {clause}"), [], |row| {
            let status: String = row.get(4)?;
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, Option<String>>(3)?, status, row.get::<_, f64>(5)?, row.get::<_, f64>(6)?, row.get::<_, f64>(7)?, row.get::<_, f64>(8)?, row.get::<_, Option<f64>>(9)?, row.get::<_, i64>(10)?, row.get::<_, bool>(11)?, row.get::<_, f64>(12)?))
        }).optional()?)
    }).await?;
    let Some((
        id,
        planet,
        map_view_id,
        map_view_name,
        status,
        start_lon,
        start_lat,
        current_lon,
        current_lat,
        last_position_at,
        hop_count,
        visit_on_key,
        updated_at,
    )) = header
    else {
        return Ok(None);
    };
    let stops = db.with_reader(move |conn| {
        let mut stmt = conn.prepare("SELECT s.id, s.pin_id, s.ordinal, s.status, p.name, p.icon, p.lon, p.lat, s.completed_at, s.completion_source FROM navigation_stops s JOIN map_pins p ON p.id = s.pin_id WHERE s.run_id = ?1 ORDER BY s.ordinal")?;
        let rows = stmt.query_map([id], |row| {
            let status: String = row.get(3)?;
            Ok(NavigationStop { id: row.get(0)?, pin_id: row.get(1)?, ordinal: row.get(2)?, status: StopStatus::parse(&status).unwrap_or(StopStatus::Pending), name: row.get(4)?, icon: row.get(5)?, lon: row.get(6)?, lat: row.get(7)?, completed_at: row.get(8)?, completion_source: row.get(9)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }).await?;
    Ok(Some(NavigationRun {
        id,
        planet,
        map_view_id,
        map_view_name,
        status: RunStatus::parse(&status).unwrap_or(RunStatus::Ended),
        start_lon,
        start_lat,
        current_lon,
        current_lat,
        last_position_at,
        hop_count,
        visit_on_key,
        updated_at,
        stops,
        pending_harvest: None,
    }))
}

#[cfg(test)]
mod tests {
    //! Route navigation over a stand-in screen. Positions are read through
    //! the real coordinate grammar from a hand-drawn region whose two
    //! halves the test prints; guidance looks at drawn radars. Time is the
    //! test's: the clock moves only when a test moves it, and guidance
    //! looks only when a test asks, each wait being for a state the service
    //! reaches, never for a length of time.

    use super::*;
    use crate::clock::MockClock;
    use crate::coord_capture::{CoordCaptureProviders, CoordRegion, ValueRefusal};
    use crate::game_ui::geometry::BgrImage;
    use crate::game_ui::seams::{FrameArea, FrameReader, GameFrame};
    use crate::game_ui::test_support::{
        blank, bundled_profile, window_frame, RadarScene, RINGS, RING_LABELS,
    };
    use crate::keystroke_source::MockKeystrokeSource;
    use crate::map_pins::{MapPinsService, NewMapPin};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

    /// Where the drawn planet's coordinates start: the client prints
    /// coordinates of four to six digits, so the route is laid out this far
    /// from zero and read back through the same grammar the game's own
    /// lettering is.
    const ORIGIN: i64 = 50_000;
    const O: f64 = ORIGIN as f64;

    fn at(lon: i64, lat: i64) -> (i64, i64) {
        (ORIGIN + lon, ORIGIN + lat)
    }

    /// A generous bound on any wait for the worker: it is woken by the
    /// state it waits for, and this only keeps a broken service from
    /// hanging the suite.
    const GUARD: Duration = Duration::from_secs(120);

    fn candidate(id: i64, lon: f64, lat: f64) -> Candidate {
        Candidate { id, lon, lat }
    }

    #[test]
    fn optimiser_is_deterministic_bounded_and_keeps_unique_stops() {
        let pins = vec![
            candidate(1, 10.0, 0.0),
            candidate(2, 2.0, 0.0),
            candidate(3, 4.0, 3.0),
            candidate(4, 5.0, 0.0),
        ];
        let a = optimise_open_route((0.0, 0.0), &pins, 3);
        let b = optimise_open_route((0.0, 0.0), &pins, 3);
        assert_eq!(
            a.iter().map(|p| p.id).collect::<Vec<_>>(),
            b.iter().map(|p| p.id).collect::<Vec<_>>()
        );
        assert_eq!(a.len(), 3);
        let ids: std::collections::BTreeSet<_> = a.iter().map(|pin| pin.id).collect();
        assert_eq!(ids.len(), a.len());
    }

    #[test]
    fn arrival_policy_is_euclidean_and_inclusive_at_fifteen_units() {
        assert_eq!(ARRIVAL_TOLERANCE_UNITS, 15.0);
        assert_eq!(distance((0.0, 0.0), (9.0, 12.0)), ARRIVAL_TOLERANCE_UNITS);
        assert!(distance((0.0, 0.0), (9.0, 12.0)) <= ARRIVAL_TOLERANCE_UNITS);
        assert!(distance((0.0, 0.0), (15.01, 0.0)) > ARRIVAL_TOLERANCE_UNITS);
        // The pin-drop duplicate radius is a separate, tighter policy.
        assert_eq!(DUPLICATE_TOLERANCE_UNITS, 5.0);
    }

    /// A game window's pixels and its place on the screen.
    type Shown = (BgrImage, (i64, i64));

    /// A stand-in screen. The game window shows `shown`, read by `reader`;
    /// with no radar on it, positions are read from the hand-drawn region,
    /// whose two halves print `position`. The test decides what is shown,
    /// counts every capture, and can hold captures until it releases them.
    struct Screen {
        shown: Mutex<Result<Shown, FrameUnavailable>>,
        reader: Mutex<Option<FrameReader>>,
        position: Arc<Mutex<(i64, i64)>>,
        /// The longitude half reads with its label misread.
        garble_lon: AtomicBool,
        /// A capture of the game window fails unexpectedly.
        broken: AtomicBool,
        /// Seconds that pass on the clock while the region is recognised.
        read_takes: Mutex<f64>,
        captures: AtomicUsize,
        region_captures: AtomicUsize,
        held: Mutex<bool>,
        released: std::sync::Condvar,
        clock: Arc<MockClock>,
    }

    impl Screen {
        fn new(position: Arc<Mutex<(i64, i64)>>, clock: Arc<MockClock>) -> Arc<Self> {
            Arc::new(Self {
                // A game window with no radar on it.
                shown: Mutex::new(Ok((blank(64, 48, [120, 110, 90]), (0, 0)))),
                reader: Mutex::new(None),
                position,
                garble_lon: AtomicBool::new(false),
                broken: AtomicBool::new(false),
                read_takes: Mutex::new(0.0),
                captures: AtomicUsize::new(0),
                region_captures: AtomicUsize::new(0),
                held: Mutex::new(false),
                released: std::sync::Condvar::new(),
                clock,
            })
        }

        fn show(&self, image: BgrImage, reader: FrameReader, origin: (i64, i64)) {
            *self.shown.lock().unwrap() = Ok((image, origin));
            *self.reader.lock().unwrap() = Some(reader);
        }

        fn show_nothing(&self, why: FrameUnavailable) {
            *self.shown.lock().unwrap() = Err(why);
        }

        fn show_no_radar(&self) {
            *self.shown.lock().unwrap() = Ok((blank(64, 48, [120, 110, 90]), (0, 0)));
        }

        fn captures(&self) -> usize {
            self.captures.load(AtomicOrdering::SeqCst)
        }

        fn hold(&self) {
            *self.held.lock().unwrap() = true;
        }

        fn release(&self) {
            *self.held.lock().unwrap() = false;
            self.released.notify_all();
        }

        fn wait_if_held(&self) {
            let mut held = self.held.lock().unwrap();
            while *held {
                held = self.released.wait(held).unwrap();
            }
        }

        /// A two-column strip whose columns carry the bytes the region's
        /// reader tells its halves apart by.
        fn strip() -> BgrImage {
            BgrImage {
                data: [vec![1u8; 3], vec![2u8; 3], vec![1u8; 3], vec![2u8; 3]].concat(),
                h: 2,
                w: 2,
            }
        }

        fn providers(self: &Arc<Self>) -> CoordCaptureProviders {
            let (window, region, reading, cursor) = (
                self.clone(),
                self.clone(),
                self.clone(),
                self.position.clone(),
            );
            CoordCaptureProviders {
                frame: Arc::new(
                    move |area: FrameArea| -> Result<GameFrame, FrameUnavailable> {
                        window.captures.fetch_add(1, AtomicOrdering::SeqCst);
                        window.wait_if_held();
                        assert!(
                            !window.broken.load(AtomicOrdering::SeqCst),
                            "the capturer failed unexpectedly"
                        );
                        let shown = window.shown.lock().unwrap().clone();
                        shown.and_then(|(image, origin)| window_frame(&image, origin, area))
                    },
                ),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    let half = crop.w == 1 && matches!(crop.data.first(), Some(1 | 2));
                    if !half {
                        let reader = reading.reader.lock().unwrap().clone();
                        return reader.and_then(|reader| reader(crop));
                    }
                    let takes = *reading.read_takes.lock().unwrap();
                    reading.clock.advance(takes).expect("forward");
                    let (lon, lat) = *reading.position.lock().unwrap();
                    let text = if crop.data[0] == 1 {
                        let label = if reading.garble_lon.load(AtomicOrdering::SeqCst) {
                            "L0N"
                        } else {
                            "LON"
                        };
                        format!("{label} {lon}")
                    } else {
                        format!("LAT {lat}")
                    };
                    Some((text, 0.99))
                })),
                planets: Vec::new(),
                region: Arc::new(|| {
                    Some(CoordRegion {
                        x: 0,
                        y: 0,
                        w: 120,
                        h: 20,
                    })
                }),
                capture_region: Arc::new(move |_, _, _, _| {
                    region.region_captures.fetch_add(1, AtomicOrdering::SeqCst);
                    region.wait_if_held();
                    Some(Screen::strip())
                }),
                cursor_position: Arc::new(move || Some(*cursor.lock().unwrap())),
                ..CoordCaptureProviders::default()
            }
        }
    }

    /// Everything a test drives the service through.
    struct Rig {
        dir: tempfile::TempDir,
        service: Arc<NavigationService>,
        screen: Arc<Screen>,
        position: Arc<Mutex<(i64, i64)>>,
        changes: Arc<AtomicUsize>,
        input: Arc<MockKeystrokeSource>,
        clock: Arc<MockClock>,
    }

    impl Rig {
        fn changes(&self) -> usize {
            self.changes.load(AtomicOrdering::SeqCst)
        }

        fn tick(&self, seconds: f64) {
            self.clock.advance(seconds).expect("forward");
        }

        fn stand(&self, (lon, lat): (i64, i64)) {
            *self.position.lock().unwrap() = (lon, lat);
        }

        /// Start a route from the origin, its start read off the screen
        /// the way the route overlay captures it, so the route's first
        /// position is a reading.
        async fn start(&self, pins: Option<Vec<i64>>, visit_on_key: bool) -> NavigationRun {
            self.stand(at(0, 0));
            let bounds = (self.service.bounds)("Calypso");
            self.service
                .coord_capture
                .scan("Calypso", bounds, Trigger::Player)
                .expect("the start reads");
            self.service
                .start("Calypso".into(), None, O, O, pins, visit_on_key)
                .await
                .unwrap()
        }

        async fn recorded(&self) -> (f64, f64, Option<f64>) {
            let run = self.service.snapshot().await.unwrap().unwrap();
            (run.current_lon, run.current_lat, run.last_position_at)
        }

        async fn visits(&self) -> Vec<(i64, String, f64, f64, f64, bool)> {
            self.service
                .db
                .with_reader(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT pin_id, source, observed_lon, observed_lat, observed_distance, position_read FROM map_pin_visits ORDER BY id",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    })?;
                    Ok(rows.collect::<Result<Vec<_>, _>>()?)
                })
                .await
                .unwrap()
        }

        async fn stop_observation(&self, stop: i64) -> Option<(f64, f64, f64)> {
            self.service
                .db
                .with_reader(move |conn| {
                    Ok(conn.query_row(
                        "SELECT observed_lon, observed_lat, observed_distance FROM navigation_stops WHERE id = ?1",
                        [stop],
                        |row| {
                            Ok(row
                                .get::<_, Option<f64>>(0)?
                                .zip(row.get::<_, Option<f64>>(1)?)
                                .zip(row.get::<_, Option<f64>>(2)?)
                                .map(|((lon, lat), d)| (lon, lat, d)))
                        },
                    )?)
                })
                .await
                .unwrap()
        }

        async fn poll(&self) -> PositionUpdate {
            self.service.update_position().await.unwrap()
        }
    }

    type Fixture = (
        tempfile::TempDir,
        Arc<NavigationService>,
        Arc<Mutex<(i64, i64)>>,
        Arc<AtomicUsize>,
        Arc<MockKeystrokeSource>,
    );

    async fn navigation_fixture() -> Fixture {
        let rig = rig().await;
        (rig.dir, rig.service, rig.position, rig.changes, rig.input)
    }

    async fn rig() -> Rig {
        rig_with(None, NavigationSettings::default()).await
    }

    async fn rig_with(focus: Option<GameFocusProbe>, settings: NavigationSettings) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let shared: Arc<dyn Clock> = clock.clone();
        let pins = MapPinsService::new(db.clone(), shared.clone());
        // Route stops are special-tree pins, so seed a tree configuration and
        // make every fixture pin an instance of it.
        let configs = crate::pin_configs::PinConfigsService::new(db.clone(), shared.clone());
        let tree = configs
            .create(crate::pin_configs::NewPinConfig {
                planet: "Calypso".into(),
                map_view_id: None,
                label: "Tree".into(),
                category: "special".into(),
                special_kind: Some("tree".into()),
                icon: "🌳".into(),
                radius_m: None,
                colour: "#22c55e".into(),
                cooldown_colour: Some("#f59e0b".into()),
            })
            .await
            .unwrap();
        // Laid out for the fifteen-unit arrival radius: A and D sit close enough
        // to share a point (the "prefer the active tree" case), while B and C are
        // spread beyond the radius (the out-of-order and ambiguity cases).
        for (name, lon, lat) in [
            ("A", 30.0, 0.0),
            ("D", 39.0, 0.0),
            ("B", 60.0, 9.0),
            ("C", 24.0, 36.0),
        ] {
            pins.create(NewMapPin {
                planet: "Calypso".into(),
                lon: O + lon,
                lat: O + lat,
                altitude: None,
                name: name.into(),
                icon: "🌳".into(),
                kind: "tree".into(),
                radius_m: None,
                notes: None,
                session_id: None,
                map_view_id: None,
                pin_config_id: Some(tree.id),
            })
            .await
            .unwrap();
        }
        let position = Arc::new(Mutex::new(at(0, 0)));
        let screen = Screen::new(position.clone(), clock.clone());
        let changes = Arc::new(AtomicUsize::new(0));
        let input = Arc::new(MockKeystrokeSource::new());
        let service = spawn_navigation(
            db,
            clock.clone(),
            screen.clone(),
            changes.clone(),
            input.clone(),
            focus,
            settings,
        )
        .await;
        // The route overlays are showing, as they are while a route runs.
        service.set_overlays_shown(true);
        Rig {
            dir,
            service,
            screen,
            position,
            changes,
            input,
            clock,
        }
    }

    /// Build a navigation service over `screen`, so a second service can
    /// be spawned on the same database to exercise startup behaviour.
    async fn spawn_navigation(
        db: Db,
        clock: Arc<MockClock>,
        screen: Arc<Screen>,
        changes: Arc<AtomicUsize>,
        input: Arc<MockKeystrokeSource>,
        focus: Option<GameFocusProbe>,
        settings: NavigationSettings,
    ) -> Arc<NavigationService> {
        let shared: Arc<dyn Clock> = clock;
        let capture = CoordCaptureService::new(screen.providers(), shared.clone());
        let change_count = changes.clone();
        NavigationService::new(
            db,
            shared,
            NavigationProviders {
                coord_capture: capture,
                bounds: Arc::new(|_| {
                    Some(CoordinateBounds {
                        lon: (ORIGIN - 9_000, ORIGIN + 9_000),
                        lat: (ORIGIN - 9_000, ORIGIN + 9_000),
                    })
                }),
                changed: Arc::new(move || {
                    change_count.fetch_add(1, AtomicOrdering::SeqCst);
                }),
                input: Some(input),
                focus,
                settings,
            },
        )
        .await
    }

    #[tokio::test]
    async fn custom_selection_is_an_exact_eligible_pin_allow_list() {
        let (dir, service, _position, _changes, _input) = navigation_fixture().await;
        let all = service
            .start("Calypso".into(), None, O, O, None, false)
            .await
            .unwrap();
        let selected = vec![all.stops[1].pin_id, all.stops[3].pin_id];
        let cooled = all.stops[2].pin_id;
        service.end().await.unwrap();
        service.cooldown_pin(cooled).await.unwrap();

        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let configs = crate::pin_configs::PinConfigsService::new(db.clone(), clock.clone());
        let generic = configs
            .create(crate::pin_configs::NewPinConfig {
                planet: "Calypso".into(),
                map_view_id: None,
                label: "Marker".into(),
                category: "generic".into(),
                special_kind: None,
                icon: "x".into(),
                radius_m: None,
                colour: "#38bdf8".into(),
                cooldown_colour: None,
            })
            .await
            .unwrap();
        let generic_pin = MapPinsService::new(db.clone(), clock.clone())
            .create(NewMapPin {
                planet: "Calypso".into(),
                lon: O + 1.0,
                lat: O + 1.0,
                altitude: None,
                name: "Marker".into(),
                icon: "x".into(),
                kind: "marker".into(),
                radius_m: None,
                notes: None,
                session_id: None,
                map_view_id: None,
                pin_config_id: Some(generic.id),
            })
            .await
            .unwrap();

        let custom = service
            .start(
                "Calypso".into(),
                None,
                O,
                O,
                Some(vec![
                    selected[1],
                    selected[0],
                    selected[1],
                    cooled,
                    generic_pin.id,
                    i64::MAX,
                ]),
                false,
            )
            .await
            .unwrap();
        let actual: std::collections::BTreeSet<_> =
            custom.stops.iter().map(|stop| stop.pin_id).collect();
        assert_eq!(actual, selected.into_iter().collect());
        assert_eq!(custom.hop_count, 2);
    }

    #[tokio::test]
    async fn custom_selection_must_not_be_empty() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let error = service
            .start("Calypso".into(), None, O, O, Some(Vec::new()), false)
            .await
            .unwrap_err();
        assert!(matches!(error, NavigationError::EmptyPinSelection));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn radar_calibration_reuses_the_gated_listener_off_the_runtime_thread() {
        let (_dir, service, position, _changes, input) = navigation_fixture().await;
        let listener = service
            .attach_radar_confirm_listener(Some(input.clone()), tokio::runtime::Handle::current());
        *position.lock().unwrap() = (100, 100);
        service.radar_calibration_start();

        let first_input = input.clone();
        std::thread::spawn(move || {
            let now = chrono::Utc::now();
            first_input.inject("return", now, KeystrokeKind::Press);
            first_input.inject("return", now, KeystrokeKind::Release);
        })
        .join()
        .expect("off-runtime input dispatch");
        while !matches!(
            service.radar_calibration_phase(),
            RadarCalibrationPhase::AwaitNorthEdge { .. }
        ) {
            tokio::task::yield_now().await;
        }

        *position.lock().unwrap() = (100, 80);
        let second_input = input.clone();
        std::thread::spawn(move || {
            let now = chrono::Utc::now();
            second_input.inject("return", now, KeystrokeKind::Press);
            second_input.inject("return", now, KeystrokeKind::Release);
        })
        .join()
        .expect("off-runtime input dispatch");
        let geometry = loop {
            if let Some(geometry) = service.radar_geometry().await.unwrap() {
                break geometry;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!((geometry.centre_x, geometry.centre_y), (100, 100));
        assert_eq!((geometry.north_x, geometry.north_y), (100, 80));
        assert_eq!(geometry.radius_px, 20.0);
        while service.radar_calibration_phase() != RadarCalibrationPhase::Idle {
            tokio::task::yield_now().await;
        }
        listener.stop();
    }

    async fn key_visits(rig: &Rig) -> Vec<(i64, f64, f64, bool)> {
        rig.visits()
            .await
            .into_iter()
            .filter(|visit| visit.1 == "key")
            .map(|(pin, _, lon, lat, _, read)| (pin, lon, lat, read))
            .collect()
    }

    async fn await_key_visits(rig: &Rig, count: usize) {
        while key_visits(rig).await.len() < count {
            tokio::task::yield_now().await;
        }
    }

    fn press_off_runtime(input: &Arc<MockKeystrokeSource>, kind: KeystrokeKind) {
        let input = input.clone();
        std::thread::spawn(move || input.inject(VISIT_KEY, chrono::Utc::now(), kind))
            .join()
            .expect("off-runtime key dispatch");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_interact_key_records_the_active_tree_from_the_dispatch_thread() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), true).await;
        assert!(run.visit_on_key);
        let first = run.active_stop().unwrap().clone();
        // Well outside the arrival radius: the key is the whole signal, so no
        // distance check holds the visit back.
        rig.tick(2.0);
        rig.stand(at(90, 90));

        press_off_runtime(&rig.input, KeystrokeKind::Press);
        await_key_visits(&rig, 1).await;

        let snapshot = rig.service.snapshot().await.unwrap().unwrap();
        let recorded = snapshot
            .stops
            .iter()
            .find(|stop| stop.id == first.id)
            .unwrap();
        assert_eq!(recorded.status, StopStatus::Visited);
        assert_eq!(recorded.completion_source.as_deref(), Some("key"));
        assert_ne!(snapshot.active_stop().unwrap().id, first.id);
        assert_eq!(
            key_visits(&rig).await,
            vec![(first.pin_id, O + 90.0, O + 90.0, true)]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_held_key_records_one_visit_and_each_new_press_records_the_next_tree() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2, 3]), true).await;
        rig.tick(1.0);
        rig.stand(at(30, 0));

        // Autorepeat delivers presses with no release between them.
        press_off_runtime(&rig.input, KeystrokeKind::Press);
        press_off_runtime(&rig.input, KeystrokeKind::Press);
        await_key_visits(&rig, 1).await;
        // Released and pressed again from the same spot moments later: a
        // neighbouring tree in a dense stand, so it records the next stop.
        press_off_runtime(&rig.input, KeystrokeKind::Release);
        press_off_runtime(&rig.input, KeystrokeKind::Press);
        await_key_visits(&rig, 2).await;
        let pins: Vec<i64> = key_visits(&rig)
            .await
            .into_iter()
            .map(|(pin_id, ..)| pin_id)
            .collect();
        assert_eq!(pins.len(), 2);
        assert_ne!(pins[0], pins[1]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_key_press_while_another_window_holds_focus_is_dropped() {
        let focus = Arc::new(Mutex::new(GameFocus::Unfocused));
        let probe_focus = focus.clone();
        let probe: GameFocusProbe = Arc::new(move || *probe_focus.lock().unwrap());
        let rig = rig_with(Some(probe), NavigationSettings::default()).await;
        rig.start(Some(vec![1, 2]), true).await;
        rig.tick(1.0);
        rig.stand(at(30, 0));

        press_off_runtime(&rig.input, KeystrokeKind::Press);
        press_off_runtime(&rig.input, KeystrokeKind::Release);
        // The press was dropped at the dispatch, before any read: nothing
        // was captured for it.
        assert_eq!(rig.screen.captures(), 1, "only the start was read");
        assert!(key_visits(&rig).await.is_empty());

        *focus.lock().unwrap() = GameFocus::Focused;
        press_off_runtime(&rig.input, KeystrokeKind::Press);
        await_key_visits(&rig, 1).await;
    }

    #[tokio::test]
    async fn a_key_route_ignores_harvests_and_a_detecting_route_ignores_the_key() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), true).await;
        let first = run.active_stop().unwrap().clone();
        rig.stand((first.lon as i64, first.lat as i64));
        rig.service.on_harvest(true).await;
        let snapshot = rig.service.snapshot().await.unwrap().unwrap();
        assert_eq!(snapshot.active_stop().unwrap().id, first.id);
        assert!(snapshot.pending_harvest.is_none());

        // A route that detects cuts from harvests leaves the keyboard alone.
        let run = rig.start(Some(vec![1, 2]), false).await;
        assert!(!run.visit_on_key);
        let first = run.active_stop().unwrap().clone();
        rig.stand((first.lon as i64, first.lat as i64));
        rig.service.on_visit_key().await;
        rig.input
            .inject(VISIT_KEY, chrono::Utc::now(), KeystrokeKind::Press);
        let snapshot = rig.service.snapshot().await.unwrap().unwrap();
        assert_eq!(snapshot.active_stop().unwrap().id, first.id);
        assert!(key_visits(&rig).await.is_empty());
    }

    #[tokio::test]
    async fn route_progress_persists_supports_undo_and_stays_visible_at_completion() {
        let rig = rig().await;
        let mut run = rig.start(Some(vec![1, 2]), false).await;
        assert_eq!(run.stops.len(), 2);
        assert_eq!(run.active_stop().unwrap().status, StopStatus::Active);

        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand((first.lon as i64, first.lat as i64));
        let PositionUpdate::Updated(updated) = rig.service.mark_visited(false).await.unwrap()
        else {
            panic!("visited at the active tree")
        };
        run = updated;
        assert_eq!(
            run.stops
                .iter()
                .filter(|stop| stop.status == StopStatus::Visited)
                .count(),
            1
        );

        let second = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand((second.lon as i64, second.lat as i64));
        let PositionUpdate::Updated(completed) = rig.service.mark_visited(false).await.unwrap()
        else {
            panic!("visited at the active tree")
        };
        assert_eq!(completed.status, RunStatus::Completed);
        assert_eq!(
            rig.service.snapshot().await.unwrap().unwrap().status,
            RunStatus::Completed
        );

        let restored = rig.service.undo().await.unwrap();
        assert_eq!(restored.status, RunStatus::Active);
        assert!(restored.active_stop().is_some());
        assert!(rig.changes() >= 4);
    }

    #[tokio::test]
    async fn arriving_at_a_pending_stop_marks_it_and_replans_from_the_observed_position() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2, 3]), false).await;
        // Pick a pending stop beyond the arrival radius of the active one, so a
        // harvest there is an unambiguous out-of-order arrival (not the
        // "prefer the active tree" case).
        let active = run.active_stop().unwrap().clone();
        let pending = run
            .stops
            .iter()
            .filter(|stop| stop.status == StopStatus::Pending)
            .max_by(|a, b| {
                distance((a.lon, a.lat), (active.lon, active.lat))
                    .total_cmp(&distance((b.lon, b.lat), (active.lon, active.lat)))
            })
            .unwrap()
            .clone();
        assert!(
            distance((pending.lon, pending.lat), (active.lon, active.lat))
                > ARRIVAL_TOLERANCE_UNITS
        );
        rig.tick(1.0);
        rig.stand((pending.lon as i64, pending.lat as i64));

        let PositionUpdate::Updated(replanned) =
            rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("position updates")
        };
        assert_eq!(
            replanned
                .stops
                .iter()
                .find(|stop| stop.id == pending.id)
                .unwrap()
                .status,
            StopStatus::Visited,
        );
        assert_eq!(replanned.status, RunStatus::Active);
        assert_eq!(replanned.active_stop().unwrap().ordinal, 1);
        assert_eq!(
            (replanned.current_lon, replanned.current_lat),
            (pending.lon, pending.lat)
        );
    }

    #[tokio::test]
    async fn repeated_harvest_swings_near_the_same_tree_do_not_advance_the_next_stop() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand((first.lon as i64, first.lat as i64));
        rig.service.harvest_position("success").await.unwrap();

        // The next tree is nine units away. A one-unit OCR shift is inside
        // both its arrival radius and the previous observation's debounce
        // radius, so this repeated swing must not consume the next stop.
        rig.tick(1.0);
        rig.stand((first.lon as i64 + 1, first.lat as i64));
        let PositionUpdate::Updated(after_repeat) =
            rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("position updates")
        };
        assert_eq!(
            after_repeat
                .stops
                .iter()
                .filter(|stop| stop.status == StopStatus::Visited)
                .count(),
            1,
        );
        assert_eq!(after_repeat.status, RunStatus::Active);
    }

    #[tokio::test]
    async fn a_harvest_prefers_the_active_tree_when_several_are_within_range() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        // A(30,0) is active and D(39,0) is pending; (34,0) is within fifteen of
        // both. The harvest resolves to the active tree, not an ambiguity.
        let active = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand(at(34, 0));
        let PositionUpdate::Updated(updated) =
            rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("the harvest resolves to the active tree")
        };
        assert_eq!(
            updated
                .stops
                .iter()
                .find(|stop| stop.id == active.id)
                .unwrap()
                .status,
            StopStatus::Visited,
        );
        assert_ne!(updated.active_stop().unwrap().id, active.id);
    }

    #[tokio::test]
    async fn a_far_harvest_awaits_confirmation_then_records_on_confirm() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        let active = run.active_stop().unwrap().clone();

        // EU trees cut from well beyond the arrival radius. A swing with no tree
        // in range must not silently drop: it records nothing yet and stashes a
        // confirmation proposing the active tree.
        rig.tick(1.0);
        rig.stand(at(60, 60));
        let PositionUpdate::Updated(after) = rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("a far harvest refreshes position without advancing")
        };
        assert_eq!(
            after
                .stops
                .iter()
                .filter(|stop| stop.status == StopStatus::Visited)
                .count(),
            0,
        );
        assert_eq!(after.active_stop().unwrap().id, active.id);

        // The snapshot surfaces the pending confirmation naming the active tree.
        let pending = rig
            .service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .expect("a pending harvest confirmation");
        assert_eq!(pending.stop_id, active.id);
        assert_eq!(pending.name, active.name);
        assert!(pending.observed.is_some());

        // Confirming records the active tree as a harvest and advances.
        let resolved = rig.service.resolve_harvest(true).await.unwrap();
        assert_eq!(
            resolved
                .stops
                .iter()
                .find(|stop| stop.id == active.id)
                .unwrap()
                .status,
            StopStatus::Visited,
        );
        assert_ne!(resolved.active_stop().unwrap().id, active.id);
        assert!(rig
            .service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_none());
        let visits = rig.visits().await;
        assert_eq!(visits.len(), 1);
        assert_eq!(
            (visits[0].2, visits[0].3, visits[0].5),
            (O + 60.0, O + 60.0, true)
        );
    }

    #[tokio::test]
    async fn a_far_harvest_dismissal_leaves_the_route_untouched() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        let active = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand(at(60, 60));
        rig.service.harvest_position("success").await.unwrap();
        assert!(rig
            .service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_some());

        let dismissed = rig.service.resolve_harvest(false).await.unwrap();
        assert_eq!(dismissed.active_stop().unwrap().id, active.id);
        assert_eq!(
            dismissed
                .stops
                .iter()
                .filter(|stop| stop.status == StopStatus::Visited)
                .count(),
            0,
        );
        assert!(rig
            .service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_none());
    }

    #[tokio::test]
    async fn a_harvest_amid_only_non_active_trees_stays_ambiguous() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2, 3]), false).await;
        // (50,4) is within fifteen of pending D(39,0) and B(60,9) but not of the
        // active A(30,0), so there is no active tree to fall back on: never guess.
        rig.tick(1.0);
        rig.stand(at(50, 4));
        let PositionUpdate::Ambiguous(ambiguous) =
            rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("two non-active trees in range stay ambiguous")
        };
        assert!(ambiguous
            .stops
            .iter()
            .all(|stop| matches!(stop.status, StopStatus::Active | StopStatus::Pending)));
    }

    #[tokio::test]
    async fn update_position_observes_without_recording_a_visit() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        let first = run.active_stop().unwrap().clone();
        rig.stand((first.lon as i64, first.lat as i64));
        rig.tick(1.0);

        let PositionUpdate::Updated(observed) = rig.poll().await else {
            panic!("position observes")
        };
        // Standing exactly on the active tree still records nothing: observing
        // is strictly separate from completing.
        assert!(observed
            .stops
            .iter()
            .all(|stop| stop.status != StopStatus::Visited));
        assert_eq!(observed.active_stop().unwrap().id, first.id);
        assert_eq!(
            (observed.current_lon, observed.current_lat),
            (first.lon, first.lat)
        );
        assert!(observed.last_position_at.is_some());
    }

    #[tokio::test]
    async fn mark_visited_outside_tolerance_needs_force_then_completes_the_active_tree() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand(at(50, 50));

        let PositionUpdate::OutOfTolerance(pending) =
            rig.service.mark_visited(false).await.unwrap()
        else {
            panic!("an out-of-range visit asks for confirmation")
        };
        // Position refreshed, but nothing recorded until the user confirms.
        assert!(pending
            .stops
            .iter()
            .all(|stop| stop.status != StopStatus::Visited));
        assert_eq!(
            (pending.current_lon, pending.current_lat),
            (O + 50.0, O + 50.0)
        );

        rig.tick(1.0);
        let PositionUpdate::Updated(forced) = rig.service.mark_visited(true).await.unwrap() else {
            panic!("a forced visit completes the active tree")
        };
        assert_eq!(
            forced
                .stops
                .iter()
                .find(|stop| stop.id == first.id)
                .unwrap()
                .status,
            StopStatus::Visited,
        );
        assert_ne!(forced.active_stop().unwrap().id, first.id);
    }

    #[tokio::test]
    async fn a_regenerated_route_excludes_recently_visited_trees() {
        let rig = rig().await;
        let run = rig.start(None, false).await;
        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand((first.lon as i64, first.lat as i64));
        rig.service.mark_visited(false).await.unwrap();
        rig.service.end().await.unwrap();

        let regenerated = rig.start(None, false).await;
        // The just-visited tree is on cooldown, so it never enters the new route.
        assert!(regenerated
            .stops
            .iter()
            .all(|stop| stop.pin_id != first.pin_id));
    }

    #[tokio::test]
    async fn a_lingering_run_is_ended_at_startup_not_resumed() {
        let (dir, service, position, _changes, _input) = navigation_fixture().await;
        service
            .start("Calypso".into(), None, O, O, Some(vec![1, 2]), false)
            .await
            .unwrap();
        assert!(service.snapshot().await.unwrap().is_some());
        drop(service);

        // A fresh service on the same database does not restore the run: an
        // interrupted route is recovered by regeneration, not hydration.
        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let restarted = spawn_navigation(
            db,
            clock.clone(),
            Screen::new(position.clone(), clock),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(MockKeystrokeSource::new()),
            None,
            NavigationSettings::default(),
        )
        .await;
        assert!(restarted.snapshot().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cooling_the_active_tree_skips_it_and_replans() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, O, O, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let active = run.active_stop().unwrap().clone();
        service.cooldown_pin(active.pin_id).await.unwrap();
        let after = service.snapshot().await.unwrap().unwrap();
        assert_eq!(
            after
                .stops
                .iter()
                .find(|stop| stop.pin_id == active.pin_id)
                .unwrap()
                .status,
            StopStatus::Skipped,
        );
        assert_ne!(
            after.active_stop().map(|stop| stop.pin_id),
            Some(active.pin_id)
        );
        assert_eq!(after.status, RunStatus::Active);
    }

    #[tokio::test]
    async fn cooling_a_tree_excludes_it_from_the_next_route_and_claims_no_position() {
        let rig = rig().await;
        let run = rig.start(None, false).await;
        let cooled = run.active_stop().unwrap().pin_id;
        rig.service.end().await.unwrap();
        rig.service.cooldown_pin(cooled).await.unwrap();
        let replanned = rig.start(None, false).await;
        assert!(replanned.stops.iter().all(|stop| stop.pin_id != cooled));
        // Put on cooldown from the map, the tree was not read to be visited.
        let visits = rig.visits().await;
        assert_eq!(visits.len(), 1);
        assert!(!visits[0].5);
    }

    #[tokio::test]
    async fn deleting_a_route_pin_drops_its_stop_and_replans() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, O, O, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let removed = run.active_stop().unwrap().pin_id;
        service.replan_after_pin_removed(removed).await.unwrap();
        let after = service.snapshot().await.unwrap().unwrap();
        assert!(after.stops.iter().all(|stop| stop.pin_id != removed));
        assert_eq!(after.status, RunStatus::Active);
        assert!(after.active_stop().is_some());
    }

    // The position check, on every path that records a position.

    #[tokio::test]
    async fn the_poll_records_a_move_the_game_can_make_and_refuses_a_jump_it_cannot() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        let announced = rig.changes();

        // Fifty metres in a second: a move.
        rig.tick(1.0);
        rig.stand(at(50, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        let moved_at = naive_to_epoch(rig.clock.now());
        assert_eq!(rig.recorded().await, (O + 50.0, O, Some(moved_at)));
        assert_eq!(
            rig.changes(),
            announced + 1,
            "an accepted position is announced once"
        );

        // A hundred and fifty metres in the next second is more than the
        // game carries a player: refused, nothing recorded or announced.
        let limit = JumpLimit::default();
        assert!(limit.speed_m_per_s + limit.allowance_m < 150.0);
        rig.tick(1.0);
        rig.stand(at(-100, 0));
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        assert_eq!(rig.recorded().await, (O + 50.0, O, Some(moved_at)));
        assert_eq!(rig.changes(), announced + 1);

        // A reading back near the last position is a move from it again:
        // the refused jump was a single wrong reading.
        rig.tick(1.0);
        rig.stand(at(60, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        assert_eq!(rig.recorded().await.0, O + 60.0);
        assert_eq!(rig.changes(), announced + 2);
    }

    #[tokio::test]
    async fn a_teleport_is_accepted_once_the_next_reading_agrees() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        rig.stand(at(-5_000, -5_000));
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        // The next poll reads the new place again (a step further on):
        // the player has teleported, and the position is recorded as of
        // this reading.
        rig.tick(1.0);
        rig.stand(at(-4_998, -5_000));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        let read_at = naive_to_epoch(rig.clock.now());
        assert_eq!(
            rig.recorded().await,
            (O - 4_998.0, O - 5_000.0, Some(read_at))
        );
    }

    #[tokio::test]
    async fn after_a_long_gap_the_allowance_stays_capped() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        // A minute with the coordinates covered: by speed alone six
        // kilometres would be allowed, and the first misreading recorded.
        rig.tick(60.0);
        let limit = JumpLimit::default();
        let capped = limit.speed_m_per_s * limit.max_gap_s + limit.allowance_m;
        assert!(capped < 1_000.0);
        rig.stand(at(1_000, 0));
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        assert_eq!(rig.recorded().await.0, O);
        // Within the capped allowance, a move is still a move.
        rig.tick(60.0);
        rig.stand(at(400, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
    }

    #[tokio::test]
    async fn a_misreading_that_repeats_is_accepted_which_the_check_cannot_stop() {
        // Recognition is deterministic: a player standing still gives the
        // same pixels, so a misreading comes back identical on the next
        // poll, and agreeing with itself it is accepted. The check stops
        // passing errors only; this pins that limit, so it is not mistaken
        // for more.
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        rig.stand(at(3_000, 0));
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        rig.tick(1.0);
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        assert_eq!(rig.recorded().await.0, O + 3_000.0);
    }

    #[tokio::test]
    async fn readings_either_side_of_a_poll_that_read_nothing_are_not_consecutive() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        let far = at(-5_000, -5_000);
        rig.tick(1.0);
        rig.stand(far);
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        rig.screen.garble_lon.store(true, AtomicOrdering::SeqCst);
        rig.tick(1.0);
        assert!(matches!(rig.poll().await, PositionUpdate::NotRead(_)));
        rig.screen.garble_lon.store(false, AtomicOrdering::SeqCst);
        rig.tick(1.0);
        assert_eq!(
            rig.poll().await,
            PositionUpdate::JumpRefused,
            "the run of readings starts again"
        );
        rig.tick(1.0);
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        assert_eq!(rig.recorded().await.0, O - 5_000.0);
    }

    #[tokio::test]
    async fn one_value_refused_records_and_announces_nothing() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        let before = rig.recorded().await;
        let announced = rig.changes();
        rig.screen.garble_lon.store(true, AtomicOrdering::SeqCst);
        rig.tick(1.0);
        rig.stand(at(5, 5));
        assert_eq!(
            rig.poll().await,
            PositionUpdate::NotRead(CoordRefusal::NotRead {
                lon: Some(ValueRefusal::Unclear),
                lat: None,
            })
        );
        // The latitude read, but is not paired with a remembered longitude.
        assert_eq!(rig.recorded().await, before);
        assert_eq!(rig.changes(), announced);
    }

    #[tokio::test]
    async fn the_poll_records_when_the_screen_was_read_not_when_the_read_finished() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        let captured_at = naive_to_epoch(rig.clock.now());
        *rig.screen.read_takes.lock().unwrap() = 7.0;
        rig.stand(at(20, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        assert_eq!(rig.recorded().await.2, Some(captured_at));
        assert!(naive_to_epoch(rig.clock.now()) > captured_at);
    }

    #[tokio::test]
    async fn a_route_started_from_a_reading_keeps_its_read_time_and_one_from_elsewhere_has_none() {
        let rig = rig().await;
        let read_at = naive_to_epoch(rig.clock.now());
        rig.start(Some(vec![1, 2]), false).await;
        assert_eq!(rig.recorded().await, (O, O, Some(read_at)));
        assert!(rig
            .service
            .position_age(rig.service.snapshot().await.unwrap().unwrap().id)
            .is_some());

        // A start typed in rather than read: no position yet, so the first
        // reading needs the next one's agreement.
        rig.service
            .start("Calypso".into(), None, O + 7.0, O, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let run = rig.service.snapshot().await.unwrap().unwrap();
        assert_eq!(run.last_position_at, None);
        assert_eq!(rig.service.position_age(run.id), None);
        rig.tick(1.0);
        rig.stand(at(8, 0));
        assert_eq!(rig.poll().await, PositionUpdate::JumpRefused);
        rig.tick(1.0);
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
    }

    #[tokio::test]
    async fn durations_follow_the_monotonic_clock_when_the_wall_clock_steps_back() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        rig.stand(at(20, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        // The wall clock steps back an hour (a correction, or a change of
        // the clocks); the monotonic reading carries on.
        let stepped = rig.clock.now() - chrono::Duration::hours(1);
        rig.clock.freeze_at(stepped);
        rig.tick(1.0);
        rig.stand(at(40, 0));
        assert!(
            matches!(rig.poll().await, PositionUpdate::Updated(_)),
            "a reading after the step is neither stale nor a jump"
        );
        // What is stored is the wall time of the reading.
        assert_eq!(
            rig.recorded().await.2,
            Some(naive_to_epoch(rig.clock.now()))
        );
        assert_eq!(rig.service.position_age(run.id), Some(0.0));
        rig.tick(3.0);
        assert_eq!(rig.service.position_age(run.id), Some(3.0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reading_for_a_route_that_ended_meanwhile_is_dropped() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        rig.stand(at(20, 0));
        rig.screen.hold();
        let polling = {
            let service = rig.service.clone();
            tokio::spawn(async move { service.update_position().await })
        };
        // The start's read captured once; the poll's capture is the second.
        while rig.screen.captures() < 2 {
            tokio::task::yield_now().await;
        }
        // The player ends the route and starts another while the position
        // is being read.
        rig.service.end().await.unwrap();
        let next = rig
            .service
            .start(
                "Calypso".into(),
                None,
                O + 1.0,
                O + 1.0,
                Some(vec![1, 2]),
                false,
            )
            .await
            .unwrap();
        let announced = rig.changes();
        rig.screen.release();
        assert_eq!(polling.await.unwrap().unwrap(), PositionUpdate::Superseded);
        let run = rig.service.snapshot().await.unwrap().unwrap();
        assert_eq!(run.id, next.id);
        assert_eq!((run.current_lon, run.current_lat), (O + 1.0, O + 1.0));
        assert_eq!(rig.changes(), announced);
    }

    #[tokio::test]
    async fn a_harvest_misread_near_another_tree_neither_visits_it_nor_moves_the_route() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2, 3]), false).await;
        rig.tick(3.0);
        rig.stand(at(-300, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        // A misreading lands on tree B, hundreds of metres from where the
        // player was a second ago.
        rig.tick(1.0);
        rig.stand(at(60, 9));
        assert_eq!(
            rig.service.harvest_position("success").await.unwrap(),
            PositionUpdate::JumpRefused
        );
        let run = rig.service.snapshot().await.unwrap().unwrap();
        assert!(run
            .stops
            .iter()
            .all(|stop| stop.status != StopStatus::Visited));
        assert_eq!(run.current_lon, O - 300.0);
        // The harvest keeps its event: it proposes the active tree, with no
        // position.
        let pending = run.pending_harvest.expect("the harvest is kept");
        assert_eq!(pending.observed, None);
        // The next correct poll is not a jump from the misreading.
        rig.tick(1.0);
        rig.stand(at(-299, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
        // Confirmed, the proposed tree is visited without a position.
        let resolved = rig.service.resolve_harvest(true).await.unwrap();
        assert_eq!(
            resolved
                .stops
                .iter()
                .filter(|s| s.status == StopStatus::Visited)
                .count(),
            1
        );
        assert!(!rig.visits().await[0].5);
    }

    #[tokio::test]
    async fn a_key_press_whose_position_is_held_back_records_the_visit_without_one() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), true).await;
        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand(at(5_000, 0));
        rig.service.on_visit_key().await;
        let after = rig.service.snapshot().await.unwrap().unwrap();
        // The press is the evidence: the stop is visited and the route
        // moves on, but no position is claimed for it.
        assert_eq!(
            after
                .stops
                .iter()
                .find(|s| s.id == first.id)
                .unwrap()
                .status,
            StopStatus::Visited
        );
        assert_ne!(after.active_stop().unwrap().id, first.id);
        assert_eq!(rig.stop_observation(first.id).await, None);
        assert_eq!(
            key_visits(&rig).await,
            vec![(first.pin_id, first.lon, first.lat, false)]
        );
        let visits = rig.visits().await;
        assert_eq!(visits[0].4, 0.0, "a placeholder, not an observation");
        assert_eq!((after.current_lon, after.current_lat), (O, O));
    }

    #[tokio::test]
    async fn a_key_press_whose_position_did_not_read_records_the_visit_without_one() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2]), true).await;
        let first = run.active_stop().unwrap().clone();
        rig.tick(1.0);
        rig.stand(at(30, 0));
        rig.screen.garble_lon.store(true, AtomicOrdering::SeqCst);
        rig.service.on_visit_key().await;
        let after = rig.service.snapshot().await.unwrap().unwrap();
        assert_ne!(after.active_stop().unwrap().id, first.id);
        assert_eq!(rig.stop_observation(first.id).await, None);
        assert_eq!(
            key_visits(&rig).await,
            vec![(first.pin_id, first.lon, first.lat, false)]
        );
        // The route's position is not refreshed by a press that read nothing.
        assert_eq!(after.last_position_at, run.last_position_at);
    }

    #[tokio::test]
    async fn visited_with_a_position_held_back_says_it_did_not_confirm() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.tick(1.0);
        rig.stand(at(5_000, 0));
        assert_eq!(
            rig.service.mark_visited(true).await.unwrap(),
            PositionUpdate::JumpRefused
        );
        let run = rig.service.snapshot().await.unwrap().unwrap();
        assert!(run
            .stops
            .iter()
            .all(|stop| stop.status != StopStatus::Visited));
        assert!(rig.visits().await.is_empty());
    }

    #[tokio::test]
    async fn a_visit_recorded_without_a_position_is_not_read_as_one() {
        // A harvest confirmed without a position leaves a placeholder at
        // tree A. The next harvest, read within the arrival radius of both
        // A's placeholder and the next tree D, is not a repeat of a swing
        // at A: no position was read there.
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.screen.garble_lon.store(true, AtomicOrdering::SeqCst);
        rig.tick(1.0);
        rig.service.harvest_position("success").await.unwrap();
        rig.service.resolve_harvest(true).await.unwrap();
        rig.screen.garble_lon.store(false, AtomicOrdering::SeqCst);
        rig.tick(1.0);
        rig.stand(at(34, 0));
        let PositionUpdate::Updated(after) = rig.service.harvest_position("success").await.unwrap()
        else {
            panic!("the harvest reads")
        };
        assert_eq!(
            after
                .stops
                .iter()
                .filter(|s| s.status == StopStatus::Visited)
                .count(),
            2,
            "D is visited, not taken for A swung at again"
        );
    }

    #[tokio::test]
    async fn capture_not_permitted_is_asked_once_across_key_harvest_and_poll() {
        let rig = rig().await;
        let run = rig.start(Some(vec![1, 2, 3]), true).await;
        let captures = rig.screen.captures();
        rig.screen.show_nothing(FrameUnavailable::CaptureNotStarted);
        rig.tick(1.0);
        rig.service.on_visit_key().await;
        assert_eq!(rig.screen.captures(), captures + 1);
        rig.service.on_visit_key().await;
        assert_eq!(
            rig.poll().await,
            PositionUpdate::PollStopped(PollStop::CaptureNotAllowed)
        );
        assert_eq!(rig.screen.captures(), captures + 1, "never asked again");
        // Both presses still count, without a position.
        let visits = key_visits(&rig).await;
        assert_eq!(visits.len(), 2);
        assert!(visits.iter().all(|visit| !visit.3));
        let _ = run;

        // A detecting route's harvests ask nothing either.
        let rig = rig_harvest_refused().await;
        let captures = rig.screen.captures();
        rig.service.harvest_position("success").await.unwrap();
        rig.service.harvest_position("success").await.unwrap();
        assert_eq!(rig.screen.captures(), captures);
        assert!(rig
            .service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_some_and(|pending| pending.observed.is_none()));

        // The player tries again from the overlay: the poll reads again.
        rig.screen.show_no_radar();
        rig.service.resume_polling();
        rig.tick(1.0);
        rig.stand(at(10, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
    }

    /// A detecting route on a screen that has refused capture once.
    async fn rig_harvest_refused() -> Rig {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        rig.screen.show_nothing(FrameUnavailable::CaptureNotStarted);
        rig.tick(1.0);
        rig.service.harvest_position("success").await.unwrap();
        rig
    }

    #[tokio::test]
    async fn the_poll_reads_only_while_the_overlays_show_and_stops_after_refusing_for_long() {
        let rig = rig().await;
        rig.start(Some(vec![1, 2]), false).await;
        let captures = rig.screen.captures();
        rig.service.set_overlays_shown(false);
        assert_eq!(
            rig.poll().await,
            PositionUpdate::PollStopped(PollStop::OverlaysHidden)
        );
        assert_eq!(rig.screen.captures(), captures);
        rig.service.set_overlays_shown(true);

        // The coordinates stay unreadable: the poll says why each time,
        // then stops once the refusal limit has passed.
        rig.screen.garble_lon.store(true, AtomicOrdering::SeqCst);
        let limit = NavigationSettings::default()
            .poll_refusal_limit
            .as_secs_f64();
        assert!(matches!(rig.poll().await, PositionUpdate::NotRead(_)));
        rig.tick(limit / 2.0);
        assert!(matches!(rig.poll().await, PositionUpdate::NotRead(_)));
        rig.tick(limit / 2.0);
        assert_eq!(
            rig.poll().await,
            PositionUpdate::PollStopped(PollStop::NotReading)
        );
        let captures = rig.screen.captures();
        assert_eq!(
            rig.poll().await,
            PositionUpdate::PollStopped(PollStop::NotReading)
        );
        assert_eq!(rig.screen.captures(), captures);
        // Resumed by the player, it reads again.
        rig.screen.garble_lon.store(false, AtomicOrdering::SeqCst);
        rig.service.resume_polling();
        rig.stand(at(10, 0));
        assert!(matches!(rig.poll().await, PositionUpdate::Updated(_)));
    }

    #[tokio::test]
    async fn a_planet_without_known_coordinates_reads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let screen = Screen::new(Arc::new(Mutex::new(at(0, 0))), clock.clone());
        let shared: Arc<dyn Clock> = clock;
        let service = NavigationService::new(
            db,
            shared.clone(),
            NavigationProviders {
                coord_capture: CoordCaptureService::new(screen.providers(), shared),
                bounds: Arc::new(|_| None),
                changed: Arc::new(|| {}),
                input: None,
                focus: None,
                settings: NavigationSettings::default(),
            },
        )
        .await;
        let read = service.read_position("Nowhere", Trigger::Automatic).await;
        assert_eq!(read, Err(CoordRefusal::PlanetUnknown));
        assert_eq!(screen.captures(), 0);
    }

    // Radar guidance.

    /// Settings under which guidance looks only when the test asks it to.
    fn test_driven() -> NavigationSettings {
        NavigationSettings {
            guidance_interval: Duration::from_secs(3600),
            ..NavigationSettings::default()
        }
    }

    /// Show a drawn radar on the stand-in screen, its rings labelled with
    /// their distances or not.
    fn show_radar(rig: &Rig, centre: (i32, i32), radius: i32, labelled: bool) {
        let mut scene = RadarScene::sized(1100, 700, radius as u64);
        scene.radar(centre, radius, &RINGS);
        if labelled {
            scene.ring_labels(centre, &RINGS, &RING_LABELS);
        }
        rig.screen
            .show(scene.image.clone(), Arc::new(scene.recogniser()), (40, 30));
    }

    /// Have guidance look once more, and wait for the look to land.
    fn look_again(rig: &Rig) -> RadarGuidance {
        let looks = rig.service.guidance_looks();
        rig.service.guidance_check_now();
        tokio::task::block_in_place(|| {
            assert!(
                rig.service.wait_for_guidance_looks(looks + 1, GUARD)
                    || !matches!(
                        rig.service.radar_guidance(),
                        RadarGuidance::Placed(_) | RadarGuidance::Lost(_)
                    ),
                "the look never landed"
            );
        });
        rig.service.radar_guidance()
    }

    /// Start guidance and wait for its first look to land.
    async fn start_guidance(rig: &Rig) -> RadarGuidance {
        let looks = rig.service.guidance_looks();
        rig.service.guidance_start().await;
        tokio::task::block_in_place(|| {
            rig.service.wait_for_guidance_looks(looks + 1, GUARD);
        });
        rig.service.radar_guidance()
    }

    fn placement(status: RadarGuidance) -> RadarPlacement {
        let RadarGuidance::Placed(placement) = status else {
            panic!("not placed: {status:?}");
        };
        placement
    }

    fn near(placement: &RadarPlacement, centre: (f64, f64)) -> bool {
        (placement.centre_x - centre.0).abs() <= 1.5 && (placement.centre_y - centre.1).abs() <= 1.5
    }

    async fn guided_rig() -> Rig {
        let rig = rig_with(None, test_driven()).await;
        rig.start(Some(vec![1, 2]), false).await;
        rig
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guidance_follows_the_radar_moved_and_resized_with_its_scale() {
        let rig = guided_rig().await;
        let told = Arc::new(AtomicUsize::new(0));
        let counter = told.clone();
        rig.service.set_guidance_changed(Arc::new(move || {
            counter.fetch_add(1, AtomicOrdering::SeqCst);
        }));
        show_radar(&rig, (300, 250), 113, true);
        // The game window sits at (40, 30) on the screen; the dot's middle
        // is half a pixel past the pixel it is drawn on.
        let first = placement(start_guidance(&rig).await);
        assert!(near(&first, (340.5, 280.5)), "{first:?}");
        assert_eq!(first.source, PlacementSource::Found);
        assert!((first.radius_px - 113.0).abs() <= 1.5, "{first:?}");
        let profile = bundled_profile();
        let band_reach = profile.radar.coordinates.band_reach;
        assert!(
            (first.clear_right_x - (first.centre_x + first.radius_px * (1.0 + band_reach))).abs()
                < 1e-9
        );
        assert_eq!(
            first.label_strip,
            Some(LabelStrip {
                from: profile.radar.rings.span[0],
                to: 1.0,
                half_height: profile.radar.rings.label_reach,
            })
        );
        let scale = first.scale.expect("the ring distances read");
        assert!((scale.metres_per_pixel / (200.0 / 103.0) - 1.0).abs() < 0.01);
        let told_once = told.load(AtomicOrdering::SeqCst);

        // The same radar looked at again: nothing new to tell.
        look_again(&rig);
        assert_eq!(told.load(AtomicOrdering::SeqCst), told_once);

        // The player drags the radar elsewhere and enlarges it; the client
        // keeps the map's scale, so the rings stay where they were.
        show_radar(&rig, (650, 300), 169, true);
        let moved = placement(look_again(&rig));
        assert!(near(&moved, (690.5, 330.5)), "{moved:?}");
        assert!((moved.radius_px - 169.0).abs() <= 2.0, "{moved:?}");
        let scale = moved.scale.expect("the ring distances read");
        assert!(
            (scale.edge_range_m / (moved.radius_px * 200.0 / 103.0) - 1.0).abs() < 0.02,
            "{scale:?}"
        );
        assert!(told.load(AtomicOrdering::SeqCst) > told_once);
        rig.service.guidance_stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ring_distances_hidden_are_said_once_two_looks_agree() {
        let rig = guided_rig().await;
        show_radar(&rig, (400, 260), 113, false);
        let first = placement(start_guidance(&rig).await);
        assert_eq!(first.scale, None);
        assert_eq!(first.scale_missing, None, "one look may be a passing one");
        let second = placement(look_again(&rig));
        assert_eq!(second.scale_missing, Some(ScaleMissing::LabelsUnread));
        rig.service.guidance_stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guidance_captures_nothing_once_the_route_ends() {
        let rig = guided_rig().await;
        show_radar(&rig, (400, 260), 113, false);
        placement(start_guidance(&rig).await);
        rig.service.end().await.unwrap();
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off { stopped: None }
        );
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        let captures = rig.screen.captures();
        rig.service.guidance_check_now();
        assert_eq!(rig.screen.captures(), captures);
        // Nor does it start again without a route.
        rig.service.guidance_start().await;
        assert_eq!(rig.screen.captures(), captures);
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off { stopped: None }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guidance_stops_with_the_overlays_and_does_not_start_without_them() {
        let rig = guided_rig().await;
        show_radar(&rig, (400, 260), 113, false);
        placement(start_guidance(&rig).await);
        rig.service.set_overlays_shown(false);
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        let captures = rig.screen.captures();
        rig.service.guidance_check_now();
        rig.service.guidance_start().await;
        assert_eq!(rig.screen.captures(), captures);
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off { stopped: None }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overlays_hidden_while_the_first_look_is_out_leave_guidance_off() {
        // What the shell sees when it shows the overlays and they are taken
        // away again before the first look lands: guidance is off and the
        // overlays are not to be shown.
        let rig = guided_rig().await;
        show_radar(&rig, (400, 260), 113, false);
        rig.screen.hold();
        let starting = {
            let service = rig.service.clone();
            tokio::spawn(async move { service.guidance_start().await })
        };
        while rig.screen.captures() < 2 {
            tokio::task::yield_now().await;
        }
        rig.service.set_overlays_shown(false);
        rig.screen.release();
        assert_eq!(
            starting.await.unwrap(),
            RadarGuidance::Off { stopped: None }
        );
        assert!(!rig.service.overlays_shown());
        assert!(rig.service.wait_for_guidance_workers(GUARD));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_look_whose_turn_comes_after_guidance_stopped_captures_nothing() {
        // A position read holds the capture; guidance is asked to look and
        // waits its turn; guidance stops; the read finishes. The waiting
        // look must not capture.
        let rig = guided_rig().await;
        show_radar(&rig, (400, 260), 113, false);
        placement(start_guidance(&rig).await);
        rig.screen.show_no_radar();
        rig.screen.hold();
        let polling = {
            let service = rig.service.clone();
            tokio::spawn(async move { service.update_position().await })
        };
        let before = rig.screen.captures();
        while rig.screen.captures() == before {
            tokio::task::yield_now().await;
        }
        rig.service.guidance_check_now();
        rig.service.guidance_stop();
        let captured = rig.screen.captures();
        rig.screen.release();
        let _ = polling.await.unwrap();
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        // The poll, finding no radar where it was, captures the whole window
        // once more; guidance captures nothing.
        assert_eq!(rig.screen.captures(), captured + 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_look_that_fails_unexpectedly_ends_guidance_with_a_reason() {
        let rig = guided_rig().await;
        rig.screen.broken.store(true, AtomicOrdering::SeqCst);
        rig.service.guidance_start().await;
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off {
                stopped: Some(GuidanceStop::Failed),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_not_allowed_ends_guidance_without_asking_again() {
        let rig = guided_rig().await;
        let captures = rig.screen.captures();
        rig.screen.show_nothing(FrameUnavailable::CaptureNotStarted);
        rig.service.guidance_start().await;
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off {
                stopped: Some(GuidanceStop::CaptureNotAllowed),
            }
        );
        assert_eq!(
            rig.screen.captures(),
            captures + 1,
            "asked once, never again"
        );
        // Starting again does not ask again either: the player must try
        // again from the overlay.
        rig.service.guidance_start().await;
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        assert_eq!(rig.screen.captures(), captures + 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_radar_not_found_for_the_idle_limit_ends_guidance() {
        let rig = guided_rig().await;
        assert_eq!(
            start_guidance(&rig).await,
            RadarGuidance::Lost(GuidanceLoss::RadarNotFound)
        );
        rig.tick(test_driven().guidance_idle_limit.as_secs_f64());
        look_again(&rig);
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off {
                stopped: Some(GuidanceStop::RadarNotSeen),
            }
        );
    }

    async fn calibrate(rig: &Rig) {
        rig.service
            .db
            .with_writer(|conn| {
                conn.execute(
                    "INSERT INTO radar_calibration (singleton, centre_x, centre_y, north_x, north_y, radius_px, display_scale, updated_at) VALUES (1, 1500, 200, 1500, 100, 100.0, 1.0, 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_hand_calibration_stands_in_only_until_the_radar_is_found() {
        let rig = guided_rig().await;
        calibrate(&rig).await;
        let calibrated = placement(start_guidance(&rig).await);
        assert!(near(&calibrated, (1500.0, 200.0)));
        assert_eq!(calibrated.source, PlacementSource::Calibrated);
        assert_eq!(calibrated.radius_px, 100.0);
        assert_eq!(calibrated.scale, None);

        show_radar(&rig, (400, 260), 113, true);
        let found = placement(look_again(&rig));
        assert_eq!(found.source, PlacementSource::Found);
        assert!(found.scale.is_some());

        // The radar covered for a look keeps its place; then, the loss
        // holding, it is lost, never the calibration again.
        rig.screen.show_no_radar();
        assert_eq!(placement(look_again(&rig)), found);
        assert_eq!(
            look_again(&rig),
            RadarGuidance::Lost(GuidanceLoss::RadarNotFound)
        );
        rig.service.guidance_stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_hand_calibration_does_not_keep_guidance_looking_for_ever() {
        let rig = guided_rig().await;
        calibrate(&rig).await;
        placement(start_guidance(&rig).await);
        rig.tick(test_driven().guidance_idle_limit.as_secs_f64());
        look_again(&rig);
        assert!(rig.service.wait_for_guidance_workers(GUARD));
        assert_eq!(
            rig.service.radar_guidance(),
            RadarGuidance::Off {
                stopped: Some(GuidanceStop::RadarNotSeen),
            }
        );
    }
}
