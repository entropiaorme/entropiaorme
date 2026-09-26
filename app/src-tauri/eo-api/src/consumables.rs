//! Consumable doses: the running doses and the reload speed they put in
//! effect, the configured consumables a dose can be started from, a dose
//! started by hand, a dose removed (a misclick) or restored, and a session's
//! doses for review.
//!
//! The tracker owns the lifecycle; this facade reads the persisted doses and
//! maps the tracker's outcomes into the generated contract. Every change the
//! tracker commits announces `consumables.updated`, so each open surface
//! re-reads the same persisted expiry instead of keeping a timer of its own.

use eo_services::config_service::load_config_readonly;
use eo_services::consumables::{
    consumable_profile_from_props, read_recent_doses, read_session_doses, DoseEffect,
    DoseEffectKind, DoseRecord, DoseRemoval, DoseSource, RECENTLY_ENDED_SECONDS,
};
use eo_services::passive_effects::{
    equipped_reload_magnitudes, reload_speed_in_effect, RELOAD_SPEED_CONSUMED_LIMIT_PERCENT,
    RELOAD_SPEED_ITEM_LIMIT_PERCENT, RELOAD_SPEED_TOTAL_LIMIT_PERCENT,
};
use eo_services::time::{instant_to_epoch, resolve_local};
use eo_services::tracker::{DoseError, DoseStart};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Api, ApiError, Nullable};

/// How a dose started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsumableDoseSource {
    /// The item's hotbar key, pressed in game.
    Hotbar,
    /// Started by hand, on the overlay or the dashboard.
    Manual,
    /// A healing tool's buff, opened by a paid heal.
    OnUse,
}

impl From<DoseSource> for ConsumableDoseSource {
    fn from(value: DoseSource) -> Self {
        match value {
            DoseSource::Hotbar => Self::Hotbar,
            DoseSource::Manual => Self::Manual,
            DoseSource::OnUse => Self::OnUse,
        }
    }
}

/// Who removed a dose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsumableDoseRemoval {
    /// The player (a misclicked key); restorable.
    Player,
    /// A correction that said the heal granting it was not a paid use;
    /// restored only by undoing that correction.
    HealCorrection,
}

impl From<DoseRemoval> for ConsumableDoseRemoval {
    fn from(value: DoseRemoval) -> Self {
        match value {
            DoseRemoval::Player => Self::Player,
            DoseRemoval::HealCorrection => Self::HealCorrection,
        }
    }
}

/// One effect a dose grants, as the item prints it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumableEffect {
    pub name: String,
    pub strength: Nullable<f64>,
    pub unit: Nullable<String>,
    /// The signed reload speed it adds, percent, when it is a reload-speed
    /// effect; the only kind the app evaluates.
    pub reload_speed_percent: Nullable<f64>,
}

impl From<&DoseEffect> for ConsumableEffect {
    fn from(effect: &DoseEffect) -> Self {
        Self {
            name: effect.name.clone(),
            strength: effect.strength.into(),
            unit: effect.unit.clone().into(),
            reload_speed_percent: match effect.kind {
                DoseEffectKind::Other => None,
                _ => effect.reload_speed_percent(),
            }
            .into(),
        }
    }
}

/// One dose, as the readouts and review show it. Times are epoch seconds.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumableDose {
    pub id: String,
    /// Null once the item was deleted from Equipment.
    pub equipment_id: Nullable<i64>,
    pub item_name: String,
    pub source: ConsumableDoseSource,
    /// The session the dose was taken in; null outside one.
    pub session_id: Nullable<String>,
    pub started_at: f64,
    /// When the effect ends (or ended): the expiry, or earlier when a
    /// re-dose of the item replaced it.
    pub ends_at: f64,
    /// Whether a re-dose of the item replaced it before its expiry.
    pub replaced: bool,
    /// What it booked to its session, PED.
    pub cost_ped: f64,
    /// Whether the item's dose cost was tracked when it was taken.
    pub cost_tracked: bool,
    pub effects: Vec<ConsumableEffect>,
    pub removed_at: Nullable<f64>,
    pub removed_by: Nullable<ConsumableDoseRemoval>,
}

