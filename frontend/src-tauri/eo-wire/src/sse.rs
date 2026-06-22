//! The event-stream hub: fan-out of domain events to per-client frame
//! queues, mirroring `backend/services/event_stream.py`.
//!
//! Semantics reproduced exactly:
//! - one bounded queue per connected client (default 256 frames),
//!   DROP-OLDEST on overflow: under push-to-pull the newest frame
//!   triggers the freshest hydration, so it is the oldest that is safe
//!   to shed;
//! - a process-monotonic sequence number assigned at dispatch, shared by
//!   every client's copy of a frame;
//! - the frame format `id: N\nevent: <topic>\ndata: <json>\n\n`, with
//!   the envelope JSON in its compact wire form (byte-identical to the
//!   Python `model_dump_json()`).
//!
//! The 15-second keep-alive and the `: ready` opening comment are
//! transport-loop concerns and live with the HTTP handler that drains a
//! client; this hub owns the fan-out behind it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::domain_events::DomainEvent;

pub const DEFAULT_MAX_QUEUE: usize = 256;

/// A client's bounded frame queue: drop-oldest, await-on-empty.
struct ClientQueue {
    frames: Mutex<VecDeque<String>>,
    notify: Notify,
    max: usize,
}

impl ClientQueue {
    fn offer(&self, frame: String) {
        let mut frames = self.frames.lock().expect("frame queue lock never poisoned");
        if frames.len() == self.max {
            frames.pop_front();
        }
        frames.push_back(frame);
        drop(frames);
        self.notify.notify_one();
    }

    async fn pop(&self) -> String {
        loop {
            let notified = self.notify.notified();
            if let Some(frame) = self
                .frames
                .lock()
                .expect("frame queue lock never poisoned")
                .pop_front()
            {
                return frame;
            }
            notified.await;
        }
    }
}

struct HubState {
    seq: u64,
    clients: Vec<(u64, Arc<ClientQueue>)>,
    next_client_id: u64,
}

/// The fan-out registry. Clone-cheap via [`Arc`]; the dispatch side is
/// driven by whoever subscribes it to the domain bus.
pub struct SseHub {
    state: Mutex<HubState>,
    max_queue: usize,
}

impl SseHub {
    pub fn new(max_queue: usize) -> Self {
        Self {
            state: Mutex::new(HubState {
                seq: 0,
                clients: Vec::new(),
                next_client_id: 0,
            }),
            max_queue,
        }
    }

    /// Register a client; the handle unregisters on drop.
    pub fn register(self: &Arc<Self>) -> SseClient {
        let queue = Arc::new(ClientQueue {
            frames: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            max: self.max_queue,
        });
        let mut state = self.state.lock().expect("hub state lock never poisoned");
        let id = state.next_client_id;
        state.next_client_id += 1;
        state.clients.push((id, queue.clone()));
        SseClient {
            id,
            queue,
            hub: self.clone(),
        }
    }

    /// Assign the next sequence number and fan the frame out to every
    /// connected client.
    pub fn dispatch(&self, event: &DomainEvent) {
        let frame = {
            let mut state = self.state.lock().expect("hub state lock never poisoned");
            state.seq += 1;
            let frame = format!(
                "id: {}\nevent: {}\ndata: {}\n\n",
                state.seq,
                event.topic(),
                event.to_wire_json()
            );
            for (_, queue) in &state.clients {
                queue.offer(frame.clone());
            }
            frame
        };
        // The frame is built and fanned out under one lock acquisition so
        // sequence numbers and delivery order agree across clients.
        let _ = frame;
    }

    pub fn client_count(&self) -> usize {
        self.state
            .lock()
            .expect("hub state lock never poisoned")
            .clients
            .len()
    }

    fn unregister(&self, id: u64) {
        self.state
            .lock()
            .expect("hub state lock never poisoned")
            .clients
            .retain(|(client_id, _)| *client_id != id);
    }
}

/// A connected client's receive handle.
pub struct SseClient {
    id: u64,
    queue: Arc<ClientQueue>,
    hub: Arc<SseHub>,
}

impl SseClient {
    /// The next frame, awaiting if the queue is empty.
    pub async fn next_frame(&self) -> String {
        self.queue.pop().await
    }
}

impl Drop for SseClient {
    fn drop(&mut self) {
        self.hub.unregister(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain_events::{
        ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
    };

    fn sample(phase: ScanPhase) -> DomainEvent {
        DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: ScanStatusChangedPayload { phase },
        })
    }

    #[tokio::test]
    async fn frames_carry_monotonic_ids_topic_and_compact_json() {
        let hub = Arc::new(SseHub::new(DEFAULT_MAX_QUEUE));
        let client = hub.register();
        hub.dispatch(&sample(ScanPhase::Capturing));
        hub.dispatch(&sample(ScanPhase::Processing));

        let first = client.next_frame().await;
        assert_eq!(
            first,
            concat!(
                "id: 1\nevent: scan.status.changed\ndata: ",
                "{\"type\":\"scan.status.changed\",\"event_version\":1,",
                "\"occurred_at\":\"2024-12-31T21:20:00+00:00\",",
                "\"payload\":{\"phase\":\"capturing\"}}\n\n"
            )
        );
        let second = client.next_frame().await;
        assert!(second.starts_with("id: 2\n"));
    }

    #[tokio::test]
    async fn overflow_drops_the_oldest_frame_not_the_newest() {
        let hub = Arc::new(SseHub::new(2));
        let client = hub.register();
        hub.dispatch(&sample(ScanPhase::Idle)); // id 1, dropped
        hub.dispatch(&sample(ScanPhase::Capturing)); // id 2
        hub.dispatch(&sample(ScanPhase::Processing)); // id 3

        assert!(client.next_frame().await.starts_with("id: 2\n"));
        assert!(client.next_frame().await.starts_with("id: 3\n"));
    }

    #[tokio::test]
    async fn sequence_is_shared_across_clients_and_drop_unregisters() {
        let hub = Arc::new(SseHub::new(DEFAULT_MAX_QUEUE));
        let first = hub.register();
        let second = hub.register();
        assert_eq!(hub.client_count(), 2);

        hub.dispatch(&sample(ScanPhase::Idle));
        assert!(first.next_frame().await.starts_with("id: 1\n"));
        assert!(second.next_frame().await.starts_with("id: 1\n"));

        drop(second);
        assert_eq!(hub.client_count(), 1);
        hub.dispatch(&sample(ScanPhase::Capturing));
        assert!(first.next_frame().await.starts_with("id: 2\n"));
    }

    #[tokio::test]
    async fn next_frame_awaits_a_dispatch_that_arrives_later() {
        let hub = Arc::new(SseHub::new(DEFAULT_MAX_QUEUE));
        let client = hub.register();
        let hub_for_task = hub.clone();
        let waiter = tokio::spawn(async move { client.next_frame().await });
        tokio::task::yield_now().await;
        hub_for_task.dispatch(&sample(ScanPhase::AwaitingReview));
        let frame = waiter.await.unwrap();
        assert!(frame.contains("awaiting_review"));
    }
}
