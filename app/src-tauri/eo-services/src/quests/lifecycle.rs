//! The quest lifecycle: start / complete / cancel, the completion and
//! overlay-event records, the cooldown predicate, and the cancel flow's
//! reward-undo helpers.

use serde_json::Value;

use crate::ped::Ped;
use crate::time::to_iso_utc;

use super::{QuestError, QuestService};

/// One actual item observed as a quest reward. This is completion evidence,
/// not a market valuation: analytics resolves its current markup separately.
#[derive(Debug, Clone)]
pub(super) struct RewardItemEvidence {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: Ped,
}

#[derive(Debug, Clone)]
pub(super) struct RewardCapture {
    pub outcome: &'static str,
    pub policy_snapshot: String,
    pub items: Vec<RewardItemEvidence>,
    pub unresolved_reason: Option<String>,
    pub evidence_json: Option<String>,
    pub had_tracked_loot: bool,
    pub manual_clump: Option<ManualClumpClaim>,
}

/// Exact source ownership carried from the hand-in candidate read into the
/// completion transaction. Every field is revalidated under the writer lock.
#[derive(Debug, Clone)]
pub(super) struct ManualClumpClaim {
    pub clump_id: i64,
    pub source_id: String,
    pub source_kind: String,
    pub source_record_id: String,
    pub session_id: String,
}

enum CompletionWrite {
    Applied,
    Skipped,
    Refused(String),
}

pub(super) fn insert_reward_item(
    conn: &rusqlite::Connection,
    completion_id: i64,
    quest_name: &str,
    completed_at: f64,
    item_name: &str,
    quantity: i64,
    value_ped: f64,
) -> rusqlite::Result<()> {
    let liquid = item_name.trim().eq_ignore_ascii_case("Universal Ammo");
    conn.execute(
        "INSERT INTO session_quest_completion_reward_items \
         (completion_id, item_name, quantity, value_ped, accounting_kind) \
         VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![
            completion_id,
            item_name,
            quantity.max(1),
            value_ped.max(0.0),
            if liquid { "liquid" } else { "stock" },
        ],
    )?;
    let reward_item_id = conn.last_insert_rowid();
    if !liquid || value_ped <= 0.0 {
        return Ok(());
    }
    let ledger_id = format!("quest-ammo-{reward_item_id}");
    let date = to_iso_utc(completed_at);
    conn.execute(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
         VALUES (?, ?, 'markup', ?, ?, 'quest_reward')",
        rusqlite::params![ledger_id, date, format!("Quest: {quest_name}"), value_ped],
    )?;
    conn.execute(
        "UPDATE session_quest_completion_reward_items SET ledger_entry_id = ? WHERE id = ?",
        rusqlite::params![ledger_id, reward_item_id],
    )?;
    Ok(())
}

/// Restore the canonical ordinary-loot total after reward evidence changes
/// the active item rows. Enhancer-rebate Shrapnel never contributes to a
/// kill's loot total, matching the tracker write path.
pub(super) fn recompute_kill_loot_total(
    conn: &rusqlite::Connection,
    kill_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE kills SET loot_total_ped = \
         (SELECT COALESCE(SUM(value_ped), 0) FROM kill_loot_items \
          WHERE kill_id = ? AND deactivated_at IS NULL \
            AND is_enhancer_shrapnel = 0) \
         WHERE id = ?",
        rusqlite::params![kill_id, kill_id],
    )?;
    Ok(())
}

