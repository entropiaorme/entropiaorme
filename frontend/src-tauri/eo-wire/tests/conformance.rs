//! Frozen Normalizer conformance table.
//!
//! Reads the committed fixture
//! `eo-wire/tests/fixtures/normalizer_conformance.json` and checks the native
//! normaliser reproduces every `expected` byte-for-byte. This is the frozen
//! equivalence evidence for the Normalizer: a fixed input/output table banked
//! when the byte-identical port was proven, asserted on every Rust CI job.

use std::path::PathBuf;

use eo_wire::normalizer::Normalizer;
use serde_json::Value;

/// Resolve the committed conformance fixture from this crate's manifest dir
/// (`frontend/src-tauri/eo-wire/tests/fixtures/`).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/normalizer_conformance.json")
}

#[test]
fn rust_leg_reproduces_every_expected() {
    let raw =
        std::fs::read_to_string(fixture_path()).expect("committed conformance fixture is readable");
    let cases: Vec<Value> =
        serde_json::from_str(&raw).expect("conformance fixture parses as JSON array");

    assert!(
        !cases.is_empty(),
        "conformance fixture must not be empty (a vacuous pass is forbidden)"
    );

    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().expect("case has a name");
        let input = &case["input"];
        let expected = case["expected"].as_str().expect("case has expected string");

        let actual = Normalizer::new().normalize_to_compact_json(input);
        if actual != expected {
            failures.push(format!(
                "case {name:?}: expected {expected:?}, got {actual:?}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} conformance case(s) diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
