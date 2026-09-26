//! Hotbar key listener: observes hotbar slot keypresses and resolves
//! them into session-scoped equipment intent on the bus.
//!
//! The listener gates its keystroke source on the capability toggle
//! and an active tracking session, observed through the bus's session
//! events. Resolution runs on one owned worker rather than a
//! short-lived thread per press; a failing resolver is contained.
//!
//! The keystroke source observes keys globally, so a slot key typed
//! into another application arrives here too. An injected focus probe
//! drops presses made while another window holds focus: they never
//! reached the game, so they must not move the tracked tool.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use crate::bus_events::{BusEvent, HotbarIntentPayload, HotbarItemKind};
use crate::eu_window::GameFocus;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::healing_profile::HealingProfile;
use crate::keystroke_source::{KeystrokeEvent, KeystrokeKind, KeystrokeSource};

/// Hotbar slot keys: the number row 1-9 and 0.
pub const HOTBAR_SLOT_KEYS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedHotbarItem {
    pub equipment_id: i64,
    pub name: String,
    pub kind: HotbarItemKind,
    pub cost_per_use_ped: f64,
    pub reload_seconds: f64,
    pub healing_profile: Option<HealingProfile>,
    pub lifesteal_percent: Option<f64>,
    pub consumable_profile: Option<crate::consumables::ConsumableProfile>,
}

/// The resolver: slot key to the current equipment snapshot, or None for an
/// empty or unreadable slot. It runs on the listener-owned worker.
pub type HotbarResolver = Arc<dyn Fn(&str) -> Option<ResolvedHotbarItem> + Send + Sync>;

/// The focus seam: where keyboard focus sits relative to the game client
/// at the moment it is asked. Production wires `eu_window::game_focus`.
pub type GameFocusProbe = Arc<dyn Fn() -> GameFocus + Send + Sync>;

/// A keystroke observer (the recording controller's seam): called with
/// (key, kind) for each hotbar-slot press.
pub type KeyTap = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct HotbarResolveRequest {
    slot: String,
    occurred_at: chrono::DateTime<chrono::Utc>,
    session_id: String,
}

struct Gate {
    hooks_enabled: AtomicBool,
    session_active: AtomicBool,
    active_session_id: Mutex<Option<String>>,
    source_running: AtomicBool,
    // One-shot per start episode: whether the "first keystroke delivered"
    // breadcrumb has been logged. Lets the rolling logfile show whether the
    // OS hook actually delivered after attaching.
    first_delivery_logged: AtomicBool,
}

pub struct HotbarListener {
    bus: Arc<EventBus>,
    source: Option<Arc<dyn KeystrokeSource>>,
    focus: Option<GameFocusProbe>,
    gate: Arc<Gate>,
    key_tap: Arc<Mutex<Option<KeyTap>>>,
    resolve_queue: Mutex<Option<Sender<HotbarResolveRequest>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    session_subscriptions: Mutex<Option<(Registration, Registration)>>,
}

