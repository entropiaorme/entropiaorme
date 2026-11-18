//! In-memory catalogue of game constants.
//!
//! The snapshot (the bundled per-endpoint JSON snapshot)
//! is the application's sole source of truth for game-fact data. The
//! store loads it once at construction and serves queries from memory.
//! Iteration order is load-bearing: endpoints sit in sorted-filename
//! order and search rows carry their keys in catalogue order, the
//! walk order the pinned response shapes depend on. An unreadable or
//! unparseable snapshot file fails construction loudly; only a parsed
//! payload of the wrong shape degrades to an empty endpoint.

use std::path::Path;

use serde_json::{Map, Value};

/// Endpoints whose snapshot file holds a single object (not a list).
const SINGLE_OBJECT_ENDPOINTS: [&str; 1] = ["skill_ranks"];

pub struct GameDataStore {
    by_endpoint: Map<String, Value>,
}

impl GameDataStore {
    /// Load every `*.json` under `snapshot_dir` (sorted by filename); a
    /// missing directory yields an empty store (warn-and-continue),
    /// while an unreadable or unparseable file is a hard error.
    pub fn new(snapshot_dir: &Path) -> std::io::Result<Self> {
        let mut by_endpoint = Map::new();
        let mut paths: Vec<_> = match std::fs::read_dir(snapshot_dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect(),
            Err(_) => Vec::new(),
        };
        paths.sort();
        for path in paths {
            let Some(endpoint) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let raw = std::fs::read_to_string(&path)?;
            let data = serde_json::from_str::<Value>(&raw).map_err(|e| {
                std::io::Error::other(format!("snapshot {} does not parse: {e}", path.display()))
            })?;
            let entities = if SINGLE_OBJECT_ENDPOINTS.contains(&endpoint) {
                // Wrap so consumers reading the first entity keep working.
                Value::Array(vec![data])
            } else if data.is_array() {
                data
            } else {
                Value::Array(Vec::new())
            };
            by_endpoint.insert(endpoint.to_string(), entities);
        }
        Ok(Self { by_endpoint })
    }

