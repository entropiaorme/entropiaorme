//! Quest CRUD: the enriched quest reads, create / update / soft-delete,
//! the quest-mob rows, and the mob-name autocomplete.

use serde_json::{json, Map, Value};

use crate::db::DbError;
use crate::time::to_iso_utc;

use super::families::{completion_anchor_instant, cooldown_lift, curated_refusal};
use super::payload::{json_truthy, value_to_sql};
use super::{QuestError, QuestService};

/// The enriched quest SELECT: every quest column plus the latest
/// completion instant (cooldown and completion counts derive at read
/// time; no counter column exists) and the family picture: the active
/// family's own columns plus the family-wide anchor instants (latest
/// member start / completion, soft-deleted members included: their
/// starts and completions happened in game, so their timers ran).
const QUEST_SELECT: &str = "\
    SELECT q.id, q.name, q.planet, q.waypoint, q.cooldown_hours, \
           q.reward_ped, q.reward_is_skill, q.expected_reward_markup_percent, \
           q.notes, q.chain_name, q.chain_position, q.chain_total, \
           q.started_at, q.is_active, q.created_at, q.category, \
           q.reward_description, q.updated_at, q.signal_loot_item, \
           q.completion_mode AS completion_trigger, \
           CASE q.reward_policy WHEN 'fixed_ped' THEN 'none' ELSE q.reward_policy END \
             AS reward_policy, \
           q.family_id, q.cooldown_anchor, q.last_started_at, q.catalogue_key, \
           f.name AS family_name, \
           f.cooldown_hours AS family_cooldown_hours, \
           f.cooldown_anchor AS family_cooldown_anchor, \
           (SELECT MAX(m.last_started_at) FROM quests m \
            WHERE m.family_id = q.family_id) AS family_last_started_at, \
           (SELECT MAX(c.completed_at) \
            FROM session_quest_completions c \
            JOIN quests m ON m.id = c.quest_id \
            WHERE m.family_id = q.family_id \
              AND NOT EXISTS (SELECT 1 FROM quest_cooldown_resets r \
                              WHERE r.completion_id = c.id)) AS family_last_completed_at, \
           (SELECT MAX(c.completed_at) \
            FROM session_quest_completions c \
            WHERE c.quest_id = q.id \
              AND NOT EXISTS (SELECT 1 FROM quest_cooldown_resets r \
                              WHERE r.completion_id = c.id)) AS last_completed_at, \
           (SELECT MAX(r.abandoned_at) FROM quest_runs r \
            WHERE r.quest_id = q.id) AS last_abandoned_at, \
           (SELECT MAX(r.abandoned_at) FROM quest_runs r \
            JOIN quests m ON m.id = r.quest_id \
            WHERE m.family_id = q.family_id) AS family_last_abandoned_at, \
           EXISTS(SELECT 1 FROM session_quest_completions c \
                  WHERE c.id = (SELECT latest.id FROM session_quest_completions latest \
                                WHERE latest.quest_id = q.id \
                                ORDER BY latest.completed_at DESC, latest.id DESC LIMIT 1) \
                    AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals rr \
                                    WHERE rr.completion_id = c.id) \
                    AND (c.ledger_entry_id IS NOT NULL OR c.quest_claim_id IS NOT NULL \
                         OR EXISTS(SELECT 1 FROM session_quest_completion_reward_items ri \
                                   WHERE ri.completion_id = c.id))) \
             AS reward_undo_available \
    FROM quests q \
    LEFT JOIN quest_families f ON f.id = q.family_id AND f.is_active = 1";

impl QuestService {
    // ── Quest CRUD ──────────────────────────────────────────────────

