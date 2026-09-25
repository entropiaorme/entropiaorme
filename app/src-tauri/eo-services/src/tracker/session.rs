//! The session lifecycle (start, stop, demo priming, config reload),
//! the `ActiveSession` typestate payload, the aggregated tracking
//! readout, and the coalesced tick flush.

use chrono::{DateTime, Utc};
use eo_wire::domain_events::{TrackingReason, TrackingStatus};
use eo_wire::normalizer::round_half_even;

use super::intervals::{
    ActiveActivity, ActivityKey, ActivityRef, CloseScope, IntervalKind, IntervalSpec,
    IntervalState, ACTIVITY_KINDS,
};
use crate::bus_events::{BusEvent, SessionLifecyclePayload};
use crate::db::DbError;
use crate::expected_hunting::{
    evaluate as evaluate_expected_hunting, HuntingLooterLevels, LooterSource,
    OffensiveComponentEvidence, OffensiveComponentKind, OffensiveLoadoutEvidence,
};
use crate::mob_lookup_service::python_whitespace;
use crate::ped::Ped;
use crate::tracking_models::{
    ActiveSessionView, HarvestGuardrailMismatchView, HealingRuntimeView, TrackingReadout,
    TrackingSession, WeaponGuardrailMismatchView,
};

use super::actor::TrackerActor;
use super::attribution::WeaponMismatch;
use super::combat::Accumulator;
use super::harvest::GuardrailMismatch;
use super::healing::HealingRuntime;
use super::mob::DeclaredMob;
use super::time::{instant_to_epoch, local_isoformat, resolve_local};
use super::weapons::WeaponRuntime;
use super::{HuntTracker, SessionState, TrackerCommandError};

/// A loot group's dedup identity: (total, item count, first item name).
pub(super) type LootFingerprint = (f64, usize, String);

/// The session-scoped facets, snapshotted from the live config at
/// session start. Independent and optional by design (the co-recording
/// model that replaced the mutually exclusive tag-or-mob capture):
/// None means "not declared", never a guessed default. The name is
/// immutable for the session's life (it names the whole session, so a
/// live edit could only rewrite history; correcting it is a post-hoc
/// move in session review). The boost may be re-declared while the
/// session runs, because a pill expiring is a genuine change worth
/// recording; the record keeps the latest declaration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionFacets {
    /// The user-designated session name (the designated analytics
    /// axis; successor of the free-text tag).
    pub name: Option<String>,
    /// The session definition this session is an instance of,
    /// validated against an ACTIVE definition at start and immutable
    /// for the session's life (it rides the name facet's selection).
    pub definition_id: Option<i64>,
    /// Whether defensive hits are recorded for later armour costing.
    pub track_protection_costs: bool,
    /// The skill-boost configuration the session runs under, as the
    /// pill's labelled percentage.
    pub skill_boost_percent: Option<i64>,
}

/// Everything that exists exactly while a session runs. Constructed at
/// `start_session`, dropped wholesale when the session stops, so no
/// session-scoped field can leak into idle state or arrive stale in
/// the next session.
pub(super) struct ActiveSession {
    pub(super) session: TrackingSession,
    pub(super) accumulator: Accumulator,
    /// Whether an event since the last flushed tick changed the live
    /// readout (the coalesced update fires only on a real mutation).
    pub(super) dirty: bool,
    /// Heal cost accrued this session (the equipped tool's per-use
    /// cost per counted activation).
    pub(super) heal_cost: Ped,
    pub(super) harvest_warning_emitted: bool,
    pub(super) protection_evidence_warning_emitted: bool,
    pub(super) warnings: Vec<String>,
    /// The declared mob that kills take their stamp from (None: no
    /// declaration; kills stamp "Unknown" with no stamp source).
    pub(super) declared_mob: Option<DeclaredMob>,
    /// The session-scoped facets snapshotted at session start.
    pub(super) facets: SessionFacets,
    /// The live interval state: which intervals are open and the context
    /// every event written right now stamps. Session-scoped by
    /// construction, so no interval can outlive its session.
    pub(super) intervals: IntervalState,
    pub(super) healing: HealingRuntime,
    /// The last recorded loot group's dedup identity and instant,
    /// always stamped together.
    pub(super) last_loot: Option<(LootFingerprint, DateTime<Utc>)>,
    /// The standing harvest-guardrail disagreement, when loot evidence
    /// last contradicted the hotbar-equipped tool (see `harvest.rs`).
    pub(super) guardrail_mismatch: Option<GuardrailMismatch>,
    pub(super) guardrail_warning_emitted: bool,
    /// The harvest-list index no attribution retro pass may walk below.
    /// Every weapon or harvesting-tool hotkey press starts a fresh
    /// evidence regime.
    pub(super) harvest_press_floor: usize,
    pub(super) weapons: WeaponRuntime,
    /// Believed-current Animal, Mutant, and Robot Looter levels captured at
    /// session start so later calibration cannot rewrite this evidence.
    pub(super) hunting_looters: HuntingLooterLevels,
}

impl ActiveSession {
    pub(super) fn new(session: TrackingSession, facets: SessionFacets) -> Self {
        Self {
            session,
            accumulator: Accumulator::default(),
            dirty: false,
            heal_cost: Ped::ZERO,
            harvest_warning_emitted: false,
            protection_evidence_warning_emitted: false,
            warnings: Vec::new(),
            declared_mob: None,
            facets,
            intervals: IntervalState::default(),
            healing: HealingRuntime::default(),
            last_loot: None,
            guardrail_mismatch: None,
            guardrail_warning_emitted: false,
            harvest_press_floor: 0,
            weapons: WeaponRuntime::default(),
            hunting_looters: HuntingLooterLevels {
                animal: 0.0,
                mutant: 0.0,
                robot: 0.0,
            },
        }
    }

