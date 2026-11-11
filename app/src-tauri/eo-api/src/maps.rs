//! The planet-maps family: the bundled map catalogue read and the raster
//! fetch behind it.
//!
//! The catalogue is a shipped resource (`resources/maps/`): per-planet
//! rasters plus tile-grid calibration, loaded once at composition into a
//! [`PlanetMapStore`]. The typed read serves the full catalogue (the
//! frontend derives its pixel/game-unit transforms from the calibration
//! fields); the raster itself is raw bytes and rides a bespoke shell
//! command outside the manifest. A facade composed without the bundle
//! serves an empty catalogue: the maps surface stands down, nothing
//! errors at startup.

use std::sync::Arc;

use eo_services::map_pins::{MapPinsError, NewMapPin};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::settings::double_option;
use crate::Nullable;
use crate::{Api, ApiError};

use eo_services::coord_capture::{
    CoordReading, CoordRefusal as ServiceCoordRefusal, ReadSource, Trigger, ValueRefusal,
};
use eo_services::game_ui::radar::CoordinateBounds;
use eo_services::navigation::{
    GuidanceLoss, GuidanceStop, NavigationError, NavigationRun as ServiceNavigationRun,
    PlacementSource, PollStop, PositionUpdate, RadarCalibrationPhase,
    RadarGuidance as ServiceGuidance, RunStatus as ServiceRunStatus, ScaleMissing,
    StopStatus as ServiceStopStatus,
};

/// A map's coordinate window in game units: the plausibility gate for
/// any coordinate claimed to lie on it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanetMapBounds {
    pub lon_min: i64,
    pub lon_max: i64,
    pub lat_min: i64,
    pub lat_max: i64,
}

/// A calibrated map's placement on the global tile grid, with the
/// per-axis pixel scales the frontend renders through.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanetMapCalibration {
    pub tile_origin_x: i64,
    pub tile_origin_y: i64,
    pub tile_width: i64,
    pub tile_height: i64,
    pub units_per_pixel_x: f64,
    pub units_per_pixel_y: f64,
    pub bounds: PlanetMapBounds,
}

/// One planet/instance map in the bundled catalogue. `calibration` is
/// null for a view-only map (displayable, but coordinates cannot be
/// placed on it); `technical_name` is null when the in-game waypoint
/// name is unknown.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanetMap {
    pub name: String,
    pub technical_name: Nullable<String>,
    pub image_mime: String,
    pub image_width_px: u32,
    pub image_height_px: u32,
    pub calibration: Nullable<PlanetMapCalibration>,
}

impl Api {
    /// The bundled planet-map catalogue, in bundle order; empty when the
    /// facade was composed without the map bundle.
    pub fn planet_maps(&self) -> Result<Vec<PlanetMap>, ApiError> {
        let Some(store) = self.planet_maps.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(store.records().iter().map(to_dto).collect())
    }

    /// A planet map's raster bytes (the shell base64-encodes them for a
    /// `data:` URL; the MIME type rides the catalogue read).
    pub fn planet_map_image(&self, planet_name: &str) -> Result<Vec<u8>, ApiError> {
        let store = self
            .planet_maps
            .as_ref()
            .ok_or_else(|| ApiError::invalid_state("planet-map bundle unavailable"))?;
        store
            .image_bytes(planet_name)
            .ok_or_else(|| ApiError::not_found(format!("no map for planet {planet_name}")))
    }
}

// ── Route navigation ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NavigationRunStatus {
    Active,
    Paused,
    Completed,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NavigationStopStatus {
    Pending,
    Active,
    Visited,
    Skipped,
}

