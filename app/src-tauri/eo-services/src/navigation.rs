//! Persisted map-route navigation and radar guidance.
//!
//! Pins are immutable route inputs. Operational progress belongs to a
//! navigation run, so reaching a tree never mutates the pin itself. All
//! state-changing operations serialise through `operation`, while the
//! synchronous database core remains behind its async reader/writer seam.

use std::sync::{Arc, Mutex};

use rusqlite::OptionalExtension;

use crate::clock::Clock;
use crate::coord_capture::{
    CoordBounds, CoordCaptureService, CoordConfirmListener, CoordRead, CoordScanOutcome,
};
use crate::db::{Db, DbError};
use crate::eu_window::GameFocus;
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

pub type BoundsProvider = Arc<dyn Fn(&str) -> Option<CoordBounds> + Send + Sync>;
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

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
    pub observed_lon: f64,
    pub observed_lat: f64,
    pub observed_distance: f64,
    pub outcome: String,
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
    NoRegion,
    CaptureFailed,
    EngineUnavailable,
    Unreadable,
    Implausible,
    Ambiguous(NavigationRun),
    /// A manual `Visited` whose observed position is outside the arrival
    /// tolerance. The run position is updated so distance/bearing are fresh,
    /// but no visit is recorded until the user confirms a forced visit.
    OutOfTolerance(NavigationRun),
}

/// The read-failure legs of a coordinate scan, kept small so the scan helper
/// does not carry a run-sized `Err` type. Each maps to its `PositionUpdate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanFailure {
    NoRegion,
    CaptureFailed,
    EngineUnavailable,
    Unreadable,
    Implausible,
}