    /// List all quests, enriched with mobs and reward configuration.
    pub async fn get_quests(&self, active_only: bool) -> Result<Vec<Value>, QuestError> {
        let where_clause = if active_only {
            "WHERE q.is_active = 1"
        } else {
            ""
        };
        let sql = format!("{QUEST_SELECT} {where_clause} ORDER BY q.created_at ASC");
        let bases = self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(row_to_quest(row));
                }
                Ok(out)
            })
            .await?;
        let mut quests = Vec::with_capacity(bases.len());
        for mut quest in bases {
            self.enrich_quest(&mut quest).await?;
            quests.push(Value::Object(quest));
        }
        Ok(quests)
    }

    /// A single quest by ID, enriched; `None` when absent.
    pub async fn get_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let sql = format!("{QUEST_SELECT} WHERE q.id = ?");
        let Some(mut quest) = self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(&sql, rusqlite::params![quest_id], |row| {
                        Ok(row_to_quest(row))
                    })
                    .optional()?)
            })
            .await?
        else {
            return Ok(None);
        };
        self.enrich_quest(&mut quest).await?;
        Ok(Some(Value::Object(quest)))
    }

    async fn enrich_quest(&self, quest: &mut Map<String, Value>) -> Result<(), QuestError> {
        let quest_id = quest["id"].as_i64().expect("integer quest id");
        quest.insert("mobs".into(), json!(self.quest_mobs(quest_id).await?));
        quest.insert(
            "reward_item_names".into(),
            json!(self.quest_reward_item_names(quest_id).await?),
        );
        Ok(())
    }

    /// Create a quest and return it.
    pub async fn create_quest(&self, data: &Value) -> Result<Value, QuestError> {
        let mut signal_loot_item = normalize_signal_loot_item(data.get("signal_loot_item"));
        let completion_trigger = normalize_completion_trigger(
            data.get("completion_trigger"),
            signal_loot_item.as_deref(),
        )?;
        let aris_daily = data
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("ARIS - "));
        let reward_policy_value = data
            .get("reward_policy")
            .cloned()
            .or_else(|| aris_daily.then(|| json!("named_items")));
        if completion_trigger == "manual_hand_in" {
            signal_loot_item = None;
        }
        let reward_policy = if completion_trigger == "manual_hand_in" {
            "completion_clump".to_string()
        } else {
            normalize_reward_policy(
                reward_policy_value.as_ref(),
                data.get("reward_ped"),
                data.get("reward_is_skill"),
            )?
        };
        let mut reward_item_names = normalize_reward_item_names(data.get("reward_item_names"))?;
        if aris_daily && reward_item_names.is_empty() {
            reward_item_names.push("Hyperion Daily Voucher".to_string());
        }
        validate_reward_policy(&reward_policy, &reward_item_names, data.get("reward_ped"))?;
        if completion_trigger == "signal_item" && signal_loot_item.is_none() {
            return Err(QuestError::Invalid(
                "Signal-item completion requires a signal loot item".to_string(),
            ));
        }
        // The cooldown anchor: absent (or null) keeps the pre-family
        // default; a string must parse the vocabulary.
        let cooldown_anchor = match data.get("cooldown_anchor").and_then(Value::as_str) {
            Some(anchor) => super::families::CooldownAnchor::parse(anchor)?.as_str(),
            None => "completion",
        };
        // Family membership: an explicit key binds (null detaches, an id
        // must name an active family); an ABSENT key auto-attaches by
        // the colon-split family part, so a variant created while its
        // family exists lands as a member without being told.
        let family_id: Option<i64> = match data.get("family_id") {
            Some(value) => self.validate_family_ref(value).await?,
            None => match data
                .get("name")
                .and_then(Value::as_str)
                .and_then(super::missions::variant_family_part)
            {
                Some(part) => self
                    .find_family_by_norm(&part)
                    .await?
                    .map(|(family_id, _)| family_id),
                None => None,
            },
        };
        let planet = match data.get("planet") {
            None => json!("Calypso"),
            Some(value) => value.clone(),
        };
        // The bind order matches the INSERT's column list; the owned
        // parameters move into the writer closure.
        let params: Vec<rusqlite::types::Value> = vec![
            value_to_sql(data.get("name").expect("quest payload carries name")),
            value_to_sql(&planet),
            value_to_sql(data.get("waypoint").unwrap_or(&Value::Null)),
            value_to_sql(data.get("cooldown_hours").unwrap_or(&Value::Null)),
            value_to_sql(data.get("reward_ped").unwrap_or(&Value::Null)),
            rusqlite::types::Value::Integer(i64::from(json_truthy(data.get("reward_is_skill")))),
            rusqlite::types::Value::Null,
            value_to_sql(data.get("notes").unwrap_or(&Value::Null)),
            value_to_sql(data.get("chain_name").unwrap_or(&Value::Null)),
            value_to_sql(data.get("chain_position").unwrap_or(&Value::Null)),
            value_to_sql(data.get("chain_total").unwrap_or(&Value::Null)),
            value_to_sql(data.get("category").unwrap_or(&Value::Null)),
            value_to_sql(data.get("reward_description").unwrap_or(&Value::Null)),
            value_to_sql(&json!(signal_loot_item)),
            value_to_sql(&json!(completion_trigger)),
            value_to_sql(&json!(reward_policy)),
            value_to_sql(&json!(family_id)),
            value_to_sql(&json!(cooldown_anchor)),
        ];
        // The original's truthiness gate: an empty (or null) mobs payload
        // writes nothing, and the `expect` fires only for a truthy
        // non-list, exactly as the original's did.
        let mobs: Option<Vec<Value>> = match data.get("mobs") {
            Some(mobs) if json_truthy(Some(mobs)) => {
                Some(mobs.as_array().expect("mobs is a list").to_vec())
            }
            _ => None,
        };

        let quest_id = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO quests (name, planet, waypoint, cooldown_hours, \
                     reward_ped, reward_is_skill, expected_reward_markup_percent, \
                     notes, chain_name, chain_position, chain_total, \
                     category, reward_description, signal_loot_item, \
                     completion_mode, reward_policy, \
                     family_id, cooldown_anchor) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params_from_iter(params),
                )?;
                let quest_id = tx.last_insert_rowid();
                if let Some(mobs) = &mobs {
                    set_quest_mobs(&tx, quest_id, mobs)?;
                }
                set_quest_reward_items(&tx, quest_id, &reward_item_names)?;
                tx.commit()?;
                Ok(quest_id)
            })
            .await?;

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
        if existing.get("curated").and_then(Value::as_bool) == Some(true) {
            return Err(curated_refusal("quest"));
        }

        const ALLOWED: [&str; 14] = [
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
            "signal_loot_item",
            "reward_policy",
        ];
        let mut updates: Vec<(&str, Value)> = Vec::new();
        for key in ALLOWED {
            if let Some(value) = data.get(key) {
                let value = if key == "reward_is_skill" {
                    json!(i64::from(json_truthy(Some(value))))
                } else if key == "signal_loot_item" {
                    json!(normalize_signal_loot_item(Some(value)))
                } else {
                    value.clone()
                };
                updates.push((key, value));
            }
        }

        // Family membership and the cooldown anchor bind only when sent
        // (an update never re-attaches implicitly, so a deliberate
        // detach stays detached); both validate before anything writes.
        if let Some(value) = data.get("family_id") {
            let family_id = self.validate_family_ref(value).await?;
            updates.push(("family_id", json!(family_id)));
        }
        if let Some(value) = data.get("cooldown_anchor") {
            let anchor = value
                .as_str()
                .map(super::families::CooldownAnchor::parse)
                .transpose()?
                .ok_or_else(|| {
                    QuestError::Invalid(
                        "cooldown_anchor must be 'pickup' or 'completion', not null".to_string(),
                    )
                })?;
            updates.push(("cooldown_anchor", json!(anchor.as_str())));
        }

        let mut merged_signal = match data.get("signal_loot_item") {
            Some(value) => normalize_signal_loot_item(Some(value)),
            None => existing
                .get("signal_loot_item")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        let merged_trigger = normalize_completion_trigger(
            data.get("completion_trigger")
                .or_else(|| existing.get("completion_trigger")),
            merged_signal.as_deref(),
        )?;
        if merged_trigger == "manual_hand_in" {
            merged_signal = None;
            match updates
                .iter_mut()
                .find(|(key, _)| *key == "signal_loot_item")
            {
                Some(existing_entry) => *existing_entry = ("signal_loot_item", Value::Null),
                None => updates.push(("signal_loot_item", Value::Null)),
            }
        }
        updates.push(("completion_mode", json!(merged_trigger.clone())));
        let merged_reward = data
            .get("reward_ped")
            .cloned()
            .unwrap_or_else(|| existing.get("reward_ped").cloned().unwrap_or(Value::Null));
        let merged_skill = data.get("reward_is_skill").cloned().unwrap_or_else(|| {
            existing
                .get("reward_is_skill")
                .cloned()
                .unwrap_or(Value::Null)
        });
        let policy_input = if data.get("reward_policy").is_some() {
            data.get("reward_policy")
        } else if data.get("reward_ped").is_some_and(Value::is_null) {
            None
        } else {
            existing.get("reward_policy")
        };
        let merged_policy = if merged_trigger == "manual_hand_in" {
            "completion_clump".to_string()
        } else {
            normalize_reward_policy(policy_input, Some(&merged_reward), Some(&merged_skill))?
        };
        if merged_trigger == "manual_hand_in" {
            match updates.iter_mut().find(|(key, _)| *key == "reward_policy") {
                Some(existing_entry) => {
                    *existing_entry = ("reward_policy", json!("completion_clump"));
                }
                None => updates.push(("reward_policy", json!("completion_clump"))),
            }
        }
        let reward_item_names = match data.get("reward_item_names") {
            Some(value) => Some(normalize_reward_item_names(Some(value))?),
            None => None,
        };
        let effective_items = reward_item_names.as_ref().cloned().unwrap_or_else(|| {
            existing
                .get("reward_item_names")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
        validate_reward_policy(&merged_policy, &effective_items, Some(&merged_reward))?;
        if merged_trigger == "signal_item" && merged_signal.is_none() {
            return Err(QuestError::Invalid(
                "Signal-item completion requires a signal loot item".to_string(),
            ));
        }

        // A present-but-null mobs payload refuses: the original crashes
        // after its mob delete, and the next commit on the shared
        // connection silently ratifies the wipe; the typed refusal plus
        // rollback is the sanctioned repair shape. Validated before the
        // writer closure (the refusal is a `QuestError::Invalid`, not a
        // driver error), so nothing commits either way: an early return
        // and an atomic rollback leave the same nothing-written state.
        let mobs: Option<Vec<Value>> = match data.get("mobs") {
            Some(mobs) => Some(
                mobs.as_array()
                    .ok_or_else(|| {
                        QuestError::Invalid("'mobs' must be a list of mob names".to_string())
                    })?
                    .clone(),
            ),
            None => None,
        };

        let update = (!updates.is_empty()).then(|| {
            let set_clause = updates
                .iter()
                .map(|(key, _)| format!("{key} = ?"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("UPDATE quests SET {set_clause} WHERE id = ?");
            let mut params: Vec<rusqlite::types::Value> = updates
                .iter()
                .map(|(_, value)| value_to_sql(value))
                .collect();
            params.push(rusqlite::types::Value::Integer(quest_id));
            (sql, params)
        });

        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Some((sql, params)) = update {
                    tx.execute(&sql, rusqlite::params_from_iter(params))?;
                }
                if let Some(mobs) = &mobs {
                    set_quest_mobs(&tx, quest_id, mobs)?;
                }
                if let Some(items) = &reward_item_names {
                    set_quest_reward_items(&tx, quest_id, items)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;

        self.get_quest(quest_id).await
    }

    /// Soft-delete a quest.
    pub async fn delete_quest(&self, quest_id: i64) -> Result<bool, QuestError> {
        if let Some(quest) = self.get_quest(quest_id).await? {
            if quest.get("curated").and_then(Value::as_bool) == Some(true) {
                return Err(curated_refusal("quest"));
            }
        }
        let affected = self
            .db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    "UPDATE quests SET is_active = 0 WHERE id = ? AND is_active = 1",
                    rusqlite::params![quest_id],
                )?)
            })
            .await?;
        Ok(affected > 0)
    }

    // ── Mob autocomplete ────────────────────────────────────────────

    /// All distinct mob names across active quests, for autocomplete.
    pub async fn get_all_mob_names(&self) -> Result<Vec<String>, QuestError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT qm.mob_name FROM quest_mobs qm \
                     JOIN quests q ON q.id = qm.quest_id \
                     WHERE q.is_active = 1 \
                     ORDER BY qm.mob_name",
                )?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(row.get::<_, String>(0)?);
                }
                Ok(out)
            })
            .await?)
    }

    // ── Shared helpers ──────────────────────────────────────────────

    async fn quest_mobs(&self, quest_id: i64) -> Result<Vec<String>, QuestError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT mob_name FROM quest_mobs WHERE quest_id = ? ORDER BY mob_name",
                )?;
                let mut rows = stmt.query(rusqlite::params![quest_id])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    let name = row.get::<_, String>(0)?;
                    if !name.is_empty() {
                        out.push(name);
                    }
                }
                Ok(out)
            })
            .await?)
    }

    async fn quest_reward_item_names(&self, quest_id: i64) -> Result<Vec<String>, QuestError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT item_name FROM quest_reward_item_rules \
                     WHERE quest_id = ? ORDER BY sort_order, item_name",
                )?;
                let names = stmt
                    .query_map(rusqlite::params![quest_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(names)
            })
            .await?)
    }
}