fn snapshot_reward_attributions(
    conn: &rusqlite::Connection,
    completion_id: i64,
    run_id: i64,
    completed_at: f64,
) -> rusqlite::Result<()> {
    #[derive(Debug)]
    struct ContextWeight {
        context_id: i64,
        definition_id: Option<i64>,
        cycled: f64,
        duration: f64,
    }

    let rows: Vec<ContextWeight> = {
        let mut stmt = conn.prepare(
            "SELECT c.id, s.definition_id, \
                    COALESCE((SELECT SUM(k.cost_ped + k.enhancer_cost) \
                              FROM kills k WHERE k.context_id = c.id), 0) + \
                    COALESCE((SELECT SUM(h.cost_ped) \
                              FROM harvest_events h WHERE h.context_id = c.id), 0), \
                    MAX(0, COALESCE((SELECT MIN(next.created_at) \
                                     FROM session_contexts next \
                                     WHERE next.session_id = c.session_id \
                                       AND next.created_at > c.created_at), \
                                    s.ended_at, ?) - c.created_at) \
             FROM session_contexts c \
             JOIN tracking_sessions s ON s.id = c.session_id \
             WHERE EXISTS ( \
                 SELECT 1 FROM session_context_intervals sci \
                 JOIN quest_run_intervals qri ON qri.interval_id = sci.interval_id \
                 WHERE sci.context_id = c.id AND qri.run_id = ? \
             ) \
             ORDER BY c.created_at, c.id",
        )?;
        let mapped = stmt.query_map(rusqlite::params![completed_at, run_id], |row| {
            Ok(ContextWeight {
                context_id: row.get(0)?,
                definition_id: row.get(1)?,
                cycled: row.get::<_, f64>(2)?.max(0.0),
                duration: row.get::<_, f64>(3)?.max(0.0),
            })
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let total_cycled: f64 = rows.iter().map(|row| row.cycled).sum();
    let total_duration: f64 = rows.iter().map(|row| row.duration).sum();
    let (basis, total) = if total_cycled > 0.0 {
        ("cycled", total_cycled)
    } else if total_duration > 0.0 {
        ("duration", total_duration)
    } else {
        return Ok(());
    };
    for row in rows {
        let numerator = if basis == "cycled" {
            row.cycled
        } else {
            row.duration
        };
        if numerator <= 0.0 {
            continue;
        }
        conn.execute(
            "INSERT INTO quest_reward_attributions \
             (completion_id, activity_context_id, session_definition_id, weight, basis, \
              cycled_ped, duration_seconds) VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                completion_id,
                row.context_id,
                row.definition_id,
                numerator / total,
                basis,
                row.cycled,
                row.duration,
            ],
        )?;
    }
    Ok(())
}

/// The overlay-event vocabulary the quest flows record: a started quest,
/// a completed liquid-TT outcome, and a completed skill (PES) outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotableEventKind {
    Started,
    Completed,
    CompletedPes,
}

impl NotableEventKind {
    fn as_str(self) -> &'static str {
        match self {
            NotableEventKind::Started => "quest_started",
            NotableEventKind::Completed => "quest_completed",
            NotableEventKind::CompletedPes => "quest_completed_pes",
        }
    }
}

impl QuestService {
    // ── Quest actions ───────────────────────────────────────────────

