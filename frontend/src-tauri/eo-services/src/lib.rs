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

pub mod cost_engine;
pub mod db;

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