/// One quest row to its dict shape, with the derived cooldown expiry
/// (UTC ISO instant) computed from the latest completion.
fn row_to_quest(row: &rusqlite::Row) -> Map<String, Value> {
    let mut quest = Map::new();
    quest.insert("id".into(), json!(row.get_unwrap::<_, i64>("id")));
    quest.insert("name".into(), json!(row.get_unwrap::<_, String>("name")));
    quest.insert(
        "planet".into(),
        json!(row.get_unwrap::<_, String>("planet")),
    );
    quest.insert(
        "waypoint".into(),
        json!(row.get_unwrap::<_, Option<String>>("waypoint")),
    );
    quest.insert(
        "cooldown_hours".into(),
        json!(row.get_unwrap::<_, Option<f64>>("cooldown_hours")),
    );
    quest.insert(
        "reward_ped".into(),
        json!(row.get_unwrap::<_, Option<f64>>("reward_ped")),
    );
    quest.insert(
        "reward_is_skill".into(),
        json!(row.get_unwrap::<_, i64>("reward_is_skill")),
    );
    quest.insert(
        "expected_reward_markup_percent".into(),
        json!(row.get_unwrap::<_, Option<f64>>("expected_reward_markup_percent")),
    );
    quest.insert(
        "notes".into(),
        json!(row.get_unwrap::<_, Option<String>>("notes")),
    );
    quest.insert(
        "chain_name".into(),
        json!(row.get_unwrap::<_, Option<String>>("chain_name")),
    );
    quest.insert(
        "chain_position".into(),
        json!(row.get_unwrap::<_, Option<i64>>("chain_position")),
    );
    quest.insert(
        "chain_total".into(),
        json!(row.get_unwrap::<_, Option<i64>>("chain_total")),
    );
    quest.insert(
        "started_at".into(),
        json!(row.get_unwrap::<_, Option<f64>>("started_at")),
    );
    quest.insert(
        "is_active".into(),
        json!(row.get_unwrap::<_, i64>("is_active")),
    );
    quest.insert(
        "created_at".into(),
        json!(row.get_unwrap::<_, f64>("created_at")),
    );
    quest.insert(
        "category".into(),
        json!(row.get_unwrap::<_, Option<String>>("category")),
    );
    quest.insert(
        "reward_description".into(),
        json!(row.get_unwrap::<_, Option<String>>("reward_description")),
    );
    quest.insert(
        "updated_at".into(),
        json!(row.get_unwrap::<_, Option<f64>>("updated_at")),
    );
    quest.insert(
        "signal_loot_item".into(),
        json!(row.get_unwrap::<_, Option<String>>("signal_loot_item")),
    );
    quest.insert(
        "completion_trigger".into(),
        json!(row.get_unwrap::<_, String>("completion_trigger")),
    );
    quest.insert(
        "reward_policy".into(),
        json!(row.get_unwrap::<_, String>("reward_policy")),
    );
    quest.insert(
        "family_id".into(),
        json!(row.get_unwrap::<_, Option<i64>>("family_id")),
    );
    let catalogue_key = row.get_unwrap::<_, Option<String>>("catalogue_key");
    quest.insert("curated".into(), json!(catalogue_key.is_some()));
    quest.insert("catalogue_key".into(), json!(catalogue_key));
    let anchor = row.get_unwrap::<_, String>("cooldown_anchor");
    quest.insert("cooldown_anchor".into(), json!(anchor));
    let last_started = row.get_unwrap::<_, Option<f64>>("last_started_at");
    quest.insert("last_started_at".into(), json!(last_started));
    quest.insert(
        "family_name".into(),
        json!(row.get_unwrap::<_, Option<String>>("family_name")),
    );
    let family_cooldown_hours = row.get_unwrap::<_, Option<f64>>("family_cooldown_hours");
    quest.insert("family_cooldown_hours".into(), json!(family_cooldown_hours));
    let family_anchor = row.get_unwrap::<_, Option<String>>("family_cooldown_anchor");
    quest.insert("family_cooldown_anchor".into(), json!(family_anchor));
    let last_completed = row.get_unwrap::<_, Option<f64>>("last_completed_at");
    quest.insert("last_completed_at".into(), json!(last_completed));
    let last_abandoned = row.get_unwrap::<_, Option<f64>>("last_abandoned_at");
    quest.insert("last_abandoned_at".into(), json!(last_abandoned));
    quest.insert(
        "reward_undo_available".into(),
        json!(row.get_unwrap::<_, i64>("reward_undo_available") != 0),
    );

    // The quest's OWN cooldown expiry, anchored per its own anchor.
    // Pre-family rows carry the 'completion' default, so their derived
    // expiry is unchanged.
    let cooldown_hours = row.get_unwrap::<_, Option<f64>>("cooldown_hours");
    let own_anchor_instant = match anchor.as_str() {
        "pickup" => last_started,
        _ => completion_anchor_instant(last_completed, last_abandoned),
    };
    let expires = cooldown_lift(own_anchor_instant, cooldown_hours).map(to_iso_utc);
    quest.insert("cooldown_expires_at".into(), json!(expires));

    // The FAMILY's cooldown expiry, from the family's anchor over the
    // family-wide instants. Availability is the LATER of the two
    // expiries; the frontend derives that, keeping both visible.
    let family_anchor_instant = match family_anchor.as_deref() {
        Some("pickup") => row.get_unwrap::<_, Option<f64>>("family_last_started_at"),
        Some(_) => completion_anchor_instant(
            row.get_unwrap::<_, Option<f64>>("family_last_completed_at"),
            row.get_unwrap::<_, Option<f64>>("family_last_abandoned_at"),
        ),
        None => None,
    };
    let family_expires =
        cooldown_lift(family_anchor_instant, family_cooldown_hours).map(to_iso_utc);
    quest.insert("family_cooldown_expires_at".into(), json!(family_expires));
    quest
}

