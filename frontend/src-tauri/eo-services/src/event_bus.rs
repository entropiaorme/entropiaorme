//! Synchronous in-process event dispatch, ported from
//! the original Python implementation.
//!
//! Thread-safe pub/sub for services sharing process memory: per-topic
//! subscribers, plus full-stream taps that observe every publish (the
//! fingerprint recorder's attachment point, so published payload
//! shapes are golden-observable). Dispatch is synchronous on the
//! publisher's thread, taps before subscribers, and a panicking
//! callback is contained exactly as the original contains exceptions.
//! Subscriptions are handle-based where the original deduplicates by
//! callback identity (closures carry no identity here). The consumers'
//! real invariant is balanced subscribe/unsubscribe (the tracker
//! subscribes per session start and unsubscribes on stop), so a
//! ported subscriber must hold its registration handle for the
//! unsubscribe the original performs by bound-method equality.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// The bus topics, mirroring the string constants in the original
/// Python implementation plus the frontend-facing domain-event
/// topic the tracker publishes (from the domain-events module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    Combat,
    LootGroup,
    SkillGain,
    EnhancerBreak,
    Global,
    ActiveToolChanged,
    ActiveHealToolChanged,
    SessionStarted,
    SessionStopped,
    MissionReceived,
    TickFlushed,
    TrackingSessionUpdated,
    ScanStatusChanged,
}

impl Topic {
    /// The wire string, matching the backend constant's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Topic::Combat => "combat",
            Topic::LootGroup => "loot_group",
            Topic::SkillGain => "skill_gain",
            Topic::EnhancerBreak => "enhancer_break",
            Topic::Global => "global",
            Topic::ActiveToolChanged => "active_tool_changed",
            Topic::ActiveHealToolChanged => "active_heal_tool_changed",
            Topic::SessionStarted => "session_started",
            Topic::SessionStopped => "session_stopped",
            Topic::MissionReceived => "mission_received",
            Topic::TickFlushed => "tick_flushed",
            Topic::TrackingSessionUpdated => eo_wire::domain_events::TOPIC_TRACKING_SESSION_UPDATED,
            Topic::ScanStatusChanged => eo_wire::domain_events::TOPIC_SCAN_STATUS_CHANGED,
        }
    }
}

type Subscriber = Arc<dyn Fn(&Value) + Send + Sync>;
type Tap = Arc<dyn Fn(Topic, &Value) + Send + Sync>;

/// A handle for removing a subscriber or tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration(u64);

#[derive(Default)]
struct Registry {
    subscribers: HashMap<Topic, Vec<(u64, Subscriber)>>,
    taps: Vec<(u64, Tap)>,
}

