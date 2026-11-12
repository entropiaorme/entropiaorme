//! Coordinate capture: the player's position, read from the coordinates
//! the game prints under its radar, for one-click map pins and the route
//! position poll; and the radar itself, found for the guidance overlays.
//!
//! The game window is captured and the interface reader finds the radar
//! wherever the player has put it, at whatever size, and reads the `LON`
//! and `LAT` printed under it. Each value is read exactly or refused with
//! its own reason, and a position is answered only when both read: a
//! refusal carries no number, and nothing here ever pairs a fresh value
//! with a remembered one. The planet's name printed above the radar is
//! read too: a position under a radar that names another planet than the
//! one it is read for is refused.
//!
//! Once the radar has been found, the next read captures only its
//! neighbourhood (the disc, the band under it and the name above it); the
//! whole window is captured again when the radar is not found there.
//!
//! When the radar cannot be found on the player's setup at all, a screen
//! rectangle the player has drawn round the coordinates by hand (the
//! two-point calibration below) is read instead. It is an escape hatch:
//! it is read only when the radar is not found, so a refusal under a
//! found radar stands.
//!
//! Captures are taken one at a time: a pin drop, the position poll and
//! the guidance check never capture beside each other. When the desktop
//! does not permit a capture, the service remembers it: no automatic read
//! (the poll, a key press, a harvest, guidance) asks the player for
//! consent again until the player asks to try again.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::game_ui::geometry::{crop, BgrImage, Rect};
use crate::game_ui::profile::UiProfile;
use crate::game_ui::radar::{
    find_radar, read_coordinates, read_planet_label, read_region_coordinates, ring_scale,
    rings_found, CoordError, Coordinate, CoordinateBounds, Coordinates, Radar,
};
use crate::game_ui::seams::{FrameArea, FrameReader, FrameUnavailable, GameFrame, GameWindowFrame};
use crate::time::naive_to_epoch;

/// The persisted capture rectangle, in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordRegion {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// A planet the radar may name, and the names it may be printed as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPlanet {
    pub name: String,
    pub printed: Vec<String>,
}

/// The cursor-position seam (calibration corners come from it).
pub type CursorPosition = Arc<dyn Fn() -> Option<(i64, i64)> + Send + Sync>;
/// The hand-drawn region, when the player has drawn one.
pub type RegionProvider = Arc<dyn Fn() -> Option<CoordRegion> + Send + Sync>;
/// The persistence sink a completed calibration writes through.
pub type RegionSink = Arc<dyn Fn(CoordRegion) -> Result<(), String> + Send + Sync>;
/// Where scan debug artefacts (the captured crop and what the recogniser
/// answered) should be written, or None to write nothing.
pub type DebugDir = Arc<dyn Fn() -> Option<std::path::PathBuf> + Send + Sync>;

/// The seams the composition root wires in.
pub struct CoordCaptureProviders {
    /// The game window.
    pub frame: GameWindowFrame,
    /// The interface profile; `None` when it could not be loaded.
    pub profile: Option<Arc<UiProfile>>,
    /// The recogniser; `None` when none was loaded. It may still fail on
    /// a particular crop, which is that crop not reading.
    pub read_text: Option<FrameReader>,
    /// The planets the radar may name, for telling a reading under another
    /// planet's radar from one under the planet asked for.
    pub planets: Vec<KnownPlanet>,
    pub cursor_position: CursorPosition,
    /// The hand-drawn region, in screen coordinates. It is read from the
    /// same capture of the game window the radar was not found in.
    pub region: RegionProvider,
    pub persist_region: RegionSink,
    pub debug_dir: DebugDir,
}

impl Default for CoordCaptureProviders {
    fn default() -> Self {
        Self {
            frame: Arc::new(|_| Err(FrameUnavailable::WindowNotFound)),
            profile: None,
            read_text: None,
            planets: Vec::new(),
            cursor_position: Arc::new(|| None),
            region: Arc::new(|| None),
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

/// Who asked for a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The player, this moment: a capture may ask the desktop's consent.
    Player,
    /// The app on its own schedule or on a game event: once a capture was
    /// not permitted, it asks nothing until the player tries again.
    Automatic,
}

/// What the coordinates were read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSource {
    /// Under the radar, found in the game window.
    Radar,
    /// The player's hand-drawn region, because the radar was not found.
    Region,
}

/// The player's position, both values exactly as the screen showed them.
/// The client prints longitude and latitude only; there is no altitude
/// to read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordReading {
    pub lon: u32,
    pub lat: u32,
    pub source: ReadSource,
    /// When the screen was captured, in epoch seconds: the moment the
    /// player stood there, not the moment the read finished. For storing.
    pub read_at: f64,
    /// The same moment on the clock's monotonic reading: for every
    /// duration and ordering decision, which a step of the wall clock must
    /// not disturb.
    pub read_mono: f64,
}

/// Why one of the two values was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ValueRefusal {
    /// The place under the radar where it is printed runs off the screen.
    #[error("it is printed off the screen")]
    OffScreen,
    /// A window of the game lies over it.
    #[error("a window covers it")]
    Covered,
    /// Nothing is printed where it should be.
    #[error("it is not shown")]
    NotFound,
    /// Something is printed there, but it did not read cleanly as the
    /// value: misread, cut, or run into the scene behind it.
    #[error("it did not read cleanly")]
    Unclear,
    /// It read, but lies outside the coordinates of the planet the player
    /// is said to be on.
    #[error("it lies outside the planet's coordinates")]
    OutOfBounds,
}

impl From<&CoordError> for ValueRefusal {
    fn from(error: &CoordError) -> Self {
        match error {
            CoordError::OffScreen => ValueRefusal::OffScreen,
            CoordError::Covered => ValueRefusal::Covered,
            CoordError::NotFound => ValueRefusal::NotFound,
            CoordError::OutOfBounds { .. } => ValueRefusal::OutOfBounds,
            CoordError::Ambiguous
            | CoordError::Clipped
            | CoordError::Invalid { .. }
            | CoordError::NoSpace { .. }
            | CoordError::Misplaced { .. }
            | CoordError::WrongWidth { .. }
            | CoordError::WidthsDisagree { .. }
            | CoordError::NoRecogniser
            | CoordError::RadarNotFound
            | CoordError::RegionOutsideFrame => ValueRefusal::Unclear,
        }
    }
}

