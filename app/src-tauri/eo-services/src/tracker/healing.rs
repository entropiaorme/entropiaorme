//! Intent-led healing attribution.
//!
//! A resolved healer hotbar press opens an activation candidate. Chat-log
//! healing is output evidence: it can confirm one paid activation, belong to
//! an already-open effect window, or remain passive/unattributed at zero
//! cost. Cross-producer ordering is handled by retaining a short unresolved
//! output tail and reconciling it when the earlier OS key occurrence arrives.
//!
//! Persisted absolute expiry is the truth for effect windows: a session starts
//! from every live window whose expiry is still ahead, whichever session paid
//! for it, so a restart or a new session never loses a running heal-over-time
//! effect. Its later ticks keep the paying activation's provenance at zero
//! cost while stamping the context they land in. Expiry closes a window by
//! comparison with the injected clock, so it closes once however often the
//! state is read back.

use std::collections::HashMap;

use crate::bus_events::{
    ActiveHarvestToolChangedPayload, ActiveHealToolChangedPayload, BusEvent, HotbarIntentPayload,
    HotbarItemKind,
};
use crate::db::DbError;
use crate::healing_profile::HealingProfile;
use crate::ped::Ped;

use super::actor::TrackerActor;
use super::time::{instant_to_epoch, resolve_local};

const DELIVERY_TAIL_SECONDS: f64 = 1.25;
const DAMAGE_CORRELATION_SECONDS: f64 = 1.0;
/// How far back a starting session looks for each healer's latest paid use.
/// Reloads are seconds long, so an hour bounds the read without ever cutting
/// a cooldown short.
const COOLDOWN_LOOKBACK_SECONDS: f64 = 3600.0;
/// How much sooner than its reload a healer's next paid use may be observed.
/// Heals are timed by when their chat lines are read, and the log is read in
/// polls and written in bursts, so a use at the healer's full rate can reach
/// the tracker a little inside the reload. A retry during a real cooldown
/// prints no heal at all, so the allowance cannot bill one.
const COOLDOWN_LEEWAY_SECONDS: f64 = 0.3;

#[derive(Debug, Clone)]
pub(super) struct HealingIntent {
    pub(super) equipment_id: i64,
    pub(super) tool_name: String,
    pub(super) cost_per_use: Ped,
    pub(super) reload_seconds: f64,
    pub(super) profile: HealingProfile,
    pub(super) occurred_at: f64,
    pub(super) closed_at: Option<f64>,
}

#[derive(Debug, Clone)]
pub(super) struct HealingEffectWindow {
    pub(super) id: String,
    pub(super) activation_id: String,
    /// Absent once the paying item was deleted from the equipment library.
    pub(super) equipment_id: Option<i64>,
    pub(super) tool_name: String,
    pub(super) profile: HealingProfile,
    pub(super) started_at: f64,
    pub(super) expires_at: f64,
}

#[derive(Debug, Clone)]
struct PendingOutput {
    id: String,
    observed_at: f64,
    chat_timestamp: String,
    amount: f64,
    context_id: Option<i64>,
    classification: &'static str,
}

#[derive(Debug, Clone, Default)]
pub(super) struct HealingRuntime {
    pub(super) intent: Option<HealingIntent>,
    recent_intent: Option<HealingIntent>,
    pub(super) effect_windows: Vec<HealingEffectWindow>,
    last_activation: HashMap<i64, f64>,
    pending_outputs: Vec<PendingOutput>,
    last_damage: Option<(f64, f64)>,
    pub(super) weapon_lifesteal_percent: Option<f64>,
    pub(super) activation_count: i64,
    pub(super) direct_output_count: i64,
    pub(super) effect_output_count: i64,
    pub(super) passive_output_count: i64,
    pub(super) unattributed_output_count: i64,
}

#[derive(Debug, Clone)]
struct ActivationWrite {
    activation_id: String,
    output_id: String,
    effect: Option<HealingEffectWindow>,
    intent: HealingIntent,
    observed_at: f64,
    chat_timestamp: String,
    amount: f64,
    context_id: Option<i64>,
    provenance: &'static str,
    retrospective: bool,
}