impl HotbarListener {
    /// A `None` source leaves the listener inert, matching the
    /// original's missing-hook-library path. A `None` focus probe admits
    /// every press, as does a probe answering `Unknown`.
    pub fn new(
        bus: Arc<EventBus>,
        source: Option<Arc<dyn KeystrokeSource>>,
        resolver: Option<HotbarResolver>,
        focus: Option<GameFocusProbe>,
    ) -> Arc<Self> {
        let gate = Arc::new(Gate {
            hooks_enabled: AtomicBool::new(false),
            session_active: AtomicBool::new(false),
            active_session_id: Mutex::new(None),
            source_running: AtomicBool::new(false),
            first_delivery_logged: AtomicBool::new(false),
        });
        let key_tap: Arc<Mutex<Option<KeyTap>>> = Arc::new(Mutex::new(None));

        // One owned worker drains slot resolutions off the hook
        // thread (rather than a thread per press).
        let (queue, worker) = match resolver {
            None => (None, None),
            Some(resolver) => {
                let (sender, receiver) = channel::<HotbarResolveRequest>();
                let worker_bus = bus.clone();
                let worker_gate = gate.clone();
                let handle = std::thread::Builder::new()
                    .name("hotbar-resolve".into())
                    .spawn(move || {
                        while let Ok(request) = receiver.recv() {
                            resolve_hotbar_slot(&worker_bus, &worker_gate, &resolver, request);
                        }
                    })
                    .expect("resolve worker spawns");
                (Some(sender), Some(handle))
            }
        };

        let listener = Arc::new(Self {
            bus: bus.clone(),
            source: source.clone(),
            focus,
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
        let started = bus.subscribe(Topic::SessionStarted, move |event| {
            let BusEvent::SessionStarted(payload) = event else {
                return;
            };
            *started_listener
                .gate
                .active_session_id
                .lock()
                .expect("active session") = Some(payload.session_id.clone());
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
            *stopped_listener
                .gate
                .active_session_id
                .lock()
                .expect("active session") = None;
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
        *self.gate.active_session_id.lock().expect("active session") = None;
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
        if !self.gate.source_running.load(Ordering::SeqCst)
            || !self.gate.session_active.load(Ordering::SeqCst)
        {
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
        // Asked on the source's dispatch worker, never the hook thread.
        // Only a definite Unfocused drops the press: a platform that
        // cannot tell keeps admitting every press, as before the probe.
        if let Some(focus) = &self.focus {
            if focus() == GameFocus::Unfocused {
                tracing::debug!(
                    target: "eo::input",
                    "hotbar press ignored: the game window does not hold focus"
                );
                return;
            }
        }
        let tap = self.key_tap.lock().expect("key tap").clone();
        if let Some(tap) = tap {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&event.key, "press")));
        }
        let session_id = self
            .gate
            .active_session_id
            .lock()
            .expect("active session")
            .clone();
        if let (Some(queue), Some(session_id)) = (
            self.resolve_queue.lock().expect("resolve queue").as_ref(),
            session_id,
        ) {
            let _ = queue.send(HotbarResolveRequest {
                slot: event.key.clone(),
                occurred_at: event.timestamp,
                session_id,
            });
        }
    }
}

/// Resolve a slot and publish the tool change; runs on the owned
/// worker, with failures contained exactly as the original contains
/// its worker-thread errors.
fn resolve_hotbar_slot(
    bus: &EventBus,
    gate: &Gate,
    resolver: &HotbarResolver,
    request: HotbarResolveRequest,
) {
    let resolved =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolver(&request.slot)))
            .unwrap_or(None);
    let Some(item) = resolved else {
        return;
    };
    if !session_matches(gate, &request.session_id) {
        return;
    }
    let slot = request.slot;
    bus.publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
        session_id: Some(request.session_id),
        slot,
        occurred_at: request.occurred_at.timestamp_micros() as f64 / 1_000_000.0,
        equipment_id: item.equipment_id,
        item_name: item.name.clone(),
        item_kind: item.kind,
        cost_per_use_ped: item.cost_per_use_ped,
        reload_seconds: item.reload_seconds,
        healing_profile: item.healing_profile.clone(),
        lifesteal_percent: item.lifesteal_percent,
        consumable_profile: item.consumable_profile.clone(),
    })));
}

