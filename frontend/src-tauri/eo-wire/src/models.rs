//! HTTP response models: the native side of the API wire contract.
//!
//! Conventions, mirroring the original Python implementation:
//!
//! - **Extra-allow carry-forward**: every response model ends with a
//!   flattened map, so undeclared keys pass through serialisation
//!   untouched (the `_Loose` base's behaviour). Declared fields emit
//!   first in declaration order, extras after, matching the Python dump
//!   order.
//! - **Uniform float typing**: numeric value fields are `f64` and
//!   serialise in float form; genuinely integral fields (counts, ranks,
//!   ids) are integers.
//! - **Lean polymorphic shapes**: optional fields skip serialisation
//!   when `None`, mirroring the `response_model_exclude_unset=True`
//!   discipline under which the Python handlers never emit an explicit
//!   null for these fields. A model whose response goldens show explicit
//!   nulls overrides this per field at its port.
//!
//! Each model declares a [`ModelContract`] that the OpenAPI conformance
//! gate asserts against the committed `openapi.snapshot.json` component
//! of the same name, so the native types are mechanically tied to the
//! ratified contract from the moment they land. Models join this module
//! as their routes port; the registry must cover every component by the
//! time the HTTP surface moves natively.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The JSON shape a declared field must occupy in the snapshot component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSchema {
    Str,
    Bool,
    Int,
    Float,
    /// A free-form JSON object (`dict[str, Any]`).
    Object,
    /// An array of floats.
    ListFloat,
    /// An array of `$ref` items naming another component.
    ListRef(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    pub name: &'static str,
    pub schema: FieldSchema,
    pub required: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelContract {
    pub component: &'static str,
    pub extra_allow: bool,
    pub fields: &'static [FieldSpec],
}

/// Implemented by every native response model; the conformance gate
/// walks [`registered_contracts`].
pub trait WireModel {
    fn contract() -> ModelContract;
}

/// Every native response model landed so far, in landing order.
pub fn registered_contracts() -> Vec<ModelContract> {
    vec![
        HealthStatus::contract(),
        NotableEvent::contract(),
        TrackingSnapshot::contract(),
    ]
}

const fn field(name: &'static str, schema: FieldSchema, required: bool) -> FieldSpec {
    FieldSpec {
        name,
        schema,
        required,
    }
}

/// The health-check acknowledgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WireModel for HealthStatus {
    fn contract() -> ModelContract {
        const FIELDS: &[FieldSpec] = &[field("status", FieldSchema::Str, true)];
        ModelContract {
            component: "HealthStatus",
            extra_allow: true,
            fields: FIELDS,
        }
    }
}

/// A dashboard activity-feed entry (notable events and tracker warnings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotableEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub value: f64,
    #[serde(rename = "eventType", default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WireModel for NotableEvent {
    fn contract() -> ModelContract {
        const FIELDS: &[FieldSpec] = &[
            field("type", FieldSchema::Str, true),
            field("description", FieldSchema::Str, true),
            field("value", FieldSchema::Float, true),
            field("eventType", FieldSchema::Str, false),
            field("timestamp", FieldSchema::Str, false),
            field("id", FieldSchema::Str, false),
        ];
        ModelContract {
            component: "NotableEvent",
            extra_allow: true,
            fields: FIELDS,
        }
    }
}