    /// The mob name a kill stamps or the readout shows: the declared
    /// mob's display name when it is set AND non-empty (an empty
    /// declared name behaves as unset, the original's falsy check).
    pub(super) fn stamped_mob_name(&self) -> Option<&str> {
        self.declared_mob
            .as_ref()
            .map(|declared| declared.name.as_str())
            .filter(|name| !name.is_empty())
    }
}

/// The standing activities, in the order they were opened: the ack
/// every Activities transition echoes, matching the snapshot's own
/// ordering. Opening order rather than recency, because these render as
/// a row of chips and a chip must not jump when another joins it.
fn active_activities(active: &ActiveSession) -> Vec<ActiveActivity> {
    active
        .intervals
        .open_of_kinds(&ACTIVITY_KINDS)
        .filter_map(|interval| {
            Some(ActiveActivity {
                kind: interval.kind,
                name: interval.label.clone()?,
                quest_id: interval.ref_id,
            })
        })
        .collect()
}

/// The in-memory half of the tracking readout, computed by the actor
/// against its owned state and returned detached. The caller finishes
/// the readout with the two session-scoped database reads, off the
/// actor's task.
pub(super) struct SessionAggregate {
    pub(super) session_id: String,
    pub(super) started_at: String,
    pub(super) elapsed: i64,
    pub(super) kill_count: i64,
    pub(super) cost: Ped,
    pub(super) returns: Ped,
    pub(super) damage_total: f64,
    pub(super) shots_total: i64,
    pub(super) crits_total: i64,
    pub(super) max_damage: f64,
    pub(super) live_weapon_damage: f64,
    pub(super) weapon_cost: Ped,
    pub(super) expected_tt_rate: Option<f64>,
    pub(super) expected_return_coverage: Option<f64>,
    pub(super) expected_return_model: Option<String>,
    pub(super) globals_count: i64,
    pub(super) hofs_count: i64,
    pub(super) latest_kill_loot: Option<Ped>,
    pub(super) multiplier_last: Option<f64>,
    pub(super) multiplier_avg: Option<f64>,
    pub(super) multiplier_max: Option<f64>,
    pub(super) multiplier_history: Vec<f64>,
    pub(super) cumulative_net: Vec<f64>,
    pub(super) mob_name: Option<String>,
    pub(super) session_name: Option<String>,
    pub(super) definition_id: Option<i64>,
    pub(super) track_protection_costs: bool,
    pub(super) skill_boost_percent: Option<i64>,
    pub(super) active_activities: Vec<ActiveActivity>,
    pub(super) harvest_swings: i64,
    pub(super) harvest_successes: i64,
    pub(super) harvest_loot: Ped,
    pub(super) harvest_cost: Ped,
    pub(super) guardrail_mismatch: Option<GuardrailMismatch>,
    pub(super) weapon_mismatch: Option<WeaponMismatch>,
    pub(super) unpriced_shots: i64,
    pub(super) warnings: Vec<String>,
    pub(super) healing: HealingRuntimeView,
}

impl TrackerActor {
    /// The in-memory aggregation over the live session: the held item and its
    /// semantic kind, plus the aggregate (None when idle).
    pub(super) fn aggregate(
        &self,
    ) -> (
        Option<String>,
        Option<crate::bus_events::HotbarItemKind>,
        Option<SessionAggregate>,
    ) {
        let Some(active) = self.session.active() else {
            return (
                self.held_item.as_ref().map(|item| item.0.clone()),
                self.held_item.as_ref().map(|item| item.1),
                None,
            );
        };
        // With no hotbar press yet (or the listener off), the weapon being
        // recorded is the best answer to "what is in hand".
        let current_tool = self
            .held_item
            .as_ref()
            .map(|item| item.0.clone())
            .or_else(|| active.weapons.attribution.recording().map(str::to_string));
        let current_tool_kind = self.held_item.as_ref().map(|item| item.1).or_else(|| {
            current_tool
                .as_ref()
                .map(|_| crate::bus_events::HotbarItemKind::Weapon)
        });

        let kills = &active.session.kills;
        let mut weapon_cost: Ped = kills
            .iter()
            .flat_map(|kill| kill.tool_stats.iter())
            .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired)
            .sum();
        let mut enhancer_cost: Ped = kills.iter().map(|kill| kill.enhancer_cost).sum();
        weapon_cost += active.accumulator.weapon_cost();
        enhancer_cost += active.accumulator.enhancer_cost;
        let heal_cost = active.heal_cost;

        // Flatten every immutable tool phase at the TT it actually cycled.
        // Equipment changes create distinct phases, while the three hunting
        // looters remain the session-start snapshot.
        let mut expected_evidence = OffensiveLoadoutEvidence {
            components: Vec::new(),
            looters: active.hunting_looters,
            looter_source: LooterSource::ThreeLooterMean,
        };
        for stats in kills
            .iter()
            .flat_map(|kill| kill.tool_stats.iter().map(|(_, stats)| stats))
            .chain(active.accumulator.tool_stats.iter().map(|(_, stats)| stats))
        {
            let Some(evidence) = stats.expected_economics.as_ref() else {
                if stats.shots_fired > 0 && stats.cost_per_shot.is_positive() {
                    expected_evidence
                        .components
                        .push(OffensiveComponentEvidence {
                            kind: OffensiveComponentKind::Weapon,
                            catalog_id: None,
                            name: stats.tool_name.clone(),
                            efficiency_pct: None,
                            raw_tt_per_use: stats.cost_per_shot.value() * stats.shots_fired as f64,
                            consumed_premium_per_use: 0.0,
                        });
                }
                continue;
            };
            for component in &evidence.components {
                let mut component = component.clone();
                component.raw_tt_per_use *= stats.shots_fired.max(0) as f64;
                component.consumed_premium_per_use *= stats.shots_fired.max(0) as f64;
                expected_evidence.components.push(component);
            }
        }
        let expected_result = (!expected_evidence.components.is_empty())
            .then(|| evaluate_expected_hunting(&expected_evidence).ok())
            .flatten();