    /// All entities for an endpoint (empty when unknown).
    pub fn get_entities(&self, endpoint: &str) -> &[Value] {
        self.by_endpoint
            .get(endpoint)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Every named entity of one endpoint, as rows shaped `{endpoint,
    /// item_id, item_name, data}` in walk order: the catalogue a picker
    /// searches. Matching a query is the frontend's shared search, so no
    /// text matching happens here.
    pub fn catalogue(&self, endpoint: &str) -> Vec<Value> {
        self.get_entities(endpoint)
            .iter()
            .filter_map(|entity| {
                let name = display_name(entity, endpoint).filter(|name| !name.is_empty())?;
                let mut row = Map::new();
                row.insert("endpoint".into(), Value::from(endpoint));
                row.insert(
                    "item_id".into(),
                    entity.get("id").cloned().unwrap_or(Value::Null),
                );
                row.insert("item_name".into(), Value::from(name));
                row.insert("data".into(), entity.clone());
                Some(Value::Object(row))
            })
            .collect()
    }

    /// The entity whose stringified `id` matches, or None.
    pub fn find_entity(&self, endpoint: &str, item_id: &Value) -> Option<&Value> {
        let target = python_str(item_id);
        self.get_entities(endpoint).iter().find(|entity| {
            python_str(entity.get("id").unwrap_or(&Value::String(String::new()))) == target
        })
    }

    /// Per-endpoint entity counts, in endpoint order.
    pub fn endpoint_counts(&self) -> Map<String, Value> {
        self.by_endpoint
            .iter()
            .map(|(ep, items)| {
                (
                    ep.clone(),
                    Value::from(items.as_array().map(Vec::len).unwrap_or(0)),
                )
            })
            .collect()
    }

    pub fn total_entities(&self) -> usize {
        self.by_endpoint
            .values()
            .map(|items| items.as_array().map(Vec::len).unwrap_or(0))
            .sum()
    }
}

/// The display name a catalogue row carries: `species.name` for mobs,
/// `name` everywhere else.
fn display_name(entity: &Value, endpoint: &str) -> Option<String> {
    let value = if endpoint == "mobs" {
        entity.get("species").and_then(|s| s.get("name"))
    } else {
        entity.get("name")
    };
    value.and_then(Value::as_str).map(str::to_string)
}

/// Python `str(value)` over the id shapes the snapshots carry.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                eo_wire::normalizer::python_repr_f64(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("weapons.json"),
            r#"[{"id": 1, "name": "Opalo"}, {"id": 2, "name": "Korss H400"}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mobs.json"),
            r#"[{"id": 7, "species": {"name": "Atrox"}, "maturities": [{"name": "Young"}, {"name": "Old"}]},
                {"id": 8, "species": {"name": "Daikiba"}, "maturities": []}]"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("skill_ranks.json"), r#"{"cap": 25}"#).unwrap();
        std::fs::write(dir.path().join("broken.json"), r#"{"not": "a list"}"#).unwrap();
        dir
    }

    #[test]
    fn loads_endpoints_in_sorted_order_with_wrapping_and_shape_rules() {
        let dir = snapshot_dir();
        let store = GameDataStore::new(dir.path()).unwrap();
        let counts = store.endpoint_counts();
        let keys: Vec<&String> = counts.keys().collect();
        assert_eq!(keys, ["broken", "mobs", "skill_ranks", "weapons"]);
        assert_eq!(counts["broken"], 0, "non-list payloads load empty");
        assert_eq!(counts["skill_ranks"], 1, "single objects wrap");
        assert_eq!(store.total_entities(), 5);
        assert_eq!(store.get_entities("skill_ranks")[0]["cap"], 25);
        assert!(store.get_entities("unknown").is_empty());
    }

    #[test]
    fn missing_directory_yields_an_empty_store() {
        let store = GameDataStore::new(Path::new("/nonexistent/snapshot")).unwrap();
        assert_eq!(store.total_entities(), 0);
    }

    #[test]
    fn catalogue_lists_one_endpoint_by_display_name_in_walk_order() {
        let dir = snapshot_dir();
        let store = GameDataStore::new(dir.path()).unwrap();
        let rows = store.catalogue("weapons");
        let names: Vec<&str> = rows
            .iter()
            .map(|row| row["item_name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Opalo", "Korss H400"]);
        let keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["endpoint", "item_id", "item_name", "data"]);
        assert_eq!(rows[0]["endpoint"], "weapons");
        assert_eq!(rows[0]["item_id"], 1);

        // Mobs are named by species.
        let rows = store.catalogue("mobs");
        assert_eq!(rows[0]["item_name"], "Atrox");
        assert_eq!(rows[0]["data"]["species"]["name"], "Atrox");

        assert!(store.catalogue("unknown").is_empty());
    }

    #[test]
    fn catalogue_skips_unnamed_entities() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("weapons.json"),
            r#"[{"id": 1, "name": ""}, {"id": 2}, {"id": 3, "name": "Opalo"}]"#,
        )
        .unwrap();
        let store = GameDataStore::new(dir.path()).unwrap();
        let rows = store.catalogue("weapons");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["item_name"], "Opalo");
    }

    #[test]
    fn find_entity_compares_stringified_ids() {
        let dir = snapshot_dir();
        let store = GameDataStore::new(dir.path()).unwrap();
        assert_eq!(
            store.find_entity("weapons", &Value::from(1)).unwrap()["name"],
            "Opalo"
        );
        assert_eq!(
            store.find_entity("weapons", &Value::from("2")).unwrap()["name"],
            "Korss H400"
        );
        assert!(store.find_entity("weapons", &Value::from(9)).is_none());
        assert!(store.find_entity("unknown", &Value::from(1)).is_none());
    }
}
