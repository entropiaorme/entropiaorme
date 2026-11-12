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
    #[error("the interface profile is not consistent: {0}")]
    Inconsistent(String),
}

/// Why no entry of a per-scale list serves a surface. The client's
/// lettering does not scale uniformly (it hints a glyph's height to whole
/// pixels while its width follows the scale), so measures stated in
/// lettering heights hold at one interface scale only, and a surface
/// that needs them is read with the entry for the scale it is drawn at.
/// That scale is decided from a measure that scales exactly and is taken
/// from geometry alone, before any value is read: never by trying each
/// entry and keeping whichever makes a value pass, which would let a
/// misreading choose its own measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScaleError {
    /// No entry claims what was measured: the surface is drawn at an
    /// interface scale the profile has not been measured at.
    #[error("the interface scale is not in the profile: no entry claims {measured} px")]
    NotListed { measured: i32 },
    /// More than one entry claims it.
    #[error("the interface scale is ambiguous: more than one entry claims {measured} px")]
    Ambiguous { measured: i32 },
    /// What the scale is told by is covered or out of view, so the scale
    /// is not known and no entry can be chosen.
    #[error("the interface scale could not be measured: what tells it is covered or out of view")]
    Unmeasured,
}

/// The one entry of `entries` whose claimed range (inclusive, in pixels)
/// holds `measured`, with its index.
pub fn claiming<T>(
    entries: &[T],
    measured: i32,
    claim: impl Fn(&T) -> [i32; 2],
) -> Result<(usize, &T), ScaleError> {
    let mut found = entries.iter().enumerate().filter(|(_, entry)| {
        let [least, most] = claim(entry);
        (least..=most).contains(&measured)
    });
    match (found.next(), found.next()) {
        (Some(one), None) => Ok(one),
        (None, _) => Err(ScaleError::NotListed { measured }),
        (Some(_), Some(_)) => Err(ScaleError::Ambiguous { measured }),
    }
}

/// The index of the entry of `entries` whose claimed range lies nearest
/// `measured`: what a measuring tool starts from at a scale no entry
/// claims. Never a reader's choice, which is [`claiming`]'s or none.
#[doc(hidden)]
pub fn nearest_to_measure<T>(
    entries: &[T],
    measured: i32,
    claim: impl Fn(&T) -> [i32; 2],
) -> usize {
    (0..entries.len())
        .min_by_key(|i| {
            let [least, most] = claim(&entries[*i]);
            (least - measured).max(measured - most).max(0)
        })
        .unwrap_or(0)
}

/// Whether the claimed ranges of a per-scale list are each well formed
/// and pairwise apart, so that whatever is measured is claimed by one
/// entry at most; `what` names the list in the message.
pub fn claims_apart<T>(
    entries: &[T],
    what: &str,
    claim: impl Fn(&T) -> [i32; 2],
) -> Result<(), String> {
    if entries.is_empty() {
        return Err(format!("{what} lists no scale"));
    }
    for (i, entry) in entries.iter().enumerate() {
        let [least, most] = claim(entry);
        if least > most {
            return Err(format!(
                "{what}[{i}] claims [{least}, {most}], an empty range"
            ));
        }
        for (j, other) in entries.iter().enumerate().skip(i + 1) {
            let [other_least, other_most] = claim(other);
            if least <= other_most && other_least <= most {
                return Err(format!(
                    "{what}[{i}] and {what}[{j}] claim overlapping ranges"
                ));
            }
            // A pixel left unclaimed between two entries: a measure one
            // pixel off its scale's is refused, never taken for the next.
            if least == other_most + 1 || other_least == most + 1 {
                return Err(format!(
                    "{what}[{i}] and {what}[{j}] claim adjacent ranges, with no pixel between them"
                ));
            }
        }
    }
    Ok(())
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
        claims_apart(&profile.read.field_scales, "read.field_scales", |s| {
            s.marker_height
        })
        .map_err(ProfileError::Inconsistent)?;
        for (key, window) in &profile.windows {
            if let Some(table) = &window.skills_table {
                claims_apart(
                    &table.scales,
                    &format!("windows.{key}.skills_table.scales"),
                    |s| s.row_pitch,
                )
                .map_err(ProfileError::Inconsistent)?;
            }
            if let Some(queue) = &window.sale_queue {
                claims_apart(
                    &queue.scales,
                    &format!("windows.{key}.sale_queue.scales"),
                    |s| s.header_height,
                )
                .map_err(ProfileError::Inconsistent)?;
            }
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
    fn exactly_one_entry_must_claim_a_measure() {
        let entries = [[29, 31], [37, 39]];
        let claim = |e: &[i32; 2]| *e;
        assert_eq!(claiming(&entries, 30, claim), Ok((0, &[29, 31])));
        assert_eq!(claiming(&entries, 39, claim), Ok((1, &[37, 39])));
        for measured in [28, 32, 36, 40] {
            assert_eq!(
                claiming(&entries, measured, claim),
                Err(ScaleError::NotListed { measured })
            );
        }
        let overlapping = [[29, 33], [33, 39]];
        assert_eq!(
            claiming(&overlapping, 33, claim),
            Err(ScaleError::Ambiguous { measured: 33 })
        );
        assert!(claims_apart(&entries, "scales", claim).is_ok());
        assert!(claims_apart(&overlapping, "scales", claim).is_err());
        // Ranges that meet with no pixel between them are refused in
        // either order; one pixel between them is enough.
        let adjacent = [[29, 31], [32, 34]];
        for pair in [adjacent, [adjacent[1], adjacent[0]]] {
            assert!(claims_apart(&pair, "scales", claim).is_err_and(|why| why.contains("adjacent")));
        }
        assert!(claims_apart(&[[29, 31], [33, 35]], "scales", claim).is_ok());
        assert!(claims_apart(&[[31, 29]], "scales", claim).is_err());
        assert!(claims_apart::<[i32; 2]>(&[], "scales", claim).is_err());
        // Measuring starts from the entry nearest a scale none claims.
        assert_eq!(nearest_to_measure(&entries, 33, claim), 0);
        assert_eq!(nearest_to_measure(&entries, 35, claim), 1);
        assert_eq!(nearest_to_measure(&entries, 30, claim), 0);
        assert_eq!(nearest_to_measure(&entries, 60, claim), 1);
    }

    #[test]
    fn a_profile_whose_scale_entries_overlap_is_refused() {
        let bundled: serde_json::Value = serde_json::from_str(&bundled_text()).unwrap();
        for scales in [
            "/windows/auction_sell/sale_queue/scales",
            "/read/field_scales",
            "/windows/skills/skills_table/scales",
        ] {
            let mut twice = bundled.clone();
            let first = twice.pointer(&format!("{scales}/0")).unwrap().clone();
            twice
                .pointer_mut(scales)
                .and_then(serde_json::Value::as_array_mut)
                .unwrap()
                .push(first);
            assert!(
                matches!(
                    UiProfile::from_json(&twice.to_string()),
                    Err(ProfileError::Inconsistent(why)) if why.contains("overlapping")
                ),
                "{scales}"
            );
            let mut none = bundled.clone();
            *none.pointer_mut(scales).unwrap() = serde_json::json!([]);
            assert!(
                matches!(
                    UiProfile::from_json(&none.to_string()),
                    Err(ProfileError::Inconsistent(why)) if why.contains("lists no scale")
                ),
                "{scales}"
            );
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
