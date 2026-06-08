//! Cost formula engine: Rust port of `backend/services/cost_engine.py`.
//!
//! Per-use cost (decay + ammo + markups) and reference damage / heal ranges
//! from equipment-catalogue payloads, at maxed skill. Pure arithmetic, no
//! clock, no DB, no events: the canonical leaf and the runner's per-unit
//! `cargo test` proving target. The engine operates on `serde_json::Value`
//! equipment dicts so the inputs are the same payloads the Python service
//! consumes, and rounds every intermediate figure through the shared
//! Python-faithful `round_half_even` so the figures stay bit-identical to the
//! oracle. These figures carry no own fingerprint golden of their own; their
//! byte-equality is asserted where they fold into a downstream service's
//! tracker fingerprint, so it is proven there rather than here.

use eo_wire::normalizer::round_half_even;
use serde_json::{json, Value};

const DAMAGE_TYPES: [&str; 9] = [
    "impact",
    "cut",
    "stab",
    "penetration",
    "shrapnel",
    "burn",
    "cold",
    "acid",
    "electric",
];

fn round4(x: f64) -> f64 {
    round_half_even(x, 4)
}

/// Python truthiness for the equipment-dict checks (`if absorber:` / `if
/// scope:`): null/false/0/empty-string/empty-collection are falsy.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `entity.get("economy") or {}`: the economy subdict, defaulting to empty.
fn economy(entity: &Value) -> Value {
    match entity.get("economy") {
        Some(eco) if is_truthy(eco) => eco.clone(),
        _ => json!({}),
    }
}

/// `value.get(key) or 0.0` over a numeric field: the number if present and
/// truthy, else 0.0. (Every default in the engine is 0.0, so a stored 0
/// collapses to the same value either way.)
fn num_or_zero(value: &Value, key: &str) -> f64 {
    match value.get(key).and_then(Value::as_f64) {
        Some(n) if n != 0.0 => n,
        _ => 0.0,
    }
}

/// True if the entity name contains "(L)", indicating a limited item.
pub fn is_limited(entity: &Value) -> bool {
    entity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .contains("(L)")
}

/// Sum the per-type damage fields published on the entity; `None` if the
/// entity is absent or the total is zero (`total or None`).
fn sum_damage(entity: Option<&Value>) -> Option<f64> {
    let entity = entity?;
    let damage = match entity.get("damage") {
        Some(d) if is_truthy(d) => d.clone(),
        _ => json!({}),
    };
    let total: f64 = DAMAGE_TYPES.iter().map(|t| num_or_zero(&damage, t)).sum();
    if total == 0.0 {
        None
    } else {
        Some(total)
    }
}

/// Total weapon damage from base + amp + damage enhancers.
pub fn weapon_total_damage(
    weapon: &Value,
    amp: Option<&Value>,
    damage_enhancers: i64,
) -> Option<f64> {
    let base_damage = sum_damage(Some(weapon))?;
    let mut total_damage = base_damage * (1.0 + damage_enhancers as f64 * 0.1);
    if let Some(amp_damage) = sum_damage(amp) {
        total_damage += (base_damage / 2.0).min(amp_damage);
    }
    Some(total_damage)
}

/// Damage range at maxed skill: `[0.5 * total, total]`.
pub fn damage_range_at_max_skill(total_damage: f64) -> Value {
    json!({"min": total_damage * 0.5, "max": total_damage})
}

/// Derived damage profile suitable for tool inference / display.
pub fn get_weapon_damage_profile(
    weapon: &Value,
    amp: Option<&Value>,
    damage_enhancers: i64,
) -> Option<Value> {
    let total_damage = weapon_total_damage(weapon, amp, damage_enhancers)?;
    Some(json!({
        "totalDamage": total_damage,
        "damageMin": total_damage * 0.5,
        "damageMax": total_damage,
    }))
}

