//! Domain services for the EntropiaOrme backend.
//!
//! Skeleton member: this crate will carry the backend's service layer
//! (cost accounting, tracking, scans, quests, and the rest) as each
//! service is ported from the Python implementation. Porting rules and
//! the per-service equivalence obligations are documented in
//! `backend/architecture/PORTING-RULEBOOK.md`.

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
