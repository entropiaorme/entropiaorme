//! Rust leg of the cross-language Normalizer conformance table.
//!
//! Reads the committed fixture
//! `backend/testing/equivalence/normalizer_conformance.json` (the same one the
//! Python leg asserts against) and checks the native normaliser reproduces
//! every `expected` byte-for-byte. This test is hermetic: it needs only the
//! committed fixture, no Python at runtime, so it runs on every Rust CI job.

use std::path::PathBuf;

use eo_wire::normalizer::Normalizer;
use serde_json::Value;

/// Resolve the repo-root-relative fixture path from this crate's manifest dir
/// (`frontend/src-tauri/eo-wire` -> three levels up is the repo root).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../backend/testing/equivalence/normalizer_conformance.json")
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
