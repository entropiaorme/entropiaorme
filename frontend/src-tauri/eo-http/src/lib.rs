//! HTTP substrate for the EntropiaOrme backend.
//!
//! Skeleton member: this crate will carry the in-process HTTP server,
//! router, and middleware through which backend routes move out of the
//! Python sidecar one at a time. The route-by-route takeover plan is
//! documented in `backend/architecture/PORT-READINESS.md`.

/// Identifies this crate in diagnostics and smoke checks.
pub fn crate_name() -> &'static str {
    "eo-http"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::crate_name(), "eo-http");
    }
}