/// Why a scan produced no position. None of these carries a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoordRefusal {
    /// No recogniser or interface profile is loaded. Nothing the player
    /// does in the game helps.
    #[error("the coordinate reader is not available")]
    ReaderUnavailable,
    /// The planet the player is on has no known coordinates, so no value
    /// could be checked against them.
    #[error("the planet's coordinates are not known")]
    PlanetUnknown,
    #[error("the game window was not found")]
    GameWindowNotFound,
    #[error("the game window is off the screen that can be captured")]
    GameWindowOffScreen,
    #[error("the game window could not be captured")]
    CaptureFailed,
    /// Screen capture was not permitted. An automatic read asks nothing
    /// more until the player tries again.
    #[error("screen capture was not allowed")]
    CaptureNotAllowed,
    /// No radar is in view (it is hidden, covered or off the screen) and
    /// no region has been drawn by hand.
    #[error("the radar is not on the screen")]
    RadarNotFound,
    /// The radar names another planet than the one the read is for.
    #[error("the radar names another planet")]
    OtherPlanet,
    /// The radar is not on the screen, and the region the player drew
    /// round the coordinates by hand is not wholly inside the game window:
    /// the window has moved or shrunk since, and the region must be drawn
    /// again.
    #[error("the region drawn round the coordinates is outside the game window")]
    RegionOutsideWindow,
    /// At least one value was refused; each refused value says why, and a
    /// value that read is `None`, its number not kept.
    #[error("the coordinates were not read (longitude: {lon:?}, latitude: {lat:?})")]
    NotRead {
        lon: Option<ValueRefusal>,
        lat: Option<ValueRefusal>,
    },
}

impl From<FrameUnavailable> for CoordRefusal {
    fn from(error: FrameUnavailable) -> Self {
        match error {
            FrameUnavailable::WindowNotFound => CoordRefusal::GameWindowNotFound,
            FrameUnavailable::OutsideCapturableArea => CoordRefusal::GameWindowOffScreen,
            FrameUnavailable::CaptureFailed => CoordRefusal::CaptureFailed,
            FrameUnavailable::CaptureNotStarted => CoordRefusal::CaptureNotAllowed,
        }
    }
}

/// The radar's scale, as its ring distances give it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarScale {
    pub metres_per_pixel: f64,
    /// How far the disc's edge is from the player, in metres.
    pub edge_range_m: f64,
}

/// What became of the radar's scale on one look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleReading {
    /// The ring distances read and agree with the rings.
    Read(RadarScale),
    /// The rings are there, but their distances did not read: the client
    /// can hide them.
    LabelsUnread,
    /// No rings were found to measure.
    NoRings,
    /// No recogniser is loaded to read the distances with.
    NoReader,
}

/// The strip along the line from the radar's centre to its right edge
/// where the client prints the ring distances, in shares of the radius:
/// from `from` to `to` along the line, `half_height` above and below it.
/// Whatever is drawn over the radar stays out of it, or the scale it is
/// read for is lost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelStrip {
    pub from: f64,
    pub to: f64,
    pub half_height: f64,
}

/// The radar found on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarSighting {
    /// The player's dot at the radar's centre, in screen coordinates.
    pub centre: (f64, f64),
    /// The map disc's radius, in pixels.
    pub radius: f64,
    /// The scale, from this same look; never assumed.
    pub scale: ScaleReading,
}

/// Why the radar was not found on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SightingFailure {
    /// No interface profile is loaded.
    ReaderUnavailable,
    Frame(FrameUnavailable),
    RadarNotFound,
    /// The caller no longer wanted the look by the time it could be taken.
    Withdrawn,
}

/// A rectangle a calibration may persist: both dimensions at least this
/// many pixels (a degenerate rectangle reads nothing).
const MIN_REGION_PX: i64 = 4;

/// The largest rectangle a scan will capture per axis. The readout is a
/// small strip; a stored region beyond any plausible screen (a
/// hand-edited config) reads as no region rather than driving an
/// enormous grab.
const MAX_REGION_PX: i64 = 4096;

/// Where the radar was last found, in the game window's own coordinates.
#[derive(Debug, Clone, Copy)]
struct Neighbourhood {
    /// The part of the window to capture next: the radar's neighbourhood,
    /// cut at the window's edges.
    area: Rect,
    /// The window's client area as last captured whole.
    window: Rect,
}

/// The coordinate-capture service: the reads over the injected seams,
/// and the two-point calibration of the hand-drawn region.
pub struct CoordCaptureService {
    providers: CoordCaptureProviders,
    clock: Arc<dyn Clock>,
    /// Held across each capture and its read, so no two run at once.
    reading: Mutex<()>,
    /// The last capture was not permitted: automatic reads ask nothing
    /// until the player tries again (or a capture they ask for succeeds).
    capture_refused: AtomicBool,
    /// Where the radar was last found.
    neighbourhood: Mutex<Option<Neighbourhood>>,
    /// The current run of failed reads has left its evidence: a run of
    /// failures writes one picture, not one per poll.
    evidence_kept: AtomicBool,
    /// The last position read, and the planet it was read for.
    last_reading: Mutex<Option<(String, CoordReading)>>,
    phase: Mutex<CalibrationPhase>,
    /// The Enter listener's gate, attached post-construction (weak: the
    /// listener holds the service strongly). The service flips it so
    /// the listener is enabled exactly while a flow is live.
    confirm_listener: Mutex<Option<std::sync::Weak<CoordConfirmListener>>>,
}

