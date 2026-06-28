//! Emitter proof: the native DB-snapshot and HTTP-fingerprint emitters (and the
//! event-stream fingerprint) reproduce the committed `basic_hunt_10_events`
//! goldens byte-for-byte.
//!
//! Hermetic: it feeds the committed raw-capture fixtures (the pre-normalisation
//! bus events, DB rows, and HTTP responses a replay produced) through the Rust
//! emitters and asserts byte-equality against the committed goldens. The goldens
//! are the frozen equivalence evidence banked when the byte-identical port was
//! proven; the raw captures and goldens are committed together, so a stale
//! fixture cannot pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use eo_wire::http_fingerprint::RawResponse;
use eo_wire::normalizer::Normalizer;
use eo_wire::{db_snapshot, fingerprint, http_fingerprint};
use serde_json::{Map, Value};

fn scenario_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/corpus/scripted/basic_hunt_10_events")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The fingerprint and DB snapshot share one Normalizer (the bus events are
/// normalised first, then the DB rows continue the symbol table), exactly as
/// `GoldenSet`. Proving both under one normaliser is the byte-equality proof.
#[test]
fn fingerprint_and_db_snapshot_match_committed_goldens() {
    let scenario = scenario_dir();
    let raw = scenario.join("raw_captures");
    let expected = scenario.join("expected");

    let mut normalizer = Normalizer::new();

    // --- fingerprint (events first, assigning the shared symbols) ---
    let events_raw = read_json(&raw.join("events.json"));
    let events: Vec<(String, Value)> = events_raw
        .as_array()
        .expect("events.json is an array")
        .iter()
        .map(|e| {
            let topic = e["topic"].as_str().expect("event has a topic").to_string();
            (topic, e["payload"].clone())
        })
        .collect();
    let actual_fingerprint = fingerprint::serialize_events(&events, &mut normalizer);
    let expected_fingerprint = read(&expected.join("fingerprint.jsonl"));
    assert_eq!(
        actual_fingerprint, expected_fingerprint,
        "fingerprint.jsonl diverged"
    );

    // --- DB snapshot (continues the same normaliser's symbol table) ---
    let db_rows_raw = read_json(&raw.join("db_rows.json"));
    let db_rows: Map<String, Value> = db_rows_raw
        .as_object()
        .expect("db_rows.json is an object")
        .clone();
    let snapshot = db_snapshot::capture(&db_rows, &mut normalizer);
    let actual_db = db_snapshot::serialize(&snapshot);
    let expected_db = read(&expected.join("db_state.json"));
    assert_eq!(actual_db, expected_db, "db_state.json diverged");
}

/// The HTTP fingerprints share a separate fresh Normalizer across the curated
/// endpoint set, captured in the fixed dump order so the symbol table grows
/// deterministically.
#[test]
fn http_fingerprints_match_committed_goldens() {
    let scenario = scenario_dir();
    let raw = scenario.join("raw_captures");
    let http_dir = scenario.join("expected/http_responses");

    let captures_raw = read_json(&raw.join("http_responses.json"));
    let captures = captures_raw
        .as_array()
        .expect("http_responses.json is an array");
    assert!(
        !captures.is_empty(),
        "http raw captures must not be empty (a vacuous pass is forbidden)"
    );

    let mut normalizer = Normalizer::new();
    let mut diverged: Vec<String> = Vec::new();

    for capture in captures {
        let endpoint_id = capture["endpoint_id"].as_str().expect("endpoint_id");
        let method = capture["method"].as_str().expect("method");
        let path = capture["path"].as_str().expect("path");
        let query: Map<String, Value> = capture["query"].as_object().cloned().unwrap_or_default();
        let status_code = capture["status_code"].as_i64().expect("status_code");
        let headers: Map<String, Value> = capture["headers"].as_object().expect("headers").clone();
        let body = base64::engine::general_purpose::STANDARD
            .decode(capture["body_b64"].as_str().expect("body_b64"))
            .expect("body_b64 decodes");

        let raw_response = RawResponse {
            method,
            path,
            query: &query,
            status_code,
            headers: &headers,
            body: &body,
        };
        let actual = http_fingerprint::serialize_capture(&http_fingerprint::capture(
            &raw_response,
            &mut normalizer,
        ));
        let expected = read(&http_dir.join(format!("{endpoint_id}.json")));
        if actual != expected {
            diverged.push(endpoint_id.to_string());
        }
    }

    assert!(
        diverged.is_empty(),
        "{} HTTP fingerprint(s) diverged: {:?}",
        diverged.len(),
        diverged
    );
}

/// Guard the dump order: the curated endpoint set the symbol table depends on
/// must stay at ten, mirroring the Python contract's cardinality pin.
#[test]
fn http_capture_set_is_the_full_curated_ten() {
    let captures_raw = read_json(&scenario_dir().join("raw_captures/http_responses.json"));
    let ids: BTreeMap<usize, String> = captures_raw
        .as_array()
        .expect("array")
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                i,
                c["endpoint_id"].as_str().expect("endpoint_id").to_string(),
            )
        })
        .collect();
    assert_eq!(ids.len(), 10, "expected the full curated 10 endpoints");
}
