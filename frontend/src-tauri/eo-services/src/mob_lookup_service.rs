//! Mob-name lookup against the bundled mobs catalogue, ported from
//! `backend/services/mob_lookup_service.py`. Used by manual-mob
//! tracking flows for autocomplete and validation.

use serde_json::{Map, Value};

use crate::game_data_store::GameDataStore;

pub struct MobLookupService<'a> {
    game_data: &'a GameDataStore,
}

impl<'a> MobLookupService<'a> {
    pub fn new(game_data: &'a GameDataStore) -> Self {
        Self { game_data }
    }

    /// Exact mob display names as `{maturity} {species}` suggestions:
    /// matched on the whole query or on every whitespace token,
    /// deduplicated by (species, maturity), sorted stably by
    /// (starts-with-query, display), and truncated to `limit`.
    pub fn search_mob_names(&self, query: &str, limit: usize) -> Vec<Value> {
        let q = query.trim_matches(python_whitespace).to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let q_parts: Vec<&str> = q
            .split(python_whitespace)
            .filter(|part| !part.is_empty())
            .collect();

        let mut results: Vec<Value> = Vec::new();
        let mut seen: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();

        let matches = |display_lower: &str| {
            display_lower.contains(&q) || q_parts.iter().all(|part| display_lower.contains(part))
        };
        let row = |display: &str, species: &str, maturity: &str| {
            let mut map = Map::new();
            map.insert("display".into(), Value::from(display));
            map.insert("species".into(), Value::from(species));
            map.insert("maturity".into(), Value::from(maturity));
            Value::Object(map)
        };

        for mob in self.game_data.get_entities("mobs") {
            let species = species_name(mob);
            if species.is_empty() {
                continue;
            }

            let maturities = mob
                .get("maturities")
                .and_then(Value::as_array)
                .filter(|list| !list.is_empty());
            let Some(maturities) = maturities else {
                let key = (species.clone(), String::new());
                let display_lower = species.to_lowercase();
                if !seen.contains(&key) && matches(&display_lower) {
                    seen.insert(key);
                    results.push(row(&species, &species, ""));
                }
                continue;
            };

            for maturity_entry in maturities {
                let maturity = maturity_entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let display = if maturity.is_empty() {
                    species.clone()
                } else {
                    format!("{maturity} {species}")
                };
                let key = (species.clone(), maturity.clone());
                let display_lower = display.to_lowercase();
                if seen.contains(&key) || !matches(&display_lower) {
                    continue;
                }
                seen.insert(key);
                results.push(row(&display, &species, &maturity));
            }
        }

        // Stable sort by (starts-with-query, display), as the backend's
        // tuple key sorts.
        results.sort_by(|a, b| {
            let rank = |value: &Value| {
                let display = value["display"].as_str().unwrap_or("");
                let starts = i32::from(!display.to_lowercase().starts_with(&q));
                (starts, display.to_string())
            };
            rank(a).cmp(&rank(b))
        });
        results.truncate(limit);
        results
    }

    /// True when the exact species/maturity pair exists in the catalogue.
    /// Once a species matches, only its own maturities decide, as the
    /// backend's early return does.
    pub fn has_mob_name(&self, species: &str, maturity: &str) -> bool {
        let species = species.trim_matches(python_whitespace);
        let maturity = maturity.trim_matches(python_whitespace);
        if species.is_empty() {
            return false;
        }

        for mob in self.game_data.get_entities("mobs") {
            let cached_species = species_name(mob);
            if cached_species != species {
                continue;
            }

            let maturities = mob
                .get("maturities")
                .and_then(Value::as_array)
                .filter(|list| !list.is_empty());
            let Some(maturities) = maturities else {
                return maturity.is_empty();
            };

            return maturities.iter().any(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    == maturity
            });
        }
        false
    }
}

/// Python's whitespace class: Unicode White_Space plus the file/group/
/// record/unit separators (U+001C..U+001F), which `str.split` and
/// `str.strip` treat as whitespace but Rust's `char::is_whitespace`
/// does not.
pub(crate) fn python_whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `((mob.get("species") or {}).get("name") or mob.get("name") or "").strip()`.
fn species_name(mob: &Value) -> String {
    mob.get("species")
        .and_then(|species| species.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .or_else(|| mob.get("name").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, GameDataStore) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mobs.json"),
            r#"[
                {"id": 1, "species": {"name": "Atrox"}, "maturities": [{"name": "Young"}, {"name": "Old"}, {"name": "Young"}]},
                {"id": 2, "species": {"name": "Daikiba"}, "maturities": []},
                {"id": 3, "name": "Snablesnot Female", "maturities": [{"name": "Young"}]},
                {"id": 4, "species": {"name": ""}, "maturities": []},
                {"id": 5, "species": {"name": "Kold"}, "maturities": [{"name": "Young"}]}
            ]"#,
        )
        .unwrap();
        let store = GameDataStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn suggestions_match_tokens_dedupe_and_sort_stably() {
        let (_dir, store) = store();
        let lookup = MobLookupService::new(&store);

        let rows = lookup.search_mob_names("young atrox", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["display"], "Young Atrox");
        assert_eq!(rows[0]["species"], "Atrox");
        assert_eq!(rows[0]["maturity"], "Young");

        // Reversed token order still matches (every token, any position).
        let rows = lookup.search_mob_names("atrox young", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["display"], "Young Atrox");

        // Prefix matches rank before substring matches.
        let rows = lookup.search_mob_names("old", 10);
        let displays: Vec<&str> = rows
            .iter()
            .map(|row| row["display"].as_str().unwrap())
            .collect();
        assert_eq!(displays, ["Old Atrox", "Young Kold"]);

        // Duplicate maturity entries deduplicate by (species, maturity).
        let rows = lookup.search_mob_names("atrox", 10);
        let displays: Vec<&str> = rows
            .iter()
            .map(|row| row["display"].as_str().unwrap())
            .collect();
        // starts-with ranks "Atrox..." never (query "atrox" prefixes no
        // display? "Atrox" alone is not a row: every maturity prefixes);
        // alphabetical within the non-prefix rank.
        assert_eq!(displays, ["Old Atrox", "Young Atrox"]);

        // Maturity-less species suggest the bare name; prefix rank wins.
        let rows = lookup.search_mob_names("dai", 10);
        assert_eq!(rows[0]["display"], "Daikiba");
        assert_eq!(rows[0]["maturity"], "");

        // The species fallback to the top-level name.
        let rows = lookup.search_mob_names("snable", 10);
        assert_eq!(rows[0]["display"], "Young Snablesnot Female");

        // Blank and whitespace queries return nothing.
        assert!(lookup.search_mob_names("   ", 10).is_empty());
        // Limit truncates after the sort.
        assert_eq!(lookup.search_mob_names("young", 1).len(), 1);
    }

    #[test]
    fn exact_pair_validation_short_circuits_per_species() {
        let (_dir, store) = store();
        let lookup = MobLookupService::new(&store);
        assert!(lookup.has_mob_name("Atrox", "Young"));
        assert!(lookup.has_mob_name(" Atrox ", " Old "));
        assert!(!lookup.has_mob_name("Atrox", ""));
        assert!(!lookup.has_mob_name("Atrox", "Provider"));
        assert!(lookup.has_mob_name("Daikiba", ""));
        assert!(!lookup.has_mob_name("Daikiba", "Young"));
        assert!(!lookup.has_mob_name("", "Young"));
        assert!(!lookup.has_mob_name("Unknown", ""));
    }
}
