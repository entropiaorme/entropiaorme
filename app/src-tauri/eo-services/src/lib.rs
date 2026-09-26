//! Domain services for the EntropiaOrme backend.
//!
//! This crate is the backend's service layer (cost accounting,
//! tracking, scans, quests, and the rest): all domain logic and all
//! SQL, behind seams the replay corpus drives deterministically.
//!
//! [`cost_engine`] is the pure-arithmetic leaf its per-unit `cargo test`
//! loop runs on.

pub mod activity_recommender;
pub mod analytics;
pub mod attack_rate;
pub mod auction_fee;
pub mod auction_fee_research;
pub mod bus_events;
pub mod character_calc;
pub mod chatlog_parser;
pub mod chatlog_time;
pub mod chatlog_watcher;
pub mod clock;
pub mod codex;
pub mod codex_categories;
pub mod config_service;
pub mod coord_capture;
pub mod cost_engine;
pub mod daily_rollup;
pub mod db;
pub mod difflib;
pub mod equipment_pricing;
pub mod eu_window;
pub mod event_bus;
pub mod expected_hunting;
pub mod fingerprint_recorder;
pub mod fuzzy_match;
pub mod game_data_store;
pub mod harvest_yield;
pub mod healing_profile;
pub mod healing_review;
pub mod hotbar_listener;
pub mod keyset;
pub mod keystroke_source;
pub mod loot_filter;
pub mod maintenance;
pub mod map_pins;
pub mod market_paste;
pub mod market_service;
pub mod mob_lookup_service;
pub mod navigation;
pub mod observability_config;
pub mod ocr_engine;
pub mod ocr_text;
pub mod passive_effects;
pub mod paths;
pub mod ped;
pub mod pin_configs;
pub mod planet_maps;
pub mod protection;
pub mod quests;
pub mod repair_ocr;
pub mod sale_window_ocr;
pub mod scan_completion;
pub mod scan_drift;
pub mod scan_presets;
pub mod screen_capture;
pub mod session_definitions;
pub mod session_rollup;
pub mod session_summary;
pub mod skill_panel;
pub mod skill_scan_manual;
pub mod skill_tracker;
pub mod spacebar_capture_listener;
pub mod stock_allocation;
pub mod time;
pub mod tracker;
pub mod tracking_models;
pub mod tracking_reads;
pub mod tt_value_curve;
pub mod weapon_effect;
pub mod weapon_review;

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
