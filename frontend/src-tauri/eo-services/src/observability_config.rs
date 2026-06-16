//! The shell-owned observability config: a small JSON file the native side
//! owns outright, holding the default-off crash-reporting opt-in (and any
//! future observability toggles).
//!
//! It lives in `<data_dir>/observability.json`, deliberately NOT in
//! `settings.json`. `settings.json` is the dual-arm equivalence surface (the
//! native and the frozen Python settings routes are diffed on the real
//! database), and its typed `AppConfig` serialises straight into the settings
//! response, so a new field there would diverge the native arm from the Python
//! arm. A Rust-owned file sidesteps that and keeps the feature behaviour-neutral.
//!
//! This module is the single home for reading and writing that file, so both
//! consumers share one implementation: the shell's crash hook reads the opt-in,
//! and the hidden dev-tools route reads and writes it.

use std::path::Path;

use serde_json::{Map, Value};

const OBSERVABILITY_CONFIG: &str = "observability.json";
const CRASH_REPORTING_KEY: &str = "crash_reporting_enabled";

/// Whether crash reporting is enabled. Absent, unreadable, malformed, or
/// missing-the-key all read as `false` (the default-off contract).
pub fn crash_reporting_enabled(data_dir: &Path) -> bool {
    let raw = match std::fs::read_to_string(data_dir.join(OBSERVABILITY_CONFIG)) {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|value| value.get(CRASH_REPORTING_KEY).and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Set the crash-reporting opt-in, preserving any other keys already in the
/// file (an atomic-enough read-merge-write). The file may not exist yet (the
/// default-off contract), in which case it is created.
pub fn set_crash_reporting_enabled(data_dir: &Path, enabled: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(OBSERVABILITY_CONFIG);
    let mut object: Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    object.insert(CRASH_REPORTING_KEY.to_string(), Value::Bool(enabled));
    let body = serde_json::to_string_pretty(&Value::Object(object))?;
    std::fs::write(&path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_when_absent_or_malformed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!crash_reporting_enabled(dir.path()));
        std::fs::write(dir.path().join(OBSERVABILITY_CONFIG), "{not json").unwrap();
        assert!(!crash_reporting_enabled(dir.path()));
        std::fs::write(dir.path().join(OBSERVABILITY_CONFIG), "{}").unwrap();
        assert!(!crash_reporting_enabled(dir.path()));
    }

    #[test]
    fn the_opt_in_round_trips_and_preserves_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(OBSERVABILITY_CONFIG),
            r#"{"some_future_key": 7}"#,
        )
        .unwrap();
        set_crash_reporting_enabled(dir.path(), true).unwrap();
        assert!(crash_reporting_enabled(dir.path()));
        let raw = std::fs::read_to_string(dir.path().join(OBSERVABILITY_CONFIG)).unwrap();
        assert!(
            raw.contains("some_future_key"),
            "unrelated key survives the merge"
        );
        set_crash_reporting_enabled(dir.path(), false).unwrap();
        assert!(!crash_reporting_enabled(dir.path()));
    }
}
