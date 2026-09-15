//! Manual quest hand-in over exact raw loot-clump evidence.
//!
//! The watcher records a bounded raw journal only for clumps stamped under a
//! standing manual quest interval. Opening the flow offers the latest eligible
//! clump; rejecting it arms the run after that identity so the next clump can
//! be offered. Confirmation feeds the shared completion transaction with an
//! exact source claim, never a timestamp or item-name guess.

use rusqlite::OptionalExtension as _;
use serde_json::json;

use crate::chatlog_watcher::RawLootClump;
use crate::ped::Ped;

use super::lifecycle::{ManualClumpClaim, NotableEventKind, RewardCapture, RewardItemEvidence};
use super::{QuestError, QuestService};

const RAW_CLUMP_RETENTION: i64 = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct HandInRewardItem {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandInCandidate {
    pub id: i64,
    pub observed_at: f64,
    pub items: Vec<HandInRewardItem>,
    pub total_ped: f64,
    pub source_id: String,
    pub source_kind: String,
    pub source_record_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandInState {
    pub quest_id: i64,
    pub quest_name: String,
    pub waiting: bool,
    pub candidate: Option<HandInCandidate>,
}

struct RunState {
    quest_name: String,
    run_started_at: f64,
    waiting: bool,
    after_clump_id: Option<i64>,
}

impl QuestService {
    /// Absorb one raw watcher clump after the ordinary tracker has persisted
    /// its exact source row. Signal-quest completion retains its established
    /// path over the same raw item lines.
    pub async fn raw_loot_clump_check(&self, clump: &RawLootClump) -> Result<(), QuestError> {
        // The synchronous watcher filter already kept safely owned signal
        // rewards out of ordinary loot. Mirror that exact line decision in
        // the manual journal so one raw item cannot become two quests'
        // immutable reward evidence when their stretches overlap.
        let loot_data = clump
            .items
            .iter()
            .map(|item| {
                json!({
                    "item_name": item.item_name,
                    "quantity": item.quantity,
                    "value": item.value_ped.value(),
                })
            })
            .collect::<Vec<_>>();
        let mut manual_clump = clump.clone();
        if let Some(result) = self.signal_reward_filter(&loot_data).await? {
            let mut indices = result
                .get("suppress_loot_indices")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_u64)
                .map(|index| index as usize)
                .filter(|index| *index < manual_clump.items.len())
                .collect::<Vec<_>>();
            indices.sort_unstable();
            indices.dedup();
            for index in indices.into_iter().rev() {
                manual_clump.items.remove(index);
            }
        }
        self.record_manual_reward_clump(&manual_clump).await?;
        self.signal_loot_check(&clump.items).await
    }

    async fn record_manual_reward_clump(&self, clump: &RawLootClump) -> Result<(), QuestError> {
        if clump.items.is_empty() {
            return Ok(());
        }
        let source_id = clump.source_id.clone();
        let items = clump.items.clone();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let source = tx
                    .query_row(
                        "SELECT 'kill', id, session_id, timestamp, context_id \
                         FROM kills WHERE loot_source_id = ? \
                         UNION ALL \
                         SELECT 'harvest', id, session_id, timestamp, context_id \
                         FROM harvest_events WHERE loot_source_id = ?",
                        rusqlite::params![source_id, source_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, Option<i64>>(4)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((source_kind, source_record_id, session_id, observed_at, context_id)) =
                    source
                else {
                    tx.commit()?;
                    return Ok(());
                };
                let Some(context_id) = context_id else {
                    tx.commit()?;
                    return Ok(());
                };
                let relevant = tx.query_row(
                    "SELECT EXISTS( \
                     SELECT 1 FROM session_context_intervals sci \
                     JOIN session_intervals i ON i.id = sci.interval_id \
                     JOIN quests q ON q.id = i.ref_id \
                     JOIN quest_runs r ON r.quest_id = q.id AND r.status = 'in_progress' \
                     WHERE sci.context_id = ? AND i.kind = 'quest' \
                       AND q.completion_mode = 'manual_hand_in' \
                       AND r.started_at <= ?)",
                    rusqlite::params![context_id, observed_at],
                    |row| row.get::<_, i64>(0),
                )? != 0;
                if !relevant {
                    tx.commit()?;
                    return Ok(());
                }

                tx.execute(
                    "INSERT OR IGNORE INTO quest_reward_clumps \
                     (source_id, session_id, source_kind, source_record_id, context_id, observed_at) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        source_id,
                        session_id,
                        source_kind,
                        source_record_id,
                        context_id,
                        observed_at,
                    ],
                )?;
                let clump_id: i64 = tx.query_row(
                    "SELECT id FROM quest_reward_clumps WHERE source_id = ?",
                    rusqlite::params![source_id],
                    |row| row.get(0),
                )?;
                for (line_index, item) in items.iter().enumerate() {
                    let name = item.item_name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    tx.execute(
                        "INSERT OR IGNORE INTO quest_reward_clump_items \
                         (clump_id, line_index, item_name, quantity, value_ped) \
                         VALUES (?, ?, ?, ?, ?)",
                        rusqlite::params![
                            clump_id,
                            line_index as i64,
                            name,
                            item.quantity.max(1),
                            item.value_ped.value().max(0.0),
                        ],
                    )?;
                }

                // Unclaimed evidence is a short operational journal, not a
                // second permanent loot history. Claimed rows stay as durable
                // provenance beside the immutable completion evidence.
                // id-order: retention (the journal keeps its most recently
                // recorded clumps, in arrival order like the hand-in cursor).
                tx.execute(
                    "DELETE FROM quest_reward_clump_items WHERE clump_id IN ( \
                       SELECT id FROM quest_reward_clumps \
                       WHERE claimed_completion_id IS NULL \
                         AND id NOT IN (SELECT id FROM quest_reward_clumps \
                                        WHERE claimed_completion_id IS NULL \
                                        ORDER BY id DESC LIMIT ?))",
                    rusqlite::params![RAW_CLUMP_RETENTION],
                )?;
                // id-order: retention (same window as the items above).
                tx.execute(
                    "DELETE FROM quest_reward_clumps \
                     WHERE claimed_completion_id IS NULL \
                       AND id NOT IN (SELECT id FROM quest_reward_clumps \
                                      WHERE claimed_completion_id IS NULL \
                                      ORDER BY id DESC LIMIT ?)",
                    rusqlite::params![RAW_CLUMP_RETENTION],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Open the one-button hand-in flow. With no retrospective candidate,
    /// opening itself arms the run for the next clump.
    pub async fn hand_in_begin(&self, quest_id: i64) -> Result<HandInState, QuestError> {
        let state = self.hand_in_state(quest_id).await?;
        if state.candidate.is_some() || state.waiting {
            return Ok(state);
        }
        self.arm_hand_in_wait(quest_id, None).await?;
        self.hand_in_state(quest_id).await
    }

    pub async fn hand_in_state(&self, quest_id: i64) -> Result<HandInState, QuestError> {
        self.read_hand_in_state(quest_id, None).await
    }

    /// Reject the displayed candidate and wait for the next exact clump.
    pub async fn hand_in_wait(
        &self,
        quest_id: i64,
        after_clump_id: i64,
    ) -> Result<HandInState, QuestError> {
        self.arm_hand_in_wait(quest_id, Some(after_clump_id))
            .await?;
        self.hand_in_state(quest_id).await
    }

    async fn arm_hand_in_wait(
        &self,
        quest_id: i64,
        after_clump_id: Option<i64>,
    ) -> Result<(), QuestError> {
        let session_id = self.require_hand_in_session()?;
        let resolution = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let run_id = active_manual_run(&tx, quest_id, &session_id)?;
                let Some(run_id) = run_id else {
                    tx.commit()?;
                    return Ok(false);
                };
                // id-order: cursor (the hand-in waits for the next clump
                // recorded after this position, whatever its observed_at).
                let marker = match after_clump_id {
                    Some(id) => id,
                    None => tx.query_row(
                        "SELECT COALESCE(MAX(id), 0) FROM quest_reward_clumps \
                         WHERE session_id = ?",
                        rusqlite::params![session_id],
                        |row| row.get(0),
                    )?,
                };
                tx.execute(
                    "UPDATE quest_runs SET hand_in_waiting = 0 \
                     WHERE hand_in_waiting = 1",
                    [],
                )?;
                tx.execute(
                    "UPDATE quest_runs SET hand_in_waiting = 1, hand_in_after_clump_id = ? \
                     WHERE id = ?",
                    rusqlite::params![(marker > 0).then_some(marker), run_id],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if resolution {
            Ok(())
        } else {
            Err(QuestError::Invalid(
                "This manual quest is not the active session activity".to_string(),
            ))
        }
    }

    /// Leave the quest and its accounting untouched; only disarm the pending
    /// next-clump capture.
    pub async fn hand_in_cancel(&self, quest_id: i64) -> Result<(), QuestError> {
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "UPDATE quest_runs SET hand_in_waiting = 0, hand_in_after_clump_id = NULL \
                     WHERE quest_id = ? AND status = 'in_progress'",
                    rusqlite::params![quest_id],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn hand_in_confirm(&self, quest_id: i64, clump_id: i64) -> Result<(), QuestError> {
        let state = self.read_hand_in_state(quest_id, Some(clump_id)).await?;
        let Some(candidate) = state.candidate else {
            return Err(QuestError::Invalid(
                "That loot clump is no longer available for this quest".to_string(),
            ));
        };
        let had_tracked_loot = self
            .source_has_tracked_loot(&candidate.source_kind, &candidate.source_record_id)
            .await?;
        let evidence = json!({
            "kind": "manual_hand_in_clump",
            "source_id": candidate.source_id,
            "source_kind": candidate.source_kind,
            "source_record_id": candidate.source_record_id,
            "observed_at": candidate.observed_at,
            "items": candidate.items.iter().map(|item| json!({
                "item_name": item.item_name,
                "quantity": item.quantity,
                "value": item.value_ped,
            })).collect::<Vec<_>>(),
        });
        let source_id = candidate.source_id.clone();
        let source_kind = candidate.source_kind.clone();
        let source_record_id = candidate.source_record_id.clone();
        let session_id = candidate.session_id.clone();
        let reward_items = candidate
            .items
            .iter()
            .map(|item| RewardItemEvidence {
                item_name: item.item_name.clone(),
                quantity: item.quantity,
                value_ped: Ped(item.value_ped),
            })
            .collect();
        let completed = self
            .complete_quest_with_reward_capture(
                quest_id,
                RewardCapture {
                    outcome: "confirmed",
                    policy_snapshot: "completion_clump".to_string(),
                    items: reward_items,
                    unresolved_reason: None,
                    evidence_json: Some(evidence.to_string()),
                    had_tracked_loot,
                    manual_clump: Some(ManualClumpClaim {
                        clump_id,
                        source_id,
                        source_kind,
                        source_record_id,
                        session_id,
                    }),
                },
            )
            .await?;
        if completed.is_none() {
            return Err(QuestError::Invalid("Quest not found".to_string()));
        }
        self.record_notable_event(
            NotableEventKind::Completed,
            &state.quest_name,
            Ped(candidate.total_ped),
        )
        .await;
        Ok(())
    }

    async fn source_has_tracked_loot(
        &self,
        source_kind: &str,
        source_record_id: &str,
    ) -> Result<bool, QuestError> {
        let source_kind = source_kind.to_string();
        let source_record_id = source_record_id.to_string();
        Ok(self
            .db
            .with_reader(move |conn| {
                let sql = match source_kind.as_str() {
                    "kill" => {
                        "SELECT EXISTS(SELECT 1 FROM kill_loot_items \
                         WHERE kill_id = ? AND deactivated_at IS NULL)"
                    }
                    "harvest" => {
                        "SELECT EXISTS(SELECT 1 FROM harvest_loot_items \
                         WHERE harvest_id = ? AND deactivated_at IS NULL)"
                    }
                    _ => return Ok(false),
                };
                Ok(
                    conn.query_row(sql, rusqlite::params![source_record_id], |row| {
                        row.get::<_, i64>(0)
                    })? != 0,
                )
            })
            .await?)
    }

    fn require_hand_in_session(&self) -> Result<String, QuestError> {
        self.current_session()
            .filter(|session_id| !session_id.is_empty())
            .ok_or_else(|| QuestError::Invalid("No active session".to_string()))
    }

    async fn read_hand_in_state(
        &self,
        quest_id: i64,
        exact_clump_id: Option<i64>,
    ) -> Result<HandInState, QuestError> {
        let session_id = self.require_hand_in_session()?;
        let state = self
            .db
            .with_reader(move |conn| {
                let Some(run) = read_run_state(conn, quest_id, &session_id)? else {
                    return Ok(None);
                };
                let candidate = read_candidate(conn, quest_id, &session_id, &run, exact_clump_id)?;
                Ok(Some(HandInState {
                    quest_id,
                    quest_name: run.quest_name,
                    waiting: run.waiting && candidate.is_none(),
                    candidate,
                }))
            })
            .await?;
        state.ok_or_else(|| {
            QuestError::Invalid("This manual quest is not the active session activity".to_string())
        })
    }
}

fn active_manual_run(
    conn: &rusqlite::Connection,
    quest_id: i64,
    session_id: &str,
) -> Result<Option<i64>, crate::db::DbError> {
    Ok(conn
        .query_row(
            "SELECT r.id FROM quests q \
             JOIN quest_runs r ON r.quest_id = q.id AND r.status = 'in_progress' \
             WHERE q.id = ? AND q.is_active = 1 \
               AND q.completion_mode = 'manual_hand_in' \
               AND EXISTS(SELECT 1 FROM session_intervals i \
                          WHERE i.session_id = ? AND i.kind = 'quest' \
                            AND i.ref_id = q.id AND i.ended_at IS NULL)",
            rusqlite::params![quest_id, session_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn read_run_state(
    conn: &rusqlite::Connection,
    quest_id: i64,
    session_id: &str,
) -> Result<Option<RunState>, crate::db::DbError> {
    Ok(conn
        .query_row(
            "SELECT q.name, r.started_at, r.hand_in_waiting, \
                    r.hand_in_after_clump_id \
             FROM quests q \
             JOIN quest_runs r ON r.quest_id = q.id AND r.status = 'in_progress' \
             WHERE q.id = ? AND q.is_active = 1 \
               AND q.completion_mode = 'manual_hand_in' \
               AND EXISTS(SELECT 1 FROM session_intervals i \
                          WHERE i.session_id = ? AND i.kind = 'quest' \
                            AND i.ref_id = q.id AND i.ended_at IS NULL)",
            rusqlite::params![quest_id, session_id],
            |row| {
                Ok(RunState {
                    quest_name: row.get(0)?,
                    run_started_at: row.get(1)?,
                    waiting: row.get::<_, i64>(2)? != 0,
                    after_clump_id: row.get(3)?,
                })
            },
        )
        .optional()?)
}

fn read_candidate(
    conn: &rusqlite::Connection,
    quest_id: i64,
    session_id: &str,
    run: &RunState,
    exact_clump_id: Option<i64>,
) -> Result<Option<HandInCandidate>, crate::db::DbError> {
    // id-order: cursor (a waiting run takes the first clump recorded
    // past its marker; otherwise the exact clump the caller named).
    let (predicate, order) = if exact_clump_id.is_some() {
        ("AND c.id = ?", "ORDER BY c.id DESC")
    } else if run.waiting {
        ("AND c.id > ?", "ORDER BY c.id ASC")
    } else {
        ("AND (? = 0 OR c.id = ?)", "ORDER BY c.id DESC")
    };
    let sql = format!(
        "SELECT c.id, c.observed_at, c.source_id, c.source_kind, \
                c.source_record_id, c.session_id \
         FROM quest_reward_clumps c \
         JOIN session_context_intervals sci ON sci.context_id = c.context_id \
         JOIN session_intervals i ON i.id = sci.interval_id \
         WHERE c.session_id = ? AND c.claimed_completion_id IS NULL \
           AND c.observed_at >= ? AND i.kind = 'quest' AND i.ref_id = ? \
           {predicate} {order} LIMIT 1"
    );
    let marker = exact_clump_id.or(run.after_clump_id).unwrap_or_default();
    let record = if exact_clump_id.is_some() || run.waiting {
        conn.query_row(
            &sql,
            rusqlite::params![session_id, run.run_started_at, quest_id, marker],
            candidate_record,
        )
        .optional()?
    } else {
        conn.query_row(
            &sql,
            rusqlite::params![session_id, run.run_started_at, quest_id, marker, marker,],
            candidate_record,
        )
        .optional()?
    };
    let Some((id, observed_at, source_id, source_kind, source_record_id, source_session)) = record
    else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT item_name, quantity, value_ped \
         FROM quest_reward_clump_items WHERE clump_id = ? ORDER BY line_index",
    )?;
    let items = stmt
        .query_map(rusqlite::params![id], |row| {
            Ok(HandInRewardItem {
                item_name: row.get(0)?,
                quantity: row.get(1)?,
                value_ped: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if items.is_empty() {
        return Ok(None);
    }
    let total_ped = items.iter().map(|item| item.value_ped).sum();
    Ok(Some(HandInCandidate {
        id,
        observed_at,
        items,
        total_ped,
        source_id,
        source_kind,
        source_record_id,
        session_id: source_session,
    }))
}

type CandidateRecord = (i64, f64, String, String, String, String);

fn candidate_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<CandidateRecord> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}
