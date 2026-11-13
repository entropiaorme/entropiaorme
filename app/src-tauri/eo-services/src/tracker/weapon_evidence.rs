//! Stored weapon evidence and the live decisions on a standing mismatch.
//!
//! Most shots agree with the declared weapon and live only in the kill's
//! per-weapon phases. The shots worth keeping individually are the ones a
//! player may want to see or correct: evidence that overrode a declared
//! weapon, hits outside the held weapon's band, shots left unpriced, and
//! effect ticks. Their rows are buffered
//! with the kill accumulator and written in the same transaction as the kill
//! they settle into (or with the session's stop, for shots after its last
//! kill), so a row exists exactly when its shot's cost does.
//!
//! A decision on a standing mismatch moves the regime's shots between
//! weapons in memory first, against a backup, then writes the decision,
//! the repriced kills, and the moved rows in one transaction; a failed write
//! restores the backup, so memory and the database never disagree.

use serde::Serialize;

use crate::db::DbError;
use crate::ped::Ped;
use crate::tracking_models::{Kill, ToolStats};

use super::actor::TrackerActor;
use super::attribution::{
    Attribution, AttributionKind, AttributionRuntime, Observation, Reprice, Resolved, ShotContext,
    ShotLocation, ShotRecord, UnpricedCharge,
};
use super::combat::UNPRICED_TOOL;
use super::providers::Providers;
use super::session::ActiveSession;
use super::time::{instant_to_epoch, resolve_local};

/// What the player decided about a standing mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MismatchDecision {
    /// The evidence is right: record the evidence weapon from now, and
    /// reprice the regime's shots it plausibly fired.
    Confirm,
    /// The hotbar is right: reprice the evidence shots back to the declared
    /// weapon, and stop that weapon's evidence overriding it this regime.
    Keep,
}

impl MismatchDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            MismatchDecision::Confirm => "confirmed",
            MismatchDecision::Keep => "kept",
        }
    }
}

/// Why a mismatch decision could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WeaponDecisionError {
    #[error("No active session")]
    NoActiveSession,
    #[error("The decision could not be saved; nothing changed")]
    NotSaved,
}

/// One carried weapon as a stored row offers it for review.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Candidate {
    pub(super) equipment_id: i64,
    pub(super) name: String,
    pub(super) fits: bool,
}

/// One open effect window as a stored row offers it: the effect the shot
/// was (or may have been) a tick of.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EffectCandidate {
    pub(super) window_id: String,
    pub(super) tool_name: String,
    /// When the paid hit that opened it landed.
    pub(super) activated_at: f64,
}

/// One stored shot.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ShotEvidence {
    pub(super) id: String,
    pub(super) session_id: String,
    pub(super) context_id: Option<i64>,
    pub(super) observed_at: f64,
    /// None for a countered shot.
    pub(super) amount: Option<f64>,
    pub(super) critical: bool,
    pub(super) kind: AttributionKind,
    /// The weapon the hotbar declared at the time.
    pub(super) hotbar_tool: Option<String>,
    /// The weapon the shot is priced to; None while unpriced.
    pub(super) tool_name: Option<String>,
    pub(super) cost_per_shot: Ped,
    pub(super) candidates: Vec<Candidate>,
    pub(super) reason: String,
    /// The one window a tick belongs to; None when several explained it.
    pub(super) effect_window_id: Option<String>,
    /// Every open window that explained it.
    pub(super) effect_candidates: Vec<EffectCandidate>,
    pub(super) review_id: Option<String>,
    /// The reload speed in effect the shot was priced under, so review
    /// prices it at the rate in force when it landed.
    pub(super) reload_speed_percent: f64,
}

impl ShotEvidence {
    /// Whether a shot's state keeps a stored row. Agreement inside the band
    /// is the silent default, and evidence with nothing declared is simply
    /// how shots are attributed without the hotbar listener: neither is
    /// worth a row per shot. Evidence that overrode a declared weapon, a hit
    /// outside the held weapon's band, an unpriced shot, and an effect tick
    /// are.
    pub(super) fn keeps_row(attribution: &Attribution, runtime: &AttributionRuntime) -> bool {
        match attribution {
            Attribution::Agrees { reason, .. } => reason.margin().is_some(),
            Attribution::Evidence { .. } => runtime.declared().is_some(),
            Attribution::Unresolved | Attribution::EffectTick { .. } => true,
        }
    }