        // Harvesting joins the session economy: swing decay is cycled
        // spend, wood TT is liquid loot (no new accounting class).
        let harvests = &active.session.harvests;
        let harvest_cost: Ped = harvests.iter().map(|harvest| harvest.cost_ped).sum();
        let harvest_loot: Ped = harvests.iter().map(|harvest| harvest.loot_total_ped).sum();

        let cost = weapon_cost + heal_cost + enhancer_cost + harvest_cost;
        let returns: Ped = kills.iter().map(|kill| kill.loot_total_ped).sum::<Ped>() + harvest_loot;

        let damage_total: f64 = kills.iter().map(|kill| kill.damage_dealt).sum();
        let live_weapon_damage = damage_total + active.accumulator.damage_dealt;

        // Multipliers use kill.cost_ped (weapon cost only) per EU
        // convention; a ratio of two PED amounts is dimensionless.
        let mult_per_kill: Vec<f64> = kills
            .iter()
            .filter(|kill| kill.cost_ped.is_positive())
            .map(|kill| kill.loot_total_ped / kill.cost_ped)
            .collect();
        let multiplier_avg = if mult_per_kill.is_empty() {
            None
        } else {
            Some(mult_per_kill.iter().sum::<f64>() / mult_per_kill.len() as f64)
        };
        let multiplier_max = mult_per_kill
            .iter()
            .copied()
            .fold(None, |acc: Option<f64>, value| {
                Some(acc.map_or(value, |best| best.max(value)))
            });
        let multiplier_last = kills
            .last()
            .filter(|kill| kill.cost_ped.is_positive())
            .map(|kill| kill.loot_total_ped / kill.cost_ped);
        let multiplier_history: Vec<f64> = mult_per_kill
            .iter()
            .rev()
            .take(120)
            .rev()
            .map(|value| round_half_even(*value, 4))
            .collect();

