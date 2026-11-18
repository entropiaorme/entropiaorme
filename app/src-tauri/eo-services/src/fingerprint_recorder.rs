//! The event-stream fingerprint recorder.
//!
//! Captures every event published on a bus, in publish order, before
//! subscriber dispatch (the original wraps the publish method; here
//! the bus's full-stream tap is the same observation point). Raw
//! payloads are stored and normalisation deferred to serialisation,
//! so a test inspecting the live event list sees the original values.
//! The serialised form is one compact line per event, keys sorted,
//! with a trailing newline whenever any events were recorded: the
//! byte form of the committed fingerprint goldens.

use std::sync::{Arc, Mutex};

use eo_wire::normalizer::{to_python_json, Normalizer};
use serde_json::{Map, Value};

use crate::bus_events::BusEvent;
use crate::event_bus::{EventBus, Registration};

#[derive(Default)]
struct State {
    events: Vec<BusEvent>,
    installed: Option<Registration>,
}

#[derive(Default)]
pub struct FingerprintRecorder {
    state: Arc<Mutex<State>>,
}

impl FingerprintRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach to the bus's full-stream tap; idempotent while installed
    /// (the original is idempotent on the same bus and re-homes from a
    /// prior bus, which the registration handle makes explicit here:
    /// uninstall from the old bus before installing on a new one).
    pub fn install(&self, bus: &EventBus) {
        let mut state = self.state.lock().expect("recorder state");
        if state.installed.is_some() {
            return;
        }
        let sink = self.state.clone();
        let registration = bus.add_tap(move |event| {
            sink.lock()
                .expect("recorder state")
                .events
                .push(event.clone());
        });
        state.installed = Some(registration);
    }

    /// Detach from the bus; recorded events are kept.
    pub fn uninstall(&self, bus: &EventBus) {
        let mut state = self.state.lock().expect("recorder state");
        if let Some(registration) = state.installed.take() {
            bus.remove_tap(registration);
        }
    }

    /// A copy of the raw recorded events.
    pub fn events(&self) -> Vec<BusEvent> {
        self.state.lock().expect("recorder state").events.clone()
    }

    /// Render the recorded events as the canonical fingerprint JSONL:
    /// one compact line per event in publish order, keys sorted, a
    /// trailing newline whenever any events were recorded. The payload
    /// JSON is the typed event's serde form, which reproduces the
    /// previously hand-built maps exactly (pinned by the bus_events
    /// parity tests), so committed goldens are unchanged.
    pub fn serialize(&self, normalizer: &mut Normalizer) -> String {
        let state = self.state.lock().expect("recorder state");
        if state.events.is_empty() {
            return String::new();
        }
        let mut lines = Vec::with_capacity(state.events.len());
        for event in &state.events {
            let mut entry = Map::new();
            entry.insert("topic".into(), Value::from(event.topic().as_str()));
            entry.insert(
                "payload".into(),
                normalizer.normalize(&event.payload_value()),
            );
            lines.push(to_python_json(&Value::Object(entry), None));
        }
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus_events::{
        GlobalPayload, LootGroupPayload, LootItem, LootTag, TickFlushedPayload,
    };

    #[test]
    fn records_in_publish_order_and_serialises_the_golden_byte_form() {
        let bus = EventBus::new();
        let recorder = FingerprintRecorder::new();
        recorder.install(&bus);
        recorder.install(&bus); // Idempotent while installed.

        bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            arrived_ms: None,
            timestamp: Some("2026-05-19T10:00:02".into()),
            items: vec![LootItem {
                item_name: "Wool".into(),
                quantity: 1,
                value_ped: 1.5,
                is_enhancer_shrapnel: false,
            }],
            total_ped: 1.5,
        }));
        bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
            timestamp: Some("2026-05-19T10:00:02".into()),
        }));

        let mut normalizer = Normalizer::new();
        let fingerprint = recorder.serialize(&mut normalizer);
        let lines: Vec<&str> = fingerprint.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(fingerprint.ends_with('\n'));
        assert_eq!(
            lines[0],
            r#"{"payload": {"items": [{"is_enhancer_shrapnel": false, "item_name": "Wool", "quantity": 1, "value_ped": 1.5}], "timestamp": "<TS_1>", "total_ped": 1.5, "type": "loot"}, "topic": "loot_group"}"#
        );
        assert_eq!(
            lines[1],
            r#"{"payload": {"timestamp": "<TS_1>"}, "topic": "tick_flushed"}"#
        );
    }

    #[test]
    fn uninstall_stops_recording_and_keeps_events() {
        let bus = EventBus::new();
        let recorder = FingerprintRecorder::new();
        recorder.install(&bus);
        let global = BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-05-19T10:00:02".into(),
            player: "Test Player".into(),
            creature: "Atrox".into(),
            value: 1.0,
        });
        bus.publish(&global);
        recorder.uninstall(&bus);
        bus.publish(&global);
        assert_eq!(recorder.events().len(), 1);

        let mut normalizer = Normalizer::new();
        assert!(FingerprintRecorder::new()
            .serialize(&mut normalizer)
            .is_empty());
    }
}
