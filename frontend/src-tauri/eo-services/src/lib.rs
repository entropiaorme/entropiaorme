//! Domain services for the EntropiaOrme backend.
//!
//! Skeleton member: this crate will carry the backend's service layer
//! (cost accounting, tracking, scans, quests, and the rest) as each
//! service is ported from the Python implementation. Porting rules and
//! the per-service equivalence obligations are documented in
//! `backend/architecture/PORTING-RULEBOOK.md`.
//!
//! First ported service: [`cost_engine`], the pure-arithmetic leaf the
//! equivalence runner proves its per-unit `cargo test` loop on.

pub mod character_calc;
pub mod chatlog_parser;
pub mod chatlog_watcher;
pub mod clock;
pub mod codex_categories;
pub mod config_service;
pub mod cost_engine;
pub mod db;
pub mod eu_window;
pub mod event_bus;
pub mod fingerprint_recorder;
pub mod game_data_store;
pub mod hotbar_listener;
pub mod keystroke_source;
pub mod loot_filter;
pub mod mob_lookup_service;
pub mod scan_drift;
pub mod scan_presets;
pub mod session_summary;
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
