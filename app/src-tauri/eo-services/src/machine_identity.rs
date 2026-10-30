//! A stable identity for the machine the app runs on, so a setting that
//! describes the machine rather than the player (keyless tracking) can be
//! held to the machine it was turned on for. Read once at composition and
//! injected; nothing else reads it.
//!
//! Linux uses the systemd/D-Bus machine id, Windows the computer name. A
//! host with neither yields `None`, and a machine-scoped setting then stays
//! off.

/// The current machine's identity, when one can be read.
pub fn current() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        ["/etc/machine-id", "/var/lib/dbus/machine-id"]
            .iter()
            .find_map(|path| normalise(&std::fs::read_to_string(path).ok()?))
    }
    #[cfg(windows)]
    {
        normalise(&std::env::var("COMPUTERNAME").ok()?)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// A trimmed, non-empty identity, or `None`.
fn normalise(raw: &str) -> Option<String> {
    let id = raw.trim();
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_trimmed_and_blank_ones_refused() {
        assert_eq!(normalise("abc123\n").as_deref(), Some("abc123"));
        assert_eq!(normalise("  \n"), None);
        assert_eq!(normalise(""), None);
    }
}