/// The healing state persisted beyond any one session: every live effect
/// window whose absolute expiry is still ahead, and each healer's latest live
/// paid use.
#[derive(Debug, Default)]
pub(super) struct PersistedHealing {
    windows: Vec<HealingEffectWindow>,
    last_activation: HashMap<i64, f64>,
}

/// Read the persisted healing state as of `now`. Superseded windows and
/// activations are corrections' provenance, never live state. A window whose
/// stored profile cannot be read is skipped rather than guessed at: its later
/// ticks then stay zero-cost unattributed evidence.
pub(super) fn read_persisted_healing(
    conn: &rusqlite::Connection,
    now: f64,
) -> Result<PersistedHealing, DbError> {
    let mut windows = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT w.id, w.activation_id, w.equipment_id, w.tool_name, \
                    w.started_at, w.expires_at, a.profile_json \
             FROM healing_effect_windows w \
             JOIN healing_activations a ON a.id = w.activation_id \
             WHERE w.superseded_at IS NULL AND a.superseded_at IS NULL \
               AND w.expires_at >= ?1 \
             ORDER BY w.started_at, w.id",
        )?;
        let rows = stmt.query_map([now - DELIVERY_TAIL_SECONDS], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (id, activation_id, equipment_id, tool_name, started_at, expires_at, profile) =
                row?;
            match serde_json::from_str::<HealingProfile>(&profile) {
                Ok(profile) => windows.push(HealingEffectWindow {
                    id,
                    activation_id,
                    equipment_id,
                    tool_name,
                    profile,
                    started_at,
                    expires_at,
                }),
                Err(error) => tracing::warn!(
                    target: "eo::tracker",
                    window = %id,
                    %error,
                    "healing effect window has an unreadable profile; not restored",
                ),
            }
        }
    }
    let mut last_activation = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT equipment_id, MAX(observed_at) FROM healing_activations \
             WHERE superseded_at IS NULL AND equipment_id IS NOT NULL \
               AND observed_at >= ?1 \
             GROUP BY equipment_id",
        )?;
        let rows = stmt.query_map([now - COOLDOWN_LOOKBACK_SECONDS], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (equipment_id, observed_at) = row?;
            last_activation.insert(equipment_id, observed_at);
        }
    }
    Ok(PersistedHealing {
        windows,
        last_activation,
    })
}

impl HealingRuntime {
    /// Adopt the persisted state as the live effect windows and cooldowns.
    /// Every live activation is written before memory changes, so the
    /// persisted state already holds this session's own windows; replacing
    /// wholesale is what lets a correction elsewhere take a window away or
    /// give one back.
    pub(super) fn restore(&mut self, persisted: PersistedHealing) {
        self.effect_windows = persisted.windows;
        self.last_activation = persisted.last_activation;
    }

    pub(super) fn last_activation_at(&self, equipment_id: i64) -> Option<f64> {
        self.last_activation.get(&equipment_id).copied()
    }

    pub(super) fn note_damage(&mut self, observed_at: f64, amount: f64) {
        self.last_damage = Some((observed_at, amount));
    }

    fn prune(&mut self, observed_at: f64) {
        self.effect_windows
            .retain(|window| window.expires_at + DELIVERY_TAIL_SECONDS >= observed_at);
        self.pending_outputs
            .retain(|output| observed_at - output.observed_at <= DELIVERY_TAIL_SECONDS);
        if self
            .recent_intent
            .as_ref()
            .and_then(|intent| intent.closed_at)
            .is_some_and(|closed| observed_at - closed > DELIVERY_TAIL_SECONDS)
        {
            self.recent_intent = None;
        }
    }

    fn candidate(&self, observed_at: f64) -> Option<&HealingIntent> {
        self.intent
            .as_ref()
            .filter(|intent| intent.occurred_at <= observed_at + 0.05)
            .or_else(|| {
                self.recent_intent.as_ref().filter(|intent| {
                    intent.occurred_at <= observed_at + 0.05
                        && intent
                            .closed_at
                            .is_some_and(|closed| observed_at <= closed + DELIVERY_TAIL_SECONDS)
                })
            })
    }