/// Heal range at maxed skill: the tool's published `min_heal` / `max_heal`.
pub fn heal_range_at_max_skill(tool: &Value) -> Option<Value> {
    let max_heal = tool.get("max_heal").filter(|v| !v.is_null())?;
    let min_heal = tool.get("min_heal").filter(|v| !v.is_null())?;
    Some(json!({"min": min_heal, "max": max_heal}))
}

/// Reload at maxed skill: mindforce cooldown if present, else `60 / uses_per_minute`.
pub fn heal_reload_seconds(tool: &Value) -> f64 {
    let cooldown = tool
        .get("mindforce")
        .filter(|v| is_truthy(v))
        .and_then(|m| m.get("cooldown"))
        .and_then(Value::as_f64);
    if let Some(c) = cooldown {
        if c != 0.0 {
            return c;
        }
    }
    let uses_per_minute = tool.get("uses_per_minute").and_then(Value::as_f64);
    match uses_per_minute {
        Some(u) if u != 0.0 => 60.0 / u,
        _ => 60.0 / 24.0,
    }
}

/// Calculate the cost breakdown for a weapon configuration, returning
/// `{"costBreakdown": [...], "totalCostPerUse": float}`.
#[allow(clippy::too_many_arguments)]
pub fn cost_per_shot(
    weapon: &Value,
    amp: Option<&Value>,
    scope: Option<&Value>,
    absorber: Option<&Value>,
    damage_enhancers: i64,
    weapon_markup: f64,
    amp_markup: f64,
    scope_markup: f64,
    absorber_markup: f64,
) -> Value {
    let eco = economy(weapon);
    let base_decay = num_or_zero(&eco, "decay");
    let base_ammo_pec = num_or_zero(&eco, "ammo_burn") / 100.0;

    let enhancer_mult = 1.0 + damage_enhancers as f64 * 0.1;
    let mut weapon_decay = base_decay * enhancer_mult;
    let weapon_ammo = base_ammo_pec * enhancer_mult;

    // `if absorber:` is a truthiness check (empty dict is falsy).
    let absorber_truthy = absorber.map(is_truthy).unwrap_or(false);
    let mut absorber_decay = 0.0;
    if absorber_truthy {
        let absorption = num_or_zero(&economy(absorber.unwrap()), "absorption");
        absorber_decay = weapon_decay * absorption;
        weapon_decay -= absorber_decay;
    }

    // `if amp is not None:` is an explicit None check (empty dict still runs).
    let mut amp_decay = 0.0;
    let mut amp_ammo = 0.0;
    if let Some(amp_value) = amp {
        let amp_eco = economy(amp_value);
        amp_decay = num_or_zero(&amp_eco, "decay");
        amp_ammo = num_or_zero(&amp_eco, "ammo_burn") / 100.0;
    }

    let mut breakdown: Vec<Value> = Vec::new();
    let mut total = 0.0;
    let mut add_line = |component: &str, cost_pec: f64, markup: f64| {
        let effective = round4(cost_pec * markup);
        breakdown.push(json!({
            "component": component,
            "costPec": round4(cost_pec),
            "markupMultiplier": round4(markup),
            "effectiveCostPec": effective,
        }));
        total += effective;
    };

    if absorber_truthy && absorber_decay > 0.0 {
        add_line("Absorber decay", absorber_decay, absorber_markup);
    }
    add_line("Weapon decay", weapon_decay, weapon_markup);
    if amp.is_some() {
        add_line("Amp decay", amp_decay, amp_markup);
    }
    if scope.map(is_truthy).unwrap_or(false) {
        let scope_decay = num_or_zero(&economy(scope.unwrap()), "decay");
        add_line("Scope decay", scope_decay, scope_markup);
    }
    if weapon_ammo > 0.0 {
        let label = if amp.is_some() {
            "Ammo (weapon)"
        } else {
            "Ammo"
        };
        add_line(label, weapon_ammo, 1.0);
    }
    if amp.is_some() && amp_ammo > 0.0 {
        add_line("Ammo (amp)", amp_ammo, 1.0);
    }

    json!({
        "costBreakdown": breakdown,
        "totalCostPerUse": round4(total),
    })
}

