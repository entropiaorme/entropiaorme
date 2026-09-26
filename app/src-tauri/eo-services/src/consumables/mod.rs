//! Consumable doses: what one dose of a stimulant, pill, or similar item
//! grants and costs, and the persisted lifecycle every surface reads.
//!
//! A dose is a persisted record with an absolute expiry. The tracker owns the
//! lifecycle (start, re-dose, removal and restore, expiry) and publishes the
//! doses still running to the [`DoseBoard`]; each dose's reload-speed effect
//! is the consumed input to
//! [`reload_speed_in_effect`](crate::passive_effects::reload_speed_in_effect),
//! so weapon attack rates and healing reloads read one reload speed in
//! effect. Countdowns anywhere are projections of the stored expiry, never
//! timers of their own.

mod board;
mod profile;
mod store;

pub use board::{DoseBoard, DoseSource, LiveDose};
pub use profile::{
    consumable_profile_from_props, effects_reload_speed_percent, on_use_effect_from_props,
    ConsumableProfile, DoseEffect, DoseEffectKind, OnUseEffect, ResolvedConsumable,
};
pub use store::{
    adjust_session_consumable_cost, detach_session, insert_dose, read_dose,
    read_doses_of_activation, read_recent_doses, read_running_dose_of, read_running_doses,
    read_session_doses, set_activation_doses_removed, set_interval, set_removed, set_superseded,
    DoseRecord, DoseRemoval,
};

/// How long an ended dose stays on the readouts, seconds, so the player can
/// re-dose it from there.
pub const RECENTLY_ENDED_SECONDS: f64 = 60.0;