impl CoordCaptureService {
    pub fn new(providers: CoordCaptureProviders, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            providers,
            clock,
            reading: Mutex::new(()),
            capture_refused: AtomicBool::new(false),
            neighbourhood: Mutex::new(None),
            evidence_kept: AtomicBool::new(false),
            last_reading: Mutex::new(None),
            phase: Mutex::new(CalibrationPhase::Idle),
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

    /// The hand-drawn region, through the provider seam.
    pub fn region(&self) -> Option<CoordRegion> {
        (self.providers.region)()
    }

    /// Current physical cursor position through the same injected seam used
    /// by coordinate-boundary calibration. Other pointer calibrations reuse
    /// this provider rather than reaching around the established flow.
    pub fn cursor_position(&self) -> Option<(i64, i64)> {
        (self.providers.cursor_position)()
    }

    /// Whether an Enter press should currently be observed at all: the
    /// listener's gate, so the flow being idle means Enter presses are
    /// ignored entirely.
    pub fn calibration_active(&self) -> bool {
        !matches!(self.calibration_phase(), CalibrationPhase::Idle)
    }

    /// Advance the flow on an Enter press: capture the cursor position
    /// as the pending corner. On the second corner the rectangle is
    /// normalised (corners may be given in any order), size-checked and
    /// persisted. Returns the phase after the press.
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
                }
                *phase = CalibrationPhase::Idle;
                drop(phase);
                self.set_listener_enabled(false);
                return CalibrationPhase::Idle;
            }
        }
        *phase
    }

    /// Whether the last capture was not permitted, so automatic reads are
    /// held until the player tries again.
    pub fn capture_refused(&self) -> bool {
        self.capture_refused.load(Ordering::SeqCst)
    }

    /// The player asks to try capturing again: the next read, automatic
    /// or not, may ask the desktop's consent once more.
    pub fn allow_capture_again(&self) {
        self.capture_refused.store(false, Ordering::SeqCst);
    }

    /// The last position read for `planet`, when the last position read
    /// was for it.
    pub fn last_reading(&self, planet: &str) -> Option<CoordReading> {
        self.last_reading
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(read_for, _)| read_for == planet)
            .map(|(_, reading)| *reading)
    }

    /// Capture `area` of the game window, remembering whether capture was
    /// permitted.
    fn capture(&self, area: FrameArea) -> Result<GameFrame, FrameUnavailable> {
        let got = (self.providers.frame)(area);
        match &got {
            Ok(_) => self.capture_refused.store(false, Ordering::SeqCst),
            Err(FrameUnavailable::CaptureNotStarted) => {
                self.capture_refused.store(true, Ordering::SeqCst)
            }
            Err(_) => {}
        }
        got
    }

    /// Capture round where the radar was last found and `look` there; when
    /// it does not find the radar wholly inside what was captured, capture
    /// the whole window and look again. `look` answers the radar it found,
    /// if any, with what it read. The neighbourhood is kept for next time.
    fn capture_radar<T>(
        &self,
        profile: &UiProfile,
        mut look: impl FnMut(&GameFrame) -> Option<(Radar, T)>,
    ) -> Result<(GameFrame, Option<(Radar, T)>), FrameUnavailable> {
        let known = *self
            .neighbourhood
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = known {
            // The window may have shrunk away from where the radar was: then
            // the whole window is captured, as when the radar has moved.
            let frame = match self.capture(FrameArea::Part(known.area)) {
                Ok(frame) => Some(frame),
                Err(FrameUnavailable::OutsideCapturableArea | FrameUnavailable::CaptureFailed) => {
                    None
                }
                Err(unavailable) => return Err(unavailable),
            };
            if let Some((frame, (radar, read))) =
                frame.and_then(|frame| look(&frame).map(|found| (frame, found)))
            {
                let needed =
                    in_window(&frame, radar.neighbourhood(&profile.radar)).intersect(&known.window);
                let captured = in_window(
                    &frame,
                    Rect::new(0, 0, frame.image.w as i32 - 1, frame.image.h as i32 - 1),
                );
                if needed.is_some_and(|needed| captured.intersect(&needed) == Some(needed)) {
                    self.remember(Some(Neighbourhood {
                        area: needed.unwrap_or(known.area),
                        window: known.window,
                    }));
                    return Ok((frame, Some((radar, read))));
                }
            }
        }
        let frame = self.capture(FrameArea::Whole)?;
        let found = look(&frame);
        let window = in_window(
            &frame,
            Rect::new(0, 0, frame.image.w as i32 - 1, frame.image.h as i32 - 1),
        );
        let neighbourhood = found.as_ref().and_then(|(radar, _)| {
            in_window(&frame, radar.neighbourhood(&profile.radar))
                .intersect(&window)
                .map(|area| Neighbourhood { area, window })
        });
        self.remember(neighbourhood);
        Ok((frame, found))
    }

    fn remember(&self, neighbourhood: Option<Neighbourhood>) {
        *self
            .neighbourhood
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = neighbourhood;
    }

    /// Read the player's position on `planet`: both values exactly as the
    /// screen shows them, each within `bounds` (the planet's coordinates),
    /// or a refusal saying why there is none. Nothing is captured without
    /// bounds to check against, or without a profile and a recogniser to
    /// read with, and an automatic read captures nothing while capture is
    /// not permitted.
    pub fn scan(
        &self,
        planet: &str,
        bounds: Option<CoordinateBounds>,
        trigger: Trigger,
    ) -> Result<CoordReading, CoordRefusal> {
        let bounds = bounds.ok_or(CoordRefusal::PlanetUnknown)?;
        let (Some(profile), Some(read_text)) =
            (self.providers.profile.as_deref(), &self.providers.read_text)
        else {
            return Err(CoordRefusal::ReaderUnavailable);
        };
        if trigger == Trigger::Automatic && self.capture_refused() {
            return Err(CoordRefusal::CaptureNotAllowed);
        }
        let _reading = self.reading.lock().unwrap_or_else(PoisonError::into_inner);
        // A crop the recogniser fails on reads as nothing, which no
        // coordinate's grammar accepts.
        let recogniser =
            |image: &BgrImage| Some(read_text(image).unwrap_or_else(|| (String::new(), 0.0)));
        let mut read_at = (0.0, 0.0);
        let (frame, found) = self.capture_radar(profile, |frame| {
            read_at = (naive_to_epoch(self.clock.now()), self.clock.monotonic());
            read_coordinates(&frame.image, profile, &bounds, &recogniser)
                .ok()
                .map(|coordinates| (coordinates.radar, coordinates))
        })?;
        let outcome = match found {
            Some((radar, coordinates)) => {
                let outcome = pair(&coordinates, read_at).and_then(|reading| {
                    self.check_planet(&frame.image, &radar, profile, planet, &recogniser)
                        .map(|()| reading)
                });
                if outcome.is_err() {
                    self.keep_evidence(
                        &radar_crop(&frame.image, &radar, profile),
                        &format!("{coordinates:#?}"),
                        &outcome,
                    );
                }
                outcome
            }
            None => match (self.providers.region)() {
                Some(region) => {
                    self.read_region(region, &frame, profile, &bounds, &recogniser, read_at)
                }
                None => Err(CoordRefusal::RadarNotFound),
            },
        };
        match &outcome {
            Ok(read) => {
                self.evidence_kept.store(false, Ordering::SeqCst);
                *self
                    .last_reading
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some((planet.to_string(), *read));
                tracing::info!(
                    target: "eo::coord_capture",
                    source = ?read.source,
                    "coordinates read"
                )
            }
            Err(refusal) => tracing::info!(
                target: "eo::coord_capture",
                %refusal,
                "coordinates not read"
            ),
        }
        outcome
    }

    /// Refuse a reading under a radar that names another planet than
    /// `planet`. A name that does not read vetoes nothing.
    fn check_planet(
        &self,
        image: &BgrImage,
        radar: &Radar,
        profile: &UiProfile,
        planet: &str,
        recogniser: &dyn crate::game_ui::field::Recogniser,
    ) -> Result<(), CoordRefusal> {
        let printed: Vec<&str> = self
            .providers
            .planets
            .iter()
            .flat_map(|known| known.printed.iter().map(String::as_str))
            .collect();
        let Some(read) = read_planet_label(image, radar, profile, &printed, recogniser) else {
            return Ok(());
        };
        let named = self
            .providers
            .planets
            .iter()
            .find(|known| known.printed.contains(&read));
        match named {
            Some(known) if !known.name.eq_ignore_ascii_case(planet) => {
                Err(CoordRefusal::OtherPlanet)
            }
            _ => Ok(()),
        }
    }

    /// Read the hand-drawn region, used only when the radar was not found,
    /// from `frame`, the capture of the whole game window the radar was
    /// looked for in, captured at `read_at`. The region is in screen
    /// coordinates, as the player drew it, and is placed in the frame by
    /// where the frame was taken from on the screen; a region the frame
    /// does not wholly hold is refused as outside the game window. The reader
    /// applies the grammar and the planet's bounds (see
    /// [`read_region_coordinates`]). Nor can the planet's name be read: it
    /// is printed above the radar, which is not in view.
    fn read_region(
        &self,
        region: CoordRegion,
        frame: &GameFrame,
        profile: &UiProfile,
        bounds: &CoordinateBounds,
        recogniser: &dyn crate::game_ui::field::Recogniser,
        read_at: (f64, f64),
    ) -> Result<CoordReading, CoordRefusal> {
        if region.w < MIN_REGION_PX
            || region.h < MIN_REGION_PX
            || region.w > MAX_REGION_PX
            || region.h > MAX_REGION_PX
        {
            return Err(CoordRefusal::RadarNotFound);
        }
        let corner = |at: i64, origin: i64, length: i64| {
            let from = i32::try_from(at - origin).ok()?;
            Some((from, from.checked_add(i32::try_from(length).ok()? - 1)?))
        };
        let (Some((x0, x1)), Some((y0, y1))) = (
            corner(region.x, frame.origin.0, region.w),
            corner(region.y, frame.origin.1, region.h),
        ) else {
            return Err(CoordRefusal::RegionOutsideWindow);
        };
        let rect = Rect::new(x0, y0, x1, y1);
        let coordinates = read_region_coordinates(&frame.image, rect, profile, bounds, recogniser)
            .map_err(|error| match error {
                CoordError::RegionOutsideFrame => CoordRefusal::RegionOutsideWindow,
                _ => CoordRefusal::ReaderUnavailable,
            })?;
        let value = |value: &Result<Coordinate, CoordError>| {
            value
                .as_ref()
                .map(|coordinate| coordinate.value)
                .map_err(ValueRefusal::from)
        };
        let outcome = both(
            value(&coordinates.lon),
            value(&coordinates.lat),
            ReadSource::Region,
            read_at,
        );
        if outcome.is_err() {
            self.keep_evidence(
                &crop(&frame.image, rect),
                &format!("{coordinates:#?}"),
                &outcome,
            );
        }
        outcome
    }

    /// Whether the radar can be looked for at all: the interface profile
    /// that describes it is loaded.
    pub fn can_sight_radar(&self) -> bool {
        self.providers.profile.is_some()
    }

    /// Find the radar on the screen, with its scale as the ring distances
    /// read on the same look. Always an automatic read: while capture is
    /// not permitted it captures nothing. `still_wanted` is asked once the
    /// capture's turn has come, so a look its caller gave up while waiting
    /// is never taken.
    pub fn sight_radar(
        &self,
        still_wanted: &dyn Fn() -> bool,
    ) -> Result<RadarSighting, SightingFailure> {
        let profile = self
            .providers
            .profile
            .as_deref()
            .ok_or(SightingFailure::ReaderUnavailable)?;
        if self.capture_refused() {
            return Err(SightingFailure::Frame(FrameUnavailable::CaptureNotStarted));
        }
        let _reading = self.reading.lock().unwrap_or_else(PoisonError::into_inner);
        if !still_wanted() {
            return Err(SightingFailure::Withdrawn);
        }
        let (frame, found) = self
            .capture_radar(profile, |frame| {
                find_radar(&frame.image, profile).map(|radar| (radar, ()))
            })
            .map_err(SightingFailure::Frame)?;
        let (radar, ()) = found.ok_or(SightingFailure::RadarNotFound)?;
        let scale = match &self.providers.read_text {
            None => ScaleReading::NoReader,
            Some(read_text) => {
                let recogniser = |image: &BgrImage| {
                    Some(read_text(image).unwrap_or_else(|| (String::new(), 0.0)))
                };
                match ring_scale(&frame.image, &radar, profile, &recogniser) {
                    Some(scale) => ScaleReading::Read(RadarScale {
                        metres_per_pixel: scale.metres_per_pixel,
                        edge_range_m: scale.edge_range_m,
                    }),
                    None if rings_found(&frame.image, &radar, profile) => {
                        ScaleReading::LabelsUnread
                    }
                    None => ScaleReading::NoRings,
                }
            }
        };
        // The dot's centre is the mean of its pixels' indices; a pixel
        // covers the square from its index to the next, so its middle is
        // half a pixel on.
        Ok(RadarSighting {
            centre: (
                frame.origin.0 as f64 + radar.x + 0.5,
                frame.origin.1 as f64 + radar.y + 0.5,
            ),
            radius: radar.radius,
            scale,
        })
    }

    /// How far right of the radar's centre, in pixels, the radar and the
    /// coordinates printed under it reach: what a window placed beside the
    /// radar must stay clear of, so it never covers what is read.
    pub fn reach_right(&self, radius: f64) -> f64 {
        let band_reach = self
            .providers
            .profile
            .as_ref()
            .map_or(0.0, |profile| profile.radar.coordinates.band_reach);
        radius * (1.0 + band_reach)
    }

    /// Where the radar's ring distances are printed, as the reader looks
    /// for them; `None` without a profile.
    pub fn label_strip(&self) -> Option<LabelStrip> {
        self.providers.profile.as_ref().map(|profile| {
            let rings = &profile.radar.rings;
            LabelStrip {
                from: rings.span[0],
                to: 1.0,
                half_height: rings.label_reach,
            }
        })
    }

    /// Write a failed read's evidence: the crop the values were read from
    /// as `coord-scan-last.png` and what was read there as
    /// `coord-scan-last.txt`. A clean read writes nothing, so the position
    /// poll never leaves a picture of where the player is; a run of failed
    /// reads writes its first, not one per poll. Best-effort: a failed
    /// write only logs.
    fn keep_evidence(
        &self,
        image: &BgrImage,
        read: &str,
        outcome: &Result<CoordReading, CoordRefusal>,
    ) {
        if self.evidence_kept.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(dir) = (self.providers.debug_dir)() else {
            return;
        };
        crate::screen_capture::write_debug_frame(&dir, "coord-scan-last.png", image);
        write_debug_report(&dir, image, read, outcome);
    }
}

