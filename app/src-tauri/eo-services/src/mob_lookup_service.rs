//! Mob-name lookup against the bundled mobs catalogue, used by
//! manual-mob tracking flows for autocomplete and validation.

use serde_json::{Map, Value};

use crate::game_data_store::GameDataStore;

pub struct MobLookupService<'a> {
    game_data: &'a GameDataStore,
}

impl<'a> MobLookupService<'a> {
    pub fn new(game_data: &'a GameDataStore) -> Self {
        Self { game_data }
    }

    /// Every mob the catalogue names, as `{display, species, maturity}`
    /// rows (`display` is `{maturity} {species}`, or the bare species when
    /// it has no maturities), deduplicated by (species, maturity) and
    /// sorted by display. The declared-mob picker searches this list with
    /// the frontend's shared search.
    pub fn mob_names(&self) -> Vec<Value> {
        let mut results: Vec<Value> = Vec::new();
        let mut seen: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();

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
                if seen.insert((species.clone(), String::new())) {
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
                if seen.insert((species.clone(), maturity.clone())) {
                    results.push(row(&display, &species, &maturity));
                }
            }
        }

        results.sort_by(|a, b| a["display"].as_str().cmp(&b["display"].as_str()));
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
pub fn python_whitespace(c: char) -> bool {
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
    fn names_list_every_maturity_once_sorted_by_display() {
        let (_dir, store) = store();
        let lookup = MobLookupService::new(&store);
        let rows = lookup.mob_names();
        let displays: Vec<&str> = rows
            .iter()
            .map(|row| row["display"].as_str().unwrap())
            .collect();
        // The duplicate Young Atrox entry collapses; the unnamed species
        // is skipped; a maturity-less species is its bare name; the
        // species falls back to the top-level name.
        assert_eq!(
            displays,
            [
                "Daikiba",
                "Old Atrox",
                "Young Atrox",
                "Young Kold",
                "Young Snablesnot Female"
            ]
        );
        let young_atrox = &rows[2];
        assert_eq!(young_atrox["species"], "Atrox");
        assert_eq!(young_atrox["maturity"], "Young");
        assert_eq!(rows[0]["maturity"], "");
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
