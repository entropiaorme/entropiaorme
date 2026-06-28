//! Quest service, ported from the original Python implementation:
//! the quest and playlist CRUD surface with its shared helper layer
//! (row shaping, cooldown derivation, reward-markup normalisation,
//! mob and playlist-item management), plus the lifecycle actions
//! (start/complete/cancel with ledger and claim integration), the
//! curated session-link suggestions, and the chat-log mission
//! detection (auto-start, auto-complete, and reward suppression),
//! and the analytics readers (per-quest and per-playlist
//! sustainability metrics over curated session links).
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

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use regex::Regex;
use serde_json::{json, Map, Value};
use sqlx::sqlite::SqliteConnection;
use sqlx::{Row, SqlitePool};
use tokio::runtime::Handle;
use unicode_normalization::UnicodeNormalization;

use crate::clock::Clock;
use crate::difflib::sequence_ratio;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::tracker::{naive_to_epoch, to_iso_utc};

/// Stripped from chat.log mission names before matching.
static REPEATABLE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*\(repeatable\)\s*$").expect("the suffix pattern compiles")
});

/// The fuzzy-match floor for mission-name matching.
pub const FUZZY_THRESHOLD: f64 = 0.8;

/// Normalise a quest name for comparison: NFKD decomposition, ASCII
/// only, trimmed, lowercased.
pub fn normalize_quest_name(name: &str) -> String {
    name.nfkd()
        .filter(char::is_ascii)
        .collect::<String>()
        .trim()
        .to_lowercase()
}

pub const PLAYLIST_GROUP_IMMEDIATE: &str = "immediate";
pub const PLAYLIST_GROUP_LONG_HORIZON: &str = "long_horizon";

/// The service's error surface: `Invalid` carries the original's
/// raised-exception messages (its `ValueError` texts verbatim; the
/// null-list refusals name crashes the original leaves unworded). The
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

/// Quest operations: CRUD, playlists, the completion lifecycle,
/// chat-log mission detection, and the analytics readers.
pub struct QuestService {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    /// The active tracking session, fed by the bus handlers.
    current_session_id: Mutex<Option<String>>,
    /// The identifier source for ledger rows and session-less
    /// completion keys (random by default; injected by the tests so the
    /// committed goldens stamp the same identifiers).
    id_source: Mutex<Arc<dyn Fn() -> String + Send + Sync>>,
    /// The runtime the bus handlers bridge their database work onto,
    /// set when the service subscribes.
    runtime: Mutex<Option<Handle>>,
    /// Held for the service's lifetime: the original subscribes once
    /// in its constructor and never unsubscribes.
    _subscriptions: Mutex<Vec<(Topic, Registration)>>,
}