    pub(super) fn for_shot(
        active: &ActiveSession,
        observation: Observation,
        observed_at: f64,
        resolved: &Resolved,
        tool_name: Option<String>,
        cost_per_shot: Ped,
    ) -> Self {
        let (amount, critical) = match observation {
            Observation::Hit { amount, critical } => (Some(amount), critical),
            Observation::Countered => (None, false),
        };
        let runtime = &active.weapons.attribution;
        let candidates = runtime
            .carried()
            .iter()
            .map(|weapon| Candidate {
                equipment_id: weapon.equipment_id,
                name: weapon.name.clone(),
                fits: amount.is_some() && resolved.fits.contains(&weapon.name),
            })
            .collect();
        let effect_window_id = match &resolved.attribution {
            Attribution::EffectTick { window_id } => window_id.clone(),
            _ => None,
        };
        let effect_candidates = runtime
            .effect_windows()
            .iter()
            .filter(|window| resolved.windows.contains(&window.id))
            .map(|window| EffectCandidate {
                window_id: window.id.clone(),
                tool_name: window.tool.clone(),
                activated_at: window.started_at,
            })
            .collect();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: active.session.id.clone(),
            context_id: active.intervals.context_id(),
            observed_at,
            amount,
            critical,
            kind: resolved.attribution.kind(),
            hotbar_tool: runtime.declared().map(str::to_string),
            tool_name,
            cost_per_shot,
            candidates,
            reason: resolved.reason.clone(),
            effect_window_id,
            effect_candidates,
            review_id: None,
            reload_speed_percent: active.reload_speed_percent,
        }
    }

    /// Write the row, settled into `kill_id` (None: after the session's
    /// last kill, where its cost is the session's dangling cost).
    pub(super) fn insert(
        &self,
        conn: &rusqlite::Connection,
        kill_id: Option<&str>,
    ) -> Result<(), DbError> {
        let candidates =
            serde_json::to_string(&self.candidates).map_err(|source| DbError::Decode {
                context: "weapon evidence candidates encode",
                source,
            })?;
        let effect_candidates =
            serde_json::to_string(&self.effect_candidates).map_err(|source| DbError::Decode {
                context: "weapon evidence effect candidates encode",
                source,
            })?;
        conn.execute(
            "INSERT INTO weapon_shot_evidence \
             (id, session_id, kill_id, context_id, observed_at, amount, critical, \
              attribution, hotbar_tool, tool_name, cost_per_shot, candidates_json, reason, \
              effect_window_id, review_id, effect_candidates_json, reload_speed_percent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            rusqlite::params![
                self.id,
                self.session_id,
                kill_id,
                self.context_id,
                self.observed_at,
                self.amount,
                i64::from(self.critical),
                self.kind.as_str(),
                self.hotbar_tool,
                self.tool_name,
                self.cost_per_shot.value(),
                candidates,
                self.reason,
                self.effect_window_id,
                self.review_id,
                effect_candidates,
                self.reload_speed_percent,
            ],
        )?;
        Ok(())
    }
}

/// Replace a kill's stored weapon phases and weapon cost with what memory
/// holds: the one write shape both a new kill and a repriced one use.
pub(super) fn write_kill_phases(conn: &rusqlite::Connection, kill: &Kill) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM kill_tool_stats WHERE kill_id = ?1",
        rusqlite::params![kill.id],
    )?;
    for (_, stats) in &kill.tool_stats {
        let expected_economics_json = stats
            .expected_economics
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| DbError::Decode {
                context: "kill tool expected economics encode",
                source,
            })?;
        conn.execute(
            "INSERT OR REPLACE INTO kill_tool_stats \
             (kill_id, tool_name, shots_fired, damage_dealt, \
              critical_hits, cost_per_shot, expected_economics_json, \
              evidence_fingerprint) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                kill.id,
                stats.tool_name,
                stats.shots_fired,
                stats.damage_dealt,
                stats.critical_hits,
                stats.cost_per_shot.value(),
                expected_economics_json,
                expected_economics_json.as_deref().unwrap_or(""),
            ],
        )?;
    }
    conn.execute(
        "UPDATE kills SET cost_ped = ?1 WHERE id = ?2",
        rusqlite::params![kill.cost_ped.value(), kill.id],
    )?;
    Ok(())
}

