//! Differential check: the native chat.log watcher pipeline vs the
//! backend implementation. Each case streams a line sequence through
//! the real watcher on both sides (temporary file, tail loop, drain,
//! stop) with a full-stream bus tap recording every publish, and the
//! ordered (topic, payload) sequences are byte-compared through each
//! side's Python-faithful encoder. Corpus scenarios cover the real
//! tick shapes; curated cases cover quest suppression, refund
//! matching, and tick boundaries.
//!
//! Gated behind the `cross-language` feature. Run with:
//!   cargo test -p eo-services --features cross-language --test chatlog_watcher_differential
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eo_services::chatlog_watcher::{ChatlogWatcher, QuestRewardFilter};
use eo_services::event_bus::EventBus;
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
            .arg("backend.testing.chatlog_watcher_cli")
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

    fn run(&mut self, lines: &[String], suppress: Option<&Value>) -> String {
        let mut request = json!({ "lines": lines });
        if let Some(suppress) = suppress {
            request["suppress"] = suppress.clone();
        }
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

/// The same run through the native watcher: temp file, tail, drain,
/// stop, recorded tap stream.
fn native_run(lines: &[String], suppress: Option<&Value>) -> String {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("chat_replay.log");
    std::fs::File::create(&log_path).unwrap();

    let bus = Arc::new(EventBus::new());
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = recorded.clone();
    bus.add_tap(move |topic, data| {
        sink.lock().unwrap().push(json!({
            "topic": topic.as_str(),
            "payload": data,
        }));
    });

    let filter: Option<QuestRewardFilter> = suppress.map(|value| {
        let fixed = value.clone();
        let filter: QuestRewardFilter = Arc::new(move |_, _, _| Some(fixed.clone()));
        filter
    });
    let watcher = ChatlogWatcher::new(bus, &log_path, filter);
    watcher.start();
    {
        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        for line in lines {
            writeln!(handle, "{line}").unwrap();
        }
    }
    watcher
        .wait_until_drained(lines.len() as u64, Duration::from_secs(10))
        .unwrap();
    watcher.stop();

    let recorded = recorded.lock().unwrap();
    to_python_json(&Value::Array(recorded.clone()), None)
}

#[test]
fn corpus_scenarios_stream_identically() {
    let corpus = repo_root().join("backend/tests/e2e/corpus");
    let mut logs = Vec::new();
    for family in ["scripted", "recorded"] {
        let Ok(entries) = std::fs::read_dir(corpus.join(family)) else {
            continue;
        };
        for entry in entries.flatten() {
            let log = entry.path().join("chat_replay.log");
            if log.exists() {
                logs.push(log);
            }
        }
    }
    logs.sort();
    assert!(logs.len() >= 12);

    let mut oracle = Oracle::spawn();
    for log in &logs {
        let body = std::fs::read_to_string(log).expect("log read");
        let lines: Vec<String> = body.lines().map(str::to_string).collect();
        if lines.is_empty() {
            continue;
        }
        let expected = oracle.run(&lines, None);
        let native = native_run(&lines, None);
        assert_eq!(native, expected, "stream diverged for {}", log.display());
    }
}

#[test]
fn curated_pipelines_stream_identically() {
    let cases: Vec<(Vec<&str>, Option<Value>)> = vec![
        // Quest suppression by fixed indexes.
        (
            vec![
                "2026-05-19 10:00:05 [System] [] You received Wool Value: 1.50 PED",
                "2026-05-19 10:00:05 [System] [] You received Reward Token Value: 5.00 PED",
                "2026-05-19 10:00:05 [System] [] You have gained 0.21 Combat Reflexes",
                "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
            ],
            Some(json!({"suppress_loot_index": 1, "suppress_skill_index": 0})),
        ),
        // Out-of-range suppression indexes are ignored.
        (
            vec![
                "2026-05-19 10:00:05 [System] [] You received Wool Value: 1.50 PED",
                "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
            ],
            Some(json!({"suppress_loot_index": 9, "suppress_skill_index": -1})),
        ),
        // Enhancer refunds flag the first matching shrapnel only.
        (
            vec![
                "2026-05-19 10:00:04 [System] [] Your enhancer Weapon Damage Enhancer 3 on your ArMatrix LR-35 broke. You have 7 enhancers remaining on the item. You received 0.8000 PED Shrapnel. ",
                "2026-05-19 10:00:04 [System] [] You received Shrapnel x (80) Value: 0.80 PED",
                "2026-05-19 10:00:04 [System] [] You received Shrapnel x (80) Value: 0.80 PED",
                "2026-05-19 10:00:04 [System] [] You received Wool Value: 0.80 PED",
            ],
            None,
        ),
        // Tick boundaries split on the second; unknown lines never break a tick.
        (
            vec![
                "2026-05-19 10:00:02 [System] [] You received Shrapnel x (10) Value: 0.10 PED",
                "2026-05-19 10:00:02 [Local] [] interleaved chatter",
                "2026-05-19 10:00:02 [System] [] You received Wool Value: 1.50 PED",
                "2026-05-19 10:00:03 [System] [] You received Hide Value: 0.20 PED",
                "2026-05-19 10:00:06 [Globals] [] Hunter Dude killed a creature (Atrox Young) with a value of 56 PED!",
                "2026-05-19 10:00:06 [System] [] New Mission received (Daily Hunting)",
            ],
            None,
        ),
    ];

    let mut oracle = Oracle::spawn();
    for (index, (lines, suppress)) in cases.iter().enumerate() {
        let lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        let expected = oracle.run(&lines, suppress.as_ref());
        let native = native_run(&lines, suppress.as_ref());
        assert_eq!(native, expected, "case {index} diverged");
    }
}
