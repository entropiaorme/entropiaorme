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
use serde_json::{json, Map, Value};

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
    let store = eo_services::game_data_store::GameDataStore::new(&snapshot).unwrap();
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

    for (query, limit) in [
        ("atrox", 10),
        ("young atrox", 5),
        ("atrox young", 5),
        ("dai", 10),
        ("  ", 10),
    ] {
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

/// The character calculations over the real professions and skills
/// snapshots with deterministic synthetic level maps, byte-compared
/// against the backend implementation.
#[test]
fn character_calc_over_the_real_snapshot_matches() {
    use eo_services::character_calc;

    let snapshot = repo_root().join("backend/data/snapshot");
    let store = eo_services::game_data_store::GameDataStore::new(&snapshot).unwrap();
    let professions = store.get_entities("professions");
    let skills_data = store.get_entities("skills");
    let ranks = store.get_entities("skill_ranks")[0]["table"]["rows"]
        .as_array()
        .cloned()
        .expect("the skill_ranks snapshot carries table.rows");
    assert!(!ranks.is_empty(), "the rank sweep must drive real rows");

    // Deterministic level maps over the real skill names: a sparse map,
    // a dense mid-range map, and a high-level map with fractional
    // levels (attribute skills included via their real names).
    let mut level_maps: Vec<Map<String, serde_json::Value>> = vec![Map::new(); 3];
    for (i, skill) in skills_data.iter().enumerate() {
        let Some(name) = skill.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if i % 3 == 0 {
            level_maps[0].insert(name.to_string(), json!(((i * 137) % 4000) as f64 * 0.5));
        }
        level_maps[1].insert(name.to_string(), json!(((i * 61) % 2500) as f64 + 0.25));
        level_maps[2].insert(name.to_string(), json!(((i * 211) % 12000) as f64 * 1.01));
    }

    for (mi, levels) in level_maps.iter().enumerate() {
        let levels_value = serde_json::Value::Object(levels.clone());

        // Every profession's level, in one shot.
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "all_profession_levels",
            "skill_levels": levels_value,
            "professions": professions,
        }));
        let native = character_calc::all_profession_levels(levels, professions);
        assert_oracle_eq(
            &reply,
            &serde_json::Value::Object(native),
            &format!("all_profession_levels map {mi}"),
        );

        // Optimizers on a spread of professions.
        for pi in [0usize, 7, 23, 61] {
            let Some(profession) = professions.get(pi) else {
                continue;
            };
            let reply = oracle().lock().unwrap().ask(&json!({
                "op": "profession_skill_optimizer",
                "skill_levels": levels_value,
                "profession": profession,
            }));
            let native = character_calc::profession_skill_optimizer(levels, profession);
            assert_oracle_eq(
                &reply,
                &native,
                &format!("skill_optimizer map {mi} prof {pi}"),
            );

            // A current-relative target keeps the greedy loop live on
            // every map rather than early-returning on high levels.
            let target = character_calc::profession_level(levels, profession) + 0.75;
            let reply = oracle().lock().unwrap().ask(&json!({
                "op": "profession_path_optimizer",
                "skill_levels": levels_value,
                "profession": profession,
                "target_level": target,
            }));
            let native =
                character_calc::profession_path_optimizer(levels, profession, Some(target), None)
                    .unwrap();
            assert_oracle_eq(&reply, &native, &format!("path target map {mi} prof {pi}"));

            let reply = oracle().lock().unwrap().ask(&json!({
                "op": "profession_path_optimizer",
                "skill_levels": levels_value,
                "profession": profession,
                "ped_budget": 250.0,
            }));
            let native =
                character_calc::profession_path_optimizer(levels, profession, None, Some(250.0))
                    .unwrap();
            assert_oracle_eq(&reply, &native, &format!("path budget map {mi} prof {pi}"));
        }

        // HP figures over the real skills data.
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "calculate_hp",
            "skill_levels": levels_value,
            "skills_data": skills_data,
        }));
        let native = character_calc::calculate_hp(levels, skills_data);
        assert_oracle_eq(&reply, &json!(native), &format!("calculate_hp map {mi}"));

        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "hp_skill_optimizer",
            "skill_levels": levels_value,
            "skills_data": skills_data,
        }));
        let native = character_calc::hp_skill_optimizer(levels, skills_data);
        assert_oracle_eq(&reply, &native, &format!("hp_skill_optimizer map {mi}"));
    }

    // Skill ranks across the real threshold table.
    for level in [0.0, 0.5, 1.0, 24.99, 25.0, 100.0, 6000.0, 100000.0] {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "skill_rank", "level": level, "ranks": ranks,
        }));
        let native = character_calc::skill_rank(level, &ranks);
        assert_oracle_eq(&reply, &json!(native), &format!("skill_rank {level}"));
    }

    // Codex helpers across real skill names (with and without category).
    for name in ["Rifle", "Anatomy", "Agility", "No Such Skill"] {
        for level in [0.0, 199.5, 1234.5] {
            let reply = oracle().lock().unwrap().ask(&json!({
                "op": "codex_next_reward", "skill_name": name, "current_level": level,
            }));
            let native = character_calc::codex_next_reward(name, level);
            assert_oracle_eq(
                &reply,
                &json!(native),
                &format!("next_reward {name} {level}"),
            );

            let reply = oracle().lock().unwrap().ask(&json!({
                "op": "codex_tier_progress", "skill_name": name, "current_level": level,
            }));
            let native = character_calc::codex_tier_progress(name, level);
            assert_oracle_eq(
                &reply,
                &json!(native),
                &format!("tier_progress {name} {level}"),
            );
        }
    }
}