/// Take one shot out of the phase it was booked under. A phase left with
/// nothing in it disappears. False when no phase holds the shot (memory no
/// longer matches the record; the move is then skipped).
pub(super) fn unbook(
    tool_stats: &mut Vec<(String, ToolStats)>,
    booked: Option<&str>,
    cost: Ped,
    amount: f64,
    critical: bool,
) -> bool {
    let tool = booked.unwrap_or(UNPRICED_TOOL);
    let Some(index) = tool_stats.iter().position(|(_, stats)| {
        stats.tool_name == tool
            && (stats.cost_per_shot.value() - cost.value()).abs() < 1e-9
            && stats.shots_fired > 0
    }) else {
        return false;
    };
    let stats = &mut tool_stats[index].1;
    stats.shots_fired -= 1;
    if amount > 0.0 {
        stats.damage_dealt -= amount;
    }
    if critical {
        stats.critical_hits -= 1;
    }
    if stats.shots_fired == 0 && stats.damage_dealt.abs() < 1e-9 {
        tool_stats.remove(index);
    }
    true
}

/// Everything a move of shots may change, restored if its write fails.
pub(super) struct Backup {
    attribution: AttributionRuntime,
    tool_stats: Vec<(String, ToolStats)>,
    evidence: Vec<ShotEvidence>,
    kills: Vec<(usize, Kill)>,
    active_key: Option<String>,
    observed_name: Option<String>,
    held_item: Option<(String, crate::bus_events::HotbarItemKind)>,
    unpriced_cost_bound: Ped,
    unpriced_no_fit: i64,
}

impl Backup {
    pub(super) fn of(
        active: &ActiveSession,
        held_item: &Option<(String, crate::bus_events::HotbarItemKind)>,
    ) -> Self {
        Self {
            attribution: active.weapons.attribution.clone(),
            tool_stats: active.accumulator.tool_stats.clone(),
            evidence: active.accumulator.evidence.clone(),
            kills: Vec::new(),
            active_key: active.weapons.active_key.clone(),
            observed_name: active.weapons.observed_name.clone(),
            held_item: held_item.clone(),
            unpriced_cost_bound: active.unpriced_cost_bound,
            unpriced_no_fit: active.unpriced_no_fit,
        }
    }

    /// The kills the move changed, as they stand now.
    pub(super) fn saved_kills_now(&self, active: &ActiveSession) -> Vec<Kill> {
        self.kills
            .iter()
            .map(|(index, _)| active.session.kills[*index].clone())
            .collect()
    }

    /// Save a kill as it stood before the move first changes it.
    pub(super) fn save_kill(&mut self, index: usize, kill: &Kill) {
        if !self.kills.iter().any(|(saved, _)| *saved == index) {
            self.kills.push((index, kill.clone()));
        }
    }

    /// Put memory back as it stood before the move.
    pub(super) fn restore(
        self,
        active: &mut ActiveSession,
        held_item: &mut Option<(String, crate::bus_events::HotbarItemKind)>,
    ) {
        active.weapons.attribution = self.attribution;
        active.accumulator.tool_stats = self.tool_stats;
        active.accumulator.evidence = self.evidence;
        for (index, kill) in self.kills {
            active.session.kills[index] = kill;
        }
        active.weapons.active_key = self.active_key;
        active.weapons.observed_name = self.observed_name;
        active.unpriced_cost_bound = self.unpriced_cost_bound;
        active.unpriced_no_fit = self.unpriced_no_fit;
        active.dirty = true;
        *held_item = self.held_item;
    }
}

