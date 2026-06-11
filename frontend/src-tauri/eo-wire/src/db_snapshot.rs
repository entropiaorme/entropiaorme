//! Canonical DB-state snapshot emitter: Rust port of
//! `backend/testing/db_snapshot.py`.
//!
//! The Python snapshot runs a fixed catalogue of `SELECT`s against the
//! tracking schema and renders the rows under the shared [`Normalizer`]. This
//! port reproduces the rendering half end-to-end: it normalises pre-fetched
//! catalogue rows in catalogue order (the order that drives shared-symbol
//! assignment) and serialises them as the committed `db_state.json` golden.
//!
//! ## Scope at this phase
//!
//! The catalogue's `SELECT` strings are reproduced verbatim as the porting
//! reference, but this emitter does NOT execute them: the native persistence
//! layer (and its SQLite driver) is a later, separately-gated decision, so the
//! runner is proven over the catalogue's row output (the same rows the Python
//! `_fetch_rows` returns) rather than by running SQL in Rust. When the native
//! DB layer lands, `capture` executes these queries directly against it.

use serde_json::{Map, Value};

use crate::normalizer::{to_python_json, Normalizer};

/// One catalogue entry: the table, the columns the snapshot selects (verbatim
/// from the Python catalogue, the porting reference), and the deterministic row
/// order. `query`/`order_by` document the contract the row provider must honour;
/// the emitter consumes rows already in this order.
pub struct TableSpec {
    pub name: &'static str,
    pub query: &'static str,
    pub order_by: &'static [&'static str],
}

/// The tracking-domain and quest/codex tables the snapshot captures,
/// in the order that drives shared-symbol assignment (mirrors
/// `db_snapshot.CATALOGUE`).
pub const CATALOGUE: &[TableSpec] = &[
    TableSpec {
        name: "tracking_sessions",
        query: "SELECT id, started_at, ended_at, is_active, \
                COALESCE(heal_cost, 0.0) AS heal_cost, \
                COALESCE(dangling_cost, 0.0) AS dangling_cost \
                FROM tracking_sessions",
        order_by: &["rowid"],
    },
    TableSpec {
        name: "kills",
        query: "SELECT id, session_id, mob_name, mob_species, mob_maturity, \
                timestamp, shots_fired, damage_dealt, damage_taken, \
                critical_hits, cost_ped, enhancer_cost, loot_total_ped, \
                is_global, is_hof \
                FROM kills",
        order_by: &["timestamp", "rowid"],
    },
    TableSpec {
        name: "kill_loot_items",
        query: "SELECT kli.kill_id, kli.item_name, kli.quantity, \
                kli.value_ped, kli.is_enhancer_shrapnel \
                FROM kill_loot_items kli \
                JOIN kills k ON kli.kill_id = k.id",
        order_by: &["k.timestamp", "kli.rowid"],
    },
    TableSpec {
        name: "kill_tool_stats",
        query: "SELECT kts.kill_id, kts.tool_name, kts.shots_fired, \
                kts.damage_dealt, kts.critical_hits, kts.cost_per_shot \
                FROM kill_tool_stats kts \
                JOIN kills k ON kts.kill_id = k.id",
        order_by: &["k.timestamp", "kts.rowid"],
    },
    TableSpec {
        name: "ledger_entries",
        query: "SELECT id, date, type, description, amount, tag FROM ledger_entries",
        order_by: &["rowid"],
    },
    TableSpec {
        name: "notable_events",
        query: "SELECT session_id, kill_id, event_type, mob_or_item, \
                value_ped, timestamp \
                FROM notable_events",
        order_by: &["timestamp", "rowid"],
    },
    TableSpec {
        name: "quests",
        query: "SELECT id, name, planet, waypoint, cooldown_hours, reward_ped, \
                reward_is_skill, expected_reward_markup_percent, notes, \
                chain_name, chain_position, chain_total, started_at, is_active, \
                category, reward_description \
                FROM quests",
        order_by: &["rowid"],
    },
    TableSpec {
        name: "quest_mobs",
        query: "SELECT quest_id, mob_name FROM quest_mobs",
        order_by: &["quest_id", "mob_name"],
    },
    TableSpec {
        name: "quest_playlists",
        query: "SELECT id, name, planet, estimated_minutes, is_active \
                FROM quest_playlists",
        order_by: &["rowid"],
    },
    TableSpec {
        name: "quest_playlist_items",
        query: "SELECT playlist_id, quest_id, sort_order, description, group_type \
                FROM quest_playlist_items",
        order_by: &["playlist_id", "sort_order", "rowid"],
    },
    TableSpec {
        name: "session_quest_completions",
        query: "SELECT session_id, quest_id, completed_at \
                FROM session_quest_completions",
        order_by: &["completed_at", "rowid"],
    },
    TableSpec {
        name: "codex_progress",
        query: "SELECT species_name, current_rank FROM codex_progress",
        order_by: &["species_name"],
    },
    TableSpec {
        name: "codex_claims",
        query: "SELECT species_name, rank, skill_name, ped_value, claimed_at, \
                kind, attribute_name \
                FROM codex_claims",
        order_by: &["claimed_at", "rowid"],
    },
    TableSpec {
        name: "quest_claims",
        query: "SELECT quest_id, quest_name, ped_value, claimed_at FROM quest_claims",
        order_by: &["claimed_at", "rowid"],
    },
    TableSpec {
        name: "session_quest_analytics_links",
        query: "SELECT session_id, link_type, quest_id, playlist_id, linked_at \
                FROM session_quest_analytics_links",
        order_by: &["linked_at", "rowid"],
    },
    TableSpec {
        name: "skill_calibrations",
        query: "SELECT skill_name, level, source, scanned_at FROM skill_calibrations",
        order_by: &["scanned_at", "rowid"],
    },
];

