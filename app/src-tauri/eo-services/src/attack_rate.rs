//! The server's attack-rate limit and what it does to a buffed weapon.
//!
//! The game server accepts at most 100 attacks a minute. A weapon whose
//! reload-speed-buffed rate would exceed that still attacks 100 times a
//! minute, but each attack deals proportionally more damage, and consumes
//! proportionally more ammunition and decay, than it would at its own rate
//! (Entropia Universe release 15.7.1, 2015-12-15). Damage per PEC is
//! therefore unchanged, while the per-attack cost and the damage band a hit
//! is checked against both scale by the same factor: the buffed rate over
//! the limit.
//!
//! The factor is a read-time derivation. [`with_attack_rate`] computes it
//! from the weapon's catalogue base rate and the reload speed in effect, and
//! carries it on the weapon props under [`ATTACK_RATE_FACTOR_KEY`] so every
//! consumer of those props (the cost engine, the attribution band, the
//! Equipment figures) reads one value. It is never stored: only read paths
//! enrich, and every enrichment rewrites the key.

use std::sync::Arc;

use serde_json::Value;

use crate::game_data_store::GameDataStore;

/// Attacks a minute the server processes, whatever the buffs.
pub const SERVER_ATTACKS_PER_MINUTE_LIMIT: f64 = 100.0;

/// Where [`with_attack_rate`] carries the factor on weapon props.
pub const ATTACK_RATE_FACTOR_KEY: &str = "attack_rate_factor";

/// A weapon's attack rate under the reload speed in effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackRate {
    /// The catalogue rate, attacks a minute, before any effect.
    pub base_per_minute: f64,
    /// The reload speed in effect, after the game's stacking limits.
    pub reload_speed_percent: f64,
    /// The rate the reload speed asks for.
    pub buffed_per_minute: f64,
    /// The rate the server actually runs: the buffed rate, held at the
    /// limit.
    pub effective_per_minute: f64,
    /// How much each attack's damage and cost grow to make up for the
    /// attacks the limit withholds; 1 at or below the limit.
    pub factor: f64,
}

impl AttackRate {
    /// Whether the limit is holding the weapon back.
    pub fn is_limited(&self) -> bool {
        self.factor > 1.0
    }
}

/// The attack rate of a weapon with `base_per_minute` under
/// `reload_speed_percent`. None when the base rate is unusable (missing,
/// non-positive, or non-finite) or the reload speed would stop the weapon
/// outright; callers then treat the weapon as unaffected.
pub fn attack_rate(base_per_minute: f64, reload_speed_percent: f64) -> Option<AttackRate> {
    let multiplier = 1.0 + reload_speed_percent / 100.0;
    if !base_per_minute.is_finite()
        || base_per_minute <= 0.0
        || !multiplier.is_finite()
        || multiplier <= 0.0
    {
        return None;
    }
    let buffed_per_minute = base_per_minute * multiplier;
    let effective_per_minute = buffed_per_minute.min(SERVER_ATTACKS_PER_MINUTE_LIMIT);
    let factor = if buffed_per_minute > SERVER_ATTACKS_PER_MINUTE_LIMIT {
        buffed_per_minute / SERVER_ATTACKS_PER_MINUTE_LIMIT
    } else {
        1.0
    };
    Some(AttackRate {
        base_per_minute,
        reload_speed_percent,
        buffed_per_minute,
        effective_per_minute,
        factor,
    })
}

/// The factor carried on enriched weapon props: 1 when the props were never
/// enriched or carry anything but a finite factor of at least 1.
pub fn factor_from_props(props: &Value) -> f64 {
    props
        .get(ATTACK_RATE_FACTOR_KEY)
        .and_then(Value::as_f64)
        .filter(|factor| factor.is_finite() && *factor >= 1.0)
        .unwrap_or(1.0)
}

/// The weapon's catalogue base rate: the current catalogue's figure when the
/// weapon resolves there, else the figure saved with the weapon.
pub fn base_rate_from_props(props: &Value, game_data: Option<&GameDataStore>) -> Option<f64> {
    let weapon = props
        .get("weapon_entity")
        .filter(|entity| !entity.is_null())?;
    let catalogue = game_data.and_then(|game_data| {
        let catalog_id = props
            .get("weapon_catalog_id")
            .filter(|id| !id.is_null())
            .or_else(|| weapon.get("id").filter(|id| !id.is_null()))?;
        game_data
            .find_entity("weapons", catalog_id)?
            .get("uses_per_minute")?
            .as_f64()
    });
    catalogue.or_else(|| weapon.get("uses_per_minute").and_then(Value::as_f64))
}