impl From<&DoseRecord> for ConsumableDose {
    fn from(dose: &DoseRecord) -> Self {
        Self {
            id: dose.id.clone(),
            equipment_id: dose.equipment_id.into(),
            item_name: dose.item_name.clone(),
            source: dose.source.into(),
            session_id: dose.session_id.clone().into(),
            started_at: dose.started_at,
            ends_at: dose.ends_at(),
            replaced: dose.superseded_at.is_some_and(|at| at < dose.expires_at),
            cost_ped: dose.cost_ped,
            cost_tracked: dose.cost_tracked,
            effects: dose.effects.iter().map(ConsumableEffect::from).collect(),
            removed_at: dose.removed_at.into(),
            removed_by: dose.removed_by.map(ConsumableDoseRemoval::from).into(),
        }
    }
}

/// A configured consumable a dose can be started from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumableOption {
    pub equipment_id: i64,
    pub name: String,
    /// How long one dose lasts, seconds; 0 for an immediate item.
    pub duration_seconds: f64,
    /// What one dose costs at its recorded markup, PED.
    pub dose_cost_ped: f64,
    /// Whether taking a dose books that cost to the session.
    pub cost_tracked: bool,
    /// The reload speed one dose adds, percent, as printed.
    pub reload_speed_percent: f64,
    /// The hotbar slot the item is bound to, when it is; its key starts a
    /// dose in game.
    pub hotbar_slot: Nullable<String>,
}

/// The reload speed in effect and where it comes from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReloadSpeedNow {
    /// What the enabled equipped sources print, summed.
    pub equipped_percent: f64,
    /// What the running doses print, summed.
    pub consumed_percent: f64,
    /// What reaches weapon attack rates and healing reloads: each group
    /// under its own limit, their sum under the total limit, then slowing.
    pub in_effect_percent: f64,
    pub item_limit_percent: f64,
    pub consumed_limit_percent: f64,
    pub total_limit_percent: f64,
}

/// The dose readout: the running doses and those just ended (so they can be
/// re-dosed), the reload speed in effect, and what a dose can be started
/// from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumableDoses {
    /// Now, epoch seconds, as the doses were read: a countdown measures from
    /// here.
    pub now: f64,
    pub doses: Vec<ConsumableDose>,
    pub reload_speed: ReloadSpeedNow,
    pub options: Vec<ConsumableOption>,
}

fn dose_error(error: DoseError) -> ApiError {
    match error {
        DoseError::NotFound => ApiError::not_found("Dose not found"),
        DoseError::Refused(reason) => ApiError::conflict(reason),
        DoseError::Db(error) => ApiError::internal("consumable dose write")(error),
    }
}

