//! Loot-item include/exclude decisions for tracking, ported from
//! `backend/tracking/loot_filter.py`.
//!
//! Keys casefold and collapse internal whitespace before comparison.
//! (The original casefolds; this lowercases, which agrees over every
//! name the game client writes; the exotic casefold-only characters
//! have no item-name writer.)

use std::collections::BTreeSet;

use crate::mob_lookup_service::python_whitespace;

/// The default exclusion: ammunition restocks are not loot returns.
pub fn default_blacklist() -> BTreeSet<String> {
    ["universal ammo".to_string()].into()
}

/// Collapse whitespace and casefold: the comparison key.
fn key(name: &str) -> String {
    name.to_lowercase()
        .split(python_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalise a configured blacklist: an absent or empty configuration
/// falls back to the default; blank entries drop. The fallback keys on
/// the input, not the result, so a non-empty configuration of only
/// blanks deliberately blacklists nothing, exactly as the original's
/// falsy check does.
pub fn normalize_blacklist<'a>(
    names: Option<impl IntoIterator<Item = &'a str>>,
) -> BTreeSet<String> {
    let Some(names) = names else {
        return default_blacklist();
    };
    let names: Vec<&'a str> = names.into_iter().collect();
    if names.is_empty() {
        return default_blacklist();
    }
    names
        .into_iter()
        .filter(|name| !name.trim_matches(python_whitespace).is_empty())
        .map(key)
        .collect()
}

/// Whether a loot item counts toward tracked returns.
pub fn is_tracked_loot(item_name: &str, blacklist: &BTreeSet<String>) -> bool {
    !blacklist.contains(&key(item_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_collapse_case_and_whitespace() {
        let blacklist = default_blacklist();
        assert!(!is_tracked_loot("Universal Ammo", &blacklist));
        assert!(!is_tracked_loot("  universal\t\tAMMO  ", &blacklist));
        assert!(is_tracked_loot("Animal Muscle Oil", &blacklist));
    }

    #[test]
    fn normalisation_falls_back_and_drops_blanks() {
        assert_eq!(normalize_blacklist(None::<Vec<&str>>), default_blacklist());
        assert_eq!(
            normalize_blacklist(Some(Vec::<&str>::new())),
            default_blacklist()
        );
        assert_eq!(
            normalize_blacklist(Some(vec!["  ", ""])),
            BTreeSet::new(),
            "a non-empty configuration of only blanks blacklists nothing"
        );
        let custom = normalize_blacklist(Some(vec!["Shrapnel", "  Vibrant  Sweat "]));
        assert!(custom.contains("shrapnel"));
        assert!(custom.contains("vibrant sweat"));
        assert!(!custom.contains("universal ammo"));
    }
}