/// A harvest awaiting the player's confirm or dismiss in the overlay: one
/// beyond the arrival radius, or one whose position did not read. Names the
/// proposed (active) tree and how far the harvest was read to be from it;
/// `distance` is null when no position was read for the harvest.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingHarvest {
    pub name: String,
    pub distance: Nullable<f64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NavigationStop {
    pub id: i64,
    pub pin_id: i64,
    pub ordinal: i64,
    pub status: NavigationStopStatus,
    pub name: String,
    pub icon: String,
    pub lon: f64,
    pub lat: f64,
    pub completed_at: Nullable<f64>,
    pub completion_source: Nullable<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NavigationRun {
    pub id: i64,
    pub planet: String,
    pub map_view_id: Nullable<i64>,
    pub map_view_name: Nullable<String>,
    pub status: NavigationRunStatus,
    pub start_lon: f64,
    pub start_lat: f64,
    pub current_lon: f64,
    pub current_lat: f64,
    pub last_position_at: Nullable<f64>,
    /// How many seconds ago the route's position was read, on a clock no
    /// change of the wall clock disturbs; null when no position has been
    /// read for the route. Steering from an old position is misleading.
    pub position_age_s: Nullable<f64>,
    pub hop_count: i64,
    /// Trees are recorded visited on the game's interact key rather than
    /// detected from harvests.
    pub visit_on_key: bool,
    pub updated_at: f64,
    pub distance_to_active: Nullable<f64>,
    /// Degrees clockwise from north.
    pub bearing_degrees: Nullable<f64>,
    pub pending_harvest: Nullable<PendingHarvest>,
    pub stops: Vec<NavigationStop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NavigationPositionStatus {
    Updated,
    NoActiveRun,
    Paused,
    /// The position was not read; `refusal` says why.
    NotRead,
    /// The position read is further from the route's last than the game
    /// can carry a player between the two readings; nothing was recorded.
    JumpRefused,
    /// The route ended or changed while the position was read; the
    /// reading was dropped.
    Superseded,
    /// The position poll is not reading; `stopped` says why.
    PollStopped,
    Ambiguous,
    OutOfTolerance,
}

/// Why the position poll is not reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PositionPollStop {
    /// The route overlays are not showing.
    OverlaysHidden,
    /// Every read was refused for a while; the player resumes it.
    NotReading,
    /// Screen capture was not allowed; the player tries again.
    CaptureNotAllowed,
}

/// One position verb's answer. `refusal` is set exactly when `status` is
/// `notRead`, `stopped` exactly when it is `pollStopped`; no refusal
/// carries a coordinate.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NavigationPositionResult {
    pub status: NavigationPositionStatus,
    pub run: Nullable<NavigationRun>,
    pub refusal: Nullable<CoordRefusal>,
    pub stopped: Nullable<PositionPollStop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RadarCalibrationStatus {
    Idle,
    AwaitCentre,
    AwaitNorthEdge,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarGeometry {
    pub centre_x: i64,
    pub centre_y: i64,
    pub north_x: i64,
    pub north_y: i64,
    pub radius_px: f64,
    pub display_scale: f64,
}

impl Api {
    pub async fn navigation_snapshot(&self) -> Result<Nullable<NavigationRun>, ApiError> {
        let navigation = self.navigation()?;
        navigation
            .snapshot()
            .await
            .map(|run| run.map(|run| self.run_dto(run)).into())
            .map_err(db_error)
    }

    pub async fn navigation_start(
        &self,
        planet: String,
        map_view_id: Option<i64>,
        start_lon: f64,
        start_lat: f64,
        selected_pin_ids: Option<Vec<i64>>,
        visit_on_key: bool,
    ) -> Result<NavigationRun, ApiError> {
        self.validate_pin_coords(&planet, start_lon, start_lat)?;
        self.validate_map_view(&planet, map_view_id).await?;
        self.navigation()?
            .start(
                planet,
                map_view_id,
                start_lon,
                start_lat,
                selected_pin_ids,
                visit_on_key,
            )
            .await
            .map(|run| self.run_dto(run))
            .map_err(navigation_error)
    }

    pub async fn navigation_update_position(&self) -> Result<NavigationPositionResult, ApiError> {
        self.navigation()?
            .update_position()
            .await
            .map(|update| self.position_dto(update))
            .map_err(navigation_error)
    }

    pub async fn navigation_mark_visited(
        &self,
        force: bool,
    ) -> Result<NavigationPositionResult, ApiError> {
        self.navigation()?
            .mark_visited(force)
            .await
            .map(|update| self.position_dto(update))
            .map_err(navigation_error)
    }

    pub async fn navigation_skip(&self) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .skip()
            .await
            .map(|run| self.run_dto(run))
            .map_err(navigation_error)
    }

    pub async fn navigation_resolve_harvest(
        &self,
        confirm: bool,
    ) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .resolve_harvest(confirm)
            .await
            .map(|run| self.run_dto(run))
            .map_err(navigation_error)
    }

    pub async fn navigation_undo(&self) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .undo()
            .await
            .map(|run| self.run_dto(run))
            .map_err(navigation_error)
    }

    pub async fn navigation_end(&self) -> Result<(), ApiError> {
        self.navigation()?.end().await.map_err(navigation_error)
    }

    /// The player asks the position poll to read again: after it stopped
    /// with every read refused, or with screen capture not allowed (which
    /// may now ask the desktop's consent once more).
    pub fn navigation_resume_position(&self) -> Result<(), ApiError> {
        self.navigation()?.resume_polling();
        Ok(())
    }

    /// The route overlays have been shown or taken away (the shell does,
    /// as it shows and hides them): the poll and guidance serve only
    /// overlays someone can see.
    pub fn navigation_set_overlays_shown(&self, shown: bool) -> Result<(), ApiError> {
        self.navigation()?.set_overlays_shown(shown);
        Ok(())
    }

    /// Whether the route overlays are showing.
    pub fn navigation_overlays_shown(&self) -> bool {
        self.navigation
            .as_ref()
            .is_some_and(|navigation| navigation.overlays_shown())
    }

    fn run_dto(&self, run: ServiceNavigationRun) -> NavigationRun {
        let age = self
            .navigation
            .as_ref()
            .and_then(|navigation| navigation.position_age(run.id));
        navigation_to_dto(run, age)
    }

    fn position_dto(&self, update: PositionUpdate) -> NavigationPositionResult {
        let age = |run: &ServiceNavigationRun| {
            self.navigation
                .as_ref()
                .and_then(|navigation| navigation.position_age(run.id))
        };
        position_to_dto(update, age)
    }

    pub fn radar_calibration_start(&self) -> Result<RadarCalibrationStatus, ApiError> {
        Ok(radar_phase_to_dto(
            self.navigation()?.radar_calibration_start(),
        ))
    }

    /// Take the current radar point at the cursor, exactly as an Enter
    /// press does.
    pub async fn radar_calibration_confirm(&self) -> Result<RadarCalibrationStatus, ApiError> {
        let navigation = self.navigation()?;
        navigation.radar_confirm().await;
        Ok(radar_phase_to_dto(navigation.radar_calibration_phase()))
    }

    pub fn radar_calibration_cancel(&self) -> Result<(), ApiError> {
        self.navigation()?.radar_calibration_cancel();
        Ok(())
    }

    pub fn radar_calibration_status(&self) -> Result<RadarCalibrationStatus, ApiError> {
        Ok(radar_phase_to_dto(
            self.navigation()?.radar_calibration_phase(),
        ))
    }

    pub async fn radar_geometry(&self) -> Result<Nullable<RadarGeometry>, ApiError> {
        self.navigation()?
            .radar_geometry()
            .await
            .map(|geometry| {
                geometry
                    .map(|geometry| RadarGeometry {
                        centre_x: geometry.centre_x,
                        centre_y: geometry.centre_y,
                        north_x: geometry.north_x,
                        north_y: geometry.north_y,
                        radius_px: geometry.radius_px,
                        display_scale: geometry.display_scale,
                    })
                    .into()
            })
            .map_err(db_error)
    }

    fn navigation(&self) -> Result<&Arc<eo_services::navigation::NavigationService>, ApiError> {
        self.navigation
            .as_ref()
            .ok_or_else(|| ApiError::invalid_state("map navigation unavailable"))
    }
}

fn navigation_to_dto(run: ServiceNavigationRun, position_age_s: Option<f64>) -> NavigationRun {
    let (distance_to_active, bearing_degrees) = run.active_stop().map_or((None, None), |active| {
        let dx = active.lon - run.current_lon;
        let dy = active.lat - run.current_lat;
        (
            Some(dx.hypot(dy)),
            Some(dx.atan2(dy).to_degrees().rem_euclid(360.0)),
        )
    });
    NavigationRun {
        id: run.id,
        planet: run.planet,
        map_view_id: run.map_view_id.into(),
        map_view_name: run.map_view_name.into(),
        status: match run.status {
            ServiceRunStatus::Active => NavigationRunStatus::Active,
            ServiceRunStatus::Paused => NavigationRunStatus::Paused,
            ServiceRunStatus::Completed => NavigationRunStatus::Completed,
            ServiceRunStatus::Ended => NavigationRunStatus::Ended,
        },
        start_lon: run.start_lon,
        start_lat: run.start_lat,
        current_lon: run.current_lon,
        current_lat: run.current_lat,
        last_position_at: run.last_position_at.into(),
        position_age_s: position_age_s.into(),
        hop_count: run.hop_count,
        visit_on_key: run.visit_on_key,
        updated_at: run.updated_at,
        distance_to_active: distance_to_active.into(),
        bearing_degrees: bearing_degrees.into(),
        pending_harvest: run
            .pending_harvest
            .map(|pending| PendingHarvest {
                name: pending.name,
                distance: pending.observed.map(|observed| observed.distance).into(),
            })
            .into(),
        stops: run
            .stops
            .into_iter()
            .map(|stop| NavigationStop {
                id: stop.id,
                pin_id: stop.pin_id,
                ordinal: stop.ordinal,
                status: match stop.status {
                    ServiceStopStatus::Pending => NavigationStopStatus::Pending,
                    ServiceStopStatus::Active => NavigationStopStatus::Active,
                    ServiceStopStatus::Visited => NavigationStopStatus::Visited,
                    ServiceStopStatus::Skipped => NavigationStopStatus::Skipped,
                },
                name: stop.name,
                icon: stop.icon,
                lon: stop.lon,
                lat: stop.lat,
                completed_at: stop.completed_at.into(),
                completion_source: stop.completion_source.into(),
            })
            .collect(),
    }
}

