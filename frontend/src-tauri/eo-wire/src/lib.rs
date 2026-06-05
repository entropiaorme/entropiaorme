//! Wire-format contracts for the EntropiaOrme backend.
//!
//! Skeleton member: this crate will carry the HTTP response and event
//! envelope types and their serialisation rules as backend routes move
//! into the shell process. The byte-level contract each type must
//! reproduce is documented in `backend/architecture/PORTING-RULEBOOK.md`.

/// Identifies this crate in diagnostics and smoke checks.
pub fn crate_name() -> &'static str {
    "eo-wire"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::crate_name(), "eo-wire");
    }
}