impl From<ScanFailure> for PositionUpdate {
    fn from(failure: ScanFailure) -> Self {
        match failure {
            ScanFailure::NoRegion => PositionUpdate::NoRegion,
            ScanFailure::CaptureFailed => PositionUpdate::CaptureFailed,
            ScanFailure::EngineUnavailable => PositionUpdate::EngineUnavailable,
            ScanFailure::Unreadable => PositionUpdate::Unreadable,
            ScanFailure::Implausible => PositionUpdate::Implausible,
        }
    }
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

pub struct NavigationService {
    db: Db,
    clock: Arc<dyn Clock>,
    coord_capture: Arc<CoordCaptureService>,
    bounds: BoundsProvider,
    changed: ChangedSink,
    operation: tokio::sync::Mutex<()>,
    radar_phase: Mutex<RadarCalibrationPhase>,
    radar_confirm_listener: Mutex<Option<std::sync::Weak<CoordConfirmListener>>>,
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
    pub async fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        coord_capture: Arc<CoordCaptureService>,
        bounds: BoundsProvider,
        changed: ChangedSink,
        input: Option<Arc<dyn KeystrokeSource>>,
        focus: Option<GameFocusProbe>,
    ) -> Arc<Self> {
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
            operation: tokio::sync::Mutex::new(()),
            radar_phase: Mutex::new(RadarCalibrationPhase::Idle),
            radar_confirm_listener: Mutex::new(None),
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
        let run_id = self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_runs SET status = 'ended', updated_at = ?1 WHERE status IN ('active', 'paused', 'completed')",
                [now],
            )?;
            tx.execute(
                "INSERT INTO navigation_runs (planet, map_view_id, status, start_lon, start_lat, current_lon, current_lat, hop_count, visit_on_key, created_at, updated_at) VALUES (?1, ?2, 'active', ?3, ?4, ?3, ?4, ?5, ?6, ?7, ?7)",
                rusqlite::params![planet, map_view_id, start_lon, start_lat, route.len() as i64, visit_on_key, now],
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

    /// Observe the current position. Strictly captures the coordinate and
    /// refreshes the run's position so distance and bearing to the active
    /// tree recompute; it never records a visit or advances the route. The
    /// overlay's automatic position updates ride this path.
    pub async fn update_position(&self) -> Result<PositionUpdate, NavigationError> {
        let _guard = self.operation.lock().await;
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        let read = match self.scan_position(&run.planet) {
            Ok(read) => read,
            Err(failure) => return Ok(failure.into()),
        };
        let now = naive_to_epoch(self.clock.now());
        update_run_position(&self.db, run.id, read.lon as f64, read.lat as f64, now).await?;
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
    /// recorded, and `OutOfTolerance` asks the caller to confirm.
    pub async fn mark_visited(&self, force: bool) -> Result<PositionUpdate, NavigationError> {
        let _guard = self.operation.lock().await;
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        let read = match self.scan_position(&run.planet) {
            Ok(read) => read,
            Err(failure) => return Ok(failure.into()),
        };
        let lon = read.lon as f64;
        let lat = read.lat as f64;
        let now = naive_to_epoch(self.clock.now());
        let Some(active) = run.active_stop() else {
            update_run_position(&self.db, run.id, lon, lat, now).await?;
            (self.changed)();
            return Ok(PositionUpdate::NoActiveRun);
        };
        let observed_distance = distance((lon, lat), (active.lon, active.lat));
        if observed_distance > ARRIVAL_TOLERANCE_UNITS && !force {
            update_run_position(&self.db, run.id, lon, lat, now).await?;
            let refreshed = load_run(&self.db, run.id)
                .await?
                .ok_or(NavigationError::NoActiveRun)?;
            (self.changed)();
            return Ok(PositionUpdate::OutOfTolerance(refreshed));
        }
        let refreshed = self
            .record_active_visit(run.id, active, (lon, lat), "manual", now)
            .await?;
        Ok(PositionUpdate::Updated(refreshed))
    }

    /// The interact key was pressed on a route planned to visit on it: the
    /// player has started cutting the tree the route points at, so record the
    /// active tree. No harvest is waited for and no arrival radius applies.
    /// The observation is a fresh coordinate read, or the last observed
    /// position when the read fails, so a press is never lost to OCR. Every
    /// distinct press counts, however close in time and space to the last:
    /// in a dense stand the next tree can be a step away. Only a held key's
    /// autorepeat is folded into one press, at the dispatch.
    pub async fn on_visit_key(&self) {
        let _ = self.key_visit().await;
    }

    async fn key_visit(&self) -> Result<Option<NavigationRun>, NavigationError> {
        let _guard = self.operation.lock().await;
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(None);
        };
        if !run.visit_on_key {
            return Ok(None);
        }
        let Some(active) = run.active_stop() else {
            return Ok(None);
        };
        let observed = self
            .scan_position(&run.planet)
            .map(|read| (read.lon as f64, read.lat as f64))
            .unwrap_or((run.current_lon, run.current_lat));
        let now = naive_to_epoch(self.clock.now());
        self.record_active_visit(run.id, active, observed, "key", now)
            .await
            .map(Some)
    }

    /// Whether the run's latest visit from `source` was recorded moments ago
    /// within the arrival radius of `observed`: the same tree signalled again.
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
                        "SELECT visited_at, observed_lon, observed_lat FROM map_pin_visits WHERE run_id = ?1 AND source = ?2 ORDER BY visited_at DESC, id DESC LIMIT 1",
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

    /// Complete the active stop from an observation: the stop is marked
    /// visited, a durable per-pin visit is written (starting its cooldown),
    /// and the route advances to the next stop or completes.
    async fn record_active_visit(
        &self,
        run_id: i64,
        stop: &NavigationStop,
        (lon, lat): (f64, f64),
        source: &'static str,
        now: f64,
    ) -> Result<NavigationRun, NavigationError> {
        let observed_distance = distance((lon, lat), (stop.lon, stop.lat));
        let stop_id = stop.id;
        let pin_id = stop.pin_id;
        self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_runs SET current_lon = ?2, current_lat = ?3, last_position_at = ?4, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![run_id, lon, lat, now],
            )?;
            tx.execute(
                "UPDATE navigation_stops SET status = 'visited', completed_at = ?2, completion_source = ?3, observed_lon = ?4, observed_lat = ?5, observed_distance = ?6 WHERE id = ?1",
                rusqlite::params![stop_id, now, source, lon, lat, observed_distance],
            )?;
            tx.execute(
                "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)",
                rusqlite::params![pin_id, run_id, now, source, lon, lat, observed_distance],
            )?;
            activate_next_or_complete(&tx, run_id, now)?;
            tx.commit()?;
            Ok(())
        }).await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_live
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.release_input_if_idle();
        }
        (self.changed)();
        Ok(refreshed)
    }

    /// Scan the calibrated coordinate region, mapping every read failure to
    /// its typed `PositionUpdate` so the position paths share one grammar.
    fn scan_position(&self, planet: &str) -> Result<CoordRead, ScanFailure> {
        match self.coord_capture.scan((self.bounds)(planet)) {
            CoordScanOutcome::Read(read) => Ok(read),
            CoordScanOutcome::NoRegion => Err(ScanFailure::NoRegion),
            CoordScanOutcome::CaptureFailed => Err(ScanFailure::CaptureFailed),
            CoordScanOutcome::EngineUnavailable => Err(ScanFailure::EngineUnavailable),
            CoordScanOutcome::Unreadable { .. } => Err(ScanFailure::Unreadable),
            CoordScanOutcome::Implausible { .. } => Err(ScanFailure::Implausible),
        }
    }

    async fn apply_position(
        &self,
        run: NavigationRun,
        lon: f64,
        lat: f64,
        source: &str,
        outcome: &str,
    ) -> Result<PositionUpdate, NavigationError> {
        let now = naive_to_epoch(self.clock.now());
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
                let observed_distance = distance((lon, lat), (active.lon, active.lat));
                update_run_position(&self.db, run.id, lon, lat, now).await?;
                *self.pending_harvest.lock().expect("pending harvest") = Some(PendingHarvest {
                    stop_id,
                    pin_id,
                    name,
                    observed_lon: lon,
                    observed_lat: lat,
                    observed_distance,
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
            self.route_live
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.release_input_if_idle();
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
                    "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance) \
                     SELECT id, NULL, ?2, 'manual', 'manual', lon, lat, 0 FROM map_pins WHERE id = ?1",
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
            self.route_live
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.release_input_if_idle();
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
            self.route_live
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.release_input_if_idle();
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
        self.route_live
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.release_input_if_idle();
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
        let now = naive_to_epoch(self.clock.now());
        let run_id = run.id;
        self.db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE navigation_runs SET current_lon = ?2, current_lat = ?3, last_position_at = ?4, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![run_id, pending.observed_lon, pending.observed_lat, now],
            )?;
            tx.execute(
                "UPDATE navigation_stops SET status = 'visited', completed_at = ?2, completion_source = 'harvest', observed_lon = ?3, observed_lat = ?4, observed_distance = ?5 WHERE id = ?1",
                rusqlite::params![pending.stop_id, now, pending.observed_lon, pending.observed_lat, pending.observed_distance],
            )?;
            tx.execute(
                "INSERT INTO map_pin_visits (pin_id, run_id, visited_at, source, outcome, observed_lon, observed_lat, observed_distance) VALUES (?1, ?2, ?3, 'harvest', ?4, ?5, ?6, ?7)",
                rusqlite::params![pending.pin_id, run_id, now, pending.outcome, pending.observed_lon, pending.observed_lat, pending.observed_distance],
            )?;
            activate_next_or_complete(&tx, run_id, now)?;
            tx.commit()?;
            Ok(())
        }).await?;
        let refreshed = load_run(&self.db, run_id)
            .await?
            .ok_or(NavigationError::NoActiveRun)?;
        if refreshed.status == RunStatus::Completed {
            self.route_live
                .store(false, std::sync::atomic::Ordering::SeqCst);
            self.release_input_if_idle();
        }
        (self.changed)();
        Ok(refreshed)
    }

    /// The automatic arrival path: a harvest swing proves arrival. Scans the
    /// current position and runs full arrival matching (debounce, ambiguity
    /// safety, out-of-order replan) so a matched pending stop is recorded and
    /// the route advances without blocking tracker processing.
    async fn harvest_position(&self, outcome: &str) -> Result<PositionUpdate, NavigationError> {
        let _guard = self.operation.lock().await;
        let Some(run) = load_live_run(&self.db).await? else {
            return Ok(PositionUpdate::NoActiveRun);
        };
        // A route that visits on the interact key takes the key as the only
        // visit signal; harvests leave it untouched.
        if run.visit_on_key {
            return Ok(PositionUpdate::Updated(run));
        }
        let read = match self.scan_position(&run.planet) {
            Ok(read) => read,
            Err(failure) => return Ok(failure.into()),
        };
        let updated = self
            .apply_position(run, read.lon as f64, read.lat as f64, "harvest", outcome)
            .await?;
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
                let _ = self.db.with_writer(move |conn| {
                    conn.execute(
                        "INSERT INTO radar_calibration (singleton, centre_x, centre_y, north_x, north_y, radius_px, display_scale, updated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5, 1.0, ?6) ON CONFLICT(singleton) DO UPDATE SET centre_x = excluded.centre_x, centre_y = excluded.centre_y, north_x = excluded.north_x, north_y = excluded.north_y, radius_px = excluded.radius_px, display_scale = excluded.display_scale, updated_at = excluded.updated_at",
                        rusqlite::params![centre.0, centre.1, cursor.0, cursor.1, radius, now],
                    )?;
                    Ok(())
                }).await;
                *self.radar_phase.lock().expect("radar phase") = RadarCalibrationPhase::Idle;
                self.set_radar_listener_enabled(false);
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
    use super::*;
    use crate::clock::MockClock;
    use crate::coord_capture::{CoordCaptureProviders, CoordRegion};
    use crate::keystroke_source::MockKeystrokeSource;
    use crate::map_pins::{MapPinsService, NewMapPin};
    use crate::skill_panel::BgrImage;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

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

    type Fixture = (
        tempfile::TempDir,
        Arc<NavigationService>,
        Arc<Mutex<(i64, i64)>>,
        Arc<AtomicUsize>,
        Arc<MockKeystrokeSource>,
    );

    async fn navigation_fixture() -> Fixture {
        navigation_fixture_with_focus(None).await
    }

    async fn navigation_fixture_with_focus(focus: Option<GameFocusProbe>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let pins = MapPinsService::new(db.clone(), clock.clone());
        // Route stops are special-tree pins, so seed a tree configuration and
        // make every fixture pin an instance of it.
        let configs = crate::pin_configs::PinConfigsService::new(db.clone(), clock.clone());
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
                lon,
                lat,
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
        let position = Arc::new(Mutex::new((0_i64, 0_i64)));
        let changes = Arc::new(AtomicUsize::new(0));
        let input = Arc::new(MockKeystrokeSource::new());
        let service = spawn_navigation(
            db,
            clock,
            position.clone(),
            changes.clone(),
            input.clone(),
            focus,
        )
        .await;
        (dir, service, position, changes, input)
    }

    #[tokio::test]
    async fn custom_selection_is_an_exact_eligible_pin_allow_list() {
        let (dir, service, _position, _changes, _input) = navigation_fixture().await;
        let all = service
            .start("Calypso".into(), None, 0.0, 0.0, None, false)
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
                lon: 1.0,
                lat: 1.0,
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
                0.0,
                0.0,
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
            .start("Calypso".into(), None, 0.0, 0.0, Some(Vec::new()), false)
            .await
            .unwrap_err();
        assert!(matches!(error, NavigationError::EmptyPinSelection));
    }

    /// Build a navigation service reading its coordinates from `position`,
    /// so a second service can be spawned on the same database to exercise
    /// startup behaviour.
    async fn spawn_navigation(
        db: Db,
        clock: Arc<dyn Clock>,
        position: Arc<Mutex<(i64, i64)>>,
        changes: Arc<AtomicUsize>,
        input: Arc<MockKeystrokeSource>,
        focus: Option<GameFocusProbe>,
    ) -> Arc<NavigationService> {
        let read_position = position.clone();
        let cursor_position = position.clone();
        let capture = CoordCaptureService::new(CoordCaptureProviders {
            region: Arc::new(|| {
                Some(CoordRegion {
                    x: 0,
                    y: 0,
                    w: 8,
                    h: 8,
                })
            }),
            capture_region: Arc::new(|_, _, _, _| {
                Some(BgrImage {
                    data: vec![0; 3],
                    h: 1,
                    w: 1,
                })
            }),
            read_text: Arc::new(move |_| {
                let (lon, lat) = *read_position.lock().unwrap();
                Some((format!("{lon} {lat}"), 1.0))
            }),
            cursor_position: Arc::new(move || Some(*cursor_position.lock().unwrap())),
            ..CoordCaptureProviders::default()
        });
        let change_count = changes.clone();
        NavigationService::new(
            db,
            clock,
            capture,
            Arc::new(|_| {
                Some(CoordBounds {
                    lon_min: -100,
                    lon_max: 100,
                    lat_min: -100,
                    lat_max: 100,
                })
            }),
            Arc::new(move || {
                change_count.fetch_add(1, AtomicOrdering::SeqCst);
            }),
            Some(input),
            focus,
        )
        .await
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
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !matches!(
                service.radar_calibration_phase(),
                RadarCalibrationPhase::AwaitNorthEdge { .. }
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("centre confirmation");

        *position.lock().unwrap() = (100, 80);
        let second_input = input.clone();
        std::thread::spawn(move || {
            let now = chrono::Utc::now();
            second_input.inject("return", now, KeystrokeKind::Press);
            second_input.inject("return", now, KeystrokeKind::Release);
        })
        .join()
        .expect("off-runtime input dispatch");
        let geometry = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Some(geometry) = service.radar_geometry().await.unwrap() {
                    break geometry;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("north-edge confirmation");
        assert_eq!((geometry.centre_x, geometry.centre_y), (100, 100));
        assert_eq!((geometry.north_x, geometry.north_y), (100, 80));
        assert_eq!(geometry.radius_px, 20.0);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while service.radar_calibration_phase() != RadarCalibrationPhase::Idle {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("calibration teardown");
        listener.stop();
    }

    async fn key_visits(service: &NavigationService) -> Vec<(i64, String, f64, f64)> {
        let run_id = service.snapshot().await.unwrap().unwrap().id;
        service
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT pin_id, outcome, observed_lon, observed_lat FROM map_pin_visits WHERE run_id = ?1 AND source = 'key' ORDER BY id",
                )?;
                let rows = stmt.query_map([run_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .unwrap()
    }

    async fn await_key_visits(service: &NavigationService, count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while key_visits(service).await.len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("interact-key visit");
    }

    fn press_off_runtime(input: &Arc<MockKeystrokeSource>, kind: KeystrokeKind) {
        let input = input.clone();
        std::thread::spawn(move || input.inject(VISIT_KEY, chrono::Utc::now(), kind))
            .join()
            .expect("off-runtime key dispatch");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_interact_key_records_the_active_tree_from_the_dispatch_thread() {
        let (_dir, service, position, _changes, input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), true)
            .await
            .unwrap();
        assert!(run.visit_on_key);
        let first = run.active_stop().unwrap().clone();
        // Well outside the arrival radius: the key is the whole signal, so no
        // distance check holds the visit back.
        *position.lock().unwrap() = (90, 90);

        press_off_runtime(&input, KeystrokeKind::Press);
        await_key_visits(&service, 1).await;

        let snapshot = service.snapshot().await.unwrap().unwrap();
        let recorded = snapshot
            .stops
            .iter()
            .find(|stop| stop.id == first.id)
            .unwrap();
        assert_eq!(recorded.status, StopStatus::Visited);
        assert_eq!(recorded.completion_source.as_deref(), Some("key"));
        assert_ne!(snapshot.active_stop().unwrap().id, first.id);
        assert_eq!(
            key_visits(&service).await,
            vec![(first.pin_id, "key".to_owned(), 90.0, 90.0)]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_held_key_records_one_visit_and_each_new_press_records_the_next_tree() {
        let (_dir, service, position, _changes, input) = navigation_fixture().await;
        service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2, 3]), true)
            .await
            .unwrap();
        *position.lock().unwrap() = (30, 0);

        // Autorepeat delivers presses with no release between them.
        press_off_runtime(&input, KeystrokeKind::Press);
        press_off_runtime(&input, KeystrokeKind::Press);
        await_key_visits(&service, 1).await;
        // Released and pressed again from the same spot moments later: a
        // neighbouring tree in a dense stand, so it records the next stop.
        press_off_runtime(&input, KeystrokeKind::Release);
        press_off_runtime(&input, KeystrokeKind::Press);
        await_key_visits(&service, 2).await;
        let pins: Vec<i64> = key_visits(&service)
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
        let (_dir, service, position, _changes, input) =
            navigation_fixture_with_focus(Some(probe)).await;
        service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), true)
            .await
            .unwrap();
        *position.lock().unwrap() = (30, 0);

        press_off_runtime(&input, KeystrokeKind::Press);
        press_off_runtime(&input, KeystrokeKind::Release);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(key_visits(&service).await.is_empty());

        *focus.lock().unwrap() = GameFocus::Focused;
        press_off_runtime(&input, KeystrokeKind::Press);
        await_key_visits(&service, 1).await;
    }

    #[tokio::test]
    async fn a_key_route_ignores_harvests_and_a_detecting_route_ignores_the_key() {
        let (_dir, service, position, _changes, input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), true)
            .await
            .unwrap();
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);
        service.on_harvest(true).await;
        let snapshot = service.snapshot().await.unwrap().unwrap();
        assert_eq!(snapshot.active_stop().unwrap().id, first.id);
        assert!(snapshot.pending_harvest.is_none());

        // A route that detects cuts from harvests leaves the keyboard alone.
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        assert!(!run.visit_on_key);
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);
        service.on_visit_key().await;
        input.inject(VISIT_KEY, chrono::Utc::now(), KeystrokeKind::Press);
        let snapshot = service.snapshot().await.unwrap().unwrap();
        assert_eq!(snapshot.active_stop().unwrap().id, first.id);
        assert!(key_visits(&service).await.is_empty());
    }

    #[tokio::test]
    async fn route_progress_persists_supports_undo_and_stays_visible_at_completion() {
        let (_dir, service, position, changes, _input) = navigation_fixture().await;
        let mut run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        assert_eq!(run.stops.len(), 2);
        assert_eq!(run.active_stop().unwrap().status, StopStatus::Active);

        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);
        let PositionUpdate::Updated(updated) = service.mark_visited(false).await.unwrap() else {
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
        *position.lock().unwrap() = (second.lon as i64, second.lat as i64);
        let PositionUpdate::Updated(completed) = service.mark_visited(false).await.unwrap() else {
            panic!("visited at the active tree")
        };
        assert_eq!(completed.status, RunStatus::Completed);
        assert_eq!(
            service.snapshot().await.unwrap().unwrap().status,
            RunStatus::Completed
        );

        let restored = service.undo().await.unwrap();
        assert_eq!(restored.status, RunStatus::Active);
        assert!(restored.active_stop().is_some());
        assert!(changes.load(AtomicOrdering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn arriving_at_a_pending_stop_marks_it_and_replans_from_the_observed_position() {
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2, 3]), false)
            .await
            .unwrap();
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
        *position.lock().unwrap() = (pending.lon as i64, pending.lat as i64);

        let PositionUpdate::Updated(replanned) = service.harvest_position("success").await.unwrap()
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);
        service.harvest_position("success").await.unwrap();

        // The next tree is nine units away. A one-unit OCR shift is inside
        // both its arrival radius and the previous observation's debounce
        // radius, so this repeated swing must not consume the next stop.
        *position.lock().unwrap() = (first.lon as i64 + 1, first.lat as i64);
        let PositionUpdate::Updated(after_repeat) =
            service.harvest_position("success").await.unwrap()
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        // A(30,0) is active and D(39,0) is pending; (34,0) is within fifteen of
        // both. The harvest resolves to the active tree, not an ambiguity.
        let active = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (34, 0);
        let PositionUpdate::Updated(updated) = service.harvest_position("success").await.unwrap()
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let active = run.active_stop().unwrap().clone();

        // EU trees cut from well beyond the arrival radius. A swing with no tree
        // in range must not silently drop: it records nothing yet and stashes a
        // confirmation proposing the active tree.
        *position.lock().unwrap() = (60, 60);
        let PositionUpdate::Updated(after) = service.harvest_position("success").await.unwrap()
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
        let pending = service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .expect("a pending harvest confirmation");
        assert_eq!(pending.stop_id, active.id);
        assert_eq!(pending.name, active.name);

        // Confirming records the active tree as a harvest and advances.
        let resolved = service.resolve_harvest(true).await.unwrap();
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
        assert!(service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_none());
    }

    #[tokio::test]
    async fn a_far_harvest_dismissal_leaves_the_route_untouched() {
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let active = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (60, 60);
        service.harvest_position("success").await.unwrap();
        assert!(service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_some());

        let dismissed = service.resolve_harvest(false).await.unwrap();
        assert_eq!(dismissed.active_stop().unwrap().id, active.id);
        assert_eq!(
            dismissed
                .stops
                .iter()
                .filter(|stop| stop.status == StopStatus::Visited)
                .count(),
            0,
        );
        assert!(service
            .snapshot()
            .await
            .unwrap()
            .unwrap()
            .pending_harvest
            .is_none());
    }

    #[tokio::test]
    async fn a_harvest_amid_only_non_active_trees_stays_ambiguous() {
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2, 3]), false)
            .await
            .unwrap();
        // (50,4) is within fifteen of pending D(39,0) and B(60,9) but not of the
        // active A(30,0), so there is no active tree to fall back on: never guess.
        *position.lock().unwrap() = (50, 4);
        let PositionUpdate::Ambiguous(ambiguous) =
            service.harvest_position("success").await.unwrap()
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);

        let PositionUpdate::Updated(observed) = service.update_position().await.unwrap() else {
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (50, 50);

        let PositionUpdate::OutOfTolerance(pending) = service.mark_visited(false).await.unwrap()
        else {
            panic!("an out-of-range visit asks for confirmation")
        };
        // Position refreshed, but nothing recorded until the user confirms.
        assert!(pending
            .stops
            .iter()
            .all(|stop| stop.status != StopStatus::Visited));
        assert_eq!((pending.current_lon, pending.current_lat), (50.0, 50.0));

        let PositionUpdate::Updated(forced) = service.mark_visited(true).await.unwrap() else {
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
        let (_dir, service, position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, None, false)
            .await
            .unwrap();
        let first = run.active_stop().unwrap().clone();
        *position.lock().unwrap() = (first.lon as i64, first.lat as i64);
        service.mark_visited(false).await.unwrap();
        service.end().await.unwrap();

        let regenerated = service
            .start("Calypso".into(), None, 0.0, 0.0, None, false)
            .await
            .unwrap();
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
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        assert!(service.snapshot().await.unwrap().is_some());
        drop(service);

        // A fresh service on the same database does not restore the run: an
        // interrupted route is recovered by regeneration, not hydration.
        let db = Db::open(&dir.path().join("navigation.db")).await.unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let restarted = spawn_navigation(
            db,
            clock,
            position.clone(),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(MockKeystrokeSource::new()),
            None,
        )
        .await;
        assert!(restarted.snapshot().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cooling_the_active_tree_skips_it_and_replans() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
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
    async fn cooling_a_tree_excludes_it_from_the_next_route() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, None, false)
            .await
            .unwrap();
        let cooled = run.active_stop().unwrap().pin_id;
        service.end().await.unwrap();
        service.cooldown_pin(cooled).await.unwrap();
        let replanned = service
            .start("Calypso".into(), None, 0.0, 0.0, None, false)
            .await
            .unwrap();
        assert!(replanned.stops.iter().all(|stop| stop.pin_id != cooled));
    }

    #[tokio::test]
    async fn deleting_a_route_pin_drops_its_stop_and_replans() {
        let (_dir, service, _position, _changes, _input) = navigation_fixture().await;
        let run = service
            .start("Calypso".into(), None, 0.0, 0.0, Some(vec![1, 2]), false)
            .await
            .unwrap();
        let removed = run.active_stop().unwrap().pin_id;
        service.replan_after_pin_removed(removed).await.unwrap();
        let after = service.snapshot().await.unwrap().unwrap();
        assert!(after.stops.iter().all(|stop| stop.pin_id != removed));
        assert_eq!(after.status, RunStatus::Active);
        assert!(after.active_stop().is_some());
    }
}
