//! Frontend-facing domain events: the typed wire contract.
//!
//! Mirrors the original Python implementation exactly. The wire is closed in
//! both directions (unknown keys are rejected, mirroring the Python
//! models' forbidden extras), payload keys are camelCase, `occurred_at`
//! is a required ISO-8601 UTC string, and serialisation emits fields in
//! the envelope's declaration order (`type`, `event_version`,
//! `occurred_at`, `payload`) so the JSON bytes match the Python
//! `model_dump_json()` output for the same envelope. The committed
//! `event_schemas.snapshot.json` is asserted against these types by the
//! schema-conformance test; the value-level wire vectors are pinned in
//! this module's tests.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const TOPIC_TRACKING_SESSION_UPDATED: &str = "tracking.session.updated";
pub const TOPIC_SCAN_STATUS_CHANGED: &str = "scan.status.changed";

/// A field that serialises to exactly one topic literal and refuses any
/// other input: the discriminator the union routes on, kept closed so a
/// mistagged frame fails loudly instead of coercing.
macro_rules! topic_tag {
    ($name:ident, $literal:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($literal)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                if raw == $literal {
                    Ok(Self)
                } else {
                    Err(D::Error::custom(concat!("expected \"", $literal, "\"")))
                }
            }
        }
    };
}

topic_tag!(TrackingSessionUpdatedTag, "tracking.session.updated");
topic_tag!(ScanStatusChangedTag, "scan.status.changed");

fn default_event_version() -> i64 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackingStatus {
    Active,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackingReason {
    Started,
    Updated,
    Stopped,
}

/// Push-to-pull invalidation signal for the live tracking session: which
/// session, the coarse state, and why it fired. Subscribers re-hydrate
/// the full readout from the snapshot GET.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingSessionUpdatedPayload {
    /// Serialised as `null` when absent, exactly as the Python model
    /// dumps its `None` default.
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    pub status: TrackingStatus,
    pub reason: TrackingReason,
}

/// The session aggregates changed (started, advanced a tick, or stopped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingSessionUpdated {
    #[serde(rename = "type")]
    pub topic: TrackingSessionUpdatedTag,
    #[serde(default = "default_event_version")]
    pub event_version: i64,
    pub occurred_at: String,
    pub payload: TrackingSessionUpdatedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    Idle,
    Capturing,
    Processing,
    AwaitingReview,
}

/// Push-to-pull invalidation signal for the manual skill-scan flow: the
/// coarse phase only; the full status comes from the scan-status GET.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanStatusChangedPayload {
    pub phase: ScanPhase,
}

/// The manual skill-scan status changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanStatusChanged {
    #[serde(rename = "type")]
    pub topic: ScanStatusChangedTag,
    #[serde(default = "default_event_version")]
    pub event_version: i64,
    pub occurred_at: String,
    pub payload: ScanStatusChangedPayload,
}

/// The discriminated union of every frontend-facing domain event. The
/// untagged dispatch is made exact by the closed topic-tag fields: a
/// frame routes to the one variant whose `type` literal it carries, and
/// a missing or unrecognised `type` fails outright, mirroring the Python
/// discriminated union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DomainEvent {
    TrackingSessionUpdated(TrackingSessionUpdated),
    ScanStatusChanged(ScanStatusChanged),
}