/// One logged shot to move: to another weapon, or to none, where it is
/// left unpriced.
pub(super) struct ShotMove {
    pub(super) seq: u64,
    /// The weapon it moves to; None leaves it unpriced.
    pub(super) to_tool: Option<String>,
    /// How the shot is attributed after the move, when the move itself
    /// decides it; None when the attribution runtime already did (a
    /// decision on a mismatch).
    pub(super) attribution: Option<Attribution>,
    /// The record as it stood before the move: where its cost sits, and at
    /// what.
    pub(super) record: ShotRecord,
    /// Where it was recorded, for a shot left unpriced that kept no stored
    /// row: its row is written now.
    pub(super) context: Option<ShotContext>,
    /// Why a row first written for it says it is unpriced.
    pub(super) reason: &'static str,
    /// What the shot adds to the unpriced shots' bound before the move,
    /// for a shot the retention keeps; None for a priced shot, and for a
    /// decision's moves.
    pub(super) charge: Option<UnpricedCharge>,
}

impl From<&Reprice> for ShotMove {
    fn from(reprice: &Reprice) -> Self {
        Self {
            seq: reprice.seq,
            to_tool: Some(reprice.to_tool.clone()),
            attribution: None,
            record: reprice.record.clone(),
            context: None,
            reason: "",
            charge: None,
        }
    }
}

/// A stored row a move changes: the weapon it is priced to now (none:
/// unpriced) and at what.
pub(super) struct RowUpdate {
    id: String,
    tool: Option<String>,
    cost: Ped,
}

/// What a move of shots changed in the database's terms: the kills it
/// repriced, the rows it changed, and the rows it first wrote into kills
/// already settled.
#[derive(Default)]
pub(super) struct Moved {
    pub(super) cost_delta: Ped,
    pub(super) repriced: i64,
    pub(super) kills: Vec<Kill>,
    row_updates: Vec<RowUpdate>,
    /// Rows first written into kills already settled, with the kill.
    pub(super) row_inserts: Vec<(ShotEvidence, String)>,
    /// Stored rows of settled shots that are gone: a shot placed again
    /// keeps a row only if its new attribution does.
    pub(super) row_deletes: Vec<String>,
}