        // Cumulative-net history (per kill and per harvesting swing,
        // merged in timestamp order), distributing the session-level
        // heal cost pro-rata across kills by their weapon-cost share
        // so the curve's final point reconciles with the displayed
        // Net stat (returns - cost). Harvest swings carry their own
        // exact per-event net (wood TT minus swing decay), so they
        // take no heal share.
        let per_kill_weapon: Vec<Ped> = kills
            .iter()
            .map(|kill| {
                kill.tool_stats
                    .iter()
                    .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired)
                    .sum()
            })
            .collect();
        let total_weapon: Ped = per_kill_weapon.iter().copied().sum();
        let mut net_events: Vec<(f64, Ped)> = Vec::with_capacity(kills.len() + harvests.len());
        for (kill, weapon) in kills.iter().zip(per_kill_weapon.iter()) {
            let heal_share = if total_weapon.is_positive() {
                heal_cost * (*weapon / total_weapon)
            } else {
                Ped::ZERO
            };
            net_events.push((
                kill.timestamp,
                kill.loot_total_ped - *weapon - kill.enhancer_cost - heal_share,
            ));
        }
        for harvest in harvests {
            net_events.push((harvest.timestamp, harvest.loot_total_ped - harvest.cost_ped));
        }
        // Both sources arrive chronologically; the sort only interleaves
        // them (stable, so same-second events keep their arrival order).
        net_events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cumulative_net = Vec::with_capacity(net_events.len());
        let mut running = Ped::ZERO;
        for (_, net) in &net_events {
            running += *net;
            cumulative_net.push(running.round_half_even(2).value());
        }
        let cumulative_net: Vec<f64> = cumulative_net
            .iter()
            .rev()
            .take(120)
            .rev()
            .copied()
            .collect();

        let start_ts = instant_to_epoch(active.session.start_time);
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let effect_until = active
            .healing
            .effect_windows
            .iter()
            .map(|window| window.expires_at)
            .filter(|expires| *expires >= now)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let tool_name = active
            .healing
            .intent
            .as_ref()
            .map(|intent| intent.tool_name.clone());
        let cooldown_until = active.healing.intent.as_ref().and_then(|intent| {
            active
                .healing
                .last_activation_at(intent.equipment_id)
                .map(|last| last + intent.reload_seconds)
                .filter(|until| *until > now)
        });
        let healing_state = if effect_until.is_some() {
            "effect"
        } else if cooldown_until.is_some() {
            "cooldown"
        } else if tool_name.is_some() {
            "ready"
        } else {
            "passive"
        };
        let aggregate = SessionAggregate {
            session_id: active.session.id.clone(),
            started_at: local_isoformat(active.session.start_time),
            elapsed: (now - start_ts) as i64,
            kill_count: kills.len() as i64,
            cost,
            returns,
            damage_total,
            shots_total: kills.iter().map(|kill| kill.shots_fired).sum(),
            crits_total: kills.iter().map(|kill| kill.critical_hits).sum(),
            max_damage: kills
                .iter()
                .map(|kill| kill.damage_dealt)
                .fold(0.0, f64::max),
            live_weapon_damage,
            weapon_cost,
            expected_tt_rate: expected_result
                .as_ref()
                .and_then(|result| result.expected_tt_rate),
            expected_return_coverage: expected_result.as_ref().map(|result| result.coverage),
            expected_return_model: expected_result.map(|result| result.model_version),
            globals_count: kills.iter().filter(|kill| kill.is_global).count() as i64,
            hofs_count: kills.iter().filter(|kill| kill.is_hof).count() as i64,
            latest_kill_loot: kills.last().map(|kill| kill.loot_total_ped),
            multiplier_last,
            multiplier_avg,
            multiplier_max,
            multiplier_history,
            cumulative_net,
            mob_name: active.stamped_mob_name().map(str::to_string),
            session_name: active.facets.name.clone(),
            definition_id: active.facets.definition_id,
            track_protection_costs: active.facets.track_protection_costs,
            // Read from the interval state, not the row mirror: the row's
            // scalar cannot hold a declared zero (0019's `> 0 OR NULL`),
            // and the readout is what the overlay renders the facet from.
            skill_boost_percent: active
                .intervals
                .modifier_magnitude()
                .map(|magnitude| magnitude as i64),
            // The interval state is the only source: an activity exists
            // exactly while its session runs, so there is no idle or
            // row-mirror fallback to disagree with it.
            active_activities: active_activities(active),
            harvest_swings: harvests.len() as i64,
            harvest_successes: harvests.iter().filter(|harvest| harvest.success).count() as i64,
            harvest_loot,
            harvest_cost,
            guardrail_mismatch: active.guardrail_mismatch.clone(),
            weapon_mismatch: active.weapons.attribution.mismatch().cloned(),
            unpriced_shots: active.weapons.attribution.counts().unresolved,
            warnings: active.warnings.clone(),
            healing: HealingRuntimeView {
                tool_name,
                state: healing_state.to_string(),
                cooldown_until,
                effect_until,
                activation_count: active.healing.activation_count,
                direct_output_count: active.healing.direct_output_count,
                effect_output_count: active.healing.effect_output_count,
                passive_output_count: active.healing.passive_output_count,
                unattributed_output_count: active.healing.unattributed_output_count,
            },
        };
        (current_tool, current_tool_kind, Some(aggregate))
    }

    /// Prime the tracker with a fully-formed demo session, bypassing
    /// the normal `start_session` lifecycle (no handlers subscribe,
    /// nothing persists). It exists solely for guide-mode demo playback
    /// over a throwaway database and must never run on the live
    /// tracker.
    pub(super) fn prime_demo(
        &mut self,
        session: TrackingSession,
        declared_mob: Option<DeclaredMob>,
        facets: SessionFacets,
    ) {
        let mut active = ActiveSession::new(session, facets);
        active.declared_mob = declared_mob;
        self.session = SessionState::Active(Box::new(active));
        self.publish_status();
    }

    /// Declare the skill boost now in force. The boost is the facet
    /// that may move while the session runs, because it is a fact about
    /// the world that expires: a pill running out is a real change the
    /// session must be able to record.
    ///
    /// `percent` is three-state, and the distinction is the whole point:
    /// `None` withdraws the declaration entirely (nothing is claimed
    /// about the boost from here on), while `Some(0)` declares
    /// deliberately-unboosted play. Only the second can serve as the
    /// baseline an effect is measured against, so they must not collapse
    /// into one value.
    ///
    /// Two records move together: the interval carries the full
    /// three-state declaration (the attribution truth), and the session
    /// row mirrors the latest declaration where its column can express
    /// it (a positive magnitude or NULL; the schema cannot hold the
    /// declared zero, which lives on the interval alone). Persistence
    /// failures are contained: the in-memory facet still moves and the
    /// row re-lands at the next declaration or the stop.
    pub(super) async fn set_skill_boost(
        &mut self,
        percent: Option<i64>,
    ) -> Result<(), TrackerCommandError> {
        let percent = percent.filter(|value| *value >= 0);
        let session_id = {
            let Some(active) = self.session.active_mut() else {
                return Err(TrackerCommandError::NoActiveSession);
            };
            active.facets.skill_boost_percent = percent.filter(|value| *value > 0);
            active.dirty = true;
            active.session.id.clone()
        };
        let row_mirror = percent.filter(|value| *value > 0);
        {
            let session_id = session_id.clone();
            let _ = self
                .db
                .with_writer(move |conn| {
                    conn.execute(
                        "UPDATE tracking_sessions SET skill_boost_percent = ? WHERE id = ?",
                        rusqlite::params![row_mirror, session_id],
                    )?;
                    Ok(())
                })
                .await;
        }
        let now = instant_to_epoch(resolve_local(self.clock.now()));

        // Contained like the other persistence failures: a interval write
        // that cannot land leaves the declaration unrecorded rather than
        // stamping events with a context that does not describe them.
        {
            let db = self.db.clone();
            let Some(active) = self.session.active_mut() else {
                return Err(TrackerCommandError::NoActiveSession);
            };
            let outcome = match percent {
                Some(value) => active
                    .intervals
                    .open_interval(
                        &db,
                        &session_id,
                        now,
                        IntervalSpec::new(IntervalKind::Modifier).magnitude(Some(value as f64)),
                    )
                    .await
                    .map(|_| ()),
                None => active
                    .intervals
                    .close_kind(&db, &session_id, now, IntervalKind::Modifier)
                    .await
                    .map(|_| ()),
            };
            if outcome.is_err() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Declare an activity: open its stretch on the running session.
    ///
    /// The default is the one-tap switch, exclusive across BOTH activity
    /// kinds: declaring the next boss seals the standing quest stretch
    /// and any standing segment in the same motion, because one control
    /// offers them and a tap means "this is what I am doing now". The
    /// interval primitive still supports overlap; only the gesture
    /// insists on one at a time.
    ///
    /// `additive` is the deliberate co-activation, and there each kind
    /// keeps its own standing rule: quests stack (a hunt genuinely
    /// advancing two dailies records inside both, which the context
    /// expresses natively and a per-axis column could not), while a
    /// segment still seals the previous segment, because a player-drawn
    /// slice is a sequential cut of the run rather than a state.
    ///
    /// An already-standing activity never grows a duplicate stretch: the
    /// additive re-declaration is a no-op, and the exclusive one seals
    /// only the others (closing and reopening the target would split one
    /// continuous stretch into two).
    ///
    /// A segment carries the name the player gave it; there is no
    /// unnamed boundary, because an auto-numbered slice names nothing
    /// and a slice worth recording is worth saying what it is.
    ///
    /// Contained like the other interval writes: a write that cannot
    /// land leaves the declaration unrecorded, and the returned set
    /// echoes the state actually in force.
    pub(super) async fn activate_activity(
        &mut self,
        activity: ActivityRef,
        additive: bool,
    ) -> Result<Vec<ActiveActivity>, TrackerCommandError> {
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let db = self.db.clone();
        let Some(active) = self.session.active_mut() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        let session_id = active.session.id.clone();

        // One match over the declaration, so what identifies the target
        // and what the interval records cannot drift apart.
        let (kind, ref_id, label, standing) = match &activity {
            ActivityRef::Quest { quest_id, name } => (
                IntervalKind::Quest,
                Some(*quest_id),
                name.clone(),
                active.intervals.open_of_ref(IntervalKind::Quest, *quest_id),
            ),
            ActivityRef::Segment { name } => {
                let label = name.trim().to_string();
                let standing = active
                    .intervals
                    .open_of_label(IntervalKind::Segment, &label);
                (IntervalKind::Segment, None, label, standing)
            }
        };
        let standing = standing.map(|interval| interval.id);

        if let Some(keep_id) = standing {
            if !additive {
                let _ = active
                    .intervals
                    .close_kinds_except_id(&db, &session_id, now, &ACTIVITY_KINDS, keep_id)
                    .await;
            }
            return Ok(active_activities(active));
        }

        let spec = IntervalSpec::new(kind).label(Some(label)).ref_id(ref_id);
        // Co-activation keeps each kind's own standing rule; the switch
        // seals every activity, whichever kind it is.
        let spec = if additive {
            match kind {
                IntervalKind::Quest => spec.stacking(),
                _ => spec,
            }
        } else {
            spec.closes(CloseScope::Kinds(ACTIVITY_KINDS.to_vec()))
        };
        let _ = active
            .intervals
            .open_interval(&db, &session_id, now, spec)
            .await;
        Ok(active_activities(active))
    }

    /// End one standing activity (the user's toggle-off, or a quest's
    /// completion closing its stretch), leaving the others running.
    /// Idempotent: ending one that is not standing is a no-op, so a
    /// stale control cannot fail the user. Returns the set still in
    /// force.
    pub(super) async fn deactivate_activity(
        &mut self,
        target: ActivityKey,
    ) -> Result<Vec<ActiveActivity>, TrackerCommandError> {
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let db = self.db.clone();
        let Some(active) = self.session.active_mut() else {
            return Err(TrackerCommandError::NoActiveSession);
        };
        let session_id = active.session.id.clone();
        match target {
            ActivityKey::Quest(quest_id) => {
                let _ = active
                    .intervals
                    .close_ref(&db, &session_id, now, IntervalKind::Quest, quest_id)
                    .await;
            }
            ActivityKey::Segment(name) => {
                if let Some(id) = active
                    .intervals
                    .open_of_label(IntervalKind::Segment, &name)
                    .map(|interval| interval.id)
                {
                    let _ = active
                        .intervals
                        .close_ids(&db, &session_id, now, &[id])
                        .await;
                }
            }
        }
        Ok(active_activities(active))
    }

    /// Refresh the config-derived tracker state after a settings change:
    /// the carried weapons (a regime in progress carries on over the new
    /// set), the harvest guardrail, and the loot filter. The providers may
    /// read the database; the actor simply runs them inline (nothing else
    /// can touch its state meanwhile).
    pub(super) fn reload_config(&mut self) {
        self.harvest_guardrail = self.providers.equipment.resolve_harvest_guardrail();
        self.refresh_loot_filter();
        let Self {
            session, providers, ..
        } = self;
        let Some(active) = session.active_mut() else {
            return;
        };
        active
            .weapons
            .load_carried(providers.equipment.carried_weapons());

        // Sync the declared mob with the live config (the declare and
        // release commands write the config first, so this also covers
        // a settings-page edit landing mid-session).
        active.declared_mob = providers
            .config
            .manual_mob()
            .map(|(species, maturity)| DeclaredMob::from_parts(species, maturity));
    }

    /// Start a new tracking session; any prior session stops first, so
    /// its own stop events publish cleanly before the start events.
    pub(super) async fn start_session(&mut self) -> Result<TrackingSession, DbError> {
        if self.session.active().is_some() {
            self.stop_session().await?;
        }

        // Snapshot the session facets from the live config: the name
        // (trimmed; empty is "not declared") and the skill boost (zero
        // or negative is "no boost", stored as NULL). Both are captured
        // here and never re-read from the config mid-session.
        // The declaration is three-state; the session ROW's scalar is not
        // (migration 0019 constrains it to `> 0 OR NULL`, which is why
        // the interval model superseded it as the source of truth). The
        // row therefore keeps the magnitude only, while the declaration
        // itself rides the opening interval below, where `Some(0)`
        // survives as the baseline it is.
        let declared_boost = self
            .providers
            .config
            .declared_skill_boost_percent()
            .filter(|percent| *percent >= 0);
        // The definition resolves against the database at the stamping
        // moment: a selection deleted after being picked falls through
        // to the protected default rather than stamping a dead id, and
        // an install that never picked one starts under that default
        // too, so every session is an instance of something.
        // The two facets are coupled (a selection writes both), so they
        // are read together BEFORE the resolving await: reading the name
        // afterwards could pair one selection's id with another's name.
        let configured_selection = self.providers.config.session_definition_id();
        // The configured name is the user's own declaration and wins;
        // only when there is none does the resolved definition name the
        // session, which is what keeps a never-touched install from
        // recording nameless history.
        let configured_name = self
            .providers
            .config
            .session_name()
            .trim_matches(python_whitespace)
            .to_string();
        let resolved =
            crate::session_definitions::resolve_selection(&self.db, configured_selection).await?;
        let track_protection_costs = resolved
            .as_ref()
            .is_none_or(|(_, _, track_costs)| *track_costs);
        let facets = SessionFacets {
            name: Some(configured_name)
                .filter(|name| !name.is_empty())
                .or_else(|| resolved.as_ref().map(|(_, name, _)| name.clone())),
            definition_id: resolved.as_ref().map(|(id, _, _)| *id),
            track_protection_costs,
            skill_boost_percent: declared_boost.filter(|percent| *percent > 0),
        };
        let session_id = uuid::Uuid::new_v4().to_string();
        let carried = self.providers.equipment.carried_weapons();
        self.harvest_guardrail = self.providers.equipment.resolve_harvest_guardrail();

        self.refresh_loot_filter();
        let session = TrackingSession {
            id: session_id.clone(),
            // The one wall-clock read of the start path, resolved to
            // its instant at the boundary.
            start_time: resolve_local(self.clock.now()),
            end_time: None,
            kills: Vec::new(),
            harvests: Vec::new(),
            dangling_cost: Ped::ZERO,
        };
        let start_ts = instant_to_epoch(session.start_time);

        // Persist session start BEFORE activating in memory: a failed
        // insert leaves the tracker idle rather than a phantom session
        // with no row for its kills. The facets snapshot onto the row
        // at start; the name then never moves for the session's life,
        // while a boost re-declaration re-lands on the row so it always
        // reads as the latest declaration. (`mob_tracking_mode`
        // keeps its column default: the mode vocabulary is legacy and
        // records nothing about a facet-era session.)
        let insert_id = session_id.clone();
        let insert_name = facets.name.clone();
        let insert_definition = facets.definition_id;
        let insert_boost = facets.skill_boost_percent;
        let insert_track_protection_costs = facets.track_protection_costs;
        let opening_boost = declared_boost;
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO tracking_sessions \
                     (id, started_at, is_active, session_name, definition_id, \
                      skill_boost_percent, track_protection_costs, track_protection_by_segment) \
                     VALUES (?, ?, 1, ?, ?, ?, ?, 0)",
                    // The per-segment armour flag is retired: protection
                    // costs are spread by hits at recording time, so every
                    // new session records 0 for it.
                    rusqlite::params![
                        insert_id,
                        start_ts,
                        insert_name,
                        insert_definition,
                        insert_boost,
                        insert_track_protection_costs as i64,
                    ],
                )?;
                Ok(())
            })
            .await?;

        // The fresh ActiveSession IS the session reset: every
        // session-scoped field starts at its documented initial
        // state by construction.
        let mut active = ActiveSession::new(session.clone(), facets);
        active.hunting_looters = self.providers.equipment.hunting_looter_levels();
        active.weapons.load_carried(carried);

        // Seed the declared mob from the configured declaration, when
        // one is set (the same seeding the declare command performs).
        if let Some((species, maturity)) = self.providers.config.manual_mob() {
            active.declared_mob = Some(DeclaredMob::from_parts(species, maturity));
        }

        self.session = SessionState::Active(Box::new(active));
        // A heal-over-time effect paid for before this session (in the
        // previous session, or before a restart) keeps running in the game:
        // its window carries over by its absolute expiry.
        self.restore_persisted_healing(start_ts).await;
        self.subscribe_handlers();
        self.publish_status();

        self.bus
            .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                session_id: session_id.clone(),
            }));
        // The session's opening context: the empty set, minted even when
        // nothing is declared, because an event stamped with it was
        // recorded under the interval model with nothing in force, and
        // that is a different fact from an event predating the model.
        // Deliberately NOT a bus event: the only cross-service consumer
        // reads the session's current context from the database at
        // insert, which is exact, and keeps the recorded event stream
        // free of internal plumbing.
        {
            let db = self.db.clone();
            if let Some(active) = self.session.active_mut() {
                if active
                    .intervals
                    .open_session(&db, &session_id, start_ts)
                    .await
                    .is_ok()
                {
                    if let Some(percent) = opening_boost {
                        let _ = active
                            .intervals
                            .open_interval(
                                &db,
                                &session_id,
                                start_ts,
                                IntervalSpec::new(IntervalKind::Modifier)
                                    .magnitude(Some(percent as f64)),
                            )
                            .await;
                    }
                }
            }
        }
        self.emit_session_event(
            TrackingReason::Started,
            TrackingStatus::Active,
            start_ts,
            Some(&session_id),
        );
        Ok(session)
    }

    /// Stop the active session: dangling cost, the handler
    /// unsubscribes and the end stamp; then persistence, ledger gains,
    /// summary, and the stop events; then the in-memory clear
    /// (dropping the whole `ActiveSession`).
    pub(super) async fn stop_session(&mut self) -> Result<Option<TrackingSession>, DbError> {
        let (
            session,
            session_id,
            end_time,
            heal_cost,
            dangling_cost,
            session_name,
            session_boost,
            dangling_evidence,
            attribution_counts,
        ) = {
            let Some(active) = self.session.active_mut() else {
                return Ok(None);
            };
            let dangling_cost = active.accumulator.total_cost();
            // Evidence of the shots after the last kill settles with the
            // session's dangling cost.
            let dangling_evidence = active.accumulator.evidence.clone();
            let attribution_counts = active.weapons.attribution.counts();
            active.session.end_time = Some(resolve_local(self.clock.now()));
            active.session.dangling_cost = dangling_cost;
            let snapshot = active.session.clone();
            let session_id = snapshot.id.clone();
            let end_time = snapshot.end_time.expect("just stamped");
            let heal_cost = active.heal_cost;
            let session_name = active.facets.name.clone();
            let session_boost = active.facets.skill_boost_percent;
            (
                snapshot,
                session_id,
                end_time,
                heal_cost,
                dangling_cost,
                session_name,
                session_boost,
                dangling_evidence,
                attribution_counts,
            )
        };
        // Close every still-open interval before the session record is
        // sealed, so no interval outlives the session that owns it and a
        // duration read never has to guess at a missing end.
        {
            let db = self.db.clone();
            let end_ts = instant_to_epoch(end_time);
            if let Some(active) = self.session.active_mut() {
                let _ = active.intervals.close_session(&db, end_ts).await;
            }
        }
        // One transaction over the whole stop sequence, matching the
        // original's single commit: a failure (or crash) mid-way leaves
        // no half-stopped session, no orphaned ledger gains, and no
        // summary computed from a partially persisted stop. The bus
        // forwarders stay subscribed until it commits, so a failed stop
        // leaves the session fully live (still tracking, still hearing
        // events) rather than active-but-deaf; no event can interleave
        // meanwhile because the actor processes one message at a time.
        let sid = session_id.clone();
        let end_epoch = instant_to_epoch(end_time);
        let heal_value = heal_cost.value();
        let dangling_value = dangling_cost.value();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                // Both facets re-land from memory at stop, so a contained
                // failure of a mid-session write cannot strand the summary
                // on a stale row.
                tx.execute(
                    "UPDATE tracking_sessions SET ended_at = ?, is_active = 0, \
                     heal_cost = ?, dangling_cost = ?, session_name = ?, \
                     skill_boost_percent = ?, weapon_shots_agreed = ?, \
                     weapon_shots_evidenced = ? WHERE id = ?",
                    rusqlite::params![
                        end_epoch,
                        heal_value,
                        dangling_value,
                        session_name,
                        session_boost,
                        attribution_counts.agreed,
                        attribution_counts.evidenced,
                        sid
                    ],
                )?;
                for row in &dangling_evidence {
                    row.insert(&tx, None)?;
                }
                // Enhancer-break Shrapnel is an immediate cost rebate. Ordinary
                // Shrapnel remains stock until the player explicitly converts it.
                Self::create_enhancer_rebate_ledger_entry(&tx, &sid, end_time)?;
                crate::session_summary::write_session_summary(&tx, &sid)?;
                crate::daily_rollup::refresh_session_days(&tx, &sid)?;
                crate::session_rollup::recompute_session(&tx, &sid)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.unsubscribe_handlers();

        // Session end is a quiescent boundary: checkpoint and truncate the WAL
        // so its growth over a tracked session is bounded. Best-effort: the
        // stop's data is already committed, and TRUNCATE can be briefly blocked
        // by an in-flight reader (it simply retries at the next session end), so
        // a failure here must not fail the stop. A failure is logged rather than
        // swallowed, so a persistently failing checkpoint (a stuck reader) leaves
        // a diagnostic trail instead of silently unbounded WAL growth.
        if let Err(error) = self.db.checkpoint_truncate().await {
            tracing::warn!(
                target: "eo::tracker",
                %session_id,
                %error,
                "WAL checkpoint at session end failed; log growth is not bounded this stop",
            );
        }

        self.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: session_id.clone(),
            }));
        // `end_time` was stamped from the injected clock above, so the
        // required `occurred_at` always carries the stop instant.
        self.emit_session_event(
            TrackingReason::Stopped,
            TrackingStatus::Idle,
            instant_to_epoch(end_time),
            Some(&session_id),
        );

        // Dropping the ActiveSession IS the in-memory clear: the
        // accumulator, weapon runtime, and mob selection cannot
        // survive the session because they live inside it. (The
        // equipped heal tool deliberately does.)
        self.session = SessionState::Idle;
        self.publish_status();
        Ok(Some(session))
    }

    /// Coalesce a settled tick's mutations into one domain event.
    /// Subscribed only while a session is active; fires only when the
    /// tick actually changed the live session readout, stamped with
    /// the tick's own timestamp (already on the tick's loot/combat
    /// events) or the injected clock when the tick carries none.
    pub(super) fn on_tick_flushed(&mut self, event: &BusEvent) {
        let BusEvent::TickFlushed(payload) = event else {
            return;
        };
        let session_id = {
            let Some(active) = self.session.active_mut() else {
                return;
            };
            if !active.dirty {
                return;
            }
            active.dirty = false;
            active.session.id.clone()
        };
        // The original's three-way stamp: a datetime-equivalent string
        // takes its instant, an epoch-float string goes through
        // `float()` (an unparseable value raises there, contained with
        // the dirty flag already consumed: no event), and an absent
        // timestamp falls back to the injected clock.
        let occurred_ts = match &payload.timestamp {
            None => instant_to_epoch(resolve_local(self.clock.now())),
            Some(text) => match super::time::parse_timestamp_instant(&self.chatlog_clock, text) {
                Some(instant) => instant_to_epoch(instant),
                None => match text.trim().parse::<f64>() {
                    Ok(numeric) => numeric,
                    Err(_) => return,
                },
            },
        };
        self.emit_session_event(
            TrackingReason::Updated,
            TrackingStatus::Active,
            occurred_ts,
            Some(&session_id),
        );
    }
}