/// Calculate weapon cost from an `equipment_library` `properties_json` payload.
pub fn cost_per_shot_from_props(props: &Value, damage_enhancers: Option<i64>) -> Value {
    let configured: f64 = match damage_enhancers {
        Some(de) => de as f64,
        None => props
            .get("damage_enhancers")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    };
    // `max(0, int(configured or 0))`.
    let enhancers = (configured as i64).max(0);

    let opt = |key: &str| props.get(key).filter(|v| !v.is_null());
    let markup = |key: &str| props.get(key).and_then(Value::as_f64).unwrap_or(100.0) / 100.0;

    // `weapon_entity` is mandatory, mirroring the Python `props["weapon_entity"]`
    // (which raises on a missing key, and on a null value when `_economy` calls
    // `.get`). Fail fast rather than defaulting a missing/null weapon to an empty
    // economy, which would silently diverge from the oracle.
    let weapon = props
        .get("weapon_entity")
        .filter(|v| !v.is_null())
        .expect("cost_per_shot_from_props requires a non-null weapon_entity");

    cost_per_shot(
        weapon,
        opt("amp_entity"),
        opt("scope_entity"),
        opt("absorber_entity"),
        enhancers,
        markup("weapon_markup"),
        markup("amp_markup"),
        markup("scope_markup"),
        markup("absorber_markup"),
    )
}

