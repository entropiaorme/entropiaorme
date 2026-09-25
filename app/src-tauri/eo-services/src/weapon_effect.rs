//! A weapon's declared damage-over-time effect.
//!
//! Some weapons (Electrocution chips, for one) keep dealing damage after the
//! paid activation that started it: the activation may land an initial hit,
//! and then its effect ticks for a while, printing a damage line per tick.
//! Those ticks are outcomes of the one paid activation, never new shots.
//!
//! The bundled catalogue carries no duration, cadence, or tick figure for
//! such a weapon (its single damage number does not reconcile with what the
//! game prints), so the profile is the player's own declaration, stored with
//! the weapon in Equipment. Every window an activation opens snapshots the
//! profile it was opened under, so a later edit never reinterprets recorded
//! play.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tracker::DamageBand;

/// The props key a weapon's declared effect is stored under.
pub const EFFECT_PROFILE_KEY: &str = "effect_profile";

/// The longest effect a profile may declare. A damage-over-time effect lasts
/// seconds; the bound keeps a mistyped duration from holding a window open
/// long enough to swallow unrelated hits.
pub const MAX_EFFECT_SECONDS: f64 = 600.0;

/// The largest hit or tick a profile may declare. Far above any damage the
/// game prints; it keeps a mistyped figure from turning every later hit
/// into a free tick.
pub const MAX_EFFECT_DAMAGE: f64 = 100_000.0;

/// How a paid activation of the weapon shows up in the chat log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponEffectMode {
    /// Only ticks: the first tick is the activation's outcome.
    OverTime,
    /// An initial hit, then ticks.
    Compound,
}

impl WeaponEffectMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OverTime => "over_time",
            Self::Compound => "compound",
        }
    }
}

/// The declared effect of one paid activation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct WeaponEffectProfile {
    pub mode: WeaponEffectMode,
    /// The initial hit's range; present exactly for a compound effect.
    #[serde(default)]
    pub hit_min: Option<f64>,
    #[serde(default)]
    pub hit_max: Option<f64>,
    /// How long the effect ticks after the activation lands.
    pub duration_seconds: f64,
    /// The range one tick prints.
    pub tick_min: f64,
    pub tick_max: f64,
    /// Seconds between ticks, when known. Informational: attribution never
    /// rejects a tick for arriving early or late, since the chat log can
    /// deliver a burst of lines at once.
    #[serde(default)]
    pub tick_seconds: Option<f64>,
}

/// Why a declared profile cannot be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WeaponEffectError {
    #[error("An effect needs a duration above zero and at most ten minutes")]
    Duration,
    #[error("A tick range needs a minimum no higher than its maximum, and a maximum above zero")]
    TickRange,
    #[error("A declared hit or tick is at most 100,000 damage")]
    TooLarge,
    #[error("A tick cadence must be above zero")]
    Cadence,
    #[error("A hit plus effect needs its initial hit range, minimum no higher than maximum")]
    HitRange,
    #[error("An effect with only ticks has no initial hit range")]
    UnexpectedHit,
}

fn finite_non_negative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

impl WeaponEffectProfile {
    /// The profile as the player declared it, or why it cannot be stored.
    pub fn validated(self) -> Result<Self, WeaponEffectError> {
        if !self.duration_seconds.is_finite()
            || self.duration_seconds <= 0.0
            || self.duration_seconds > MAX_EFFECT_SECONDS
        {
            return Err(WeaponEffectError::Duration);
        }
        if !finite_non_negative(self.tick_min)
            || !finite_non_negative(self.tick_max)
            || self.tick_max <= 0.0
            || self.tick_min > self.tick_max
        {
            return Err(WeaponEffectError::TickRange);
        }
        if self.tick_max > MAX_EFFECT_DAMAGE
            || self.hit_max.is_some_and(|max| max > MAX_EFFECT_DAMAGE)
        {
            return Err(WeaponEffectError::TooLarge);
        }
        if self
            .tick_seconds
            .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
        {
            return Err(WeaponEffectError::Cadence);
        }
        match self.mode {
            WeaponEffectMode::Compound => match (self.hit_min, self.hit_max) {
                (Some(min), Some(max))
                    if finite_non_negative(min) && max.is_finite() && max > 0.0 && min <= max => {}
                _ => return Err(WeaponEffectError::HitRange),
            },
            WeaponEffectMode::OverTime => {
                if self.hit_min.is_some() || self.hit_max.is_some() {
                    return Err(WeaponEffectError::UnexpectedHit);
                }
            }
        }
        Ok(self)
    }

    /// The range one tick prints.
    pub fn tick_band(&self) -> DamageBand {
        DamageBand {
            min: self.tick_min,
            max: self.tick_max,
        }
    }

    /// The range the activation's own outcome prints: the initial hit of a
    /// compound effect, the first tick of an effect with only ticks. This is
    /// the band attribution checks the weapon's shots against, in place of
    /// the catalogue's figure.
    pub fn activation_band(&self) -> DamageBand {
        match (self.mode, self.hit_min, self.hit_max) {
            (WeaponEffectMode::Compound, Some(min), Some(max)) => DamageBand { min, max },
            _ => self.tick_band(),
        }
    }
}

