//! Differential property fuzz: the native Normalizer vs the Python oracle.
//!
//! For each randomly generated JSON value, this sends the value to a long-lived
//! Python oracle process (`backend.testing.normalize_cli` in line-server mode)
//! and asserts the native `eo-wire::normalizer` produces byte-identical output.
//! This is the guard for divergences a hand-authored conformance table can
//! miss, above all the "ryu vs Python `float.__repr__`" float-formatting
//! divergence the rounding/repr port has to reproduce exactly.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime; the hermetic conformance
//! test (`conformance.rs`) covers the same surface against a committed fixture
//! on the Python-free CI jobs. Run it with:
//!   cargo test -p eo-wire --features cross-language --test fuzz_differential
//!
//! The oracle interpreter is `$EO_ORACLE_PYTHON` if set, else the local
//! virtualenv (`.venv/Scripts/python.exe` on Windows, `.venv/bin/python`
//! elsewhere).
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use eo_wire::normalizer::Normalizer;
use proptest::prelude::*;
use serde_json::{Number, Value};

/// Repo root, three levels above this crate's manifest dir.
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

/// A long-lived Python normaliser process driven one value per line.
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
            .arg("backend.testing.normalize_cli")
            .current_dir(repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // Suppress a console-window flash if a GUI-subsystem parent ever spawns
        // the oracle on Windows; the stdio pipes are unaffected.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .expect("spawn the Python normaliser oracle (is the venv installed?)");
        let stdin = child.stdin.take().expect("oracle stdin");
        let stdout = BufReader::new(child.stdout.take().expect("oracle stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Send one compact JSON line, return the normalised line (newline trimmed).
    fn normalize(&mut self, json_line: &str) -> String {
        writeln!(self.stdin, "{json_line}").expect("write to oracle stdin");
        self.stdin.flush().expect("flush oracle stdin");
        let mut response = String::new();
        let read = self
            .stdout
            .read_line(&mut response)
            .expect("read oracle stdout");
        assert!(
            read > 0,
            "oracle closed its output unexpectedly (process dead?)"
        );
        response.trim_end_matches(['\r', '\n']).to_string()
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        // Closing stdin lets the server loop hit EOF and exit; reap it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn oracle() -> &'static Mutex<Oracle> {
    static ORACLE: OnceLock<Mutex<Oracle>> = OnceLock::new();
    ORACLE.get_or_init(|| Mutex::new(Oracle::spawn()))
}

// --- generators --------------------------------------------------------------

fn float_value() -> impl Strategy<Value = Value> {
    // Fixed hazards alongside a wide finite range, so the fuzz reliably hits
    // the ties-to-even, integer-valued, epoch-window, and scientific-threshold
    // paths rather than relying on the range to stumble onto them.
    let fixed = prop_oneof![
        Just(0.0f64),
        Just(-0.0f64),
        Just(15.0f64),
        Just(100.0f64),
        Just(0.03125f64),
        Just(0.09375f64),
        Just(0.0001f64),
        Just(-0.0001f64),
        Just(1e16f64),
        Just(1.5e16f64),
        Just(1e-5f64),
        Just(1_000_000_000.0f64),  // epoch min boundary
        Just(20_000_000_000.0f64), // epoch max boundary
        Just(1_700_000_000.0f64),  // inside epoch window
        Just(20_000_000_001.0f64), // just above the window
    ];
    let ranged = (-1e22f64..1e22f64).prop_filter("finite", |f| f.is_finite());
    prop_oneof![fixed, ranged].prop_map(|f| Value::Number(Number::from_f64(f).expect("finite")))
}

fn string_value() -> impl Strategy<Value = Value> {
    // Random ASCII (including control chars and DEL, to exercise escaping),
    // plus injected UUID / ISO / non-ASCII candidates. Strings stay ASCII for
    // the random arm so the recogniser's ASCII timestamp/UUID domain holds; the
    // injected non-ASCII literals cannot form an ISO/UUID prefix.
    let ascii = prop::collection::vec(0u8..=0x7f, 0..12)
        .prop_map(|bytes| String::from_utf8(bytes).expect("ascii bytes are valid utf-8"));
    prop_oneof![
        9 => ascii,
        1 => Just("11111111-1111-1111-1111-111111111111".to_string()),
        1 => Just("22222222-2222-2222-2222-222222222222".to_string()),
        1 => Just("2026-01-01T00:00:00".to_string()),
        1 => Just("2026-01-01 00:00:00.5+00:00".to_string()),
        1 => Just("café ⚔ naïve".to_string()),
    ]
    .prop_map(Value::String)
}

fn key_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::range('a', 'z'), 1..6)
        .prop_map(|chars| chars.into_iter().collect())
}

fn int_value() -> impl Strategy<Value = Value> {
    // Cover the whole wire integer domain [i64::MIN, u64::MAX]: signed and
    // unsigned 64-bit, plus the exact extremes (the boundary beyond which
    // serde_json would spill an integer into the f64 arm).
    prop_oneof![
        any::<i64>().prop_map(|i| Value::Number(i.into())),
        any::<u64>().prop_map(|u| Value::Number(u.into())),
        Just(Value::Number(i64::MIN.into())),
        Just(Value::Number(u64::MAX.into())),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        int_value(),
        float_value(),
        string_value(),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec((key_strategy(), inner), 0..6)
                .prop_map(|pairs| { Value::Object(pairs.into_iter().collect()) }),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn native_normaliser_matches_python_oracle(value in json_value()) {
        // serde_json's compact form round-trips every generated value to the
        // identical f64/int/string on the Python side, so both legs normalise
        // the same input.
        let input_line = serde_json::to_string(&value).expect("serialise generated value");
        let python = oracle().lock().expect("oracle lock").normalize(&input_line);
        let rust = Normalizer::new().normalize_to_compact_json(&value);
        prop_assert_eq!(&rust, &python, "input: {}", input_line);
    }
}
