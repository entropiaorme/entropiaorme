//! Cost formula engine.
//!
//! Per-use cost (decay + ammo + markups) and reference damage / heal ranges
//! from equipment-catalogue payloads, at maxed skill. Pure arithmetic, no
//! clock, no DB, no events: the canonical leaf and the runner's per-unit
//! `cargo test` proving target. The engine operates on `serde_json::Value`
//! equipment dicts (the stored catalogue payload shape) and rounds every
//! intermediate figure through the shared `round_half_even` banker's
//! rounding, keeping the figures bit-identical to the frozen goldens that
//! pin them. These figures carry no own fingerprint golden of their own; their
//! byte-equality is asserted where they fold into a downstream service's
//! tracker fingerprint, so it is proven there rather than here.

use eo_wire::normalizer::round_half_even;
use serde_json::{json, Value};

use crate::attack_rate;

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

/// The damage profile of a stored weapon setup (`weapon_entity`,
/// `amp_entity`, `damage_enhancers`), per attack at the attack-rate factor
/// the props carry: past the server's limit each attack lands that much
/// harder. None when the weapon publishes no damage.
pub fn weapon_damage_profile_from_props(props: &Value) -> Option<Value> {
    let weapon = props.get("weapon_entity").filter(|v| !v.is_null())?;
    let enhancers = (props
        .get("damage_enhancers")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64)
        .max(0);
    let amp = props.get("amp_entity").filter(|v| !v.is_null());
    let total_damage =
        weapon_total_damage(weapon, amp, enhancers)? * attack_rate::factor_from_props(props);
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

/// A device's decay-absorption fraction from its catalogue economy
/// (`absorption`, 0..=1). Measured in-game behaviour: absorption
/// redistributes the host item's catalogue decay rather than adding to
/// it, and standard (non-absorbing) Mindforce implants publish ~0.02:
/// they still take their small share of every powered use.
fn absorption_share(device: &Value) -> f64 {
    num_or_zero(&economy(device), "absorption").clamp(0.0, 1.0)
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
    cost_per_shot_with_implant(
        weapon,
        amp,
        scope,
        absorber,
        None,
        damage_enhancers,
        weapon_markup,
        amp_markup,
        scope_markup,
        absorber_markup,
        1.0,
    )
}

/// [`cost_per_shot`] with a Mindforce implant powering the weapon: the
/// implant's catalogue absorption share leaves the weapon's decay first
/// (measured order: implant, then absorber/extender on the remainder),
/// priced at the implant's own markup as an "Implant decay" line. With no
/// implant the figures are bit-identical to [`cost_per_shot`].
#[allow(clippy::too_many_arguments)]
pub fn cost_per_shot_with_implant(
    weapon: &Value,
    amp: Option<&Value>,
    scope: Option<&Value>,
    absorber: Option<&Value>,
    implant: Option<&Value>,
    damage_enhancers: i64,
    weapon_markup: f64,
    amp_markup: f64,
    scope_markup: f64,
    absorber_markup: f64,
    implant_markup: f64,
) -> Value {
    scaled_cost_per_shot(
        weapon,
        amp,
        scope,
        absorber,
        implant,
        damage_enhancers,
        [
            weapon_markup,
            amp_markup,
            scope_markup,
            absorber_markup,
            implant_markup,
        ],
        1.0,
    )
}

/// [`cost_per_shot_with_implant`] with every line scaled by
/// `attack_rate_factor`: past the server's attack-rate limit, each attack
/// consumes that much more decay and ammunition (see
/// [`crate::attack_rate`]). A factor of 1 leaves every figure bit-identical.
#[allow(clippy::too_many_arguments)]
fn scaled_cost_per_shot(
    weapon: &Value,
    amp: Option<&Value>,
    scope: Option<&Value>,
    absorber: Option<&Value>,
    implant: Option<&Value>,
    damage_enhancers: i64,
    [weapon_markup, amp_markup, scope_markup, absorber_markup, implant_markup]: [f64; 5],
    attack_rate_factor: f64,
) -> Value {
    let eco = economy(weapon);
    let base_decay = num_or_zero(&eco, "decay");
    let base_ammo_pec = num_or_zero(&eco, "ammo_burn") / 100.0;

    let enhancer_mult = 1.0 + damage_enhancers as f64 * 0.1;
    let mut weapon_decay = base_decay * enhancer_mult;
    let weapon_ammo = base_ammo_pec * enhancer_mult;

    let implant_truthy = implant.map(is_truthy).unwrap_or(false);
    let mut implant_decay = 0.0;
    if implant_truthy {
        implant_decay = weapon_decay * absorption_share(implant.unwrap());
        weapon_decay -= implant_decay;
    }

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
        let cost_pec = cost_pec * attack_rate_factor;
        let effective = round4(cost_pec * markup);
        breakdown.push(json!({
            "component": component,
            "costPec": round4(cost_pec),
            "markupMultiplier": round4(markup),
            "effectiveCostPec": effective,
        }));
        total += effective;
    };

    if implant_truthy && implant_decay > 0.0 {
        add_line("Implant decay", implant_decay, implant_markup);
    }
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

/// Calculate weapon cost from an `equipment_library` `properties_json`
/// payload, per attack at the attack-rate factor the props carry (1 unless
/// [`attack_rate::with_attack_rate`] enriched them).
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

    scaled_cost_per_shot(
        weapon,
        opt("amp_entity"),
        opt("scope_entity"),
        opt("absorber_entity"),
        opt("implant_entity"),
        enhancers,
        [
            markup("weapon_markup"),
            markup("amp_markup"),
            markup("scope_markup"),
            markup("absorber_markup"),
            markup("implant_markup"),
        ],
        attack_rate::factor_from_props(props),
    )
}

