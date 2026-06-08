//! Wire-format contracts for the EntropiaOrme backend.
//!
//! Skeleton member: this crate will carry the HTTP response and event
//! envelope types and their serialisation rules as backend routes move
//! into the shell process. The byte-level contract each type must
//! reproduce is documented in `backend/architecture/PORTING-RULEBOOK.md`.
//!
//! The first landed surfaces are the cross-language equivalence runner's
//! emitters: [`normalizer`] (the shared canonicaliser), [`fingerprint`] (the
//! event-stream JSONL), [`db_snapshot`] (the DB-state snapshot), and
//! [`http_fingerprint`] (the HTTP response goldens). Each is a byte-exact port
//! of its `backend/testing/` counterpart, asserted against the committed Python
//! goldens by the runner.

pub mod db_snapshot;
pub mod fingerprint;
pub mod http_fingerprint;
pub mod normalizer;

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