/// The signal item, trimmed; blank and null both mean "no signal" (the
/// quest stays on the mission-log lifecycle).
fn normalize_signal_loot_item(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
}

fn normalize_completion_trigger(
    value: Option<&Value>,
    signal_item: Option<&str>,
) -> Result<String, QuestError> {
    let trigger = value
        .and_then(Value::as_str)
        .unwrap_or(if signal_item.is_some() {
            "signal_item"
        } else {
            "mission_log"
        });
    match trigger {
        "mission_log" | "signal_item" | "manual_hand_in" => Ok(trigger.to_string()),
        _ => Err(QuestError::Invalid(format!(
            "Unknown completion trigger: {trigger}"
        ))),
    }
}

fn normalize_reward_policy(
    value: Option<&Value>,
    reward_ped: Option<&Value>,
    reward_is_skill: Option<&Value>,
) -> Result<String, QuestError> {
    let inferred = if reward_ped
        .and_then(Value::as_f64)
        .is_some_and(|reward| reward > 0.0)
    {
        if json_truthy(reward_is_skill) {
            "fixed_pes"
        } else {
            "none"
        }
    } else {
        "none"
    };
    let policy = value.and_then(Value::as_str).unwrap_or(inferred);
    match policy {
        "fixed_ped" => Ok("none".to_string()),
        "none" | "fixed_pes" | "named_items" | "completion_clump" => Ok(policy.to_string()),
        _ => Err(QuestError::Invalid(format!(
            "Unknown quest reward policy: {policy}"
        ))),
    }
}