impl TrackerActor {
    /// Move logged shots between weapons in memory, against `backup`: each
    /// leaves the phase it was booked under (in the accumulator, or in the
    /// kill it settled into) for the phase of the weapon it moves to, at
    /// that weapon's current price, or for the unpriced phase at none; the
    /// kill's cost follows, and so does its stored row. A shot left
    /// unpriced that kept no row gains one, as an unpriced shot always
    /// has. `review_id` names the decision the moved rows record, when a
    /// decision made the move.
    pub(super) fn move_shots(
        providers: &Providers,
        active: &mut ActiveSession,
        moves: &[ShotMove],
        review_id: Option<&str>,
        backup: &mut Backup,
    ) -> Moved {
        let mut moved = Moved::default();
        for shot in moves {
            let record = &shot.record;
            let (to_cost, to_evidence) = match &shot.to_tool {
                Some(to_tool) => (
                    Self::current_cost_for_tool(providers, &mut active.weapons, to_tool, Ped::ZERO),
                    Self::expected_evidence_for_tool(
                        providers,
                        &mut active.weapons,
                        to_tool,
                        active.hunting_looters,
                    ),
                ),
                None => (Ped::ZERO, None),
            };
            let amount = AttributionRuntime::amount_of(record);
            let critical = matches!(record.observation, Observation::Hit { critical: true, .. });
            let tool_stats = match &record.location {
                ShotLocation::Pending => &mut active.accumulator.tool_stats,
                ShotLocation::Kill(kill_id) => {
                    let Some(index) = active
                        .session
                        .kills
                        .iter()
                        .rposition(|kill| &kill.id == kill_id)
                    else {
                        continue;
                    };
                    if !backup.kills.iter().any(|(saved, _)| *saved == index) {
                        backup
                            .kills
                            .push((index, active.session.kills[index].clone()));
                    }
                    &mut active.session.kills[index].tool_stats
                }
            };
            if !unbook(
                tool_stats,
                record.booked.as_deref(),
                record.cost,
                amount,
                critical,
            ) {
                tracing::warn!(
                    target: "eo::tracker",
                    seq = shot.seq,
                    "a repriced shot was not found in its phase; left as recorded",
                );
                continue;
            }
            let phase = shot.to_tool.as_deref().unwrap_or(UNPRICED_TOOL);
            let stats = Self::tool_stats_for_phase(tool_stats, phase, to_cost, to_evidence);
            stats.shots_fired += 1;
            if amount > 0.0 {
                stats.damage_dealt += amount;
            }
            if critical {
                stats.critical_hits += 1;
            }
            if let ShotLocation::Kill(kill_id) = &record.location {
                if let Some(kill) = active
                    .session
                    .kills
                    .iter_mut()
                    .rev()
                    .find(|kill| &kill.id == kill_id)
                {
                    kill.cost_ped += to_cost - record.cost;
                }
            }
            moved.cost_delta += to_cost - record.cost;
            moved.repriced += 1;
            let mut written = None;
            match &record.evidence_id {
                Some(evidence_id) => match active
                    .accumulator
                    .evidence
                    .iter_mut()
                    .find(|row| &row.id == evidence_id)
                {
                    Some(row) => {
                        row.tool_name = shot.to_tool.clone();
                        row.cost_per_shot = to_cost;
                        if shot.to_tool.is_none() {
                            row.kind = AttributionKind::Unresolved;
                        }
                        if let Some(review_id) = review_id {
                            row.review_id = Some(review_id.to_string());
                        }
                    }
                    None => moved.row_updates.push(RowUpdate {
                        id: evidence_id.clone(),
                        tool: shot.to_tool.clone(),
                        cost: to_cost,
                    }),
                },
                None if shot.to_tool.is_none() => {
                    let row = Self::unpriced_row(active, shot);
                    written = Some(row.id.clone());
                    match &record.location {
                        ShotLocation::Pending => active.accumulator.evidence.push(row),
                        ShotLocation::Kill(kill_id) => {
                            moved.row_inserts.push((row, kill_id.clone()));
                        }
                    }
                }
                None => {}
            }
            if shot.to_tool.is_none() {
                let charge =
                    Self::note_unpriced(providers, active, record.observation, &record.fits);
                active
                    .weapons
                    .attribution
                    .set_charge(shot.seq, Some(charge));
            } else if let (Some(_), Some(charge)) = (&shot.attribution, shot.charge) {
                // A shot leaving unpriced takes back what it added.
                Self::unnote_unpriced(active, charge);
                active.weapons.attribution.set_charge(shot.seq, None);
            }
            if let Some(attribution) = &shot.attribution {
                active
                    .weapons
                    .attribution
                    .recount(&record.attribution, attribution);
            }
            active.weapons.attribution.rebook(
                shot.seq,
                shot.attribution.as_ref(),
                shot.to_tool.clone(),
                to_cost,
                written,
            );
        }
        moved.kills = backup
            .kills
            .iter()
            .map(|(index, _)| active.session.kills[*index].clone())
            .collect();
        moved
    }

    /// The stored row of a shot left unpriced by a move that kept none.
    fn unpriced_row(active: &ActiveSession, shot: &ShotMove) -> ShotEvidence {
        let record = &shot.record;
        let (amount, critical) = match record.observation {
            Observation::Hit { amount, critical } => (Some(amount), critical),
            Observation::Countered => (None, false),
        };
        let context = shot.context.unwrap_or(ShotContext {
            context_id: active.intervals.context_id(),
            reload_speed_percent: active.reload_speed_percent,
        });
        let candidates = active
            .weapons
            .attribution
            .carried()
            .iter()
            .map(|weapon| Candidate {
                equipment_id: weapon.equipment_id,
                name: weapon.name.clone(),
                fits: amount.is_some() && record.fits.contains(&weapon.name),
            })
            .collect();
        ShotEvidence {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: active.session.id.clone(),
            context_id: context.context_id,
            observed_at: record.observed_at,
            amount,
            critical,
            kind: AttributionKind::Unresolved,
            hotbar_tool: record.booked.clone(),
            tool_name: None,
            cost_per_shot: Ped::ZERO,
            candidates,
            reason: shot.reason.to_string(),
            effect_window_id: None,
            effect_candidates: Vec::new(),
            review_id: None,
            reload_speed_percent: context.reload_speed_percent,
        }
    }