fn position_to_dto(
    update: PositionUpdate,
    age: impl Fn(&ServiceNavigationRun) -> Option<f64>,
) -> NavigationPositionResult {
    let mut refusal = None;
    let mut stopped = None;
    let (status, run) = match update {
        PositionUpdate::Updated(run) => (NavigationPositionStatus::Updated, Some(run)),
        PositionUpdate::NoActiveRun => (NavigationPositionStatus::NoActiveRun, None),
        PositionUpdate::Paused(run) => (NavigationPositionStatus::Paused, Some(run)),
        PositionUpdate::NotRead(why) => {
            refusal = Some(CoordRefusal::from(why));
            (NavigationPositionStatus::NotRead, None)
        }
        PositionUpdate::JumpRefused => (NavigationPositionStatus::JumpRefused, None),
        PositionUpdate::Superseded => (NavigationPositionStatus::Superseded, None),
        PositionUpdate::PollStopped(why) => {
            stopped = Some(match why {
                PollStop::OverlaysHidden => PositionPollStop::OverlaysHidden,
                PollStop::NotReading => PositionPollStop::NotReading,
                PollStop::CaptureNotAllowed => PositionPollStop::CaptureNotAllowed,
            });
            (NavigationPositionStatus::PollStopped, None)
        }
        PositionUpdate::Ambiguous(run) => (NavigationPositionStatus::Ambiguous, Some(run)),
        PositionUpdate::OutOfTolerance(run) => {
            (NavigationPositionStatus::OutOfTolerance, Some(run))
        }
    };
    NavigationPositionResult {
        status,
        run: run
            .map(|run| {
                let age = age(&run);
                navigation_to_dto(run, age)
            })
            .into(),
        refusal: refusal.into(),
        stopped: stopped.into(),
    }
}

fn radar_phase_to_dto(phase: RadarCalibrationPhase) -> RadarCalibrationStatus {
    match phase {
        RadarCalibrationPhase::Idle => RadarCalibrationStatus::Idle,
        RadarCalibrationPhase::AwaitCentre => RadarCalibrationStatus::AwaitCentre,
        RadarCalibrationPhase::AwaitNorthEdge { .. } => RadarCalibrationStatus::AwaitNorthEdge,
    }
}

fn navigation_error(error: NavigationError) -> ApiError {
    match error {
        NavigationError::NoActiveRun | NavigationError::NoPins => {
            ApiError::invalid_state(error.to_string())
        }
        NavigationError::EmptyPinSelection | NavigationError::InvalidRadarRadius => {
            ApiError::bad_request(error.to_string())
        }
        NavigationError::Db(error) => db_error(error),
    }
}

// ── Pins ────────────────────────────────────────────────────────────

/// One stored cartography pin. Coordinates are game units; `radius_m`
/// null marks an exact point, a value an area pin of that radius in
/// metres; `session_id` backlinks the tracked session the pin was
/// dropped during, when any.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapPin {
    pub id: i64,
    pub planet: String,
    pub lon: f64,
    pub lat: f64,
    pub altitude: Nullable<f64>,
    pub name: String,
    pub icon: String,
    pub kind: String,
    pub radius_m: Nullable<f64>,
    pub notes: Nullable<String>,
    pub session_id: Nullable<String>,
    pub map_view_id: Nullable<i64>,
    /// Epoch seconds.
    pub created_at: f64,
    /// Epoch seconds of the pin's most recent confirmed visit, if any.
    pub last_visited_at: Nullable<f64>,
    /// Epoch seconds until which the most recent visit keeps the pin on
    /// cooldown, if any.
    pub cooldown_until: Nullable<f64>,
    /// The palette configuration this pin is an instance of, if any.
    pub pin_config_id: Nullable<i64>,
    /// The pin's colour, from its configuration (generic or special-active).
    pub colour: Nullable<String>,
    /// The special-tree on-cooldown colour, from its configuration.
    pub cooldown_colour: Nullable<String>,
    /// The configuration's category (`generic` / `special`), if any.
    pub category: Nullable<String>,
    /// The configuration's special kind (`tree`), if any.
    pub special_kind: Nullable<String>,
}

/// A new pin's fields (id and creation time are assigned server-side).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapPinInput {
    pub planet: String,
    pub lon: f64,
    pub lat: f64,
    #[serde(default)]
    pub altitude: Option<f64>,
    pub name: String,
    pub icon: String,
    pub kind: String,
    #[serde(default)]
    pub radius_m: Option<f64>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub map_view_id: Option<i64>,
    /// The palette configuration this pin instantiates, if any.
    #[serde(default)]
    pub pin_config_id: Option<i64>,
    /// Explicit confirmation that a pin may be created within the
    /// duplicate-advisory radius of an existing pin.
    #[serde(default)]
    pub allow_nearby: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NearbyMapPin {
    pub pin: MapPin,
    pub distance: f64,
}

/// One user-named pin set over a planet map. Default is represented by
/// a null view id and therefore has no row of its own.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapView {
    pub id: i64,
    pub planet: String,
    pub name: String,
    pub created_at: f64,
}

/// A partial pin update: absent fields stay untouched. The nullable
/// fields (altitude, radius, notes) are double options so an explicit
/// `null` (clear it) stays distinct from an absent field.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapPinPatch {
    #[serde(default)]
    pub lon: Option<f64>,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default, deserialize_with = "double_option")]
    pub altitude: Option<Option<f64>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub radius_m: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub notes: Option<Option<String>>,
}

