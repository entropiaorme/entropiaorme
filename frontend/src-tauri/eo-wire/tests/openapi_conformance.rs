//! OpenAPI-conformance gate: every registered native response model
//! against its component in the committed `openapi.snapshot.json`.
//!
//! The snapshot is the ratification-governed contract document. For each
//! native model's declared contract this test asserts, against the
//! component of the same name: the property set matches exactly, the
//! required list matches, every field's JSON shape matches (plain type
//! for required fields, `anyOf [T, null]` for optionals, array item
//! shapes, `$ref` targets), and the extra-allow posture matches the
//! component's `additionalProperties`. Models register as their routes
//! port; the registry must cover the full component set by the time the
//! HTTP surface moves natively.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eo_wire::models::{registered_contracts, FieldSchema};
use serde_json::Value;

fn snapshot() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../backend/tests/expected/openapi.snapshot.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("committed snapshot unreadable at {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("committed snapshot parses as JSON")
}

/// The non-null JSON-schema fragment a field schema must occupy.
fn assert_shape_matches(component: &str, name: &str, schema: FieldSchema, fragment: &Value) {
    let context = format!("{component}.{name}");
    match schema {
        FieldSchema::Str => assert_eq!(fragment["type"], "string", "{context}"),
        FieldSchema::Bool => assert_eq!(fragment["type"], "boolean", "{context}"),
        FieldSchema::Int => assert_eq!(fragment["type"], "integer", "{context}"),
        FieldSchema::Float => assert_eq!(fragment["type"], "number", "{context}"),
        FieldSchema::Object => assert_eq!(fragment["type"], "object", "{context}"),
        FieldSchema::ListFloat => {
            assert_eq!(fragment["type"], "array", "{context}");
            assert_eq!(fragment["items"]["type"], "number", "{context} items");
        }
        FieldSchema::ListRef(target) => {
            assert_eq!(fragment["type"], "array", "{context}");
            assert_eq!(
                fragment["items"]["$ref"],
                format!("#/components/schemas/{target}"),
                "{context} item ref"
            );
        }
    }
}

#[test]
fn every_registered_model_matches_its_snapshot_component() {
    let doc = snapshot();
    let components = doc["components"]["schemas"]
        .as_object()
        .expect("snapshot has component schemas");

    for contract in registered_contracts() {
        let component = components
            .get(contract.component)
            .unwrap_or_else(|| panic!("{} missing from the snapshot", contract.component));

        // Extra-allow posture.
        assert_eq!(
            component["additionalProperties"],
            Value::Bool(contract.extra_allow),
            "{}: extra-allow posture diverged",
            contract.component
        );

        // Exact property set.
        let declared: BTreeSet<&str> = contract.fields.iter().map(|f| f.name).collect();
        let in_schema: BTreeSet<&str> = component["properties"]
            .as_object()
            .expect("component has properties")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            declared, in_schema,
            "{}: property set diverged",
            contract.component
        );

        // Exact required set.
        let declared_required: BTreeSet<&str> = contract
            .fields
            .iter()
            .filter(|f| f.required)
            .map(|f| f.name)
            .collect();
        let schema_required: BTreeSet<&str> = component["required"]
            .as_array()
            .map(|list| list.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        assert_eq!(
            declared_required, schema_required,
            "{}: required set diverged",
            contract.component
        );

        // Per-field shape: required fields are plain types; optional
        // fields are nullable anyOf pairs (the pydantic v2 rendering).
        for field in contract.fields {
            let fragment = &component["properties"][field.name];
            if field.required {
                assert_shape_matches(contract.component, field.name, field.schema, fragment);
            } else {
                let arms = fragment["anyOf"].as_array().unwrap_or_else(|| {
                    panic!(
                        "{}.{}: optional field is not an anyOf",
                        contract.component, field.name
                    )
                });
                assert_eq!(
                    arms.len(),
                    2,
                    "{}.{}: expected [T, null] arms",
                    contract.component,
                    field.name
                );
                assert_shape_matches(contract.component, field.name, field.schema, &arms[0]);
                assert_eq!(
                    arms[1]["type"], "null",
                    "{}.{}: second arm is the null arm",
                    contract.component, field.name
                );
            }
        }
    }
}

/// The registry itself is covered by at least the spine models, so an
/// accidentally-emptied registry cannot pass vacuously.
#[test]
fn registry_is_not_vacuous() {
    let components: BTreeSet<&str> = ["HealthStatus", "NotableEvent", "TrackingSnapshot"]
        .into_iter()
        .collect();
    let registered: BTreeSet<&str> = registered_contracts().iter().map(|c| c.component).collect();
    assert!(
        registered.is_superset(&components),
        "spine models missing from the registry: {registered:?}"
    );
}