/// `rect`, in `frame`'s image coordinates, in the game window's own.
fn in_window(frame: &GameFrame, rect: Rect) -> Rect {
    rect.offset(frame.offset.0 as i32, frame.offset.1 as i32)
}

/// Both values of a read under the radar, or why not.
fn pair(coordinates: &Coordinates, read_at: (f64, f64)) -> Result<CoordReading, CoordRefusal> {
    let value = |value: &Result<Coordinate, CoordError>| {
        value
            .as_ref()
            .map(|coordinate| coordinate.value)
            .map_err(ValueRefusal::from)
    };
    both(
        value(&coordinates.lon),
        value(&coordinates.lat),
        ReadSource::Radar,
        read_at,
    )
}

/// A position only when both values read; otherwise each value's reason,
/// and no number at all.
fn both(
    lon: Result<u32, ValueRefusal>,
    lat: Result<u32, ValueRefusal>,
    source: ReadSource,
    (read_at, read_mono): (f64, f64),
) -> Result<CoordReading, CoordRefusal> {
    match (lon, lat) {
        (Ok(lon), Ok(lat)) => Ok(CoordReading {
            lon,
            lat,
            source,
            read_at,
            read_mono,
        }),
        (lon, lat) => Err(CoordRefusal::NotRead {
            lon: lon.err(),
            lat: lat.err(),
        }),
    }
}