/// One palette entry: a pin *type* scoped to a `(planet, map view)` preset.
/// `category` is `generic` or `special`; the only special `kind` so far is
/// `tree`, which carries a `cooldownColour`. `colour` is the generic colour or
/// the special-active colour. `placedCount` is how many pins reference it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PinConfig {
    pub id: i64,
    pub planet: String,
    pub map_view_id: Nullable<i64>,
    pub label: String,
    pub category: String,
    pub special_kind: Nullable<String>,
    pub icon: String,
    pub radius_m: Nullable<f64>,
    pub colour: String,
    pub cooldown_colour: Nullable<String>,
    pub ordinal: i64,
    /// Epoch seconds.
    pub created_at: f64,
    pub placed_count: i64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PinConfigInput {
    pub planet: String,
    #[serde(default)]
    pub map_view_id: Option<i64>,
    pub label: String,
    pub category: String,
    #[serde(default)]
    pub special_kind: Option<String>,
    pub icon: String,
    #[serde(default)]
    pub radius_m: Option<f64>,
    pub colour: String,
    #[serde(default)]
    pub cooldown_colour: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PinConfigEditInput {
    pub label: String,
    pub category: String,
    #[serde(default)]
    pub special_kind: Option<String>,
    pub icon: String,
    #[serde(default)]
    pub radius_m: Option<f64>,
    pub colour: String,
    #[serde(default)]
    pub cooldown_colour: Option<String>,
}

impl Api {
    /// Every pin on a planet, newest first.
    pub async fn map_pins_list(
        &self,
        planet: String,
        map_view_id: Option<i64>,
    ) -> Result<Vec<MapPin>, ApiError> {
        self.validate_map_view(&planet, map_view_id).await?;
        let pins = self
            .map_pins
            .list(planet, map_view_id)
            .await
            .map_err(db_error)?;
        Ok(pins.into_iter().map(pin_to_dto).collect())
    }

    /// Pins inside a coordinate viewport. This is the scalable map read;
    /// the unbounded list remains for compact lists and compatibility.
    pub async fn map_pins_viewport(
        &self,
        planet: String,
        map_view_id: Option<i64>,
        lon_min: f64,
        lon_max: f64,
        lat_min: f64,
        lat_max: f64,
    ) -> Result<Vec<MapPin>, ApiError> {
        self.validate_map_view(&planet, map_view_id).await?;
        for value in [lon_min, lon_max, lat_min, lat_max] {
            if !value.is_finite() {
                return Err(ApiError::bad_request("viewport bounds must be finite"));
            }
        }
        if lon_min > lon_max || lat_min > lat_max {
            return Err(ApiError::bad_request("viewport bounds are inverted"));
        }
        self.map_pins
            .list_in_bounds(planet, map_view_id, lon_min, lon_max, lat_min, lat_max)
            .await
            .map(|pins| pins.into_iter().map(pin_to_dto).collect())
            .map_err(db_error)
    }

    // ── Pin configurations (the per-preset palette) ─────────────────────

    pub async fn pin_configs_list(
        &self,
        planet: String,
        map_view_id: Option<i64>,
    ) -> Result<Vec<PinConfig>, ApiError> {
        self.validate_map_view(&planet, map_view_id).await?;
        self.pin_configs
            .list(planet, map_view_id)
            .await
            .map(|configs| configs.into_iter().map(config_to_dto).collect())
            .map_err(db_error)
    }

    pub async fn pin_config_create(&self, input: PinConfigInput) -> Result<PinConfig, ApiError> {
        validate_config_fields(
            &input.category,
            &input.special_kind,
            &input.label,
            &input.colour,
            &input.cooldown_colour,
        )?;
        self.validate_map_view(&input.planet, input.map_view_id)
            .await?;
        self.pin_configs
            .create(eo_services::pin_configs::NewPinConfig {
                planet: input.planet,
                map_view_id: input.map_view_id,
                label: input.label,
                category: input.category,
                special_kind: input.special_kind,
                icon: input.icon,
                radius_m: input.radius_m,
                colour: input.colour,
                cooldown_colour: input.cooldown_colour,
            })
            .await
            .map(config_to_dto)
            .map_err(db_error)
    }

    pub async fn pin_config_update(
        &self,
        id: i64,
        input: PinConfigEditInput,
    ) -> Result<PinConfig, ApiError> {
        validate_config_fields(
            &input.category,
            &input.special_kind,
            &input.label,
            &input.colour,
            &input.cooldown_colour,
        )?;
        self.pin_configs
            .update(
                id,
                eo_services::pin_configs::PinConfigEdit {
                    label: input.label,
                    category: input.category,
                    special_kind: input.special_kind,
                    icon: input.icon,
                    radius_m: input.radius_m,
                    colour: input.colour,
                    cooldown_colour: input.cooldown_colour,
                },
            )
            .await
            .map(config_to_dto)
            .map_err(configs_error)
    }

    pub async fn pin_config_delete(&self, id: i64) -> Result<(), ApiError> {
        self.pin_configs.delete(id).await.map_err(configs_error)
    }

    pub async fn pin_config_reorder(&self, ids: Vec<i64>) -> Result<(), ApiError> {
        self.pin_configs.reorder(ids).await.map_err(db_error)
    }

    pub async fn map_pin_nearby(
        &self,
        planet: String,
        map_view_id: Option<i64>,
        lon: f64,
        lat: f64,
    ) -> Result<Nullable<NearbyMapPin>, ApiError> {
        self.validate_pin_coords(&planet, lon, lat)?;
        self.validate_map_view(&planet, map_view_id).await?;
        self.map_pins
            .nearby(
                planet,
                map_view_id,
                lon,
                lat,
                eo_services::navigation::DUPLICATE_TOLERANCE_UNITS,
            )
            .await
            .map(|nearby| {
                nearby
                    .map(|(pin, distance)| NearbyMapPin {
                        pin: pin_to_dto(pin),
                        distance,
                    })
                    .into()
            })
            .map_err(db_error)
    }

    /// Create a pin. When the planet is in the bundled catalogue and
    /// calibrated, the coordinates must lie inside its bounds: an
    /// implausible pin is refused, never silently stored.
    pub async fn map_pin_create(&self, pin: MapPinInput) -> Result<MapPin, ApiError> {
        self.validate_pin_coords(&pin.planet, pin.lon, pin.lat)?;
        self.validate_map_view(&pin.planet, pin.map_view_id).await?;
        if pin.name.trim().is_empty() {
            return Err(ApiError::bad_request("a pin needs a name"));
        }
        if let Some(radius) = pin.radius_m {
            if !radius.is_finite() || radius <= 0.0 {
                return Err(ApiError::bad_request("a pin radius must be positive"));
            }
        }
        if !pin.allow_nearby {
            if let Some((nearby, distance)) = self
                .map_pins
                .nearby(
                    pin.planet.clone(),
                    pin.map_view_id,
                    pin.lon,
                    pin.lat,
                    eo_services::navigation::DUPLICATE_TOLERANCE_UNITS,
                )
                .await
                .map_err(db_error)?
            {
                return Err(ApiError::bad_request(format!(
                    "nearby pin {} already exists {:.1} m away; confirm create anyway",
                    nearby.id, distance
                )));
            }
        }
        let stored = self
            .map_pins
            .create(NewMapPin {
                planet: pin.planet,
                lon: pin.lon,
                lat: pin.lat,
                altitude: pin.altitude,
                name: pin.name,
                icon: pin.icon,
                kind: pin.kind,
                radius_m: pin.radius_m,
                notes: pin.notes,
                session_id: pin.session_id,
                map_view_id: pin.map_view_id,
                pin_config_id: pin.pin_config_id,
            })
            .await
            .map_err(db_error)?;
        Ok(pin_to_dto(stored))
    }

    /// Apply a partial update; a moved pin re-clears the bounds gate.
    pub async fn map_pin_update(&self, id: i64, patch: MapPinPatch) -> Result<MapPin, ApiError> {
        self.ensure_pin_not_in_current_route(id).await?;
        if let Some(name) = patch.name.as_deref() {
            if name.trim().is_empty() {
                return Err(ApiError::bad_request("a pin needs a name"));
            }
        }
        if let Some(Some(radius)) = patch.radius_m {
            if !radius.is_finite() || radius <= 0.0 {
                return Err(ApiError::bad_request("a pin radius must be positive"));
            }
        }
        if patch.lon.is_some() || patch.lat.is_some() {
            // The move gate needs the pin's planet (and its other axis);
            // read it first so a cross-bounds move is refused untouched.
            let current = self.map_pins.get(id).await.map_err(pins_error)?;
            let lon = patch.lon.unwrap_or(current.lon);
            let lat = patch.lat.unwrap_or(current.lat);
            self.validate_pin_coords(&current.planet, lon, lat)?;
        }
        let stored = self
            .map_pins
            .update(
                id,
                eo_services::map_pins::MapPinPatch {
                    lon: patch.lon,
                    lat: patch.lat,
                    altitude: patch.altitude,
                    name: patch.name,
                    icon: patch.icon,
                    kind: patch.kind,
                    radius_m: patch.radius_m,
                    notes: patch.notes,
                },
            )
            .await
            .map_err(pins_error)?;
        Ok(pin_to_dto(stored))
    }

    /// Delete a pin. A pin in use by the live route is allowed to go: the route
    /// drops its stop and recomputes from the current position.
    pub async fn map_pin_delete(&self, id: i64) -> Result<(), ApiError> {
        self.map_pins.delete(id).await.map_err(pins_error)?;
        if let Some(navigation) = self.navigation.as_ref() {
            navigation
                .replan_after_pin_removed(id)
                .await
                .map_err(navigation_error)?;
        }
        Ok(())
    }

    /// Put a tree on cooldown from the map. Records a manual cooldown visit so
    /// the tree is excluded from the next planned route, and if it is a stop in
    /// the live route it is skipped and the route recomputed. Returns the pin
    /// with its refreshed cooldown state.
    pub async fn map_pin_cooldown(&self, id: i64) -> Result<MapPin, ApiError> {
        self.navigation()?
            .cooldown_pin(id)
            .await
            .map_err(navigation_error)?;
        self.map_pins
            .get(id)
            .await
            .map(pin_to_dto)
            .map_err(pins_error)
    }

    /// Named views for a planet. The permanent Default view is virtual
    /// and is added by the frontend.
    pub async fn map_views_list(&self, planet: String) -> Result<Vec<MapView>, ApiError> {
        self.map_pins
            .list_views(planet)
            .await
            .map(|views| views.into_iter().map(view_to_dto).collect())
            .map_err(db_error)
    }

    /// Create a named view and return it as stored.
    pub async fn map_view_create(&self, planet: String, name: String) -> Result<MapView, ApiError> {
        self.validate_planet(&planet)?;
        let name = validate_view_name(name)?;
        self.map_pins
            .create_view(planet, name)
            .await
            .map(view_to_dto)
            .map_err(pins_error)
    }

    /// Rename a named view.
    pub async fn map_view_rename(&self, id: i64, name: String) -> Result<MapView, ApiError> {
        let name = validate_view_name(name)?;
        self.map_pins
            .rename_view(id, name)
            .await
            .map(view_to_dto)
            .map_err(pins_error)
    }

    /// Delete a named view and its pins.
    pub async fn map_view_delete(&self, id: i64) -> Result<(), ApiError> {
        self.ensure_view_not_in_current_route(id).await?;
        self.map_pins.delete_view(id).await.map_err(pins_error)
    }

    async fn ensure_pin_not_in_current_route(&self, pin_id: i64) -> Result<(), ApiError> {
        let in_use = self
            .db
            .with_reader(move |connection| {
                Ok(connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM navigation_stops s JOIN navigation_runs r ON r.id = s.run_id WHERE s.pin_id = ?1 AND r.status IN ('active', 'paused', 'completed'))",
                    [pin_id],
                    |row| row.get::<_, bool>(0),
                )?)
            })
            .await
            .map_err(db_error)?;
        if in_use {
            return Err(ApiError::invalid_state(
                "finish the current route before changing one of its pins",
            ));
        }
        Ok(())
    }

    async fn ensure_view_not_in_current_route(&self, view_id: i64) -> Result<(), ApiError> {
        let in_use = self
            .db
            .with_reader(move |connection| {
                Ok(connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM navigation_runs WHERE map_view_id = ?1 AND status IN ('active', 'paused', 'completed'))",
                    [view_id],
                    |row| row.get::<_, bool>(0),
                )?)
            })
            .await
            .map_err(db_error)?;
        if in_use {
            return Err(ApiError::invalid_state(
                "finish the current route before deleting its map",
            ));
        }
        Ok(())
    }

    async fn validate_map_view(
        &self,
        planet: &str,
        map_view_id: Option<i64>,
    ) -> Result<(), ApiError> {
        let Some(id) = map_view_id else {
            return Ok(());
        };
        let view = self.map_pins.get_view(id).await.map_err(pins_error)?;
        if view.planet != planet {
            return Err(ApiError::bad_request(format!(
                "map view {id} does not belong to {planet}"
            )));
        }
        Ok(())
    }

    fn validate_planet(&self, planet: &str) -> Result<(), ApiError> {
        if let Some(store) = self.planet_maps.as_ref() {
            if store.record(planet).is_none() {
                return Err(ApiError::bad_request(format!(
                    "no bundled map for planet {planet}"
                )));
            }
        }
        Ok(())
    }

    /// The bounds gate: when the catalogue is composed, require a known,
    /// calibrated planet and refuse coordinates outside its window. A
    /// facade composed without the bundle keeps the service-level test and
    /// recovery seam available, but the shipped application never invents
    /// an uncatalogued pin namespace.
    fn validate_pin_coords(&self, planet: &str, lon: f64, lat: f64) -> Result<(), ApiError> {
        if !lon.is_finite() || !lat.is_finite() {
            return Err(ApiError::bad_request("pin coordinates must be finite"));
        }
        let Some(store) = self.planet_maps.as_ref() else {
            return Ok(());
        };
        let record = store
            .record(planet)
            .ok_or_else(|| ApiError::bad_request(format!("no bundled map for planet {planet}")))?;
        let bounds = record
            .calibration
            .as_ref()
            .ok_or_else(|| ApiError::bad_request(format!("{planet}'s map is view-only")))?
            .bounds;
        // Compared in float space: a coordinate a fraction past the edge
        // must not round back inside and persist off the map.
        if lon < bounds.lon_min as f64
            || lon > bounds.lon_max as f64
            || lat < bounds.lat_min as f64
            || lat > bounds.lat_max as f64
        {
            return Err(ApiError::bad_request(format!(
                "coordinates ({lon}, {lat}) lie outside {planet}'s map bounds"
            )));
        }
        Ok(())
    }
}

