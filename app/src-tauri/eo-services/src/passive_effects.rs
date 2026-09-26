//! Persistent equipment effects and their shared evaluators.
//!
//! A source is the durable item or condition the user has declared, while
//! effects are typed capabilities carried by that source. Declared
//! magnitudes are what the items print; the evaluators apply the game's
//! stacking limit, so every consumer reads the reload speed actually in
//! effect. Persistent clothing
//! is the only source lifetime supported today; time-bounded consumables can
//! later feed the same evaluator without changing its callers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassiveEffectKind {
    ReloadSpeed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassiveEffect {
    pub kind: PassiveEffectKind,
    pub magnitude_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassiveEffectSource {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub effects: Vec<PassiveEffect>,
}

/// The most reload speed equipped items can add, whatever their sum. The
/// game limits each effect by where it comes from: for reload speed, 15%
/// from equipped items, 20% from consumed actions, 30% in total. Every
/// source declared here is persistent equipment, so only the item limit can
/// bind; the total limit matters once consumed doses feed this evaluator.
pub const RELOAD_SPEED_ITEM_LIMIT_PERCENT: f64 = 15.0;

/// The reload speed the enabled sources declare, before the game's limit.
pub fn declared_reload_speed_percent(sources: &[PassiveEffectSource]) -> f64 {
    reload_speed_magnitudes(sources).sum()
}

/// The reload speed in effect: the enabled sources' increases, held at the
/// item limit, plus any declared slowing.
pub fn reload_speed_percent(sources: &[PassiveEffectSource]) -> f64 {
    let increase: f64 = reload_speed_magnitudes(sources)
        .filter(|magnitude| *magnitude > 0.0)
        .sum();
    let slowing: f64 = reload_speed_magnitudes(sources)
        .filter(|magnitude| *magnitude < 0.0)
        .sum();
    increase.min(RELOAD_SPEED_ITEM_LIMIT_PERCENT) + slowing
}

fn reload_speed_magnitudes(sources: &[PassiveEffectSource]) -> impl Iterator<Item = f64> + '_ {
    sources
        .iter()
        .filter(|source| source.enabled)
        .flat_map(|source| &source.effects)
        .filter(|effect| effect.kind == PassiveEffectKind::ReloadSpeed)
        .map(|effect| effect.magnitude_percent)
}

/// Convert a base reload duration into the duration under the declared speed
/// multiplier. Invalid legacy or hand-edited totals fail safe to the base
/// duration; the settings boundary prevents new totals at or below -100%.
pub fn effective_reload_seconds(base_seconds: f64, sources: &[PassiveEffectSource]) -> f64 {
    let multiplier = 1.0 + reload_speed_percent(sources) / 100.0;
    if !base_seconds.is_finite()
        || base_seconds < 0.0
        || !multiplier.is_finite()
        || multiplier <= 0.0
    {
        return base_seconds;
    }
    base_seconds / multiplier
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(enabled: bool, magnitude_percent: f64) -> PassiveEffectSource {
        PassiveEffectSource {
            id: "ares-perfect".into(),
            name: "Ares Ring, Perfected".into(),
            enabled,
            effects: vec![PassiveEffect {
                kind: PassiveEffectKind::ReloadSpeed,
                magnitude_percent,
            }],
        }
    }

    #[test]
    fn reload_speed_is_a_throughput_multiplier() {
        let effective = effective_reload_seconds(2.5, &[source(true, 14.0)]);
        assert!((effective - 2.192_982_456).abs() < 0.000_000_001);
    }

    #[test]
    fn an_unrepresentable_declaration_is_held_at_the_item_limit() {
        // Each magnitude is finite, but their sum is not. The game's limit
        // still bounds what reaches the reload.
        let sources = [source(true, f64::MAX), source(true, f64::MAX)];
        assert!(!declared_reload_speed_percent(&sources).is_finite());
        assert_eq!(
            reload_speed_percent(&sources),
            RELOAD_SPEED_ITEM_LIMIT_PERCENT
        );
        assert!((effective_reload_seconds(2.5, &sources) - 2.5 / 1.15).abs() < 1e-12);
    }

    #[test]
    fn a_slowing_total_that_would_stop_reloads_falls_back_to_the_catalogue_duration() {
        let sources = [source(true, -100.0)];
        assert_eq!(effective_reload_seconds(2.5, &sources), 2.5);
    }

    #[test]
    fn disabled_and_negative_sources_compose_without_special_cases() {
        let sources = [source(true, 8.0), source(false, 50.0), source(true, -3.0)];
        assert!((reload_speed_percent(&sources) - 5.0).abs() < f64::EPSILON);
        assert!((effective_reload_seconds(2.5, &sources) - 2.5 / 1.05).abs() < 1e-12);
    }

    #[test]
    fn equipped_increases_stop_at_the_item_limit_before_slowing_applies() {
        let sources = [source(true, 14.0), source(true, 10.0), source(true, -4.0)];
        assert!((declared_reload_speed_percent(&sources) - 20.0).abs() < f64::EPSILON);
        assert!((reload_speed_percent(&sources) - 11.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_declaration_within_the_limit_is_untouched() {
        let sources = [source(true, 9.0), source(true, 6.0)];
        assert_eq!(reload_speed_percent(&sources), 15.0);
        assert_eq!(declared_reload_speed_percent(&sources), 15.0);
    }
}