    fn matching_effect(
        &self,
        observed_at: f64,
        amount: f64,
    ) -> Option<(Option<HealingEffectWindow>, &'static str)> {
        let mut matches = self.effect_windows.iter().rev().filter(|window| {
            window.started_at <= observed_at + 0.05
                && observed_at <= window.expires_at + DELIVERY_TAIL_SECONDS
                && window.profile.tick_matches(amount)
        });
        let first = matches.next()?.clone();
        if matches.next().is_some() {
            Some((
                None,
                "matched several active healing effect windows; source left ambiguous",
            ))
        } else {
            Some((Some(first), "matched an active healing effect window"))
        }
    }

    fn damage_correlated(&self, observed_at: f64, amount: f64) -> bool {
        let Some((damage_at, damage)) = self.last_damage else {
            return false;
        };
        if observed_at - damage_at > DAMAGE_CORRELATION_SECONDS || observed_at < damage_at - 0.05 {
            return false;
        }
        match self.weapon_lifesteal_percent {
            Some(percent) => {
                let expected = damage * percent / 100.0;
                let tolerance = (expected * 0.2).max(0.5);
                (amount - expected).abs() <= tolerance
            }
            None => true,
        }
    }

    fn cooldown_ready(&self, intent: &HealingIntent, observed_at: f64) -> bool {
        self.last_activation
            .get(&intent.equipment_id)
            .is_none_or(|last| {
                observed_at - last >= (intent.reload_seconds - COOLDOWN_LEEWAY_SECONDS).max(0.0)
            })
    }

    fn activation_match(&self, intent: &HealingIntent, amount: f64) -> Option<&'static str> {
        intent
            .profile
            .confirms_activation(amount)
            .then_some("direct")
    }
}

impl TrackerActor {
    pub(super) async fn on_hotbar_intent(&mut self, event: &BusEvent) {
        let BusEvent::HotbarIntent(payload) = event else {
            return;
        };
        let should_reconcile = {
            let Some(active) = self.session.active_mut() else {
                return;
            };
            if payload
                .session_id
                .as_ref()
                .is_some_and(|session_id| session_id != &active.session.id)
            {
                return;
            }
            active.healing.prune(payload.occurred_at);
            match payload.item_kind {
                HotbarItemKind::Healing => {
                    if let Some(profile) = payload.healing_profile.clone() {
                        active.healing.intent = Some(intent_from_payload(payload, profile));
                        true
                    } else {
                        close_healing_intent(&mut active.healing, payload.occurred_at);
                        active.healing.recent_intent = None;
                        false
                    }
                }
                HotbarItemKind::Weapon => {
                    close_healing_intent(&mut active.healing, payload.occurred_at);
                    active.healing.weapon_lifesteal_percent = payload.lifesteal_percent;
                    false
                }
                HotbarItemKind::Harvesting | HotbarItemKind::Consumable => {
                    close_healing_intent(&mut active.healing, payload.occurred_at);
                    false
                }
            }
        };

        // Active-equipment state is derived in the actor from the same
        // session-scoped event. Publishing a second, unscoped event from
        // the listener would leave a session transition between validation
        // and dispatch where stale equipment could reach the next session.
        let source = Some(format!("hotbar:{}", payload.slot));
        match payload.item_kind {
            HotbarItemKind::Weapon => {
                self.on_weapon_press(&payload.item_name, payload.occurred_at);
            }
            HotbarItemKind::Healing => {
                self.on_heal_tool_changed(&BusEvent::ActiveHealToolChanged(
                    ActiveHealToolChangedPayload {
                        tool_name: payload.item_name.clone(),
                        cost_per_use_ped: payload.cost_per_use_ped,
                        reload_seconds: payload.reload_seconds,
                        source,
                    },
                ));
            }
            HotbarItemKind::Harvesting => {
                self.on_harvest_tool_changed(&BusEvent::ActiveHarvestToolChanged(
                    ActiveHarvestToolChangedPayload {
                        tool_name: payload.item_name.clone(),
                        cost_per_use_ped: payload.cost_per_use_ped,
                        source,
                    },
                ));
            }
            HotbarItemKind::Consumable => {
                // The press is the dose: the item's key in game uses it.
                if let Some(profile) = payload.consumable_profile.clone() {
                    let start = super::doses::DoseStart {
                        equipment_id: payload.equipment_id,
                        item_name: payload.item_name.clone(),
                        profile,
                    };
                    if let Err(error) = self
                        .start_dose(start, crate::consumables::DoseSource::Hotbar, None)
                        .await
                    {
                        tracing::warn!(
                            target: "eo::tracker",
                            %error,
                            "a consumable's hotbar dose could not be recorded",
                        );
                    }
                }
            }
        }

        if should_reconcile {
            self.reconcile_pending_heal(payload.occurred_at).await;
        }
    }

