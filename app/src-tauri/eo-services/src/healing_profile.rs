//! Healing-tool activation and output profile.
//!
//! Catalogue values supply the direct-heal interval. User-owned effect
//! values extend that base for restoration-style tools whose paid use can
//! produce later chat-log outputs. The profile is snapshotted onto every
//! activation so later equipment edits never reinterpret recorded cost.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealingMode {
    #[default]
    Direct,
    OverTime,
    Compound,
}

impl HealingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::OverTime => "over_time",
            Self::Compound => "compound",
        }
    }

    pub fn has_direct(self) -> bool {
        matches!(self, Self::Direct | Self::Compound)
    }

    pub fn has_effect(self) -> bool {
        matches!(self, Self::OverTime | Self::Compound)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct HealingProfile {
    pub mode: HealingMode,
    pub direct_min: Option<f64>,
    pub direct_max: Option<f64>,
    pub effect_duration_seconds: Option<f64>,
    pub tick_min: Option<f64>,
    pub tick_max: Option<f64>,
    pub tick_seconds: Option<f64>,
    /// Reload evidence is populated by the live hotbar resolver. Catalogue
    /// profiles outside an activation leave these fields absent.
    #[serde(default)]
    pub base_reload_seconds: Option<f64>,
    #[serde(default)]
    pub reload_speed_percent: Option<f64>,
    #[serde(default)]
    pub effective_reload_seconds: Option<f64>,
}

impl HealingProfile {
    pub fn direct_matches(&self, amount: f64) -> bool {
        if !self.mode.has_direct() {
            return false;
        }
        range_matches(self.direct_min, self.direct_max, amount)
    }

    pub fn health_capped_direct_matches(&self, amount: f64) -> bool {
        if !self.mode.has_direct() || amount <= 0.0 {
            return false;
        }
        match self.direct_max {
            Some(max) => amount <= max,
            None => true,
        }
    }

    /// Whether `amount` can confirm a paid use: a direct heal inside the
    /// direct interval or, for an item with no direct part, the first tick
    /// of its effect.
    pub fn confirms_activation(&self, amount: f64) -> bool {
        self.direct_matches(amount) || (!self.mode.has_direct() && self.tick_matches(amount))
    }

    pub fn tick_matches(&self, amount: f64) -> bool {
        self.mode.has_effect() && range_matches(self.tick_min, self.tick_max, amount)
    }

    pub fn effect_duration(&self) -> Option<f64> {
        self.mode
            .has_effect()
            .then_some(self.effect_duration_seconds.unwrap_or(0.0))
            .filter(|duration| *duration > 0.0)
    }
}

fn range_matches(min: Option<f64>, max: Option<f64>, amount: f64) -> bool {
    if amount <= 0.0 {
        return false;
    }
    match (min, max) {
        (Some(min), Some(max)) => min <= amount && amount <= max,
        (Some(min), None) => min <= amount,
        (None, Some(max)) => amount <= max,
        (None, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_tick_intervals_are_mode_aware() {
        let profile = HealingProfile {
            mode: HealingMode::Compound,
            direct_min: Some(60.0),
            direct_max: Some(100.0),
            effect_duration_seconds: Some(20.0),
            tick_min: Some(9.0),
            tick_max: Some(11.0),
            tick_seconds: Some(2.0),
            base_reload_seconds: None,
            reload_speed_percent: None,
            effective_reload_seconds: None,
        };
        assert!(profile.direct_matches(80.0));
        assert!(!profile.direct_matches(10.0));
        assert!(profile.health_capped_direct_matches(20.0));
        assert!(profile.tick_matches(10.0));
        assert_eq!(profile.effect_duration(), Some(20.0));
    }
}