fn session_matches(gate: &Gate, session_id: &str) -> bool {
    gate.active_session_id
        .lock()
        .expect("active session")
        .as_deref()
        == Some(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus_events::SessionLifecyclePayload;
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
        rig_with_focus(resolver, None)
    }

    fn rig_with_focus(resolver: Option<HotbarResolver>, focus: Option<GameFocusProbe>) -> Rig {
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |event| {
            sink.lock()
                .unwrap()
                .push((event.topic(), event.payload_value()));
        });
        let source = Arc::new(MockKeystrokeSource::new());
        let listener = HotbarListener::new(bus.clone(), Some(source.clone()), resolver, focus);
        Rig {
            bus,
            source,
            listener,
            stream,
        }
    }

    fn standard_resolver() -> HotbarResolver {
        Arc::new(|slot: &str| match slot {
            "1" => Some(ResolvedHotbarItem {
                equipment_id: 1,
                name: "Opalo".to_string(),
                kind: HotbarItemKind::Weapon,
                cost_per_use_ped: 0.05,
                reload_seconds: 0.0,
                healing_profile: None,
                lifesteal_percent: None,
                consumable_profile: None,
            }),
            "2" => Some(ResolvedHotbarItem {
                equipment_id: 2,
                name: "Healer".to_string(),
                kind: HotbarItemKind::Healing,
                cost_per_use_ped: 0.088,
                reload_seconds: 2.5,
                healing_profile: Some(HealingProfile {
                    direct_min: Some(60.0),
                    direct_max: Some(100.0),
                    ..HealingProfile::default()
                }),
                lifesteal_percent: None,
                consumable_profile: None,
            }),
            "3" => Some(ResolvedHotbarItem {
                equipment_id: 3,
                name: "Snack".to_string(),
                kind: HotbarItemKind::Consumable,
                cost_per_use_ped: 0.01,
                reload_seconds: 0.0,
                healing_profile: None,
                lifesteal_percent: None,
                consumable_profile: None,
            }),
            _ => None,
        })
    }

    fn wait_for_intents(rig: &Rig, expected: usize) {
        // The resolve worker is asynchronous. Wait for the observable
        // publication boundary rather than assuming the queue's speed.
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let count = rig
                .stream
                .lock()
                .unwrap()
                .iter()
                .filter(|(topic, _)| *topic == Topic::HotbarIntent)
                .count();
            if count >= expected {
                return;
            }
        }
        panic!("timed out waiting for {expected} hotbar intents");
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
            rig.bus
                .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                    session_id: "s1".into(),
                }));
            rig.source.inject("1", now(), KeystrokeKind::Press);
            wait_for_intents(&rig, 1);
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

        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        assert!(rig.listener.is_running(), "toggle + session = running");

        rig.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        assert!(!rig.listener.is_running());

        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.listener.set_hotbar_hooks_enabled(false);
        assert!(!rig.listener.is_running(), "toggle off stops the source");
        rig.listener.stop();
    }

    #[test]
    fn presses_resolve_into_session_scoped_intents() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));

        rig.source.inject("1", now(), KeystrokeKind::Press);
        wait_for_intents(&rig, 1);
        rig.source.inject("2", now(), KeystrokeKind::Press);
        rig.source.inject("3", now(), KeystrokeKind::Press);
        rig.source.inject("9", now(), KeystrokeKind::Press);
        wait_for_intents(&rig, 3);
        rig.listener.stop();

        let stream = rig.stream.lock().unwrap();
        let intents: Vec<&Value> = stream
            .iter()
            .filter(|(topic, _)| *topic == Topic::HotbarIntent)
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(intents.len(), 3, "every resolved press carries intent");
        assert_eq!(intents[0]["session_id"], "s1");
        assert_eq!(intents[0]["item_name"], "Opalo");
        assert_eq!(intents[0]["item_kind"], "weapon");
        assert_eq!(intents[0]["occurred_at"], 1_779_184_800.0);
        assert_eq!(intents[1]["equipment_id"], 2);
        assert_eq!(intents[1]["cost_per_use_ped"], 0.088);
        assert_eq!(intents[1]["reload_seconds"], 2.5);
        assert_eq!(intents[1]["healing_profile"]["direct_min"], 60.0);
        assert_eq!(intents[2]["item_kind"], "consumable");
    }

    #[test]
    fn filtering_drops_releases_and_non_slot_keys() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));

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
    fn presses_made_while_another_window_holds_focus_are_dropped() {
        let focus = Arc::new(Mutex::new(GameFocus::Unfocused));
        let probe_focus = focus.clone();
        let probe: GameFocusProbe = Arc::new(move || *probe_focus.lock().unwrap());
        let rig = rig_with_focus(Some(standard_resolver()), Some(probe));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));

        let taps = Arc::new(Mutex::new(Vec::new()));
        let sink = taps.clone();
        rig.listener
            .set_key_tap(Arc::new(move |key: &str, _kind: &str| {
                sink.lock().unwrap().push(key.to_string());
            }));

        rig.source.inject("1", now(), KeystrokeKind::Press);
        *focus.lock().unwrap() = GameFocus::Focused;
        rig.source.inject("2", now(), KeystrokeKind::Press);
        *focus.lock().unwrap() = GameFocus::Unknown;
        rig.source.inject("3", now(), KeystrokeKind::Press);
        wait_for_intents(&rig, 2);
        rig.listener.stop();

        assert_eq!(
            *taps.lock().unwrap(),
            ["2", "3"],
            "the unfocused press never reaches the key tap"
        );
        // The resolve queue is FIFO, so an admitted "1" would have
        // published ahead of "2".
        let stream = rig.stream.lock().unwrap();
        let slots: Vec<&Value> = stream
            .iter()
            .filter(|(topic, _)| *topic == Topic::HotbarIntent)
            .map(|(_, payload)| &payload["slot"])
            .collect();
        assert_eq!(
            slots,
            ["2", "3"],
            "focused and unknown presses both resolve"
        );
    }

    #[test]
    fn a_panicking_resolver_is_contained() {
        let resolver: HotbarResolver = Arc::new(|_| panic!("resolver down"));
        let rig = rig(Some(resolver));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.source.inject("1", now(), KeystrokeKind::Press);
        std::thread::sleep(std::time::Duration::from_millis(50));
        rig.source.inject("2", now(), KeystrokeKind::Press);
        std::thread::sleep(std::time::Duration::from_millis(50));
        rig.listener.stop();
        let stream = rig.stream.lock().unwrap();
        let intents = stream
            .iter()
            .filter(|(topic, _)| *topic == Topic::HotbarIntent)
            .count();
        assert_eq!(intents, 0, "failures are contained, the worker survives");
    }

    #[test]
    fn a_resolution_queued_by_an_old_session_cannot_reach_the_next_session() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (drained_tx, drained_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let invocation = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolver: HotbarResolver = Arc::new(move |_| {
            if invocation.fetch_add(1, Ordering::SeqCst) != 0 {
                drained_tx.send(()).unwrap();
                return None;
            }
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Some(ResolvedHotbarItem {
                equipment_id: 1,
                name: "Opalo".to_string(),
                kind: HotbarItemKind::Weapon,
                cost_per_use_ped: 0.05,
                reload_seconds: 0.0,
                healing_profile: None,
                lifesteal_percent: None,
                consumable_profile: None,
            })
        });
        let rig = rig(Some(resolver));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.source.inject("1", now(), KeystrokeKind::Press);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        rig.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s2".into(),
            }));
        release_tx.send(()).unwrap();
        rig.source.inject("9", now(), KeystrokeKind::Press);
        drained_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        assert_eq!(
            rig.stream
                .lock()
                .unwrap()
                .iter()
                .filter(|(topic, _)| *topic == Topic::HotbarIntent)
                .count(),
            0
        );
        rig.listener.stop();
    }

    #[test]
    fn hotbar_publication_allows_reentrant_session_lifecycle_events() {
        let rig = rig(Some(standard_resolver()));
        let reentrant_bus = rig.bus.clone();
        let _registration = rig.bus.subscribe(Topic::HotbarIntent, move |_| {
            reentrant_bus.publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
            reentrant_bus.publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s2".into(),
            }));
        });
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));

        rig.source.inject("1", now(), KeystrokeKind::Press);
        wait_for_intents(&rig, 1);

        assert!(rig.listener.is_running());
        rig.listener.stop();
    }

    #[test]
    fn stop_unsubscribes_the_session_events() {
        let rig = rig(Some(standard_resolver()));
        rig.listener.set_hotbar_hooks_enabled(true);
        rig.listener.stop();
        rig.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        assert!(
            !rig.listener.is_running(),
            "a stopped listener no longer reconciles on session events"
        );
    }

    #[test]
    fn the_explicit_stop_is_what_releases_the_listener() {
        let bus = Arc::new(EventBus::new());
        let source = Arc::new(MockKeystrokeSource::new());
        let listener = HotbarListener::new(
            bus.clone(),
            Some(source.clone()),
            Some(standard_resolver()),
            None,
        );
        // The bus subscriptions hold the listener alive through their
        // closures; scope exit alone cannot tear it down.
        assert!(bus.has_subscribers(Topic::SessionStarted));
        listener.stop();
        assert!(!bus.has_subscribers(Topic::SessionStarted));
        assert!(!bus.has_subscribers(Topic::SessionStopped));
    }
}
