//! Append-only review of ambiguous reward captures.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension as _;
use serde_json::{json, Value};

use super::{QuestError, QuestService};

enum ReviewResolution {
    Applied(Vec<String>),
    Refused(String),
}

enum ReviewSource {
    Tracked {
        source_id: i64,
        kill_id: String,
        loot_source_id: Option<String>,
        item_name: String,
        quantity: i64,
        value_ped: f64,
    },
    UntrackedLiquid {
        item_name: String,
        quantity: i64,
        value_ped: f64,
    },
}

type CompletionReviewEvidence = (
    String,
    Option<i64>,
    f64,
    Option<String>,
    Option<String>,
    String,
);

impl QuestService {
    /// List unresolved completion evidence that has no append-only review.
    pub async fn unresolved_reward_reviews(&self) -> Result<Vec<Value>, QuestError> {
        Ok(self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT c.id, c.quest_id, q.name, c.completed_at, \
                            c.reward_policy_snapshot, c.reward_unresolved_reason, \
                            c.reward_evidence_json \
                     FROM session_quest_completions c \
                     JOIN quests q ON q.id = c.quest_id \
                     WHERE c.reward_outcome = 'unresolved' \
                       AND NOT EXISTS (SELECT 1 FROM quest_reward_reviews r \
                                       WHERE r.completion_id = c.id) \
                     ORDER BY c.completed_at DESC, c.id DESC",
                )?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    let evidence: Option<String> = row.get(6)?;
                    let parsed = evidence
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
                    let completion_id = row.get::<_, i64>(0)?;
                    let quest_id = row.get::<_, i64>(1)?;
                    let policy = row.get::<_, Option<String>>(4)?;
                    out.push(json!({
                        "completion_id": completion_id,
                        "quest_id": quest_id,
                        "quest_name": row.get::<_, String>(2)?,
                        "completed_at": row.get::<_, f64>(3)?,
                        "policy": policy,
                        "reason": row.get::<_, Option<String>>(5)?,
                        "loot": parsed
                            .as_ref()
                            .and_then(|parsed| parsed.get("loot").cloned())
                            .unwrap_or_else(|| json!([])),
                        // A tick shape is named only where a tick was
                        // recorded; a completion with no evidence has none.
                        "isolated": parsed
                            .as_ref()
                            .and_then(|parsed| parsed.get("isolated"))
                            .and_then(Value::as_bool),
                        "expected_items": expected_items(conn, quest_id)?,
                        "candidates": loot_candidates(conn, completion_id)?,
                        "declarable": policy.as_deref() == Some("named_items"),
                    }));
                }
                Ok(out)
            })
            .await?)
    }

    /// Append one terminal review of an unresolved completion, refusing
    /// repeated reviews and any selection that cannot be reclassified to one
    /// exact tracked acquisition.
    pub async fn resolve_reward_review(
        &self,
        completion_id: i64,
        selected_indices: &[i64],
        declare_none: bool,
    ) -> Result<(), QuestError> {
        if completion_id <= 0 {
            return Err(QuestError::Invalid(
                "completion id must be positive".to_string(),
            ));
        }
        if declare_none && !selected_indices.is_empty() {
            return Err(QuestError::Invalid(
                "a no-reward review cannot select reward items".to_string(),
            ));
        }
        if !declare_none && selected_indices.is_empty() {
            return Err(QuestError::Invalid(
                "select at least one reward item".to_string(),
            ));
        }
        let mut readable_indices = selected_indices.to_vec();
        readable_indices.sort_unstable();
        if readable_indices.first().is_some_and(|index| *index < 0) {
            return Err(QuestError::Invalid(
                "reward selection is out of range".to_string(),
            ));
        }
        readable_indices.dedup();
        if readable_indices.len() != selected_indices.len() {
            return Err(QuestError::Invalid(
                "reward selection contains a duplicate item".to_string(),
            ));
        }

        let now = self.now_epoch();
        let selected_indices = selected_indices.to_vec();
        let resolution = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let completion: Option<CompletionReviewEvidence> = tx
                    .query_row(
                        "SELECT c.session_id, c.activity_context_id, c.completed_at, \
                                c.reward_policy_snapshot, c.reward_evidence_json, q.name \
                         FROM session_quest_completions c \
                         JOIN quests q ON q.id = c.quest_id \
                         WHERE c.id = ? \
                           AND c.reward_outcome = 'unresolved'",
                        rusqlite::params![completion_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((session_id, context_id, completed_at, policy, evidence, quest_name)) = completion
                else {
                    return Ok(ReviewResolution::Refused(
                        "unresolved completion not found".to_string(),
                    ));
                };
                let already_reviewed: i64 = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM quest_reward_reviews WHERE completion_id = ?)",
                    rusqlite::params![completion_id],
                    |row| row.get(0),
                )?;
                if already_reviewed != 0 {
                    return Ok(ReviewResolution::Refused(
                        "completion has already been reviewed".to_string(),
                    ));
                }

                if declare_none {
                    tx.execute(
                        "INSERT INTO quest_reward_reviews \
                         (completion_id, outcome, policy, note, reviewed_at) \
                         VALUES (?, 'none', 'none', 'Declared no separate reward', ?)",
                        rusqlite::params![completion_id, now],
                    )?;
                    tx.commit()?;
                    return Ok(ReviewResolution::Applied(Vec::new()));
                }
                let Some(context_id) = context_id else {
                    return Ok(ReviewResolution::Refused(
                        "the completion has no activity context for safe reclassification"
                            .to_string(),
                    ));
                };
                let Some(evidence) = evidence
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                else {
                    return Ok(ReviewResolution::Refused(
                        "reward evidence is unreadable".to_string(),
                    ));
                };
                let Some(loot) = evidence.get("loot").and_then(Value::as_array) else {
                    return Ok(ReviewResolution::Refused(
                        "reward evidence has no loot lines".to_string(),
                    ));
                };
                let mut sources = Vec::new();
                let mut tracked_source_ids = BTreeSet::new();
                for index in selected_indices {
                    let Some(candidate) = loot.get(index as usize) else {
                        return Ok(ReviewResolution::Refused(
                            "reward selection is out of range".to_string(),
                        ));
                    };
                    let item_name = candidate
                        .get("item_name")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let quantity = candidate
                        .get("quantity")
                        .and_then(Value::as_i64)
                        .unwrap_or(1)
                        .max(1);
                    let value_ped = candidate
                        .get("value")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        .max(0.0);
                    let matches = {
                        let mut stmt = tx.prepare(
                            "SELECT li.id, li.kill_id, k.loot_source_id \
                             FROM kill_loot_items li \
                             JOIN kills k ON k.id = li.kill_id \
                             WHERE k.session_id = ? AND k.context_id = ? \
                               AND li.deactivated_at IS NULL \
                               AND li.item_name = ? AND li.quantity = ? \
                               AND abs(li.value_ped - ?) < 0.000001 \
                               AND abs(k.timestamp - ?) <= 30",
                        )?;
                        let matches = stmt
                            .query_map(
                            rusqlite::params![
                                session_id,
                                context_id,
                                item_name,
                                quantity,
                                value_ped,
                                completed_at
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, Option<String>>(2)?,
                                ))
                            },
                        )?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        matches
                    };
                    if matches.len() == 1 {
                        if !tracked_source_ids.insert(matches[0].0) {
                            return Ok(ReviewResolution::Refused(format!(
                                "{item_name} cannot be reclassified safely: the same acquisition was selected twice"
                            )));
                        }
                        sources.push(ReviewSource::Tracked {
                            source_id: matches[0].0,
                            kill_id: matches[0].1.clone(),
                            loot_source_id: matches[0].2.clone(),
                            item_name: item_name.to_string(),
                            quantity,
                            value_ped,
                        });
                    } else if matches.is_empty()
                        && item_name.trim().eq_ignore_ascii_case("Universal Ammo")
                    {
                        // Universal Ammo is deliberately filtered from
                        // ordinary loot. The immutable completion evidence is
                        // therefore its acquisition source; unlike stock, it
                        // has no Inventory row to reclassify.
                        sources.push(ReviewSource::UntrackedLiquid {
                            item_name: item_name.to_string(),
                            quantity,
                            value_ped,
                        });
                    } else {
                        return Ok(ReviewResolution::Refused(format!(
                            "{item_name} cannot be reclassified safely: expected one exact acquisition, found {}",
                            matches.len()
                        )));
                    }
                }

                tx.execute(
                    "INSERT INTO quest_reward_reviews \
                     (completion_id, outcome, policy, note, reviewed_at) \
                     VALUES (?, 'confirmed', ?, 'Confirmed from observed completion evidence', ?)",
                    rusqlite::params![
                        completion_id,
                        policy.as_deref().unwrap_or("named_items"),
                        now
                    ],
                )?;
                let review_id = tx.last_insert_rowid();
                let has_item_reward = !sources.is_empty();
                let mut affected_kill_ids = BTreeSet::new();
                let mut affected_loot_sources = BTreeSet::new();
                for source in sources {
                    let (item_name, quantity, value_ped) = match source {
                        ReviewSource::Tracked {
                            source_id,
                            kill_id,
                            loot_source_id,
                            item_name,
                            quantity,
                            value_ped,
                        } => {
                            affected_kill_ids.insert(kill_id);
                            if let Some(source_id) = loot_source_id {
                                affected_loot_sources.insert(source_id);
                            }
                            tx.execute(
                                "INSERT INTO quest_reward_review_items \
                                 (review_id, source_loot_item_id, item_name, quantity, value_ped) \
                                 VALUES (?, ?, ?, ?, ?)",
                                rusqlite::params![
                                    review_id,
                                    source_id,
                                    item_name,
                                    quantity,
                                    value_ped
                                ],
                            )?;
                            tx.execute(
                                "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
                                rusqlite::params![now, source_id],
                            )?;
                            (item_name, quantity, value_ped)
                        }
                        ReviewSource::UntrackedLiquid {
                            item_name,
                            quantity,
                            value_ped,
                        } => (item_name, quantity, value_ped),
                    };
                    super::lifecycle::insert_reward_item(
                        &tx,
                        completion_id,
                        &quest_name,
                        completed_at,
                        &item_name,
                        quantity,
                        value_ped,
                        super::lifecycle::EvidenceBasis::Observed,
                    )?;
                }
                for kill_id in affected_kill_ids {
                    super::lifecycle::recompute_kill_loot_total(&tx, &kill_id)?;
                }
                crate::session_rollup::recompute_session(&tx, &session_id)?;
                crate::daily_rollup::refresh_session_days(&tx, &session_id)?;
                if has_item_reward {
                    crate::daily_rollup::refresh_days(
                        &tx,
                        [crate::daily_rollup::epoch_day(completed_at)],
                    )?;
                }
                tx.commit()?;
                Ok(ReviewResolution::Applied(
                    affected_loot_sources.into_iter().collect(),
                ))
            })
            .await?;
        match resolution {
            ReviewResolution::Applied(source_ids) => {
                for source_id in source_ids {
                    self.report_loot_reconciled(source_id).await;
                }
                Ok(())
            }
            ReviewResolution::Refused(message) => Err(QuestError::Invalid(message)),
        }
    }

    /// Settle an unresolved completion with recorded loot lines: the
    /// quest's named reward arrived as ordinary loot (a pickup the chat log
    /// split from its completion line), so the exact acquisition moves from
    /// ordinary loot to the quest reward. Each line is revalidated: active,
    /// unclaimed, in the completion's session (and declared activity, when
    /// it has one), and one of the quest's named reward items.
    pub async fn settle_reward_with_loot(
        &self,
        completion_id: i64,
        loot_item_ids: &[i64],
    ) -> Result<(), QuestError> {
        if loot_item_ids.is_empty() {
            return Err(QuestError::Invalid(
                "select at least one reward line".to_string(),
            ));
        }
        let mut ids = loot_item_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != loot_item_ids.len() {
            return Err(QuestError::Invalid(
                "reward selection contains a duplicate line".to_string(),
            ));
        }
        let now = self.now_epoch();
        let resolution = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(completion) = open_completion(&tx, completion_id)? else {
                    return Ok(ReviewResolution::Refused(
                        "unresolved completion not found".to_string(),
                    ));
                };
                let candidates = loot_candidates(&tx, completion_id)?;
                let mut chosen = Vec::with_capacity(ids.len());
                for id in &ids {
                    let Some(candidate) = candidates
                        .iter()
                        .find(|candidate| candidate["id"].as_i64() == Some(*id))
                    else {
                        return Ok(ReviewResolution::Refused(
                            "that loot line is no longer available for this reward".to_string(),
                        ));
                    };
                    chosen.push(candidate.clone());
                }
                tx.execute(
                    "INSERT INTO quest_reward_reviews \
                     (completion_id, outcome, policy, note, reviewed_at, basis) \
                     VALUES (?, 'confirmed', 'named_items', \
                             'Confirmed from a recorded loot line', ?, 'observed')",
                    rusqlite::params![completion_id, now],
                )?;
                let review_id = tx.last_insert_rowid();
                let mut kill_ids = BTreeSet::new();
                let mut loot_sources = BTreeSet::new();
                for candidate in chosen {
                    let id = candidate["id"].as_i64().unwrap_or_default();
                    let item_name = candidate["item_name"].as_str().unwrap_or_default();
                    let quantity = candidate["quantity"].as_i64().unwrap_or(1).max(1);
                    let value_ped = candidate["value_ped"].as_f64().unwrap_or(0.0).max(0.0);
                    tx.execute(
                        "INSERT INTO quest_reward_review_items \
                         (review_id, source_loot_item_id, item_name, quantity, value_ped) \
                         VALUES (?, ?, ?, ?, ?)",
                        rusqlite::params![review_id, id, item_name, quantity, value_ped],
                    )?;
                    tx.execute(
                        "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
                        rusqlite::params![now, id],
                    )?;
                    super::lifecycle::insert_reward_item(
                        &tx,
                        completion_id,
                        &completion.quest_name,
                        completion.completed_at,
                        item_name,
                        quantity,
                        value_ped,
                        super::lifecycle::EvidenceBasis::Observed,
                    )?;
                    if let Some(kill_id) = candidate["kill_id"].as_str() {
                        kill_ids.insert(kill_id.to_string());
                    }
                    if let Some(source) = candidate["loot_source_id"].as_str() {
                        loot_sources.insert(source.to_string());
                    }
                }
                for kill_id in kill_ids {
                    super::lifecycle::recompute_kill_loot_total(&tx, &kill_id)?;
                }
                crate::session_rollup::recompute_session(&tx, &completion.session_id)?;
                crate::daily_rollup::refresh_session_days(&tx, &completion.session_id)?;
                crate::daily_rollup::refresh_days(
                    &tx,
                    [crate::daily_rollup::epoch_day(completion.completed_at)],
                )?;
                tx.commit()?;
                Ok(ReviewResolution::Applied(
                    loot_sources.into_iter().collect(),
                ))
            })
            .await?;
        match resolution {
            ReviewResolution::Applied(source_ids) => {
                for source_id in source_ids {
                    self.report_loot_reconciled(source_id).await;
                }
                Ok(())
            }
            ReviewResolution::Refused(message) => Err(QuestError::Invalid(message)),
        }
    }

    /// Settle an unresolved named-item completion by the player's word that
    /// the reward was paid, when nothing recorded can show it. The items are
    /// recorded once at no observed value and marked declared, so nothing
    /// downstream mistakes them for captured evidence. Refused while a
    /// recorded line could settle it instead: declaring then would count the
    /// same item twice.
    pub async fn declare_reward_paid(&self, completion_id: i64) -> Result<(), QuestError> {
        let now = self.now_epoch();
        let resolution = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let Some(completion) = open_completion(&tx, completion_id)? else {
                    return Ok(ReviewResolution::Refused(
                        "unresolved completion not found".to_string(),
                    ));
                };
                if completion.policy.as_deref() != Some("named_items") {
                    return Ok(ReviewResolution::Refused(
                        "only a named reward can be declared paid".to_string(),
                    ));
                }
                if !loot_candidates(&tx, completion_id)?.is_empty() {
                    return Ok(ReviewResolution::Refused(
                        "a recorded loot line can settle this reward; use it instead".to_string(),
                    ));
                }
                let items = expected_items(&tx, completion.quest_id)?;
                if items.is_empty() {
                    return Ok(ReviewResolution::Refused(
                        "the quest names no reward item to declare".to_string(),
                    ));
                }
                tx.execute(
                    "INSERT INTO quest_reward_reviews \
                     (completion_id, outcome, policy, note, reviewed_at, basis) \
                     VALUES (?, 'confirmed', 'named_items', 'Declared paid by the player', ?, \
                             'declared')",
                    rusqlite::params![completion_id, now],
                )?;
                for item in &items {
                    super::lifecycle::insert_reward_item(
                        &tx,
                        completion_id,
                        &completion.quest_name,
                        completion.completed_at,
                        item,
                        1,
                        0.0,
                        super::lifecycle::EvidenceBasis::Declared,
                    )?;
                }
                crate::daily_rollup::refresh_days(
                    &tx,
                    [crate::daily_rollup::epoch_day(completion.completed_at)],
                )?;
                tx.commit()?;
                Ok(ReviewResolution::Applied(Vec::new()))
            })
            .await?;
        match resolution {
            ReviewResolution::Applied(_) => Ok(()),
            ReviewResolution::Refused(message) => Err(QuestError::Invalid(message)),
        }
    }
}

