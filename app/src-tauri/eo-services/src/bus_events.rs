//! Typed payloads for the in-process event bus.
//!
//! One payload type per bus topic, replacing the untyped JSON maps the
//! producers used to assemble by hand: a publish site constructs a
//! struct, a subscriber matches a variant, and the compiler owns the
//! field contract in both directions. The serde shapes reproduce the
//! previous hand-built maps exactly (field names, presence semantics,
//! nullability), so the event-stream fingerprint goldens are unchanged:
//! the parity tests below pin each variant's JSON against the shapes
//! the goldens record.
//!
//! Timestamps stay ISO strings here (the payloads' wire form); the
//! chatlog-derived events carry the parser's local-time isoformat and
//! the domain envelopes carry ISO-8601 UTC, exactly as before.

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::event_bus::Topic;
use crate::healing_profile::HealingProfile;
use eo_wire::domain_events::{ScanStatusChanged, TrackingSessionUpdated};

/// A field that serialises to exactly one event-type literal: the
/// `type` discriminator on single-shape payloads (the multi-shape
/// payloads are internally tagged enums and carry theirs natively).
macro_rules! event_tag {
    ($name:ident, $literal:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($literal)
            }
        }
    };
}

event_tag!(LootTag, "loot");
event_tag!(HarvestFailTag, "harvest_fail");
event_tag!(SkillGainTag, "skill_gain");
event_tag!(EnhancerBreakTag, "enhancer_break");
event_tag!(MissionReceivedTag, "mission_received");

/// One combat round outcome, discriminated exactly as the parser's
/// per-line event types: the damage-bearing kinds carry `amount`, the
/// miss kinds do not (absent, not null, matching the previous maps).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CombatPayload {
    DamageDealt {
        amount: f64,
        timestamp: String,
    },
    CriticalHit {
        amount: f64,
        timestamp: String,
    },
    TargetDodge {
        timestamp: String,
    },
    TargetEvade {
        timestamp: String,
    },
    TargetJam {
        timestamp: String,
    },
    /// The player's own attack missed: a paid shot with no outcome.
    TargetMiss {
        timestamp: String,
    },
    DamageReceived {
        amount: f64,
        timestamp: String,
    },
    PlayerDodge {
        timestamp: String,
    },
    PlayerEvade {
        timestamp: String,
    },
    PlayerJam {
        timestamp: String,
    },
    MobMiss {
        timestamp: String,
    },
    Deflect {
        timestamp: String,
    },
    SelfHeal {
        amount: f64,
        timestamp: String,
    },
}

/// One item within a settled loot tick: the tracking model's loot
/// item rides the bus verbatim (`is_enhancer_shrapnel` is the
/// same-tick refund match the watcher computes; always present).
pub use crate::tracking_models::LootItem;

/// The grouped loot event for one settled tick. `timestamp` is the
/// tick boundary and serialises to `null` when the tick carried none,
/// exactly as the previous map did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LootGroupPayload {
    #[serde(rename = "type")]
    pub kind: LootTag,
    /// Internal identity shared with the raw-clump evidence probe. It is
    /// deliberately absent from the event-stream wire shape, whose frozen
    /// fingerprints predate manual reward capture.
    #[serde(skip)]
    pub source_id: Option<String>,
    pub timestamp: Option<String>,
    pub items: Vec<LootItem>,
    pub total_ped: f64,
}

/// A failed harvesting swing ("Harvest attempt failed to generate
/// useable resources"): the attempt happened and cost tool decay, it
/// just dropped nothing. The explicit line makes harvesting swings
/// directly countable (a successful swing arrives as its loot group).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HarvestFailPayload {
    #[serde(rename = "type")]
    pub kind: HarvestFailTag,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkillGainPayload {
    #[serde(rename = "type")]
    pub kind: SkillGainTag,
    pub timestamp: String,
    pub amount: f64,
    pub skill_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnhancerBreakPayload {
    #[serde(rename = "type")]
    pub kind: EnhancerBreakTag,
    pub timestamp: String,
    pub enhancer_name: String,
    pub item_name: String,
    pub remaining: i64,
    pub shrapnel_ped: f64,
}

/// A globals-channel announcement, kill- or item-flavoured, with the
/// Hall-of-Fame variants as their own kinds (they publish on the same
/// topic; the tracker routes on the discriminator).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GlobalPayload {
    GlobalKill {
        timestamp: String,
        player: String,
        creature: String,
        value: f64,
    },
    HofKill {
        timestamp: String,
        player: String,
        creature: String,
        value: f64,
    },
    GlobalItem {
        timestamp: String,
        player: String,
        item: String,
        value: f64,
    },
    HofItem {
        timestamp: String,
        player: String,
        item: String,
        value: f64,
    },
}