/// The weapon's attack rate from its props under `reload_speed_percent`.
pub fn attack_rate_from_props(
    props: &Value,
    game_data: Option<&GameDataStore>,
    reload_speed_percent: f64,
) -> Option<AttackRate> {
    attack_rate(
        base_rate_from_props(props, game_data)?,
        reload_speed_percent,
    )
}

/// The weapon props with the catalogue base rate on the weapon entity and
/// the attack-rate factor for `reload_speed_percent` alongside. A weapon
/// with no usable base rate carries a factor of 1. Props without a weapon
/// entity come back unchanged.
pub fn with_attack_rate(
    props: &Value,
    game_data: Option<&GameDataStore>,
    reload_speed_percent: f64,
) -> Value {
    let mut enriched = props.clone();
    let Some(object) = enriched.as_object_mut() else {
        return enriched;
    };
    if !object
        .get("weapon_entity")
        .is_some_and(|entity| entity.is_object())
    {
        return enriched;
    }
    let base = base_rate_from_props(props, game_data);
    if let (Some(base), Some(weapon)) = (
        base,
        object
            .get_mut("weapon_entity")
            .and_then(Value::as_object_mut),
    ) {
        weapon.insert("uses_per_minute".into(), Value::from(base));
    }
    let factor = base
        .and_then(|base| attack_rate(base, reload_speed_percent))
        .map_or(1.0, |rate| rate.factor);
    object.insert(ATTACK_RATE_FACTOR_KEY.into(), Value::from(factor));
    enriched
}

/// The reload speed in effect, read each time a weapon is priced.
pub type ReloadSpeedSource = Arc<dyn Fn() -> f64 + Send + Sync>;

/// How a surface prepares a stored weapon for pricing: the catalogue it
/// resolves base rates in and where the reload speed in effect comes from.
/// The facade shares one instance between Equipment and post-play review;
/// live tracking applies the same [`with_attack_rate`] over the same live
/// config, so all three price a weapon alike.
#[derive(Clone)]
pub struct WeaponPricing {
    game_data: Option<Arc<GameDataStore>>,
    reload_speed: ReloadSpeedSource,
}

impl WeaponPricing {
    pub fn new(game_data: Option<Arc<GameDataStore>>, reload_speed: ReloadSpeedSource) -> Self {
        Self {
            game_data,
            reload_speed,
        }
    }

    /// The props enriched with the attack rate in force now (see
    /// [`with_attack_rate`]).
    pub fn prepare(&self, props: &Value) -> Value {
        with_attack_rate(props, self.game_data.as_deref(), (self.reload_speed)())
    }

    /// The props enriched with the attack rate under a reload speed that was
    /// in force at some earlier moment: how review prices a stored shot at
    /// the rate it landed under.
    pub fn prepare_at(&self, props: &Value, reload_speed_percent: f64) -> Value {
        with_attack_rate(props, self.game_data.as_deref(), reload_speed_percent)
    }

    /// The reload speed in effect now.
    pub fn reload_speed_percent(&self) -> f64 {
        (self.reload_speed)()
    }

    /// The weapon's attack rate in force now, when its base rate is known.
    pub fn attack_rate(&self, props: &Value) -> Option<AttackRate> {
        attack_rate_from_props(props, self.game_data.as_deref(), (self.reload_speed)())
    }
}