    /// Adopt the persisted healing state as of `now`: the session start's
    /// read-back, and the re-read after a correction elsewhere committed. A
    /// failed read leaves the live state as it was; the session keeps
    /// tracking, and ticks no window explains stay zero-cost evidence.
    pub(super) async fn restore_persisted_healing(&mut self, now: f64) -> bool {
        if self.session.active().is_none() {
            return false;
        }
        match self
            .db
            .with_reader(move |conn| read_persisted_healing(conn, now))
            .await
        {
            Ok(persisted) => match self.session.active_mut() {
                Some(active) => {
                    active.healing.restore(persisted);
                    true
                }
                None => false,
            },
            Err(error) => {
                tracing::warn!(
                    target: "eo::tracker",
                    %error,
                    "persisted healing state could not be read; live effect windows unchanged",
                );
                false
            }
        }
    }

    /// A correction superseded or restored an activation: re-read the live
    /// windows so a taken-back effect stops explaining ticks and an undone
    /// correction's effect explains them again. The readout's effect state
    /// may have moved, so the next tick announces it.
    pub(super) async fn on_healing_updated(&mut self) {
        // A correction may have taken a heal's on-use buff away or given it
        // back.
        self.restore_doses().await;
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        if self.restore_persisted_healing(now).await {
            if let Some(active) = self.session.active_mut() {
                active.dirty = true;
            }
        }
    }

    pub(super) async fn on_self_heal(&mut self, amount: f64, chat_timestamp: &str) -> bool {
        if amount <= 0.0 {
            return false;
        }
        let observed_at = instant_to_epoch(resolve_local(self.clock.now()));
        let decision = {
            let Some(active) = self.session.active_mut() else {
                return false;
            };
            active.healing.prune(observed_at);
            let context_id = active.intervals.context_id();
            let session_id = active.session.id.clone();
            let direct = active.healing.candidate(observed_at).and_then(|intent| {
                active
                    .healing
                    .activation_match(intent, amount)
                    .filter(|_| active.healing.cooldown_ready(intent, observed_at))
                    .map(|provenance| (intent.clone(), provenance))
            });
            let capped = active.healing.candidate(observed_at).and_then(|intent| {
                (!active.healing.damage_correlated(observed_at, amount)
                    && intent.profile.health_capped_direct_matches(amount)
                    && active.healing.cooldown_ready(intent, observed_at))
                .then(|| intent.clone())
            });

            let effect = active.healing.matching_effect(observed_at, amount);
            // A fresh healer edge plus a matching direct output is sufficient
            // activation evidence even when an old effect overlaps it. Once
            // that edge ages out, the effect receives the conservative claim:
            // a merely held healer cannot turn an ambiguous tick into cost.
            let direct_has_fresh_edge = direct.as_ref().is_some_and(|(intent, _)| {
                observed_at >= intent.occurred_at - 0.05
                    && observed_at - intent.occurred_at <= DELIVERY_TAIL_SECONDS
            });
            let fresh_direct = direct.clone().filter(|_| direct_has_fresh_edge);

            if let Some((intent, provenance)) = fresh_direct {
                HealingDecision::Activation {
                    session_id,
                    write: Box::new(activation_write(
                        intent,
                        observed_at,
                        chat_timestamp,
                        amount,
                        context_id,
                        provenance,
                        false,
                    )),
                }
            } else if let Some((window, reason)) = effect {
                HealingDecision::Output {
                    session_id,
                    output_id: uuid::Uuid::new_v4().to_string(),
                    activation_id: window.as_ref().map(|match_| match_.activation_id.clone()),
                    effect_window_id: window.map(|match_| match_.id),
                    context_id,
                    observed_at,
                    chat_timestamp: chat_timestamp.to_string(),
                    amount,
                    classification: "effect",
                    reason,
                }
            } else if let Some((intent, provenance)) = direct {
                HealingDecision::Activation {
                    session_id,
                    write: Box::new(activation_write(
                        intent,
                        observed_at,
                        chat_timestamp,
                        amount,
                        context_id,
                        provenance,
                        false,
                    )),
                }
            } else if let Some(intent) = capped {
                HealingDecision::Activation {
                    session_id,
                    write: Box::new(activation_write(
                        intent,
                        observed_at,
                        chat_timestamp,
                        amount,
                        context_id,
                        "health_capped",
                        false,
                    )),
                }
            } else {
                let passive = active.healing.damage_correlated(observed_at, amount);
                HealingDecision::Uncosted {
                    session_id,
                    output: PendingOutput {
                        id: uuid::Uuid::new_v4().to_string(),
                        observed_at,
                        chat_timestamp: chat_timestamp.to_string(),
                        amount,
                        context_id,
                        classification: if passive { "passive" } else { "unattributed" },
                    },
                    reason: if passive {
                        "damage-correlated healing with no compatible paid-healer activation"
                    } else {
                        "no compatible paid-healer activation"
                    },
                }
            }
        };

        self.persist_healing_decision(decision).await
    }