/// Normalise the pre-fetched catalogue rows, returning the snapshot value.
///
/// `raw_rows` maps each table name to its rows (in the catalogue's `order_by`
/// order). Tables absent from the map render as empty lists, matching the
/// Python `capture`'s missing-table handling. Rows are normalised in catalogue
/// order so the shared `normalizer`'s symbol table grows exactly as the Python
/// snapshot's does; the output keys are sorted at serialisation time.
pub fn capture(raw_rows: &Map<String, Value>, normalizer: &mut Normalizer) -> Value {
    let mut out = Map::new();
    for spec in CATALOGUE {
        let normalised: Vec<Value> = raw_rows
            .get(spec.name)
            .and_then(Value::as_array)
            .map(|rows| rows.iter().map(|row| normalizer.normalize(row)).collect())
            .unwrap_or_default();
        out.insert(spec.name.to_string(), Value::Array(normalised));
    }
    Value::Object(out)
}

/// Render the snapshot as the committed golden text (sorted keys, 2-space
/// indent, trailing newline), matching `db_snapshot.serialize`.
pub fn serialize(snapshot: &Value) -> String {
    to_python_json(snapshot, Some(2)) + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_tables_render_as_empty_lists() {
        let mut norm = Normalizer::new();
        let snapshot = capture(&Map::new(), &mut norm);
        for spec in CATALOGUE {
            assert_eq!(snapshot[spec.name], json!([]));
        }
    }

    #[test]
    fn rows_normalise_in_catalogue_order_for_shared_symbols() {
        // tracking_sessions is first in the catalogue, so its id takes <UUID_1>
        // even though serialisation later sorts kills ahead of it.
        let mut norm = Normalizer::new();
        let mut raw = Map::new();
        raw.insert(
            "tracking_sessions".to_string(),
            json!([{"id": "11111111-1111-1111-1111-111111111111"}]),
        );
        raw.insert(
            "kills".to_string(),
            json!([{"id": "22222222-2222-2222-2222-222222222222"}]),
        );
        let snapshot = capture(&raw, &mut norm);
        assert_eq!(snapshot["tracking_sessions"][0]["id"], json!("<UUID_1>"));
        assert_eq!(snapshot["kills"][0]["id"], json!("<UUID_2>"));
    }
}