impl QuestService {
    pub fn new(pool: SqlitePool, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            clock,
            current_session_id: Mutex::new(None),
            id_source: Mutex::new(Arc::new(|| uuid::Uuid::new_v4().to_string())),
            runtime: Mutex::new(None),
            _subscriptions: Mutex::new(Vec::new()),
        }
    }

    /// Replace the identifier source (tests and the differential).
    pub fn set_id_source(&self, source: Arc<dyn Fn() -> String + Send + Sync>) {
        *self
            .id_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = source;
    }

    fn next_id(&self) -> String {
        let source = self
            .id_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        source()
    }

    /// The session guard, tolerating poison: a contained panic must
    /// not brick the service.
    fn lock_session(&self) -> MutexGuard<'_, Option<String>> {
        self.current_session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Subscribe to the bus (the original's constructor-time
    /// subscriptions): session start/stop track the active session,
    /// and a received mission auto-starts its matching quest.
    pub fn subscribe(self: &Arc<Self>, bus: &Arc<EventBus>, runtime: Handle) {
        *self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
        type Handler = fn(&QuestService, &Value);
        let pairs: [(Topic, Handler); 3] = [
            (Topic::SessionStarted, Self::on_session_start),
            (Topic::SessionStopped, Self::on_session_stop),
            (Topic::MissionReceived, Self::on_mission_received),
        ];
        let mut subscriptions = Vec::new();
        for (topic, handler) in pairs {
            let subscriber = self.clone();
            let registration = bus.subscribe(topic, move |data| handler(&subscriber, data));
            subscriptions.push((topic, registration));
        }
        *self
            ._subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = subscriptions;
    }

    /// Bridge a database future from either calling context (the
    /// tracker's dual shape).
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        let handle = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("a subscribed service carries its runtime");
        if Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| handle.block_on(future))
        } else {
            handle.block_on(future)
        }
    }

    fn on_session_start(&self, data: &Value) {
        *self.lock_session() = data
            .get("session_id")
            .and_then(Value::as_str)
            .map(String::from);
    }

    fn on_session_stop(&self, _data: &Value) {
        *self.lock_session() = None;
    }

    fn on_mission_received(&self, data: &Value) {
        let mission_name = data
            .get("mission_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !mission_name.is_empty() {
            // A failure surfaces nowhere, exactly as the original's
            // bus contains a handler exception.
            let _ = self.block_on(self.start_quest_from_mission(mission_name));
        }
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

    // ── Quest actions ───────────────────────────────────────────────

    /// Mark a quest as in-progress; `None` when absent or inactive.
    pub async fn start_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let now = naive_to_epoch(self.clock.now());
        let affected =
            sqlx::query("UPDATE quests SET started_at = ? WHERE id = ? AND is_active = 1")
                .bind(now)
                .bind(quest_id)
                .execute(&self.pool)
                .await?
                .rows_affected();
        if affected > 0 {
            self.get_quest(quest_id).await
        } else {
            Ok(None)
        }
    }

    /// Complete a quest: clear the in-progress state, record the
    /// reward (liquid rewards into the ledger, skill rewards into
    /// quest claims), and link the completion to the active session
    /// (or a synthetic key when none is active). Each step commits
    /// separately, exactly as the original's commit points fall.
    pub async fn complete_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };
        let now = naive_to_epoch(self.clock.now());
        sqlx::query("UPDATE quests SET started_at = NULL WHERE id = ?")
            .bind(quest_id)
            .execute(&self.pool)
            .await?;

        let reward_ped = quest.get("reward_ped").and_then(Value::as_f64);
        if let Some(reward) = reward_ped.filter(|&reward| reward > 0.0) {
            let name = quest["name"].as_str().expect("quest name");
            if json_truthy(quest.get("reward_is_skill")) {
                // Skill rewards are PES, not PED: a claim row, not a
                // ledger entry.
                sqlx::query(
                    "INSERT INTO quest_claims (quest_id, quest_name, ped_value, claimed_at) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(quest_id)
                .bind(name)
                .bind(reward)
                .bind(now)
                .execute(&self.pool)
                .await?;
            } else {
                let ledger_id = self.next_id();
                let date = to_iso_utc(now);
                sqlx::query(
                    "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&ledger_id)
                .bind(&date)
                .bind("markup")
                .bind(format!("Quest: {name}"))
                .bind(reward)
                .bind("quest_reward")
                .execute(&self.pool)
                .await?;
            }
        }

        let session_id = self.lock_session().clone();
        self.record_session_completion(session_id.as_deref(), quest_id, Some(now))
            .await?;
        self.get_quest(quest_id).await
    }

    /// Undo an in-progress quest, or reset an active cooldown back to
    /// ready by deleting the most recent completion (optionally
    /// undoing the recorded reward). A quest that is neither started
    /// nor cooling returns as-is.
    pub async fn cancel_quest(
        &self,
        quest_id: i64,
        undo_reward: bool,
    ) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };

        if !quest["started_at"].is_null() {
            sqlx::query("UPDATE quests SET started_at = NULL WHERE id = ? AND is_active = 1")
                .bind(quest_id)
                .execute(&self.pool)
                .await?;
            return self.get_quest(quest_id).await;
        }

        if !self.is_quest_cooling(&quest) {
            return Ok(Some(quest));
        }

        // The original groups the completion delete and the optional
        // reward undo under one commit.
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM session_quest_completions \
             WHERE id = ( \
                 SELECT id FROM session_quest_completions \
                 WHERE quest_id = ? \
                 ORDER BY completed_at DESC, id DESC \
                 LIMIT 1 \
             )",
        )
        .bind(quest_id)
        .execute(&mut *tx)
        .await?;

        if undo_reward {
            let reward_ped = quest.get("reward_ped").and_then(Value::as_f64);
            if let Some(reward) = reward_ped.filter(|&reward| reward > 0.0) {
                if json_truthy(quest.get("reward_is_skill")) {
                    delete_latest_quest_claim(&mut tx, quest_id).await?;
                } else {
                    delete_latest_quest_reward_entry(
                        &mut tx,
                        quest["name"].as_str().expect("quest name"),
                        reward,
                    )
                    .await?;
                }
            }
        }
        tx.commit().await?;
        self.get_quest(quest_id).await
    }

    // ── Session link suggestions ────────────────────────────────────

    /// Suggest a curated analytics link for a completed session.
    pub async fn get_session_link_suggestion(&self, session_id: &str) -> Result<Value, QuestError> {
        if let Some((link_type, quest_id, playlist_id)) =
            self.session_analytics_link(session_id).await?
        {
            let reason = if link_type == "declined" {
                "declined"
            } else {
                "already_linked"
            };
            return Ok(json!({
                "suggestion_type": "none",
                "reason": reason,
                "quest_id": quest_id,
                "quest_name": self.quest_name(quest_id).await?,
                "playlist_id": playlist_id,
                "playlist_name": self.playlist_name(playlist_id).await?,
            }));
        }

        let quest_ids = self.session_completed_quest_ids(session_id).await?;
        if quest_ids.is_empty() {
            return Ok(json!({
                "suggestion_type": "none",
                "reason": "no_completions",
                "quest_id": null,
                "quest_name": null,
                "playlist_id": null,
                "playlist_name": null,
            }));
        }

        if quest_ids.len() == 1 {
            let quest_id = quest_ids[0];
            return Ok(json!({
                "suggestion_type": "quest",
                "reason": "single_quest",
                "quest_id": quest_id,
                "quest_name": self.quest_name(Some(quest_id)).await?,
                "playlist_id": null,
                "playlist_name": null,
            }));
        }

        let playlist_ids = self.find_matching_playlists(&quest_ids).await?;
        if playlist_ids.len() == 1 {
            let playlist_id = playlist_ids[0];
            return Ok(json!({
                "suggestion_type": "playlist",
                "reason": "exact_playlist",
                "quest_id": null,
                "quest_name": null,
                "playlist_id": playlist_id,
                "playlist_name": self.playlist_name(Some(playlist_id)).await?,
            }));
        }

        let reason = if playlist_ids.is_empty() {
            "unclean"
        } else {
            "ambiguous_playlist"
        };
        Ok(json!({
            "suggestion_type": "none",
            "reason": reason,
            "quest_id": null,
            "quest_name": null,
            "playlist_id": null,
            "playlist_name": null,
        }))
    }

    /// Persist the current curated analytics suggestion for a session.
    pub async fn accept_session_link_suggestion(
        &self,
        session_id: &str,
    ) -> Result<Value, QuestError> {
        let suggestion = self.get_session_link_suggestion(session_id).await?;
        match suggestion["suggestion_type"].as_str() {
            Some("quest") => {
                self.set_session_analytics_link(
                    session_id,
                    "quest",
                    suggestion["quest_id"].as_i64(),
                    None,
                )
                .await?;
            }
            Some("playlist") => {
                self.set_session_analytics_link(
                    session_id,
                    "playlist",
                    None,
                    suggestion["playlist_id"].as_i64(),
                )
                .await?;
            }
            _ => {
                return Err(QuestError::Invalid(format!(
                    "No linkable suggestion for session {session_id}: {}",
                    suggestion["reason"].as_str().unwrap_or("")
                )));
            }
        }
        Ok(suggestion)
    }

    /// Persist that the user declined curated analytics linkage.
    pub async fn decline_session_link(&self, session_id: &str) -> Result<(), QuestError> {
        self.set_session_analytics_link(session_id, "declined", None, None)
            .await
    }

    // ── Chat.log mission detection ──────────────────────────────────

    /// Find a quest whose name matches a chat.log mission name: the
    /// "(repeatable)" suffix strips, then a normalised exact match, a
    /// normalised containment (five characters minimum), and finally
    /// the highest fuzzy score at or above the threshold.
    pub async fn match_quest_by_mission_name(
        &self,
        mission_name: &str,
    ) -> Result<Option<Value>, QuestError> {
        let stripped = REPEATABLE_SUFFIX.replace(mission_name, "");
        let mission_norm = normalize_quest_name(stripped.trim());
        let quests = self.get_quests(true).await?;

        for quest in &quests {
            if normalize_quest_name(quest["name"].as_str().expect("quest name")) == mission_norm {
                return Ok(Some(quest.clone()));
            }
        }

        for quest in &quests {
            let quest_norm = normalize_quest_name(quest["name"].as_str().expect("quest name"));
            if quest_norm.len() >= 5 && mission_norm.contains(&quest_norm) {
                return Ok(Some(quest.clone()));
            }
        }

        let mission_chars: Vec<char> = mission_norm.chars().collect();
        let mut best_score = 0.0f64;
        let mut best_quest: Option<&Value> = None;
        for quest in &quests {
            let quest_norm = normalize_quest_name(quest["name"].as_str().expect("quest name"));
            let quest_chars: Vec<char> = quest_norm.chars().collect();
            let score = sequence_ratio(&quest_chars, &mission_chars);
            if score > best_score {
                best_score = score;
                best_quest = Some(quest);
            }
        }
        Ok(if best_score >= FUZZY_THRESHOLD {
            best_quest.cloned()
        } else {
            None
        })
    }

    /// A "New Mission received" chat.log event: match the mission to a
    /// known quest and start tracking it as if the user clicked Start.
    pub async fn start_quest_from_mission(&self, mission_name: &str) -> Result<(), QuestError> {
        let Some(quest) = self.match_quest_by_mission_name(mission_name).await? else {
            return Ok(());
        };
        if json_truthy(quest.get("started_at")) {
            return Ok(());
        }
        self.start_quest(quest["id"].as_i64().expect("quest id"))
            .await?;
        self.record_notable_event(
            "quest_started",
            quest["name"].as_str().expect("quest name"),
            0.0,
        )
        .await;
        Ok(())
    }

    /// A MISSION_COMPLETE tick: match the mission, auto-complete the
    /// quest, and name which loot item or skill gain to suppress so
    /// the reward is not double-counted by tracking.
    pub async fn quest_reward_filter(
        &self,
        mission_name: &str,
        loot_items: &[Value],
        skill_gains: &[Value],
    ) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.match_quest_by_mission_name(mission_name).await? else {
            return Ok(None);
        };

        self.complete_quest(quest["id"].as_i64().expect("quest id"))
            .await?;

        let reward_ped = quest.get("reward_ped").and_then(Value::as_f64);
        let is_skill = json_truthy(quest.get("reward_is_skill"));
        let mut result = None;
        let mut suppressed_desc: Option<String> = None;

        if is_skill {
            // The in-game skill pop-up is the same PES reward just
            // recorded as a claim; suppress it from tracking.
            if !skill_gains.is_empty() {
                result = Some(json!({
                    "suppress_loot_index": null,
                    "suppress_skill_index": 0,
                }));
                suppressed_desc = Some("skill reward suppressed".to_string());
            }
        } else if let Some(reward) = reward_ped {
            if !loot_items.is_empty() {
                if reward > 0.0 {
                    let mut best_idx: Option<usize> = None;
                    let mut best_diff = f64::INFINITY;
                    for (index, item) in loot_items.iter().enumerate() {
                        let value = item.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                        let diff = (value - reward).abs();
                        if diff < best_diff && diff <= 0.02 {
                            best_diff = diff;
                            best_idx = Some(index);
                        }
                    }
                    if let Some(best_idx) = best_idx {
                        result = Some(json!({
                            "suppress_loot_index": best_idx,
                            "suppress_skill_index": null,
                        }));
                        let item_name = loot_items[best_idx]
                            .get("item_name")
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        suppressed_desc = Some(format!("{item_name} ({reward:.2} PED) suppressed"));
                    }
                } else {
                    // A non-positive reward still suppresses the
                    // cheapest item of the tick.
                    let mut min_idx = 0usize;
                    let mut min_value = f64::INFINITY;
                    for (index, item) in loot_items.iter().enumerate() {
                        let value = item.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                        if value < min_value {
                            min_value = value;
                            min_idx = index;
                        }
                    }
                    result = Some(json!({
                        "suppress_loot_index": min_idx,
                        "suppress_skill_index": null,
                    }));
                    let item_name = loot_items[min_idx]
                        .get("item_name")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    suppressed_desc = Some(format!("{item_name} suppressed"));
                }
            }
        }

        let mut description = quest["name"].as_str().expect("quest name").to_string();
        if let Some(suppressed) = suppressed_desc {
            description.push_str(": ");
            description.push_str(&suppressed);
        }
        let event_type = if is_skill {
            "quest_completed_pes"
        } else {
            "quest_completed"
        };
        self.record_notable_event(event_type, &description, reward_ped.unwrap_or(0.0))
            .await;

        Ok(result)
    }

    // ── Analytics ───────────────────────────────────────────────────

    /// Per-quest sustainability metrics across all linked sessions:
    /// raw totals (the frontend derives averages), only for quests
    /// with at least one curated linked session.
    pub async fn get_quest_analytics(&self) -> Result<Vec<Value>, QuestError> {
        let quest_rows = sqlx::query(
            "SELECT q.id, q.name, q.planet, q.category, q.reward_ped, \
                    q.reward_is_skill, q.expected_reward_markup_percent \
             FROM quests q \
             WHERE q.is_active = 1 \
             ORDER BY q.name",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for row in quest_rows {
            let quest_id = row.get::<i64, _>(0);
            let stats = self.compute_quest_session_stats(quest_id).await?;
            if stats["linked_sessions"] == json!(0) {
                continue;
            }
            let reward_ped = row.get::<Option<f64>, _>(4);
            let reward_is_skill = row.get::<i64, _>(5) != 0;
            let markup = row.get::<Option<f64>, _>(6);
            // The original's `or 0` collapses an absent or zero reward
            // to the integer zero.
            let reward_value = match reward_ped {
                Some(reward) if reward != 0.0 => json!(reward),
                _ => json!(0),
            };
            let linked_sessions = stats["linked_sessions"].as_i64().expect("session count");
            let mut entry = Map::new();
            entry.insert("quest_id".into(), json!(quest_id));
            entry.insert("quest_name".into(), json!(row.get::<String, _>(1)));
            entry.insert("planet".into(), json!(row.get::<String, _>(2)));
            entry.insert("category".into(), json!(row.get::<Option<String>, _>(3)));
            entry.insert("reward_ped".into(), reward_value.clone());
            entry.insert("reward_is_skill".into(), json!(reward_is_skill));
            entry.insert("expected_reward_markup_percent".into(), json!(markup));
            entry.insert(
                "total_expected_reward_ped".into(),
                expected_reward_total(&reward_value, reward_is_skill, markup, linked_sessions),
            );
            for (key, value) in stats.as_object().expect("stats object") {
                entry.insert(key.clone(), value.clone());
            }
            results.push(Value::Object(entry));
        }
        Ok(results)
    }

    /// Aggregate economics for all sessions where this quest was
    /// completed, via the curated analytics link table.
    async fn compute_quest_session_stats(&self, quest_id: i64) -> Result<Value, QuestError> {
        let rows = sqlx::query(
            "SELECT session_id FROM session_quest_analytics_links \
             WHERE quest_id = ? AND link_type = 'quest'",
        )
        .bind(quest_id)
        .fetch_all(&self.pool)
        .await?;
        let session_ids: Vec<String> = rows.into_iter().map(|row| row.get(0)).collect();
        self.compute_session_set_stats(&session_ids).await
    }

    /// Per-playlist sustainability metrics from curated linked
    /// sessions, for every active playlist.
    pub async fn get_all_playlist_analytics(&self) -> Result<Vec<Value>, QuestError> {
        let playlists = self.get_playlists(true).await?;
        let mut results = Vec::new();
        for playlist in playlists {
            let playlist_id = playlist["id"].as_i64().expect("playlist id");
            if let Some(stats) = self.get_playlist_analytics(playlist_id).await? {
                results.push(stats);
            }
        }
        Ok(results)
    }

    /// Analytics for a single playlist from curated linked sessions;
    /// `None` when the playlist is absent.
    pub async fn get_playlist_analytics(
        &self,
        playlist_id: i64,
    ) -> Result<Option<Value>, QuestError> {
        let Some(playlist) = self.get_playlist(playlist_id).await? else {
            return Ok(None);
        };

        let immediate_ids = self
            .playlist_quest_ids(playlist_id, Some(PLAYLIST_GROUP_IMMEDIATE))
            .await?;
        let long_horizon_ids = self
            .playlist_quest_ids(playlist_id, Some(PLAYLIST_GROUP_LONG_HORIZON))
            .await?;
        if immediate_ids.is_empty() {
            return Ok(Some(json!({
                "playlist_id": playlist_id,
                "playlist_name": playlist["name"],
                "quest_count": 0,
                "long_horizon_quest_count": long_horizon_ids.len(),
                "matched_sessions": 0,
                "total_reward_ped": 0,
                "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0,
                "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0,
                "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0,
                "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            })));
        }

        let session_ids = self.curated_playlist_session_ids(playlist_id).await?;
        let stats = if session_ids.is_empty() {
            json!({
                "linked_sessions": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            })
        } else {
            self.compute_session_set_stats(&session_ids).await?
        };
        let reward_stats = self
            .compute_playlist_reward_stats(&session_ids, &immediate_ids, &long_horizon_ids)
            .await?;

        let mut entry = Map::new();
        entry.insert("playlist_id".into(), json!(playlist_id));
        entry.insert("playlist_name".into(), playlist["name"].clone());
        entry.insert("quest_count".into(), json!(immediate_ids.len()));
        entry.insert(
            "long_horizon_quest_count".into(),
            json!(long_horizon_ids.len()),
        );
        for (key, value) in reward_stats.as_object().expect("reward stats") {
            entry.insert(key.clone(), value.clone());
        }
        entry.insert("matched_sessions".into(), stats["linked_sessions"].clone());
        for (key, value) in stats.as_object().expect("stats object") {
            entry.insert(key.clone(), value.clone());
        }
        Ok(Some(Value::Object(entry)))
    }

    async fn curated_playlist_session_ids(
        &self,
        playlist_id: i64,
    ) -> Result<Vec<String>, QuestError> {
        let rows = sqlx::query(
            "SELECT session_id FROM session_quest_analytics_links \
             WHERE playlist_id = ? AND link_type = 'playlist'",
        )
        .bind(playlist_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    /// Aggregate economics for a set of sessions: completed-session
    /// durations and costs, weapon costs through the per-tool stats,
    /// and loot and skill totals.
    async fn compute_session_set_stats(&self, session_ids: &[String]) -> Result<Value, QuestError> {
        if session_ids.is_empty() {
            return Ok(json!({
                "linked_sessions": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            }));
        }

        let placeholders = vec!["?"; session_ids.len()].join(",");
        let bind_all = |sql: String| {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for session_id in session_ids {
                query = query.bind(session_id.as_str());
            }
            query
        };

        let sess_row = bind_all(format!(
            "SELECT COUNT(*), \
                    COALESCE(SUM(s.ended_at - s.started_at), 0), \
                    COALESCE(SUM(s.heal_cost), 0), \
                    COALESCE(SUM(s.armour_cost), 0) \
             FROM tracking_sessions s \
             WHERE s.id IN ({placeholders}) AND s.is_active = 0"
        ))
        .fetch_one(&self.pool)
        .await?;

        let weapon_cost = bind_all(format!(
            "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
             FROM kill_tool_stats ts \
             JOIN kills k ON k.id = ts.kill_id \
             WHERE k.session_id IN ({placeholders})"
        ))
        .fetch_one(&self.pool)
        .await?;

        let enhancer_cost = bind_all(format!(
            "SELECT COALESCE(SUM(k.enhancer_cost), 0) \
             FROM kills k \
             WHERE k.session_id IN ({placeholders})"
        ))
        .fetch_one(&self.pool)
        .await?;

        let loot_tt = bind_all(format!(
            "SELECT COALESCE(SUM(k.loot_total_ped), 0) \
             FROM kills k \
             WHERE k.session_id IN ({placeholders})"
        ))
        .fetch_one(&self.pool)
        .await?;

        let skill_tt = bind_all(format!(
            "SELECT COALESCE(SUM(sg.ped_value), 0) \
             FROM skill_gains sg \
             WHERE sg.session_id IN ({placeholders})"
        ))
        .fetch_one(&self.pool)
        .await?;

        Ok(json!({
            "linked_sessions": row_i64(&sess_row, 0),
            "total_duration": sql_number(&sess_row, 1),
            "weapon_cost": sql_number(&weapon_cost, 0),
            "heal_cost": sql_number(&sess_row, 2),
            "enhancer_cost": sql_number(&enhancer_cost, 0),
            "armour_cost": sql_number(&sess_row, 3),
            "loot_tt": sql_number(&loot_tt, 0),
            "skill_tt": sql_number(&skill_tt, 0),
        }))
    }

    async fn compute_playlist_reward_stats(
        &self,
        session_ids: &[String],
        immediate_ids: &[i64],
        long_horizon_ids: &[i64],
    ) -> Result<Value, QuestError> {
        if session_ids.is_empty() {
            return Ok(json!({
                "total_reward_ped": 0,
                "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0,
                "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0,
                "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0,
                "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0,
            }));
        }

        let immediate = self
            .sum_session_quest_rewards(session_ids, immediate_ids, false, None)
            .await?;
        let bonus = self
            .sum_session_quest_rewards(session_ids, long_horizon_ids, false, None)
            .await?;
        let immediate_skill = self
            .sum_session_quest_rewards(session_ids, immediate_ids, false, Some(true))
            .await?;
        let bonus_skill = self
            .sum_session_quest_rewards(session_ids, long_horizon_ids, false, Some(true))
            .await?;
        let expected_immediate = self
            .sum_session_quest_rewards(session_ids, immediate_ids, true, None)
            .await?;
        let expected_bonus = self
            .sum_session_quest_rewards(session_ids, long_horizon_ids, true, None)
            .await?;
        Ok(json!({
            "total_reward_ped": number_sum(&immediate, &bonus),
            "total_immediate_reward_ped": immediate,
            "total_bonus_reward_ped": bonus,
            "total_skill_reward_ped": number_sum(&immediate_skill, &bonus_skill),
            "total_immediate_skill_reward_ped": immediate_skill,
            "total_bonus_skill_reward_ped": bonus_skill,
            "total_expected_reward_ped": number_sum(&expected_immediate, &expected_bonus),
            "total_expected_immediate_reward_ped": expected_immediate,
            "total_expected_bonus_reward_ped": expected_bonus,
        }))
    }

    /// The summed rewards of a session set's completions over a quest
    /// set, optionally as the markup-expected value or filtered to
    /// skill rewards. NULL rewards contribute nothing to the sum; an
    /// empty id set short-circuits to the integer zero, and a falsy
    /// sum collapses to it, both as the original returns.
    async fn sum_session_quest_rewards(
        &self,
        session_ids: &[String],
        quest_ids: &[i64],
        expected: bool,
        skill_only: Option<bool>,
    ) -> Result<Value, QuestError> {
        if session_ids.is_empty() || quest_ids.is_empty() {
            return Ok(json!(0));
        }
        let session_placeholders = vec!["?"; session_ids.len()].join(",");
        let quest_placeholders = vec!["?"; quest_ids.len()].join(",");
        let reward_expr = if expected {
            "CASE \
                WHEN q.reward_is_skill = 1 OR q.reward_ped IS NULL THEN q.reward_ped \
                WHEN q.expected_reward_markup_percent IS NULL THEN q.reward_ped \
                ELSE q.reward_ped * q.expected_reward_markup_percent / 100.0 \
            END"
        } else {
            "q.reward_ped"
        };
        let skill_filter = match skill_only {
            Some(true) => " AND q.reward_is_skill = 1",
            Some(false) => " AND q.reward_is_skill = 0",
            None => "",
        };
        let sql = format!(
            "SELECT COALESCE(SUM({reward_expr}), 0) \
             FROM session_quest_completions sqc \
             JOIN quests q ON q.id = sqc.quest_id \
             WHERE sqc.session_id IN ({session_placeholders}) \
               AND sqc.quest_id IN ({quest_placeholders}) \
               {skill_filter}"
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for session_id in session_ids {
            query = query.bind(session_id.as_str());
        }
        for quest_id in quest_ids {
            query = query.bind(quest_id);
        }
        let row = query.fetch_one(&self.pool).await?;
        let value = sql_number(&row, 0);
        Ok(if json_truthy(Some(&value)) {
            value
        } else {
            json!(0)
        })
    }

    /// Playlist quest ids in item order, optionally filtered to one
    /// group.
    async fn playlist_quest_ids(
        &self,
        playlist_id: i64,
        group_type: Option<&str>,
    ) -> Result<Vec<i64>, QuestError> {
        let mut sql =
            String::from("SELECT quest_id FROM quest_playlist_items WHERE playlist_id = ?");
        if group_type.is_some() {
            sql.push_str(" AND group_type = ?");
        }
        sql.push_str(" ORDER BY sort_order");
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(playlist_id);
        if let Some(group_type) = group_type {
            query = query.bind(group_type);
        }
        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    // ── Completion and link records ─────────────────────────────────

    /// Insert an overlay event when a tracking session is active; any
    /// failure is swallowed, exactly as the original's bare except.
    async fn record_notable_event(&self, event_type: &str, description: &str, value_ped: f64) {
        // The original gates on truthiness, so an empty session id
        // skips the write exactly like an absent one.
        let Some(session_id) = self.lock_session().clone().filter(|id| !id.is_empty()) else {
            return;
        };
        let now = naive_to_epoch(self.clock.now());
        let _ = sqlx::query(
            "INSERT INTO notable_events \
             (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) \
             VALUES (?, NULL, ?, ?, ?, ?)",
        )
        .bind(&session_id)
        .bind(event_type)
        .bind(description)
        .bind(value_ped)
        .bind(now)
        .execute(&self.pool)
        .await;
    }

    /// Record a completion for cooldown and analytics: keyed by the
    /// active session, or a synthetic `manual-` key so a session-less
    /// completion still feeds the derived cooldown.
    async fn record_session_completion(
        &self,
        session_id: Option<&str>,
        quest_id: i64,
        completed_at: Option<f64>,
    ) -> Result<(), QuestError> {
        let key = match session_id {
            Some(session_id) => session_id.to_string(),
            None => format!("manual-{}", self.next_id()),
        };
        let ts = match completed_at {
            Some(ts) => ts,
            None => naive_to_epoch(self.clock.now()),
        };
        sqlx::query(
            "INSERT OR IGNORE INTO session_quest_completions \
             (session_id, quest_id, completed_at) VALUES (?, ?, ?)",
        )
        .bind(&key)
        .bind(quest_id)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn session_completed_quest_ids(&self, session_id: &str) -> Result<Vec<i64>, QuestError> {
        let rows = sqlx::query(
            "SELECT DISTINCT quest_id \
             FROM session_quest_completions \
             WHERE session_id = ? \
             ORDER BY quest_id",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    async fn session_analytics_link(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Option<i64>, Option<i64>)>, QuestError> {
        Ok(sqlx::query(
            "SELECT session_id, link_type, quest_id, playlist_id \
             FROM session_quest_analytics_links \
             WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| (row.get(1), row.get(2), row.get(3))))
    }

    async fn set_session_analytics_link(
        &self,
        session_id: &str,
        link_type: &str,
        quest_id: Option<i64>,
        playlist_id: Option<i64>,
    ) -> Result<(), QuestError> {
        sqlx::query(
            "INSERT INTO session_quest_analytics_links \
             (session_id, link_type, quest_id, playlist_id, linked_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(session_id) DO UPDATE SET \
                 link_type = excluded.link_type, \
                 quest_id = excluded.quest_id, \
                 playlist_id = excluded.playlist_id, \
                 linked_at = excluded.linked_at",
        )
        .bind(session_id)
        .bind(link_type)
        .bind(quest_id)
        .bind(playlist_id)
        .bind(naive_to_epoch(self.clock.now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Playlists whose immediate set is fully completed while every
    /// completion stays within the playlist's scope.
    async fn find_matching_playlists(
        &self,
        completed_quest_ids: &[i64],
    ) -> Result<Vec<i64>, QuestError> {
        let completed: HashSet<i64> = completed_quest_ids.iter().copied().collect();
        let mut matches = Vec::new();
        for playlist in self.get_playlists(true).await? {
            let ids = |key: &str| -> HashSet<i64> {
                playlist[key]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_i64)
                    .collect()
            };
            let immediate = ids("immediate_quest_ids");
            if immediate.is_empty() {
                continue;
            }
            let mut scope = immediate.clone();
            scope.extend(ids("long_horizon_quest_ids"));
            if immediate.is_subset(&completed) && completed.is_subset(&scope) {
                matches.push(playlist["id"].as_i64().expect("playlist id"));
            }
        }
        Ok(matches)
    }

    async fn quest_name(&self, quest_id: Option<i64>) -> Result<Option<String>, QuestError> {
        let Some(quest_id) = quest_id else {
            return Ok(None);
        };
        Ok(sqlx::query("SELECT name FROM quests WHERE id = ?")
            .bind(quest_id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| row.get(0)))
    }

    async fn playlist_name(&self, playlist_id: Option<i64>) -> Result<Option<String>, QuestError> {
        let Some(playlist_id) = playlist_id else {
            return Ok(None);
        };
        Ok(sqlx::query("SELECT name FROM quest_playlists WHERE id = ?")
            .bind(playlist_id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| row.get(0)))
    }

    /// Whether the quest's cooldown window is still open against the
    /// injected clock.
    fn is_quest_cooling(&self, quest: &Value) -> bool {
        let last = quest.get("last_completed_at").and_then(Value::as_f64);
        let cooldown_hours = quest.get("cooldown_hours").and_then(Value::as_f64);
        let (Some(last), Some(cooldown_hours)) = (last, cooldown_hours) else {
            return false;
        };
        if cooldown_hours <= 0.0 {
            return false;
        }
        (last + cooldown_hours * 3600.0) > naive_to_epoch(self.clock.now())
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

/// The expected total reward over a completion count: skill rewards
/// and unmarked rewards multiply plainly; marked positive liquid
/// rewards apply the markup percentage. A non-positive count is the
/// integer zero, and the collapsed integer-zero reward multiplies in
/// integers, both exactly as the original returns them.
fn expected_reward_total(
    reward: &Value,
    reward_is_skill: bool,
    expected_markup: Option<f64>,
    completions: i64,
) -> Value {
    if completions <= 0 {
        return json!(0);
    }
    let Some(reward_ped) = reward.as_f64().filter(|_| reward.is_f64()) else {
        return json!(reward.as_i64().unwrap_or(0) * completions);
    };
    match expected_markup {
        Some(markup) if !reward_is_skill && reward_ped > 0.0 => {
            json!(reward_ped * (markup / 100.0) * completions as f64)
        }
        _ => json!(reward_ped * completions as f64),
    }
}

/// A COUNT column: always an integer.
fn row_i64(row: &sqlx::sqlite::SqliteRow, index: usize) -> i64 {
    row.get::<i64, _>(index)
}

/// An aggregate column with the engine's own numeric type: SQLite
/// returns INTEGER for empty-set COALESCE fallbacks and integer sums,
/// REAL otherwise, and the original emits whichever arrives.
fn sql_number(row: &sqlx::sqlite::SqliteRow, index: usize) -> Value {
    match row.try_get::<f64, _>(index) {
        Ok(value) => json!(value),
        Err(_) => json!(row.get::<i64, _>(index)),
    }
}

/// The sum of two engine-typed numbers, integer when both are (the
/// original's Python addition).
fn number_sum(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(left), Some(right)) => json!(left + right),
        _ => json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0)),
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

/// Delete the newest claim for a quest (the cancel flow's undo).
async fn delete_latest_quest_claim(
    conn: &mut SqliteConnection,
    quest_id: i64,
) -> Result<bool, QuestError> {
    let Some(row) = sqlx::query(
        "SELECT id FROM quest_claims \
         WHERE quest_id = ? \
         ORDER BY claimed_at DESC, id DESC \
         LIMIT 1",
    )
    .bind(quest_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query("DELETE FROM quest_claims WHERE id = ?")
        .bind(row.get::<i64, _>(0))
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// Delete the newest matching quest-reward ledger entry (the cancel
/// flow's undo for liquid rewards).
async fn delete_latest_quest_reward_entry(
    conn: &mut SqliteConnection,
    quest_name: &str,
    reward_ped: f64,
) -> Result<bool, QuestError> {
    let Some(row) = sqlx::query(
        "SELECT id FROM ledger_entries \
         WHERE type = 'markup' \
           AND tag = 'quest_reward' \
           AND description = ? \
           AND amount = ? \
         ORDER BY date DESC, id DESC \
         LIMIT 1",
    )
    .bind(format!("Quest: {quest_name}"))
    .bind(reward_ped)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query("DELETE FROM ledger_entries WHERE id = ?")
        .bind(row.get::<String, _>(0))
        .execute(&mut *conn)
        .await?;
    Ok(true)
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
/// immediate items. A present-but-null `items` falls through to the
/// `quest_ids` leg exactly as the original's is-not-None test does,
/// so `{"items": null}` alone clears the playlist (the original's
/// semantics, pinned). A present-but-null (or non-list) `quest_ids`
/// refuses instead: the original crashes iterating it (an unhandled
/// error on the wire, with no surviving write), and the update path
/// is reachable with an explicit null through the route model.
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
// outputs, computed by running the original Python implementation
// over byte-identical payloads and database seeds (created_at and
// updated_at pinned by direct UPDATE on both sides, since the schema
// stamps them from the wall clock).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn service_with_clock(
        dir: &std::path::Path,
    ) -> (Arc<QuestService>, SqlitePool, Arc<crate::clock::MockClock>) {
        let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
        let pool = db.pool().clone();
        let clock = Arc::new(crate::clock::MockClock::new(
            Some(
                chrono::NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                    .unwrap(),
            ),
            0.0,
        ));
        let svc = Arc::new(QuestService::new(pool.clone(), clock.clone()));
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        svc.set_id_source(Arc::new(move || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            format!("fixed-{n:04}")
        }));
        (svc, pool, clock)
    }

    async fn service(dir: &std::path::Path) -> (Arc<QuestService>, SqlitePool) {
        let (svc, pool, _clock) = service_with_clock(dir).await;
        (svc, pool)
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

    #[tokio::test]
    async fn mob_autocomplete_lists_active_quest_mobs_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        svc.create_quest(&full_quest_payload()).await.unwrap();
        svc.create_quest(&json!({"name": "Side Hunt", "mobs": ["Snablesnot"]}))
            .await
            .unwrap();
        assert_eq!(
            svc.get_all_mob_names().await.unwrap(),
            ["Atrax", "Atrox", "Snablesnot"]
        );
    }

    #[tokio::test]
    async fn a_zero_hour_cooldown_never_produces_an_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let mut payload = full_quest_payload();
        payload["cooldown_hours"] = json!(0);
        let q = quest_id(&svc.create_quest(&payload).await.unwrap());
        sqlx::query(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('sess-1', ?, 1772366400.0)",
        )
        .bind(q)
        .execute(&pool)
        .await
        .unwrap();

        let quest = svc.get_quest(q).await.unwrap().unwrap();
        assert_eq!(quest["last_completed_at"], json!(1772366400.0));
        assert_eq!(
            quest["cooldown_expires_at"],
            Value::Null,
            "the expiry derives only from a strictly positive cooldown"
        );
    }

    #[tokio::test]
    async fn a_null_items_payload_clears_the_playlist() {
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

        // The original's is-not-None test routes a present-null items
        // payload to the quest_ids leg, which is absent, so the
        // rewrite clears every item; null items is the documented
        // clear-all shape, unlike null quest_ids which refuses.
        let updated = svc
            .update_playlist(p1, &json!({"items": null}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["quest_ids"], json!([]));
        assert_eq!(updated["items"], json!([]));
    }

    /// One walk through the lifecycle, mirroring the original's run
    /// over identical payloads, clock advances, and identifier
    /// streams; every expected value below is the original's output.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_lifecycle_walkthrough_matches_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool, clock) = service_with_clock(dir.path()).await;
        let bus = Arc::new(crate::event_bus::EventBus::new());
        svc.subscribe(&bus, Handle::current());

        let qa = quest_id(
            &svc.create_quest(
                &json!({"name": "Iron Challenge", "reward_ped": 2.5, "cooldown_hours": 24}),
            )
            .await
            .unwrap(),
        );
        pin_ts(&pool, "quests", qa, 1000.0).await;
        let qb = quest_id(
            &svc.create_quest(&json!({"name": "Daily Hunt: Atrox", "reward_ped": 5.0,
                                       "reward_is_skill": true, "cooldown_hours": 1}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qb, 1001.0).await;
        let qc = quest_id(
            &svc.create_quest(&json!({"name": "G\u{e9}ologist Survey"}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qc, 1002.0).await;
        let qe = quest_id(
            &svc.create_quest(&json!({"name": "Zero Bounty", "reward_ped": 0}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qe, 1003.0).await;

        // Start legs.
        assert_eq!(svc.start_quest(9999).await.unwrap(), None);
        let started = svc.start_quest(qa).await.unwrap().unwrap();
        assert_eq!(started["started_at"], json!(1772366400.0));

        // A session-less completion: a ledger row (liquid reward) and
        // a synthetic manual completion key.
        clock.advance(60.0).unwrap();
        let done = svc.complete_quest(qa).await.unwrap().unwrap();
        assert_eq!(done["started_at"], Value::Null);
        assert_eq!(done["last_completed_at"], json!(1772366460.0));
        assert_eq!(
            done["cooldown_expires_at"],
            json!("2026-03-02T12:01:00+00:00")
        );
        let ledger = |sql: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(sql)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
                    .iter()
                    .map(|row| {
                        json!([
                            row.get::<String, _>(0),
                            row.get::<String, _>(1),
                            row.get::<String, _>(2),
                            row.get::<f64, _>(3),
                            row.get::<String, _>(4),
                        ])
                    })
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(
            ledger("SELECT id, date, description, amount, tag FROM ledger_entries ORDER BY id")
                .await,
            vec![json!([
                "fixed-0001",
                "2026-03-01T12:01:00+00:00",
                "Quest: Iron Challenge",
                2.5,
                "quest_reward"
            ])]
        );
        let completions = |pool: SqlitePool| async move {
            sqlx::query(
                "SELECT session_id, quest_id, completed_at FROM session_quest_completions ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|row| {
                json!([
                    row.get::<String, _>(0),
                    row.get::<i64, _>(1),
                    row.get::<f64, _>(2)
                ])
            })
            .collect::<Vec<_>>()
        };
        assert_eq!(
            completions(pool.clone()).await,
            vec![json!(["manual-fixed-0002", qa, 1772366460.0])]
        );

        // The bus feeds the active session; a session-scoped skill
        // completion writes a claim, and a repeat in the same session
        // dedupes the completion while duplicating the claim.
        bus.publish(Topic::SessionStarted, &json!({"session_id": "sess-abc"}));
        clock.advance(60.0).unwrap();
        svc.complete_quest(qb).await.unwrap().unwrap();
        clock.advance(60.0).unwrap();
        svc.complete_quest(qb).await.unwrap().unwrap();
        let claims = sqlx::query(
            "SELECT quest_id, quest_name, ped_value, claimed_at FROM quest_claims ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            json!([
                row.get::<i64, _>(0),
                row.get::<String, _>(1),
                row.get::<f64, _>(2),
                row.get::<f64, _>(3)
            ])
        })
        .collect::<Vec<_>>();
        assert_eq!(
            claims,
            vec![
                json!([qb, "Daily Hunt: Atrox", 5.0, 1772366520.0]),
                json!([qb, "Daily Hunt: Atrox", 5.0, 1772366580.0]),
            ]
        );
        assert_eq!(
            completions(pool.clone()).await,
            vec![
                json!(["manual-fixed-0002", qa, 1772366460.0]),
                json!(["sess-abc", qb, 1772366520.0]),
            ]
        );

        // Cancel legs: a started quest clears; a quest neither started
        // nor cooling passes through; a cooling quest resets its
        // cooldown and (optionally) undoes the reward.
        svc.start_quest(qc).await.unwrap().unwrap();
        let cancelled = svc.cancel_quest(qc, false).await.unwrap().unwrap();
        assert_eq!(cancelled["started_at"], Value::Null);
        let passthrough = svc.cancel_quest(qc, false).await.unwrap().unwrap();
        assert_eq!(passthrough["id"], json!(qc));
        clock.advance(60.0).unwrap();
        svc.cancel_quest(qb, true).await.unwrap().unwrap();
        let claim_count: i64 = sqlx::query("SELECT COUNT(*) FROM quest_claims")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(claim_count, 1, "the newest claim is undone");
        svc.cancel_quest(qa, true).await.unwrap().unwrap();
        let ledger_count: i64 = sqlx::query("SELECT COUNT(*) FROM ledger_entries")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(ledger_count, 0, "the reward ledger entry is undone");

        // The suggestion tree, reason by reason.
        let sugg = |s: Value, t: &str, r: &str| {
            assert_eq!(s["suggestion_type"], t, "type for {r}");
            assert_eq!(s["reason"], r);
            s
        };
        sugg(
            svc.get_session_link_suggestion("sess-none").await.unwrap(),
            "none",
            "no_completions",
        );
        for (session, quest, at) in [
            ("sess-one", qa, 5000.0),
            ("sess-two", qa, 5001.0),
            ("sess-two", qb, 5002.0),
        ] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(session)
            .bind(quest)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let single = sugg(
            svc.get_session_link_suggestion("sess-one").await.unwrap(),
            "quest",
            "single_quest",
        );
        assert_eq!(single["quest_name"], "Iron Challenge");
        svc.create_playlist(&json!({"name": "Pair Run", "quest_ids": [qa, qb]}))
            .await
            .unwrap();
        let pl = sugg(
            svc.get_session_link_suggestion("sess-two").await.unwrap(),
            "playlist",
            "exact_playlist",
        );
        assert_eq!(pl["playlist_name"], "Pair Run");
        clock.advance(60.0).unwrap();
        svc.accept_session_link_suggestion("sess-two")
            .await
            .unwrap();
        sugg(
            svc.get_session_link_suggestion("sess-two").await.unwrap(),
            "none",
            "already_linked",
        );
        svc.decline_session_link("sess-decl").await.unwrap();
        sugg(
            svc.get_session_link_suggestion("sess-decl").await.unwrap(),
            "none",
            "declined",
        );
        let error = svc
            .accept_session_link_suggestion("sess-none")
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "No linkable suggestion for session sess-none: no_completions"
        );
        svc.accept_session_link_suggestion("sess-one")
            .await
            .unwrap();
        for (session, quest, at) in [("sess-three", qa, 5003.0), ("sess-three", qc, 5004.0)] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(session)
            .bind(quest)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sugg(
            svc.get_session_link_suggestion("sess-three").await.unwrap(),
            "none",
            "unclean",
        );
        svc.create_playlist(&json!({"name": "Pair Run B", "quest_ids": [qa, qb]}))
            .await
            .unwrap();
        for (session, quest, at) in [("sess-five", qa, 5005.0), ("sess-five", qb, 5006.0)] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(session)
            .bind(quest)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sugg(
            svc.get_session_link_suggestion("sess-five").await.unwrap(),
            "none",
            "ambiguous_playlist",
        );
        let links = sqlx::query(
            "SELECT session_id, link_type, quest_id, playlist_id, linked_at \
             FROM session_quest_analytics_links ORDER BY session_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            json!([
                row.get::<String, _>(0),
                row.get::<String, _>(1),
                row.get::<Option<i64>, _>(2),
                row.get::<Option<i64>, _>(3),
                row.get::<f64, _>(4)
            ])
        })
        .collect::<Vec<_>>();
        assert_eq!(
            links,
            vec![
                json!(["sess-decl", "declined", null, null, 1772366700.0]),
                json!(["sess-one", "quest", qa, null, 1772366700.0]),
                json!(["sess-two", "playlist", null, 1, 1772366700.0]),
            ]
        );

        // Mission matching: exact (case/space), accent folding,
        // repeatable suffix, containment, fuzzy at the threshold, and
        // a miss below it.
        let match_id = |name: &'static str| {
            let svc = svc.clone();
            async move {
                svc.match_quest_by_mission_name(name)
                    .await
                    .unwrap()
                    .map(|quest| quest["id"].as_i64().unwrap())
            }
        };
        assert_eq!(match_id("  IRON CHALLENGE ").await, Some(qa));
        assert_eq!(match_id("Geologist Survey").await, Some(qc));
        assert_eq!(match_id("Iron Challenge (Repeatable)").await, Some(qa));
        assert_eq!(match_id("Mission: Iron Challenge Part II").await, Some(qa));
        assert_eq!(match_id("Iron Chalenge").await, Some(qa));
        assert_eq!(match_id("Totally Different").await, None);

        // Mission auto-start: unknown ignores, a fuzzy match starts
        // once, and an already-started quest skips.
        clock.advance(60.0).unwrap();
        svc.start_quest_from_mission("Unknown Mission")
            .await
            .unwrap();
        svc.start_quest_from_mission("Iron Chalenge").await.unwrap();
        assert!(json_truthy(
            svc.get_quest(qa).await.unwrap().unwrap().get("started_at")
        ));
        svc.start_quest_from_mission("Iron Challenge")
            .await
            .unwrap();

        // The reward filter's five legs.
        clock.advance(60.0).unwrap();
        assert_eq!(
            svc.quest_reward_filter(
                "Daily Hunt: Atrox",
                &[],
                &[json!({"skill_name": "Rifle", "amount": 1.0})]
            )
            .await
            .unwrap(),
            Some(json!({"suppress_loot_index": null, "suppress_skill_index": 0}))
        );
        clock.advance(60.0).unwrap();
        assert_eq!(
            svc.quest_reward_filter(
                "Iron Challenge",
                &[
                    json!({"item_name": "Shrapnel", "quantity": 100, "value": 0.1}),
                    json!({"item_name": "Universal Ammo", "quantity": 1, "value": 2.51}),
                ],
                &[]
            )
            .await
            .unwrap(),
            Some(json!({"suppress_loot_index": 1, "suppress_skill_index": null}))
        );
        clock.advance(60.0).unwrap();
        assert_eq!(
            svc.quest_reward_filter(
                "Iron Challenge",
                &[json!({"item_name": "Shrapnel", "quantity": 100, "value": 0.1})],
                &[]
            )
            .await
            .unwrap(),
            None
        );
        clock.advance(60.0).unwrap();
        assert_eq!(
            svc.quest_reward_filter(
                "Zero Bounty",
                &[
                    json!({"item_name": "A", "value": 0.5}),
                    json!({"item_name": "B", "value": 0.2}),
                    json!({"item_name": "C", "value": 0.9}),
                ],
                &[]
            )
            .await
            .unwrap(),
            Some(json!({"suppress_loot_index": 1, "suppress_skill_index": null}))
        );
        clock.advance(60.0).unwrap();
        assert_eq!(
            svc.quest_reward_filter(
                "Geologist Survey",
                &[json!({"item_name": "A", "value": 0.5})],
                &[]
            )
            .await
            .unwrap(),
            None
        );

        // The overlay trail, exactly as the original recorded it.
        let events = sqlx::query(
            "SELECT session_id, kill_id, event_type, mob_or_item, value_ped, timestamp \
             FROM notable_events ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            json!([
                row.get::<String, _>(0),
                row.get::<Option<String>, _>(1),
                row.get::<String, _>(2),
                row.get::<String, _>(3),
                row.get::<f64, _>(4),
                row.get::<f64, _>(5)
            ])
        })
        .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                json!([
                    "sess-abc",
                    null,
                    "quest_started",
                    "Iron Challenge",
                    0.0,
                    1772366760.0
                ]),
                json!([
                    "sess-abc",
                    null,
                    "quest_completed_pes",
                    "Daily Hunt: Atrox: skill reward suppressed",
                    5.0,
                    1772366820.0
                ]),
                json!([
                    "sess-abc",
                    null,
                    "quest_completed",
                    "Iron Challenge: Universal Ammo (2.50 PED) suppressed",
                    2.5,
                    1772366880.0
                ]),
                json!([
                    "sess-abc",
                    null,
                    "quest_completed",
                    "Iron Challenge",
                    2.5,
                    1772366940.0
                ]),
                json!([
                    "sess-abc",
                    null,
                    "quest_completed",
                    "Zero Bounty: B suppressed",
                    0.0,
                    1772367000.0
                ]),
                json!([
                    "sess-abc",
                    null,
                    "quest_completed",
                    "G\u{e9}ologist Survey",
                    0.0,
                    1772367060.0
                ]),
            ]
        );

        // The final ledger carries exactly the two liquid completions
        // the filter recorded; the zero-reward completion wrote none.
        let final_ledger: Vec<String> = sqlx::query("SELECT id FROM ledger_entries ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(final_ledger, ["fixed-0003", "fixed-0004"]);

        // A session stop clears the tracked session: notable events
        // stop recording.
        bus.publish(Topic::SessionStopped, &json!({}));
        svc.start_quest_from_mission("Geologist Survey")
            .await
            .unwrap();
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM notable_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 6, "no session, no overlay event");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_received_mission_event_starts_its_quest() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool, _clock) = service_with_clock(dir.path()).await;
        let bus = Arc::new(crate::event_bus::EventBus::new());
        svc.subscribe(&bus, Handle::current());
        let q = quest_id(
            &svc.create_quest(&json!({"name": "Iron Challenge"}))
                .await
                .unwrap(),
        );

        bus.publish(
            Topic::MissionReceived,
            &json!({"mission_name": "Iron Challenge"}),
        );
        assert!(json_truthy(
            svc.get_quest(q).await.unwrap().unwrap().get("started_at")
        ));
        // A nameless event is ignored.
        bus.publish(Topic::MissionReceived, &json!({}));
    }

    #[tokio::test]
    async fn starting_an_inactive_quest_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        let q = quest_id(&svc.create_quest(&json!({"name": "Dead"})).await.unwrap());
        svc.delete_quest(q).await.unwrap();
        assert_eq!(svc.start_quest(q).await.unwrap(), None);
    }

    #[tokio::test]
    async fn equal_fuzzy_scores_keep_the_first_quest() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let first = quest_id(
            &svc.create_quest(&json!({"name": "iron chal a"}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", first, 1000.0).await;
        let second = quest_id(
            &svc.create_quest(&json!({"name": "iron chal b"}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", second, 1001.0).await;

        // Both names score 0.9090909090909091 against the mission (the
        // reference's figure); the strictly-greater comparison keeps
        // the earlier quest.
        let matched = svc
            .match_quest_by_mission_name("iron chal c")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(matched["id"], json!(first));
    }

    #[tokio::test]
    async fn filter_ties_keep_the_first_item() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, _pool) = service(dir.path()).await;
        svc.create_quest(&json!({"name": "Tie Quest", "reward_ped": 2.5}))
            .await
            .unwrap();
        svc.create_quest(&json!({"name": "Zed Bounty", "reward_ped": 0}))
            .await
            .unwrap();

        // Equal absolute differences (2.49 and 2.51 against 2.5) keep
        // the first item, as the original's strictly-less tracking does.
        assert_eq!(
            svc.quest_reward_filter(
                "Tie Quest",
                &[
                    json!({"item_name": "A", "value": 2.49}),
                    json!({"item_name": "B", "value": 2.51}),
                ],
                &[]
            )
            .await
            .unwrap(),
            Some(json!({"suppress_loot_index": 0, "suppress_skill_index": null}))
        );
        // Equal minimum values likewise keep the first item.
        assert_eq!(
            svc.quest_reward_filter(
                "Zed Bounty",
                &[
                    json!({"item_name": "A", "value": 0.3}),
                    json!({"item_name": "B", "value": 0.3}),
                ],
                &[]
            )
            .await
            .unwrap(),
            Some(json!({"suppress_loot_index": 0, "suppress_skill_index": null}))
        );
    }

    #[tokio::test]
    async fn playlist_matching_requires_completions_within_scope() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let qa = quest_id(&svc.create_quest(&json!({"name": "Alpha"})).await.unwrap());
        let qc = quest_id(&svc.create_quest(&json!({"name": "Gamma"})).await.unwrap());
        svc.create_playlist(&json!({"name": "Solo Run", "quest_ids": [qc]}))
            .await
            .unwrap();
        for (quest, at) in [(qa, 5003.0), (qc, 5004.0)] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES ('s3', ?, ?)",
            )
            .bind(quest)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // The playlist's immediate set is complete, but the session
        // also completed a quest outside its scope: both subset tests
        // must hold, so the suggestion stays unclean.
        let suggestion = svc.get_session_link_suggestion("s3").await.unwrap();
        assert_eq!(suggestion["reason"], "unclean");
    }

    #[tokio::test]
    async fn cancelling_outside_the_cooldown_window_keeps_completions() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;
        let qa = quest_id(&svc.create_quest(&json!({"name": "Alpha"})).await.unwrap());
        sqlx::query(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('s4', ?, 1000.0)",
        )
        .bind(qa)
        .execute(&pool)
        .await
        .unwrap();

        // No cooldown configured: never cooling, the completion stays.
        let result = svc.cancel_quest(qa, false).await.unwrap().unwrap();
        assert_eq!(result["last_completed_at"], json!(1000.0));

        // A cooldown that expires exactly at the current instant is no
        // longer cooling (the strict comparison), so the completion
        // stays here too.
        let qe = quest_id(
            &svc.create_quest(&json!({"name": "Edge", "cooldown_hours": 1}))
                .await
                .unwrap(),
        );
        sqlx::query(
            "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
             VALUES ('s5', ?, ?)",
        )
        .bind(qe)
        .bind(1772366400.0 - 3600.0)
        .execute(&pool)
        .await
        .unwrap();
        let result = svc.cancel_quest(qe, false).await.unwrap().unwrap();
        assert_eq!(result["last_completed_at"], json!(1772362800.0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_empty_session_id_skips_overlay_events() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool, _clock) = service_with_clock(dir.path()).await;
        let bus = Arc::new(crate::event_bus::EventBus::new());
        svc.subscribe(&bus, Handle::current());
        svc.create_quest(&json!({"name": "Iron Challenge"}))
            .await
            .unwrap();

        // The original's truthiness gate treats an empty session id as
        // no session: the quest starts but no overlay event records.
        bus.publish(Topic::SessionStarted, &json!({"session_id": ""}));
        svc.start_quest_from_mission("Iron Challenge")
            .await
            .unwrap();
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM notable_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0);
    }

    /// The analytics readers over a seeded economy, with every
    /// expected object computed by the original implementation over
    /// byte-identical seeds (engine numeric types preserved: integer
    /// zeros from NULL sums, REAL zeros from real columns, and the
    /// raw float artefacts of the engine's arithmetic).
    #[tokio::test]
    async fn analytics_match_the_original_over_a_seeded_economy() {
        let dir = tempfile::tempdir().unwrap();
        let (svc, pool) = service(dir.path()).await;

        let qa = quest_id(
            &svc.create_quest(&json!({"name": "Alpha", "reward_ped": 2.5,
                                       "expected_reward_markup_percent": 150.0}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qa, 1000.0).await;
        let qb = quest_id(
            &svc.create_quest(&json!({"name": "Beta", "reward_ped": 5.0,
                                       "reward_is_skill": true}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qb, 1001.0).await;
        let qc = quest_id(&svc.create_quest(&json!({"name": "Gamma"})).await.unwrap());
        pin_ts(&pool, "quests", qc, 1002.0).await;
        let qd = quest_id(
            &svc.create_quest(&json!({"name": "Delta", "reward_ped": 1.25}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qd, 1003.0).await;

        let p1 = quest_id(
            &svc.create_playlist(&json!({"name": "Mixed Run", "items": [
                {"quest_id": qa, "group_type": "immediate"},
                {"quest_id": qb, "group_type": "immediate"},
                {"quest_id": qd, "group_type": "long_horizon"},
            ]}))
            .await
            .unwrap(),
        );
        pin_ts(&pool, "quest_playlists", p1, 2000.0).await;
        let p2 = quest_id(
            &svc.create_playlist(&json!({"name": "Bonus Only", "items": [
                {"quest_id": qc, "group_type": "long_horizon"},
            ]}))
            .await
            .unwrap(),
        );
        pin_ts(&pool, "quest_playlists", p2, 2001.0).await;

        for (sid, st, en, active, heal, armour) in [
            ("sess-1", 1000.0, Some(4600.0), 0i64, Some(1.5), Some(0.25)),
            ("sess-2", 5000.0, Some(5030.5), 0, None, Some(0.0)),
            ("sess-3", 6000.0, None, 1, Some(2.0), None),
        ] {
            sqlx::query(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(sid)
            .bind(st)
            .bind(en)
            .bind(active)
            .bind(heal)
            .bind(armour)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (kid, sid, mob, ts, enh, loot) in [
            ("k1", "sess-1", "Atrox", 1100.0, 0.5, 12.75),
            ("k2", "sess-1", "Atrox", 1200.0, 0.0, 3.0),
            ("k3", "sess-2", "Snable", 5010.0, 0.1, 0.0),
        ] {
            sqlx::query(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, shots_fired, damage_dealt, \
                 damage_taken, critical_hits, cost_ped, enhancer_cost, loot_total_ped) \
                 VALUES (?, ?, ?, ?, 10, 100.0, 5.0, 1, 0.3, ?, ?)",
            )
            .bind(kid)
            .bind(sid)
            .bind(mob)
            .bind(ts)
            .bind(enh)
            .bind(loot)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (kid, tool, shots, cps) in [
            ("k1", "LR-32", 40i64, 0.05),
            ("k1", "Fap-90", 5, 0.02),
            ("k3", "LR-32", 12, 0.05),
        ] {
            sqlx::query(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
                 critical_hits, cost_per_shot) VALUES (?, ?, ?, 50.0, 0, ?)",
            )
            .bind(kid)
            .bind(tool)
            .bind(shots)
            .bind(cps)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (sid, skill, ped) in [("sess-1", "Rifle", 0.8), ("sess-2", "Anatomy", 0.2)] {
            sqlx::query(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, 1100.0, ?, 1.0, ?)",
            )
            .bind(sid)
            .bind(skill)
            .bind(ped)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (sid, qid, at) in [
            ("sess-1", qa, 1500.0),
            ("sess-1", qb, 1600.0),
            ("sess-1", qd, 1700.0),
            ("sess-2", qa, 5020.0),
        ] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(sid)
            .bind(qid)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let qn = quest_id(&svc.create_quest(&json!({"name": "Nul"})).await.unwrap());
        pin_ts(&pool, "quests", qn, 1004.0).await;
        let qz = quest_id(
            &svc.create_quest(&json!({"name": "Zed", "reward_ped": 0,
                                       "expected_reward_markup_percent": 120.0}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qz, 1005.0).await;
        let qe2 = quest_id(
            &svc.create_quest(&json!({"name": "Echo", "reward_ped": 3.0}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quests", qe2, 1006.0).await;
        for (sid, st, en, active, heal) in [
            ("sess-n", 7000.0, Some(7050.0), 0i64, Some(0.0)),
            ("sess-z", 7100.0, Some(7160.0), 0, Some(0.0)),
            ("sess-act", 8000.0, None, 1, None),
            ("sess-solo", 8100.0, Some(8200.0), 0, Some(0.5)),
        ] {
            sqlx::query(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost, armour_cost) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(sid)
            .bind(st)
            .bind(en)
            .bind(active)
            .bind(heal)
            .bind(heal.map(|_| 0.0))
            .execute(&pool)
            .await
            .unwrap();
        }
        for (sid, qid, at) in [("sess-n", qn, 7040.0), ("sess-z", qz, 7150.0)] {
            sqlx::query(
                "INSERT INTO session_quest_completions (session_id, quest_id, completed_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(sid)
            .bind(qid)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let p3 = quest_id(
            &svc.create_playlist(&json!({"name": "Solo Immediate", "quest_ids": [qa]}))
                .await
                .unwrap(),
        );
        pin_ts(&pool, "quest_playlists", p3, 2002.0).await;
        for (sid, lt, qid, plid) in [
            ("sess-1", "playlist", None::<i64>, Some(p1)),
            ("sess-2", "quest", Some(qa), None),
            ("sess-3", "quest", Some(qa), None),
            ("sess-n", "quest", Some(qn), None),
            ("sess-z", "quest", Some(qz), None),
            ("sess-act", "quest", Some(qe2), None),
            ("sess-solo", "playlist", None, Some(p3)),
        ] {
            sqlx::query(
                "INSERT INTO session_quest_analytics_links \
                 (session_id, link_type, quest_id, playlist_id, linked_at) \
                 VALUES (?, ?, ?, ?, 9000.0)",
            )
            .bind(sid)
            .bind(lt)
            .bind(qid)
            .bind(plid)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Per-quest, name-ordered: Alpha (the still-active linked
        // session is excluded from the completed count but rides in
        // the id set), then the NULL-reward and zero-reward quests
        // whose collapsed rewards and expected totals stay INTEGER
        // zeros on the wire; Echo (linked only by an active session)
        // is excluded entirely.
        assert_eq!(
            svc.get_quest_analytics().await.unwrap(),
            vec![
                json!({
                    "quest_id": qa, "quest_name": "Alpha", "planet": "Calypso",
                    "category": null, "reward_ped": 2.5, "reward_is_skill": false,
                    "expected_reward_markup_percent": 150.0,
                    "total_expected_reward_ped": 3.75,
                    "linked_sessions": 1, "total_duration": 30.5,
                    "weapon_cost": 0.6000000000000001, "heal_cost": 0,
                    "enhancer_cost": 0.1, "armour_cost": 0.0, "loot_tt": 0.0,
                    "skill_tt": 0.2,
                }),
                json!({
                    "quest_id": qn, "quest_name": "Nul", "planet": "Calypso",
                    "category": null, "reward_ped": 0, "reward_is_skill": false,
                    "expected_reward_markup_percent": null,
                    "total_expected_reward_ped": 0,
                    "linked_sessions": 1, "total_duration": 50.0,
                    "weapon_cost": 0, "heal_cost": 0.0,
                    "enhancer_cost": 0, "armour_cost": 0.0, "loot_tt": 0,
                    "skill_tt": 0,
                }),
                json!({
                    "quest_id": qz, "quest_name": "Zed", "planet": "Calypso",
                    "category": null, "reward_ped": 0, "reward_is_skill": false,
                    // The zero reward normalised its markup away at
                    // creation, exactly as the original stores it.
                    "expected_reward_markup_percent": null,
                    "total_expected_reward_ped": 0,
                    "linked_sessions": 1, "total_duration": 60.0,
                    "weapon_cost": 0, "heal_cost": 0.0,
                    "enhancer_cost": 0, "armour_cost": 0.0, "loot_tt": 0,
                    "skill_tt": 0,
                }),
            ]
        );

        // An immediate-only playlist with a linked session that
        // completed nothing in scope: real session stats beside
        // integer-zero reward sums (the empty long-horizon set
        // short-circuits without touching SQL).
        assert_eq!(
            svc.get_playlist_analytics(p3).await.unwrap().unwrap(),
            json!({
                "playlist_id": p3, "playlist_name": "Solo Immediate", "quest_count": 1,
                "long_horizon_quest_count": 0,
                "total_reward_ped": 0, "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0, "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0, "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0, "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0,
                "matched_sessions": 1, "linked_sessions": 1, "total_duration": 100.0,
                "weapon_cost": 0, "heal_cost": 0.5, "enhancer_cost": 0,
                "armour_cost": 0.0, "loot_tt": 0, "skill_tt": 0,
            })
        );

        let p1_stats = svc.get_playlist_analytics(p1).await.unwrap().unwrap();
        assert_eq!(
            p1_stats,
            json!({
                "playlist_id": p1, "playlist_name": "Mixed Run", "quest_count": 2,
                "long_horizon_quest_count": 1,
                "total_reward_ped": 8.75, "total_immediate_reward_ped": 7.5,
                "total_bonus_reward_ped": 1.25, "total_skill_reward_ped": 5.0,
                "total_immediate_skill_reward_ped": 5.0, "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 10.0,
                "total_expected_immediate_reward_ped": 8.75,
                "total_expected_bonus_reward_ped": 1.25,
                "matched_sessions": 1, "linked_sessions": 1, "total_duration": 3600.0,
                "weapon_cost": 2.1, "heal_cost": 1.5, "enhancer_cost": 0.5,
                "armour_cost": 0.25, "loot_tt": 15.75, "skill_tt": 0.8,
            })
        );

        // An empty immediate set is the zeroed early-return shape
        // (which carries matched_sessions but no linked_sessions).
        let p2_stats = svc.get_playlist_analytics(p2).await.unwrap().unwrap();
        assert_eq!(
            p2_stats,
            json!({
                "playlist_id": p2, "playlist_name": "Bonus Only", "quest_count": 0,
                "long_horizon_quest_count": 1, "matched_sessions": 0,
                "total_reward_ped": 0, "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0, "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0, "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0, "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0, "total_duration": 0,
                "weapon_cost": 0, "heal_cost": 0, "enhancer_cost": 0,
                "armour_cost": 0, "loot_tt": 0, "skill_tt": 0,
            })
        );

        let p3_stats = svc.get_playlist_analytics(p3).await.unwrap().unwrap();
        assert_eq!(
            svc.get_all_playlist_analytics().await.unwrap(),
            vec![p1_stats, p2_stats, p3_stats]
        );
        assert_eq!(svc.get_playlist_analytics(9999).await.unwrap(), None);
    }

    #[test]
    fn truthiness_matches_python_bool() {
        // The expectation table is Python's bool() over each shape.
        assert!(!json_truthy(None));
        assert!(!json_truthy(Some(&json!(null))));
        assert!(!json_truthy(Some(&json!(false))));
        assert!(json_truthy(Some(&json!(true))));
        assert!(!json_truthy(Some(&json!(0))));
        assert!(!json_truthy(Some(&json!(0.0))));
        assert!(json_truthy(Some(&json!(2))));
        assert!(!json_truthy(Some(&json!(""))));
        assert!(json_truthy(Some(&json!("no"))));
        assert!(!json_truthy(Some(&json!([]))));
        assert!(json_truthy(Some(&json!(["x"]))));
        assert!(!json_truthy(Some(&json!({}))));
        assert!(json_truthy(Some(&json!({"k": 1}))));
    }
}