/// An unresolved, unreviewed completion as the settlement paths need it.
struct OpenCompletion {
    quest_id: i64,
    quest_name: String,
    session_id: String,
    completed_at: f64,
    policy: Option<String>,
}

fn open_completion(
    conn: &rusqlite::Connection,
    completion_id: i64,
) -> rusqlite::Result<Option<OpenCompletion>> {
    conn.query_row(
        "SELECT c.quest_id, q.name, c.session_id, c.completed_at, c.reward_policy_snapshot \
         FROM session_quest_completions c \
         JOIN quests q ON q.id = c.quest_id \
         WHERE c.id = ? AND c.reward_outcome = 'unresolved' \
           AND NOT EXISTS (SELECT 1 FROM quest_reward_reviews r WHERE r.completion_id = c.id)",
        rusqlite::params![completion_id],
        |row| {
            Ok(OpenCompletion {
                quest_id: row.get(0)?,
                quest_name: row.get(1)?,
                session_id: row.get(2)?,
                completed_at: row.get(3)?,
                policy: row.get(4)?,
            })
        },
    )
    .optional()
}

/// The quest's named reward items, in authored order.
fn expected_items(conn: &rusqlite::Connection, quest_id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT item_name FROM quest_reward_item_rules \
         WHERE quest_id = ? ORDER BY sort_order, item_name",
    )?;
    let names = stmt.query_map(rusqlite::params![quest_id], |row| row.get(0))?;
    names.collect()
}

