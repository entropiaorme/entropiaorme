//! Weapon runtime state: the carried weapons and their attribution
//! state, the per-weapon damage-enhancer stacks, cost resolution through
//! the memoised profile caches, and enhancer-break matching.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::cost_engine::cost_per_shot_from_props;
use crate::expected_hunting::{
    evidence_from_equipment_props, HuntingLooterLevels, OffensiveLoadoutEvidence,
};
use crate::ped::Ped;

use super::actor::TrackerActor;
use super::attribution::AttributionRuntime;
use super::providers::{CarriedWeaponProfile, Providers};

/// The session-scoped weapon runtime: which weapons are carried and which
/// one each shot is attributed to, the enhancer stacks, and the memoised
/// profile/cost lookups. Built fresh at session start; a mid-session
/// config reload replaces the carried set and the caches but keeps the
/// attribution regime.
#[derive(Default)]
pub(super) struct WeaponRuntime {
    /// Hotbar intent validated by the carried weapons' damage bands.
    pub(super) attribution: AttributionRuntime,
    /// Carried weapons' stored props by name (truthy props only).
    pub(super) carried_profiles: BTreeMap<String, Arc<Value>>,
    /// Damage-enhancer stack state per canonical weapon name.
    pub(super) enhancer_states: BTreeMap<String, DamageEnhancerState>,
    /// The canonical name of the weapon whose enhancer state is live.
    pub(super) active_key: Option<String>,
    /// The tool name as the hotbar/attribution observed it (which may
    /// differ in spelling from the canonical name).
    pub(super) observed_name: Option<String>,
    /// Memoised equipment-library profile lookups.
    pub(super) profile_cache: BTreeMap<String, Option<(String, Arc<Value>)>>,
    /// Memoised static per-shot costs for tools without enhancer state.
    pub(super) static_cost_cache: BTreeMap<String, Ped>,
}

impl WeaponRuntime {
    /// Adopt the carried weapons: their bands feed attribution, their props
    /// price them. On a mid-session reload the lookups start over (an edit
    /// may have changed a weapon's cost), while the attribution regime and
    /// the declared weapon carry on.
    pub(super) fn load_carried(&mut self, carried: Vec<CarriedWeaponProfile>) {
        self.carried_profiles.clear();
        self.enhancer_states.clear();
        self.active_key = None;
        self.observed_name = None;
        self.profile_cache.clear();
        self.static_cost_cache.clear();
        let mut weapons = Vec::with_capacity(carried.len());
        for profile in carried {
            if !profile.props.is_empty() {
                self.carried_profiles.insert(
                    profile.weapon.name.clone(),
                    Arc::new(Value::Object(profile.props)),
                );
            }
            weapons.push(profile.weapon);
        }
        self.attribution.set_carried(weapons);
    }
}

/// Per-weapon damage-enhancer state within the current session.
pub(super) struct DamageEnhancerState {
    pub(super) tool_name: String,
    pub(super) props: Arc<Value>,
    pub(super) stacks: Vec<i64>,
    pub(super) cached_cost: Option<Ped>,
}

impl DamageEnhancerState {
    pub(super) fn from_props(tool_name: &str, props: Arc<Value>) -> Self {
        // `max(0, int(props.get("damage_enhancers", 0) or 0))`.
        let configured = props
            .get("damage_enhancers")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;
        let configured = configured.max(0);
        Self {
            tool_name: tool_name.to_string(),
            props,
            stacks: vec![100; configured as usize],
            cached_cost: None,
        }
    }

    pub(super) fn active_slots(&self) -> i64 {
        self.stacks.iter().filter(|stack| **stack > 0).count() as i64
    }

    /// Redistribute a known total across the slots, front-loading the
    /// remainder.
    pub(super) fn set_total(&mut self, total: i64) {
        let total = total.max(0);
        let slot_count = self.stacks.len() as i64;
        if slot_count == 0 {
            return;
        }
        let per_slot = total / slot_count;
        let remainder = total % slot_count;
        self.stacks = (0..slot_count)
            .map(|index| per_slot + i64::from(index < remainder))
            .collect();
        self.cached_cost = None;
    }

