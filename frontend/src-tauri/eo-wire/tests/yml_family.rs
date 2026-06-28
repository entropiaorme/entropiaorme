//! The frozen `.yml`-family inheritance.
//!
//! The three listener bus-stream pins (hotbar and spacebar) and the
//! quest-automation pin are banked as canonical-JSON mirrors under
//! `eo-wire/tests/fixtures/yml_family/`. This asserts the native normaliser +
//! serialiser reproduce each pinned projection byte-for-byte, so a listener
//! change is caught against the frozen golden. Hermetic: it reads the committed
//! mirrors only.

use std::path::PathBuf;

use eo_wire::normalizer::{to_python_json, Normalizer};
use serde_json::Value;

const MIRRORS: [&str; 3] = [
    "hotbar_slot_use",
    "spacebar_scan_capture",
    "quest_automation_with_playlist_match",
];

fn mirror_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/yml_family")
}

#[test]
fn native_render_reproduces_every_yml_mirror() {
    let mut failures: Vec<String> = Vec::new();
    for stem in MIRRORS {
        let path = mirror_dir().join(format!("{stem}.json"));
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let parsed: Value = serde_json::from_str(&committed)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        let normalised = Normalizer::new().normalize(&parsed);
        let rendered = to_python_json(&normalised, Some(2)) + "\n";
        if rendered != committed {
            failures.push(stem.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} .yml-family mirror(s) did not reproduce byte-for-byte: {:?}",
        failures.len(),
        failures
    );
}