    async fn reconcile_pending_heal(&mut self, intent_at: f64) {
        let pending = {
            let Some(active) = self.session.active_mut() else {
                return;
            };
            let Some(intent) = active.healing.intent.clone() else {
                return;
            };
            active
                .healing
                .pending_outputs
                .iter()
                .rev()
                .find_map(|output| {
                    let within_tail = intent_at <= output.observed_at + 0.05
                        && output.observed_at - intent_at <= DELIVERY_TAIL_SECONDS;
                    let matches = active
                        .healing
                        .activation_match(&intent, output.amount)
                        .is_some();
                    let cooldown = active.healing.cooldown_ready(&intent, output.observed_at);
                    (within_tail && matches && cooldown).then(|| output.clone())
                })
        };
        let Some(output) = pending else {
            return;
        };
        let (session_id, write) = {
            let active = self.session.active().expect("checked above");
            let intent = active.healing.intent.clone().expect("checked above");
            (
                active.session.id.clone(),
                activation_write(
                    intent,
                    output.observed_at,
                    &output.chat_timestamp,
                    output.amount,
                    output.context_id,
                    "retrospective",
                    true,
                )
                .with_output_id(output.id),
            )
        };
        let _ = self
            .persist_healing_decision(HealingDecision::Activation {
                session_id,
                write: Box::new(write),
            })
            .await;
    }