/// The consolidated dashboard hydration shape: the union of the status,
/// live, and recent-events readouts, polymorphic across the
/// unavailable | idle | active states. Casing is preserved from the
/// readouts it unions (`session_id` family snake-case, headline numbers
/// camelCase), exactly as the Python model declares them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackingSnapshot {
    pub status: String,
    // Shared by idle + active.
    #[serde(
        rename = "hotbarListenerActive",
        skip_serializing_if = "Option::is_none"
    )]
    pub hotbar_listener_active: Option<bool>,
    #[serde(rename = "weaponAttribution", skip_serializing_if = "Option::is_none")]
    pub weapon_attribution: Option<String>,
    #[serde(rename = "repairOcrEnabled", skip_serializing_if = "Option::is_none")]
    pub repair_ocr_enabled: Option<bool>,
    #[serde(
        rename = "endOfSessionArmourReminderEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub end_of_session_armour_reminder_enabled: Option<bool>,
    #[serde(rename = "mobEntryMode", skip_serializing_if = "Option::is_none")]
    pub mob_entry_mode: Option<String>,
    #[serde(rename = "currentMob", skip_serializing_if = "Option::is_none")]
    pub current_mob: Option<String>,
    #[serde(rename = "mobSource", skip_serializing_if = "Option::is_none")]
    pub mob_source: Option<String>,
    #[serde(rename = "currentTool", skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(
        rename = "trifectaAttribution",
        skip_serializing_if = "Option::is_none"
    )]
    pub trifecta_attribution: Option<Map<String, Value>>,
    #[serde(rename = "recentEvents", skip_serializing_if = "Option::is_none")]
    pub recent_events: Option<Vec<NotableEvent>>,
    // Active only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kill_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<f64>,
    #[serde(rename = "returnRate", skip_serializing_if = "Option::is_none")]
    pub return_rate: Option<f64>,
    #[serde(rename = "damageDealtTotal", skip_serializing_if = "Option::is_none")]
    pub damage_dealt_total: Option<f64>,
    #[serde(rename = "weaponDamageDealt", skip_serializing_if = "Option::is_none")]
    pub weapon_damage_dealt: Option<f64>,
    #[serde(rename = "weaponCost", skip_serializing_if = "Option::is_none")]
    pub weapon_cost: Option<f64>,
    #[serde(rename = "shotsFiredTotal", skip_serializing_if = "Option::is_none")]
    pub shots_fired_total: Option<i64>,
    #[serde(rename = "criticalHitsTotal", skip_serializing_if = "Option::is_none")]
    pub critical_hits_total: Option<i64>,
    #[serde(rename = "maxDamage", skip_serializing_if = "Option::is_none")]
    pub max_damage: Option<f64>,
    #[serde(rename = "globalsCount", skip_serializing_if = "Option::is_none")]
    pub globals_count: Option<i64>,
    #[serde(rename = "hofsCount", skip_serializing_if = "Option::is_none")]
    pub hofs_count: Option<i64>,
    #[serde(rename = "latestKillLoot", skip_serializing_if = "Option::is_none")]
    pub latest_kill_loot: Option<f64>,
    #[serde(rename = "multiplierLast", skip_serializing_if = "Option::is_none")]
    pub multiplier_last: Option<f64>,
    #[serde(rename = "multiplierAvg", skip_serializing_if = "Option::is_none")]
    pub multiplier_avg: Option<f64>,
    #[serde(rename = "multiplierMax", skip_serializing_if = "Option::is_none")]
    pub multiplier_max: Option<f64>,
    #[serde(rename = "multiplierHistory", skip_serializing_if = "Option::is_none")]
    pub multiplier_history: Option<Vec<f64>>,
    #[serde(
        rename = "cumulativeNetHistory",
        skip_serializing_if = "Option::is_none"
    )]
    pub cumulative_net_history: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<NotableEvent>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WireModel for TrackingSnapshot {
    fn contract() -> ModelContract {
        const FIELDS: &[FieldSpec] = &[
            field("status", FieldSchema::Str, true),
            field("hotbarListenerActive", FieldSchema::Bool, false),
            field("weaponAttribution", FieldSchema::Str, false),
            field("repairOcrEnabled", FieldSchema::Bool, false),
            field(
                "endOfSessionArmourReminderEnabled",
                FieldSchema::Bool,
                false,
            ),
            field("mobEntryMode", FieldSchema::Str, false),
            field("currentMob", FieldSchema::Str, false),
            field("mobSource", FieldSchema::Str, false),
            field("currentTool", FieldSchema::Str, false),
            field("trifectaAttribution", FieldSchema::Object, false),
            field("recentEvents", FieldSchema::ListRef("NotableEvent"), false),
            field("session_id", FieldSchema::Str, false),
            field("started_at", FieldSchema::Str, false),
            field("kill_count", FieldSchema::Int, false),
            field("elapsed", FieldSchema::Int, false),
            field("cost", FieldSchema::Float, false),
            field("returns", FieldSchema::Float, false),
            field("pes", FieldSchema::Float, false),
            field("net", FieldSchema::Float, false),
            field("returnRate", FieldSchema::Float, false),
            field("damageDealtTotal", FieldSchema::Float, false),
            field("weaponDamageDealt", FieldSchema::Float, false),
            field("weaponCost", FieldSchema::Float, false),
            field("shotsFiredTotal", FieldSchema::Int, false),
            field("criticalHitsTotal", FieldSchema::Int, false),
            field("maxDamage", FieldSchema::Float, false),
            field("globalsCount", FieldSchema::Int, false),
            field("hofsCount", FieldSchema::Int, false),
            field("latestKillLoot", FieldSchema::Float, false),
            field("multiplierLast", FieldSchema::Float, false),
            field("multiplierAvg", FieldSchema::Float, false),
            field("multiplierMax", FieldSchema::Float, false),
            field("multiplierHistory", FieldSchema::ListFloat, false),
            field("cumulativeNetHistory", FieldSchema::ListFloat, false),
            field("warnings", FieldSchema::ListRef("NotableEvent"), false),
        ];
        ModelContract {
            component: "TrackingSnapshot",
            extra_allow: true,
            fields: FIELDS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_serialises_to_the_wire_shape() {
        let model = HealthStatus {
            status: "ok".into(),
            extra: Map::new(),
        };
        assert_eq!(
            serde_json::to_string(&model).unwrap(),
            "{\"status\":\"ok\"}"
        );
    }

    #[test]
    fn unavailable_snapshot_keeps_its_lean_shape() {
        // Mirrors response_model_exclude_unset: an unavailable-state
        // snapshot is exactly {"status": ...}, no null wall.
        let model = TrackingSnapshot {
            status: "unavailable".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&model).unwrap(),
            "{\"status\":\"unavailable\"}"
        );
    }

    #[test]
    fn undeclared_keys_round_trip_after_declared_fields() {
        let wire = "{\"status\":\"ok\",\"buildInfo\":{\"channel\":\"beta\"}}";
        let model: HealthStatus = serde_json::from_str(wire).unwrap();
        assert_eq!(model.status, "ok");
        assert_eq!(serde_json::to_string(&model).unwrap(), wire);
    }

    #[test]
    fn declared_fields_emit_in_declaration_order_before_extras() {
        let mut extra = Map::new();
        extra.insert("zCustom".into(), Value::from(1));
        let model = NotableEvent {
            kind: "global".into(),
            description: "d".into(),
            value: 12.5,
            event_type: Some("hunt".into()),
            timestamp: None,
            id: None,
            extra,
        };
        assert_eq!(
            serde_json::to_string(&model).unwrap(),
            "{\"type\":\"global\",\"description\":\"d\",\"value\":12.5,\"eventType\":\"hunt\",\"zCustom\":1}"
        );
    }
}