/// The weapon's declared effect, when its stored props carry a readable one.
/// An unreadable or invalid stored profile reads as none: the weapon then
/// prices every hit as a shot, as it did before a profile was declared.
pub fn effect_profile_from_props(props: &Value) -> Option<WeaponEffectProfile> {
    let raw = props
        .get(EFFECT_PROFILE_KEY)
        .filter(|value| !value.is_null())?;
    serde_json::from_value::<WeaponEffectProfile>(raw.clone())
        .ok()?
        .validated()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn compound() -> WeaponEffectProfile {
        WeaponEffectProfile {
            mode: WeaponEffectMode::Compound,
            hit_min: Some(100.0),
            hit_max: Some(160.0),
            duration_seconds: 25.0,
            tick_min: 35.0,
            tick_max: 75.0,
            tick_seconds: Some(1.2),
        }
    }

    #[test]
    fn a_compound_effect_is_confirmed_by_its_initial_hit() {
        let profile = compound().validated().unwrap();
        assert_eq!(
            profile.activation_band(),
            DamageBand {
                min: 100.0,
                max: 160.0
            }
        );
        assert_eq!(
            profile.tick_band(),
            DamageBand {
                min: 35.0,
                max: 75.0
            }
        );
    }

    #[test]
    fn an_effect_with_only_ticks_is_confirmed_by_its_first_tick() {
        let profile = WeaponEffectProfile {
            mode: WeaponEffectMode::OverTime,
            hit_min: None,
            hit_max: None,
            ..compound()
        }
        .validated()
        .unwrap();
        assert_eq!(profile.activation_band(), profile.tick_band());
    }

    #[test]
    fn validation_refuses_what_attribution_could_not_use() {
        let refused = |profile: WeaponEffectProfile| profile.validated().unwrap_err();
        assert_eq!(
            refused(WeaponEffectProfile {
                duration_seconds: 0.0,
                ..compound()
            }),
            WeaponEffectError::Duration
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                duration_seconds: MAX_EFFECT_SECONDS + 1.0,
                ..compound()
            }),
            WeaponEffectError::Duration
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                duration_seconds: f64::NAN,
                ..compound()
            }),
            WeaponEffectError::Duration
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                tick_min: 80.0,
                ..compound()
            }),
            WeaponEffectError::TickRange
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                tick_min: 0.0,
                tick_max: 0.0,
                ..compound()
            }),
            WeaponEffectError::TickRange
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                tick_min: -1.0,
                ..compound()
            }),
            WeaponEffectError::TickRange
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                tick_seconds: Some(0.0),
                ..compound()
            }),
            WeaponEffectError::Cadence
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                hit_max: None,
                ..compound()
            }),
            WeaponEffectError::HitRange
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                hit_min: Some(200.0),
                ..compound()
            }),
            WeaponEffectError::HitRange
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                mode: WeaponEffectMode::OverTime,
                ..compound()
            }),
            WeaponEffectError::UnexpectedHit
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                tick_max: 1e308,
                ..compound()
            }),
            WeaponEffectError::TooLarge
        );
        assert_eq!(
            refused(WeaponEffectProfile {
                hit_max: Some(MAX_EFFECT_DAMAGE + 1.0),
                ..compound()
            }),
            WeaponEffectError::TooLarge
        );
        // A tick floor of zero is a legitimate declaration.
        assert!(WeaponEffectProfile {
            tick_min: 0.0,
            ..compound()
        }
        .validated()
        .is_ok());
    }

    #[test]
    fn the_stored_profile_reads_back_and_a_bad_one_reads_as_none() {
        let props = json!({
            "weapon_entity": {"damage": {"electric": 2000.0}},
            "effect_profile": {
                "mode": "compound",
                "hit_min": 100.0,
                "hit_max": 160.0,
                "duration_seconds": 25.0,
                "tick_min": 35.0,
                "tick_max": 75.0,
            },
        });
        let profile = effect_profile_from_props(&props).unwrap();
        assert_eq!(profile.tick_seconds, None);
        assert_eq!(profile.mode.as_str(), "compound");
        assert_eq!(effect_profile_from_props(&json!({})), None);
        assert_eq!(
            effect_profile_from_props(&json!({"effect_profile": null})),
            None
        );
        assert_eq!(
            effect_profile_from_props(&json!({"effect_profile": {"mode": "compound"}})),
            None
        );
        assert_eq!(
            effect_profile_from_props(&json!({"effect_profile": {
                "mode": "over_time",
                "duration_seconds": -4.0,
                "tick_min": 1.0,
                "tick_max": 2.0,
            }})),
            None,
            "a stored profile that fails validation is not trusted"
        );
    }

    #[test]
    fn the_profile_round_trips_through_its_stored_form() {
        let profile = compound();
        let stored = serde_json::to_value(&profile).unwrap();
        assert_eq!(stored["mode"], "compound");
        let read: WeaponEffectProfile = serde_json::from_value(stored).unwrap();
        assert_eq!(read, profile);
    }
}