    async fn persist_healing_decision(&mut self, decision: HealingDecision) -> bool {
        match decision {
            HealingDecision::Activation { session_id, write } => {
                let profile_json = serde_json::to_string(&write.intent.profile)
                    .expect("a healing profile always serialises");
                let classification =
                    if write.effect.is_some() && !write.intent.profile.mode.has_direct() {
                        "effect"
                    } else {
                        "direct"
                    };
                let db_write = write.clone();
                let sid = session_id.clone();
                let stored = self
                    .db
                    .with_writer(move |conn| {
                        let tx = conn.transaction()?;
                        let updated = tx.execute(
                            "UPDATE tracking_sessions \
                             SET heal_cost = COALESCE(heal_cost, 0) + ? \
                             WHERE id = ? AND is_active = 1",
                            rusqlite::params![db_write.intent.cost_per_use.value(), sid],
                        )?;
                        if updated != 1 {
                            return Err(crate::db::DbError::from(
                                rusqlite::Error::QueryReturnedNoRows,
                            ));
                        }
                        tx.execute(
                            "INSERT INTO healing_activations \
                             (id, session_id, equipment_id, tool_name, intent_at, observed_at, \
                              chat_timestamp, context_id, cost_ped, profile_json, provenance, \
                              confirming_output_id, reload_speed_percent) \
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                            rusqlite::params![
                                db_write.activation_id,
                                sid,
                                db_write.intent.equipment_id,
                                db_write.intent.tool_name,
                                db_write.intent.occurred_at,
                                db_write.observed_at,
                                db_write.chat_timestamp,
                                db_write.context_id,
                                db_write.intent.cost_per_use.value(),
                                profile_json,
                                db_write.provenance,
                                db_write.output_id,
                                db_write.intent.profile.reload_speed_percent,
                            ],
                        )?;
                        if let Some(window) = &db_write.effect {
                            tx.execute(
                                "INSERT INTO healing_effect_windows \
                                 (id, activation_id, session_id, equipment_id, tool_name, \
                                  started_at, expires_at, tick_min, tick_max, tick_seconds, context_id) \
                                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                                rusqlite::params![
                                    window.id,
                                    window.activation_id,
                                    sid,
                                    window.equipment_id,
                                    window.tool_name,
                                    window.started_at,
                                    window.expires_at,
                                    window.profile.tick_min,
                                    window.profile.tick_max,
                                    window.profile.tick_seconds,
                                    db_write.context_id,
                                ],
                            )?;
                        }
                        if db_write.retrospective {
                            tx.execute(
                                "UPDATE healing_outputs SET activation_id = ?, effect_window_id = ?, \
                                 classification = ?, reason = ? WHERE id = ? AND session_id = ?",
                                rusqlite::params![
                                    db_write.activation_id,
                                    db_write.effect.as_ref().map(|window| window.id.as_str()),
                                    classification,
                                    "reconciled with an earlier hotbar occurrence",
                                    db_write.output_id,
                                    sid,
                                ],
                            )?;
                        } else {
                            tx.execute(
                                "INSERT INTO healing_outputs \
                                 (id, session_id, activation_id, effect_window_id, context_id, \
                                  observed_at, chat_timestamp, amount, classification, reason) \
                                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                                rusqlite::params![
                                    db_write.output_id,
                                    sid,
                                    db_write.activation_id,
                                    db_write.effect.as_ref().map(|window| window.id.as_str()),
                                    db_write.context_id,
                                    db_write.observed_at,
                                    db_write.chat_timestamp,
                                    db_write.amount,
                                    classification,
                                    "confirmed a paid healing activation",
                                ],
                            )?;
                        }
                        tx.commit()?;
                        Ok(())
                    })
                    .await;
                if stored.is_err() {
                    self.healing_persistence_warning();
                    return false;
                }
                if let Some(active) = self.session.active_mut() {
                    if active.session.id == session_id {
                        active.heal_cost += write.intent.cost_per_use;
                        active
                            .healing
                            .last_activation
                            .insert(write.intent.equipment_id, write.observed_at);
                        active.healing.activation_count += 1;
                        if classification == "effect" {
                            active.healing.effect_output_count += 1;
                        } else {
                            active.healing.direct_output_count += 1;
                        }
                        if let Some(window) = write.effect {
                            active.healing.effect_windows.push(window);
                        }
                        if write.retrospective {
                            if let Some(position) = active
                                .healing
                                .pending_outputs
                                .iter()
                                .position(|output| output.id == write.output_id)
                            {
                                let previous = active.healing.pending_outputs.remove(position);
                                if previous.classification == "passive" {
                                    active.healing.passive_output_count -= 1;
                                } else {
                                    active.healing.unattributed_output_count -= 1;
                                }
                            }
                        }
                        active.dirty = true;
                    }
                }
                // A tool that grants a buff on use (Eir Mk 1) opens it as a
                // dose with the paid heal; the heal already carries the cost.
                if let Some(on_use) = write.intent.profile.on_use.clone() {
                    let start = super::doses::DoseStart {
                        equipment_id: write.intent.equipment_id,
                        item_name: write.intent.tool_name.clone(),
                        profile: crate::consumables::ConsumableProfile {
                            duration_seconds: on_use.duration_seconds,
                            effects: on_use.effects,
                            tt_value_ped: 0.0,
                            markup_percent: 100.0,
                            track_cost: false,
                        },
                    };
                    if let Err(error) = self
                        .start_dose(
                            start,
                            crate::consumables::DoseSource::OnUse,
                            Some(write.activation_id.clone()),
                        )
                        .await
                    {
                        tracing::warn!(
                            target: "eo::tracker",
                            %error,
                            "a healing tool's on-use buff could not be recorded",
                        );
                    }
                }
                true
            }
            HealingDecision::Output {
                session_id,
                output_id,
                activation_id,
                effect_window_id,
                context_id,
                observed_at,
                chat_timestamp,
                amount,
                classification,
                reason,
            } => {
                let sid = session_id.clone();
                let stored = self
                    .db
                    .with_writer(move |conn| {
                        conn.execute(
                            "INSERT INTO healing_outputs \
                             (id, session_id, activation_id, effect_window_id, context_id, \
                              observed_at, chat_timestamp, amount, classification, reason) \
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                            rusqlite::params![
                                output_id,
                                sid,
                                activation_id,
                                effect_window_id,
                                context_id,
                                observed_at,
                                chat_timestamp,
                                amount,
                                classification,
                                reason,
                            ],
                        )?;
                        Ok(())
                    })
                    .await;
                if stored.is_err() {
                    self.healing_persistence_warning();
                    return false;
                }
                if let Some(active) = self.session.active_mut() {
                    active.healing.effect_output_count += 1;
                    active.dirty = true;
                }
                true
            }
            HealingDecision::Uncosted {
                session_id,
                output,
                reason,
            } => {
                let stored_output = output.clone();
                let sid = session_id.clone();
                let stored = self
                    .db
                    .with_writer(move |conn| {
                        conn.execute(
                            "INSERT INTO healing_outputs \
                             (id, session_id, context_id, observed_at, chat_timestamp, amount, \
                              classification, reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                            rusqlite::params![
                                stored_output.id,
                                sid,
                                stored_output.context_id,
                                stored_output.observed_at,
                                stored_output.chat_timestamp,
                                stored_output.amount,
                                stored_output.classification,
                                reason,
                            ],
                        )?;
                        Ok(())
                    })
                    .await;
                if stored.is_err() {
                    self.healing_persistence_warning();
                    return false;
                }
                if let Some(active) = self.session.active_mut() {
                    if output.classification == "passive" {
                        active.healing.passive_output_count += 1;
                    } else {
                        active.healing.unattributed_output_count += 1;
                    }
                    active.healing.pending_outputs.push(output);
                    active.dirty = true;
                }
                true
            }
        }
    }

    fn healing_persistence_warning(&mut self) {
        if let Some(active) = self.session.active_mut() {
            let message = "Healing evidence could not be saved; no unverified cost was added";
            if !active.warnings.iter().any(|warning| warning == message) {
                active.warnings.push(message.to_string());
            }
            active.dirty = true;
        }
    }
}