    /// Apply one break; true when a slot fully depleted.
    pub(super) fn apply_break(&mut self, remaining: Option<i64>) -> bool {
        let old_active = self.active_slots();
        match remaining {
            Some(total) if !self.stacks.is_empty() => self.set_total(total),
            _ => {
                for index in (0..self.stacks.len()).rev() {
                    if self.stacks[index] > 0 {
                        self.stacks[index] -= 1;
                        self.cached_cost = None;
                        break;
                    }
                }
            }
        }
        old_active != self.active_slots()
    }

    pub(super) fn current_cost(&mut self) -> Ped {
        if self.cached_cost.is_none() {
            let result = cost_per_shot_from_props(&self.props, Some(self.active_slots()));
            let total = result
                .get("totalCostPerUse")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            // The cost engine prices in PEC; the tracker accounts in PED.
            self.cached_cost = Some(Ped(total / 100.0));
        }
        self.cached_cost.expect("just cached")
    }
}

impl TrackerActor {
    /// Resolve a tool name to its canonical profile: the carried weapons
    /// first, then the memoised equipment-library lookup.
    fn match_weapon_profile(
        providers: &Providers,
        weapons: &mut WeaponRuntime,
        tool_name: &str,
    ) -> Option<(String, Arc<Value>)> {
        // The carried table only stores non-empty props, so a hit is a
        // usable profile.
        if let Some(profile) = weapons.carried_profiles.get(tool_name) {
            return Some((tool_name.to_string(), profile.clone()));
        }

        if let Some(cached) = weapons.profile_cache.get(tool_name) {
            return cached.clone();
        }

        let resolved = providers
            .equipment
            .weapon_profile(tool_name)
            .filter(|profile| !profile.is_empty());
        let Some(profile) = resolved else {
            weapons.profile_cache.insert(tool_name.to_string(), None);
            return None;
        };
        // `profile.get("weapon_entity", {}).get("name") or tool_name`.
        let canonical_name = profile
            .get("weapon_entity")
            .and_then(Value::as_object)
            .and_then(|entity| entity.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(tool_name)
            .to_string();
        let matched = Some((canonical_name, Arc::new(Value::Object(profile))));
        weapons
            .profile_cache
            .insert(tool_name.to_string(), matched.clone());
        matched
    }

    /// Resolve (creating if first seen) the enhancer state for a
    /// matched weapon, stamping the active-weapon markers either way.
    pub(super) fn ensure_weapon_state<'a>(
        providers: &Providers,
        weapons: &'a mut WeaponRuntime,
        tool_name: &str,
    ) -> Option<&'a mut DamageEnhancerState> {
        let Some((canonical_name, profile)) =
            Self::match_weapon_profile(providers, weapons, tool_name)
        else {
            weapons.active_key = None;
            weapons.observed_name = Some(tool_name.to_string());
            return None;
        };
        weapons
            .enhancer_states
            .entry(canonical_name.clone())
            .or_insert_with(|| DamageEnhancerState::from_props(&canonical_name, profile));
        weapons.active_key = Some(canonical_name.clone());
        weapons.observed_name = Some(tool_name.to_string());
        weapons.enhancer_states.get_mut(&canonical_name)
    }

    pub(super) fn current_cost_for_tool(
        providers: &Providers,
        weapons: &mut WeaponRuntime,
        tool_name: &str,
        inferred_cost: Ped,
    ) -> Ped {
        if let Some(weapon) = Self::ensure_weapon_state(providers, weapons, tool_name) {
            return weapon.current_cost();
        }
        if inferred_cost.is_positive() {
            return inferred_cost;
        }
        if let Some(cached) = weapons.static_cost_cache.get(tool_name) {
            return *cached;
        }
        let cost = Ped(providers.equipment.cost_per_shot(tool_name));
        weapons
            .static_cost_cache
            .insert(tool_name.to_string(), cost);
        cost
    }

    /// Resolve the immutable supported offensive streams for the current
    /// activation. The stored props, active enhancer count, and session-start
    /// looter snapshot are sufficient to reproduce this evidence later.
    pub(super) fn expected_evidence_for_tool(
        providers: &Providers,
        weapons: &mut WeaponRuntime,
        tool_name: &str,
        looters: HuntingLooterLevels,
    ) -> Option<OffensiveLoadoutEvidence> {
        let state = Self::ensure_weapon_state(providers, weapons, tool_name)?;
        let evidence =
            evidence_from_equipment_props(&state.props, Some(state.active_slots()), looters);
        (!evidence.components.is_empty()).then_some(evidence)
    }
}