impl std::fmt::Debug for WeaponPricing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeaponPricing").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn below_the_limit_a_weapon_runs_at_its_buffed_rate() {
        let rate = attack_rate(60.0, 15.0).unwrap();
        assert!(close(rate.buffed_per_minute, 69.0));
        assert!(close(rate.effective_per_minute, 69.0));
        assert_eq!(rate.factor, 1.0);
        assert!(!rate.is_limited());
    }

    #[test]
    fn exactly_at_the_limit_nothing_compresses() {
        let rate = attack_rate(80.0, 25.0).unwrap();
        assert!(close(rate.buffed_per_minute, 100.0));
        assert_eq!(rate.effective_per_minute, 100.0);
        assert_eq!(rate.factor, 1.0);
    }

    #[test]
    fn over_the_limit_the_excess_becomes_per_attack_magnitude() {
        let rate = attack_rate(90.0, 30.0).unwrap();
        assert!(close(rate.buffed_per_minute, 117.0));
        assert_eq!(rate.effective_per_minute, 100.0);
        assert!(close(rate.factor, 1.17));
        assert!(rate.is_limited());
        // Throughput is conserved: fewer attacks, each one larger.
        assert!(close(
            rate.effective_per_minute * rate.factor,
            rate.buffed_per_minute
        ));
    }

    #[test]
    fn a_catalogue_rate_above_the_limit_compresses_unbuffed() {
        let rate = attack_rate(120.0, 0.0).unwrap();
        assert_eq!(rate.effective_per_minute, 100.0);
        assert!(close(rate.factor, 1.2));
    }

    #[test]
    fn a_slowing_effect_never_compresses() {
        let rate = attack_rate(90.0, -20.0).unwrap();
        assert!(close(rate.effective_per_minute, 72.0));
        assert_eq!(rate.factor, 1.0);
    }

    #[test]
    fn unusable_inputs_leave_the_weapon_unaffected() {
        assert_eq!(attack_rate(0.0, 10.0), None);
        assert_eq!(attack_rate(-5.0, 10.0), None);
        assert_eq!(attack_rate(f64::NAN, 10.0), None);
        assert_eq!(attack_rate(60.0, -100.0), None);
        assert_eq!(attack_rate(60.0, f64::INFINITY), None);
    }

    #[test]
    fn enrichment_carries_the_saved_rate_and_its_factor() {
        let props = json!({"weapon_entity": {"name": "Fast", "uses_per_minute": 90}});
        let enriched = with_attack_rate(&props, None, 30.0);
        assert!(close(factor_from_props(&enriched), 1.17));
        assert_eq!(enriched["weapon_entity"]["uses_per_minute"], json!(90.0));
    }

    #[test]
    fn enrichment_without_a_rate_carries_a_neutral_factor() {
        let props = json!({"weapon_entity": {"name": "Unknown"}});
        let enriched = with_attack_rate(&props, None, 30.0);
        assert_eq!(enriched[ATTACK_RATE_FACTOR_KEY], json!(1.0));
        assert!(enriched["weapon_entity"].get("uses_per_minute").is_none());
    }

    #[test]
    fn enrichment_rewrites_a_stale_factor() {
        let props = json!({
            "weapon_entity": {"uses_per_minute": 50},
            ATTACK_RATE_FACTOR_KEY: 3.0,
        });
        assert_eq!(factor_from_props(&with_attack_rate(&props, None, 0.0)), 1.0);
    }

    #[test]
    fn props_that_are_not_a_weapon_pass_through() {
        let healing = json!({"tool_entity": {"uses_per_minute": 200}});
        assert_eq!(with_attack_rate(&healing, None, 30.0), healing);
        assert_eq!(with_attack_rate(&Value::Null, None, 30.0), Value::Null);
    }

    #[test]
    fn a_malformed_carried_factor_reads_as_neutral() {
        for bad in [json!(0.5), json!(0.0), json!(-2.0), json!("2"), json!(null)] {
            assert_eq!(
                factor_from_props(&json!({ATTACK_RATE_FACTOR_KEY: bad})),
                1.0
            );
        }
        assert_eq!(factor_from_props(&json!({})), 1.0);
    }

    #[test]
    fn pricing_reads_the_reload_speed_at_each_use() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let reload = Arc::new(AtomicU64::new(0f64.to_bits()));
        let source = reload.clone();
        let pricing = WeaponPricing::new(
            None,
            Arc::new(move || f64::from_bits(source.load(Ordering::SeqCst))),
        );
        let props = json!({"weapon_entity": {"uses_per_minute": 90}});
        assert_eq!(factor_from_props(&pricing.prepare(&props)), 1.0);
        reload.store(30f64.to_bits(), Ordering::SeqCst);
        assert!(close(factor_from_props(&pricing.prepare(&props)), 1.17));
        let rate = pricing.attack_rate(&props).unwrap();
        assert_eq!(rate.reload_speed_percent, 30.0);
        assert_eq!(rate.effective_per_minute, 100.0);
    }
}
