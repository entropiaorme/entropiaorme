//! What is fitted to a tool in the equipment library, read off the entry's
//! stored set-up: its amplifier, scope, absorber and implant, in the order a
//! player names them. The one reading every surface that names a set-up
//! uses (the overlay's tool in hand, the labels that tell look-alike entries
//! apart).

use serde_json::Value;

/// The part an attachment plays in a set-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SetupRole {
    Amplifier,
    Scope,
    Absorber,
    Implant,
}

impl SetupRole {
    /// Every role, in the order a player names a set-up's parts.
    pub const ALL: [SetupRole; 4] = [
        SetupRole::Amplifier,
        SetupRole::Scope,
        SetupRole::Absorber,
        SetupRole::Implant,
    ];

    /// The key the entry's stored set-up keeps this part under.
    pub fn key(self) -> &'static str {
        match self {
            SetupRole::Amplifier => "amp_entity",
            SetupRole::Scope => "scope_entity",
            SetupRole::Absorber => "absorber_entity",
            SetupRole::Implant => "implant_entity",
        }
    }
}

/// One attachment fitted to a tool, by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPart {
    pub role: SetupRole,
    pub name: String,
}

/// The name of the part in `role` of a stored set-up, if one is fitted.
fn part_name(props: &Value, role: SetupRole) -> Option<String> {
    props
        .get(role.key())
        .and_then(|entity| entity.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The name fitted in each role of a stored set-up, in [`SetupRole::ALL`]
/// order (None where the role is empty).
pub fn part_names(props: &Value) -> [Option<String>; 4] {
    SetupRole::ALL.map(|role| part_name(props, role))
}

/// The parts fitted to a stored set-up, in the order a player names them.
pub fn setup_parts(props: &Value) -> Vec<SetupPart> {
    SetupRole::ALL
        .into_iter()
        .filter_map(|role| part_name(props, role).map(|name| SetupPart { role, name }))
        .collect()
}

/// What an entry was resolved with when it was taken up: its parts, and the
/// TT one use consumes (before markup) where the resolution priced it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolSetup {
    pub parts: Vec<SetupPart>,
    pub tt_per_use_ped: Option<f64>,
}

impl ToolSetup {
    /// The set-up of an entry's stored properties, with the TT of one use.
    pub fn from_props_json(properties_json: &str, tt_per_use_ped: Option<f64>) -> Self {
        let props: Value = serde_json::from_str(properties_json).unwrap_or(Value::Null);
        Self {
            parts: setup_parts(&props),
            tt_per_use_ped,
        }
    }
}

/// The TT of one attack in a cost engine's per-shot result (PEC): the sum of
/// its breakdown lines before markup.
pub fn tt_per_shot_pec(cost: &Value) -> f64 {
    cost.get("costBreakdown")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.get("costPec").and_then(Value::as_f64))
                .sum()
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parts_are_read_in_the_order_a_player_names_them() {
        let props = json!({
            "weapon_entity": {"name": "Chip"},
            "implant_entity": {"name": "NeoPsion 85-B"},
            "absorber_entity": {"name": "ArMatrix Extender P20 (L)"},
            "amp_entity": {"name": "Mayhem MF-Amplifier Delta (L)"},
            "scope_entity": null,
        });
        assert_eq!(
            setup_parts(&props),
            vec![
                SetupPart {
                    role: SetupRole::Amplifier,
                    name: "Mayhem MF-Amplifier Delta (L)".into()
                },
                SetupPart {
                    role: SetupRole::Absorber,
                    name: "ArMatrix Extender P20 (L)".into()
                },
                SetupPart {
                    role: SetupRole::Implant,
                    name: "NeoPsion 85-B".into()
                },
            ]
        );
        assert_eq!(
            part_names(&props),
            [
                Some("Mayhem MF-Amplifier Delta (L)".into()),
                None,
                Some("ArMatrix Extender P20 (L)".into()),
                Some("NeoPsion 85-B".into()),
            ]
        );
    }

    #[test]
    fn empty_or_unnamed_parts_are_not_fitted() {
        let props = json!({
            "amp_entity": {},
            "scope_entity": {"name": "  "},
            "absorber_entity": {"name": 3},
        });
        assert!(setup_parts(&props).is_empty());
        assert!(setup_parts(&Value::Null).is_empty());
    }

    #[test]
    fn the_tt_of_a_shot_is_its_lines_before_markup() {
        let cost = json!({
            "costBreakdown": [
                {"costPec": 1.5, "effectiveCostPec": 22.5},
                {"costPec": 2.25, "effectiveCostPec": 2.25},
            ],
            "totalCostPerUse": 24.75,
        });
        assert!((tt_per_shot_pec(&cost) - 3.75).abs() < 1e-9);
        assert_eq!(tt_per_shot_pec(&json!({})), 0.0);
    }
}