// ── Coordinate capture ──────────────────────────────────────────────

/// The calibration flow's phase, as the closed wire vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoordCalibrationPhase {
    Idle,
    AwaitTopLeft,
    AwaitBottomRight,
}

impl From<eo_services::coord_capture::CalibrationPhase> for CoordCalibrationPhase {
    fn from(phase: eo_services::coord_capture::CalibrationPhase) -> Self {
        use eo_services::coord_capture::CalibrationPhase as P;
        match phase {
            P::Idle => Self::Idle,
            P::AwaitTopLeft => Self::AwaitTopLeft,
            P::AwaitBottomRight { .. } => Self::AwaitBottomRight,
        }
    }
}

/// The persisted capture rectangle, in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordRegionDto {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// What the coordinates were read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoordReadSource {
    /// Under the radar, found in the game window.
    Radar,
    /// The hand-drawn region, because the radar was not found.
    Region,
}

/// Why no position was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoordScanRefusal {
    /// No recogniser or interface profile is available.
    ReaderUnavailable,
    /// The planet's coordinates are not known, so nothing could be checked
    /// against them.
    PlanetUnknown,
    GameWindowNotFound,
    GameWindowOffScreen,
    CaptureFailed,
    /// Screen capture was not allowed.
    CaptureNotAllowed,
    /// No radar is on the screen, and no region has been drawn by hand.
    RadarNotFound,
    /// The radar names another planet than the one the read is for.
    OtherPlanet,
    /// At least one value was refused; `lon` and `lat` say why.
    NotRead,
}

/// Why one value was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoordValueRefusal {
    /// It is printed off the screen.
    OffScreen,
    /// A window of the game covers it.
    Covered,
    /// It is not shown.
    NotFound,
    /// It did not read cleanly.
    Unclear,
    /// It lies outside the planet's coordinates.
    OutOfBounds,
}

/// Why a position was not read: the reason, and for `notRead`, each
/// refused value's own reason (null for a value that read, whose number
/// is not kept). It carries no coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordRefusal {
    pub reason: CoordScanRefusal,
    pub lon: Nullable<CoordValueRefusal>,
    pub lat: Nullable<CoordValueRefusal>,
}

impl From<ValueRefusal> for CoordValueRefusal {
    fn from(refusal: ValueRefusal) -> Self {
        match refusal {
            ValueRefusal::OffScreen => Self::OffScreen,
            ValueRefusal::Covered => Self::Covered,
            ValueRefusal::NotFound => Self::NotFound,
            ValueRefusal::Unclear => Self::Unclear,
            ValueRefusal::OutOfBounds => Self::OutOfBounds,
        }
    }
}

