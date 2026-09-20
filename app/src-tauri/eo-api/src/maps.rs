//! The planet-maps family: the bundled map catalogue read and the raster
//! fetch behind it.
//!
//! The catalogue is a shipped resource (`resources/maps/`): per-planet
//! rasters plus tile-grid calibration, loaded once at composition into a
//! [`PlanetMapStore`]. The typed read serves the full catalogue (the
//! frontend derives its pixel/game-unit transforms from the calibration
//! fields); the raster itself is raw bytes and rides a bespoke shell
//! command outside the manifest, exactly like the manual-scan capture
//! preview. A facade composed without the bundle serves an empty
//! catalogue: the maps surface stands down, nothing errors at startup.

use std::sync::Arc;

use eo_services::map_pins::{MapPinsError, NewMapPin};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::settings::double_option;
use crate::Nullable;
use crate::{Api, ApiError};

use eo_services::navigation::{
    NavigationError, NavigationRun as ServiceNavigationRun, PositionUpdate, RadarCalibrationPhase,
    RunStatus as ServiceRunStatus, StopStatus as ServiceStopStatus,
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

/// A harvest detected beyond the arrival radius, awaiting the player's confirm
/// or dismiss in the overlay. Names the proposed (active) tree and how far the
/// harvest read was from it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingHarvest {
    pub name: String,
    pub distance: f64,
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
    pub hop_count: i64,
    pub hotkey: String,
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
    NoRegion,
    CaptureFailed,
    EngineUnavailable,
    Unreadable,
    Implausible,
    Ambiguous,
    OutOfTolerance,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NavigationPositionResult {
    pub status: NavigationPositionStatus,
    pub run: Nullable<NavigationRun>,
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
            .map(|run| run.map(navigation_to_dto).into())
            .map_err(db_error)
    }

    pub async fn navigation_start(
        &self,
        planet: String,
        map_view_id: Option<i64>,
        start_lon: f64,
        start_lat: f64,
        selected_pin_ids: Option<Vec<i64>>,
        hotkey: String,
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
                hotkey,
            )
            .await
            .map(navigation_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_update_position(&self) -> Result<NavigationPositionResult, ApiError> {
        self.navigation()?
            .update_position()
            .await
            .map(position_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_mark_visited(
        &self,
        force: bool,
    ) -> Result<NavigationPositionResult, ApiError> {
        self.navigation()?
            .mark_visited(force)
            .await
            .map(position_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_skip(&self) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .skip()
            .await
            .map(navigation_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_resolve_harvest(
        &self,
        confirm: bool,
    ) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .resolve_harvest(confirm)
            .await
            .map(navigation_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_undo(&self) -> Result<NavigationRun, ApiError> {
        self.navigation()?
            .undo()
            .await
            .map(navigation_to_dto)
            .map_err(navigation_error)
    }

    pub async fn navigation_end(&self) -> Result<(), ApiError> {
        self.navigation()?.end().await.map_err(navigation_error)
    }

    pub fn radar_calibration_start(&self) -> Result<RadarCalibrationStatus, ApiError> {
        Ok(radar_phase_to_dto(
            self.navigation()?.radar_calibration_start(),
        ))
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

fn navigation_to_dto(run: ServiceNavigationRun) -> NavigationRun {
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
        hop_count: run.hop_count,
        hotkey: run.hotkey,
        updated_at: run.updated_at,
        distance_to_active: distance_to_active.into(),
        bearing_degrees: bearing_degrees.into(),
        pending_harvest: run
            .pending_harvest
            .map(|pending| PendingHarvest {
                name: pending.name,
                distance: pending.observed_distance,
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

fn position_to_dto(update: PositionUpdate) -> NavigationPositionResult {
    let (status, run) = match update {
        PositionUpdate::Updated(run) => (NavigationPositionStatus::Updated, Some(run)),
        PositionUpdate::NoActiveRun => (NavigationPositionStatus::NoActiveRun, None),
        PositionUpdate::Paused(run) => (NavigationPositionStatus::Paused, Some(run)),
        PositionUpdate::NoRegion => (NavigationPositionStatus::NoRegion, None),
        PositionUpdate::CaptureFailed => (NavigationPositionStatus::CaptureFailed, None),
        PositionUpdate::EngineUnavailable => (NavigationPositionStatus::EngineUnavailable, None),
        PositionUpdate::Unreadable => (NavigationPositionStatus::Unreadable, None),
        PositionUpdate::Implausible => (NavigationPositionStatus::Implausible, None),
        PositionUpdate::Ambiguous(run) => (NavigationPositionStatus::Ambiguous, Some(run)),
        PositionUpdate::OutOfTolerance(run) => {
            (NavigationPositionStatus::OutOfTolerance, Some(run))
        }
    };
    NavigationPositionResult {
        status,
        run: run.map(navigation_to_dto).into(),
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
        NavigationError::EmptyPinSelection
        | NavigationError::InvalidHotkey
        | NavigationError::InvalidRadarRadius => ApiError::bad_request(error.to_string()),
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

/// The closed vocabulary of a coordinate scan's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CoordScanStatus {
    Read,
    NoRegion,
    CaptureFailed,
    EngineUnavailable,
    Unreadable,
    Implausible,
}

/// One coordinate scan's answer: `status` names the outcome precisely
/// (a wrong read never masquerades as a position), and the extras ride
/// where the outcome carries them (the `CaptureResult` convention):
/// coordinates on `read` and `implausible`, the raw capture text only
/// on `unreadable` (where the UI shows it); a successful read's text is
/// its digits, so the parsed coordinates already carry everything and
/// the boundary hands the webview no more screen text than it needs.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordScanResult {
    pub status: CoordScanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lon: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl From<eo_services::coord_capture::CoordScanOutcome> for CoordScanResult {
    fn from(outcome: eo_services::coord_capture::CoordScanOutcome) -> Self {
        use eo_services::coord_capture::CoordScanOutcome as O;
        let empty = |status: CoordScanStatus| CoordScanResult {
            status,
            lon: None,
            lat: None,
            raw_text: None,
            confidence: None,
        };
        match outcome {
            O::Read(read) => CoordScanResult {
                lon: Some(read.lon),
                lat: Some(read.lat),
                confidence: Some(read.confidence),
                ..empty(CoordScanStatus::Read)
            },
            O::NoRegion => empty(CoordScanStatus::NoRegion),
            O::CaptureFailed => empty(CoordScanStatus::CaptureFailed),
            O::EngineUnavailable => empty(CoordScanStatus::EngineUnavailable),
            O::Unreadable {
                raw_text,
                confidence,
            } => CoordScanResult {
                raw_text: Some(raw_text),
                confidence: Some(confidence),
                ..empty(CoordScanStatus::Unreadable)
            },
            O::Implausible { lon, lat, .. } => CoordScanResult {
                lon: Some(lon),
                lat: Some(lat),
                ..empty(CoordScanStatus::Implausible)
            },
        }
    }
}

/// The calibration surface's assembled state: the flow phase, the
/// persisted region (null until a calibration completed), and the
/// validation read echoed after the last completion.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordCalibrationStatus {
    pub phase: CoordCalibrationPhase,
    pub region: Nullable<CoordRegionDto>,
    pub last_validation: Nullable<CoordScanResult>,
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
            last_validation: service.last_validation().map(CoordScanResult::from).into(),
        }
    }

    /// Begin the two-point capture calibration; Enter arms with it.
    pub fn maps_calibration_start(&self) -> Result<CoordCalibrationStatus, ApiError> {
        let service = self.coord_capture()?;
        service.calibration_start();
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

    /// One coordinate scan, gated against the named planet's calibrated
    /// map bounds when it has them.
    pub fn maps_scan_coordinates(
        &self,
        planet: Option<String>,
    ) -> Result<CoordScanResult, ApiError> {
        let service = self.coord_capture()?;
        let bounds = planet
            .as_deref()
            .and_then(|name| {
                self.planet_maps
                    .as_ref()
                    .and_then(|store| store.record(name))
                    .and_then(|record| record.calibration.as_ref())
            })
            .map(|cal| eo_services::coord_capture::CoordBounds {
                lon_min: cal.bounds.lon_min,
                lon_max: cal.bounds.lon_max,
                lat_min: cal.bounds.lat_min,
                lat_max: cal.bounds.lat_max,
            });
        Ok(service.scan(bounds).into())
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
