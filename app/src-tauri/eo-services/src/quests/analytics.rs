//! The per-quest analytics readers: recorded completions and their rewards,
//! for every quest whose stretch (`session_intervals`, kind `quest`) a
//! completed session recorded.

use serde_json::{json, Map, Value};

use super::{QuestError, QuestService};

impl QuestService {
    // ── Analytics ───────────────────────────────────────────────────

    /// Per-quest reward metrics: raw totals over the quest's recorded
    /// completions (the frontend derives averages), only for quests at
    /// least one completed session recorded.
    pub async fn get_quest_analytics(&self) -> Result<Vec<Value>, QuestError> {
        let quest_rows = self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT q.id, q.name, q.planet, q.category \
                     FROM quests q \
                     WHERE q.is_active = 1 \
                     ORDER BY q.name",
                )?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ));
                }
                Ok(out)
            })
            .await?;

        let mut results = Vec::new();
        for (quest_id, quest_name, planet, category) in quest_rows {
            let stats = self.compute_quest_session_stats(quest_id).await?;
            if stats["linked_sessions"] == json!(0) {
                continue;
            }
            let recorded = self.compute_recorded_reward_stats(quest_id).await?;
            let recorded_items = self.compute_recorded_reward_items(quest_id).await?;
            let mut entry = Map::new();
            entry.insert("quest_id".into(), json!(quest_id));
            entry.insert("quest_name".into(), json!(quest_name));
            entry.insert("planet".into(), json!(planet));
            entry.insert("category".into(), json!(category));
            for (key, value) in recorded.as_object().expect("recorded reward stats") {
                entry.insert(key.clone(), value.clone());
            }
            entry.insert("recorded_reward_items".into(), Value::Array(recorded_items));
            for (key, value) in stats.as_object().expect("stats object") {
                entry.insert(key.clone(), value.clone());
            }
            results.push(Value::Object(entry));
        }
        Ok(results)
    }

    async fn compute_recorded_reward_stats(&self, quest_id: i64) -> Result<Value, QuestError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stats = conn.query_row(
                    "WITH effective AS ( \
                         SELECT c.id, c.reward_kind, c.reward_ped, \
                                EXISTS(SELECT 1 FROM quest_reward_reversals rr \
                                       WHERE rr.completion_id = c.id) AS reversed, \
                                COALESCE((SELECT r.outcome FROM quest_reward_reviews r \
                                          WHERE r.completion_id = c.id \
                                          ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), \
                                         c.reward_outcome) AS outcome \
                         FROM session_quest_completions c WHERE c.quest_id = ?1 \
                     ), item_values AS ( \
                         SELECT e.id, COALESCE(SUM(ri.value_ped), 0) AS tt, \
                                COALESCE(SUM(CASE WHEN ri.accounting_kind = 'stock' \
                                                  THEN ri.value_ped ELSE 0 END), 0) AS stock_tt \
                         FROM effective e \
                         LEFT JOIN session_quest_completion_reward_items ri ON ri.completion_id = e.id \
                         GROUP BY e.id \
                     ) \
                     SELECT COUNT(*), \
                            COALESCE(SUM(e.outcome = 'confirmed'), 0), \
                            COALESCE(SUM(e.outcome = 'unresolved'), 0), \
                            COALESCE(SUM(CASE WHEN e.outcome = 'confirmed' AND e.reversed = 0 \
                                              THEN iv.tt ELSE 0 END), 0), \
                            COALESCE(SUM(CASE WHEN e.outcome = 'confirmed' AND e.reversed = 0 \
                                              AND e.reward_kind = 'skill' \
                                              THEN e.reward_ped ELSE 0 END), 0), \
                            COALESCE(SUM(CASE WHEN e.outcome = 'confirmed' AND e.reversed = 0 \
                                              THEN iv.stock_tt ELSE 0 END), 0) \
                     FROM effective e LEFT JOIN item_values iv ON iv.id = e.id",
                    rusqlite::params![quest_id],
                    |row| {
                        Ok(json!({
                            "recorded_completions": row.get::<_, i64>(0)?,
                            "confirmed_completions": row.get::<_, i64>(1)?,
                            "unresolved_completions": row.get::<_, i64>(2)?,
                            "total_recorded_reward_tt": row.get::<_, f64>(3)?,
                            "total_recorded_reward_pes": row.get::<_, f64>(4)?,
                            "total_recorded_item_tt": row.get::<_, f64>(5)?,
                        }))
                    },
                )?;
                let realised_markup: f64 = conn.query_row(
                    "WITH outcomes(id, movement_kind, quantity, net_markup) AS ( \
                         SELECT id, 'listing', quantity, \
                                COALESCE(final_price, 0) - tt_value - listing_fee - COALESCE(sale_fee, 0) \
                         FROM auction_listings WHERE status = 'sold' AND undone_at IS NULL \
                           AND subject_kind = 'loot' \
                         UNION ALL \
                         SELECT id, 'trade', quantity, final_price - tt_value \
                         FROM private_sales WHERE undone_at IS NULL \
                         UNION ALL \
                         SELECT id, 'conversion_out', quantity, COALESCE(output_tt_value, tt_value) - tt_value \
                         FROM stock_conversions WHERE undone_at IS NULL \
                     ) \
                     SELECT COALESCE(SUM(o.net_markup * ABS(m.quantity) / NULLIF(o.quantity, 0)), 0) \
                     FROM outcomes o JOIN stock_movements m \
                       ON m.ref_id = o.id AND m.movement_kind = o.movement_kind \
                     WHERE m.source_kind = 'quest' AND m.quest_id = ?",
                    rusqlite::params![quest_id],
                    |row| row.get(0),
                )?;
                stats["total_realised_reward_markup"] = json!(realised_markup);
                Ok(stats)
            })
            .await?)
    }

    async fn compute_recorded_reward_items(&self, quest_id: i64) -> Result<Vec<Value>, QuestError> {
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT ri.item_name, SUM(ri.quantity), COALESCE(SUM(ri.value_ped), 0) \
                     FROM session_quest_completion_reward_items ri \
                     JOIN session_quest_completions c ON c.id = ri.completion_id \
                     WHERE c.quest_id = ? AND COALESCE(( \
                         SELECT r.outcome FROM quest_reward_reviews r \
                         WHERE r.completion_id = c.id \
                         ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1), c.reward_outcome) = 'confirmed' \
                       AND NOT EXISTS(SELECT 1 FROM quest_reward_reversals rr \
                                      WHERE rr.completion_id = c.id) \
                     GROUP BY ri.item_name ORDER BY ri.item_name",
                )?;
                let mut rows = stmt.query(rusqlite::params![quest_id])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(json!({
                        "item_name": row.get::<_, String>(0)?,
                        "quantity": row.get::<_, i64>(1)?,
                        "value_ped": row.get::<_, f64>(2)?,
                    }));
                }
                Ok(out)
            })
            .await?)
    }

    /// How many completed sessions recorded a stretch of the quest (an
    /// interval, whether auto-recorded by the lifecycle or hand-placed on
    /// history); a quest with none is left out of the analytics. The wire
    /// keeps the historical `linked_sessions` field name.
    ///
    /// Per-quest cost is deliberately not reported: a whole session's cost
    /// cannot be charged to each quest that ran in it without counting it
    /// once per co-active quest. It returns when it can be costed from the
    /// session segments that ran each quest.
    async fn compute_quest_session_stats(&self, quest_id: i64) -> Result<Value, QuestError> {
        self.db
            .with_reader(move |conn| {
                let linked_sessions = conn.query_row(
                    "SELECT COUNT(*) FROM tracking_sessions s \
                     WHERE s.is_active = 0 AND s.id IN ( \
                         SELECT session_id FROM session_intervals \
                         WHERE kind = 'quest' AND ref_id = ?)",
                    rusqlite::params![quest_id],
                    |row| Ok(row_i64(row, 0)),
                )?;
                Ok(json!({ "linked_sessions": linked_sessions }))
            })
            .await
            .map_err(QuestError::from)
    }
}

/// A COUNT column: always an integer.
fn row_i64(row: &rusqlite::Row, index: usize) -> i64 {
    row.get_unwrap::<_, i64>(index)
}