/// Ordinary loot lines that could be this completion's named reward: one of
/// the quest's named items, still active and unclaimed, recorded in the
/// completion's session (and its declared activity, when it has one). Found
/// by identity and scope alone, never by comparing the loot's game-clock
/// timestamp with the completion's wall-clock instant; the observed time is
/// carried only so the player can tell lines apart.
fn loot_candidates(
    conn: &rusqlite::Connection,
    completion_id: i64,
) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT li.id, li.kill_id, k.loot_source_id, li.item_name, li.quantity, li.value_ped, \
                k.timestamp, k.mob_name \
         FROM session_quest_completions c \
         JOIN quest_reward_item_rules rule ON rule.quest_id = c.quest_id \
         JOIN kills k ON k.session_id = c.session_id \
          AND (c.activity_context_id IS NULL OR k.context_id = c.activity_context_id) \
         JOIN kill_loot_items li ON li.kill_id = k.id \
          AND lower(trim(li.item_name)) = lower(trim(rule.item_name)) \
         WHERE c.id = ? AND c.reward_policy_snapshot = 'named_items' \
           AND li.deactivated_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM quest_reward_review_items ri \
                           WHERE ri.source_loot_item_id = li.id) \
         ORDER BY k.timestamp DESC, li.id DESC",
    )?;
    let lines = stmt.query_map(rusqlite::params![completion_id], |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "kill_id": row.get::<_, String>(1)?,
            "loot_source_id": row.get::<_, Option<String>>(2)?,
            "item_name": row.get::<_, String>(3)?,
            "quantity": row.get::<_, i64>(4)?,
            "value_ped": row.get::<_, f64>(5)?,
            "observed_at": row.get::<_, f64>(6)?,
            "mob_name": row.get::<_, Option<String>>(7)?,
        }))
    })?;
    lines.collect()
}
