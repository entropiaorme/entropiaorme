//! Differential check: the native configuration service vs the Python
//! oracle, over curated stored-file and update scenarios.
//!
//! Each scenario materialises a settings file (or none), applies a
//! sequence of updates through the real service on both sides, and
//! compares the SAVED FILE BYTES and the resulting config state
//! byte-for-byte (the host-dependent default chat-log path projects to
//! a sentinel on both sides). This pins the writer's byte shape (ASCII
//! escaping, indentation, merge positions) and the normalisation
//! semantics against the backend implementation.
//!
//! Gated behind the `cross-language` feature because it needs the
//! Python interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test config_differential
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use eo_services::config_service::{AppConfig, ConfigService};
use eo_wire::normalizer::to_python_json;
use serde_json::{json, Map, Value};

const CHATLOG_SENTINEL: &str = "<DEFAULT_CHATLOG>";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn oracle_python() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_ORACLE_PYTHON") {
        return PathBuf::from(explicit);
    }
    let root = repo_root();
    let windows = root.join(".venv/Scripts/python.exe");
    if windows.exists() {
        windows
    } else {
        root.join(".venv/bin/python")
    }
}

struct Oracle {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Oracle {
    fn spawn() -> Self {
        let mut command = Command::new(oracle_python());
        command
            .arg("-m")
            .arg("backend.testing.config_service_cli")
            .current_dir(repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().expect("oracle spawn");
        let stdin = child.stdin.take().expect("oracle stdin");
        let stdout = BufReader::new(child.stdout.take().expect("oracle stdout"));
        Oracle {
            child,
            stdin,
            stdout,
        }
    }

    fn round_trip(&mut self, request: &Value) -> Value {
        let line = serde_json::to_string(request).expect("request serialises");
        writeln!(self.stdin, "{line}").expect("oracle write");
        self.stdin.flush().expect("oracle flush");
        let mut reply = String::new();
        let bytes = self.stdout.read_line(&mut reply).expect("oracle read");
        assert!(
            bytes > 0,
            "oracle exited early (EOF on stdout); status: {:?}",
            self.child.try_wait()
        );
        serde_json::from_str(reply.trim_end()).expect("oracle reply parses")
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The native side of the same round trip.
fn native_round_trip(request: &Value) -> (String, Value) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(stored) = request.get("stored").filter(|v| !v.is_null()) {
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::to_string(stored).unwrap(),
        )
        .unwrap();
    }
    let mut service = ConfigService::new(dir.path()).unwrap();
    if let Some(update_list) = request.get("updates").and_then(Value::as_array) {
        for updates in update_list {
            let map: Map<String, Value> = updates.as_object().cloned().unwrap_or_default();
            service.update(&map).unwrap();
        }
    }
    let mut file_text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    let mut state = match serde_json::to_value(service.get()).unwrap() {
        Value::Object(map) => map,
        _ => unreachable!(),
    };

    let default_path = AppConfig::default_chatlog_path();
    if state.get("chatlog_path").and_then(Value::as_str) == Some(default_path.as_str()) {
        state.insert("chatlog_path".into(), Value::from(CHATLOG_SENTINEL));
    }
    // The file stores the path JSON-escaped (backslashes doubled on
    // Windows); project the escaped form, as the oracle does.
    let escaped: String = serde_json::to_string(&default_path).unwrap();
    file_text = file_text.replace(&escaped[1..escaped.len() - 1], CHATLOG_SENTINEL);
    (file_text, Value::Object(state))
}

fn scenarios() -> Vec<Value> {
    vec![
        // Fresh start, no stored file, no updates.
        json!({"stored": null, "updates": [{}]}),
        // Unknown keys keep their positions; toggles coerce; updates land.
        json!({
            "stored": {
                "extensionKey": {"nested": [1, 2]},
                "player_name": "Frussj\u{00e4}ger",
                "hotbar_hooks_enabled": 1,
            },
            "updates": [{"mob_tracking_tag": "tagged"}, {}],
        }),
        // Preset normalisation: blank ids, duplicates, numeric ids,
        // name fallbacks, unknown active id.
        json!({
            "stored": {
                "trifecta_presets": [
                    {"id": "  ", "name": "skipped"},
                    {"id": "alpha", "name": ""},
                    {"id": "alpha", "name": "dupe"},
                    {"id": 42, "small_weapon_id": 7},
                ],
                "active_trifecta_preset_id": "ghost",
            },
            "updates": [{}],
        }),
        // Hotbar updates re-normalise to the full slot shape.
        json!({
            "stored": null,
            "updates": [{"hotbar": {"3": 17, "11": 9}}, {}],
        }),
        // Active-preset fallback collapses the list when unresolvable.
        json!({
            "stored": null,
            "updates": [{"active_trifecta_preset_id": "ghost"}, {}],
        }),
        // Non-ASCII and astral-plane content in stored unknown keys.
        json!({
            "stored": {"note": "snowman \u{2603} and beyond \u{1F600}"},
            "updates": [{"player_name": "L\u{00e9}a"}, {}],
        }),
        // Reset-like overwrite after a populated store.
        json!({
            "stored": {"player_name": "Old", "overlay_x": 40, "overlay_y": -3},
            "updates": [{"overlay_x": 120}, {"player_name": "New"}, {}],
        }),
    ]
}

#[test]
fn config_round_trips_match_the_oracle_byte_for_byte() {
    let mut oracle = Oracle::spawn();
    for (index, request) in scenarios().iter().enumerate() {
        let reply = oracle.round_trip(request);
        let (native_file, native_state) = native_round_trip(request);
        assert_eq!(
            reply["file"].as_str().unwrap(),
            native_file,
            "scenario {index}: saved file bytes diverged"
        );
        assert_eq!(
            to_python_json(&reply["state"], None),
            to_python_json(&native_state, None),
            "scenario {index}: reloaded state diverged"
        );
    }
}
