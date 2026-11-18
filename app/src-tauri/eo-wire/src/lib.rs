//! The wire contracts and the replay-golden emitters for the
//! EntropiaOrme backend.
//!
//! Two groups live here. The **live contracts**: [`domain_events`] (the
//! typed frontend-facing event union), [`bus`] (the monomorphic
//! domain-event channel with its drop-behind delivery shaping), and
//! [`metrics`] (the in-process metrics snapshot shapes).
//!
//! The **replay-golden emitters**: [`normalizer`] (the shared
//! canonicaliser), [`fingerprint`] (the event-stream JSONL), and
//! [`db_snapshot`] (the DB-state snapshot). The corpus replay tests render
//! a replayed scenario through them and compare it with the scenario's
//! committed goldens.
//!
//! One live production policy also lives in [`normalizer`]: the
//! [`normalizer::round_half_even`] rounding and the Python-format JSON
//! writers are consumed by services at runtime (cost figures, timestamp
//! strings, settings and session-summary persistence), not only by the
//! emitters.

pub mod bus;
pub mod db_snapshot;
pub mod domain_events;
pub mod fingerprint;
pub mod metrics;
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