/// The radar and the band under it where the coordinates are printed.
fn radar_crop(image: &BgrImage, radar: &Radar, profile: &UiProfile) -> BgrImage {
    let coords = &profile.radar.coordinates;
    let r = radar.radius;
    let reach = (coords.band_reach * r).ceil() as i32;
    let disc = radar.disc();
    crop(
        image,
        Rect::new(
            disc.x0 - reach,
            disc.y0,
            disc.x1 + reach,
            (radar.y + r + coords.band[1] * r).ceil() as i32,
        ),
    )
}

/// Write the last failed scan's report beside its crop.
fn write_debug_report(
    dir: &Path,
    image: &BgrImage,
    read: &str,
    outcome: &Result<CoordReading, CoordRefusal>,
) {
    let report = format!(
        "outcome: {outcome:?}\ncrop: {}x{}\n{read}\n",
        image.w, image.h
    );
    if let Err(err) = std::fs::write(dir.join("coord-scan-last.txt"), report) {
        tracing::warn!(target: "eo::coord_capture", %err, "debug report not writable");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::game_ui::test_support::{
        bundled_profile, draw_window, paint_region, region_half, scenery, window_frame, Printed,
        RadarScene, RegionHalf, RINGS, RING_LABELS,
    };
    use std::sync::atomic::AtomicUsize;

    /// The coordinates of a planet that holds every drawn value.
    const PLANET: CoordinateBounds = CoordinateBounds {
        lon: (16384, 90112),
        lat: (24576, 98304),
    };

    const CENTRE: (i32, i32) = (900, 200);
    const RADIUS: i32 = 113;

    fn planets() -> Vec<KnownPlanet> {
        ["Calypso", "Arkadia"]
            .map(|name| KnownPlanet {
                name: name.to_string(),
                printed: vec![name.to_string()],
            })
            .to_vec()
    }

    fn read(service: &CoordCaptureService) -> Result<CoordReading, CoordRefusal> {
        service.scan("Calypso", Some(PLANET), Trigger::Player)
    }

    /// A radar with both values printed under it.
    fn radar_scene(lon: &str, lat: &str) -> RadarScene {
        RadarScene::with_coordinates(
            CENTRE,
            RADIUS,
            Some((lon, Printed::of(5))),
            Some((lat, Printed::of(5))),
        )
    }

    /// A stand-in game window: what it shows (or why there is nothing),
    /// and every capture asked of it.
    struct Window {
        shown: Mutex<Result<BgrImage, FrameUnavailable>>,
        asked: Mutex<Vec<FrameArea>>,
    }

    impl Window {
        fn showing(image: BgrImage) -> Arc<Self> {
            Arc::new(Self {
                shown: Mutex::new(Ok(image)),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<FrameArea> {
            self.asked.lock().unwrap().clone()
        }

        fn frame(self: &Arc<Self>) -> GameWindowFrame {
            let window = self.clone();
            Arc::new(move |area| {
                window.asked.lock().unwrap().push(area);
                let shown = window.shown.lock().unwrap().clone();
                shown.and_then(|image| window_frame(&image, (0, 0), area))
            })
        }
    }

    struct Rig {
        service: Arc<CoordCaptureService>,
        window: Arc<Window>,
        /// Halves of the hand-drawn region handed to the recogniser.
        region_reads: Arc<AtomicUsize>,
    }

    /// The service over `image` as the game window, read by `read_text`,
    /// with `region` as the hand-drawn region. The halves of a region
    /// painted on `image` (see [`paint_region`]) handed to `read_text` are
    /// counted.
    fn service_over(
        image: BgrImage,
        read_text: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync + 'static,
        region: Option<CoordRegion>,
    ) -> Rig {
        let window = Window::showing(image);
        let region_reads = Arc::new(AtomicUsize::new(0));
        let counted = region_reads.clone();
        let service = CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    if region_half(crop).is_some() {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                    read_text(crop)
                })),
                planets: planets(),
                region: Arc::new(move || region),
                ..CoordCaptureProviders::default()
            },
            Arc::new(MockClock::new(None, 0.0)),
        );
        Rig {
            service,
            window,
            region_reads,
        }
    }

    fn over_scene(scene: &RadarScene) -> Rig {
        service_over(scene.image.clone(), scene.recogniser(), None)
    }

    #[test]
    fn both_values_read_under_the_radar_as_of_the_moment_of_capture() {
        let scene = radar_scene("LON 67739", "LAT 87740");
        let clock = Arc::new(MockClock::new(None, 0.0));
        let captured_at = (naive_to_epoch(clock.now()), clock.monotonic());
        // Recognising takes a while; the reading is as of the capture.
        let inner = scene.recogniser();
        let slow = clock.clone();
        let window = Window::showing(scene.image.clone());
        let service = CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    slow.advance(5.0).expect("forward");
                    inner(crop)
                })),
                ..CoordCaptureProviders::default()
            },
            clock.clone(),
        );
        let reading = read(&service).expect("both values read");
        assert_eq!((reading.lon, reading.lat), (67739, 87740));
        assert_eq!(reading.source, ReadSource::Radar);
        assert_eq!((reading.read_at, reading.read_mono), captured_at);
        assert!(clock.monotonic() > captured_at.1);
        assert_eq!(service.last_reading("Calypso"), Some(reading));
        assert_eq!(service.last_reading("Arkadia"), None);
    }

    #[test]
    fn one_value_refused_is_no_position_and_carries_no_number() {
        let mut scene = RadarScene::with_coordinates(
            (900, 200),
            169,
            Some(("LON 68520", Printed::of(5))),
            Some(("LAT 87611", Printed::of(5))),
        );
        // A window over the disc's left side and the start of the band.
        draw_window(&mut scene.image, Rect::new(300, 40, 900 - 169 + 80, 650));
        let rig = over_scene(&scene);
        assert_eq!(
            read(&rig.service),
            Err(CoordRefusal::NotRead {
                lon: Some(ValueRefusal::Covered),
                lat: None,
            })
        );
        assert_eq!(rig.service.last_reading("Calypso"), None);
    }

    #[test]
    fn a_value_outside_the_planet_or_misread_is_refused() {
        let scene = radar_scene("LON 67739", "LAT 87740");
        let rig = over_scene(&scene);
        let small = CoordinateBounds {
            lon: (60000, 70000),
            lat: (60000, 70000),
        };
        assert_eq!(
            rig.service.scan("Calypso", Some(small), Trigger::Player),
            Err(CoordRefusal::NotRead {
                lon: None,
                lat: Some(ValueRefusal::OutOfBounds),
            })
        );
        // Readings a digit short, a digit long, and with a misread label:
        // each refused for its own reason, never answered.
        for (lon, why) in [
            ("L0N 67739", ValueRefusal::Unclear),
            ("LON 6773", ValueRefusal::Unclear),
            ("LON 677390", ValueRefusal::Unclear),
        ] {
            let scene = radar_scene(lon, "LAT 87740");
            let rig = over_scene(&scene);
            assert_eq!(
                read(&rig.service),
                Err(CoordRefusal::NotRead {
                    lon: Some(why),
                    lat: None,
                }),
                "{lon}"
            );
        }
    }

    #[test]
    fn a_radar_naming_another_planet_is_not_read_for_this_one() {
        let mut scene = radar_scene("LON 67739", "LAT 87740");
        scene.planet_label(CENTRE, RADIUS, "Arkadia");
        let rig = over_scene(&scene);
        assert_eq!(read(&rig.service), Err(CoordRefusal::OtherPlanet));
        assert_eq!(rig.service.last_reading("Calypso"), None);
        // Read for the planet it names, it reads.
        assert!(rig
            .service
            .scan("Arkadia", Some(PLANET), Trigger::Player)
            .is_ok());

        // Its own name, or a name that does not read, vetoes nothing.
        for label in ["Calypso", "Calyps0"] {
            let mut scene = radar_scene("LON 67739", "LAT 87740");
            scene.planet_label(CENTRE, RADIUS, label);
            assert!(read(&over_scene(&scene).service).is_ok(), "{label}");
        }
    }

    /// The hand-drawn region, in screen coordinates.
    const REGION: CoordRegion = CoordRegion {
        x: 10,
        y: 10,
        w: 120,
        h: 20,
    };

    /// A game window with no radar, at the screen's corner, showing the
    /// hand-drawn region's pixels painted where it was drawn.
    fn region_window() -> BgrImage {
        let mut image = scenery(640, 400, 3);
        paint_region(&mut image, Rect::new(10, 10, 129, 29));
        image
    }

    /// A recogniser that reads the region's halves as `left` and `right`.
    fn halves(
        left: &'static str,
        right: &'static str,
    ) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
        move |crop: &BgrImage| {
            let text = match region_half(crop) {
                Some(RegionHalf::Left) => left,
                Some(RegionHalf::Right) => right,
                None => "",
            };
            Some((text.to_string(), 0.93))
        }
    }

    #[test]
    fn without_the_radar_or_a_region_nothing_is_read() {
        let rig = service_over(region_window(), halves("LON 61234", "LAT 75456"), None);
        assert_eq!(read(&rig.service), Err(CoordRefusal::RadarNotFound));
        assert_eq!(rig.region_reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_hand_drawn_region_is_read_when_the_radar_is_not_found() {
        let rig = service_over(
            region_window(),
            halves("LON 61234", "LAT 75456"),
            Some(REGION),
        );
        let reading = read(&rig.service).expect("the region reads");
        assert_eq!(
            (reading.lon, reading.lat, reading.source),
            (61234, 75456, ReadSource::Region)
        );
        // Both halves are read from the capture of the game window the
        // radar was looked for in: nothing else is captured.
        assert_eq!(rig.region_reads.load(Ordering::SeqCst), 2);
        assert_eq!(rig.window.asked(), vec![FrameArea::Whole]);

        // The same grammar and bounds apply to the region.
        for (left, right, want) in [
            (
                "L0N 61234",
                "LAT 75456",
                (Some(ValueRefusal::Unclear), None),
            ),
            (
                "LON 61234",
                "LAT 5456",
                (None, Some(ValueRefusal::OutOfBounds)),
            ),
            (
                "LON 612 34",
                "LAT 75456",
                (Some(ValueRefusal::Unclear), None),
            ),
            // Halves that read the same did not separate the values.
            (
                "LON 61234 LAT 75456",
                "LON 61234 LAT 75456",
                (Some(ValueRefusal::Unclear), Some(ValueRefusal::Unclear)),
            ),
        ] {
            let rig = service_over(region_window(), halves(left, right), Some(REGION));
            assert_eq!(
                read(&rig.service),
                Err(CoordRefusal::NotRead {
                    lon: want.0,
                    lat: want.1
                }),
                "{left:?} {right:?}"
            );
        }
    }

    #[test]
    fn the_region_is_placed_by_where_the_game_window_is_on_the_screen() {
        // The game window sits at (100, 50) on the screen: the region drawn
        // at (110, 60) there is at (10, 10) in the window.
        let image = region_window();
        let clock = Arc::new(MockClock::new(None, 0.0));
        let captured_at = (naive_to_epoch(clock.now()), clock.monotonic());
        let slow = clock.clone();
        let inner = Arc::new(halves("LON 61234", "LAT 75456"));
        let service = |drawn: CoordRegion| {
            let image = image.clone();
            let slow = slow.clone();
            let inner = inner.clone();
            CoordCaptureService::new(
                CoordCaptureProviders {
                    frame: Arc::new(move |area| window_frame(&image, (100, 50), area)),
                    profile: Some(Arc::new(bundled_profile())),
                    // Recognising takes a while; the reading is as of the
                    // capture.
                    read_text: Some(Arc::new(move |crop: &BgrImage| {
                        slow.advance(5.0).expect("forward");
                        (*inner)(crop)
                    })),
                    region: Arc::new(move || Some(drawn)),
                    ..CoordCaptureProviders::default()
                },
                clock.clone(),
            )
        };
        let placed = CoordRegion {
            x: 110,
            y: 60,
            ..REGION
        };
        let reading = read(&service(placed)).expect("the region reads");
        assert_eq!(
            (reading.lon, reading.lat, reading.source),
            (61234, 75456, ReadSource::Region)
        );
        assert_eq!((reading.read_at, reading.read_mono), captured_at);
        // Drawn where the window is not, or running off its edge, the
        // region is outside the game window, and is not clamped to it.
        for drawn in [
            REGION,
            CoordRegion {
                x: 100 + 600,
                y: 60,
                ..REGION
            },
        ] {
            assert_eq!(
                read(&service(drawn)),
                Err(CoordRefusal::RegionOutsideWindow),
                "{drawn:?}"
            );
        }
    }

    #[test]
    fn a_refusal_under_a_found_radar_is_not_answered_from_the_region() {
        let mut scene = radar_scene("LON 67739", "LAT 87740");
        draw_window(&mut scene.image, Rect::new(300, 40, 900 - 113 + 60, 650));
        // The region is drawn on the same screen, and would read cleanly.
        paint_region(&mut scene.image, Rect::new(10, 10, 129, 29));
        let radar = scene.recogniser();
        let region = halves("LON 61234", "LAT 75456");
        let recogniser = move |crop: &BgrImage| match region_half(crop) {
            Some(_) => region(crop),
            None => radar(crop),
        };
        let rig = service_over(scene.image.clone(), recogniser, Some(REGION));
        assert_eq!(
            read(&rig.service),
            Err(CoordRefusal::NotRead {
                lon: Some(ValueRefusal::Covered),
                lat: None,
            })
        );
        assert_eq!(rig.region_reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn after_the_radar_is_found_only_its_neighbourhood_is_captured() {
        let scene = radar_scene("LON 67739", "LAT 87740");
        let rig = over_scene(&scene);
        assert!(read(&rig.service).is_ok());
        assert!(read(&rig.service).is_ok());
        let asked = rig.window.asked();
        assert_eq!(asked[0], FrameArea::Whole);
        let FrameArea::Part(part) = asked[1] else {
            panic!("the second read captured {:?}", asked[1]);
        };
        // Round the disc and its bands, far smaller than the window.
        assert!(part.contains(CENTRE.0, CENTRE.1));
        assert!(
            part.width() < 3 * RADIUS && part.height() < 4 * RADIUS,
            "{part:?}"
        );
        assert_eq!(asked.len(), 2);
    }

    /// A service over `window`, read by whatever `reader` holds.
    fn switchable(
        window: &Arc<Window>,
        reader: &Arc<Mutex<FrameReader>>,
    ) -> Arc<CoordCaptureService> {
        let reading = reader.clone();
        CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    let read = reading.lock().unwrap().clone();
                    read(crop)
                })),
                ..CoordCaptureProviders::default()
            },
            Arc::new(MockClock::new(None, 0.0)),
        )
    }

    #[test]
    fn a_radar_moved_away_from_where_it_was_is_found_again_in_the_whole_window() {
        let first = radar_scene("LON 67739", "LAT 87740");
        let window = Window::showing(first.image.clone());
        let reader: Arc<Mutex<FrameReader>> = Arc::new(Mutex::new(Arc::new(first.recogniser())));
        let service = switchable(&window, &reader);
        assert!(read(&service).is_ok());

        let moved = RadarScene::with_coordinates(
            (300, 300),
            RADIUS,
            Some(("LON 61111", Printed::of(5))),
            Some(("LAT 72222", Printed::of(5))),
        );
        *window.shown.lock().unwrap() = Ok(moved.image.clone());
        *reader.lock().unwrap() = Arc::new(moved.recogniser());
        let reading = read(&service).expect("found again");
        assert_eq!((reading.lon, reading.lat), (61111, 72222));
        let asked = window.asked();
        assert_eq!(asked.len(), 3);
        assert!(matches!(asked[1], FrameArea::Part(_)), "{asked:?}");
        assert_eq!(asked[2], FrameArea::Whole);
    }

    #[test]
    fn an_automatic_read_never_asks_again_once_capture_was_not_permitted() {
        let rig = service_over(region_window(), halves("LON 61234", "LAT 75456"), None);
        *rig.window.shown.lock().unwrap() = Err(FrameUnavailable::CaptureNotStarted);
        for _ in 0..3 {
            assert_eq!(
                rig.service
                    .scan("Calypso", Some(PLANET), Trigger::Automatic),
                Err(CoordRefusal::CaptureNotAllowed)
            );
        }
        assert_eq!(
            rig.service.sight_radar(&|| true),
            Err(SightingFailure::Frame(FrameUnavailable::CaptureNotStarted))
        );
        assert_eq!(rig.window.asked().len(), 1, "asked once, never again");
        // The player asks: that read may ask once more.
        assert_eq!(read(&rig.service), Err(CoordRefusal::CaptureNotAllowed));
        assert_eq!(rig.window.asked().len(), 2);
        // The player tries again from the overlay, and capture is allowed.
        *rig.window.shown.lock().unwrap() = Ok(scenery(640, 400, 3));
        rig.service.allow_capture_again();
        assert_eq!(
            rig.service
                .scan("Calypso", Some(PLANET), Trigger::Automatic),
            Err(CoordRefusal::RadarNotFound)
        );
        assert!(!rig.service.capture_refused());
    }

    #[test]
    fn nothing_is_captured_without_a_reader_or_a_planet() {
        let scene = radar_scene("LON 67739", "LAT 87740");
        let rig = over_scene(&scene);
        assert_eq!(
            rig.service.scan("Calypso", None, Trigger::Player),
            Err(CoordRefusal::PlanetUnknown)
        );
        assert!(rig.window.asked().is_empty());

        let window = Window::showing(scene.image.clone());
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(None, 0.0));
        let no_profile = CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                read_text: Some(Arc::new(scene.recogniser())),
                ..CoordCaptureProviders::default()
            },
            clock.clone(),
        );
        let no_recogniser = CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                profile: Some(Arc::new(bundled_profile())),
                ..CoordCaptureProviders::default()
            },
            clock,
        );
        for service in [no_profile, no_recogniser] {
            assert_eq!(read(&service), Err(CoordRefusal::ReaderUnavailable));
        }
        assert!(window.asked().is_empty());
    }

    #[test]
    fn each_way_of_having_no_frame_is_told_apart() {
        for (unavailable, refusal) in [
            (
                FrameUnavailable::WindowNotFound,
                CoordRefusal::GameWindowNotFound,
            ),
            (
                FrameUnavailable::OutsideCapturableArea,
                CoordRefusal::GameWindowOffScreen,
            ),
            (FrameUnavailable::CaptureFailed, CoordRefusal::CaptureFailed),
            (
                FrameUnavailable::CaptureNotStarted,
                CoordRefusal::CaptureNotAllowed,
            ),
        ] {
            let service = CoordCaptureService::new(
                CoordCaptureProviders {
                    frame: Arc::new(move |_| Err(unavailable)),
                    profile: Some(Arc::new(bundled_profile())),
                    read_text: Some(Arc::new(|_| Some(("LON 61234".into(), 0.9)))),
                    region: Arc::new(|| Some(REGION)),
                    ..CoordCaptureProviders::default()
                },
                Arc::new(MockClock::new(None, 0.0)),
            );
            assert_eq!(read(&service), Err(refusal));
        }
    }

    #[test]
    fn a_run_of_refused_reads_keeps_its_first_evidence_and_a_clean_read_none() {
        let dir = tempfile::tempdir().unwrap();
        let debug = dir.path().join("debug");
        let clean = radar_scene("LON 67739", "LAT 87740");
        let misread = radar_scene("LON 67739", "LAT 8774O");
        let window = Window::showing(clean.image.clone());
        let reader: Arc<Mutex<FrameReader>> = Arc::new(Mutex::new(Arc::new(clean.recogniser())));
        let reading = reader.clone();
        let target = debug.clone();
        let service = CoordCaptureService::new(
            CoordCaptureProviders {
                frame: window.frame(),
                profile: Some(Arc::new(bundled_profile())),
                read_text: Some(Arc::new(move |crop: &BgrImage| {
                    let read = reading.lock().unwrap().clone();
                    read(crop)
                })),
                debug_dir: Arc::new(move || Some(target.clone())),
                ..CoordCaptureProviders::default()
            },
            Arc::new(MockClock::new(None, 0.0)),
        );
        assert!(read(&service).is_ok());
        assert!(!debug.join("coord-scan-last.png").exists());

        *window.shown.lock().unwrap() = Ok(misread.image.clone());
        *reader.lock().unwrap() = Arc::new(misread.recogniser());
        assert!(read(&service).is_err());
        let report = std::fs::read_to_string(debug.join("coord-scan-last.txt")).unwrap();
        assert!(report.contains("LAT 8774O"), "{report}");
        // The same failure again leaves the first evidence in place.
        std::fs::remove_file(debug.join("coord-scan-last.txt")).unwrap();
        assert!(read(&service).is_err());
        assert!(!debug.join("coord-scan-last.txt").exists());
        // A clean read ends the run: the next failure keeps its own.
        *window.shown.lock().unwrap() = Ok(clean.image.clone());
        *reader.lock().unwrap() = Arc::new(clean.recogniser());
        assert!(read(&service).is_ok());
        *window.shown.lock().unwrap() = Ok(misread.image.clone());
        *reader.lock().unwrap() = Arc::new(misread.recogniser());
        assert!(read(&service).is_err());
        assert!(debug.join("coord-scan-last.txt").exists());
    }

    #[test]
    fn the_radar_is_sighted_on_the_screen_with_what_became_of_its_scale() {
        let centre = (640, 380);
        let mut scene = RadarScene::new(41);
        scene.radar(centre, RADIUS, &RINGS);
        let unlabelled = scene.image.clone();
        scene.ring_labels(centre, &RINGS, &RING_LABELS);
        let mut plain = RadarScene::new(42);
        plain.radar(centre, RADIUS, &[]);
        let sight = |image: BgrImage, read_text: Option<FrameReader>| {
            CoordCaptureService::new(
                CoordCaptureProviders {
                    // The game window sits away from the screen's corner.
                    frame: Arc::new(move |area| window_frame(&image, (100, 50), area)),
                    profile: Some(Arc::new(bundled_profile())),
                    read_text,
                    ..CoordCaptureProviders::default()
                },
                Arc::new(MockClock::new(None, 0.0)),
            )
            .sight_radar(&|| true)
        };
        let found = sight(scene.image.clone(), Some(Arc::new(scene.recogniser())))
            .expect("the radar is found");
        // The dot is drawn on pixels 639 to 641: its middle is 640.5 within
        // the window, which sits at (100, 50).
        assert!((found.centre.0 - 740.5).abs() <= 0.5, "{found:?}");
        assert!((found.centre.1 - 430.5).abs() <= 0.5, "{found:?}");
        assert!((found.radius - RADIUS as f64).abs() <= 1.5, "{found:?}");
        let ScaleReading::Read(scale) = found.scale else {
            panic!("the rings are labelled: {found:?}");
        };
        assert!(
            (scale.metres_per_pixel / (200.0 / 103.0) - 1.0).abs() < 0.01,
            "{scale:?}"
        );

        let scale_of = |image: BgrImage, read_text: Option<FrameReader>| {
            sight(image, read_text).expect("found").scale
        };
        let reader: FrameReader = Arc::new(scene.recogniser());
        assert_eq!(
            scale_of(unlabelled.clone(), Some(reader.clone())),
            ScaleReading::LabelsUnread
        );
        assert_eq!(
            scale_of(plain.image.clone(), Some(reader)),
            ScaleReading::NoRings
        );
        assert_eq!(scale_of(unlabelled, None), ScaleReading::NoReader);
        assert_eq!(
            sight(scenery(640, 400, 9), None),
            Err(SightingFailure::RadarNotFound)
        );
    }

    #[test]
    fn a_look_given_up_while_waiting_its_turn_captures_nothing() {
        let scene = radar_scene("LON 67739", "LAT 87740");
        let rig = over_scene(&scene);
        assert_eq!(
            rig.service.sight_radar(&|| false),
            Err(SightingFailure::Withdrawn)
        );
        assert!(rig.window.asked().is_empty());
    }

    /// Providers for the calibration flow: the cursor and the sink.
    fn calibration_providers() -> CoordCaptureProviders {
        CoordCaptureProviders {
            cursor_position: Arc::new(|| Some((10, 20))),
            ..CoordCaptureProviders::default()
        }
    }

    fn calibration_service(providers: CoordCaptureProviders) -> Arc<CoordCaptureService> {
        CoordCaptureService::new(providers, Arc::new(MockClock::new(None, 0.0)))
    }

    #[test]
    fn the_two_point_flow_normalises_and_persists() {
        let persisted = Arc::new(Mutex::new(None));
        let sink = persisted.clone();
        let cursor_calls = Arc::new(AtomicUsize::new(0));
        let counter = cursor_calls.clone();
        let mut providers = calibration_providers();
        // Second corner arrives up-left of the first: normalisation duty.
        providers.cursor_position = Arc::new(move || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            Some(if call == 0 { (200, 100) } else { (50, 40) })
        });
        providers.persist_region = Arc::new(move |region| {
            *sink.lock().unwrap() = Some(region);
            Ok(())
        });
        let service = calibration_service(providers);

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
    }

    #[test]
    fn a_degenerate_rectangle_rearms_the_second_corner() {
        let mut providers = calibration_providers();
        providers.cursor_position = Arc::new(|| Some((100, 100)));
        let service = calibration_service(providers);
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
        let service = calibration_service(calibration_providers());
        assert_eq!(service.on_confirm(), CalibrationPhase::Idle);
        assert!(!service.calibration_active());
    }
}