    /// Mark a quest as in-progress; `None` when absent or inactive.
    /// The start also stamps `last_started_at`, the DURABLE start fact:
    /// `started_at` is lifecycle state (completion and cancel clear
    /// it), while a pickup-anchored cooldown needs the instant the
    /// mission was collected to survive both.
    pub async fn start_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let now = self.now_epoch();
        let affected = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let affected = tx.execute(
                    "UPDATE quests SET started_at = ?, last_started_at = ? \
                     WHERE id = ? AND is_active = 1 AND started_at IS NULL",
                    rusqlite::params![now, now, quest_id],
                )?;
                if affected > 0 {
                    tx.execute(
                        "INSERT INTO quest_runs(quest_id, status, started_at) \
                         VALUES (?, 'in_progress', ?)",
                        rusqlite::params![quest_id, now],
                    )?;
                }
                tx.commit()?;
                Ok(affected)
            })
            .await?;
        if affected > 0 {
            // Deliberately no interval-layer report: starting a quest is
            // an administrative fact (it is in the mission log), not a
            // declaration that the gameplay from here advances it. Bulk
            // pickup makes the two diverge; which stretches of play are
            // toward a quest is the user's own declaration.
            self.get_quest(quest_id).await
        } else {
            Ok(None)
        }
    }

    /// Complete a quest from an administrative/manual action. Tick-driven
    /// completion supplies a typed reward capture instead.
    pub async fn complete_quest(&self, quest_id: i64) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };
        if quest
            .get("completion_trigger")
            .and_then(Value::as_str)
            .is_some_and(|trigger| trigger == "manual_hand_in")
        {
            return Err(QuestError::Invalid(
                "Manual hand-in quests must be completed by confirming an exact reward clump"
                    .to_string(),
            ));
        }
        self.complete_quest_with_evidence(quest_id, None).await
    }

    pub(super) async fn complete_quest_with_reward_capture(
        &self,
        quest_id: i64,
        capture: RewardCapture,
    ) -> Result<Option<Value>, QuestError> {
        self.complete_quest_with_evidence(quest_id, Some(capture))
            .await
    }

    /// Complete a quest and preserve the immutable economic evidence in one
    /// database transaction. The activity context is snapshotted before the
    /// declared stretch closes, matching the context already stamped on the
    /// completion tick's costs and loot.
    async fn complete_quest_with_evidence(
        &self,
        quest_id: i64,
        capture: Option<RewardCapture>,
    ) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };
        let now = self.now_epoch();
        let session_id = self.current_session();
        if let Some(existing_session_id) = session_id.as_deref().filter(|id| !id.is_empty()) {
            let existing_session_id = existing_session_id.to_string();
            let already_completed = self
                .db
                .with_reader(move |conn| {
                    Ok(conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM session_quest_completions \
                         WHERE session_id = ? AND quest_id = ?)",
                        rusqlite::params![existing_session_id, quest_id],
                        |row| row.get::<_, i64>(0),
                    )? != 0)
                })
                .await?;
            if already_completed {
                return Ok(Some(quest));
            }
        }
        let attribution = self
            .completion_attribution(session_id.as_deref(), quest_id)
            .await?;
        let manual_hand_in = capture
            .as_ref()
            .is_some_and(|capture| capture.manual_clump.is_some());
        // A declared stretch of this quest (when the user declared one)
        // closes at the completion moment, before the reward is
        // recorded, so it bounds the quest's own play and not the
        // bookkeeping that follows it. Only this quest's stretch
        // closes; a sibling daily's stretch keeps running. With no
        // stretch declared this is a no-op.
        if !manual_hand_in {
            self.report_stretch_closed(quest_id).await;
        }

        let policy = quest
            .get("reward_policy")
            .and_then(Value::as_str)
            .unwrap_or("none");
        let capture = capture.unwrap_or_else(|| match policy {
            "none" => RewardCapture {
                outcome: "none",
                policy_snapshot: policy.to_string(),
                items: Vec::new(),
                unresolved_reason: None,
                evidence_json: None,
                had_tracked_loot: false,
                manual_clump: None,
            },
            "fixed_pes" => RewardCapture {
                outcome: "confirmed",
                policy_snapshot: policy.to_string(),
                items: Vec::new(),
                unresolved_reason: None,
                evidence_json: None,
                had_tracked_loot: false,
                manual_clump: None,
            },
            _ => RewardCapture {
                outcome: "unresolved",
                policy_snapshot: policy.to_string(),
                items: Vec::new(),
                unresolved_reason: Some(
                    "Completion was recorded without a reward-bearing chat tick".to_string(),
                ),
                evidence_json: None,
                had_tracked_loot: false,
                manual_clump: None,
            },
        });
        let reward_is_skill = policy == "fixed_pes";
        let reward_ped = reward_is_skill
            .then(|| quest.get("reward_ped").and_then(Value::as_f64).map(Ped))
            .flatten();
        let reward_source = match reward_ped.filter(|reward| reward.is_positive()) {
            Some(_) if reward_is_skill => "skill",
            None if capture.had_tracked_loot => "tracked_loot",
            _ => "none",
        };
        let reward_kind = match reward_source {
            "skill" => "skill",
            _ if !capture.items.is_empty() => "item",
            "tracked_loot" => "included_in_loot",
            _ => "none",
        };
        let name = quest["name"].as_str().expect("quest name").to_string();
        let completion_key = session_id
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| format!("manual-{}", self.next_id()));
        let (activity_context_id, activity_interval_id) = attribution.unzip();
        let reward_value = reward_ped.map(Ped::value);
        let reward_outcome = capture.outcome;
        let policy_snapshot = capture.policy_snapshot;
        let unresolved_reason = capture.unresolved_reason;
        let evidence_json = capture.evidence_json;
        let reward_items = capture.items;
        let manual_clump = capture.manual_clump;
        let reclassified_source_id = manual_clump.as_ref().map(|claim| claim.source_id.clone());
        let resolution = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let run: Option<(i64, f64)> = {
                    use rusqlite::OptionalExtension as _;
                    tx.query_row(
                        "SELECT id, started_at FROM quest_runs \
                         WHERE quest_id = ? AND status = 'in_progress'",
                        rusqlite::params![quest_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?
                };
                let (run_id, run_started_at) = match run {
                    Some(run) => run,
                    None => {
                        tx.execute(
                            "INSERT INTO quest_runs(quest_id, status, started_at) \
                             VALUES (?, 'in_progress', ?)",
                            rusqlite::params![quest_id, now],
                        )?;
                        (tx.last_insert_rowid(), now)
                    }
                };
                if let Some(claim) = &manual_clump {
                    let valid = tx.query_row(
                        "SELECT EXISTS( \
                         SELECT 1 FROM quest_reward_clumps c \
                         JOIN session_context_intervals sci ON sci.context_id = c.context_id \
                         JOIN session_intervals i ON i.id = sci.interval_id \
                         WHERE c.id = ? AND c.source_id = ? AND c.source_kind = ? \
                           AND c.source_record_id = ? AND c.session_id = ? \
                           AND c.claimed_completion_id IS NULL \
                           AND i.kind = 'quest' AND i.ref_id = ? \
                           AND c.observed_at >= ? \
                           AND ( \
                             (c.source_kind = 'kill' AND EXISTS( \
                               SELECT 1 FROM kills k \
                               WHERE k.id = c.source_record_id \
                                 AND k.loot_source_id = c.source_id \
                                 AND k.session_id = c.session_id \
                                 AND k.context_id = c.context_id)) \
                             OR \
                             (c.source_kind = 'harvest' AND EXISTS( \
                               SELECT 1 FROM harvest_events h \
                               WHERE h.id = c.source_record_id \
                                 AND h.loot_source_id = c.source_id \
                                 AND h.session_id = c.session_id \
                                 AND h.context_id = c.context_id)) \
                           ))",
                        rusqlite::params![
                            claim.clump_id,
                            claim.source_id,
                            claim.source_kind,
                            claim.source_record_id,
                            claim.session_id,
                            quest_id,
                            run_started_at,
                        ],
                        |row| row.get::<_, i64>(0),
                    )? != 0;
                    if !valid {
                        return Ok(CompletionWrite::Refused(
                            "That loot clump is no longer available for this quest".to_string(),
                        ));
                    }
                }
                tx.execute(
                    "UPDATE quests SET started_at = NULL WHERE id = ?",
                    rusqlite::params![quest_id],
                )?;
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO session_quest_completions \
                     (session_id, quest_id, completed_at, activity_context_id, \
                      activity_interval_id, reward_source, reward_kind, reward_ped, \
                      expected_reward_markup_percent, reward_outcome, \
                      reward_policy_snapshot, reward_unresolved_reason, reward_evidence_json, \
                      quest_run_id) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        completion_key,
                        quest_id,
                        now,
                        activity_context_id,
                        activity_interval_id,
                        reward_source,
                        reward_kind,
                        reward_value,
                        Option::<f64>::None,
                        reward_outcome,
                        policy_snapshot,
                        unresolved_reason,
                        evidence_json,
                        run_id,
                    ],
                )?;
                if inserted == 0 {
                    tx.commit()?;
                    return Ok(CompletionWrite::Skipped);
                }
                let completion_id = tx.last_insert_rowid();
                tx.execute(
                    "UPDATE quest_runs SET status = 'completed', completed_at = ?, \
                     completion_id = ? WHERE id = ?",
                    rusqlite::params![now, completion_id, run_id],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO quest_run_intervals(run_id, interval_id) \
                     SELECT ?, id FROM session_intervals \
                     WHERE kind = 'quest' AND ref_id = ? \
                       AND started_at >= ? AND started_at <= ?",
                    rusqlite::params![run_id, quest_id, run_started_at, now],
                )?;
                snapshot_reward_attributions(&tx, completion_id, run_id, now)?;
                let has_item_reward = !reward_items.is_empty();
                for item in reward_items {
                    insert_reward_item(
                        &tx,
                        completion_id,
                        &name,
                        now,
                        &item.item_name,
                        item.quantity,
                        item.value_ped.value(),
                    )?;
                }
                if has_item_reward {
                    crate::daily_rollup::refresh_days(
                        &tx,
                        [crate::daily_rollup::epoch_day(now)],
                    )?;
                }
                if let Some(claim) = &manual_clump {
                    match claim.source_kind.as_str() {
                        "kill" => {
                            tx.execute(
                                "UPDATE kill_loot_items SET deactivated_at = ? \
                                 WHERE kill_id = ? AND deactivated_at IS NULL",
                                rusqlite::params![now, claim.source_record_id],
                            )?;
                            tx.execute(
                                "UPDATE kills SET loot_total_ped = 0 \
                                 WHERE id = ? AND loot_source_id = ?",
                                rusqlite::params![claim.source_record_id, claim.source_id],
                            )?;
                        }
                        "harvest" => {
                            tx.execute(
                                "UPDATE harvest_loot_items SET deactivated_at = ? \
                                 WHERE harvest_id = ? AND deactivated_at IS NULL",
                                rusqlite::params![now, claim.source_record_id],
                            )?;
                            tx.execute(
                                "UPDATE harvest_events SET loot_total_ped = 0 \
                                 WHERE id = ? AND loot_source_id = ?",
                                rusqlite::params![claim.source_record_id, claim.source_id],
                            )?;
                        }
                        _ => {
                            return Ok(CompletionWrite::Refused(
                                "The loot clump has an unsupported source".to_string(),
                            ));
                        }
                    }
                    let claimed = tx.execute(
                        "UPDATE quest_reward_clumps SET claimed_completion_id = ? \
                         WHERE id = ? AND claimed_completion_id IS NULL",
                        rusqlite::params![completion_id, claim.clump_id],
                    )?;
                    if claimed != 1 {
                        return Ok(CompletionWrite::Refused(
                            "That loot clump has already been used".to_string(),
                        ));
                    }
                    crate::session_rollup::recompute_session(&tx, &claim.session_id)?;
                    crate::daily_rollup::refresh_session_days(&tx, &claim.session_id)?;
                }
                if let Some(reward) = reward_ped.filter(|reward| reward.is_positive()) {
                    if reward_is_skill {
                        tx.execute(
                            "INSERT INTO quest_claims (quest_id, quest_name, ped_value, claimed_at) \
                             VALUES (?, ?, ?, ?)",
                            rusqlite::params![quest_id, name, reward.value(), now],
                        )?;
                        let claim_id = tx.last_insert_rowid();
                        tx.execute(
                            "UPDATE session_quest_completions SET quest_claim_id = ? \
                             WHERE id = ?",
                            rusqlite::params![claim_id, completion_id],
                        )?;
                        crate::daily_rollup::refresh_days(
                            &tx,
                            [crate::daily_rollup::epoch_day(now)],
                        )?;
                    }
                }
                tx.commit()?;
                Ok(CompletionWrite::Applied)
            })
            .await?;
        match resolution {
            CompletionWrite::Refused(message) => return Err(QuestError::Invalid(message)),
            CompletionWrite::Applied => {
                if let Some(source_id) = reclassified_source_id {
                    self.report_loot_reconciled(source_id).await;
                }
                if manual_hand_in {
                    self.report_stretch_closed(quest_id).await;
                }
            }
            CompletionWrite::Skipped => {}
        }
        self.get_quest(quest_id).await
    }

    /// The exact declared quest stretch and activity signature currently in
    /// force. Absence means the completion is administrative evidence only;
    /// it must never claim the whole session's economics after the fact.
    async fn completion_attribution(
        &self,
        session_id: Option<&str>,
        quest_id: i64,
    ) -> Result<Option<(i64, i64)>, QuestError> {
        use rusqlite::OptionalExtension as _;
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        let session_id = session_id.to_string();
        Ok(self
            .db
            .with_reader(move |conn| {
                // id-order: insertion (contexts append in process order, so
                // the newest is the one in force; see skill_tracker).
                Ok(conn
                    .query_row(
                        "SELECT c.id, i.id \
                         FROM session_contexts c \
                         JOIN session_context_intervals sci ON sci.context_id = c.id \
                         JOIN session_intervals i ON i.id = sci.interval_id \
                         WHERE c.session_id = ? AND i.kind = 'quest' \
                           AND i.ref_id = ? AND i.ended_at IS NULL \
                         ORDER BY c.id DESC LIMIT 1",
                        rusqlite::params![session_id, quest_id],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?)
            })
            .await?)
    }

    /// Undo an in-progress quest, or disavow the active cooldown while
    /// preserving the completion and run. Economic undo is a separate,
    /// append-only reversal and refuses while reward stock has dependants.
    pub async fn cancel_quest(
        &self,
        quest_id: i64,
        undo_reward: bool,
    ) -> Result<Option<Value>, QuestError> {
        let Some(quest) = self.get_quest(quest_id).await? else {
            return Ok(None);
        };

        if !quest["started_at"].is_null() {
            if undo_reward {
                return Err(QuestError::Invalid(
                    "Cancel the active quest run before undoing its previous reward".to_string(),
                ));
            }
            self.db
                .with_writer(move |conn| {
                    let tx = conn.transaction()?;
                    tx.execute(
                        "UPDATE quests SET started_at = NULL WHERE id = ? AND is_active = 1",
                        rusqlite::params![quest_id],
                    )?;
                    tx.execute(
                        "UPDATE quest_runs SET status = 'cancelled' \
                         WHERE quest_id = ? AND status = 'in_progress'",
                        rusqlite::params![quest_id],
                    )?;
                    tx.commit()?;
                    Ok(())
                })
                .await?;
            return self.get_quest(quest_id).await;
        }

        let cooling = self.is_quest_cooling(&quest);
        if !cooling && !undo_reward {
            return Ok(Some(quest));
        }

        // A pickup-anchored quest additionally clears its durable start
        // stamp: for that anchor, "reset the cooldown" IS forgetting
        // the last collection. The first cancel of a started quest
        // deliberately keeps the stamp (an abandoned mission's timer
        // keeps running in game); cancelling AGAIN while cooling is the
        // explicit "that start should not gate me" correction, exactly
        // parallel to the completion-anchored double-cancel.
        let clear_pickup_stamp = quest
            .get("cooldown_anchor")
            .and_then(Value::as_str)
            .is_some_and(|anchor| anchor == "pickup");
        let quest_name = quest["name"].as_str().unwrap_or("Quest").to_string();
        let now = self.now_epoch();
        let correction = self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let completion = tx
                    .query_row(
                        "SELECT c.id, c.ledger_entry_id, c.quest_claim_id, c.quest_run_id, \
                                c.completed_at, c.session_id \
                         FROM session_quest_completions c \
                         WHERE c.quest_id = ? \
                         ORDER BY c.completed_at DESC, c.id DESC LIMIT 1",
                        rusqlite::params![quest_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                                row.get::<_, Option<i64>>(3)?,
                                row.get::<_, f64>(4)?,
                                row.get::<_, String>(5)?,
                            ))
                        },
                    )
                    .optional()?;
                if cooling {
                    if let Some((completion_id, _, _, _, _, _)) = &completion {
                        tx.execute(
                            "INSERT OR IGNORE INTO quest_cooldown_resets \
                             (quest_id, completion_id, reset_at) VALUES (?, ?, ?)",
                            rusqlite::params![quest_id, completion_id, now],
                        )?;
                    }
                }
                if clear_pickup_stamp {
                    tx.execute(
                        "UPDATE quests SET last_started_at = NULL WHERE id = ?",
                        rusqlite::params![quest_id],
                    )?;
                }

                if undo_reward {
                    let Some((completion_id, legacy_ledger_id, claim_id, _, completed_at, session_id)) = &completion else {
                        return Ok(Err("There is no completed reward to undo".to_string()));
                    };
                    let already_reversed: i64 = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM quest_reward_reversals \
                         WHERE completion_id = ?)",
                        rusqlite::params![completion_id],
                        |row| row.get(0),
                    )?;
                    if already_reversed != 0 {
                        return Ok(Err("That quest reward has already been undone".to_string()));
                    }
                    let dependent_stock: i64 = tx.query_row(
                        "SELECT EXISTS( \
                         SELECT 1 FROM stock_movements m \
                         JOIN session_quest_completion_reward_items ri \
                           ON ri.id = m.quest_reward_item_id \
                         WHERE ri.completion_id = ?)",
                        rusqlite::params![completion_id],
                        |row| row.get(0),
                    )?;
                    if dependent_stock != 0 {
                        return Ok(Err(
                            "This reward stock has been listed, sold, converted, or removed; undo that transaction first"
                                .to_string(),
                        ));
                    }

                    let mut liquid_total: f64 = tx.query_row(
                        "SELECT COALESCE(SUM(ri.value_ped), 0) \
                         FROM session_quest_completion_reward_items ri \
                         WHERE ri.completion_id = ? AND ri.accounting_kind = 'liquid'",
                        rusqlite::params![completion_id],
                        |row| row.get(0),
                    )?;
                    if let Some(ledger_id) = legacy_ledger_id {
                        liquid_total += tx
                            .query_row(
                                "SELECT amount FROM ledger_entries WHERE id = ? AND type = 'markup'",
                                rusqlite::params![ledger_id],
                                |row| row.get::<_, f64>(0),
                            )
                            .optional()?
                            .unwrap_or(0.0);
                    }
                    let liquid_reversal_id = if liquid_total > 0.0 {
                        let id = format!("quest-reward-reversal-{completion_id}");
                        tx.execute(
                            "INSERT INTO ledger_entries \
                             (id, date, type, description, amount, tag) \
                             VALUES (?, ?, 'expense', ?, ?, 'quest_reward')",
                            rusqlite::params![
                                id,
                                to_iso_utc(now),
                                format!("Quest reward reversal: {quest_name}"),
                                liquid_total,
                            ],
                        )?;
                        Some(id)
                    } else {
                        None
                    };

                    let pes_reversal_claim_id = if let Some(claim_id) = claim_id {
                        let original: Option<(Option<i64>, String, f64)> = tx
                            .query_row(
                                "SELECT quest_id, quest_name, ped_value FROM quest_claims WHERE id = ?",
                                rusqlite::params![claim_id],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                            )
                            .optional()?;
                        if let Some((claim_quest_id, claim_name, value)) = original {
                            tx.execute(
                                "INSERT INTO quest_claims \
                                 (quest_id, quest_name, ped_value, claimed_at) VALUES (?, ?, ?, ?)",
                                rusqlite::params![claim_quest_id, claim_name, -value, now],
                            )?;
                            Some(tx.last_insert_rowid())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    tx.execute(
                        "INSERT INTO quest_reward_reversals \
                         (completion_id, reversed_at, liquid_ledger_entry_id, pes_reversal_claim_id) \
                         VALUES (?, ?, ?, ?)",
                        rusqlite::params![
                            completion_id,
                            now,
                            liquid_reversal_id,
                            pes_reversal_claim_id,
                        ],
                    )?;

                    let reviewed_kill_ids = {
                        let mut stmt = tx.prepare(
                            "SELECT DISTINCT li.kill_id FROM kill_loot_items li \
                             JOIN quest_reward_review_items qri \
                               ON qri.source_loot_item_id = li.id \
                             JOIN quest_reward_reviews qr ON qr.id = qri.review_id \
                             WHERE qr.completion_id = ?",
                        )?;
                        let rows = stmt
                            .query_map(rusqlite::params![completion_id], |row| {
                                row.get::<_, String>(0)
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        rows
                    };
                    tx.execute(
                        "UPDATE kill_loot_items SET deactivated_at = NULL \
                         WHERE id IN (SELECT source_loot_item_id \
                                      FROM quest_reward_review_items qri \
                                      JOIN quest_reward_reviews qr ON qr.id = qri.review_id \
                                      WHERE qr.completion_id = ?)",
                        rusqlite::params![completion_id],
                    )?;
                    for kill_id in reviewed_kill_ids {
                        recompute_kill_loot_total(&tx, &kill_id)?;
                    }
                    let clump_source: Option<(String, String)> = tx
                        .query_row(
                            "SELECT source_kind, source_record_id FROM quest_reward_clumps \
                             WHERE claimed_completion_id = ?",
                            rusqlite::params![completion_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?;
                    if let Some((source_kind, source_record_id)) = clump_source {
                        if source_kind == "kill" {
                            tx.execute(
                                "UPDATE kill_loot_items SET deactivated_at = NULL \
                                 WHERE kill_id = ?",
                                rusqlite::params![source_record_id],
                            )?;
                            recompute_kill_loot_total(&tx, &source_record_id)?;
                        } else {
                            tx.execute(
                                "UPDATE harvest_loot_items SET deactivated_at = NULL \
                                 WHERE harvest_id = ?",
                                rusqlite::params![source_record_id],
                            )?;
                            tx.execute(
                                "UPDATE harvest_events SET loot_total_ped = \
                                 (SELECT COALESCE(SUM(value_ped), 0) \
                                  FROM harvest_loot_items \
                                  WHERE harvest_id = ? AND deactivated_at IS NULL) \
                                 WHERE id = ?",
                                rusqlite::params![source_record_id, source_record_id],
                            )?;
                        }
                    }
                    crate::session_rollup::recompute_session(&tx, session_id)?;
                    crate::daily_rollup::refresh_session_days(&tx, session_id)?;
                    crate::daily_rollup::refresh_days(
                        &tx,
                        [
                            crate::daily_rollup::epoch_day(*completed_at),
                            crate::daily_rollup::epoch_day(now),
                        ],
                    )?;
                }
                tx.commit()?;
                Ok(Ok(()))
            })
            .await?;
        correction.map_err(QuestError::Invalid)?;
        if undo_reward {
            let source_ids = self
                .db
                .with_reader(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT k.loot_source_id \
                         FROM session_quest_completions c \
                         JOIN quest_reward_reviews qr ON qr.completion_id = c.id \
                         JOIN quest_reward_review_items qri ON qri.review_id = qr.id \
                         JOIN kill_loot_items li ON li.id = qri.source_loot_item_id \
                         JOIN kills k ON k.id = li.kill_id \
                         WHERE c.quest_id = ? AND k.loot_source_id IS NOT NULL \
                         UNION \
                         SELECT DISTINCT clump.source_id \
                         FROM session_quest_completions c \
                         JOIN quest_reward_clumps clump ON clump.claimed_completion_id = c.id \
                         WHERE c.quest_id = ?",
                    )?;
                    let rows = stmt
                        .query_map(rusqlite::params![quest_id, quest_id], |row| {
                            row.get::<_, String>(0)
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows)
                })
                .await?;
            for source_id in source_ids {
                self.report_loot_reconciled(source_id).await;
            }
        }
        self.get_quest(quest_id).await
    }

    /// Insert an overlay event when a tracking session is active; any
    /// failure is swallowed, exactly as the original's bare except.
    pub(super) async fn record_notable_event(
        &self,
        kind: NotableEventKind,
        description: &str,
        value: Ped,
    ) {
        // The original gates on truthiness, so an empty session id
        // skips the write exactly like an absent one.
        let Some(session_id) = self.current_session().filter(|id| !id.is_empty()) else {
            return;
        };
        let now = self.now_epoch();
        let event_type = kind.as_str();
        let description = description.to_string();
        let value = value.value();
        let _ = self
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO notable_events \
                     (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) \
                     VALUES (?, NULL, ?, ?, ?, ?)",
                    rusqlite::params![session_id, event_type, description, value, now],
                )?;
                Ok(())
            })
            .await;
    }

    /// Whether the quest's OWN cooldown window is still open against
    /// the injected clock, anchored per its own anchor. Deliberately
    /// blind to the family window: this predicate gates the cancel
    /// flow's reset branch, so FAMILY cooling alone never makes a
    /// member cancellable. (The reset itself disavows the quest's own
    /// start fact; a family window derived from that same fact moves
    /// with it, which is the point of the correction.)
    fn is_quest_cooling(&self, quest: &Value) -> bool {
        let anchor = quest.get("cooldown_anchor").and_then(Value::as_str);
        let last = match anchor {
            Some("pickup") => quest.get("last_started_at").and_then(Value::as_f64),
            _ => quest.get("last_completed_at").and_then(Value::as_f64),
        };
        let cooldown_hours = quest.get("cooldown_hours").and_then(Value::as_f64);
        let (Some(last), Some(cooldown_hours)) = (last, cooldown_hours) else {
            return false;
        };
        if cooldown_hours <= 0.0 {
            return false;
        }
        (last + cooldown_hours * 3600.0) > self.now_epoch()
    }
}
