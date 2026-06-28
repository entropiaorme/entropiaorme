//! Domain services for the EntropiaOrme backend.
//!
//! This crate carries the backend's service layer (cost accounting,
//! tracking, scans, quests, and the rest), each a byte-exact port of its
//! original Python service.
//!
//! [`cost_engine`] is the pure-arithmetic leaf its per-unit `cargo test`
//! loop runs on.

pub mod character_calc;
pub mod chatlog_parser;
pub mod chatlog_watcher;
pub mod clock;
pub mod codex;
pub mod codex_categories;
pub mod config_service;
pub mod cost_engine;
pub mod db;
pub mod difflib;
pub mod eu_window;
pub mod event_bus;
pub mod fingerprint_recorder;
pub mod fuzzy_match;
pub mod game_data_store;
pub mod hotbar_listener;
pub mod keystroke_source;
pub mod loot_filter;
pub mod mob_lookup_service;
pub mod observability_config;
pub mod ocr_engine;
pub mod paths;
pub mod quests;
pub mod repair_ocr;
pub mod scan_completion;
pub mod scan_drift;
pub mod scan_presets;
pub mod screen_capture;
pub mod session_summary;
pub mod skill_panel;
pub mod skill_scan_manual;
pub mod skill_tracker;
pub mod spacebar_capture_listener;
pub mod tool_inference;
pub mod tracker;
pub mod tracking_models;
pub mod trifecta_service;
pub mod tt_value_curve;

/// Identifies this crate in diagnostics and smoke checks.
pub fn crate_name() -> &'static str {
    "eo-services"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::crate_name(), "eo-services");
    }
}