/// Whether a break's item name names the active weapon (either the
/// canonical or the observed hotbar spelling), compared on lowercased
/// alphanumerics in either containment direction.
pub(super) fn break_matches_active_weapon(weapons: &WeaponRuntime, item_name: &str) -> bool {
    let Some(weapon) = weapons
        .active_key
        .as_ref()
        .and_then(|key| weapons.enhancer_states.get(key))
    else {
        return false;
    };
    if item_name.is_empty() {
        return false;
    }
    let normalise = |raw: &str| -> String {
        raw.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let item_norm = normalise(item_name);
    let tool_norm = normalise(&weapon.tool_name);
    let observed_norm = normalise(weapons.observed_name.as_deref().unwrap_or(""));
    !item_norm.is_empty()
        && (tool_norm.contains(&item_norm)
            || item_norm.contains(&tool_norm)
            || (!observed_norm.is_empty()
                && (observed_norm.contains(&item_norm) || item_norm.contains(&observed_norm))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loading_the_carried_weapons_restarts_the_lookups_and_keeps_the_regime() {
        use super::super::attribution::{CarriedWeapon, DamageBand};

        let mut runtime = WeaponRuntime {
            active_key: Some("opalo".to_string()),
            observed_name: Some("Opalo (L)".to_string()),
            ..WeaponRuntime::default()
        };
        runtime.attribution.declare("Opalo", 1.0);
        runtime.enhancer_states.insert(
            "opalo".to_string(),
            DamageEnhancerState::from_props("Opalo", Arc::new(json!({"damage_enhancers": 2}))),
        );
        runtime.profile_cache.insert("opalo".to_string(), None);
        runtime
            .static_cost_cache
            .insert("opalo".to_string(), Ped(0.5));
        runtime
            .carried_profiles
            .insert("Stale".to_string(), Arc::new(json!({"x": 1})));

        let weapon = |name: &str| CarriedWeapon {
            equipment_id: 1,
            name: name.to_string(),
            band: Some(DamageBand {
                min: 5.0,
                max: 10.0,
            }),
            effect: None,
        };
        runtime.load_carried(vec![
            CarriedWeaponProfile {
                weapon: weapon("Opalo"),
                props: json!({"weapon_entity": {}}).as_object().unwrap().clone(),
            },
            CarriedWeaponProfile {
                weapon: weapon("Bare"),
                props: serde_json::Map::new(),
            },
        ]);

        // The declared weapon and its regime survive.
        assert_eq!(runtime.attribution.declared(), Some("Opalo"));
        assert_eq!(runtime.attribution.carried().len(), 2);
        // Only non-empty props price a carried weapon.
        assert_eq!(
            runtime.carried_profiles.keys().collect::<Vec<_>>(),
            vec!["Opalo"]
        );
        // Everything else restarts.
        assert!(runtime.active_key.is_none());
        assert!(runtime.observed_name.is_none());
        assert!(runtime.enhancer_states.is_empty());
        assert!(runtime.profile_cache.is_empty());
        assert!(runtime.static_cost_cache.is_empty());
    }
}