impl HuntTracker {
    /// An owned, immutable view of the current tracking readout:
    /// `active` is None when idle. The in-memory aggregation runs on
    /// the actor; the two session-scoped reads (skill-gain total,
    /// notable-event feed) run here, keyed on the captured session id.
    pub async fn snapshot(&self) -> Result<TrackingReadout, DbError> {
        let (current_tool, current_tool_kind, aggregate) = self.aggregate().await;
        let Some(aggregated) = aggregate else {
            return Ok(TrackingReadout {
                current_tool,
                current_tool_kind,
                active: None,
            });
        };

        let skill_session_id = aggregated.session_id.clone();
        let skill_tt = self
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
                    rusqlite::params![skill_session_id],
                    |row| row.get::<_, f64>(0),
                )?)
            })
            .await?;

        // Latest-session notable-event feed (top 20). The live
        // session is the latest session, so this single read
        // serves the activity feed.
        let feed_session_id = aggregated.session_id.clone();
        let notable_rows: Vec<(String, String, f64, Option<f64>)> = self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT event_type, mob_or_item, value_ped, timestamp \
                     FROM notable_events WHERE session_id = ? \
                     ORDER BY timestamp DESC LIMIT 20",
                )?;
                let mapped = stmt.query_map(rusqlite::params![feed_session_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                        row.get::<_, Option<f64>>(3)?,
                    ))
                })?;
                Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await?;

        let round_opt =
            |value: Option<f64>, places: usize| value.map(|inner| round_half_even(inner, places));
        let active = ActiveSessionView {
            session_id: aggregated.session_id,
            started_at: aggregated.started_at,
            kill_count: aggregated.kill_count,
            elapsed: aggregated.elapsed,
            cost: aggregated.cost.round_half_even(2).value(),
            returns: aggregated.returns.round_half_even(2).value(),
            pes: round_half_even(skill_tt, 2),
            net: (aggregated.returns - aggregated.cost)
                .round_half_even(2)
                .value(),
            return_rate: if aggregated.cost.is_positive() {
                round_half_even(aggregated.returns / aggregated.cost, 4)
            } else {
                0.0
            },
            damage_dealt_total: round_half_even(aggregated.damage_total, 1),
            weapon_damage_dealt: round_half_even(aggregated.live_weapon_damage, 1),
            weapon_cost: aggregated.weapon_cost.round_half_even(6).value(),
            expected_tt_rate: aggregated.expected_tt_rate,
            expected_return_coverage: aggregated.expected_return_coverage,
            expected_return_model: aggregated.expected_return_model,
            shots_fired_total: aggregated.shots_total,
            critical_hits_total: aggregated.crits_total,
            max_damage: round_half_even(aggregated.max_damage, 1),
            globals_count: aggregated.globals_count,
            hofs_count: aggregated.hofs_count,
            latest_kill_loot: aggregated
                .latest_kill_loot
                .map(|loot| loot.round_half_even(2).value()),
            multiplier_last: round_opt(aggregated.multiplier_last, 4),
            multiplier_avg: round_opt(aggregated.multiplier_avg, 4),
            multiplier_max: round_opt(aggregated.multiplier_max, 4),
            multiplier_history: aggregated.multiplier_history,
            cumulative_net_history: aggregated.cumulative_net,
            current_mob: aggregated.mob_name.clone(),
            session_name: aggregated.session_name.clone(),
            definition_id: aggregated.definition_id,
            track_protection_costs: aggregated.track_protection_costs,
            skill_boost_percent: aggregated.skill_boost_percent,
            active_activities: aggregated.active_activities.clone(),
            harvest_swings: aggregated.harvest_swings,
            harvest_successes: aggregated.harvest_successes,
            // + 0.0 normalises the sign: an empty f64 sum is -0.0 (the
            // std identity), which the Python-repr writer would render
            // as "-0.0".
            harvest_loot: aggregated.harvest_loot.round_half_even(4).value() + 0.0,
            harvest_cost: aggregated.harvest_cost.round_half_even(4).value() + 0.0,
            harvest_guardrail_mismatch: aggregated.guardrail_mismatch.map(|mismatch| {
                HarvestGuardrailMismatchView {
                    expected_tool: mismatch.expected_tool,
                    observed_tool: mismatch.observed_tool,
                    tree_size: mismatch.tree_size.as_str().to_string(),
                    at_epoch: mismatch.at_epoch,
                }
            }),
            weapon_guardrail_mismatch: aggregated.weapon_mismatch.map(|mismatch| {
                WeaponGuardrailMismatchView {
                    hotbar_tool: mismatch.declared,
                    recording_tool: mismatch.evidence,
                    since: mismatch.since,
                    shots: mismatch.shots,
                }
            }),
            unpriced_shots: aggregated.unpriced_shots,
            notable_event_rows: notable_rows,
            warnings: aggregated.warnings,
            healing: aggregated.healing,
        };
        Ok(TrackingReadout {
            current_tool,
            current_tool_kind,
            active: Some(active),
        })
    }
}