impl From<ServiceCoordRefusal> for CoordRefusal {
    fn from(refusal: ServiceCoordRefusal) -> Self {
        let only = |reason| CoordRefusal {
            reason,
            lon: None.into(),
            lat: None.into(),
        };
        match refusal {
            ServiceCoordRefusal::ReaderUnavailable => only(CoordScanRefusal::ReaderUnavailable),
            ServiceCoordRefusal::PlanetUnknown => only(CoordScanRefusal::PlanetUnknown),
            ServiceCoordRefusal::GameWindowNotFound => only(CoordScanRefusal::GameWindowNotFound),
            ServiceCoordRefusal::GameWindowOffScreen => only(CoordScanRefusal::GameWindowOffScreen),
            ServiceCoordRefusal::CaptureFailed => only(CoordScanRefusal::CaptureFailed),
            ServiceCoordRefusal::CaptureNotAllowed => only(CoordScanRefusal::CaptureNotAllowed),
            ServiceCoordRefusal::RadarNotFound => only(CoordScanRefusal::RadarNotFound),
            ServiceCoordRefusal::OtherPlanet => only(CoordScanRefusal::OtherPlanet),
            ServiceCoordRefusal::NotRead { lon, lat } => CoordRefusal {
                reason: CoordScanRefusal::NotRead,
                lon: lon.map(CoordValueRefusal::from).into(),
                lat: lat.map(CoordValueRefusal::from).into(),
            },
        }
    }
}

/// One coordinate scan's answer: both values exactly as the screen
/// showed them, or why there are none. A refusal carries no coordinate,
/// so nothing downstream can mistake it for one.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CoordScanResult {
    Read {
        lon: i64,
        lat: i64,
        source: CoordReadSource,
    },
    Refused {
        refusal: CoordRefusal,
    },
}

impl From<Result<CoordReading, ServiceCoordRefusal>> for CoordScanResult {
    fn from(read: Result<CoordReading, ServiceCoordRefusal>) -> Self {
        match read {
            Ok(read) => Self::Read {
                lon: i64::from(read.lon),
                lat: i64::from(read.lat),
                source: match read.source {
                    ReadSource::Radar => CoordReadSource::Radar,
                    ReadSource::Region => CoordReadSource::Region,
                },
            },
            Err(refusal) => Self::Refused {
                refusal: refusal.into(),
            },
        }
    }
}

/// The hand-drawn region's calibration flow: its phase, and the region
/// (null until one has been drawn).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordCalibrationStatus {
    pub phase: CoordCalibrationPhase,
    pub region: Nullable<CoordRegionDto>,
}

impl Api {
    fn coord_capture(
        &self,
    ) -> Result<&std::sync::Arc<eo_services::coord_capture::CoordCaptureService>, ApiError> {
        self.coord_capture
            .as_ref()
            .ok_or_else(|| ApiError::invalid_state("coordinate capture unavailable"))
    }

    fn coord_status(
        &self,
        service: &eo_services::coord_capture::CoordCaptureService,
    ) -> CoordCalibrationStatus {
        CoordCalibrationStatus {
            phase: service.calibration_phase().into(),
            region: service
                .region()
                .map(|region| CoordRegionDto {
                    x: region.x,
                    y: region.y,
                    w: region.w,
                    h: region.h,
                })
                .into(),
        }
    }

    /// Begin the two-point region calibration; Enter arms with it.
    pub fn maps_calibration_start(&self) -> Result<CoordCalibrationStatus, ApiError> {
        let service = self.coord_capture()?;
        service.calibration_start();
        Ok(self.coord_status(service))
    }

    /// Take the current corner at the cursor, exactly as an Enter press
    /// does: the path for a machine that cannot read keys (the overlay
    /// counts down, the player hovers the corner, the capture lands).
    pub fn maps_calibration_confirm(&self) -> Result<CoordCalibrationStatus, ApiError> {
        let service = self.coord_capture()?;
        if service.calibration_active() {
            service.on_confirm();
        }
        Ok(self.coord_status(service))
    }

    /// Abandon an in-flight calibration flow.
    pub fn maps_calibration_cancel(&self) -> Result<CoordCalibrationStatus, ApiError> {
        let service = self.coord_capture()?;
        service.calibration_cancel();
        Ok(self.coord_status(service))
    }

    /// The calibration surface's current state (the flow UI polls it).
    pub fn maps_calibration_status(&self) -> Result<CoordCalibrationStatus, ApiError> {
        let service = self.coord_capture()?;
        Ok(self.coord_status(service))
    }

    /// The player's position on `planet`, each value checked against the
    /// planet's coordinates; a planet without known coordinates is
    /// refused without capturing anything.
    pub fn maps_scan_coordinates(&self, planet: String) -> Result<CoordScanResult, ApiError> {
        let service = self.coord_capture()?;
        Ok(service
            .scan(&planet, self.planet_bounds(&planet), Trigger::Player)
            .into())
    }

    /// A calibrated planet's coordinates, as the readers check them.
    fn planet_bounds(&self, planet: &str) -> Option<CoordinateBounds> {
        let bounds = self
            .planet_maps
            .as_ref()?
            .record(planet)?
            .calibration
            .as_ref()?
            .bounds;
        Some(CoordinateBounds {
            lon: (bounds.lon_min, bounds.lon_max),
            lat: (bounds.lat_min, bounds.lat_max),
        })
    }
}

// ── Radar guidance ──────────────────────────────────────────────────

/// Why radar guidance stopped by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RadarGuidanceStop {
    /// Screen capture was not allowed; guidance does not ask again by
    /// itself.
    CaptureNotAllowed,
    /// The radar was not on the screen for a while.
    RadarNotSeen,
    /// The radar cannot be placed: no interface profile, no hand
    /// calibration.
    ReaderUnavailable,
    /// Looking for the radar failed unexpectedly.
    Failed,
}

/// Why a placement has no scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RadarScaleMissing {
    /// The rings are there but their distances did not read: the game
    /// can hide them.
    LabelsUnread,
    /// No rings were found to measure.
    NoRings,
    /// No text recogniser is available to read the distances.
    NoReader,
}

/// Where the radar's ring distances are printed, in shares of the radius
/// along the line from the centre to the right edge: nothing drawn over
/// the radar may enter it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarLabelStrip {
    pub from: f64,
    pub to: f64,
    pub half_height: f64,
}

/// Why running guidance has no placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RadarGuidanceLoss {
    RadarNotFound,
    GameWindowNotFound,
    GameWindowOffScreen,
    CaptureFailed,
}

/// Where a placement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RadarPlacementSource {
    /// The radar was found on the screen.
    Found,
    /// The player's hand calibration, the radar not being found.
    Calibrated,
}

/// The radar's scale, read from its ring distances.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarScale {
    pub metres_per_pixel: f64,
    /// How far the radar's edge is from the player, in metres.
    pub edge_range_m: f64,
}