/// The scan-geometry anchors built from the real tracked calibration
/// file, and drift summaries over curated level maps, byte-compared.
/// (The capture-region arithmetic is pinned by identical hand-computed
/// values on both sides rather than an oracle op: serving it would
/// mean patching the backend's window lookup inside the oracle for six
/// integer operations, more fragility than assurance.)
#[test]
fn scan_geometry_and_drift_match_the_oracle() {
    use eo_services::scan_presets::{PanelAnchor, ScanPresets};

    let geometry_path = repo_root().join("backend/data/panel_geometry.json");
    assert!(geometry_path.exists(), "the calibration file is tracked");
    let presets = ScanPresets::new(&geometry_path);
    assert!(
        !presets.skill.cells.is_empty(),
        "the real calibration populates skill cells"
    );

    let anchor_json = |anchor: &PanelAnchor| {
        let mut cells = Map::new();
        for (name, cell) in &anchor.cells {
            cells.insert(
                name.clone(),
                json!({
                    "x_left": cell.x_left,
                    "x_right": cell.x_right,
                    "first_y_top": cell.first_y_top,
                    "last_y_top": cell.last_y_top,
                    "height": cell.height,
                }),
            );
        }
        json!({
            "width": anchor.width,
            "height": anchor.height,
            "right_offset": anchor.right_offset,
            "bottom_offset": anchor.bottom_offset,
            "n_rows": anchor.n_rows,
            "cells": cells,
        })
    };
    let reply = oracle()
        .lock()
        .unwrap()
        .ask(&json!({"op": "panel_anchors"}));
    let native = json!({
        "skill": anchor_json(&presets.skill),
        "profession": anchor_json(&presets.profession),
        "repair": anchor_json(&presets.repair),
    });
    assert_oracle_eq(&reply, &native, "panel_anchors");

    let drift_cases = [
        (json!({}), json!({})),
        (json!({"Rifle": 100.0}), json!({"Anatomy": 50.0})),
        (
            json!({"Rifle": 100.0, "Anatomy": 50.0, "Only Tracked": 5.0}),
            json!({"Rifle": 104.5, "Anatomy": 48.25, "Extra": 9.0}),
        ),
        (
            json!({"Tiny": 0.0, "Zed": 3.0, "Abe": 3.0}),
            json!({"Tiny": 0.5, "Zed": 5.0, "Abe": 1.0}),
        ),
    ];
    for (index, (tracked, scanned)) in drift_cases.iter().enumerate() {
        let reply = oracle().lock().unwrap().ask(&json!({
            "op": "summarize_level_drift",
            "tracked_levels": tracked,
            "scanned_levels": scanned,
        }));
        let native = eo_services::scan_drift::summarize_level_drift(
            tracked.as_object().unwrap(),
            scanned.as_object().unwrap(),
        )
        .unwrap_or(serde_json::Value::Null);
        assert_oracle_eq(&reply, &native, &format!("drift case {index}"));
    }
}

