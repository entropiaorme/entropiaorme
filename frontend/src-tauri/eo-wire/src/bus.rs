//! The monomorphic domain-event channel.
//!
//! The deliberate channel-shape decision for the native backend: typed
//! [`DomainEvent`] envelopes travel on a dedicated broadcast channel, so
//! "a typed event on a domain topic" is a compiler-checked invariant on
//! the producer side rather than a convention (the Python bus carries
//! both typed domain envelopes and loose intra-backend payloads on one
//! `Any` surface; the low-level intra-service events stay per-service
//! here and port with their services).
//!
//! Taps mirror the Python bus's full-stream observer affordance: they run
//! synchronously on the publishing thread, before subscriber delivery,
//! which is what the replay event-recorder relies on for deterministic
//! capture order.

use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;

use crate::domain_events::DomainEvent;

/// Full-stream observer: sees every published envelope, in publish order,
/// on the publishing thread, before subscribers.
pub type Tap = Arc<dyn Fn(&DomainEvent) + Send + Sync>;

pub struct DomainBus {
    sender: broadcast::Sender<DomainEvent>,
    taps: RwLock<Vec<Tap>>,
}

impl DomainBus {
    /// `capacity` bounds each subscriber's backlog; a subscriber that
    /// falls further behind observes a lag error from its receiver rather
    /// than stalling publishers (consumer-side delivery shaping, e.g. the
    /// event stream's drop-oldest queues, sits downstream of this).
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            taps: RwLock::new(Vec::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.sender.subscribe()
    }

    pub fn add_tap(&self, tap: Tap) {
        self.taps
            .write()
            .expect("tap registry lock never poisoned")
            .push(tap);
    }

    /// Publish a typed envelope: taps first (synchronously, in
    /// registration order), then broadcast delivery. Returns the number
    /// of subscribers the envelope reached.
    ///
    /// A panicking tap is isolated per invocation: the observer
    /// affordance must never take the bus down, and the read guard must
    /// not poison the registry for every later publish (the Python bus
    /// likewise survives a raising tap to its next publish).
    pub fn publish(&self, event: DomainEvent) -> usize {
        for tap in self
            .taps
            .read()
            .expect("tap registry lock never poisoned")
            .iter()
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&event)));
        }
        self.sender.send(event).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain_events::{
        ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
    };

    fn sample(phase: ScanPhase) -> DomainEvent {
        DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "t".into(),
            payload: ScanStatusChangedPayload { phase },
        })
    }

    #[tokio::test]
    async fn subscribers_receive_typed_envelopes_in_order() {
        let bus = DomainBus::new(16);
        let mut rx = bus.subscribe();
        assert_eq!(bus.publish(sample(ScanPhase::Capturing)), 1);
        assert_eq!(bus.publish(sample(ScanPhase::Processing)), 1);
        assert_eq!(rx.recv().await.unwrap(), sample(ScanPhase::Capturing));
        assert_eq!(rx.recv().await.unwrap(), sample(ScanPhase::Processing));
    }

    #[tokio::test]
    async fn a_panicking_tap_neither_kills_the_bus_nor_blocks_delivery() {
        let bus = DomainBus::new(16);
        bus.add_tap(Arc::new(|_| panic!("misbehaving tap")));
        let seen: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let counter = seen.clone();
        bus.add_tap(Arc::new(move |_| {
            *counter.lock().unwrap() += 1;
        }));
        let mut rx = bus.subscribe();

        // The panicking tap is isolated: later taps still run, the
        // subscriber still receives, and the NEXT publish still works.
        assert_eq!(bus.publish(sample(ScanPhase::Idle)), 1);
        assert_eq!(bus.publish(sample(ScanPhase::Capturing)), 1);
        assert_eq!(*seen.lock().unwrap(), 2);
        assert_eq!(rx.recv().await.unwrap(), sample(ScanPhase::Idle));
        assert_eq!(rx.recv().await.unwrap(), sample(ScanPhase::Capturing));
    }

    #[tokio::test]
    async fn taps_see_every_publish_even_with_no_subscribers() {
        let bus = DomainBus::new(16);
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        bus.add_tap(Arc::new(move |event| {
            record.lock().unwrap().push(event.topic().to_string());
        }));
        assert_eq!(bus.publish(sample(ScanPhase::Idle)), 0);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["scan.status.changed".to_string()]
        );
    }
}
