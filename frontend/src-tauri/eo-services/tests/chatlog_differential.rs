//! Differential check: the native chat.log parser vs the backend
//! implementation, over every corpus scenario's replay log plus
//! curated edge lines, replies byte-compared through each side's
//! Python-faithful encoder.
//!
//! Gated behind the `cross-language` feature (needs the Python
//! interpreter and the backend package). Run with:
//!   cargo test -p eo-services --features cross-language --test chatlog_differential
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use eo_services::chatlog_parser::parse_line;
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
            .arg("backend.testing.chatlog_parser_cli")
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

    fn ask(&mut self, line: &str) -> String {
        let request = serde_json::to_string(&json!({ "line": line })).unwrap();
        writeln!(self.stdin, "{request}").expect("oracle write");
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

fn native_reply(line: &str) -> String {
    match parse_line(line) {
        None => "null".to_string(),
        Some(event) => {
            let value = json!({
                "type": event.event_type.as_str(),
                "timestamp": event.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                "data": Value::Object(event.data),
                "raw_line": event.raw_line,
            });
            to_python_json(&value, None)
        }
    }
}

/// Every line of every corpus scenario's replay log, both parsers.
#[test]
fn corpus_chatlogs_parse_identically() {
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
    assert!(logs.len() >= 12, "the corpus carries replay logs");

    let mut oracle = Oracle::spawn();
    let mut compared = 0usize;
    for log in &logs {
        let body = std::fs::read_to_string(log).expect("log read");
        for line in body.lines() {
            let expected = oracle.ask(line);
            let native = native_reply(line);
            assert_eq!(
                native,
                expected,
                "line diverged in {}: {line}",
                log.display()
            );
            compared += 1;
        }
    }
    // The floor reflects the COMMITTED corpus (the scripted scenarios plus the
    // recorded placeholder, ~90 lines). The earlier `> 100` floor implicitly
    // counted on locally-present recorded bundles, which are gitignored and so
    // absent on a clean checkout / CI, making the assertion env-dependent
    // (a clean checkout fell just short). A floor the committed corpus
    // satisfies keeps the guard ("the differential ran over real content, not
    // an empty glob") deterministic regardless of local recordings.
    assert!(compared >= 80, "the corpus drives a meaningful line count");
    println!(
        "compared {compared} corpus lines across {} logs",
        logs.len()
    );
}

/// Curated edges: entity unescaping, quantity names, the HOF/global
/// boundary, mission shapes, unmatched flavours, whitespace.
#[test]
fn curated_edge_lines_parse_identically() {
    let edges = [
        "2026-05-19 10:00:07 [System] [] You received Brown &amp; Gold Paint Value: 0.30 PED",
        "2026-05-19 10:00:07 [System] [] You received Tier &lt;2&gt; Component x (3) Value: 1.50 PED",
        // 0 / leading-zero counts are not real stack sizes: both arms keep
        // the literal name with quantity 1 rather than splitting the count.
        "2026-05-19 10:00:07 [System] [] You received Token x (0) Value: 1.00 PED",
        "2026-05-19 10:00:07 [System] [] You received Token x (007) Value: 1.00 PED",
        "2026-05-19 10:00:06 [Globals] [] Lucky Finder has found a rare item (Holy Grail) with a value of 90 PED! A record has been added to the Hall of Fame!",
        "2026-05-19 10:00:06 [Globals] [] Lucky Finder has found a rare item (Holy Grail) with a value of 90 PED!",
        "2026-05-19 10:00:06 [Globals] [] Someone killed a creature (Atrox Prowler) with a value of 1500 PED! A record has been added to the Hall of Fame!",
        "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
        "2026-05-19 10:00:05 [System] [] New Mission received (Daily Hunting)",
        "2026-05-19 10:00:04 [System] [] Your enhancer Weapon Damage Enhancer 3 on your ArMatrix LR-35 broke. You have 7 enhancers remaining on the item. You received 0.8000 PED Shrapnel. ",
        "2026-05-19 10:00:03 [System] [] You have gained 0.21 Combat Reflexes",
        "2026-05-19 10:00:03 [System] [] Your Agility has improved by 0.07",
        "  2026-05-19 10:00:00 [System] [] You took 3.0 points of damage  ",
        "2026-05-19 10:00:00 [Local] [] hello there",
        "2026-05-19 10:00:00 [System] [] something the rules do not know",
        "not a chat line at all",
        "",
    ];
    let mut oracle = Oracle::spawn();
    for line in edges {
        let expected = oracle.ask(line);
        let native = native_reply(line);
        assert_eq!(native, expected, "edge diverged: {line:?}");
    }
}