/// Radar guidance, as the overlays show it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RadarGuidance {
    /// Not running; when it stopped by itself, why.
    Off {
        stopped: Nullable<RadarGuidanceStop>,
    },
    /// Running, its first look not yet in.
    Searching,
    /// The radar's place on the screen, in physical pixels.
    #[serde(rename_all = "camelCase")]
    Placed {
        centre_x: f64,
        centre_y: f64,
        radius_px: f64,
        /// The screen column right of which a window beside the radar
        /// covers neither the radar nor the coordinates under it.
        clear_right_x: f64,
        source: RadarPlacementSource,
        /// Null when the ring distances are hidden or did not read, or the
        /// placement is the hand calibration's: the overlay then shows
        /// direction only.
        scale: Nullable<RadarScale>,
        /// Why there is no scale, once two looks agree; null otherwise.
        scale_missing: Nullable<RadarScaleMissing>,
        /// Null without an interface profile.
        label_strip: Nullable<RadarLabelStrip>,
    },
    Lost {
        reason: RadarGuidanceLoss,
    },
}

impl From<ServiceGuidance> for RadarGuidance {
    fn from(guidance: ServiceGuidance) -> Self {
        match guidance {
            ServiceGuidance::Off { stopped } => Self::Off {
                stopped: stopped
                    .map(|stop| match stop {
                        GuidanceStop::CaptureNotAllowed => RadarGuidanceStop::CaptureNotAllowed,
                        GuidanceStop::RadarNotSeen => RadarGuidanceStop::RadarNotSeen,
                        GuidanceStop::ReaderUnavailable => RadarGuidanceStop::ReaderUnavailable,
                        GuidanceStop::Failed => RadarGuidanceStop::Failed,
                    })
                    .into(),
            },
            ServiceGuidance::Searching => Self::Searching,
            ServiceGuidance::Placed(placement) => Self::Placed {
                centre_x: placement.centre_x,
                centre_y: placement.centre_y,
                radius_px: placement.radius_px,
                clear_right_x: placement.clear_right_x,
                source: match placement.source {
                    PlacementSource::Found => RadarPlacementSource::Found,
                    PlacementSource::Calibrated => RadarPlacementSource::Calibrated,
                },
                scale: placement
                    .scale
                    .map(|scale| RadarScale {
                        metres_per_pixel: scale.metres_per_pixel,
                        edge_range_m: scale.edge_range_m,
                    })
                    .into(),
                scale_missing: placement
                    .scale_missing
                    .map(|missing| match missing {
                        ScaleMissing::LabelsUnread => RadarScaleMissing::LabelsUnread,
                        ScaleMissing::NoRings => RadarScaleMissing::NoRings,
                        ScaleMissing::NoReader => RadarScaleMissing::NoReader,
                    })
                    .into(),
                label_strip: placement
                    .label_strip
                    .map(|strip| RadarLabelStrip {
                        from: strip.from,
                        to: strip.to,
                        half_height: strip.half_height,
                    })
                    .into(),
            },
            ServiceGuidance::Lost(loss) => Self::Lost {
                reason: match loss {
                    GuidanceLoss::RadarNotFound => RadarGuidanceLoss::RadarNotFound,
                    GuidanceLoss::GameWindowNotFound => RadarGuidanceLoss::GameWindowNotFound,
                    GuidanceLoss::GameWindowOffScreen => RadarGuidanceLoss::GameWindowOffScreen,
                    GuidanceLoss::CaptureFailed => RadarGuidanceLoss::CaptureFailed,
                },
            },
        }
    }
}

impl Api {
    /// Where radar guidance stands.
    pub fn radar_guidance(&self) -> Result<RadarGuidance, ApiError> {
        Ok(self.navigation()?.radar_guidance().into())
    }

    /// Start radar guidance for the live route (the shell does, as it
    /// shows the overlays); answers the status after the first look.
    /// Guidance stops with the overlays (`navigation_set_overlays_shown`).
    pub async fn radar_guidance_start(&self) -> Result<RadarGuidance, ApiError> {
        Ok(self.navigation()?.guidance_start().await.into())
    }

    /// Stop radar guidance and wait briefly for its worker, at app exit.
    pub fn radar_guidance_shutdown(&self) {
        if let Some(navigation) = self.navigation.as_ref() {
            navigation.guidance_shutdown();
        }
    }

    /// The sink told whenever radar guidance's status changes: the shell's
    /// placement of the overlays.
    pub fn set_radar_guidance_changed(&self, sink: Arc<dyn Fn() + Send + Sync>) {
        if let Some(navigation) = self.navigation.as_ref() {
            navigation.set_guidance_changed(sink);
        }
    }
}

fn pin_to_dto(pin: eo_services::map_pins::MapPin) -> MapPin {
    MapPin {
        id: pin.id,
        planet: pin.planet,
        lon: pin.lon,
        lat: pin.lat,
        altitude: pin.altitude.into(),
        name: pin.name,
        icon: pin.icon,
        kind: pin.kind,
        radius_m: pin.radius_m.into(),
        notes: pin.notes.into(),
        session_id: pin.session_id.into(),
        map_view_id: pin.map_view_id.into(),
        created_at: pin.created_at,
        last_visited_at: pin.last_visited_at.into(),
        cooldown_until: pin.cooldown_until.into(),
        pin_config_id: pin.pin_config_id.into(),
        colour: pin.colour.into(),
        cooldown_colour: pin.cooldown_colour.into(),
        category: pin.category.into(),
        special_kind: pin.special_kind.into(),
    }
}

fn view_to_dto(view: eo_services::map_pins::MapView) -> MapView {
    MapView {
        id: view.id,
        planet: view.planet,
        name: view.name,
        created_at: view.created_at,
    }
}

fn validate_view_name(name: String) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("a map view needs a name"));
    }
    if name.chars().count() > 40 {
        return Err(ApiError::bad_request(
            "a map view name cannot exceed 40 characters",
        ));
    }
    if name.eq_ignore_ascii_case("default") {
        return Err(ApiError::bad_request("Default is reserved"));
    }
    Ok(name.to_owned())
}

fn db_error(err: eo_services::db::DbError) -> ApiError {
    ApiError::internal("map pins")(err)
}

fn pins_error(err: MapPinsError) -> ApiError {
    match err {
        MapPinsError::NotFound(id) => ApiError::not_found(format!("map pin {id} not found")),
        MapPinsError::ViewNotFound(id) => ApiError::not_found(format!("map view {id} not found")),
        MapPinsError::ViewNameTaken(name, planet) => ApiError::bad_request(format!(
            "a map view named {name:?} already exists on {planet}"
        )),
        MapPinsError::Db(err) => ApiError::internal("map pins")(err),
    }
}

fn configs_error(err: eo_services::pin_configs::PinConfigsError) -> ApiError {
    match err {
        eo_services::pin_configs::PinConfigsError::NotFound(id) => {
            ApiError::not_found(format!("pin configuration {id} not found"))
        }
        eo_services::pin_configs::PinConfigsError::Db(err) => {
            ApiError::internal("pin configs")(err)
        }
    }
}

/// `#rrggbb`, case-insensitive.
fn is_hex_colour(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

/// The generic/special invariants: a generic config has no special kind and no
/// cooldown colour; a special one requires a known kind and a cooldown colour.
fn validate_config_fields(
    category: &str,
    special_kind: &Option<String>,
    label: &str,
    colour: &str,
    cooldown_colour: &Option<String>,
) -> Result<(), ApiError> {
    if label.trim().is_empty() {
        return Err(ApiError::bad_request("pin label must not be empty"));
    }
    if !is_hex_colour(colour) {
        return Err(ApiError::bad_request(
            "pin colour must be a #rrggbb hex colour",
        ));
    }
    match category {
        "generic" => {
            if special_kind.is_some() {
                return Err(ApiError::bad_request("a generic pin has no special kind"));
            }
            if cooldown_colour.is_some() {
                return Err(ApiError::bad_request(
                    "a generic pin has no cooldown colour",
                ));
            }
        }
        "special" => {
            if special_kind.as_deref() != Some("tree") {
                return Err(ApiError::bad_request(
                    "a special pin needs a known special kind",
                ));
            }
            match cooldown_colour {
                Some(value) if is_hex_colour(value) => {}
                _ => {
                    return Err(ApiError::bad_request(
                        "a special tree pin needs a #rrggbb cooldown colour",
                    ))
                }
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "pin category must be generic or special",
            ))
        }
    }
    Ok(())
}

