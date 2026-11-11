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

/// One client interface, described as data.
#[derive(Debug, Clone, Deserialize)]
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
        let profile: UiProfile = serde_json::from_str(text)?;
        if profile.format != PROFILE_FORMAT {
            return Err(ProfileError::Format {
                found: profile.format,
                expected: PROFILE_FORMAT,
            });
        }
        Ok(profile)
    }

    /// Load a profile from a file.
    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }
}
