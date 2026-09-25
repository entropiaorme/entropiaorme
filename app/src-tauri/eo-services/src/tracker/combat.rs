//! Combat-stream handlers: shot recording with weapon attribution and
//! cost phases, the per-kill accumulator, hotbar tool changes, and
//! enhancer breaks.

use eo_wire::domain_events::{TrackingReason, TrackingStatus};

use crate::bus_events::{BusEvent, CombatPayload, HotbarItemKind};
use crate::expected_hunting::OffensiveLoadoutEvidence;
use crate::ped::Ped;
use crate::tracking_models::ToolStats;

use super::actor::TrackerActor;
use super::attribution::{Attribution, Observation, ShotLocation, ShotRecord};
use super::providers::Providers;
use super::session::ActiveSession;
use super::time::{instant_to_epoch, resolve_local};
use super::weapon_evidence::ShotEvidence;
use super::weapons::break_matches_active_weapon;

/// The phase an unpriced shot is counted under: no weapon is known, so
/// the shot is recorded without a cost.
pub(super) const UNPRICED_TOOL: &str = "Unknown";

/// Combat stats since the last kill (or session start).
#[derive(Default)]
pub(super) struct Accumulator {
    pub(super) shots_fired: i64,
    pub(super) damage_dealt: f64,
    pub(super) damage_taken: f64,
    pub(super) critical_hits: i64,
    pub(super) enhancer_cost: Ped,
    /// Keyed by phase key (the bare tool name, then `name#2`...), in
    /// first-seen order.
    pub(super) tool_stats: Vec<(String, ToolStats)>,
    /// Stored weapon evidence for this kill's shots, written with it.
    pub(super) evidence: Vec<ShotEvidence>,
}

struct DefenceEvidence {
    session_id: String,
    context_id: Option<i64>,
    damage: Option<f64>,
    deflected: bool,
}

impl Accumulator {
    pub(super) fn reset(&mut self) {
        *self = Accumulator::default();
    }

    pub(super) fn weapon_cost(&self) -> Ped {
        self.tool_stats
            .iter()
            .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired)
            .sum()
    }

    pub(super) fn total_cost(&self) -> Ped {
        self.weapon_cost() + self.enhancer_cost
    }
}

