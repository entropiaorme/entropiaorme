//! Hotbar key listener, ported from
//! `backend/services/hotbar_listener.py`: observes hotbar slot
//! keypresses and resolves them into active-tool, heal-tool, and
//! consumable outcomes on the bus.
//!
//! The listener gates its keystroke source on the capability toggle
//! and an active tracking session, observed through the bus's session
//! events. Resolution runs on one owned worker rather than a
//! short-lived thread per press; a failing resolver is contained.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use crate::event_bus::{EventBus, Registration, Topic};
use crate::keystroke_source::{KeystrokeEvent, KeystrokeKind, KeystrokeSource};

/// Hotbar slot keys: the number row 1-9 and 0.
pub const HOTBAR_SLOT_KEYS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

/// The resolver: slot key -> (name, cost_per_use, item_type,
/// reload_seconds) or None for an empty slot.
pub type HotbarResolver = Arc<dyn Fn(&str) -> Option<(String, f64, String, f64)> + Send + Sync>;

/// A keystroke observer (the recording controller's seam): called with
/// (key, kind) for each hotbar-slot press.
pub type KeyTap = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct Gate {
    hooks_enabled: AtomicBool,
    session_active: AtomicBool,
    source_running: AtomicBool,
    // One-shot per start episode: whether the "first keystroke delivered"
    // breadcrumb has been logged. Lets the rolling logfile show whether the
    // OS hook actually delivered after attaching.
    first_delivery_logged: AtomicBool,
}

pub struct HotbarListener {
    bus: Arc<EventBus>,
    source: Option<Arc<dyn KeystrokeSource>>,
    gate: Arc<Gate>,
    key_tap: Arc<Mutex<Option<KeyTap>>>,
    resolve_queue: Mutex<Option<Sender<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    session_subscriptions: Mutex<Option<(Registration, Registration)>>,
}

impl HotbarListener {
    /// A `None` source leaves the listener inert, matching the
    /// original's missing-hook-library path.
    pub fn new(
        bus: Arc<EventBus>,
        source: Option<Arc<dyn KeystrokeSource>>,
        resolver: Option<HotbarResolver>,
    ) -> Arc<Self> {
        let gate = Arc::new(Gate {
            hooks_enabled: AtomicBool::new(false),
            session_active: AtomicBool::new(false),
            source_running: AtomicBool::new(false),
            first_delivery_logged: AtomicBool::new(false),
        });
        let key_tap: Arc<Mutex<Option<KeyTap>>> = Arc::new(Mutex::new(None));

        // One owned worker drains slot resolutions off the hook
        // thread (rather than a thread per press).
        let (queue, worker) = match resolver {
            None => (None, None),
            Some(resolver) => {
                let (sender, receiver) = channel::<String>();
                let worker_bus = bus.clone();
                let handle = std::thread::Builder::new()
                    .name("hotbar-resolve".into())
                    .spawn(move || {
                        while let Ok(slot) = receiver.recv() {
                            resolve_hotbar_slot(&worker_bus, &resolver, &slot);
                        }
                    })
                    .expect("resolve worker spawns");
                (Some(sender), Some(handle))
            }
        };

        let listener = Arc::new(Self {
            bus: bus.clone(),
            source: source.clone(),
            gate: gate.clone(),
            key_tap: key_tap.clone(),
            resolve_queue: Mutex::new(queue),
            worker: Mutex::new(worker),
            session_subscriptions: Mutex::new(None),
        });

        if let Some(source) = source {
            let dispatch_listener = listener.clone();
            source.subscribe(Arc::new(move |event: &KeystrokeEvent| {
                dispatch_listener.on_keystroke(event);
            }));
        }

        let started_listener = listener.clone();
        let started = bus.subscribe(Topic::SessionStarted, move |_| {
            started_listener
                .gate
                .session_active
                .store(true, Ordering::SeqCst);
            started_listener.reconcile();
        });
        let stopped_listener = listener.clone();
        let stopped = bus.subscribe(Topic::SessionStopped, move |_| {
            stopped_listener
                .gate
                .session_active
                .store(false, Ordering::SeqCst);
            stopped_listener.reconcile();
        });
        *listener
            .session_subscriptions
            .lock()
            .expect("subscriptions") = Some((started, stopped));

        listener
    }

    /// True when the keystroke source is currently delivering events.
    pub fn is_running(&self) -> bool {
        self.gate.source_running.load(Ordering::SeqCst)
    }

