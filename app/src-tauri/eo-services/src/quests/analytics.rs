//! The per-quest analytics readers over sessions whose recorded quest stretches
//! (`session_intervals`, kind `quest`) name them, with the engine's
//! own numeric types preserved on the wire.

use serde_json::{json, Map, Value};

use super::{QuestError, QuestService};

impl QuestService {
    // ── Analytics ───────────────────────────────────────────────────

    /// Per-quest sustainability metrics across all sessions with a
    /// recorded stretch of the quest: raw totals (the frontend derives
    /// averages), only for quests at least one session recorded.
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

    /// The sessions a quest's metrics aggregate over: every session
    /// with a recorded stretch of the quest (an interval, whether
    /// auto-recorded by the lifecycle or hand-placed on history). The
    /// interval superseded the curated `session_quest_analytics_links`
    /// row as the membership truth; the wire keeps the historical
    /// `linked_sessions` field name.
    async fn compute_quest_session_stats(&self, quest_id: i64) -> Result<Value, QuestError> {
        let session_ids = self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT session_id FROM session_intervals \
                     WHERE kind = 'quest' AND ref_id = ? ORDER BY session_id",
                )?;
                let mut rows = stmt.query(rusqlite::params![quest_id])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(row.get::<_, String>(0)?);
                }
                Ok(out)
            })
            .await?;
        self.compute_session_set_stats(&session_ids).await
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
                "consumable_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            }));
        }

        let placeholders = vec!["?"; session_ids.len()].join(",");
        let session_ids: Vec<String> = session_ids.to_vec();
        // Every leg is a plain read, so the whole aggregate runs as one
        // synchronous unit on a reader-core connection.
        self.db
            .with_reader(move |conn| {
                let (linked_sessions, total_duration, heal_cost, armour_cost, consumable_cost) =
                    conn.query_row(
                        &format!(
                            "SELECT COUNT(*), \
                                COALESCE(SUM(s.ended_at - s.started_at), 0), \
                                COALESCE(SUM(s.heal_cost), 0), \
                                COALESCE(SUM(s.armour_cost), 0), \
                                COALESCE(SUM(s.consumable_cost), 0) \
                         FROM tracking_sessions s \
                         WHERE s.id IN ({placeholders}) AND s.is_active = 0"
                        ),
                        rusqlite::params_from_iter(session_ids.iter()),
                        |row| {
                            Ok((
                                row_i64(row, 0),
                                sql_number(row, 1),
                                sql_number(row, 2),
                                sql_number(row, 3),
                                sql_number(row, 4),
                            ))
                        },
                    )?;

                let weapon_cost = conn.query_row(
                    &format!(
                        "SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0) \
                         FROM kill_tool_stats ts \
                         JOIN kills k ON k.id = ts.kill_id \
                         WHERE k.session_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(session_ids.iter()),
                    |row| Ok(sql_number(row, 0)),
                )?;

                let enhancer_cost = conn.query_row(
                    &format!(
                        "SELECT COALESCE(SUM(k.enhancer_cost), 0) \
                         FROM kills k \
                         WHERE k.session_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(session_ids.iter()),
                    |row| Ok(sql_number(row, 0)),
                )?;

                let loot_tt = conn.query_row(
                    &format!(
                        "SELECT COALESCE(SUM(k.loot_total_ped), 0) \
                         FROM kills k \
                         WHERE k.session_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(session_ids.iter()),
                    |row| Ok(sql_number(row, 0)),
                )?;

                let skill_tt = conn.query_row(
                    &format!(
                        "SELECT COALESCE(SUM(sg.ped_value), 0) \
                         FROM skill_gains sg \
                         WHERE sg.session_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(session_ids.iter()),
                    |row| Ok(sql_number(row, 0)),
                )?;

                Ok(json!({
                    "linked_sessions": linked_sessions,
                    "total_duration": total_duration,
                    "weapon_cost": weapon_cost,
                    "heal_cost": heal_cost,
                    "consumable_cost": consumable_cost,
                    "enhancer_cost": enhancer_cost,
                    "armour_cost": armour_cost,
                    "loot_tt": loot_tt,
                    "skill_tt": skill_tt,
                }))
            })
            .await
            .map_err(QuestError::from)
    }
}

/// A COUNT column: always an integer.
fn row_i64(row: &rusqlite::Row, index: usize) -> i64 {
    row.get_unwrap::<_, i64>(index)
}

/// An aggregate column with the engine's own numeric type: SQLite
/// returns INTEGER for empty-set COALESCE fallbacks and integer sums,
/// REAL otherwise, and the original emits whichever arrives. The stored
/// value's affinity (`ValueRef`) drives the branch, mirroring the original
/// typed read.
fn sql_number(row: &rusqlite::Row, index: usize) -> Value {
    match row.get_ref_unwrap(index) {
        rusqlite::types::ValueRef::Real(value) => json!(value),
        value => json!(value.as_i64().expect("sql_number reads a numeric column")),
    }
}