impl DomainEvent {
    /// The bus/SSE topic string, identical to the envelope's `type` tag.
    pub fn topic(&self) -> &'static str {
        match self {
            DomainEvent::TrackingSessionUpdated(_) => TOPIC_TRACKING_SESSION_UPDATED,
            DomainEvent::ScanStatusChanged(_) => TOPIC_SCAN_STATUS_CHANGED,
        }
    }

    /// The compact wire JSON, byte-identical to the Python
    /// `model_dump_json()` for the same envelope.
    pub fn to_wire_json(&self) -> String {
        serde_json::to_string(self).expect("domain events always serialise")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking_sample() -> DomainEvent {
        DomainEvent::TrackingSessionUpdated(TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: TrackingSessionUpdatedPayload {
                session_id: Some("session-abc".into()),
                status: TrackingStatus::Active,
                reason: TrackingReason::Started,
            },
        })
    }

    #[test]
    fn tracking_envelope_serialises_to_the_pinned_wire_bytes() {
        // The exact vector the Python value-level wire test pins.
        let expected = concat!(
            "{\"type\":\"tracking.session.updated\",\"event_version\":1,",
            "\"occurred_at\":\"2024-12-31T21:20:00+00:00\",",
            "\"payload\":{\"sessionId\":\"session-abc\",\"status\":\"active\",",
            "\"reason\":\"started\"}}"
        );
        assert_eq!(tracking_sample().to_wire_json(), expected);
    }

    #[test]
    fn scan_envelope_serialises_to_the_pinned_wire_bytes() {
        let event = DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Processing,
            },
        });
        let expected = concat!(
            "{\"type\":\"scan.status.changed\",\"event_version\":1,",
            "\"occurred_at\":\"2024-12-31T21:20:00+00:00\",",
            "\"payload\":{\"phase\":\"processing\"}}"
        );
        assert_eq!(event.to_wire_json(), expected);
    }

    #[test]
    fn union_routes_on_the_type_tag_and_round_trips() {
        let wire = tracking_sample().to_wire_json();
        let restored: DomainEvent = serde_json::from_str(&wire).unwrap();
        assert_eq!(restored, tracking_sample());
        assert_eq!(restored.topic(), TOPIC_TRACKING_SESSION_UPDATED);

        let scan_wire = concat!(
            "{\"type\":\"scan.status.changed\",\"event_version\":1,",
            "\"occurred_at\":\"2024-12-31T21:20:00+00:00\",",
            "\"payload\":{\"phase\":\"awaiting_review\"}}"
        );
        let restored: DomainEvent = serde_json::from_str(scan_wire).unwrap();
        assert_eq!(restored.topic(), TOPIC_SCAN_STATUS_CHANGED);
    }

    #[test]
    fn null_session_id_serialises_as_null_not_omitted() {
        let event = DomainEvent::TrackingSessionUpdated(TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: TrackingSessionUpdatedPayload {
                session_id: None,
                status: TrackingStatus::Idle,
                reason: TrackingReason::Stopped,
            },
        });
        assert!(event.to_wire_json().contains("\"sessionId\":null"));
    }

    #[test]
    fn closed_wire_rejects_unknown_keys_missing_tag_and_foreign_tag() {
        let extra_payload_key = concat!(
            "{\"type\":\"scan.status.changed\",\"event_version\":1,",
            "\"occurred_at\":\"t\",\"payload\":{\"phase\":\"idle\",\"x\":1}}"
        );
        assert!(serde_json::from_str::<DomainEvent>(extra_payload_key).is_err());

        let extra_envelope_key = concat!(
            "{\"type\":\"scan.status.changed\",\"event_version\":1,",
            "\"occurred_at\":\"t\",\"payload\":{\"phase\":\"idle\"},\"x\":1}"
        );
        assert!(serde_json::from_str::<DomainEvent>(extra_envelope_key).is_err());

        let missing_tag =
            "{\"event_version\":1,\"occurred_at\":\"t\",\"payload\":{\"phase\":\"idle\"}}";
        assert!(serde_json::from_str::<DomainEvent>(missing_tag).is_err());

        let foreign_tag = concat!(
            "{\"type\":\"quest.updated\",\"event_version\":1,",
            "\"occurred_at\":\"t\",\"payload\":{\"phase\":\"idle\"}}"
        );
        assert!(serde_json::from_str::<DomainEvent>(foreign_tag).is_err());
    }

    #[test]
    fn event_version_defaults_to_one_on_input() {
        let no_version = concat!(
            "{\"type\":\"scan.status.changed\",",
            "\"occurred_at\":\"t\",\"payload\":{\"phase\":\"idle\"}}"
        );
        let restored: DomainEvent = serde_json::from_str(no_version).unwrap();
        match restored {
            DomainEvent::ScanStatusChanged(envelope) => assert_eq!(envelope.event_version, 1),
            other => panic!("routed to the wrong variant: {other:?}"),
        }
    }
}