fn config_to_dto(config: eo_services::pin_configs::PinConfig) -> PinConfig {
    PinConfig {
        id: config.id,
        planet: config.planet,
        map_view_id: config.map_view_id.into(),
        label: config.label,
        category: config.category,
        special_kind: config.special_kind.into(),
        icon: config.icon,
        radius_m: config.radius_m.into(),
        colour: config.colour,
        cooldown_colour: config.cooldown_colour.into(),
        ordinal: config.ordinal,
        created_at: config.created_at,
        placed_count: config.placed_count,
    }
}

fn to_dto(record: &eo_services::planet_maps::PlanetMapRecord) -> PlanetMap {
    PlanetMap {
        name: record.name.clone(),
        technical_name: record.technical_name.clone().into(),
        image_mime: record.image_mime.to_string(),
        image_width_px: record.image_width_px,
        image_height_px: record.image_height_px,
        calibration: record
            .calibration
            .as_ref()
            .map(|cal| PlanetMapCalibration {
                tile_origin_x: cal.tile_origin_x,
                tile_origin_y: cal.tile_origin_y,
                tile_width: cal.tile_width,
                tile_height: cal.tile_height,
                units_per_pixel_x: cal.units_per_pixel_x,
                units_per_pixel_y: cal.units_per_pixel_y,
                bounds: PlanetMapBounds {
                    lon_min: cal.bounds.lon_min,
                    lon_max: cal.bounds.lon_max,
                    lat_min: cal.bounds.lat_min,
                    lat_max: cal.bounds.lat_max,
                },
            })
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scan_refusal_reaches_the_wire_by_name_and_without_a_coordinate() {
        for (refusal, reason) in [
            (ServiceCoordRefusal::ReaderUnavailable, "readerUnavailable"),
            (ServiceCoordRefusal::PlanetUnknown, "planetUnknown"),
            (
                ServiceCoordRefusal::GameWindowNotFound,
                "gameWindowNotFound",
            ),
            (
                ServiceCoordRefusal::GameWindowOffScreen,
                "gameWindowOffScreen",
            ),
            (ServiceCoordRefusal::CaptureFailed, "captureFailed"),
            (ServiceCoordRefusal::CaptureNotAllowed, "captureNotAllowed"),
            (ServiceCoordRefusal::RadarNotFound, "radarNotFound"),
            (ServiceCoordRefusal::OtherPlanet, "otherPlanet"),
        ] {
            assert_eq!(
                serde_json::to_value(CoordScanResult::from(Err(refusal))).unwrap(),
                serde_json::json!({
                    "kind": "refused",
                    "refusal": {"reason": reason, "lon": null, "lat": null},
                }),
                "{refusal:?}"
            );
        }
        for (value, name) in [
            (ValueRefusal::OffScreen, "offScreen"),
            (ValueRefusal::Covered, "covered"),
            (ValueRefusal::NotFound, "notFound"),
            (ValueRefusal::Unclear, "unclear"),
            (ValueRefusal::OutOfBounds, "outOfBounds"),
        ] {
            let refused = ServiceCoordRefusal::NotRead {
                lon: None,
                lat: Some(value),
            };
            assert_eq!(
                serde_json::to_value(CoordScanResult::from(Err(refused))).unwrap(),
                serde_json::json!({
                    "kind": "refused",
                    "refusal": {"reason": "notRead", "lon": null, "lat": name},
                })
            );
        }
    }

    #[test]
    fn a_position_refusal_rides_beside_no_run() {
        let result = position_to_dto(
            PositionUpdate::NotRead(ServiceCoordRefusal::NotRead {
                lon: Some(ValueRefusal::Covered),
                lat: None,
            }),
            |_| None,
        );
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "status": "notRead",
                "run": null,
                "refusal": {"reason": "notRead", "lon": "covered", "lat": null},
                "stopped": null,
            })
        );
        for (update, status) in [
            (PositionUpdate::JumpRefused, "jumpRefused"),
            (PositionUpdate::Superseded, "superseded"),
            (PositionUpdate::NoActiveRun, "noActiveRun"),
        ] {
            assert_eq!(
                serde_json::to_value(position_to_dto(update, |_| None)).unwrap(),
                serde_json::json!({"status": status, "run": null, "refusal": null, "stopped": null})
            );
        }
        for (why, name) in [
            (PollStop::OverlaysHidden, "overlaysHidden"),
            (PollStop::NotReading, "notReading"),
            (PollStop::CaptureNotAllowed, "captureNotAllowed"),
        ] {
            assert_eq!(
                serde_json::to_value(position_to_dto(PositionUpdate::PollStopped(why), |_| None))
                    .unwrap(),
                serde_json::json!({"status": "pollStopped", "run": null, "refusal": null, "stopped": name})
            );
        }
    }

    #[test]
    fn every_guidance_state_reaches_the_wire() {
        use eo_services::navigation::RadarPlacement;
        for (guidance, wire) in [
            (
                ServiceGuidance::Off {
                    stopped: Some(GuidanceStop::CaptureNotAllowed),
                },
                serde_json::json!({"kind": "off", "stopped": "captureNotAllowed"}),
            ),
            (
                ServiceGuidance::Off {
                    stopped: Some(GuidanceStop::RadarNotSeen),
                },
                serde_json::json!({"kind": "off", "stopped": "radarNotSeen"}),
            ),
            (
                ServiceGuidance::Searching,
                serde_json::json!({"kind": "searching"}),
            ),
            (
                ServiceGuidance::Lost(GuidanceLoss::RadarNotFound),
                serde_json::json!({"kind": "lost", "reason": "radarNotFound"}),
            ),
            (
                ServiceGuidance::Placed(RadarPlacement {
                    centre_x: 100.0,
                    centre_y: 200.0,
                    radius_px: 113.0,
                    clear_right_x: 230.0,
                    source: PlacementSource::Found,
                    scale: None,
                    scale_missing: Some(ScaleMissing::LabelsUnread),
                    label_strip: Some(eo_services::coord_capture::LabelStrip {
                        from: 0.1,
                        to: 1.0,
                        half_height: 0.1,
                    }),
                }),
                serde_json::json!({
                    "kind": "placed",
                    "centreX": 100.0,
                    "centreY": 200.0,
                    "radiusPx": 113.0,
                    "clearRightX": 230.0,
                    "source": "found",
                    "scale": null,
                    "scaleMissing": "labelsUnread",
                    "labelStrip": {"from": 0.1, "to": 1.0, "halfHeight": 0.1},
                }),
            ),
            (
                ServiceGuidance::Off {
                    stopped: Some(GuidanceStop::Failed),
                },
                serde_json::json!({"kind": "off", "stopped": "failed"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(RadarGuidance::from(guidance)).unwrap(),
                wire
            );
        }
    }
}
