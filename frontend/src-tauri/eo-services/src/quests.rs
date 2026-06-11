//! Quest service, ported from `backend/services/quest_service.py`: this
//! first slice carries the quest and playlist CRUD surface and the
//! shared helper layer (row shaping, cooldown derivation, reward-markup
//! normalisation, mob and playlist-item management). The lifecycle
//! actions (start/complete/cancel, session links, mission detection)
//! and the analytics readers follow in their own slices.
//!
//! Payload semantics mirror the original's `dict.get` rules exactly: a
//! key that is ABSENT takes the documented default, while a key that is
//! PRESENT binds its value even when null (the original passes the
//! explicit `None` through). Truthiness gates (`reward_is_skill`, the
//! mobs list) follow Python falsiness: null, false, zero, and empty
//! strings/arrays/objects all read as false.
//!
//! Row values surface with their stored types (`reward_is_skill` and
//! `is_active` as 0/1 integers, ids as integers), exactly as the
//! original's `dict(row)` does; the camelCase wire shaping lives in the
//! router layer, not here.

use std::fmt;

use serde_json::{json, Map, Value};
use sqlx::sqlite::SqliteConnection;
use sqlx::{Row, SqlitePool};

use crate::tracker::to_iso_utc;

pub const PLAYLIST_GROUP_IMMEDIATE: &str = "immediate";
pub const PLAYLIST_GROUP_LONG_HORIZON: &str = "long_horizon";

/// The service's error surface: `Invalid` carries the original's
/// raised-exception messages (its `ValueError` texts verbatim). The
/// quest router leaves these unhandled, so they surface as 500s, not
/// 400s; the future router slice must preserve that. `Db` is a
/// database failure (also 500).
#[derive(Debug)]
pub enum QuestError {
    Invalid(String),
    Db(sqlx::Error),
}

