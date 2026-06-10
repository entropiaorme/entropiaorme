//! Differential fuzz: the native static tables vs the Python oracle.
//!
//! Random inputs drive both implementations of the TT value curve and
//! the codex category tables; replies are compared byte-for-byte, each
//! side serialised with sorted keys through its Python-faithful encoder.
//! This is the guard for the divergences hand-picked cases miss: the
//! interpolation and 64-iteration bisection float paths, half-even
//! rounding at the 4dp boundary, and the breakdown's null/list shapes.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-services --features cross-language --test static_tables_differential
//!
//! The oracle interpreter is `$EO_ORACLE_PYTHON` if set, else the local
//! virtualenv (`.venv/Scripts/python.exe` on Windows, `.venv/bin/python`
//! elsewhere).
#![cfg(feature = "cross-language")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use eo_services::{codex_categories, tt_value_curve};
use eo_wire::normalizer::to_python_json;
use proptest::prelude::*;
use serde_json::json;

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

/// A long-lived oracle process driven one JSON request per line.
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
            .arg("backend.testing.static_tables_cli")
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

    fn ask(&mut self, request: &serde_json::Value) -> String {
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
        reply.trim_end().to_string()
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

/// The native value rendered exactly as the oracle's `json.dumps`
/// renders its reply (sorted keys, Python float repr).
fn native_json(value: &serde_json::Value) -> String {
    to_python_json(value, None)
}

/// The real snapshot catalogue through both stores: counts, searches,
/// id lookups, and the mob-suggestion pipeline, byte-compared.
#[test]
fn game_data_over_the_real_snapshot_matches() {
    let snapshot = repo_root().join("backend/data/snapshot");
    let store = eo_services::game_data_store::GameDataStore::new(&snapshot);
    let lookup = eo_services::mob_lookup_service::MobLookupService::new(&store);

    let counts_reply = oracle().lock().unwrap().ask(&json!({"op": "game_counts"}));
    assert_eq!(
        counts_reply,
        native_json(&serde_json::Value::Object(store.endpoint_counts()))
    );

    for (query, endpoint, limit) in [
        ("opalo", None, 50),
        ("atrox", Some("mobs"), 50),
        ("a", None, 25),
        ("herb box", None, 50),
        ("ZZZ-NO-MATCH", None, 50),
    ] {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "game_search", "query": query, "endpoint": endpoint, "limit": limit,
        }));
        let native = store.search_entities(query, endpoint, limit);
        assert_oracle_eq(&reply, &json!(native), &format!("game_search {query:?}"));
    }

    for (endpoint, id) in [
        ("weapons", json!(1)),
        ("mobs", json!("7")),
        ("skills", json!(99999)),
    ] {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "game_find", "endpoint": endpoint, "item_id": id,
        }));
        let native = store
            .find_entity(endpoint, &id)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_oracle_eq(&reply, &native, &format!("game_find {endpoint}/{id}"));
    }

    for (query, limit) in [("atrox", 10), ("young atrox", 5), ("dai", 10), ("  ", 10)] {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "mob_suggest", "query": query, "limit": limit,
        }));
        let native = lookup.search_mob_names(query, limit);
        assert_oracle_eq(&reply, &json!(native), &format!("mob_suggest {query:?}"));
    }

    for (species, maturity) in [
        ("Atrox", "Young"),
        ("Atrox", "Old"),
        ("Atrox", ""),
        ("Daikiba", "Young"),
        ("No Such Species", ""),
    ] {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "mob_has", "species": species, "maturity": maturity,
        }));
        let native = lookup.has_mob_name(species, maturity);
        assert_oracle_eq(
            &reply,
            &json!(native),
            &format!("mob_has {species}/{maturity}"),
        );
    }
}

fn assert_oracle_eq(reply: &str, native: &serde_json::Value, context: &str) {
    assert_eq!(reply, native_json(native), "{context} diverged");
}

#[test]
fn max_level_matches() {
    let reply = oracle()
        .lock()
        .unwrap()
        .ask(&json!({"op": "max_tt_curve_level"}));
    assert_eq!(
        reply,
        native_json(&json!(tt_value_curve::max_tt_curve_level()))
    );
}

#[test]
fn known_skill_categories_match() {
    for skill in ["Aim", "Courage", "Evade", "Zoology", "Food Technology", ""] {
        let reply = oracle()
            .lock()
            .unwrap()
            .ask(&json!({"op": "get_codex_category", "skill_name": skill}));
        assert_eq!(
            reply,
            native_json(&json!(codex_categories::get_codex_category(skill))),
            "category diverged for {skill:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn tt_value_at_matches(level in -100.0f64..25000.0) {
        let reply = oracle().lock().unwrap().ask(&json!({"op": "tt_value_at", "level": level}));
        prop_assert_eq!(reply, native_json(&json!(tt_value_curve::tt_value_at(level))));
    }

    #[test]
    fn tt_value_of_gain_matches(from in -10.0f64..21000.0, to in -10.0f64..21000.0) {
        let reply = oracle().lock().unwrap().ask(
            &json!({"op": "tt_value_of_gain", "from_level": from, "to_level": to}),
        );
        prop_assert_eq!(
            reply,
            native_json(&json!(tt_value_curve::tt_value_of_gain(from, to)))
        );
    }

    #[test]
    fn levels_for_tt_value_matches(from in 0.0f64..20000.0, ped in -10.0f64..5000.0) {
        let reply = oracle().lock().unwrap().ask(
            &json!({"op": "levels_for_tt_value", "from_level": from, "ped_value": ped}),
        );
        prop_assert_eq!(
            reply,
            native_json(&json!(tt_value_curve::levels_for_tt_value(from, ped)))
        );
    }

    #[test]
    fn rank_breakdown_matches(
        base_cost in 0.0f64..500.0,
        codex_type in prop::option::of(prop_oneof![
            Just("MobLooter".to_string()),
            Just("Other".to_string()),
        ]),
    ) {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "build_rank_breakdown",
            "base_cost": base_cost,
            "codex_type": codex_type,
        }));
        let native = codex_categories::build_rank_breakdown(base_cost, codex_type.as_deref());
        let native_value = serde_json::to_value(&native).unwrap();
        prop_assert_eq!(reply, native_json(&native_value));
    }
}
