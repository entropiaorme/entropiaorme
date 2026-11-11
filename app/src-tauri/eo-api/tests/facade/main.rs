//! The facade integration tests: every command family driven over the typed
//! facade, compiled as one test binary.
//!
//! One binary rather than one per family on purpose: each test binary links
//! its own copy of the whole application, so separate binaries multiplied
//! the link time of every test run by the number of families. A family is a
//! module here, and runs on its own by filter:
//!
//! ```text
//! cargo nextest run -p eo-api --test facade -E 'test(/^tracking_facade::/)'
//! ```

mod common;

mod analytics_facade;
mod character_facade;
mod codex_facade;
mod dev_facade;
mod equipment_facade;
mod facade_fixture_bench;
mod facade_microbench;
mod loadout_comparison_facade;
mod maps_facade;
mod market_facade;
mod mob_detection_facade;
mod quests_facade;
mod scan_facade;
mod session_definitions_facade;
mod session_exclusion_facade;
mod settings_facade;
mod skilling_routes_facade;
mod tracking_facade;

/// The facade tests stay one binary: a family added as its own file directly
/// under `tests/` would link another full copy of the application into every
/// test run. Add it as a module above instead.
#[test]
fn the_facade_tests_stay_one_binary() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let separate: Vec<String> = std::fs::read_dir(&tests)
        .expect("the tests directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        separate.is_empty(),
        "separate test binaries under tests/: {separate:?}; add each as a module of tests/facade/main.rs"
    );
}