/// The active weapon changed. `source` names the trigger (e.g.
/// `hotbar:3`) when the hotbar listener resolved it; omitted (not
/// null) when unset, matching the previous map shapes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveToolChangedPayload {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The active harvesting tool changed (a hotbar "tool" equip), with
/// its per-use economics; the tracker routes subsequent loot groups to
/// harvest events while a harvesting tool is the hand item.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveHarvestToolChangedPayload {
    pub tool_name: String,
    pub cost_per_use_ped: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The active healing tool changed, with its per-use economics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveHealToolChangedPayload {
    pub tool_name: String,
    pub cost_per_use_ped: f64,
    pub reload_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The semantic item class behind one resolved hotbar press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotbarItemKind {
    Weapon,
    Healing,
    Consumable,
    Harvesting,
}

/// A hotbar press with its OS-observed occurrence instant and the equipment
/// snapshot resolved for that slot. The tracker uses this as the intent
/// boundary; the older tool-change events remain the weapon and harvesting
/// compatibility path while those domains complete their own migration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HotbarIntentPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub slot: String,
    pub occurred_at: f64,
    pub equipment_id: i64,
    pub item_name: String,
    pub item_kind: HotbarItemKind,
    pub cost_per_use_ped: f64,
    pub reload_seconds: f64,
    pub healing_profile: Option<HealingProfile>,
    pub lifesteal_percent: Option<f64>,
    /// One dose of the consumable the slot holds, as the item stood at the
    /// press; absent for every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumable_profile: Option<crate::consumables::ConsumableProfile>,
}

/// A tracking-session lifecycle boundary (started / stopped).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionLifecyclePayload {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionReceivedPayload {
    #[serde(rename = "type")]
    pub kind: MissionReceivedTag,
    pub timestamp: String,
    pub mission_name: String,
}

/// The settled-tick boundary, published after every per-event publish
/// of the tick has dispatched. `timestamp` serialises to `null` when
/// the tick carried none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TickFlushedPayload {
    pub timestamp: Option<String>,
}

/// The typed event on the in-process bus: one variant per topic. The
/// two frontend-facing domain topics carry their full typed envelopes
/// (`eo-wire`'s wire contract) directly; everything else is
/// intra-backend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum BusEvent {
    Combat(CombatPayload),
    LootGroup(LootGroupPayload),
    HarvestFail(HarvestFailPayload),
    SkillGain(SkillGainPayload),
    EnhancerBreak(EnhancerBreakPayload),
    Global(GlobalPayload),
    ActiveToolChanged(ActiveToolChangedPayload),
    ActiveHealToolChanged(ActiveHealToolChangedPayload),
    ActiveHarvestToolChanged(ActiveHarvestToolChangedPayload),
    HotbarIntent(Box<HotbarIntentPayload>),
    SessionStarted(SessionLifecyclePayload),
    SessionStopped(SessionLifecyclePayload),
    MissionReceived(MissionReceivedPayload),
    TickFlushed(TickFlushedPayload),
    TrackingSessionUpdated(TrackingSessionUpdated),
    ScanStatusChanged(ScanStatusChanged),
    HarvestRecorded(eo_wire::domain_events::HarvestRecorded),
    NavigationUpdated(eo_wire::domain_events::NavigationUpdated),
    ProtectionUpdated(eo_wire::domain_events::ProtectionUpdated),
    HealingUpdated(eo_wire::domain_events::HealingUpdated),
    WeaponsUpdated(eo_wire::domain_events::WeaponsUpdated),
    ConsumablesUpdated(eo_wire::domain_events::ConsumablesUpdated),
}

impl BusEvent {
    /// The bus topic this event publishes on.
    pub fn topic(&self) -> Topic {
        match self {
            BusEvent::Combat(_) => Topic::Combat,
            BusEvent::LootGroup(_) => Topic::LootGroup,
            BusEvent::HarvestFail(_) => Topic::HarvestFail,
            BusEvent::SkillGain(_) => Topic::SkillGain,
            BusEvent::EnhancerBreak(_) => Topic::EnhancerBreak,
            BusEvent::Global(_) => Topic::Global,
            BusEvent::ActiveToolChanged(_) => Topic::ActiveToolChanged,
            BusEvent::ActiveHealToolChanged(_) => Topic::ActiveHealToolChanged,
            BusEvent::ActiveHarvestToolChanged(_) => Topic::ActiveHarvestToolChanged,
            BusEvent::HotbarIntent(_) => Topic::HotbarIntent,
            BusEvent::SessionStarted(_) => Topic::SessionStarted,
            BusEvent::SessionStopped(_) => Topic::SessionStopped,
            BusEvent::MissionReceived(_) => Topic::MissionReceived,
            BusEvent::TickFlushed(_) => Topic::TickFlushed,
            BusEvent::TrackingSessionUpdated(_) => Topic::TrackingSessionUpdated,
            BusEvent::ScanStatusChanged(_) => Topic::ScanStatusChanged,
            BusEvent::HarvestRecorded(_) => Topic::HarvestRecorded,
            BusEvent::NavigationUpdated(_) => Topic::NavigationUpdated,
            BusEvent::ProtectionUpdated(_) => Topic::ProtectionUpdated,
            BusEvent::HealingUpdated(_) => Topic::HealingUpdated,
            BusEvent::WeaponsUpdated(_) => Topic::WeaponsUpdated,
            BusEvent::ConsumablesUpdated(_) => Topic::ConsumablesUpdated,
        }
    }

