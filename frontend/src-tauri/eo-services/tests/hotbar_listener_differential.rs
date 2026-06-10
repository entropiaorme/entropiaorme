//! Differential check: the native hotbar listener vs the backend
//! implementation. Each case scripts toggle, session, keystroke, and
//! await steps through the real listener on both sides (mock source,
//! scripted resolver, full-stream bus tap) and compares the recorded
//! (topic, payload) sequences, the keystroke-tap record, and the final
//! running state byte-for-byte.
//!
//! Gated behind the `cross-language` feature. Run with:
//!   cargo test -p eo-services --features cross-language --test hotbar_listener_differential
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use eo_services::event_bus::{EventBus, Topic};
use eo_services::hotbar_listener::{HotbarListener, HotbarResolver};
use eo_services::keystroke_source::{KeystrokeKind, MockKeystrokeSource};
use eo_wire::normalizer::to_python_json;
use serde_json::{json, Value};

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
            .arg("backend.testing.hotbar_listener_cli")
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

    fn run(&mut self, steps: &Value) -> String {
        let request = json!({ "steps": steps });
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).expect("oracle write");
        self.stdin.flush().expect("oracle flush");
        let mut reply = String::new();
        let bytes = self.stdout.read_line(&mut reply).expect("oracle read");
        assert!(
            bytes > 0,
            "oracle exited early (EOF on stdout); status: {:?}",
            self.child.try_wait()
        );
        reply.trim_end().to_string()
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn scripted_resolver() -> HotbarResolver {
    Arc::new(|slot: &str| match slot {
        "1" => Some(("Opalo".to_string(), 0.05, "weapon".to_string(), 0.0)),
        "2" => Some(("Healer".to_string(), 0.088, "healing".to_string(), 2.5)),
        "3" => Some(("Snack".to_string(), 0.01, "consumable".to_string(), 0.0)),
        _ => None,
    })
}

fn timestamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn native_run(steps: &Value) -> String {
    let bus = Arc::new(EventBus::new());
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = recorded.clone();
    bus.add_tap(move |topic, data| {
        sink.lock().unwrap().push(json!({
            "topic": topic.as_str(),
            "payload": data,
        }));
    });

    let source = Arc::new(MockKeystrokeSource::new());
    let listener =
        HotbarListener::new(bus.clone(), Some(source.clone()), Some(scripted_resolver()));
    let taps: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let tap_sink = taps.clone();
    listener.set_key_tap(Arc::new(move |key: &str, kind: &str| {
        tap_sink.lock().unwrap().push(json!([key, kind]));
    }));

    let await_tool_events = |count: usize| {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let tool_count = recorded
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| {
                    matches!(
                        entry["topic"].as_str(),
                        Some("active_tool_changed") | Some("active_heal_tool_changed")
                    )
                })
                .count();
            if tool_count >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    for step in steps.as_array().expect("steps array") {
        let step = step.as_array().expect("step array");
        match step[0].as_str().unwrap() {
            "toggle" => listener.set_hotbar_hooks_enabled(step[1].as_bool().unwrap()),
            "session" => {
                let topic = if step[1].as_bool().unwrap() {
                    Topic::SessionStarted
                } else {
                    Topic::SessionStopped
                };
                bus.publish(topic, &Value::Null);
            }
            "key" => {
                let kind = match step[2].as_str().unwrap() {
                    "press" => KeystrokeKind::Press,
                    _ => KeystrokeKind::Release,
                };
                source.inject(step[1].as_str().unwrap(), timestamp(), kind);
            }
            "await" => await_tool_events(step[1].as_u64().unwrap() as usize),
            other => panic!("unknown step kind {other}"),
        }
    }
    listener.stop();
    let running = listener.is_running();

    let reply = json!({
        "stream": Value::Array(recorded.lock().unwrap().clone()),
        "taps": Value::Array(taps.lock().unwrap().clone()),
        "running": running,
    });
    to_python_json(&reply, None)
}

#[test]
fn scripted_sessions_stream_identically() {
    let cases = [
        // The full gate-and-resolve walk: weapon, heal, consumable,
        // empty slot, non-slot key, release edge.
        json!([
            ["toggle", true],
            ["session", true],
            ["key", "1", "press"],
            ["await", 1],
            ["key", "2", "press"],
            ["await", 2],
            ["key", "3", "press"],
            ["key", "9", "press"],
            ["key", "space", "press"],
            ["key", "1", "release"],
            ["session", false]
        ]),
        // No session: nothing flows.
        json!([
            ["toggle", true],
            ["key", "1", "press"],
            ["session", true],
            ["key", "2", "press"],
            ["await", 1],
            ["toggle", false],
            ["key", "1", "press"]
        ]),
        // Toggle off from the start: the source never runs.
        json!([["session", true], ["key", "1", "press"], ["session", false]]),
    ];
    let mut oracle = Oracle::spawn();
    for (index, steps) in cases.iter().enumerate() {
        let expected = oracle.run(steps);
        let native = native_run(steps);
        assert_eq!(native, expected, "case {index} diverged");
    }
}
