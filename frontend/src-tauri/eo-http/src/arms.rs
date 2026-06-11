//! Runtime per-route arm selection.
//!
//! Every route the native backend takes over keeps both implementations
//! alive for the duration of the hybrid: the native handler and the live
//! sidecar behind the reverse proxy. The arm override map lets an
//! already-shipped build steer any flipped route back to the sidecar
//! without a rebuild: the router consults the map at request time, so a
//! misbehaving native route is one override entry away from the known-good
//! implementation.
//!
//! Override sources, later entries winning:
//! 1. A persisted JSON object file (route path -> "native" | "proxy"),
//!    path supplied by the shell.
//! 2. The `ENTROPIAORME_ROUTE_ARMS` environment variable, a comma-separated
//!    `route=arm` list (e.g. `/api/health=proxy`).
//!
//! Routes absent from the map run their default arm: native once flipped.

use std::collections::HashMap;
use std::path::Path;

/// Which implementation serves a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// The in-process native handler.
    Native,
    /// The reverse-proxied Python sidecar.
    Proxy,
}

impl Arm {
    fn parse(value: &str) -> Option<Arm> {
        match value.trim().to_ascii_lowercase().as_str() {
            "native" => Some(Arm::Native),
            "proxy" => Some(Arm::Proxy),
            _ => None,
        }
    }
}

/// The per-route override map. Absent routes default to [`Arm::Native`].
#[derive(Debug, Default, Clone)]
pub struct ArmOverrides {
    map: HashMap<String, Arm>,
}

impl ArmOverrides {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse the `ENTROPIAORME_ROUTE_ARMS` format: `route=arm[,route=arm...]`.
    /// Malformed entries are skipped; an operator typo must never take the
    /// router down.
    pub fn parse_env_value(value: &str) -> Self {
        let mut map = HashMap::new();
        for entry in value.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((route, arm)) = entry.split_once('=') else {
                continue;
            };
            let route = route.trim();
            let Some(arm) = Arm::parse(arm) else {
                continue;
            };
            if route.starts_with('/') {
                map.insert(route.to_string(), arm);
            }
        }
        Self { map }
    }

    /// Read a persisted JSON object of `{"<route>": "native"|"proxy"}`.
    /// Unreadable or malformed files yield the empty map: the persisted
    /// override is an operator convenience, never a boot blocker.
    pub fn from_json_file(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::empty();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Self::empty();
        };
        let Some(object) = value.as_object() else {
            return Self::empty();
        };
        let mut map = HashMap::new();
        for (route, arm) in object {
            if let Some(arm) = arm.as_str().and_then(Arm::parse) {
                if route.starts_with('/') {
                    map.insert(route.clone(), arm);
                }
            }
        }
        Self { map }
    }

    /// Layer `other` on top of `self` (entries in `other` win).
    pub fn overlaid(mut self, other: ArmOverrides) -> Self {
        self.map.extend(other.map);
        self
    }

    /// The arm serving `route` right now.
    pub fn arm_for(&self, route: &str) -> Arm {
        self.map.get(route).copied().unwrap_or(Arm::Native)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_routes_default_to_native() {
        assert_eq!(ArmOverrides::empty().arm_for("/api/health"), Arm::Native);
    }

    #[test]
    fn env_value_parses_routes_and_arms() {
        let overrides =
            ArmOverrides::parse_env_value("/api/health=proxy, /api/tracking/snapshot=native");
        assert_eq!(overrides.arm_for("/api/health"), Arm::Proxy);
        assert_eq!(overrides.arm_for("/api/tracking/snapshot"), Arm::Native);
        assert_eq!(overrides.arm_for("/api/other"), Arm::Native);
    }

    #[test]
    fn malformed_env_entries_are_skipped_not_fatal() {
        let overrides = ArmOverrides::parse_env_value(
            "garbage,/api/a=warp,=proxy,/api/b=proxy,no-slash=proxy,,",
        );
        assert_eq!(overrides.arm_for("/api/a"), Arm::Native);
        assert_eq!(overrides.arm_for("/api/b"), Arm::Proxy);
        assert_eq!(overrides.arm_for("garbage"), Arm::Native);
    }

    #[test]
    fn json_file_roundtrip_and_env_overlay_wins() {
        let dir = std::env::temp_dir().join("eo-http-arms-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("route_arms.json");
        std::fs::write(&path, r#"{"/api/health": "proxy", "/api/x": "proxy"}"#).unwrap();
        let file = ArmOverrides::from_json_file(&path);
        assert_eq!(file.arm_for("/api/health"), Arm::Proxy);

        let merged = file.overlaid(ArmOverrides::parse_env_value("/api/health=native"));
        assert_eq!(merged.arm_for("/api/health"), Arm::Native);
        assert_eq!(merged.arm_for("/api/x"), Arm::Proxy);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unreadable_or_malformed_file_yields_empty() {
        let missing = ArmOverrides::from_json_file(Path::new("/nonexistent/route_arms.json"));
        assert!(missing.is_empty());
        let dir = std::env::temp_dir().join("eo-http-arms-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("malformed.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(ArmOverrides::from_json_file(&path).is_empty());
        std::fs::remove_file(&path).ok();
    }
}
