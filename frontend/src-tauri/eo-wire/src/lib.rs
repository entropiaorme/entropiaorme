//! Wire-format contracts for the EntropiaOrme backend.
//!
//! This crate carries the HTTP response and event envelope types and their
//! serialisation rules. The byte-level contract each type reproduces is the
//! frozen wire encoding the backend emits.
//!
//! The equivalence emitters are [`normalizer`] (the shared canonicaliser),
//! [`fingerprint`] (the event-stream JSONL), [`db_snapshot`] (the DB-state
//! snapshot), and [`http_fingerprint`] (the HTTP response goldens). Each was a
//! byte-exact port of its Python testing-oracle counterpart; with the oracle
//! retired, they are asserted against the committed goldens by the hermetic
//! tests, with no second implementation present.
//!
//! The wire-contract spine sits beside them: [`domain_events`] (the typed
//! frontend-facing event union, gated against the committed event-schema
//! snapshot), [`bus`] (the monomorphic domain-event channel), and [`sse`]
//! (the event-stream fan-out hub with its drop-oldest delivery shaping).

pub mod bus;
pub mod db_snapshot;
pub mod domain_events;
pub mod fingerprint;
pub mod http_fingerprint;
pub mod metrics;
pub mod models;
pub mod normalizer;
pub mod sse;

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
