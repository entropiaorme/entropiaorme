//! Schema-conformance gate: the native domain-event union against the
//! committed `event_schemas.snapshot.json`.
//!
//! The snapshot is the ratification-governed source of truth for the
//! frontend-facing event contract. This test parses it and asserts the
//! native union's structure exhaustively: the discriminator mapping is
//! exactly the set of native variants, and each envelope and payload
//! definition (properties, required sets, enum values, nullability,
//! closed-world `additionalProperties`) matches what the native types
//! serialise and accept. Schema drift on either side fails this gate;
//! the value-level wire bytes are pinned separately in the
//! `domain_events` unit tests.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn snapshot() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../backend/tests/expected/event_schemas.snapshot.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("committed snapshot unreadable at {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("committed snapshot parses as JSON")
}

fn property_names(definition: &Value) -> BTreeSet<String> {
    definition["properties"]
        .as_object()
        .expect("definition has properties")
        .keys()
        .cloned()
        .collect()
}

fn required_set(definition: &Value) -> BTreeSet<String> {
    definition["required"]
        .as_array()
        .expect("definition has a required list")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn enum_values(definition: &Value, property: &str) -> Vec<String> {
    definition["properties"][property]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{property} carries an enum"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// The native union's variants, as (topic, envelope def, payload def).
const VARIANTS: [(&str, &str, &str); 2] = [
    (
        "tracking.session.updated",
        "TrackingSessionUpdated",
        "TrackingSessionUpdatedPayload",
    ),
    (
        "scan.status.changed",
        "ScanStatusChanged",
        "ScanStatusChangedPayload",
    ),
];

#[test]
fn discriminator_mapping_is_exactly_the_native_variant_set() {
    let doc = snapshot();
    assert_eq!(doc["discriminator"]["propertyName"], "type");

    let mapping = doc["discriminator"]["mapping"]
        .as_object()
        .expect("discriminator mapping present");
    let mapped: BTreeSet<&str> = mapping.keys().map(String::as_str).collect();
    let native: BTreeSet<&str> = VARIANTS.iter().map(|(topic, _, _)| *topic).collect();
    assert_eq!(
        mapped, native,
        "the union's variant set diverged from the snapshot's discriminator mapping"
    );
    for (topic, envelope, _) in VARIANTS {
        assert_eq!(
            mapping[topic],
            format!("#/$defs/{envelope}"),
            "{topic} maps to an unexpected definition"
        );
    }
    assert_eq!(
        doc["oneOf"].as_array().map(Vec::len),
        Some(VARIANTS.len()),
        "the snapshot's oneOf arm count diverged from the native union"
    );
}

#[test]
fn envelope_definitions_match_the_native_shape() {
    let doc = snapshot();
    for (topic, envelope, payload) in VARIANTS {
        let def = &doc["$defs"][envelope];
        assert_eq!(
            def["additionalProperties"], false,
            "{envelope}: the wire envelope is closed"
        );
        assert_eq!(
            property_names(def),
            ["event_version", "occurred_at", "payload", "type"]
                .into_iter()
                .map(String::from)
                .collect::<BTreeSet<_>>(),
            "{envelope}: property set diverged"
        );
        assert_eq!(
            required_set(def),
            ["occurred_at", "payload"]
                .into_iter()
                .map(String::from)
                .collect::<BTreeSet<_>>(),
            "{envelope}: required set diverged (type and event_version carry defaults)"
        );
        assert_eq!(def["properties"]["type"]["const"], topic);
        assert_eq!(def["properties"]["event_version"]["default"], 1);
        assert_eq!(def["properties"]["event_version"]["type"], "integer");
        assert_eq!(def["properties"]["occurred_at"]["type"], "string");
        assert_eq!(
            def["properties"]["payload"]["$ref"],
            format!("#/$defs/{payload}")
        );
    }
}

#[test]
fn tracking_payload_definition_matches_the_native_type() {
    let doc = snapshot();
    let def = &doc["$defs"]["TrackingSessionUpdatedPayload"];
    assert_eq!(def["additionalProperties"], false);
    assert_eq!(
        property_names(def),
        ["reason", "sessionId", "status"]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        required_set(def),
        ["reason", "status"]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(enum_values(def, "status"), ["active", "idle"]);
    assert_eq!(
        enum_values(def, "reason"),
        ["started", "updated", "stopped"]
    );
    // sessionId: nullable string, defaulting null, mirroring Option<String>.
    let session_id = &def["properties"]["sessionId"];
    let arms: Vec<&str> = session_id["anyOf"]
        .as_array()
        .expect("sessionId is an anyOf")
        .iter()
        .map(|arm| arm["type"].as_str().unwrap())
        .collect();
    assert_eq!(arms, ["string", "null"]);
    assert_eq!(session_id["default"], Value::Null);
}

#[test]
fn scan_payload_definition_matches_the_native_type() {
    let doc = snapshot();
    let def = &doc["$defs"]["ScanStatusChangedPayload"];
    assert_eq!(def["additionalProperties"], false);
    assert_eq!(
        property_names(def),
        ["phase"].into_iter().map(String::from).collect()
    );
    assert_eq!(
        required_set(def),
        ["phase"].into_iter().map(String::from).collect()
    );
    assert_eq!(
        enum_values(def, "phase"),
        ["idle", "capturing", "processing", "awaiting_review"]
    );
}

/// Every enum value the snapshot names deserialises into the native type,
/// tying the assertions above to the actual serde implementations.
#[test]
fn snapshot_enum_values_round_trip_through_the_native_union() {
    let doc = snapshot();
    for status in enum_values(&doc["$defs"]["TrackingSessionUpdatedPayload"], "status") {
        for reason in enum_values(&doc["$defs"]["TrackingSessionUpdatedPayload"], "reason") {
            let wire = format!(
                concat!(
                    "{{\"type\":\"tracking.session.updated\",\"event_version\":1,",
                    "\"occurred_at\":\"t\",\"payload\":{{\"sessionId\":null,",
                    "\"status\":\"{}\",\"reason\":\"{}\"}}}}"
                ),
                status, reason
            );
            let event: eo_wire::domain_events::DomainEvent = serde_json::from_str(&wire)
                .unwrap_or_else(|e| panic!("status={status} reason={reason}: {e}"));
            assert_eq!(event.to_wire_json(), wire);
        }
    }
    for phase in enum_values(&doc["$defs"]["ScanStatusChangedPayload"], "phase") {
        let wire = format!(
            concat!(
                "{{\"type\":\"scan.status.changed\",\"event_version\":1,",
                "\"occurred_at\":\"t\",\"payload\":{{\"phase\":\"{}\"}}}}"
            ),
            phase
        );
        let event: eo_wire::domain_events::DomainEvent =
            serde_json::from_str(&wire).unwrap_or_else(|e| panic!("phase={phase}: {e}"));
        assert_eq!(event.to_wire_json(), wire);
    }
}