    /// Install a keystroke observer.
    pub fn set_key_tap(&self, tap: KeyTap) {
        *self.key_tap.lock().expect("key tap") = Some(tap);
    }

    /// Remove the keystroke observer.
    pub fn clear_key_tap(&self) {
        *self.key_tap.lock().expect("key tap") = None;
    }

    /// Apply the hotbar capability toggle; the source still only runs
    /// while a tracking session is active.
    pub fn set_hotbar_hooks_enabled(&self, enabled: bool) {
        self.gate.hooks_enabled.store(enabled, Ordering::SeqCst);
        self.reconcile();
    }

    /// Tear down at shutdown: unsubscribe the session events, stop the
    /// source, clear the gates, and end the resolve worker. This call
    /// is the lifecycle contract (as the original's stop is): the bus
    /// subscriptions hold the listener alive through their closures,
    /// so only an explicit stop breaks that cycle and releases it.
    pub fn stop(&self) {
        if let Some((started, stopped)) = self
            .session_subscriptions
            .lock()
            .expect("subscriptions")
            .take()
        {
            self.bus.unsubscribe(Topic::SessionStarted, started);
            self.bus.unsubscribe(Topic::SessionStopped, stopped);
        }
        self.stop_source();
        self.gate.hooks_enabled.store(false, Ordering::SeqCst);
        self.gate.session_active.store(false, Ordering::SeqCst);
        *self.resolve_queue.lock().expect("resolve queue") = None;
        if let Some(worker) = self.worker.lock().expect("worker").take() {
            let _ = worker.join();
        }
    }

    fn reconcile(&self) {
        if self.gate.hooks_enabled.load(Ordering::SeqCst)
            && self.gate.session_active.load(Ordering::SeqCst)
        {
            self.start_source();
        } else {
            self.stop_source();
        }
    }

    fn start_source(&self) {
        let Some(source) = &self.source else {
            return;
        };
        if self.gate.source_running.load(Ordering::SeqCst) {
            return;
        }
        // The source reports whether the underlying mechanism actually
        // attached; running honestly reflects whether events will come.
        let attached = source.start();
        self.gate.source_running.store(attached, Ordering::SeqCst);
        // Reset the delivery breadcrumb for this episode and record the
        // attach outcome so a non-attaching hook is
        // visible in the rolling logfile of the packaged build.
        self.gate
            .first_delivery_logged
            .store(false, Ordering::SeqCst);
        tracing::info!(target: "eo::input", attached, "hotbar keystroke source start requested");
    }

    fn stop_source(&self) {
        let Some(source) = &self.source else {
            return;
        };
        if !self.gate.source_running.load(Ordering::SeqCst) {
            return;
        }
        source.stop();
        self.gate.source_running.store(false, Ordering::SeqCst);
    }

    fn on_keystroke(&self, event: &KeystrokeEvent) {
        if !self.gate.source_running.load(Ordering::SeqCst) {
            return;
        }
        // One-shot per start: confirm the hook is actually delivering
        // keystrokes. Non-content: no key value.
        if !self.gate.first_delivery_logged.swap(true, Ordering::SeqCst) {
            tracing::info!(
                target: "eo::input",
                "hotbar listener received its first keystroke since start"
            );
        }
        if event.kind != KeystrokeKind::Press {
            return;
        }
        if !HOTBAR_SLOT_KEYS.contains(&event.key.as_str()) {
            return;
        }
        let tap = self.key_tap.lock().expect("key tap").clone();
        if let Some(tap) = tap {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&event.key, "press")));
        }
        if let Some(queue) = self.resolve_queue.lock().expect("resolve queue").as_ref() {
            let _ = queue.send(event.key.clone());
        }
    }
}