impl TrackerActor {
    /// The stats entry for this tool at this cost: an existing phase within
    /// the cost tolerance, or a new phase keyed `name`, then `name#2`...
    pub(super) fn tool_stats_for_phase<'a>(
        tool_stats: &'a mut Vec<(String, ToolStats)>,
        tool_name: &str,
        cost_per_shot: Ped,
        expected_economics: Option<OffensiveLoadoutEvidence>,
    ) -> &'a mut ToolStats {
        if let Some(index) = tool_stats.iter().position(|(_, stats)| {
            stats.tool_name == tool_name
                && (stats.cost_per_shot.value() - cost_per_shot.value()).abs() < 1e-9
                && stats.expected_economics == expected_economics
        }) {
            return &mut tool_stats[index].1;
        }
        let phase_count = tool_stats
            .iter()
            .filter(|(_, stats)| stats.tool_name == tool_name)
            .count();
        let key = if phase_count == 0 {
            tool_name.to_string()
        } else {
            format!("{tool_name}#{}", phase_count + 1)
        };
        tool_stats.push((
            key,
            ToolStats::new(tool_name, cost_per_shot, expected_economics),
        ));
        &mut tool_stats.last_mut().expect("just pushed").1
    }

    /// Accumulate one offensive observation: a hit, or a jam/dodge/evade
    /// countered shot. Attribution names the weapon (or leaves the shot
    /// unpriced); an effect tick is damage an earlier activation already
    /// paid for, so it counts no shot and books no cost.
    fn record_offensive_shot(
        providers: &Providers,
        active: &mut ActiveSession,
        observation: Observation,
        observed_at: f64,
    ) {
        let resolved = active
            .weapons
            .attribution
            .classify(observation, observed_at);
        let seq = active.weapons.attribution.apply(&resolved, observed_at);
        let (amount, critical) = match observation {
            Observation::Hit { amount, critical } => (amount, critical),
            Observation::Countered => (0.0, false),
        };
        if amount > 0.0 {
            active.accumulator.damage_dealt += amount;
        }
        if matches!(resolved.attribution, Attribution::EffectTick { .. }) {
            let row = ShotEvidence::for_shot(
                active,
                observation,
                observed_at,
                &resolved,
                None,
                Ped::ZERO,
            );
            active.accumulator.evidence.push(row);
            return;
        }
        active.accumulator.shots_fired += 1;
        if critical {
            active.accumulator.critical_hits += 1;
        }

        let tool = resolved.attribution.priced_tool().map(str::to_string);
        let cost = Self::book_shot(providers, active, tool.as_deref(), amount, critical);
        let evidence_id =
            ShotEvidence::keeps_row(&resolved.attribution, &active.weapons.attribution).then(
                || {
                    let row = ShotEvidence::for_shot(
                        active,
                        observation,
                        observed_at,
                        &resolved,
                        tool.clone(),
                        cost,
                    );
                    let id = row.id.clone();
                    active.accumulator.evidence.push(row);
                    id
                },
            );
        active.weapons.attribution.remember(ShotRecord {
            seq,
            observed_at,
            observation,
            attribution: resolved.attribution,
            fits: resolved.fits,
            location: ShotLocation::Pending,
            evidence_id,
            booked: tool,
            cost,
        });
    }

    /// Count one shot into the accumulator's phase for its weapon, and
    /// return the per-shot cost it was booked at. A shot with no weapon is
    /// counted under the unpriced phase at no cost: a price is never
    /// guessed.
    pub(super) fn book_shot(
        providers: &Providers,
        active: &mut ActiveSession,
        tool: Option<&str>,
        amount: f64,
        critical: bool,
    ) -> Ped {
        let (cost, expected_economics) = match tool {
            Some(tool) => (
                Self::current_cost_for_tool(providers, &mut active.weapons, tool, Ped::ZERO),
                Self::expected_evidence_for_tool(
                    providers,
                    &mut active.weapons,
                    tool,
                    active.hunting_looters,
                ),
            ),
            None => (Ped::ZERO, None),
        };
        let tool_stats = &mut active.accumulator.tool_stats;
        let stats: &mut ToolStats = match tool {
            Some(tool) if cost.is_positive() => {
                Self::tool_stats_for_phase(tool_stats, tool, cost, expected_economics)
            }
            _ => {
                let key = tool
                    .filter(|name| !name.is_empty())
                    .unwrap_or(UNPRICED_TOOL);
                let index = match tool_stats.iter().position(|(phase, _)| phase == key) {
                    Some(index) => index,
                    None => {
                        tool_stats.push((key.to_string(), ToolStats::new(key, Ped::ZERO, None)));
                        tool_stats.len() - 1
                    }
                };
                let entry = &mut tool_stats[index].1;
                // A named weapon the profile lookup could not price may
                // still have a library cost; it resolves once, for a
                // still-costless entry. The unpriced phase never looks one
                // up.
                if tool.is_some() && !entry.cost_per_shot.is_positive() {
                    let fallback = Ped(providers.equipment.cost_per_shot(key));
                    if fallback.is_positive() {
                        entry.cost_per_shot = fallback;
                    }
                }
                entry
            }
        };
        stats.shots_fired += 1;
        if amount > 0.0 {
            stats.damage_dealt += amount;
        }
        if critical {
            stats.critical_hits += 1;
        }
        stats.cost_per_shot
    }

    /// Handle a parsed combat event from chat.log. The whole body
    /// mutates owned in-memory state, so it runs under the guard;
    /// there is no DB write or publish. Defensive incoming events
    /// stay out of the kills model.
    pub(super) async fn on_combat(&mut self, event: &BusEvent) {
        let BusEvent::Combat(payload) = event else {
            return;
        };
        if let CombatPayload::SelfHeal { amount, timestamp } = payload {
            let _ = self.on_self_heal(*amount, timestamp).await;
            return;
        }
        let observed_at = instant_to_epoch(resolve_local(self.clock.now()));
        let Self {
            db,
            session,
            providers,
            ..
        } = self;
        let Some(active) = session.active_mut() else {
            return;
        };

        // Whether this event actually changed the live session
        // readout: the coalesced tracking.session.updated fires only
        // on a real mutation, so a duplicate self-heal tick or an
        // unhandled combat kind does not wake listeners for a no-op.
        let mut mutated = false;
        let mut defence: Option<DefenceEvidence> = None;

        match payload {
            CombatPayload::DamageDealt { amount, .. } => {
                let hit = Observation::Hit {
                    amount: *amount,
                    critical: false,
                };
                Self::record_offensive_shot(providers, active, hit, observed_at);
                active.healing.note_damage(observed_at, *amount);
                mutated = true;
            }
            CombatPayload::CriticalHit { amount, .. } => {
                let hit = Observation::Hit {
                    amount: *amount,
                    critical: true,
                };
                Self::record_offensive_shot(providers, active, hit, observed_at);
                active.healing.note_damage(observed_at, *amount);
                mutated = true;
            }
            CombatPayload::TargetDodge { .. }
            | CombatPayload::TargetEvade { .. }
            | CombatPayload::TargetJam { .. } => {
                Self::record_offensive_shot(providers, active, Observation::Countered, observed_at);
                mutated = true;
            }
            CombatPayload::DamageReceived { amount, .. } => {
                active.accumulator.damage_taken += amount;
                defence = Some(DefenceEvidence {
                    session_id: active.session.id.clone(),
                    context_id: active.intervals.context_id(),
                    damage: Some(*amount),
                    deflected: false,
                });
                mutated = true;
            }
            CombatPayload::SelfHeal { .. } => unreachable!("handled before the state borrow"),
            // The player-defence kinds are parsed and recorded on the
            // stream but do not move the session model, as before.
            CombatPayload::PlayerDodge { .. }
            | CombatPayload::PlayerEvade { .. }
            | CombatPayload::PlayerJam { .. }
            | CombatPayload::MobMiss { .. } => {}
            CombatPayload::Deflect { .. } => {
                defence = Some(DefenceEvidence {
                    session_id: active.session.id.clone(),
                    context_id: active.intervals.context_id(),
                    damage: None,
                    deflected: true,
                });
            }
        }

        if mutated {
            active.dirty = true;
        }
        if !active.facets.track_protection_costs {
            defence = None;
        }
        if let Some(defence) = defence {
            let stored = db
                .with_writer(move |conn| {
                    conn.execute(
                        "INSERT INTO protection_defence_events \
                         (session_id, context_id, damage, deflected) \
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![
                            defence.session_id,
                            defence.context_id,
                            defence.damage,
                            defence.deflected as i64
                        ],
                    )?;
                    Ok(())
                })
                .await;
            if let Err(error) = stored {
                tracing::error!(target: "eo::tracker", %error, "defensive evidence write failed");
                if !active.protection_evidence_warning_emitted {
                    active.warnings.push(
                        "Protection accounting degraded: defensive evidence could not be saved"
                            .to_string(),
                    );
                    active.protection_evidence_warning_emitted = true;
                }
                active.dirty = true;
            }
        }
    }

    /// Handle a weapon equip reported without its press instant: the press
    /// is taken as now.
    pub(super) fn on_tool_changed(&mut self, event: &BusEvent) {
        let BusEvent::ActiveToolChanged(payload) = event else {
            return;
        };
        let pressed_at = instant_to_epoch(resolve_local(self.clock.now()));
        self.on_weapon_press(&payload.tool_name, pressed_at);
    }

    /// A hotbar weapon press: the pressed weapon is declared from here and
    /// a new attribution regime starts. Shots already recorded keep their
    /// attribution; the previous weapon's shots may still land for a
    /// moment after the press.
    pub(super) fn on_weapon_press(&mut self, tool_name: &str, pressed_at: f64) {
        if tool_name.is_empty() {
            return;
        }
        let nudge_session_id = {
            let Self {
                session,
                providers,
                held_item,
                ..
            } = &mut *self;
            // A weapon equip takes the hand back from the harvesting tool
            // (display state; see the actor field).
            let hand_changed = held_item
                .as_ref()
                .is_none_or(|item| item.0 != tool_name || item.1 != HotbarItemKind::Weapon);
            *held_item = Some((tool_name.to_string(), HotbarItemKind::Weapon));
            let Some(active) = session.active_mut() else {
                return;
            };
            // Any hotbar press re-syncs the app's belief with the game; a
            // standing cue of either guardrail is resolved by it (a readout
            // change worth a nudge), and no evidence may reach back past it.
            let cleared_mismatch = active.guardrail_mismatch.take().is_some()
                | active.weapons.attribution.mismatch().is_some();
            active.harvest_press_floor = active.session.harvests.len();
            let declared_changed = active.weapons.attribution.declared() != Some(tool_name);
            active.weapons.attribution.declare(tool_name, pressed_at);
            // Resolve the weapon's cost state now, so an enhancer break
            // before its first shot applies to it.
            Self::current_cost_for_tool(providers, &mut active.weapons, tool_name, Ped::ZERO);

            // A switch changes the overlay's weapon readout. The coalesced
            // session-update tick only flushes on chat-log activity, so a
            // switch with no combat would leave the overlay stale; nudge a
            // re-hydrate directly, stamped from the injected clock.
            (hand_changed || cleared_mismatch || declared_changed)
                .then(|| active.session.id.clone())
        };

        if let Some(session_id) = nudge_session_id {
            self.emit_session_event(
                TrackingReason::Updated,
                TrackingStatus::Active,
                instant_to_epoch(resolve_local(self.clock.now())),
                Some(&session_id),
            );
        }
    }

    /// Handle hotbar-driven heal tool equip: the hand now holds the healer
    /// (display state; healing billing follows the intent path). The
    /// re-hydrate nudge fires only against an active session.
    pub(super) fn on_heal_tool_changed(&mut self, event: &BusEvent) {
        let BusEvent::ActiveHealToolChanged(payload) = event else {
            return;
        };
        let held_changed = self
            .held_item
            .as_ref()
            .is_none_or(|item| item.0 != payload.tool_name || item.1 != HotbarItemKind::Healing);
        self.held_item = Some((payload.tool_name.clone(), HotbarItemKind::Healing));
        let nudge_session_id = self
            .session
            .active()
            .filter(|_| held_changed)
            .map(|active| active.session.id.clone());
        if let Some(session_id) = nudge_session_id {
            self.emit_session_event(
                TrackingReason::Updated,
                TrackingStatus::Active,
                instant_to_epoch(resolve_local(self.clock.now())),
                Some(&session_id),
            );
        }
    }

    /// Handle an enhancer break event: update enhancer state for
    /// future shots. There is no DB write or publish.
    pub(super) fn on_enhancer_break(&mut self, event: &BusEvent) {
        let BusEvent::EnhancerBreak(payload) = event else {
            return;
        };
        let Some(active) = self.session.active_mut() else {
            return;
        };

        let enhancer_name = payload.enhancer_name.as_str();
        let item_name = payload.item_name.as_str();
        // The payload's break count drives the stack update directly
        // (the parser guarantees an integer; the old missing-count
        // decrement-one fallback had no producer).
        let remaining = Some(payload.remaining);

        let applies = {
            let weapon = active
                .weapons
                .active_key
                .as_ref()
                .and_then(|key| active.weapons.enhancer_states.get(key));
            match weapon {
                Some(weapon) => {
                    !weapon.stacks.is_empty()
                        && enhancer_name.to_lowercase().contains("damage")
                        && break_matches_active_weapon(&active.weapons, item_name)
                }
                None => false,
            }
        };
        if !applies {
            return;
        }

        // The break applies to the active weapon, so the readout
        // reflects it; an ignored break (filtered out above) leaves
        // the session unchanged.
        active.dirty = true;
        let key = active.weapons.active_key.clone().expect("checked above");
        active
            .weapons
            .enhancer_states
            .get_mut(&key)
            .expect("checked above")
            .apply_break(remaining);
    }
}