#[derive(Default)]
pub struct EventBus {
    registry: Mutex<Registry>,
    next_id: AtomicU64,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(
        &self,
        topic: Topic,
        callback: impl Fn(&Value) + Send + Sync + 'static,
    ) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut registry = self.registry.lock().expect("bus registry");
        registry
            .subscribers
            .entry(topic)
            .or_default()
            .push((id, Arc::new(callback)));
        Registration(id)
    }

    pub fn unsubscribe(&self, topic: Topic, registration: Registration) {
        let mut registry = self.registry.lock().expect("bus registry");
        if let Some(list) = registry.subscribers.get_mut(&topic) {
            list.retain(|(id, _)| *id != registration.0);
            if list.is_empty() {
                registry.subscribers.remove(&topic);
            }
        }
    }

    pub fn has_subscribers(&self, topic: Topic) -> bool {
        let registry = self.registry.lock().expect("bus registry");
        registry
            .subscribers
            .get(&topic)
            .is_some_and(|list| !list.is_empty())
    }

    /// Install a full-stream observer: runs synchronously on the
    /// publisher's thread for every publish, before subscriber
    /// dispatch, and sees the payload unchanged.
    pub fn add_tap(&self, tap: impl Fn(Topic, &Value) + Send + Sync + 'static) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut registry = self.registry.lock().expect("bus registry");
        registry.taps.push((id, Arc::new(tap)));
        Registration(id)
    }

    pub fn remove_tap(&self, registration: Registration) {
        let mut registry = self.registry.lock().expect("bus registry");
        registry.taps.retain(|(id, _)| *id != registration.0);
    }

    /// Dispatch to taps then topic subscribers, containing panics so a
    /// failing callback cannot break the publish, exactly as the
    /// original contains exceptions. The snapshot is taken under the
    /// lock and dispatched after release (the original's tuple
    /// snapshot), so callbacks may re-enter the bus freely.
    pub fn publish(&self, topic: Topic, data: &Value) {
        // Observe-only event-throughput instrumentation: count the publish and
        // emit a structured trace carrying the TOPIC ONLY, never `data` (which
        // can hold chatlog-derived loot/skill content). Both are panic-free
        // atomic/macro operations and run before dispatch, so they cannot
        // perturb the synchronous fan-out the original contains exceptions
        // around.
        eo_wire::metrics::metrics().record_event_published();
        tracing::trace!(target: "eo::events", topic = topic.as_str(), "event published");
        let (taps, subscribers): (Vec<Tap>, Vec<Subscriber>) = {
            let registry = self.registry.lock().expect("bus registry");
            (
                registry.taps.iter().map(|(_, tap)| tap.clone()).collect(),
                registry
                    .subscribers
                    .get(&topic)
                    .map(|list| list.iter().map(|(_, cb)| cb.clone()).collect())
                    .unwrap_or_default(),
            )
        };
        for tap in taps {
            let _ = catch_unwind(AssertUnwindSafe(|| tap(topic, data)));
        }
        for callback in subscribers {
            let _ = catch_unwind(AssertUnwindSafe(|| callback(data)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[test]
    fn subscribers_receive_their_topic_only() {
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        bus.subscribe(Topic::Combat, move |data| {
            assert_eq!(data["amount"], 5.0);
            observed.fetch_add(1, Ordering::SeqCst);
        });
        bus.publish(Topic::Combat, &json!({"amount": 5.0}));
        bus.publish(Topic::SkillGain, &json!({"amount": 1.0}));
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        assert!(bus.has_subscribers(Topic::Combat));
        assert!(!bus.has_subscribers(Topic::SkillGain));
    }

    #[test]
    fn unsubscribe_clears_topics_and_taps_see_everything() {
        let bus = EventBus::new();
        let registration = bus.subscribe(Topic::Combat, |_| {});
        bus.unsubscribe(Topic::Combat, registration);
        assert!(!bus.has_subscribers(Topic::Combat));

        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        let tap = bus.add_tap(move |topic, data| {
            sink.lock().unwrap().push((topic, data.clone()));
        });
        bus.publish(Topic::Global, &json!({"value": 1}));
        bus.publish(Topic::TickFlushed, &json!({}));
        assert_eq!(stream.lock().unwrap().len(), 2);
        assert_eq!(stream.lock().unwrap()[0].0, Topic::Global);
        bus.remove_tap(tap);
        bus.publish(Topic::Global, &json!({"value": 2}));
        assert_eq!(stream.lock().unwrap().len(), 2);
    }

    #[test]
    fn panicking_callbacks_are_contained() {
        let bus = EventBus::new();
        let reached = Arc::new(AtomicUsize::new(0));
        bus.add_tap(|_, _| panic!("tap down"));
        bus.subscribe(Topic::Combat, |_| panic!("subscriber down"));
        let observed = reached.clone();
        bus.subscribe(Topic::Combat, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        bus.publish(Topic::Combat, &json!({}));
        assert_eq!(reached.load(Ordering::SeqCst), 1, "dispatch survives");
    }

    #[test]
    fn publishing_records_the_event_throughput_metric() {
        // The observe-only instrumentation in `publish` counts every publish
        // into the process-wide registry (the per-service event-throughput
        // signal). The counter is monotonic and shared, so assert a strict
        // increase across two publishes rather than an absolute value.
        let bus = EventBus::new();
        let before = eo_wire::metrics::metrics().snapshot().events_published;
        bus.publish(Topic::Combat, &json!({"amount": 1.0}));
        bus.publish(Topic::TickFlushed, &json!({}));
        let after = eo_wire::metrics::metrics().snapshot().events_published;
        assert!(
            after >= before + 2,
            "two publishes record two throughput samples (before={before}, after={after})"
        );
    }

    #[test]
    fn topic_wire_values_match_the_backend_constants() {
        let expected = [
            (Topic::Combat, "combat"),
            (Topic::LootGroup, "loot_group"),
            (Topic::SkillGain, "skill_gain"),
            (Topic::EnhancerBreak, "enhancer_break"),
            (Topic::Global, "global"),
            (Topic::ActiveToolChanged, "active_tool_changed"),
            (Topic::ActiveHealToolChanged, "active_heal_tool_changed"),
            (Topic::SessionStarted, "session_started"),
            (Topic::SessionStopped, "session_stopped"),
            (Topic::MissionReceived, "mission_received"),
            (Topic::TickFlushed, "tick_flushed"),
            (Topic::TrackingSessionUpdated, "tracking.session.updated"),
            (Topic::ScanStatusChanged, "scan.status.changed"),
        ];
        for (topic, wire) in expected {
            assert_eq!(topic.as_str(), wire);
        }
    }
}