impl fmt::Display for QuestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuestError::Invalid(message) => write!(f, "{message}"),
            QuestError::Db(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for QuestError {}

impl From<sqlx::Error> for QuestError {
    fn from(error: sqlx::Error) -> Self {
        QuestError::Db(error)
    }
}

/// The enriched quest SELECT: every quest column plus the latest
/// completion instant (cooldown and completion counts derive at read
/// time; no counter column exists).
const QUEST_SELECT: &str = "\
    SELECT q.id, q.name, q.planet, q.waypoint, q.cooldown_hours, \
           q.reward_ped, q.reward_is_skill, q.expected_reward_markup_percent, \
           q.notes, q.chain_name, q.chain_position, q.chain_total, \
           q.started_at, q.is_active, q.created_at, q.category, \
           q.reward_description, q.updated_at, \
           (SELECT MAX(completed_at) \
            FROM session_quest_completions \
            WHERE quest_id = q.id) AS last_completed_at \
    FROM quests q";

/// Quest operations: CRUD and playlists (the lifecycle actions and
/// analytics readers arrive with their own slices).
pub struct QuestService {
    pool: SqlitePool,
}

impl QuestService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ── Quest CRUD ──────────────────────────────────────────────────

    /// List all quests, enriched with mobs and playlist membership.
    pub async fn get_quests(&self, active_only: bool) -> Result<Vec<Value>, QuestError> {
        let where_clause = if active_only {
            "WHERE q.is_active = 1"
        } else {
            ""
        };
        let sql = format!("{QUEST_SELECT} {where_clause} ORDER BY q.created_at ASC");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        let mut quests = Vec::with_capacity(rows.len());
        for row in rows {
            let mut quest = row_to_quest(&row);
            self.enrich_quest(&mut quest).await?;
            quests.push(Value::Object(quest));
        }
        Ok(quests)
    }

    /// A single quest by ID, enriched; `None` when absent.
    pub async fn get_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let sql = format!("{QUEST_SELECT} WHERE q.id = ?");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(quest_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let mut quest = row_to_quest(&row);
        self.enrich_quest(&mut quest).await?;
        Ok(Some(Value::Object(quest)))
    }

    async fn enrich_quest(&self, quest: &mut Map<String, Value>) -> Result<(), QuestError> {
        let quest_id = quest["id"].as_i64().expect("integer quest id");
        quest.insert("mobs".into(), json!(self.quest_mobs(quest_id).await?));
        quest.insert(
            "playlist_ids".into(),
            json!(self.quest_playlist_ids(quest_id).await?),
        );
        Ok(())
    }

    /// Create a quest and return it.
    pub async fn create_quest(&self, data: &Value) -> Result<Value, QuestError> {
        let markup = normalize_expected_reward_markup(
            data.get("reward_ped"),
            data.get("reward_is_skill"),
            data.get("expected_reward_markup_percent"),
        );
        let mut tx = self.pool.begin().await?;
        let query = sqlx::query(
            "INSERT INTO quests (name, planet, waypoint, cooldown_hours, \
             reward_ped, reward_is_skill, expected_reward_markup_percent, \
             notes, chain_name, chain_position, chain_total, \
             category, reward_description) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        );
        let planet = match data.get("planet") {
            None => json!("Calypso"),
            Some(value) => value.clone(),
        };
        let result = bind_json(query, data.get("name").expect("quest payload carries name"));
        let result = bind_json(result, &planet);
        let result = bind_json(result, data.get("waypoint").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("cooldown_hours").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("reward_ped").unwrap_or(&Value::Null));
        let result = result.bind(i64::from(json_truthy(data.get("reward_is_skill"))));
        let markup_value = json!(markup);
        let result = bind_json(result, &markup_value);
        let result = bind_json(result, data.get("notes").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("chain_name").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("chain_position").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("chain_total").unwrap_or(&Value::Null));
        let result = bind_json(result, data.get("category").unwrap_or(&Value::Null));
        let result = bind_json(
            result,
            data.get("reward_description").unwrap_or(&Value::Null),
        );
        let quest_id = result.execute(&mut *tx).await?.last_insert_rowid();

        if let Some(mobs) = data.get("mobs") {
            // The original's truthiness gate: an empty (or null) mobs
            // payload writes nothing.
            if json_truthy(Some(mobs)) {
                set_quest_mobs(&mut tx, quest_id, mobs.as_array().expect("mobs is a list")).await?;
            }
        }
        tx.commit().await?;

        Ok(self
            .get_quest(quest_id)
            .await?
            .expect("the quest was just inserted"))
    }

    /// Update a quest's fields; `None` when the quest is absent.
    pub async fn update_quest(
        &self,
        quest_id: i64,
        data: &Value,
    ) -> Result<Option<Value>, QuestError> {
        let Some(existing) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };

        const ALLOWED: [&str; 13] = [
            "name",
            "planet",
            "waypoint",
            "cooldown_hours",
            "reward_ped",
            "reward_is_skill",
            "notes",
            "chain_name",
            "chain_position",
            "chain_total",
            "category",
            "reward_description",
            "expected_reward_markup_percent",
        ];
        let mut updates: Vec<(&str, Value)> = Vec::new();
        for key in ALLOWED {
            if let Some(value) = data.get(key) {
                let value = if key == "reward_is_skill" {
                    json!(i64::from(json_truthy(Some(value))))
                } else {
                    value.clone()
                };
                updates.push((key, value));
            }
        }

        // A change to any reward field re-normalises the stored markup
        // from the merged (incoming-over-existing) reward picture.
        let reward_keys = [
            "reward_ped",
            "reward_is_skill",
            "expected_reward_markup_percent",
        ];
        if reward_keys.iter().any(|key| data.get(key).is_some()) {
            let merged = |key: &str| {
                data.get(key)
                    .cloned()
                    .unwrap_or_else(|| existing.get(key).cloned().unwrap_or(Value::Null))
            };
            let markup = normalize_expected_reward_markup(
                Some(&merged("reward_ped")),
                Some(&merged("reward_is_skill")),
                Some(&merged("expected_reward_markup_percent")),
            );
            let entry = ("expected_reward_markup_percent", json!(markup));
            match updates
                .iter_mut()
                .find(|(key, _)| *key == "expected_reward_markup_percent")
            {
                Some(existing_entry) => *existing_entry = entry,
                None => updates.push(entry),
            }
        }

        let mut tx = self.pool.begin().await?;
        if !updates.is_empty() {
            let set_clause = updates
                .iter()
                .map(|(key, _)| format!("{key} = ?"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("UPDATE quests SET {set_clause} WHERE id = ?");
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in &updates {
                query = bind_json(query, value);
            }
            query.bind(quest_id).execute(&mut *tx).await?;
        }

        if let Some(mobs) = data.get("mobs") {
            // A present-but-null mobs payload refuses: the original
            // crashes after its mob delete, and the next commit on the
            // shared connection silently ratifies the wipe; the typed
            // refusal plus rollback is the sanctioned repair shape.
            let mobs = mobs.as_array().ok_or_else(|| {
                QuestError::Invalid("'mobs' must be a list of mob names".to_string())
            })?;
            set_quest_mobs(&mut tx, quest_id, mobs).await?;
        }
        tx.commit().await?;

        self.get_quest(quest_id).await
    }

    /// Soft-delete a quest, detaching it from every playlist.
    pub async fn delete_quest(&self, quest_id: i64) -> Result<bool, QuestError> {
        let affected =
            sqlx::query("UPDATE quests SET is_active = 0 WHERE id = ? AND is_active = 1")
                .bind(quest_id)
                .execute(&self.pool)
                .await?
                .rows_affected();
        if affected > 0 {
            sqlx::query("DELETE FROM quest_playlist_items WHERE quest_id = ?")
                .bind(quest_id)
                .execute(&self.pool)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    // ── Playlist CRUD ───────────────────────────────────────────────

    /// List all playlists with classified items in order.
    pub async fn get_playlists(&self, active_only: bool) -> Result<Vec<Value>, QuestError> {
        let where_clause = if active_only {
            "WHERE is_active = 1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, name, planet, estimated_minutes, is_active, created_at, updated_at \
             FROM quest_playlists {where_clause} ORDER BY created_at ASC"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        let mut playlists = Vec::with_capacity(rows.len());
        for row in rows {
            playlists.push(Value::Object(self.shape_playlist(&row).await?));
        }
        Ok(playlists)
    }

    /// A single playlist by ID; `None` when absent.
    pub async fn get_playlist(&self, playlist_id: i64) -> Result<Option<Value>, QuestError> {
        let Some(row) = sqlx::query(
            "SELECT id, name, planet, estimated_minutes, is_active, created_at, updated_at \
             FROM quest_playlists WHERE id = ?",
        )
        .bind(playlist_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(Value::Object(self.shape_playlist(&row).await?)))
    }

    async fn shape_playlist(
        &self,
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<Map<String, Value>, QuestError> {
        let mut playlist = row_to_playlist(row);
        let playlist_id = playlist["id"].as_i64().expect("integer playlist id");
        let items = self.playlist_items(playlist_id).await?;
        let (immediate_ids, long_horizon_ids) = split_playlist_item_groups(&items);
        playlist.insert(
            "quest_ids".into(),
            json!(items
                .iter()
                .map(|item| item["quest_id"].clone())
                .collect::<Vec<_>>()),
        );
        playlist.insert("immediate_quest_ids".into(), json!(immediate_ids));
        playlist.insert("long_horizon_quest_ids".into(), json!(long_horizon_ids));
        playlist.insert("items".into(), json!(items));
        Ok(playlist)
    }

    /// Create a playlist with classified items.
    pub async fn create_playlist(&self, data: &Value) -> Result<Value, QuestError> {
        let items = normalize_playlist_items(data)?;
        let mut tx = self.pool.begin().await?;
        let query = sqlx::query(
            "INSERT INTO quest_playlists (name, planet, estimated_minutes) VALUES (?, ?, ?)",
        );
        let planet = match data.get("planet") {
            None => json!("Calypso"),
            Some(value) => value.clone(),
        };
        let estimated = match data.get("estimated_minutes") {
            None => json!(30),
            Some(value) => value.clone(),
        };
        let query = bind_json(
            query,
            data.get("name").expect("playlist payload carries name"),
        );
        let query = bind_json(query, &planet);
        let query = bind_json(query, &estimated);
        let playlist_id = query.execute(&mut *tx).await?.last_insert_rowid();
        set_playlist_items(&mut tx, playlist_id, &items).await?;
        tx.commit().await?;

        Ok(self
            .get_playlist(playlist_id)
            .await?
            .expect("the playlist was just inserted"))
    }

    /// Update a playlist's fields and/or classified quest groups;
    /// `None` when absent.
    pub async fn update_playlist(
        &self,
        playlist_id: i64,
        data: &Value,
    ) -> Result<Option<Value>, QuestError> {
        if self.get_playlist(playlist_id).await?.is_none() {
            return Ok(None);
        }

        const ALLOWED: [&str; 3] = ["name", "planet", "estimated_minutes"];
        let updates: Vec<(&str, &Value)> = ALLOWED
            .iter()
            .filter_map(|key| data.get(*key).map(|value| (*key, value)))
            .collect();

        let replace_items = data.get("items").is_some() || data.get("quest_ids").is_some();
        let items = if replace_items {
            Some(normalize_playlist_items(data)?)
        } else {
            None
        };

        let mut tx = self.pool.begin().await?;
        if !updates.is_empty() {
            let set_clause = updates
                .iter()
                .map(|(key, _)| format!("{key} = ?"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("UPDATE quest_playlists SET {set_clause} WHERE id = ?");
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in &updates {
                query = bind_json(query, value);
            }
            query.bind(playlist_id).execute(&mut *tx).await?;
        }
        if let Some(items) = items {
            set_playlist_items(&mut tx, playlist_id, &items).await?;
        }
        tx.commit().await?;

        self.get_playlist(playlist_id).await
    }

    /// Soft-delete a playlist and clear its items.
    pub async fn delete_playlist(&self, playlist_id: i64) -> Result<bool, QuestError> {
        let affected =
            sqlx::query("UPDATE quest_playlists SET is_active = 0 WHERE id = ? AND is_active = 1")
                .bind(playlist_id)
                .execute(&self.pool)
                .await?
                .rows_affected();
        if affected > 0 {
            sqlx::query("DELETE FROM quest_playlist_items WHERE playlist_id = ?")
                .bind(playlist_id)
                .execute(&self.pool)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    // ── Mob autocomplete ────────────────────────────────────────────

    /// All distinct mob names across active quests, for autocomplete.
    pub async fn get_all_mob_names(&self) -> Result<Vec<String>, QuestError> {
        let rows = sqlx::query(
            "SELECT DISTINCT qm.mob_name FROM quest_mobs qm \
             JOIN quests q ON q.id = qm.quest_id \
             WHERE q.is_active = 1 \
             ORDER BY qm.mob_name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    // ── Shared helpers ──────────────────────────────────────────────

    async fn quest_mobs(&self, quest_id: i64) -> Result<Vec<String>, QuestError> {
        let rows =
            sqlx::query("SELECT mob_name FROM quest_mobs WHERE quest_id = ? ORDER BY mob_name")
                .bind(quest_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>(0))
            .filter(|name| !name.is_empty())
            .collect())
    }

    async fn quest_playlist_ids(&self, quest_id: i64) -> Result<Vec<i64>, QuestError> {
        let rows = sqlx::query(
            "SELECT DISTINCT qpi.playlist_id FROM quest_playlist_items qpi \
             JOIN quest_playlists qp ON qp.id = qpi.playlist_id \
             WHERE qpi.quest_id = ? AND qp.is_active = 1",
        )
        .bind(quest_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    async fn playlist_items(&self, playlist_id: i64) -> Result<Vec<Value>, QuestError> {
        // Immediate items sort ahead of long-horizon ones (the boolean
        // expression), then by their explicit order.
        let rows = sqlx::query(
            "SELECT quest_id, description, group_type \
             FROM quest_playlist_items \
             WHERE playlist_id = ? \
             ORDER BY group_type = ?, sort_order",
        )
        .bind(playlist_id)
        .bind(PLAYLIST_GROUP_LONG_HORIZON)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "quest_id": row.get::<i64, _>(0),
                    "description": row.get::<Option<String>, _>(1),
                    "group_type": row.get::<String, _>(2),
                })
            })
            .collect())
    }
}

/// One quest row to its dict shape, with the derived cooldown expiry
/// (UTC ISO instant) computed from the latest completion.
fn row_to_quest(row: &sqlx::sqlite::SqliteRow) -> Map<String, Value> {
    let mut quest = Map::new();
    quest.insert("id".into(), json!(row.get::<i64, _>("id")));
    quest.insert("name".into(), json!(row.get::<String, _>("name")));
    quest.insert("planet".into(), json!(row.get::<String, _>("planet")));
    quest.insert(
        "waypoint".into(),
        json!(row.get::<Option<String>, _>("waypoint")),
    );
    quest.insert(
        "cooldown_hours".into(),
        json!(row.get::<Option<f64>, _>("cooldown_hours")),
    );
    quest.insert(
        "reward_ped".into(),
        json!(row.get::<Option<f64>, _>("reward_ped")),
    );
    quest.insert(
        "reward_is_skill".into(),
        json!(row.get::<i64, _>("reward_is_skill")),
    );
    quest.insert(
        "expected_reward_markup_percent".into(),
        json!(row.get::<Option<f64>, _>("expected_reward_markup_percent")),
    );
    quest.insert("notes".into(), json!(row.get::<Option<String>, _>("notes")));
    quest.insert(
        "chain_name".into(),
        json!(row.get::<Option<String>, _>("chain_name")),
    );
    quest.insert(
        "chain_position".into(),
        json!(row.get::<Option<i64>, _>("chain_position")),
    );
    quest.insert(
        "chain_total".into(),
        json!(row.get::<Option<i64>, _>("chain_total")),
    );
    quest.insert(
        "started_at".into(),
        json!(row.get::<Option<f64>, _>("started_at")),
    );
    quest.insert("is_active".into(), json!(row.get::<i64, _>("is_active")));
    quest.insert("created_at".into(), json!(row.get::<f64, _>("created_at")));
    quest.insert(
        "category".into(),
        json!(row.get::<Option<String>, _>("category")),
    );
    quest.insert(
        "reward_description".into(),
        json!(row.get::<Option<String>, _>("reward_description")),
    );
    quest.insert(
        "updated_at".into(),
        json!(row.get::<Option<f64>, _>("updated_at")),
    );
    let last_completed = row.get::<Option<f64>, _>("last_completed_at");
    quest.insert("last_completed_at".into(), json!(last_completed));

    let cooldown_hours = row.get::<Option<f64>, _>("cooldown_hours");
    let expires = match (last_completed, cooldown_hours) {
        (Some(last), Some(hours)) if hours > 0.0 => Some(to_iso_utc(last + hours * 3600.0)),
        _ => None,
    };
    quest.insert("cooldown_expires_at".into(), json!(expires));
    quest
}

fn row_to_playlist(row: &sqlx::sqlite::SqliteRow) -> Map<String, Value> {
    let mut playlist = Map::new();
    playlist.insert("id".into(), json!(row.get::<i64, _>("id")));
    playlist.insert("name".into(), json!(row.get::<String, _>("name")));
    playlist.insert("planet".into(), json!(row.get::<String, _>("planet")));
    playlist.insert(
        "estimated_minutes".into(),
        json!(row.get::<i64, _>("estimated_minutes")),
    );
    playlist.insert("is_active".into(), json!(row.get::<i64, _>("is_active")));
    playlist.insert("created_at".into(), json!(row.get::<f64, _>("created_at")));
    playlist.insert(
        "updated_at".into(),
        json!(row.get::<Option<f64>, _>("updated_at")),
    );
    playlist
}

/// The stored markup only exists for liquid (non-skill) rewards with a
/// positive PED value; anything else normalises to null.
fn normalize_expected_reward_markup(
    reward_ped: Option<&Value>,
    reward_is_skill: Option<&Value>,
    expected_markup: Option<&Value>,
) -> Option<f64> {
    if json_truthy(reward_is_skill) {
        return None;
    }
    let reward_ped = reward_ped.filter(|value| !value.is_null())?;
    let reward_ped = reward_ped.as_f64().expect("numeric reward_ped");
    if reward_ped <= 0.0 {
        return None;
    }
    let expected_markup = expected_markup.filter(|value| !value.is_null())?;
    Some(expected_markup.as_f64().expect("numeric expected markup"))
}

async fn set_quest_mobs(
    conn: &mut SqliteConnection,
    quest_id: i64,
    mobs: &[Value],
) -> Result<(), QuestError> {
    sqlx::query("DELETE FROM quest_mobs WHERE quest_id = ?")
        .bind(quest_id)
        .execute(&mut *conn)
        .await?;
    for mob in mobs {
        let mob = mob.as_str().expect("mob names are strings").trim();
        if !mob.is_empty() {
            sqlx::query("INSERT OR IGNORE INTO quest_mobs (quest_id, mob_name) VALUES (?, ?)")
                .bind(quest_id)
                .bind(mob)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// Rewrite a playlist's items with explicit grouping. The original
/// validates each item inside the loop, after its delete; an invalid
/// group raises there with nothing committed, and this port's enclosing
/// transaction rolls the partial rewrite back on the same error.
async fn set_playlist_items(
    conn: &mut SqliteConnection,
    playlist_id: i64,
    items: &[Value],
) -> Result<(), QuestError> {
    sqlx::query("DELETE FROM quest_playlist_items WHERE playlist_id = ?")
        .bind(playlist_id)
        .execute(&mut *conn)
        .await?;
    for (index, item) in items.iter().enumerate() {
        let (quest_id, description, group_type) = match item {
            Value::Object(entry) => (
                entry
                    .get("quest_id")
                    .expect("item carries quest_id")
                    .clone(),
                entry.get("description").cloned().unwrap_or(Value::Null),
                entry
                    .get("group_type")
                    .cloned()
                    .unwrap_or_else(|| json!(PLAYLIST_GROUP_IMMEDIATE)),
            ),
            other => (other.clone(), Value::Null, json!(PLAYLIST_GROUP_IMMEDIATE)),
        };
        let valid_group = group_type
            .as_str()
            .is_some_and(|g| g == PLAYLIST_GROUP_IMMEDIATE || g == PLAYLIST_GROUP_LONG_HORIZON);
        if !valid_group {
            return Err(QuestError::Invalid(format!(
                "Invalid playlist group type: {}",
                python_str(&group_type)
            )));
        }
        let query = sqlx::query(
            "INSERT INTO quest_playlist_items \
             (playlist_id, quest_id, sort_order, description, group_type) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(playlist_id);
        let query = bind_json(query, &quest_id);
        let query = query.bind(index as i64);
        let query = bind_json(query, &description);
        let query = bind_json(query, &group_type);
        query.execute(&mut *conn).await?;
    }
    Ok(())
}

/// Normalise playlist payloads to classified items: an `items` list
/// passes through with group defaults; otherwise `quest_ids` builds
/// immediate items. A present-but-null (or non-list) `quest_ids`
/// refuses: the original crashes iterating it (an unhandled error on
/// the wire, with no surviving write), and the update path is
/// reachable with an explicit null through the route model.
fn normalize_playlist_items(data: &Value) -> Result<Vec<Value>, QuestError> {
    if let Some(items) = data.get("items").filter(|value| !value.is_null()) {
        return Ok(items
            .as_array()
            .expect("items is a list")
            .iter()
            .map(|item| {
                json!({
                    "quest_id": item.get("quest_id").expect("item carries quest_id"),
                    "description": item.get("description").cloned().unwrap_or(Value::Null),
                    "group_type": item
                        .get("group_type")
                        .cloned()
                        .unwrap_or_else(|| json!(PLAYLIST_GROUP_IMMEDIATE)),
                })
            })
            .collect());
    }
    let quest_ids = match data.get("quest_ids") {
        None => &[] as &[Value],
        Some(value) => value
            .as_array()
            .ok_or_else(|| {
                QuestError::Invalid("'quest_ids' must be a list of quest ids".to_string())
            })?
            .as_slice(),
    };
    Ok(quest_ids
        .iter()
        .map(|quest_id| {
            json!({
                "quest_id": quest_id,
                "description": null,
                "group_type": PLAYLIST_GROUP_IMMEDIATE,
            })
        })
        .collect())
}

/// Partition item quest ids by group: everything not long-horizon is
/// immediate.
fn split_playlist_item_groups(items: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let group = |item: &Value| {
        item.get("group_type")
            .and_then(Value::as_str)
            .map(String::from)
    };
    let immediate = items
        .iter()
        .filter(|item| group(item).as_deref() != Some(PLAYLIST_GROUP_LONG_HORIZON))
        .map(|item| item["quest_id"].clone())
        .collect();
    let long_horizon = items
        .iter()
        .filter(|item| group(item).as_deref() == Some(PLAYLIST_GROUP_LONG_HORIZON))
        .map(|item| item["quest_id"].clone())
        .collect();
    (immediate, long_horizon)
}

/// Python truthiness over JSON values: null, false, zero, and empty
/// strings/arrays/objects are false.
fn json_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(entries)) => !entries.is_empty(),
    }
}

/// Bind a JSON payload value with the original's sqlite3 adapter
/// semantics (booleans as integers); a structured value has no adapter
/// and is a caller error, as it is in the original.
fn bind_json<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
    value: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
    match value {
        Value::Null => query.bind(None::<String>),
        Value::Bool(flag) => query.bind(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => query.bind(integer),
            None => query.bind(number.as_f64().expect("finite numeric payload")),
        },
        Value::String(text) => query.bind(text.as_str()),
        other => panic!("unbindable payload value: {other}"),
    }
}

/// Render a JSON value the way a Python f-string renders the
/// corresponding object (for byte-exact error messages).
fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

// Expected values in these tests are the original implementation's
// outputs, computed by running `backend/services/quest_service.py`
// over byte-identical payloads and database seeds (created_at and
// updated_at pinned by direct UPDATE on both sides, since the schema
// stamps them from the wall clock).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn service(dir: &std::path::Path) -> (QuestService, SqlitePool) {
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        let pool = db.pool().clone();
        (QuestService::new(pool.clone()), pool)
    }

    async fn pin_ts(pool: &SqlitePool, table: &str, id: i64, ts: f64) {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET created_at = ?, updated_at = ? WHERE id = ?"
        )))
        .bind(ts)
        .bind(ts)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    fn quest_id(value: &Value) -> i64 {
        value["id"].as_i64().unwrap()
    }

    fn full_quest_payload() -> Value {
        json!({
            "name": "Atrox Cull", "planet": "Foma", "waypoint": "/wp 1,2",
            "cooldown_hours": 24, "reward_ped": 12.5, "reward_is_skill": false,
            "expected_reward_markup_percent": 150.0, "notes": "bring fap",
            "chain_name": "Cull", "chain_position": 1, "chain_total": 3,
            "category": "hunt", "reward_description": "ammo",
            "mobs": [" Atrox ", "", "Atrax", "Atrox"],
        })
    }

    #[tokio::test]
    async fn creates_apply_defaults_normalisation_and_mob_rules() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;

        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", q1, 1000.0).await;
        let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());
        pin_ts(&pool, "quests", q2, 1001.0).await;
        let q3 = quest_id(
            &svc.create_quest(&json!({
                "name": "Skill Run", "reward_ped": 5.0, "reward_is_skill": true,
                "expected_reward_markup_percent": 120.0,
            }))
            .await
            .unwrap(),
        );
        pin_ts(&pool, "quests", q3, 1002.0).await;
        assert_eq!((q1, q2, q3), (1, 2, 3));

        // The minimal quest: planet defaults, everything else null,
        // the skill flag stored as integer 0.
        let q1_fresh = svc.get_quest(q1).await.unwrap().unwrap();
        assert_eq!(
            q1_fresh,
            json!({
                "id": 1, "name": "Iron Challenge", "planet": "Calypso", "waypoint": null,
                "cooldown_hours": null, "reward_ped": null, "reward_is_skill": 0,
                "expected_reward_markup_percent": null, "notes": null, "chain_name": null,
                "chain_position": null, "chain_total": null, "started_at": null,
                "is_active": 1, "created_at": 1000.0, "category": null,
                "reward_description": null, "updated_at": 1000.0, "last_completed_at": null,
                "cooldown_expires_at": null, "mobs": [], "playlist_ids": [],
            })
        );

        // The full quest: mobs strip, drop empties, dedupe, and read
        // back sorted; the integer cooldown stores as REAL; a liquid
        // positive reward keeps its markup.
        let q2_fresh = svc.get_quest(q2).await.unwrap().unwrap();
        assert_eq!(q2_fresh["planet"], "Foma");
        assert_eq!(q2_fresh["cooldown_hours"], json!(24.0));
        assert_eq!(q2_fresh["expected_reward_markup_percent"], json!(150.0));
        assert_eq!(q2_fresh["mobs"], json!(["Atrax", "Atrox"]));
        assert_eq!(q2_fresh["reward_is_skill"], json!(0));
        assert_eq!(q2_fresh["chain_position"], json!(1));

        // A skill reward normalises its markup away at creation.
        let q3_fresh = svc.get_quest(q3).await.unwrap().unwrap();
        assert_eq!(q3_fresh["reward_is_skill"], json!(1));
        assert_eq!(q3_fresh["expected_reward_markup_percent"], Value::Null);
    }

    #[tokio::test]
    async fn cooldown_derives_from_the_latest_completion() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());

        sqlx::query(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('sess-1', ?, 1772366400.0)",
        )
        .bind(q2)
        .execute(&pool)
        .await
        .unwrap();

        let quest = svc.get_quest(q2).await.unwrap().unwrap();
        assert_eq!(quest["last_completed_at"], json!(1772366400.0));
        assert_eq!(
            quest["cooldown_expires_at"],
            json!("2026-03-02T12:00:00+00:00"),
            "completion instant plus 24 hours, rendered as a UTC ISO instant"
        );
    }

    #[tokio::test]
    async fn updates_merge_and_renormalise_the_markup() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );

        // Setting a positive liquid reward with a markup keeps it.
        let updated = svc
            .update_quest(
                q1,
                &json!({"reward_ped": 10.0, "expected_reward_markup_percent": 130.0}),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["reward_ped"], json!(10.0));
        assert_eq!(updated["expected_reward_markup_percent"], json!(130.0));

        // Flipping to a skill reward re-normalises the merged picture:
        // the stored markup clears even though the update names only
        // the flag.
        let updated = svc
            .update_quest(q1, &json!({"reward_is_skill": true}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["reward_is_skill"], json!(1));
        assert_eq!(updated["expected_reward_markup_percent"], Value::Null);

        assert_eq!(
            svc.update_quest(9999, &json!({"name": "x"})).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn deletes_are_soft_and_detach_playlist_items() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", q1, 1000.0).await;
        let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());
        pin_ts(&pool, "quests", q2, 1001.0).await;
        let p1 = quest_id(
            &svc.create_playlist(&json!({"name": "Morning Run", "quest_ids": [q1, q2]}))
                .await
                .unwrap(),
        );

        assert!(svc.delete_quest(q2).await.unwrap());
        assert!(!svc.delete_quest(q2).await.unwrap(), "already inactive");

        let active: Vec<i64> = svc
            .get_quests(true)
            .await
            .unwrap()
            .iter()
            .map(quest_id)
            .collect();
        assert_eq!(active, [q1]);
        let all: Vec<i64> = svc
            .get_quests(false)
            .await
            .unwrap()
            .iter()
            .map(quest_id)
            .collect();
        assert_eq!(all, [q1, q2]);

        // The deleted quest left the playlist.
        let playlist = svc.get_playlist(p1).await.unwrap().unwrap();
        assert_eq!(playlist["quest_ids"], json!([q1]));

        // Mob autocomplete reads active quests only.
        assert_eq!(svc.get_all_mob_names().await.unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn playlists_classify_items_and_split_groups() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );
        let q2 = quest_id(&svc.create_quest(&full_quest_payload()).await.unwrap());

        // A bare id list classifies everything immediate, with the
        // planet and duration defaults.
        let p1 = quest_id(
            &svc.create_playlist(&json!({"name": "Morning Run", "quest_ids": [q1, q2]}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quest_playlists", p1, 2000.0).await;
        let p1_fresh = svc.get_playlist(p1).await.unwrap().unwrap();
        assert_eq!(
            p1_fresh,
            json!({
                "id": 1, "name": "Morning Run", "planet": "Calypso",
                "estimated_minutes": 30, "is_active": 1, "created_at": 2000.0,
                "updated_at": 2000.0, "quest_ids": [1, 2],
                "immediate_quest_ids": [1, 2], "long_horizon_quest_ids": [],
                "items": [
                    {"quest_id": 1, "description": null, "group_type": "immediate"},
                    {"quest_id": 2, "description": null, "group_type": "immediate"},
                ],
            })
        );

        // Classified items keep their groups; immediate items list
        // ahead of long-horizon ones regardless of insertion order.
        let p2 = quest_id(
            &svc.create_playlist(&json!({
                "name": "Big Loop", "planet": "Foma", "estimated_minutes": 90,
                "items": [
                    {"quest_id": q2, "description": "warmup", "group_type": "immediate"},
                    {"quest_id": q1, "group_type": "long_horizon"},
                ],
            }))
            .await
            .unwrap(),
        );
        let p2_fresh = svc.get_playlist(p2).await.unwrap().unwrap();
        assert_eq!(p2_fresh["quest_ids"], json!([q2, q1]));
        assert_eq!(p2_fresh["immediate_quest_ids"], json!([q2]));
        assert_eq!(p2_fresh["long_horizon_quest_ids"], json!([q1]));
        assert_eq!(
            p2_fresh["items"],
            json!([
                {"quest_id": q2, "description": "warmup", "group_type": "immediate"},
                {"quest_id": q1, "description": null, "group_type": "long_horizon"},
            ])
        );

        // Updates rewrite items from either payload shape, and soft
        // deletes clear them.
        let updated = svc
            .update_playlist(p1, &json!({"name": "Dawn Run", "quest_ids": [q2]}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["name"], "Dawn Run");
        assert_eq!(updated["quest_ids"], json!([q2]));
        assert!(svc.delete_playlist(p2).await.unwrap());
        assert!(!svc.delete_playlist(p2).await.unwrap());
        assert_eq!(svc.get_playlists(true).await.unwrap().len(), 1);
        assert_eq!(svc.get_playlists(false).await.unwrap().len(), 2);
        assert_eq!(svc.get_playlist(9999).await.unwrap(), None);
        assert_eq!(
            svc.update_playlist(9999, &json!({"name": "x"}))
                .await
                .unwrap(),
            None
        );

        // The active quest's playlist membership reflects only live
        // playlists.
        let q2_now = svc.get_quest(q2).await.unwrap().unwrap();
        assert_eq!(q2_now["playlist_ids"], json!([p1]));
    }

    #[tokio::test]
    async fn invalid_groups_reject_verbatim_and_leave_no_trace() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );
        let p1 = quest_id(
            &svc.create_playlist(&json!({"name": "Morning Run", "quest_ids": [q1]}))
                .await
                .unwrap(),
        );

        let error = svc
            .create_playlist(&json!({
                "name": "Bad",
                "items": [{"quest_id": q1, "group_type": "weekly"}],
            }))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Invalid playlist group type: weekly");

        // A present-but-null group is rendered the way the original's
        // message renders None.
        let error = svc
            .update_playlist(
                p1,
                &json!({"items": [{"quest_id": q1, "group_type": null}]}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Invalid playlist group type: None");

        // The failed writes roll back whole: no phantom playlist, and
        // the failed item rewrite keeps the prior items. (The original
        // leaves these partial writes pending on its shared connection
        // for a later commit to ratify; the pooled port repairs that
        // by construction, per the migration's settled architecture.)
        let playlists = svc.get_playlists(true).await.unwrap();
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0]["name"], "Morning Run");
        assert_eq!(playlists[0]["quest_ids"], json!([q1]));
    }

    #[tokio::test]
    async fn present_null_lists_refuse_and_leave_state_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge", "mobs": ["Atrox"]}))
                .await
                .unwrap(),
        );
        let p1 = quest_id(
            &svc.create_playlist(&json!({"name": "Morning Run", "quest_ids": [q1]}))
                .await
                .unwrap(),
        );

        // An explicit-null quest_ids update refuses (the original
        // crashes iterating it, with no surviving write) instead of
        // clearing the playlist.
        let error = svc
            .update_playlist(p1, &json!({"quest_ids": null}))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "'quest_ids' must be a list of quest ids");
        let playlist = svc.get_playlist(p1).await.unwrap().unwrap();
        assert_eq!(playlist["quest_ids"], json!([q1]));

        // An explicit-null mobs update refuses likewise; the mob rows
        // survive (the original's crash leaves its mob delete pending
        // for the next commit to ratify silently; the typed refusal
        // plus rollback is the sanctioned repair shape).
        let error = svc
            .update_quest(q1, &json!({"mobs": null}))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "'mobs' must be a list of mob names");
        let quest = svc.get_quest(q1).await.unwrap().unwrap();
        assert_eq!(quest["mobs"], json!(["Atrox"]));
    }

    #[tokio::test]
    async fn soft_deleting_a_quest_keeps_its_mob_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let q1 = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge", "mobs": ["Atrox"]}))
                .await
                .unwrap(),
        );

        assert!(svc.delete_quest(q1).await.unwrap());
        // The soft delete detaches playlist items only; the mob rows
        // stay (the autocomplete reader filters by active quests, so
        // they vanish from that surface without being destroyed).
        let mobs: i64 = sqlx::query("SELECT COUNT(*) FROM quest_mobs WHERE quest_id = ?")
            .bind(q1)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(mobs, 1);
        assert_eq!(svc.get_all_mob_names().await.unwrap(), Vec::<String>::new());
    }
}