/// Resolve a slot and publish the tool change; runs on the owned
/// worker, with failures contained exactly as the original contains
/// its worker-thread errors.
fn resolve_hotbar_slot(bus: &EventBus, resolver: &HotbarResolver, slot: &str) {
    let resolved =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolver(slot))).unwrap_or(None);
    let Some((name, cost, item_type, reload_seconds)) = resolved else {
        return;
    };
    match item_type.as_str() {
        "healing" => {
            bus.publish(
                Topic::ActiveHealToolChanged,
                &serde_json::json!({
                    "tool_name": name,
                    "cost_per_use_ped": cost,
                    "reload_seconds": reload_seconds,
                    "source": format!("hotbar:{slot}"),
                }),
            );
        }
        // Consumables are one-off actions: never switch the active
        // weapon in cost tracking.
        "consumable" => {}
        _ => {
            bus.publish(
                Topic::ActiveToolChanged,
                &serde_json::json!({
                    "tool_name": name,
                    "source": format!("hotbar:{slot}"),
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystroke_source::MockKeystrokeSource;
    use chrono::{DateTime, Utc};
    use serde_json::Value;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    struct Rig {
        bus: Arc<EventBus>,
        source: Arc<MockKeystrokeSource>,
        listener: Arc<HotbarListener>,
        stream: Arc<Mutex<Vec<(Topic, Value)>>>,
    }

    fn rig(resolver: Option<HotbarResolver>) -> Rig {
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |topic, data| {
            sink.lock().unwrap().push((topic, data.clone()));
        });
        let source = Arc::new(MockKeystrokeSource::new());
        let listener = HotbarListener::new(bus.clone(), Some(source.clone()), resolver);
        Rig {
            bus,
            source,
            listener,
            stream,
        }
    }

    fn standard_resolver() -> HotbarResolver {
        Arc::new(|slot: &str| match slot {
            "1" => Some(("Opalo".to_string(), 0.05, "weapon".to_string(), 0.0)),
            "2" => Some(("Healer".to_string(), 0.088, "healing".to_string(), 2.5)),
            "3" => Some(("Snack".to_string(), 0.01, "consumable".to_string(), 0.0)),
            _ => None,
        })
    }

    fn drain_resolutions(rig: &Rig) {
        // The resolve worker is asynchronous; give it a moment to
        // drain (bounded, not timing-sensitive: the queue is tiny).
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if !rig.stream.lock().unwrap().is_empty() {
                break;
            }
        }
    }

    /// Captures the message + `attached` field of each `eo::input` event,
    /// so the input breadcrumbs can be asserted in-process.
    #[derive(Default, Debug)]
    struct EventCapture {
        message: String,
        attached: Option<bool>,
    }

    impl tracing::field::Visit for EventCapture {
        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "attached" {
                self.attached = Some(value);
            }
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.message = format!("{value:?}");
            }
        }
    }

    struct CaptureLayer(Arc<Mutex<Vec<(String, EventCapture)>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut capture = EventCapture::default();
            event.record(&mut capture);
            self.0
                .lock()
                .unwrap()
                .push((event.metadata().target().to_string(), capture));
        }
    }

    #[test]
    fn the_eo_input_breadcrumbs_fire_on_attach_and_first_delivery() {
        use tracing_subscriber::layer::SubscriberExt;
        // Operators diagnose the shared keyboard hook by reading the
        // eo::input attach/first-delivery breadcrumbs from the rolling
        // logfile; this test guards that those breadcrumbs fire. Drive the
        // listener through the gate (toggle + session) and a first injected
        // keystroke, and assert the attach + first-delivery breadcrumbs
        // fire at the right points.
        let captured: Arc<Mutex<Vec<(String, EventCapture)>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let rig = rig(Some(standard_resolver()));
            rig.listener.set_hotbar_hooks_enabled(true);
            rig.bus.publish(Topic::SessionStarted, &Value::Null);
            rig.source.inject("1", now(), KeystrokeKind::Press);
            drain_resolutions(&rig);
            rig.listener.stop();
        });
        let events = captured.lock().unwrap();
        let input: Vec<&(String, EventCapture)> = events
            .iter()
            .filter(|(target, _)| target == "eo::input")
            .collect();
        assert!(
            input
                .iter()
                .any(|(_, e)| e.message.contains("start requested") && e.attached == Some(true)),
            "the attach breadcrumb fires with attached=true: {input:?}"
        );
        assert!(
            input
                .iter()
                .any(|(_, e)| e.message.contains("first keystroke since start")),
            "the first-keystroke breadcrumb fires: {input:?}"
        );
    }

    #[test]
    fn the_gate_needs_both_the_toggle_and_an_active_session() {
        let rig = rig(Some(standard_resolver()));
        assert!(!rig.listener.is_running());

        rig.listener.set_hotbar_hooks_enabled(true);
        assert!(!rig.listener.is_running(), "no session yet");

        rig.bus.publish(Topic::SessionStarted, &Value::Null);
        assert!(rig.listener.is_running(), "toggle + session = running");

        rig.bus.publish(Topic::SessionStopped, &Value::Null);
        assert!(!rig.listener.is_running());

        rig.bus.publish(Topic::SessionStarted, &Value::Null);
        rig.listener.set_hotbar_hooks_enabled(false);
        assert!(!rig.listener.is_running(), "toggle off stops the source");
        rig.listener.stop();
    }

    #[test]
    fn presses_resolve_into_the_three_outcome_branches() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus.publish(Topic::SessionStarted, &Value::Null);

        rig.source.inject("1", now(), KeystrokeKind::Press);
        drain_resolutions(&rig);
        rig.source.inject("2", now(), KeystrokeKind::Press);
        rig.source.inject("3", now(), KeystrokeKind::Press);
        rig.source.inject("9", now(), KeystrokeKind::Press);
        rig.listener.stop();

        let stream = rig.stream.lock().unwrap();
        let interesting: Vec<&(Topic, Value)> = stream
            .iter()
            .filter(|(topic, _)| {
                matches!(
                    topic,
                    Topic::ActiveToolChanged | Topic::ActiveHealToolChanged
                )
            })
            .collect();
        assert_eq!(
            interesting.len(),
            2,
            "consumable and empty slots stay quiet"
        );
        assert_eq!(interesting[0].0, Topic::ActiveToolChanged);
        assert_eq!(interesting[0].1["tool_name"], "Opalo");
        assert_eq!(interesting[0].1["source"], "hotbar:1");
        assert_eq!(interesting[1].0, Topic::ActiveHealToolChanged);
        assert_eq!(interesting[1].1["cost_per_use_ped"], 0.088);
        assert_eq!(interesting[1].1["reload_seconds"], 2.5);
    }

    #[test]
    fn filtering_drops_releases_and_non_slot_keys() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus.publish(Topic::SessionStarted, &Value::Null);

        let taps = Arc::new(Mutex::new(Vec::new()));
        let sink = taps.clone();
        rig.listener
            .set_key_tap(Arc::new(move |key: &str, kind: &str| {
                sink.lock()
                    .unwrap()
                    .push((key.to_string(), kind.to_string()));
            }));

        rig.source.inject("1", now(), KeystrokeKind::Release);
        rig.source.inject("space", now(), KeystrokeKind::Press);
        rig.source.inject("5", now(), KeystrokeKind::Press);
        rig.listener.clear_key_tap();
        rig.source.inject("6", now(), KeystrokeKind::Press);
        rig.listener.stop();

        let taps = taps.lock().unwrap();
        assert_eq!(*taps, [("5".to_string(), "press".to_string())]);
    }

    #[test]
    fn a_panicking_resolver_is_contained() {
        let resolver: HotbarResolver = Arc::new(|_| panic!("resolver down"));
        let rig = rig(Some(resolver));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus.publish(Topic::SessionStarted, &Value::Null);
        rig.source.inject("1", now(), KeystrokeKind::Press);
        std::thread::sleep(std::time::Duration::from_millis(50));
        rig.source.inject("2", now(), KeystrokeKind::Press);
        std::thread::sleep(std::time::Duration::from_millis(50));
        rig.listener.stop();
        let stream = rig.stream.lock().unwrap();
        let tool_events = stream
            .iter()
            .filter(|(topic, _)| {
                matches!(
                    topic,
                    Topic::ActiveToolChanged | Topic::ActiveHealToolChanged
                )
            })
            .count();
        assert_eq!(
            tool_events, 0,
            "failures are contained, the worker survives"
        );
    }

    #[test]
    fn stop_unsubscribes_the_session_events() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.listener.stop();
        rig.bus.publish(Topic::SessionStarted, &Value::Null);
        assert!(
            !rig.listener.is_running(),
            "a stopped listener no longer reconciles on session events"
        );
    }

    #[test]
    fn the_explicit_stop_is_what_releases_the_listener() {
        let bus = Arc::new(EventBus::new());
        let source = Arc::new(MockKeystrokeSource::new());
        let listener =
            HotbarListener::new(bus.clone(), Some(source.clone()), Some(standard_resolver()));
        // The bus subscriptions hold the listener alive through their
        // closures; scope exit alone cannot tear it down.
        assert!(bus.has_subscribers(Topic::SessionStarted));
        listener.stop();
        assert!(!bus.has_subscribers(Topic::SessionStarted));
        assert!(!bus.has_subscribers(Topic::SessionStopped));
    }
}