/// Cost per use for a medical tool: `(decay + ammo) * markup` in PEC, rounded.
pub fn heal_cost_per_use(tool: &Value, markup: f64) -> f64 {
    let eco = economy(tool);
    let decay = num_or_zero(&eco, "decay");
    let ammo_pec = num_or_zero(&eco, "ammo_burn") / 100.0;
    round4((decay + ammo_pec) * markup)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values below are the Python cost_engine's output for the same
    // payloads (the ported numeric expectations).

    #[test]
    fn weapon_only_tt_cost() {
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 200}});
        let result = cost_per_shot(&weapon, None, None, None, 0, 1.0, 1.0, 1.0, 1.0);
        // decay 0.05 @ 1.0 + ammo 2.0 @ 1.0 = 2.05
        assert_eq!(result["totalCostPerUse"], json!(2.05));
        assert_eq!(
            result["costBreakdown"][0]["component"],
            json!("Weapon decay")
        );
        assert_eq!(result["costBreakdown"][0]["effectiveCostPec"], json!(0.05));
        assert_eq!(result["costBreakdown"][1]["component"], json!("Ammo"));
        assert_eq!(result["costBreakdown"][1]["effectiveCostPec"], json!(2.0));
    }

    #[test]
    fn weapon_with_markup_rounds_each_line() {
        let weapon = json!({"economy": {"decay": 0.123456, "ammo_burn": 0}});
        let result = cost_per_shot(&weapon, None, None, None, 0, 1.15, 1.0, 1.0, 1.0);
        // costPec round(0.123456,4)=0.1235; effective round(0.123456*1.15,4)=0.142
        assert_eq!(result["costBreakdown"][0]["costPec"], json!(0.1235));
        assert_eq!(result["costBreakdown"][0]["markupMultiplier"], json!(1.15));
        assert_eq!(result["costBreakdown"][0]["effectiveCostPec"], json!(0.142));
        assert_eq!(result["totalCostPerUse"], json!(0.142));
    }

    #[test]
    fn amp_present_relabels_ammo_and_adds_amp_lines() {
        let weapon = json!({"economy": {"decay": 0.1, "ammo_burn": 100}});
        let amp = json!({"economy": {"decay": 0.02, "ammo_burn": 50}});
        let result = cost_per_shot(&weapon, Some(&amp), None, None, 0, 1.0, 1.0, 1.0, 1.0);
        let components: Vec<&str> = result["costBreakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["component"].as_str().unwrap())
            .collect();
        assert_eq!(
            components,
            vec!["Weapon decay", "Amp decay", "Ammo (weapon)", "Ammo (amp)"]
        );
        // 0.1 + 0.02 + 1.0 + 0.5 = 1.62
        assert_eq!(result["totalCostPerUse"], json!(1.62));
    }

    #[test]
    fn absorber_splits_weapon_decay() {
        let weapon = json!({"economy": {"decay": 0.1, "ammo_burn": 0}});
        let absorber = json!({"economy": {"absorption": 0.3}});
        let result = cost_per_shot(&weapon, None, None, Some(&absorber), 0, 1.0, 1.0, 1.0, 1.2);
        let components: Vec<&str> = result["costBreakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["component"].as_str().unwrap())
            .collect();
        assert_eq!(components, vec!["Absorber decay", "Weapon decay"]);
        // absorber_decay 0.03 @ 1.2 = 0.036 ; remaining weapon 0.07 @ 1.0 = 0.07
        assert_eq!(result["costBreakdown"][0]["effectiveCostPec"], json!(0.036));
        assert_eq!(result["costBreakdown"][1]["effectiveCostPec"], json!(0.07));
        assert_eq!(result["totalCostPerUse"], json!(0.106));
    }

    #[test]
    fn empty_absorber_dict_is_falsy_no_split() {
        let weapon = json!({"economy": {"decay": 0.1, "ammo_burn": 0}});
        let absorber = json!({}); // empty dict is falsy in Python
        let result = cost_per_shot(&weapon, None, None, Some(&absorber), 0, 1.0, 1.0, 1.0, 1.0);
        assert_eq!(result["costBreakdown"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["costBreakdown"][0]["component"],
            json!("Weapon decay")
        );
    }

    #[test]
    fn from_props_applies_markups_and_enhancer_clamp() {
        let props = json!({
            "weapon_entity": {"economy": {"decay": 0.1, "ammo_burn": 100}},
            "weapon_markup": 120,
            "damage_enhancers": -3,
        });
        let result = cost_per_shot_from_props(&props, None);
        // enhancers clamp to 0; decay 0.1 @ 1.2 = 0.12 ; ammo 1.0 @ 1.0 = 1.0
        assert_eq!(result["totalCostPerUse"], json!(1.12));
    }

    #[test]
    #[should_panic(expected = "weapon_entity")]
    fn from_props_requires_a_weapon_entity() {
        // The Python oracle raises on a missing weapon_entity; the port mirrors
        // that fail-fast rather than defaulting to a zero-cost empty economy.
        cost_per_shot_from_props(&json!({"weapon_markup": 100}), None);
    }

    #[test]
    fn heal_cost_rounds_to_four_places() {
        let tool = json!({"economy": {"decay": 0.0512, "ammo_burn": 30}});
        // (0.0512 + 0.3) * 1.0 = 0.3512
        assert_eq!(heal_cost_per_use(&tool, 1.0), 0.3512);
    }

    #[test]
    fn damage_enhancers_add_ten_percent_each() {
        let weapon = json!({"damage": {"impact": 50.0}});
        assert_eq!(weapon_total_damage(&weapon, None, 0), Some(50.0));
        assert_eq!(weapon_total_damage(&weapon, None, 2), Some(60.0));
        // No damage -> None.
        assert_eq!(weapon_total_damage(&json!({}), None, 0), None);
    }

    #[test]
    fn heal_reload_prefers_mindforce_cooldown() {
        assert_eq!(
            heal_reload_seconds(&json!({"mindforce": {"cooldown": 2.5}, "uses_per_minute": 30})),
            2.5
        );
        assert_eq!(heal_reload_seconds(&json!({"uses_per_minute": 30})), 2.0);
        assert_eq!(heal_reload_seconds(&json!({})), 60.0 / 24.0);
    }
}