enum HealingDecision {
    Activation {
        session_id: String,
        write: Box<ActivationWrite>,
    },
    Output {
        session_id: String,
        output_id: String,
        activation_id: Option<String>,
        effect_window_id: Option<String>,
        context_id: Option<i64>,
        observed_at: f64,
        chat_timestamp: String,
        amount: f64,
        classification: &'static str,
        reason: &'static str,
    },
    Uncosted {
        session_id: String,
        output: PendingOutput,
        reason: &'static str,
    },
}

fn intent_from_payload(payload: &HotbarIntentPayload, profile: HealingProfile) -> HealingIntent {
    HealingIntent {
        equipment_id: payload.equipment_id,
        tool_name: payload.item_name.clone(),
        cost_per_use: Ped(payload.cost_per_use_ped),
        reload_seconds: payload.reload_seconds,
        profile,
        occurred_at: payload.occurred_at,
        closed_at: None,
    }
}

fn close_healing_intent(runtime: &mut HealingRuntime, occurred_at: f64) {
    if let Some(mut intent) = runtime.intent.take() {
        intent.closed_at = Some(occurred_at);
        runtime.recent_intent = Some(intent);
    }
}

fn activation_write(
    intent: HealingIntent,
    observed_at: f64,
    chat_timestamp: &str,
    amount: f64,
    context_id: Option<i64>,
    provenance: &'static str,
    retrospective: bool,
) -> ActivationWrite {
    let activation_id = uuid::Uuid::new_v4().to_string();
    let effect = intent
        .profile
        .effect_duration()
        .map(|duration| HealingEffectWindow {
            id: uuid::Uuid::new_v4().to_string(),
            activation_id: activation_id.clone(),
            equipment_id: Some(intent.equipment_id),
            tool_name: intent.tool_name.clone(),
            profile: intent.profile.clone(),
            started_at: observed_at,
            expires_at: observed_at + duration,
        });
    ActivationWrite {
        activation_id,
        output_id: uuid::Uuid::new_v4().to_string(),
        effect,
        intent,
        observed_at,
        chat_timestamp: chat_timestamp.to_string(),
        amount,
        context_id,
        provenance,
        retrospective,
    }
}

impl ActivationWrite {
    fn with_output_id(mut self, output_id: String) -> Self {
        self.output_id = output_id;
        self
    }
}