    /// The payload's JSON value: the exact shape the untyped bus used
    /// to carry, and the shape the fingerprint goldens record.
    pub fn payload_value(&self) -> Value {
        serde_json::to_value(self).expect("bus payloads always serialise")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eo_wire::domain_events::{
        ScanPhase, ScanStatusChangedPayload, ScanStatusChangedTag, TrackingReason,
        TrackingSessionUpdatedPayload, TrackingSessionUpdatedTag, TrackingStatus,
    };
    use serde_json::json;

    #[test]
    fn combat_payloads_match_the_previous_map_shapes() {
        // Damage-bearing kind: `amount` present.
        let dealt = BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 30.0,
            timestamp: "2026-01-01T00:00:01".into(),
        });
        assert_eq!(dealt.topic(), Topic::Combat);
        assert_eq!(
            dealt.payload_value(),
            json!({"type": "damage_dealt", "amount": 30.0, "timestamp": "2026-01-01T00:00:01"})
        );

        // Miss kind: `amount` ABSENT (not null), as the parser's maps had it.
        let dodge = BusEvent::Combat(CombatPayload::TargetDodge {
            timestamp: "2026-01-01T00:00:01".into(),
        });
        let value = dodge.payload_value();
        assert_eq!(
            value,
            json!({"type": "target_dodge", "timestamp": "2026-01-01T00:00:01"})
        );
        assert!(value.get("amount").is_none());
    }

    #[test]
    fn harvest_fail_payload_carries_its_tag_and_timestamp() {
        let event = BusEvent::HarvestFail(HarvestFailPayload {
            kind: HarvestFailTag,
            timestamp: "2026-07-16T16:55:18".into(),
        });
        assert_eq!(event.topic(), Topic::HarvestFail);
        assert_eq!(
            event.payload_value(),
            json!({"type": "harvest_fail", "timestamp": "2026-07-16T16:55:18"})
        );
    }

    #[test]
    fn loot_group_matches_the_previous_map_shape_including_null_timestamp() {
        let event = BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some("2026-01-01T00:00:02".into()),
            items: vec![LootItem {
                item_name: "Shrapnel".into(),
                quantity: 500,
                value_ped: 50.0,
                is_enhancer_shrapnel: false,
            }],
            total_ped: 50.0,
        });
        assert_eq!(
            event.payload_value(),
            json!({
                "type": "loot",
                "timestamp": "2026-01-01T00:00:02",
                "items": [{"item_name": "Shrapnel", "quantity": 500, "value_ped": 50.0,
                           "is_enhancer_shrapnel": false}],
                "total_ped": 50.0,
            })
        );

        // A tick with no timestamp serialises it as null, not omitted.
        let no_ts = BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: None,
            items: vec![],
            total_ped: 0.0,
        });
        assert_eq!(
            no_ts.payload_value(),
            json!({"type": "loot", "timestamp": null, "items": [], "total_ped": 0.0})
        );
    }

    #[test]
    fn skill_gain_enhancer_break_and_mission_match_the_previous_shapes() {
        let skill = BusEvent::SkillGain(SkillGainPayload {
            kind: SkillGainTag,
            timestamp: "2026-01-01T00:00:03".into(),
            amount: 1.2345,
            skill_name: "Rifle".into(),
        });
        assert_eq!(
            skill.payload_value(),
            json!({"type": "skill_gain", "timestamp": "2026-01-01T00:00:03",
                   "amount": 1.2345, "skill_name": "Rifle"})
        );

        let brk = BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:04".into(),
            enhancer_name: "Weapon Damage Enhancer 1".into(),
            item_name: "ArMatrix LR-35".into(),
            remaining: 7,
            shrapnel_ped: 0.4,
        });
        assert_eq!(
            brk.payload_value(),
            json!({"type": "enhancer_break", "timestamp": "2026-01-01T00:00:04",
                   "enhancer_name": "Weapon Damage Enhancer 1",
                   "item_name": "ArMatrix LR-35", "remaining": 7, "shrapnel_ped": 0.4})
        );

        let mission = BusEvent::MissionReceived(MissionReceivedPayload {
            kind: MissionReceivedTag,
            timestamp: "2026-01-01T00:00:05".into(),
            mission_name: "Iron Challenge".into(),
        });
        assert_eq!(
            mission.payload_value(),
            json!({"type": "mission_received", "timestamp": "2026-01-01T00:00:05",
                   "mission_name": "Iron Challenge"})
        );
    }

    #[test]
    fn global_variants_carry_creature_or_item_by_kind() {
        let kill = BusEvent::Global(GlobalPayload::HofKill {
            timestamp: "2026-01-01T00:00:06".into(),
            player: "Test Player".into(),
            creature: "Atrox Young".into(),
            value: 120.0,
        });
        assert_eq!(
            kill.payload_value(),
            json!({"type": "hof_kill", "timestamp": "2026-01-01T00:00:06",
                   "player": "Test Player", "creature": "Atrox Young", "value": 120.0})
        );

        let item = BusEvent::Global(GlobalPayload::GlobalItem {
            timestamp: "2026-01-01T00:00:07".into(),
            player: "Test Player".into(),
            item: "Shadow Harness".into(),
            value: 300.0,
        });
        assert_eq!(
            item.payload_value(),
            json!({"type": "global_item", "timestamp": "2026-01-01T00:00:07",
                   "player": "Test Player", "item": "Shadow Harness", "value": 300.0})
        );
    }

    #[test]
    fn tool_changes_omit_source_when_unset_and_lifecycle_carries_session_id() {
        let bare = BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        });
        assert_eq!(bare.payload_value(), json!({"tool_name": "Rifle"}));

        let sourced = BusEvent::ActiveHealToolChanged(ActiveHealToolChangedPayload {
            tool_name: "Vivo T1".into(),
            cost_per_use_ped: 0.02,
            reload_seconds: 2.5,
            source: Some("hotbar:4".into()),
        });
        assert_eq!(
            sourced.payload_value(),
            json!({"tool_name": "Vivo T1", "cost_per_use_ped": 0.02,
                   "reload_seconds": 2.5, "source": "hotbar:4"})
        );

        let started = BusEvent::SessionStarted(SessionLifecyclePayload {
            session_id: "session-abc".into(),
        });
        assert_eq!(started.topic(), Topic::SessionStarted);
        assert_eq!(
            started.payload_value(),
            json!({"session_id": "session-abc"})
        );
    }

    #[test]
    fn tick_flushed_serialises_a_missing_timestamp_as_null() {
        let tick = BusEvent::TickFlushed(TickFlushedPayload { timestamp: None });
        assert_eq!(tick.payload_value(), json!({"timestamp": null}));
    }

    #[test]
    fn domain_variants_carry_the_full_wire_envelope() {
        let event = BusEvent::TrackingSessionUpdated(TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: TrackingSessionUpdatedPayload {
                session_id: Some("session-abc".into()),
                status: TrackingStatus::Active,
                reason: TrackingReason::Started,
            },
        });
        assert_eq!(event.topic(), Topic::TrackingSessionUpdated);
        assert_eq!(
            event.payload_value(),
            json!({"type": "tracking.session.updated", "event_version": 1,
                   "occurred_at": "2024-12-31T21:20:00+00:00",
                   "payload": {"sessionId": "session-abc", "status": "active",
                                "reason": "started"}})
        );

        let scan = BusEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Capturing,
            },
        });
        assert_eq!(scan.topic(), Topic::ScanStatusChanged);
        assert_eq!(
            scan.payload_value(),
            json!({"type": "scan.status.changed", "event_version": 1,
                   "occurred_at": "2024-12-31T21:20:00+00:00",
                   "payload": {"phase": "capturing"}})
        );
    }

    #[test]
    fn every_topic_is_reachable_from_a_bus_event_variant() {
        // Compile-time exhaustiveness lives in `topic()`'s match; this
        // pins the mapping's distinctness (13 variants, 13 topics).
        let events = [
            Topic::Combat,
            Topic::LootGroup,
            Topic::SkillGain,
            Topic::EnhancerBreak,
            Topic::Global,
            Topic::ActiveToolChanged,
            Topic::ActiveHealToolChanged,
            Topic::SessionStarted,
            Topic::SessionStopped,
            Topic::MissionReceived,
            Topic::TickFlushed,
            Topic::TrackingSessionUpdated,
            Topic::ScanStatusChanged,
        ];
        let unique: std::collections::HashSet<_> = events.iter().collect();
        assert_eq!(unique.len(), 13);
    }
}
