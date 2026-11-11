//! The interface profile: everything the readers know about how this
//! version of the client draws its interface.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::field::ReadProfile;
use super::locate::{TitleProfile, WindowProfile};
use super::mob_history::MobHistoryProfile;
use super::mob_names::MobNameProfile;
use super::plates::PlateProfile;
use super::radar::RadarProfile;
use super::table::RuleProfile;
use super::text::TextProfile;
use super::window::ChromeProfile;

/// The profile format this build reads.
pub const PROFILE_FORMAT: u32 = 1;

/// The shortest interval the creature watcher may look at the screen at,
/// whatever a profile says: a floor on what the watcher may cost the
/// machine the game is running on, not a measurement of the client.
pub const MIN_PULSE_MS: i64 = 250; // not layout: a floor on the profile's cadence

/// One client interface, described as data.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiProfile {
    /// Format of this file; a build refuses a format it does not know.
    pub format: u32,
    /// Which client interface the measurements were taken from.
    pub client_interface: String,
    /// The frame every client window shares.
    pub chrome: ChromeProfile,
    /// How lettering inside a window groups into runs.
    pub text: TextProfile,
    /// What the rule lines of a table look like.
    pub rules: RuleProfile,
    /// How runs are handed to the recogniser and labels matched.
    pub read: ReadProfile,
    /// Where a window's title is and how strictly it must match.
    pub titles: TitleProfile,
    /// The kinds of window the readers know, by key.
    pub windows: BTreeMap<String, WindowProfile>,
    /// The radar, and the coordinates printed under it.
    pub radar: RadarProfile,
    /// Creatures' nameplates: their health bars and their lettering.
    pub plates: PlateProfile,
    /// How strictly a name read off a plate must match a known creature.
    pub mob_names: MobNameProfile,
    /// How plates read over time are grouped into fights and voted on.
    pub mob_history: MobHistoryProfile,
}

/// Why a profile could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("the interface profile could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("the interface profile is not valid: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("the interface profile has format {found}; this build reads format {expected}")]
    Format { found: u32, expected: u32 },
}

impl UiProfile {
    /// Parse a profile from its JSON text.
    pub fn from_json(text: &str) -> Result<Self, ProfileError> {
        let mut profile: UiProfile = serde_json::from_str(text)?;
        if profile.format != PROFILE_FORMAT {
            return Err(ProfileError::Format {
                found: profile.format,
                expected: PROFILE_FORMAT,
            });
        }
        profile.mob_history.pulse_ms = profile.mob_history.pulse_ms.max(MIN_PULSE_MS);
        Ok(profile)
    }

    /// Load a profile from a file.
    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_text() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../entropia-orme/resources/ui_profile.json"),
        )
        .expect("the bundled profile reads")
    }

    #[test]
    fn the_bundled_profile_loads() {
        let profile = UiProfile::from_json(&bundled_text()).expect("the bundled profile loads");
        assert_eq!(profile.format, PROFILE_FORMAT);
        assert!(profile.windows.contains_key("skills"));
    }

    /// Every object in `value` that a profile struct deserialises (all of
    /// them but the maps keyed by name: the windows, and a window's
    /// fields), by its path.
    fn struct_paths(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                let keyed_by_name = path == "/windows"
                    || (path.starts_with("/windows/") && path.ends_with("/fields"));
                if !keyed_by_name {
                    out.push(path.to_string());
                }
                for (key, child) in map {
                    struct_paths(child, &format!("{path}/{key}"), out);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    struct_paths(child, &format!("{path}/{i}"), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn an_entry_the_profile_does_not_know_is_refused_at_every_level() {
        let bundled: serde_json::Value = serde_json::from_str(&bundled_text()).unwrap();
        let mut paths = Vec::new();
        struct_paths(&bundled, "", &mut paths);
        assert!(paths.len() > 40, "{paths:?}");
        for path in paths {
            let mut stale = bundled.clone();
            stale
                .pointer_mut(&path)
                .and_then(serde_json::Value::as_object_mut)
                .expect("an object")
                .insert("misspelt_entry".to_string(), serde_json::Value::Null);
            match UiProfile::from_json(&stale.to_string()) {
                Err(ProfileError::Parse(error)) => assert!(
                    error.to_string().contains("unknown field `misspelt_entry`"),
                    "{path}: {error}"
                ),
                other => panic!("{path}: an unknown entry was accepted: {other:?}"),
            }
        }
    }

    #[test]
    fn the_watcher_cadence_is_held_to_its_floor() {
        let text = bundled_text();
        assert_eq!(
            UiProfile::from_json(&text).unwrap().mob_history.pulse_ms,
            1000
        );
        for (written, kept) in [
            (10, MIN_PULSE_MS),
            (0, MIN_PULSE_MS),
            (-5, MIN_PULSE_MS),
            (400, 400),
        ] {
            let quick = text.replace("\"pulse_ms\": 1000", &format!("\"pulse_ms\": {written}"));
            assert_ne!(quick, text);
            assert_eq!(
                UiProfile::from_json(&quick).unwrap().mob_history.pulse_ms,
                kept
            );
        }
    }
}