/// Cost per use for a medical tool: `(decay + ammo) * markup` in PEC, rounded.
pub fn heal_cost_per_use(tool: &Value, markup: f64) -> f64 {
    heal_cost_per_use_with_implant(tool, markup, None, 1.0)
}

/// [`heal_cost_per_use`] with a Mindforce implant powering the tool: the
/// implant's catalogue absorption share of the tool's decay is priced at
/// the implant's markup; the tool keeps the rest (and its ammo) at the
/// tool's markup. With no implant the figure is bit-identical to
/// [`heal_cost_per_use`].
pub fn heal_cost_per_use_with_implant(
    tool: &Value,
    markup: f64,
    implant: Option<&Value>,
    implant_markup: f64,
) -> f64 {
    let eco = economy(tool);
    let decay = num_or_zero(&eco, "decay");
    let ammo_pec = num_or_zero(&eco, "ammo_burn") / 100.0;
    let implant_share = implant
        .filter(|device| is_truthy(device))
        .map(absorption_share)
        .unwrap_or(0.0);
    let implant_decay = decay * implant_share;
    let tool_decay = decay - implant_decay;
    round4(implant_decay * implant_markup + (tool_decay + ammo_pec) * markup)
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

    #[test]
    fn limited_items_are_detected_by_name_marker() {
        assert!(is_limited(&json!({"name": "Breaker (L)"})));
        assert!(!is_limited(&json!({"name": "Breaker"})));
        assert!(!is_limited(&json!({})));
    }

    #[test]
    fn damage_range_halves_the_total() {
        assert_eq!(
            damage_range_at_max_skill(10.0),
            json!({"min": 5.0, "max": 10.0})
        );
    }

    #[test]
    fn amp_damage_adds_capped_at_half_the_base() {
        let weapon = json!({"damage": {"impact": 10.0}});
        // Below the cap: 10 + 3 = 13.
        assert_eq!(
            weapon_total_damage(&weapon, Some(&json!({"damage": {"burn": 3.0}})), 0),
            Some(13.0)
        );
        // Above the cap: 10 + min(5, 8) = 15.
        assert_eq!(
            weapon_total_damage(&weapon, Some(&json!({"damage": {"burn": 8.0}})), 0),
            Some(15.0)
        );
    }

    #[test]
    fn enhancers_scale_decay_and_ammo_with_hand_computed_totals() {
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 200}});
        let result = cost_per_shot(&weapon, None, None, None, 2, 1.5, 1.0, 1.0, 1.0);
        // mult 1.2: decay 0.06 at markup 1.5 -> 0.09; ammo 2.4 at 1.0.
        assert_eq!(result["totalCostPerUse"], 2.49);
        let breakdown = result["costBreakdown"].as_array().unwrap();
        assert_eq!(breakdown[0]["costPec"], 0.06);
        assert_eq!(breakdown[0]["effectiveCostPec"], 0.09);
        assert_eq!(breakdown[1]["costPec"], 2.4);
    }

    #[test]
    fn falsy_scope_shapes_add_no_breakdown_line() {
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 0}});
        for falsy in [json!(0.0), json!(""), json!([]), json!({}), json!(false)] {
            let result = cost_per_shot(&weapon, None, Some(&falsy), None, 0, 1.0, 1.0, 1.0, 1.0);
            let components: Vec<&str> = result["costBreakdown"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line["component"].as_str().unwrap())
                .collect();
            assert_eq!(components, ["Weapon decay"], "scope {falsy}");
        }
    }

    #[test]
    fn truthy_scope_adds_a_marked_up_scope_decay_line_after_amp_before_ammo() {
        // A real scope with positive decay emits a "Scope decay" line at
        // effectiveCostPec = round4(scope_decay * scope_markup), positioned
        // after "Amp decay" and before the ammo lines. The falsy-scope case
        // (no line) is pinned separately; this freezes the positive branch,
        // its ordering, and the scope-markup application.
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 100}});
        let amp = json!({"economy": {"decay": 0.02, "ammo_burn": 0}});
        let scope = json!({"economy": {"decay": 0.04}});
        let result = cost_per_shot(
            &weapon,
            Some(&amp),
            Some(&scope),
            None,
            0,
            1.0,
            1.0,
            1.5,
            1.0,
        );
        let breakdown = result["costBreakdown"].as_array().unwrap();
        let components: Vec<&str> = breakdown
            .iter()
            .map(|line| line["component"].as_str().unwrap())
            .collect();
        assert_eq!(
            components,
            ["Weapon decay", "Amp decay", "Scope decay", "Ammo (weapon)"]
        );
        // scope decay 0.04 @ 1.5 markup = 0.06.
        assert_eq!(breakdown[2]["costPec"], json!(0.04));
        assert_eq!(breakdown[2]["markupMultiplier"], json!(1.5));
        assert_eq!(breakdown[2]["effectiveCostPec"], json!(0.06));
        // 0.05 + 0.02 + 0.06 + 1.0.
        assert_eq!(result["totalCostPerUse"], json!(1.13));
    }

    #[test]
    fn zero_absorption_absorber_adds_no_line_and_keeps_weapon_decay() {
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 0}});
        let absorber = json!({"economy": {"absorption": 0}});
        let result = cost_per_shot(&weapon, None, None, Some(&absorber), 0, 1.0, 1.0, 1.0, 1.0);
        let breakdown = result["costBreakdown"].as_array().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0]["component"], "Weapon decay");
        assert_eq!(breakdown[0]["costPec"], 0.05);
    }

    #[test]
    fn amp_with_zero_ammo_adds_no_amp_ammo_line() {
        let weapon = json!({"economy": {"decay": 0.05, "ammo_burn": 100}});
        let amp = json!({"economy": {"decay": 0.01, "ammo_burn": 0}});
        let result = cost_per_shot(&weapon, Some(&amp), None, None, 0, 1.0, 1.0, 1.0, 1.0);
        let components: Vec<&str> = result["costBreakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["component"].as_str().unwrap())
            .collect();
        assert_eq!(components, ["Weapon decay", "Amp decay", "Ammo (weapon)"]);
    }

    #[test]
    fn heal_reload_defaults_when_uses_per_minute_is_zero() {
        assert_eq!(heal_reload_seconds(&json!({"uses_per_minute": 0})), 2.5);
    }

    #[test]
    fn heal_cost_multiplies_by_markup() {
        let tool = json!({"economy": {"decay": 0.08, "ammo_burn": 0}});
        assert_eq!(heal_cost_per_use(&tool, 2.0), 0.16);
    }

    #[test]
    #[should_panic(expected = "weapon_entity")]
    fn from_props_rejects_a_null_weapon_entity() {
        let _ = cost_per_shot_from_props(&json!({"weapon_entity": null}), None);
    }

    // The implant expectations below pin the measured in-game decay-split
    // model (catalogue decay conserved and redistributed; the implant's
    // absorption share leaves first, the absorber/extender's share applies
    // to the remainder, so the composition is multiplicative).

    #[test]
    fn implant_and_extender_split_decay_at_their_own_markups() {
        // The measured 20% implant + 20% extender case at three markups:
        // implant 20% @ 1.10, extender 20% of the remainder = 16% @ 1.08,
        // weapon keeps 64% @ 15.00 (the 1500% limited-chip scenario).
        let weapon = json!({"economy": {"decay": 1.0, "ammo_burn": 0}});
        let implant = json!({"name": "NeoPsion 85-B Mindforce Implant (L)",
                             "economy": {"absorption": 0.2}});
        let extender = json!({"name": "ArMatrix Extender P20 (L)",
                              "economy": {"absorption": 0.2}});
        let result = cost_per_shot_with_implant(
            &weapon,
            None,
            None,
            Some(&extender),
            Some(&implant),
            0,
            15.0,
            1.0,
            1.0,
            1.08,
            1.1,
        );
        let breakdown = result["costBreakdown"].as_array().unwrap();
        let components: Vec<&str> = breakdown
            .iter()
            .map(|line| line["component"].as_str().unwrap())
            .collect();
        assert_eq!(
            components,
            ["Implant decay", "Absorber decay", "Weapon decay"]
        );
        assert_eq!(breakdown[0]["costPec"], json!(0.2));
        assert_eq!(breakdown[0]["effectiveCostPec"], json!(0.22));
        assert_eq!(breakdown[1]["costPec"], json!(0.16));
        assert_eq!(breakdown[1]["effectiveCostPec"], json!(0.1728));
        assert_eq!(breakdown[2]["costPec"], json!(0.64));
        assert_eq!(breakdown[2]["effectiveCostPec"], json!(9.6));
        // Effective markup ~999% of base decay, not the additive 943%.
        assert_eq!(result["totalCostPerUse"], json!(9.9928));
    }

    #[test]
    fn standard_implant_takes_its_published_two_percent() {
        // A standard (non-absorbing) implant publishes absorption 0.02 and
        // still takes that slice of every powered use.
        let weapon = json!({"economy": {"decay": 0.572, "ammo_burn": 523}});
        let implant = json!({"economy": {"absorption": 0.02}});
        let result = cost_per_shot_with_implant(
            &weapon,
            None,
            None,
            None,
            Some(&implant),
            0,
            1.0,
            1.0,
            1.0,
            1.0,
            1.0,
        );
        let breakdown = result["costBreakdown"].as_array().unwrap();
        assert_eq!(breakdown[0]["component"], json!("Implant decay"));
        assert_eq!(breakdown[0]["costPec"], json!(0.0114));
        assert_eq!(breakdown[1]["component"], json!("Weapon decay"));
        assert_eq!(breakdown[1]["costPec"], json!(0.5606));
        // Total decay is conserved (0.572) and the ammo line is untouched.
        assert_eq!(result["totalCostPerUse"], json!(5.802));
    }

    #[test]
    fn no_implant_is_bit_identical_to_the_plain_engine() {
        let weapon = json!({"economy": {"decay": 0.123456, "ammo_burn": 200}});
        let plain = cost_per_shot(&weapon, None, None, None, 2, 1.15, 1.0, 1.0, 1.0);
        for falsy in [None, Some(json!({})), Some(json!(null))] {
            let with_implant = cost_per_shot_with_implant(
                &weapon,
                None,
                None,
                None,
                falsy.as_ref(),
                2,
                1.15,
                1.0,
                1.0,
                1.0,
                1.7,
            );
            assert_eq!(plain, with_implant);
        }
    }

    #[test]
    fn from_props_applies_a_stored_implant() {
        let props = json!({
            "weapon_entity": {"economy": {"decay": 1.0, "ammo_burn": 0}},
            "weapon_markup": 1500,
            "implant_entity": {"economy": {"absorption": 0.2}},
            "implant_markup": 110,
            "absorber_entity": {"economy": {"absorption": 0.2}},
            "absorber_markup": 108,
        });
        let result = cost_per_shot_from_props(&props, None);
        assert_eq!(result["totalCostPerUse"], json!(9.9928));
    }

    #[test]
    fn heal_implant_prices_its_share_and_keeps_the_legacy_figure_without_one() {
        let tool = json!({"economy": {"decay": 0.0512, "ammo_burn": 30}});
        // No implant reproduces heal_cost_per_use exactly.
        assert_eq!(
            heal_cost_per_use_with_implant(&tool, 1.1, None, 1.7),
            heal_cost_per_use(&tool, 1.1)
        );
        // 20% implant: 0.01024 @ 1.1; tool keeps 0.04096 + ammo 0.3 @ 1.5.
        let implant = json!({"economy": {"absorption": 0.2}});
        let expected = round4(0.01024 * 1.1 + (0.04096 + 0.3) * 1.5);
        assert_eq!(
            heal_cost_per_use_with_implant(&tool, 1.5, Some(&implant), 1.1),
            expected
        );
    }

    #[test]
    fn from_props_passes_optional_components_through() {
        let props = json!({
            "weapon_entity": {"economy": {"decay": 0.05, "ammo_burn": 0}},
            "amp_entity": {"economy": {"decay": 0.02, "ammo_burn": 0}},
        });
        let result = cost_per_shot_from_props(&props, None);
        // Weapon decay 0.05 + amp decay 0.02, both at default markup.
        assert_eq!(result["totalCostPerUse"], 0.07);
        let components: Vec<&str> = result["costBreakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["component"].as_str().unwrap())
            .collect();
        assert_eq!(components, ["Weapon decay", "Amp decay"]);
    }

    /// A stored weapon setup for the attack-rate cases: 60 damage, 2 PEC
    /// decay and 100 ammo (1 PEC) per attack, amplified, at `uses_per_minute`.
    fn rated_props(uses_per_minute: f64) -> Value {
        json!({
            "weapon_entity": {
                "name": "Rated",
                "uses_per_minute": uses_per_minute,
                "economy": {"decay": 2.0, "ammo_burn": 100},
                "damage": {"impact": 40, "penetration": 20},
            },
            "amp_entity": {
                "name": "Amp",
                "economy": {"decay": 0.5, "ammo_burn": 50},
                "damage": {"burn": 10},
            },
        })
    }

    fn priced_at(uses_per_minute: f64, reload_speed_percent: f64) -> Value {
        crate::attack_rate::with_attack_rate(
            &rated_props(uses_per_minute),
            None,
            reload_speed_percent,
        )
    }

    fn total(props: &Value) -> f64 {
        cost_per_shot_from_props(props, None)["totalCostPerUse"]
            .as_f64()
            .unwrap()
    }

    fn damage_max(props: &Value) -> f64 {
        weapon_damage_profile_from_props(props).unwrap()["damageMax"]
            .as_f64()
            .unwrap()
    }

    #[test]
    fn unenriched_props_price_and_hit_at_the_weapons_own_rate() {
        let props = rated_props(90.0);
        // 2 + 1 + 0.5 + 0.5 PEC; 60 damage plus the amp's 10.
        assert_eq!(total(&props), 4.0);
        assert_eq!(damage_max(&props), 70.0);
    }

    #[test]
    fn cost_and_damage_share_one_factor_across_the_limit() {
        // Below the limit (60 x 1.15 = 69), exactly at it (80 x 1.25), and
        // past it (90 x 1.3 = 117): cost and damage move together, and only
        // past the limit.
        for (base, reload, factor) in [(60.0, 15.0, 1.0), (80.0, 25.0, 1.0), (90.0, 30.0, 1.17)] {
            let props = priced_at(base, reload);
            assert!(
                (total(&props) - 4.0 * factor).abs() < 1e-9,
                "{base} at {reload}%"
            );
            assert!(
                (damage_max(&props) - 70.0 * factor).abs() < 1e-9,
                "{base} at {reload}%"
            );
        }
    }

    #[test]
    fn every_breakdown_line_carries_the_factor_so_the_table_still_sums() {
        let result = cost_per_shot_from_props(&priced_at(90.0, 30.0), None);
        let lines = result["costBreakdown"].as_array().unwrap();
        let sum: f64 = lines
            .iter()
            .map(|line| line["effectiveCostPec"].as_f64().unwrap())
            .sum();
        assert!((sum - result["totalCostPerUse"].as_f64().unwrap()).abs() < 1e-9);
        assert_eq!(lines[0]["component"], "Weapon decay");
        assert_eq!(lines[0]["costPec"], json!(2.34));
    }

    mod attack_rate_properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]

            /// Past the server limit an attack deals and costs more, never
            /// more efficiently: damage per PEC is the same at any rate and
            /// reload speed, and neither figure ever shrinks.
            #[test]
            fn damage_per_pec_is_invariant_under_the_attack_rate(
                base in 1.0f64..160.0,
                reload in -50.0f64..60.0,
                decay in 0.01f64..20.0,
                ammo in 0.0f64..2000.0,
                damage in 1.0f64..400.0,
                enhancers in 0i64..10,
            ) {
                let props = |reload_in_force: Option<f64>| {
                    let raw = json!({
                        "weapon_entity": {
                            "uses_per_minute": base,
                            "economy": {"decay": decay, "ammo_burn": ammo},
                            "damage": {"impact": damage},
                        },
                        "damage_enhancers": enhancers,
                    });
                    match reload_in_force {
                        Some(reload) => crate::attack_rate::with_attack_rate(&raw, None, reload),
                        None => raw,
                    }
                };
                let own = props(None);
                let rated = props(Some(reload));
                let factor = crate::attack_rate::factor_from_props(&rated);
                prop_assert!(factor >= 1.0);
                // Damage scales exactly; cost scales to the engine's display
                // rounding (four decimals on each of at most two lines). So
                // damage per PEC holds, and neither figure ever shrinks.
                prop_assert!((damage_max(&rated) - damage_max(&own) * factor).abs()
                    <= 1e-9 * damage_max(&own));
                prop_assert!((total(&rated) - total(&own) * factor).abs() <= 2e-4 * factor);
                prop_assert!(damage_max(&rated) >= damage_max(&own));
                prop_assert!(total(&rated) + 1e-4 >= total(&own));
            }
        }
    }
}