impl Api {
    fn epoch_now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// The dose readout (see [`ConsumableDoses`]).
    pub async fn consumable_doses(&self) -> Result<ConsumableDoses, ApiError> {
        let now = self.epoch_now();
        let config =
            load_config_readonly(&self.data_dir).map_err(ApiError::internal("settings read"))?;
        let game_data = self.game_data.clone();
        let (recent, rows) = self
            .db
            .with_reader(move |conn| {
                let recent = read_recent_doses(conn, now - RECENTLY_ENDED_SECONDS)?;
                let mut stmt = conn.prepare(
                    "SELECT id, name, properties_json FROM equipment_library \
                     WHERE item_type = 'consumable' ORDER BY name COLLATE NOCASE, id",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((recent, rows))
            })
            .await
            .map_err(ApiError::internal("consumable doses read"))?;
        let slot_of = |id: i64| {
            config
                .hotbar
                .iter()
                .find(|(_, bound)| bound.as_i64() == Some(id))
                .map(|(slot, _)| slot.clone())
        };
        let options = rows
            .into_iter()
            .map(|(equipment_id, name, properties)| {
                let props = serde_json::from_str::<Value>(&properties).unwrap_or(Value::Null);
                let profile = consumable_profile_from_props(&props, Some(&game_data)).profile;
                ConsumableOption {
                    equipment_id,
                    name,
                    duration_seconds: profile.duration_seconds,
                    dose_cost_ped: profile.dose_cost_ped(),
                    cost_tracked: profile.track_cost,
                    reload_speed_percent: eo_services::consumables::effects_reload_speed_percent(
                        &profile.effects,
                    ),
                    hotbar_slot: slot_of(equipment_id).into(),
                }
            })
            .collect();
        let board = self.tracker.dose_board();
        let consumed: Vec<f64> = board.consumed_reload_at(now);
        let sources = &config.passive_effect_sources;
        let reload_speed = ReloadSpeedNow {
            equipped_percent: eo_services::passive_effects::declared_reload_speed_percent(sources),
            consumed_percent: consumed.iter().sum(),
            in_effect_percent: reload_speed_in_effect(
                equipped_reload_magnitudes(sources),
                consumed.iter().copied(),
            ),
            item_limit_percent: RELOAD_SPEED_ITEM_LIMIT_PERCENT,
            consumed_limit_percent: RELOAD_SPEED_CONSUMED_LIMIT_PERCENT,
            total_limit_percent: RELOAD_SPEED_TOTAL_LIMIT_PERCENT,
        };
        Ok(ConsumableDoses {
            now,
            doses: recent.iter().map(ConsumableDose::from).collect(),
            reload_speed,
            options,
        })
    }

    /// Start a dose of a configured consumable by hand (one bound to a
    /// hotbar slot starts from its key in game). A running dose of the same
    /// item ends where this one starts.
    pub async fn consumable_dose_start(
        &self,
        equipment_id: i64,
    ) -> Result<ConsumableDoses, ApiError> {
        let row = self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension;
                Ok(conn
                    .query_row(
                        "SELECT name, properties_json FROM equipment_library \
                         WHERE id = ?1 AND item_type = 'consumable'",
                        [equipment_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?)
            })
            .await
            .map_err(ApiError::internal("consumable read"))?;
        let Some((name, properties)) = row else {
            return Err(ApiError::not_found("Consumable not found"));
        };
        let props = serde_json::from_str::<Value>(&properties).unwrap_or(Value::Null);
        let profile = consumable_profile_from_props(&props, Some(&self.game_data)).profile;
        self.tracker
            .start_dose(DoseStart {
                equipment_id,
                item_name: name,
                profile,
            })
            .await
            .map_err(dose_error)?;
        self.consumable_doses().await
    }

    /// Remove a dose (a misclick): its effect and any cost it booked are
    /// taken back, exactly restorable.
    pub async fn consumable_dose_remove(
        &self,
        dose_id: String,
    ) -> Result<ConsumableDoses, ApiError> {
        self.tracker
            .remove_dose(&dose_id)
            .await
            .map_err(dose_error)?;
        self.consumable_doses().await
    }

    /// Give a removed dose back exactly.
    pub async fn consumable_dose_restore(
        &self,
        dose_id: String,
    ) -> Result<ConsumableDoses, ApiError> {
        self.tracker
            .restore_dose(&dose_id)
            .await
            .map_err(dose_error)?;
        self.consumable_doses().await
    }

    /// Every dose taken in a session, removed ones included, oldest first.
    pub async fn consumable_session_doses(
        &self,
        session_id: String,
    ) -> Result<Vec<ConsumableDose>, ApiError> {
        let doses = self
            .db
            .with_reader(move |conn| read_session_doses(conn, &session_id))
            .await
            .map_err(ApiError::internal("session doses read"))?;
        Ok(doses.iter().map(ConsumableDose::from).collect())
    }
}
