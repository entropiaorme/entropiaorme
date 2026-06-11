//! The native event-stream HTTP handler.
//!
//! Drains an [`eo_wire::sse::SseHub`] client into a `text/event-stream`
//! response, reproducing the Python endpoint's transport behaviour: the
//! `: ready` comment flushes immediately on connect (the relay's signal
//! that registration is complete), every hub frame streams as it
//! arrives, and a `: keep-alive` comment goes out after 15 seconds of
//! silence. Response headers mirror the Python endpoint's set.
//!
//! NOT yet routed: the live `/api/events` stays reverse-proxied to the
//! Python backend for the whole hybrid and moves natively last, when the
//! HTTP surface collapses. This handler exists now so the transport
//! semantics are pinned by tests alongside the hub they drain.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use eo_wire::sse::SseHub;
use tokio_stream::wrappers::ReceiverStream;

pub const KEEPALIVE: Duration = Duration::from_secs(15);

/// Build the streaming response for one event-stream subscriber.
pub fn event_stream_response(hub: Arc<SseHub>) -> Response {
    let client = hub.register();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
    tokio::spawn(async move {
        if tx.send(Ok(": ready\n\n".to_string())).await.is_err() {
            return;
        }
        loop {
            let frame = match tokio::time::timeout(KEEPALIVE, client.next_frame()).await {
                Ok(frame) => frame,
                Err(_) => ": keep-alive\n\n".to_string(),
            };
            if tx.send(Ok(frame)).await.is_err() {
                // Subscriber went away; dropping the client unregisters it.
                return;
            }
        }
    });
    Response::builder()
        .status(http::StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            "text/event-stream; charset=utf-8",
        )
        .header(http::header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .expect("static event-stream response builds")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eo_wire::domain_events::{
        DomainEvent, ScanPhase, ScanStatusChanged, ScanStatusChangedPayload, ScanStatusChangedTag,
    };
    use http_body_util::BodyExt;

    fn sample() -> DomainEvent {
        DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "t".into(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Capturing,
            },
        })
    }

    async fn next_data(body: &mut Body) -> String {
        loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        return String::from_utf8(data.to_vec()).unwrap();
                    }
                }
                other => panic!("stream ended unexpectedly: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn ready_comment_flushes_first_then_frames_stream() {
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        let response = event_stream_response(hub.clone());
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
        assert_eq!(response.headers().get("x-accel-buffering").unwrap(), "no");
        let mut body = response.into_body();
        assert_eq!(next_data(&mut body).await, ": ready\n\n");

        hub.dispatch(&sample());
        let frame = next_data(&mut body).await;
        assert!(frame.starts_with("id: 1\nevent: scan.status.changed\n"));
    }

    #[tokio::test(start_paused = true)]
    async fn silence_produces_a_keep_alive_comment_at_the_cadence() {
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        let response = event_stream_response(hub.clone());
        let mut body = response.into_body();
        assert_eq!(next_data(&mut body).await, ": ready\n\n");

        // No dispatches: the paused clock advances through the cadence and
        // the keep-alive comment arrives instead of a frame.
        let keep_alive = next_data(&mut body).await;
        assert_eq!(keep_alive, ": keep-alive\n\n");

        // A frame dispatched after a keep-alive still streams promptly.
        hub.dispatch(&sample());
        let frame = next_data(&mut body).await;
        assert!(frame.contains("\"phase\":\"capturing\""));
    }

    #[tokio::test]
    async fn dropping_the_response_unregisters_the_client() {
        let hub = Arc::new(SseHub::new(eo_wire::sse::DEFAULT_MAX_QUEUE));
        let response = event_stream_response(hub.clone());
        assert_eq!(hub.client_count(), 1);
        drop(response);
        // The pump task notices the closed receiver on its next send.
        hub.dispatch(&sample());
        tokio::time::sleep(Duration::from_millis(50)).await;
        hub.dispatch(&sample());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hub.client_count(), 0);
    }
}
