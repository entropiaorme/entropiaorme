//! Differential property fuzz: the native cost engine vs the Python oracle.
//!
//! For each randomly generated `properties_json` equipment payload, this asserts
//! `eo-services::cost_engine::cost_per_shot_from_props` produces byte-identical
//! normalised output to `backend.services.cost_engine.cost_per_shot_from_props`
//! (compared through the shared Normalizer, so the per-line `round(_, 4)`
//! semantics and float rendering are proven equal, not just approximately so).
//!
//! Gated behind the `cross-language` feature (needs the Python interpreter +
//! backend at runtime). Run with:
//!   cargo test -p eo-services --features cross-language --test cost_differential
//!
//! The oracle interpreter is `$EO_ORACLE_PYTHON` if set, else the local
//! virtualenv.
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use eo_services::cost_engine::cost_per_shot_from_props;
use eo_wire::normalizer::Normalizer;
use proptest::prelude::*;
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
            .arg("backend.testing.cost_engine_cli")
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
            .expect("spawn the Python cost-engine oracle (is the venv installed?)");
        let stdin = child.stdin.take().expect("oracle stdin");
        let stdout = BufReader::new(child.stdout.take().expect("oracle stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn cost(&mut self, props_line: &str) -> String {
        writeln!(self.stdin, "{props_line}").expect("write oracle stdin");
        self.stdin.flush().expect("flush oracle stdin");
        let mut response = String::new();
        let read = self
            .stdout
            .read_line(&mut response)
            .expect("read oracle stdout");
        assert!(read > 0, "oracle closed its output unexpectedly");
        response.trim_end_matches(['\r', '\n']).to_string()
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn oracle() -> &'static Mutex<Oracle> {
    static ORACLE: OnceLock<Mutex<Oracle>> = OnceLock::new();
    ORACLE.get_or_init(|| Mutex::new(Oracle::spawn()))
}

/// A small economy subdict with random decay / ammo_burn.
fn economy(decay: f64, ammo_burn: f64) -> Value {
    json!({"economy": {"decay": decay, "ammo_burn": ammo_burn}})
}

fn props_strategy() -> impl Strategy<Value = Value> {
    let amount = -2.0f64..200.0f64; // includes zero/negative edges
    let markup = prop_oneof![Just(100.0f64), 90.0f64..250.0f64];
    (
        amount.clone(),
        amount.clone(),
        // optional amp / scope / absorber: None, an empty dict (falsy), or real
        proptest::option::of(prop_oneof![
            Just(json!({})),
            (amount.clone(), amount.clone()).prop_map(|(d, a)| economy(d, a)),
        ]),
        proptest::option::of(prop_oneof![
            Just(json!({})),
            amount
                .clone()
                .prop_map(|d| json!({"economy": {"decay": d}})),
        ]),
        proptest::option::of(prop_oneof![
            Just(json!({})),
            (0.0f64..1.0f64).prop_map(|abs| json!({"economy": {"absorption": abs}})),
        ]),
        markup.clone(),
        markup.clone(),
        markup.clone(),
        markup,
        -3i64..6i64,
    )
        .prop_map(
            |(
                decay,
                ammo,
                amp,
                scope,
                absorber,
                w_markup,
                a_markup,
                s_markup,
                ab_markup,
                enhancers,
            )| {
                let mut props = serde_json::Map::new();
                props.insert("weapon_entity".into(), economy(decay, ammo));
                if let Some(a) = amp {
                    props.insert("amp_entity".into(), a);
                }
                if let Some(s) = scope {
                    props.insert("scope_entity".into(), s);
                }
                if let Some(ab) = absorber {
                    props.insert("absorber_entity".into(), ab);
                }
                props.insert("weapon_markup".into(), json!(w_markup));
                props.insert("amp_markup".into(), json!(a_markup));
                props.insert("scope_markup".into(), json!(s_markup));
                props.insert("absorber_markup".into(), json!(ab_markup));
                props.insert("damage_enhancers".into(), json!(enhancers));
                Value::Object(props)
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn native_cost_matches_python_oracle(props in props_strategy()) {
        let props_line = serde_json::to_string(&props).expect("serialise props");
        let python = oracle().lock().expect("oracle lock").cost(&props_line);
        let rust_result = cost_per_shot_from_props(&props, None);
        let rust = Normalizer::new().normalize_to_compact_json(&rust_result);
        prop_assert_eq!(&rust, &python, "props: {}", props_line);
    }
}