    /// Decide a standing weapon mismatch. False when none stands (a stale
    /// control): nothing changes.
    pub(super) async fn decide_weapon_mismatch(
        &mut self,
        decision: MismatchDecision,
    ) -> Result<bool, WeaponDecisionError> {
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let review_id = uuid::Uuid::new_v4().to_string();
        let (backup, write) = {
            let Self {
                session,
                providers,
                held_item,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return Err(WeaponDecisionError::NoActiveSession);
            };
            let mut backup = Backup::of(active, held_item);
            let decided = match decision {
                MismatchDecision::Confirm => active.weapons.attribution.confirm(now),
                MismatchDecision::Keep => active.weapons.attribution.keep(),
            };
            let Some(decided) = decided else {
                return Ok(false);
            };

            let moves: Vec<ShotMove> = decided.moves.iter().map(ShotMove::from).collect();
            let moved = Self::move_shots(providers, active, &moves, Some(&review_id), &mut backup);

            // The weapon in hand after the decision is the declared one:
            // enhancer breaks apply to it, and the overlay shows it.
            if let Some(declared) = active.weapons.attribution.declared().map(str::to_string) {
                Self::current_cost_for_tool(providers, &mut active.weapons, &declared, Ped::ZERO);
                if decision == MismatchDecision::Confirm {
                    *held_item = Some((declared, crate::bus_events::HotbarItemKind::Weapon));
                }
            }
            active.dirty = true;

            let write = DecisionWrite {
                review_id,
                session_id: active.session.id.clone(),
                decision,
                withdrawn: decided.withdrawn,
                declared: decided.mismatch.declared,
                evidence: decided.mismatch.evidence,
                since: decided.mismatch.since,
                decided_at: now,
                moved,
            };
            (backup, write)
        };

        let session_id = write.session_id.clone();
        let stored = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO weapon_attribution_reviews \
                     (id, session_id, decision, hotbar_tool, evidence_tool, mismatch_since, \
                      decided_at, repriced_shots, cost_delta_ped) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        write.review_id,
                        write.session_id,
                        write.decision.as_str(),
                        write.declared,
                        write.evidence,
                        write.since,
                        write.decided_at,
                        write.moved.repriced,
                        write.moved.cost_delta.value(),
                    ],
                )?;
                write_moved(&tx, &write.moved, Some(&write.review_id))?;
                for window in &write.withdrawn {
                    tx.execute(
                        "UPDATE weapon_effect_windows \
                         SET withdrawn_at = ?1, withdrawn_by_review_id = ?2 WHERE id = ?3",
                        rusqlite::params![write.decided_at, write.review_id, window],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;

        if let Err(error) = stored {
            tracing::error!(target: "eo::tracker", %error, "weapon mismatch decision write failed");
            let Self {
                session, held_item, ..
            } = &mut *self;
            if let Some(active) = session.active_mut() {
                backup.restore(active, held_item);
            }
            return Err(WeaponDecisionError::NotSaved);
        }

        self.emit_session_event(
            eo_wire::domain_events::TrackingReason::Updated,
            eo_wire::domain_events::TrackingStatus::Active,
            now,
            Some(&session_id),
        );
        Ok(true)
    }
}