/// Damage attribution and loot filtering over curated profile sets,
/// sweeps, and key-normalisation cases, byte-compared.
#[test]
fn tool_inference_and_loot_filter_match_the_oracle() {
    use eo_services::tool_inference::DamageAttributor;

    let profile_sets: Vec<Value> = vec![
        json!([
            {"name": "Pistol", "min_damage": 5.0, "max_damage": 10.0, "cost_per_shot": 0.05, "role": "small_weapon"},
            {"name": "Cannon", "min_damage": 20.0, "max_damage": 40.0, "cost_per_shot": 0.2, "role": "big_weapon"},
        ]),
        json!([
            {"name": "Wide", "min_damage": 0.5, "max_damage": 100.0, "cost_per_shot": 0.1},
            {"name": "Narrow", "min_damage": 5.0, "max_damage": 15.0, "cost_per_shot": 0.2},
        ]),
        json!([
            {"name": "Beta", "min_damage": 5.0, "max_damage": 15.0, "cost_per_shot": 0.1},
            {"name": "Alpha", "min_damage": 10.0, "max_damage": 20.0, "cost_per_shot": 0.2},
        ]),
        json!([]),
    ];
    let sweep = [
        0.0, 0.5, 4.9, 5.0, 7.0, 10.0, 12.0, 15.0, 19.9, 20.0, 25.0, 30.0, 40.0, 55.0, 90.0, 120.0,
        121.0,
    ];

    let mut oracle = oracle().lock().unwrap();
    for (set_index, profiles) in profile_sets.iter().enumerate() {
        let mut attributor = DamageAttributor::new();
        for profile in profiles.as_array().unwrap() {
            attributor.add_weapon_profile(
                profile["name"].as_str().unwrap(),
                profile["min_damage"].as_f64().unwrap(),
                profile["max_damage"].as_f64().unwrap(),
                profile
                    .get("base_damage")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                profile
                    .get("cost_per_shot")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                profile.get("role").and_then(Value::as_str),
            );
        }
        for amount in sweep {
            for critical in [false, true] {
                let reply = oracle.ask(&json!({
                    "op": "match_damage",
                    "profiles": profiles,
                    "amount": amount,
                    "critical": critical,
                }));
                let native = match attributor.match_damage(amount, critical) {
                    None => Value::Null,
                    Some(hit) => json!({
                        "tool_name": hit.tool_name,
                        "cost_per_shot": hit.cost_per_shot,
                    }),
                };
                assert_oracle_eq(
                    &reply,
                    &native,
                    &format!("match_damage set {set_index} amount {amount} crit {critical}"),
                );
            }
        }
    }

    // Loot filtering: keys, fallbacks, blanks.
    let cases: Vec<(Value, Value)> = vec![
        (json!("Universal Ammo"), Value::Null),
        (json!("  universal\tAMMO "), Value::Null),
        (json!("Animal Muscle Oil"), Value::Null),
        (json!("Shrapnel"), json!(["Shrapnel", "  Vibrant  Sweat "])),
        (
            json!("vibrant sweat"),
            json!(["Shrapnel", "  Vibrant  Sweat "]),
        ),
        (json!("Universal Ammo"), json!(["Shrapnel"])),
        (json!("Wool"), json!(["", "  "])),
    ];
    for (index, (item, blacklist)) in cases.iter().enumerate() {
        let reply = oracle.ask(&json!({
            "op": "is_tracked_loot",
            "item_name": item,
            "blacklist": blacklist,
        }));
        let names: Option<Vec<&str>> = blacklist
            .as_array()
            .map(|list| list.iter().filter_map(Value::as_str).collect());
        let native_blacklist = eo_services::loot_filter::normalize_blacklist(names);
        let native =
            eo_services::loot_filter::is_tracked_loot(item.as_str().unwrap(), &native_blacklist);
        assert_oracle_eq(&reply, &json!(native), &format!("loot case {index}"));
    }

    let reply = oracle.ask(&json!({
        "op": "normalize_blacklist",
        "names": ["Shrapnel", "  Vibrant  Sweat ", ""],
    }));
    let native: Vec<String> = eo_services::loot_filter::normalize_blacklist(Some(vec![
        "Shrapnel",
        "  Vibrant  Sweat ",
        "",
    ]))
    .into_iter()
    .collect();
    assert_oracle_eq(&reply, &json!(native), "normalize_blacklist");
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