fn normalize_reward_item_names(value: Option<&Value>) -> Result<Vec<String>, QuestError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| {
        QuestError::Invalid("reward_item_names must be a list of item names".to_string())
    })?;
    let mut names = Vec::new();
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| QuestError::Invalid("reward item names must be strings".to_string()))?;
        let name = name.trim();
        if !name.is_empty()
            && !names
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

fn validate_reward_policy(
    policy: &str,
    item_names: &[String],
    reward_ped: Option<&Value>,
) -> Result<(), QuestError> {
    let amount = reward_ped.and_then(Value::as_f64).unwrap_or(0.0);
    if policy == "fixed_pes" && amount <= 0.0 {
        return Err(QuestError::Invalid(
            "A fixed quest reward requires a positive amount".to_string(),
        ));
    }
    if policy == "named_items" && item_names.is_empty() {
        return Err(QuestError::Invalid(
            "A named-item reward requires at least one item".to_string(),
        ));
    }
    Ok(())
}

fn set_quest_mobs(
    conn: &rusqlite::Connection,
    quest_id: i64,
    mobs: &[Value],
) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM quest_mobs WHERE quest_id = ?",
        rusqlite::params![quest_id],
    )?;
    for mob in mobs {
        let mob = mob.as_str().expect("mob names are strings").trim();
        if !mob.is_empty() {
            conn.execute(
                "INSERT OR IGNORE INTO quest_mobs (quest_id, mob_name) VALUES (?, ?)",
                rusqlite::params![quest_id, mob],
            )?;
        }
    }
    Ok(())
}

fn set_quest_reward_items(
    conn: &rusqlite::Connection,
    quest_id: i64,
    items: &[String],
) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM quest_reward_item_rules WHERE quest_id = ?",
        rusqlite::params![quest_id],
    )?;
    for (sort_order, item) in items.iter().enumerate() {
        conn.execute(
            "INSERT INTO quest_reward_item_rules(quest_id, item_name, sort_order) \
             VALUES (?, ?, ?)",
            rusqlite::params![quest_id, item, sort_order as i64],
        )?;
    }
    Ok(())
}