/// Write what a move of shots changed: the repriced kills' phases and
/// costs, the changed rows, and the rows first written into settled
/// kills. A decision's rows name its review.
pub(super) fn write_moved(
    conn: &rusqlite::Connection,
    moved: &Moved,
    review_id: Option<&str>,
) -> Result<(), DbError> {
    for kill in &moved.kills {
        write_kill_phases(conn, kill)?;
    }
    for update in &moved.row_updates {
        match (&update.tool, review_id) {
            (Some(tool), Some(review_id)) => {
                conn.execute(
                    "UPDATE weapon_shot_evidence \
                     SET tool_name = ?1, cost_per_shot = ?2, review_id = ?3 WHERE id = ?4",
                    rusqlite::params![tool, update.cost.value(), review_id, update.id],
                )?;
            }
            (Some(tool), None) => {
                conn.execute(
                    "UPDATE weapon_shot_evidence \
                     SET tool_name = ?1, cost_per_shot = ?2 WHERE id = ?3",
                    rusqlite::params![tool, update.cost.value(), update.id],
                )?;
            }
            (None, _) => {
                conn.execute(
                    "UPDATE weapon_shot_evidence \
                     SET tool_name = NULL, cost_per_shot = 0, attribution = 'unresolved' \
                     WHERE id = ?1",
                    rusqlite::params![update.id],
                )?;
            }
        }
    }
    for id in &moved.row_deletes {
        conn.execute(
            "DELETE FROM weapon_shot_evidence WHERE id = ?1",
            rusqlite::params![id],
        )?;
    }
    for (row, kill_id) in &moved.row_inserts {
        row.insert(conn, Some(kill_id))?;
    }
    Ok(())
}

/// A decision's database write, detached from the actor's state.
struct DecisionWrite {
    review_id: String,
    session_id: String,
    decision: MismatchDecision,
    /// Effect windows the decision took back.
    withdrawn: Vec<String>,
    declared: String,
    evidence: String,
    since: f64,
    decided_at: f64,
    moved: Moved,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(tool: &str, cost: f64, shots: i64, damage: f64, crits: i64) -> (String, ToolStats) {
        let mut stats = ToolStats::new(tool, Ped(cost), None);
        stats.shots_fired = shots;
        stats.damage_dealt = damage;
        stats.critical_hits = crits;
        (tool.to_string(), stats)
    }

    #[test]
    fn unbooking_takes_one_shot_out_of_its_exact_phase() {
        let mut phases = vec![
            stats("Pistol", 0.1, 3, 30.0, 1),
            stats("Pistol", 0.2, 2, 20.0, 0),
        ];
        assert!(unbook(&mut phases, Some("Pistol"), Ped(0.2), 8.0, false));
        assert_eq!(phases[1].1.shots_fired, 1);
        assert_eq!(phases[1].1.damage_dealt, 12.0);
        assert_eq!(phases[0].1.shots_fired, 3, "the other phase is untouched");
        assert!(unbook(&mut phases, Some("Pistol"), Ped(0.1), 25.0, true));
        assert_eq!(phases[0].1.critical_hits, 0);
        assert_eq!(phases[0].1.damage_dealt, 5.0);
        // No phase at that cost.
        assert!(!unbook(&mut phases, Some("Pistol"), Ped(0.3), 1.0, false));
        assert!(!unbook(&mut phases, Some("Cannon"), Ped(0.1), 1.0, false));
    }

    #[test]
    fn a_phase_left_empty_disappears_and_unpriced_shots_leave_the_unknown_phase() {
        let mut phases = vec![
            stats(UNPRICED_TOOL, 0.0, 1, 9.0, 0),
            stats("Pistol", 0.1, 1, 0.0, 0),
        ];
        assert!(unbook(&mut phases, None, Ped::ZERO, 9.0, false));
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].0, "Pistol");
        // A countered shot carries no damage.
        assert!(unbook(&mut phases, Some("Pistol"), Ped(0.1), 0.0, false));
        assert!(phases.is_empty());
        assert!(!unbook(&mut phases, None, Ped::ZERO, 0.0, false));
    }

    #[test]
    fn decisions_record_their_closed_vocabulary() {
        assert_eq!(MismatchDecision::Confirm.as_str(), "confirmed");
        assert_eq!(MismatchDecision::Keep.as_str(), "kept");
    }
}
