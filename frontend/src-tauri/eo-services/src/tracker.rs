//! The hunt tracker, ported from `backend/tracking/tracker.py`: the
//! central coordinator that subscribes to the bus, accumulates combat
//! stats, creates kill records on loot events, and persists to the
//! database.
//!
//! The kills model: shots accumulate with cost; a loot group is a
//! kill (snapshot the accumulator, stamp the configured mob or tag,
//! persist, reset); deaths are invisible; a session ending with
//! unresolved shots carries them as dangling cost.
//!
//! Concurrency shape: one mutex owns the in-memory session state, and
//! the original's documented invariants hold structurally here. Bus
//! publishes run only after the guard drops; the session-persistence
//! writes run after the guard drops (bridged onto the async pool
//! through a runtime handle, preserving the original's lock order:
//! the tracker lock is never held across SQLite for the tracker's own
//! writes); the provider callbacks reached from handlers may read the
//! database while the guard is held, exactly as the original's lock
//! order allows. The original's re-entrant lock is unnecessary once
//! the stop-before-lock shape is kept, which the borrow checker now
//! enforces rather than documents.
//!
//! Representation differences, all observation-equivalent: the
//! original's `_last_kill` alias of `session.kills[-1]` is the
//! `last_mut()` of the kills list (the alias and the tail are the
//! same object there, established by the loot handler and cleared
//! with the session); phase-keyed tool stats live in an ordered
//! vector rather than an insertion-ordered dict; the original's
//! logging, debug-only performance counters and development-build
//! priming hook are omitted, as is its `enhancer_tt_lookup` provider
//! (stored but never read there).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::NaiveDateTime;
use eo_wire::domain_events::{
    DomainEvent, TrackingReason, TrackingSessionUpdated, TrackingSessionUpdatedPayload,
    TrackingSessionUpdatedTag, TrackingStatus,
};
use eo_wire::normalizer::round_half_even;
use serde_json::{Map, Value};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use tokio::runtime::Handle;

use crate::clock::Clock;
use crate::cost_engine::cost_per_shot_from_props;
use crate::db::{decoded_f64, DbError};
use crate::event_bus::{EventBus, Registration, Topic};
use crate::loot_filter::{is_tracked_loot, normalize_blacklist};
use crate::mob_lookup_service::python_whitespace;
use crate::session_summary::write_session_summary;
use crate::tool_inference::DamageAttributor;
use crate::tracking_models::{
    ActiveSessionView, Kill, LootItem, ToolStats, TrackingReadout, TrackingSession,
};

/// Loot groups with an identical fingerprint within this window are
/// duplicates.
pub const LOOT_DEDUP_WINDOW_SECONDS: f64 = 2.0;

/// Tagging a global/HoF onto the latest kill requires the kill to be
/// at most this many seconds away.
const GLOBAL_CORRELATION_WINDOW_SECONDS: f64 = 5.0;

/// An equipment profile from the library lookup, when the tool is
/// known.
pub type EquipmentProfile = Option<Map<String, Value>>;

/// The provider callbacks the composition root wires in; every field
/// defaults to the original's inert fallback. The lookups may read
/// the database (the lock order allows a provider read under the
/// tracker lock); the resolver is invoked outside the lock.
pub struct Providers {
    pub equipment_cost_lookup: Arc<dyn Fn(&str) -> f64 + Send + Sync>,
    pub equipment_profile_lookup: Arc<dyn Fn(&str) -> EquipmentProfile + Send + Sync>,
    pub player_name: String,
    pub loot_filter_blacklist: Vec<String>,
    pub loot_filter_blacklist_provider: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    pub weapon_attribution_trifecta: Arc<dyn Fn() -> bool + Send + Sync>,
    pub mob_tracking_mode: Arc<dyn Fn() -> String + Send + Sync>,
    pub mob_tracking_tag: Arc<dyn Fn() -> String + Send + Sync>,
    pub manual_mob_entry_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub manual_mob: Arc<dyn Fn() -> Option<(String, String)> + Send + Sync>,
    pub trifecta_resolver: Arc<dyn Fn() -> Option<Map<String, Value>> + Send + Sync>,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            equipment_cost_lookup: Arc::new(|_| 0.0),
            equipment_profile_lookup: Arc::new(|_| None),
            player_name: String::new(),
            loot_filter_blacklist: Vec::new(),
            loot_filter_blacklist_provider: None,
            weapon_attribution_trifecta: Arc::new(|| false),
            mob_tracking_mode: Arc::new(|| "mob".to_string()),
            mob_tracking_tag: Arc::new(String::new),
            manual_mob_entry_enabled: Arc::new(|| true),
            manual_mob: Arc::new(|| None),
            trifecta_resolver: Arc::new(|| None),
        }
    }
}

/// The mob/tag command preconditions the original raises as
/// `RuntimeError`/`ValueError`; the messages match verbatim so the
/// HTTP layer surfaces identical text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerCommandError {
    NoActiveSession,
    NotTagMode,
    EmptyTag,
    TagModeLocksMob,
    ManualEntryDisabled,
}

impl std::fmt::Display for TrackerCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            TrackerCommandError::NoActiveSession => "No active session",
            TrackerCommandError::NotTagMode => "Active session is not in tag mode",
            TrackerCommandError::EmptyTag => "Tag cannot be empty",
            TrackerCommandError::TagModeLocksMob => {
                "Tag mode sessions do not allow manual mob locking"
            }
            TrackerCommandError::ManualEntryDisabled => {
                "Manual mob entry is not enabled for this session"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for TrackerCommandError {}

/// Combat stats since the last kill (or session start).
#[derive(Default)]
struct Accumulator {
    shots_fired: i64,
    damage_dealt: f64,
    damage_taken: f64,
    critical_hits: i64,
    enhancer_cost: f64,
    /// Keyed by phase key (the bare tool name, then `name#2`...), in
    /// first-seen order.
    tool_stats: Vec<(String, ToolStats)>,
}

impl Accumulator {
    fn reset(&mut self) {
        *self = Accumulator::default();
    }

    fn weapon_cost(&self) -> f64 {
        self.tool_stats
            .iter()
            .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired as f64)
            .sum()
    }

    fn total_cost(&self) -> f64 {
        self.weapon_cost() + self.enhancer_cost
    }
}

/// Per-weapon damage-enhancer state within the current session.
struct DamageEnhancerState {
    tool_name: String,
    props: Arc<Value>,
    stacks: Vec<i64>,
    cached_cost_ped: Option<f64>,
}

impl DamageEnhancerState {
    fn from_props(tool_name: &str, props: Arc<Value>) -> Self {
        // `max(0, int(props.get("damage_enhancers", 0) or 0))`.
        let configured = props
            .get("damage_enhancers")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;
        let configured = configured.max(0);
        Self {
            tool_name: tool_name.to_string(),
            props,
            stacks: vec![100; configured as usize],
            cached_cost_ped: None,
        }
    }

    fn active_slots(&self) -> i64 {
        self.stacks.iter().filter(|stack| **stack > 0).count() as i64
    }

    /// Redistribute a known total across the slots, front-loading the
    /// remainder.
    fn set_total(&mut self, total: i64) {
        let total = total.max(0);
        let slot_count = self.stacks.len() as i64;
        if slot_count == 0 {
            return;
        }
        let per_slot = total / slot_count;
        let remainder = total % slot_count;
        self.stacks = (0..slot_count)
            .map(|index| per_slot + i64::from(index < remainder))
            .collect();
        self.cached_cost_ped = None;
    }

    /// Apply one break; true when a slot fully depleted.
    fn apply_break(&mut self, remaining: Option<i64>) -> bool {
        let old_active = self.active_slots();
        match remaining {
            Some(total) if !self.stacks.is_empty() => self.set_total(total),
            _ => {
                for index in (0..self.stacks.len()).rev() {
                    if self.stacks[index] > 0 {
                        self.stacks[index] -= 1;
                        self.cached_cost_ped = None;
                        break;
                    }
                }
            }
        }
        old_active != self.active_slots()
    }

    fn current_cost_ped(&mut self) -> f64 {
        if self.cached_cost_ped.is_none() {
            let result = cost_per_shot_from_props(&self.props, Some(self.active_slots()));
            let total = result
                .get("totalCostPerUse")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            self.cached_cost_ped = Some(total / 100.0);
        }
        self.cached_cost_ped.expect("just cached")
    }
}

/// The in-memory state the tracker's one mutex owns.
#[derive(Default)]
struct TrackerState {
    session: Option<TrackingSession>,
    accumulator: Option<Accumulator>,
    session_dirty: bool,
    session_heal_cost: f64,
    heal_warning_emitted: bool,
    session_warnings: Vec<String>,
    loot_blacklist: BTreeSet<String>,
    current_mob_name: String,
    current_mob_species: String,
    current_mob_maturity: String,
    confirmed_mob_name: String,
    confirmed_mob_species: String,
    confirmed_mob_maturity: String,
    mob_source: Option<&'static str>,
    session_mob_tracking_mode: String,
    session_mob_tracking_tag: String,
    last_heal_time: Option<NaiveDateTime>,
    last_loot_fingerprint: Option<(f64, usize, String)>,
    last_loot_time: Option<NaiveDateTime>,
    trifecta_unmatched_warning_emitted: bool,
    active_hotbar_tool_name: Option<String>,
    active_heal_tool_name: Option<String>,
    heal_cost_per_use_ped: f64,
    heal_reload_seconds: f64,
    heal_amount_min: Option<f64>,
    heal_amount_max: Option<f64>,
    trifecta_weapon_profiles: BTreeMap<String, Arc<Value>>,
    weapon_enhancer_states: BTreeMap<String, DamageEnhancerState>,
    active_weapon_state_key: Option<String>,
    active_weapon_observed_name: Option<String>,
    last_offensive_tool_name: Option<String>,
    damage_attributor: DamageAttributor,
    profile_match_cache: BTreeMap<String, Option<(String, Arc<Value>)>>,
    static_tool_cost_cache: BTreeMap<String, f64>,
}

pub struct HuntTracker {
    bus: Arc<EventBus>,
    pool: SqlitePool,
    runtime: Handle,
    clock: Arc<dyn Clock>,
    providers: Providers,
    state: Mutex<TrackerState>,
    subscriptions: Mutex<Vec<(Topic, Registration)>>,
    subscribed: AtomicBool,
}

impl HuntTracker {
    /// Build the tracker over an already-migrated pool. Recovery of
    /// crash-orphaned sessions runs here, as the original's
    /// constructor does. The handler closures hold the tracker
    /// strongly while subscribed (released on session stop), so the
    /// composition root keeps one `Arc` for the process lifetime.
    pub fn new(
        bus: Arc<EventBus>,
        pool: SqlitePool,
        runtime: Handle,
        clock: Arc<dyn Clock>,
        mut providers: Providers,
    ) -> Result<Arc<Self>, DbError> {
        providers.player_name = providers
            .player_name
            .trim_matches(python_whitespace)
            .to_string();
        let state = TrackerState {
            heal_reload_seconds: 2.5,
            session_mob_tracking_mode: "mob".to_string(),
            loot_blacklist: normalize_blacklist(Some(
                providers.loot_filter_blacklist.iter().map(String::as_str),
            )),
            ..TrackerState::default()
        };

        let tracker = Arc::new(Self {
            bus,
            pool,
            runtime,
            clock,
            providers,
            state: Mutex::new(state),
            subscriptions: Mutex::new(Vec::new()),
            subscribed: AtomicBool::new(false),
        });
        tracker.refresh_loot_filter_locked(&mut tracker.lock_state());
        tracker.recover_orphaned_sessions()?;
        Ok(tracker)
    }

    /// Bridge a database future onto the runtime from either calling
    /// context: a runtime worker thread (the web layer) yields its
    /// slot via `block_in_place`, while a plain producer thread (the
    /// chat-log tail, the hotbar listener) parks directly.
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        if Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.runtime.block_on(future))
        } else {
            self.runtime.block_on(future)
        }
    }

    /// The state guard, tolerating poison: a panicking provider or
    /// cost computation must not brick the tracker, mirroring the
    /// original's per-event exception containment (its state stays
    /// serviceable after a contained failure).
    fn lock_state(&self) -> std::sync::MutexGuard<'_, TrackerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_tracking(&self) -> bool {
        self.lock_state().session.is_some()
    }

    /// Close sessions left open by a crash: end at the latest kill
    /// (or the start), write the ledger gains and the summary, and
    /// clear the active flag.
    fn recover_orphaned_sessions(&self) -> Result<(), DbError> {
        self.block_on(async {
            let rows =
                sqlx::query("SELECT id, started_at FROM tracking_sessions WHERE is_active = 1")
                    .fetch_all(&self.pool)
                    .await?;
            for row in rows {
                let session_id: String = row.try_get(0)?;
                let started_at: f64 = row.try_get(1)?;
                let kill_row = sqlx::query("SELECT MAX(timestamp) FROM kills WHERE session_id = ?")
                    .bind(&session_id)
                    .fetch_one(&self.pool)
                    .await?;
                // The original's falsy fallback, not a None check: a
                // zero maximum also falls back to the session start.
                let ended_at = match kill_row.try_get::<Option<f64>, _>(0)? {
                    Some(latest) if latest != 0.0 => latest,
                    _ => started_at,
                };
                sqlx::query(
                    "UPDATE tracking_sessions SET ended_at = ?, is_active = 0 WHERE id = ?",
                )
                .bind(ended_at)
                .bind(&session_id)
                .execute(&self.pool)
                .await?;

                let end_dt = epoch_to_naive(ended_at);
                self.create_enhancer_rebate_ledger_entry(&session_id, end_dt)
                    .await?;
                self.create_shrapnel_ledger_entry(&session_id, end_dt)
                    .await?;
                write_session_summary(&self.pool, &session_id).await?;
            }
            Ok(())
        })
    }

    /// An owned, immutable view of the current tracking readout:
    /// `active` is None when idle. The in-memory aggregation runs
    /// under the state guard; the two session-scoped reads (skill-gain
    /// total, notable-event feed) run after release, keyed on the
    /// captured session id.
    pub fn snapshot(&self) -> Result<TrackingReadout, DbError> {
        struct Aggregated {
            session_id: String,
            started_at: String,
            start_ts: f64,
            kill_count: i64,
            cost: f64,
            returns: f64,
            damage_total: f64,
            shots_total: i64,
            crits_total: i64,
            max_damage: f64,
            live_weapon_damage: f64,
            weapon_cost: f64,
            globals_count: i64,
            hofs_count: i64,
            latest_kill_loot: Option<f64>,
            multiplier_last: Option<f64>,
            multiplier_avg: Option<f64>,
            multiplier_max: Option<f64>,
            multiplier_history: Vec<f64>,
            cumulative_net: Vec<f64>,
            confirmed_mob_name: String,
            mob_source: Option<&'static str>,
            mob_entry_mode: String,
            warnings: Vec<String>,
        }

        let (current_tool, aggregated) = {
            let state = self.lock_state();
            let current_tool = state.active_hotbar_tool_name.clone();
            let Some(session) = state.session.as_ref() else {
                return Ok(TrackingReadout {
                    current_tool,
                    active: None,
                });
            };

            let kills = &session.kills;
            let mut weapon_cost: f64 = kills
                .iter()
                .flat_map(|kill| kill.tool_stats.iter())
                .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired as f64)
                .sum();
            let mut enhancer_cost: f64 = kills.iter().map(|kill| kill.enhancer_cost).sum();
            if let Some(accumulator) = state.accumulator.as_ref() {
                weapon_cost += accumulator.weapon_cost();
                enhancer_cost += accumulator.enhancer_cost;
            }
            let heal_cost = state.session_heal_cost;
            let cost = weapon_cost + heal_cost + enhancer_cost;
            let returns: f64 = kills.iter().map(|kill| kill.loot_total_ped).sum();

            let damage_total: f64 = kills.iter().map(|kill| kill.damage_dealt).sum();
            let live_weapon_damage = damage_total
                + state
                    .accumulator
                    .as_ref()
                    .map(|accumulator| accumulator.damage_dealt)
                    .unwrap_or(0.0);

            // Multipliers use kill.cost_ped (weapon cost only) per EU
            // convention.
            let mult_per_kill: Vec<f64> = kills
                .iter()
                .filter(|kill| kill.cost_ped > 0.0)
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
                .filter(|kill| kill.cost_ped > 0.0)
                .map(|kill| kill.loot_total_ped / kill.cost_ped);
            let multiplier_history: Vec<f64> = mult_per_kill
                .iter()
                .rev()
                .take(120)
                .rev()
                .map(|value| round_half_even(*value, 4))
                .collect();

            // Cumulative-net history (per kill), distributing the
            // session-level heal cost pro-rata across kills by their
            // weapon-cost share so the curve's final point reconciles
            // with the displayed Net stat (returns - cost).
            let per_kill_weapon: Vec<f64> = kills
                .iter()
                .map(|kill| {
                    kill.tool_stats
                        .iter()
                        .map(|(_, stats)| stats.cost_per_shot * stats.shots_fired as f64)
                        .sum()
                })
                .collect();
            let total_weapon: f64 = per_kill_weapon.iter().sum();
            let mut cumulative_net = Vec::new();
            let mut running = 0.0;
            for (kill, weapon) in kills.iter().zip(per_kill_weapon.iter()) {
                let heal_share = if total_weapon > 0.0 {
                    heal_cost * (weapon / total_weapon)
                } else {
                    0.0
                };
                running += kill.loot_total_ped - weapon - kill.enhancer_cost - heal_share;
                cumulative_net.push(round_half_even(running, 2));
            }
            let cumulative_net: Vec<f64> = cumulative_net
                .iter()
                .rev()
                .take(120)
                .rev()
                .copied()
                .collect();

            let aggregated = Aggregated {
                session_id: session.id.clone(),
                started_at: naive_isoformat(session.start_time),
                start_ts: naive_to_epoch(session.start_time),
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
                globals_count: kills.iter().filter(|kill| kill.is_global).count() as i64,
                hofs_count: kills.iter().filter(|kill| kill.is_hof).count() as i64,
                latest_kill_loot: kills.last().map(|kill| kill.loot_total_ped),
                multiplier_last,
                multiplier_avg,
                multiplier_max,
                multiplier_history,
                cumulative_net,
                confirmed_mob_name: state.confirmed_mob_name.clone(),
                mob_source: state.mob_source,
                mob_entry_mode: state.session_mob_tracking_mode.clone(),
                warnings: state.session_warnings.clone(),
            };
            (current_tool, aggregated)
        };

        let (skill_tt, notable_rows) = self.block_on(async {
            let skill_row = sqlx::query(
                "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
            )
            .bind(&aggregated.session_id)
            .fetch_one(&self.pool)
            .await?;
            let skill_tt = decoded_f64(&skill_row, 0);

            // Latest-session notable-event feed (top 20). The live
            // session is the latest session, so this single read
            // serves the activity feed.
            let rows = sqlx::query(
                "SELECT event_type, mob_or_item, value_ped, timestamp \
                 FROM notable_events WHERE session_id = ? \
                 ORDER BY timestamp DESC LIMIT 20",
            )
            .bind(&aggregated.session_id)
            .fetch_all(&self.pool)
            .await?;
            let mut notable_rows = Vec::new();
            for row in rows {
                notable_rows.push((
                    row.try_get::<String, _>(0)?,
                    row.try_get::<String, _>(1)?,
                    decoded_f64(&row, 2),
                    row.try_get::<Option<f64>, _>(3)?,
                ));
            }
            Ok::<_, DbError>((skill_tt, notable_rows))
        })?;

        let round_opt =
            |value: Option<f64>, places: usize| value.map(|inner| round_half_even(inner, places));
        let active = ActiveSessionView {
            session_id: aggregated.session_id,
            started_at: aggregated.started_at,
            kill_count: aggregated.kill_count,
            elapsed: (naive_to_epoch(self.clock.now()) - aggregated.start_ts) as i64,
            cost: round_half_even(aggregated.cost, 2),
            returns: round_half_even(aggregated.returns, 2),
            pes: round_half_even(skill_tt, 2),
            net: round_half_even(aggregated.returns - aggregated.cost, 2),
            return_rate: if aggregated.cost > 0.0 {
                round_half_even(aggregated.returns / aggregated.cost, 4)
            } else {
                0.0
            },
            damage_dealt_total: round_half_even(aggregated.damage_total, 1),
            weapon_damage_dealt: round_half_even(aggregated.live_weapon_damage, 1),
            weapon_cost: round_half_even(aggregated.weapon_cost, 6),
            shots_fired_total: aggregated.shots_total,
            critical_hits_total: aggregated.crits_total,
            max_damage: round_half_even(aggregated.max_damage, 1),
            globals_count: aggregated.globals_count,
            hofs_count: aggregated.hofs_count,
            latest_kill_loot: round_opt(aggregated.latest_kill_loot, 2),
            multiplier_last: round_opt(aggregated.multiplier_last, 4),
            multiplier_avg: round_opt(aggregated.multiplier_avg, 4),
            multiplier_max: round_opt(aggregated.multiplier_max, 4),
            multiplier_history: aggregated.multiplier_history,
            cumulative_net_history: aggregated.cumulative_net,
            current_mob: if aggregated.confirmed_mob_name.is_empty() {
                None
            } else {
                Some(aggregated.confirmed_mob_name.clone())
            },
            mob_source: if aggregated.confirmed_mob_name.is_empty() {
                None
            } else {
                aggregated.mob_source.map(str::to_string)
            },
            mob_entry_mode: aggregated.mob_entry_mode,
            notable_event_rows: notable_rows,
            warnings: aggregated.warnings,
        };
        Ok(TrackingReadout {
            current_tool,
            active: Some(active),
        })
    }

    /// Refresh trifecta-attribution state after config changes. The
    /// trifecta is resolved (a DB read) before the lock so only the
    /// in-memory load runs under it; there is no DB write or publish.
    pub fn reload_config(&self) {
        let trifecta_mode = (self.providers.weapon_attribution_trifecta)();
        let trifecta = if trifecta_mode {
            (self.providers.trifecta_resolver)()
        } else {
            None
        };
        let mut state = self.lock_state();
        self.refresh_loot_filter_locked(&mut state);
        if state.session.is_none() {
            return;
        }
        if trifecta_mode {
            Self::load_trifecta_weapon_profiles(&mut state, trifecta.as_ref());
        } else {
            state.damage_attributor.clear();
            state.active_heal_tool_name = None;
            state.heal_cost_per_use_ped = 0.0;
            state.heal_reload_seconds = 2.5;
            state.heal_amount_min = None;
            state.heal_amount_max = None;
            state.heal_warning_emitted = false;
            Self::reset_weapon_runtime_state(&mut state);
        }

        if state.session_mob_tracking_mode == "tag" {
            return;
        }

        if (self.providers.manual_mob_entry_enabled)() {
            let Some((species, maturity)) = (self.providers.manual_mob)() else {
                if state.mob_source == Some("manual") {
                    Self::clear_mob_state(&mut state);
                }
                return;
            };
            let display = if maturity.is_empty() {
                species.clone()
            } else {
                format!("{maturity} {species}")
            };
            Self::set_manual_mob_state(&mut state, &display, &species, &maturity);
            return;
        }

        if state.mob_source == Some("manual") {
            Self::clear_mob_state(&mut state);
        }
    }

    /// Immediately set the active free-text tag for tag-mode kill
    /// stamping.
    pub fn set_manual_tag(&self, tag: &str) -> Result<(), TrackerCommandError> {
        let mut state = self.lock_state();
        if state.session.is_none() {
            return Err(TrackerCommandError::NoActiveSession);
        }
        if state.session_mob_tracking_mode != "tag" {
            return Err(TrackerCommandError::NotTagMode);
        }
        let cleaned = tag.trim_matches(python_whitespace);
        if cleaned.is_empty() {
            return Err(TrackerCommandError::EmptyTag);
        }
        state.session_mob_tracking_tag = cleaned.to_string();
        Self::set_session_tag(&mut state, cleaned);
        Ok(())
    }

    /// Immediately set the active mob for manual kill stamping.
    pub fn set_manual_mob(
        &self,
        mob_name: &str,
        species: &str,
        maturity: &str,
    ) -> Result<(), TrackerCommandError> {
        let mut state = self.lock_state();
        if state.session.is_none() {
            return Err(TrackerCommandError::NoActiveSession);
        }
        if state.session_mob_tracking_mode == "tag" {
            return Err(TrackerCommandError::TagModeLocksMob);
        }
        if !(self.providers.manual_mob_entry_enabled)() {
            return Err(TrackerCommandError::ManualEntryDisabled);
        }
        Self::set_manual_mob_state(&mut state, mob_name, species, maturity);
        Ok(())
    }

    /// Clear the current/confirmed mob state, returning the released
    /// name.
    pub fn release_current_mob(&self) -> Option<String> {
        let mut state = self.lock_state();
        let released = if !state.confirmed_mob_name.is_empty() {
            Some(state.confirmed_mob_name.clone())
        } else if !state.current_mob_name.is_empty() {
            Some(state.current_mob_name.clone())
        } else {
            None
        };
        Self::clear_mob_state(&mut state);
        released
    }

    /// Start a new tracking session; any prior session stops first,
    /// outside the state guard so its own stop events publish cleanly.
    pub fn start_session(self: &Arc<Self>) -> Result<TrackingSession, DbError> {
        if self.is_tracking() {
            self.stop_session()?;
        }

        let session_mob_tracking_mode = (self.providers.mob_tracking_mode)();
        let session_mob_tracking_tag = (self.providers.mob_tracking_tag)()
            .trim_matches(python_whitespace)
            .to_string();
        let session_id = uuid::Uuid::new_v4().to_string();
        // Resolve the trifecta (a DB read) before the lock; only the
        // in-memory load runs under it.
        let trifecta_mode = (self.providers.weapon_attribution_trifecta)();
        let trifecta = if trifecta_mode {
            (self.providers.trifecta_resolver)()
        } else {
            None
        };

        let (session, start_ts) = {
            let mut state = self.lock_state();
            self.refresh_loot_filter_locked(&mut state);
            let session = TrackingSession {
                id: session_id.clone(),
                start_time: self.clock.now(),
                end_time: None,
                kills: Vec::new(),
                dangling_cost: 0.0,
            };
            state.session = Some(session.clone());
            state.accumulator = Some(Accumulator::default());
            state.active_hotbar_tool_name = None;
            state.last_heal_time = None;
            state.session_heal_cost = 0.0;
            state.heal_warning_emitted = false;
            state.session_warnings.clear();
            state.last_loot_fingerprint = None;
            state.last_loot_time = None;
            Self::clear_mob_state(&mut state);
            state.session_mob_tracking_mode = session_mob_tracking_mode.clone();
            state.session_mob_tracking_tag = session_mob_tracking_tag.clone();
            state.trifecta_unmatched_warning_emitted = false;
            // Reset under the lock, ordered with the handler
            // subscribes below, so a producer mutation arriving after
            // release correctly re-sets it.
            state.session_dirty = false;
            state.damage_attributor.clear();
            Self::reset_weapon_runtime_state(&mut state);

            if trifecta_mode {
                Self::load_trifecta_weapon_profiles(&mut state, trifecta.as_ref());
            }

            if state.session_mob_tracking_mode == "tag" && !session_mob_tracking_tag.is_empty() {
                Self::set_session_tag(&mut state, &session_mob_tracking_tag);
            } else if (self.providers.manual_mob_entry_enabled)() {
                if let Some((species, maturity)) = (self.providers.manual_mob)() {
                    let display = if maturity.is_empty() {
                        species.clone()
                    } else {
                        format!("{maturity} {species}")
                    };
                    Self::set_manual_mob_state(&mut state, &display, &species, &maturity);
                }
            }

            self.subscribe_handlers();
            let start_ts = naive_to_epoch(session.start_time);
            (session, start_ts)
        };

        // Persist session start. `mob_tracking_mode` records the input
        // mode the session was captured under so post-hoc UI surfaces
        // can choose label vocabulary; the value never mutates after
        // session start.
        self.block_on(async {
            sqlx::query(
                "INSERT INTO tracking_sessions \
                 (id, started_at, is_active, mob_tracking_mode) \
                 VALUES (?, ?, 1, ?)",
            )
            .bind(&session_id)
            .bind(start_ts)
            .bind(&session_mob_tracking_mode)
            .execute(&self.pool)
            .await?;
            Ok::<(), DbError>(())
        })?;

        self.bus.publish(
            Topic::SessionStarted,
            &serde_json::json!({"session_id": session_id}),
        );
        self.emit_session_event(
            TrackingReason::Started,
            TrackingStatus::Active,
            start_ts,
            Some(&session_id),
        );
        Ok(session)
    }

    /// Stop the active session: dangling cost, the handler
    /// unsubscribes and the end stamp under the guard; persistence,
    /// ledger gains, summary, and the stop events after it; then the
    /// in-memory clear.
    pub fn stop_session(&self) -> Result<Option<TrackingSession>, DbError> {
        let (session, session_id, end_time, heal_cost, dangling_cost) = {
            let mut state = self.lock_state();
            let dangling_cost = state
                .accumulator
                .as_ref()
                .map(Accumulator::total_cost)
                .unwrap_or(0.0);
            let Some(session) = state.session.as_mut() else {
                return Ok(None);
            };
            // Unsubscribe so no producer event mutates the session
            // past here.
            session.end_time = Some(self.clock.now());
            session.dangling_cost = dangling_cost;
            let snapshot = session.clone();
            let session_id = snapshot.id.clone();
            let end_time = snapshot.end_time.expect("just stamped");
            let heal_cost = state.session_heal_cost;
            self.unsubscribe_handlers();
            (snapshot, session_id, end_time, heal_cost, dangling_cost)
        };

        // The original groups these writes under one commit; the port
        // issues them sequentially on the autocommit pool (the summary
        // helper owns its own statements), reaching the same durable
        // state with narrower crash atomicity.
        self.block_on(async {
            sqlx::query(
                "UPDATE tracking_sessions SET ended_at = ?, is_active = 0, \
                 heal_cost = ?, dangling_cost = ? WHERE id = ?",
            )
            .bind(naive_to_epoch(end_time))
            .bind(heal_cost)
            .bind(dangling_cost)
            .bind(&session_id)
            .execute(&self.pool)
            .await?;
            // Auto-generate ledger gains derived from persisted loot
            // rows.
            self.create_enhancer_rebate_ledger_entry(&session_id, end_time)
                .await?;
            self.create_shrapnel_ledger_entry(&session_id, end_time)
                .await?;
            write_session_summary(&self.pool, &session_id).await?;
            Ok::<(), DbError>(())
        })?;

        self.bus.publish(
            Topic::SessionStopped,
            &serde_json::json!({"session_id": session_id}),
        );
        // `end_time` was stamped from the injected clock above, so the
        // required `occurred_at` always carries the stop instant.
        self.emit_session_event(
            TrackingReason::Stopped,
            TrackingStatus::Idle,
            naive_to_epoch(end_time),
            Some(&session_id),
        );

        {
            let mut state = self.lock_state();
            state.session = None;
            state.accumulator = None;
            state.active_hotbar_tool_name = None;
            Self::reset_weapon_runtime_state(&mut state);
            Self::clear_mob_state(&mut state);
        }
        Ok(Some(session))
    }

    fn subscribe_handlers(self: &Arc<Self>) {
        if self.subscribed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut subscriptions = self.subscriptions.lock().expect("subscriptions");
        type Handler = fn(&HuntTracker, &Value);
        let pairs: [(Topic, Handler); 7] = [
            (Topic::Combat, Self::on_combat),
            (Topic::LootGroup, Self::on_loot),
            (Topic::ActiveToolChanged, Self::on_tool_changed),
            (Topic::ActiveHealToolChanged, Self::on_heal_tool_changed),
            (Topic::Global, Self::on_global),
            (Topic::EnhancerBreak, Self::on_enhancer_break),
            (Topic::TickFlushed, Self::on_tick_flushed),
        ];
        for (topic, handler) in pairs {
            let tracker = self.clone();
            let registration = self
                .bus
                .subscribe(topic, move |data| handler(&tracker, data));
            subscriptions.push((topic, registration));
        }
    }

    fn unsubscribe_handlers(&self) {
        if !self.subscribed.swap(false, Ordering::SeqCst) {
            return;
        }
        let mut subscriptions = self.subscriptions.lock().expect("subscriptions");
        for (topic, registration) in subscriptions.drain(..) {
            self.bus.unsubscribe(topic, registration);
        }
    }

    /// Publish the coarse, frontend-facing tracking.session.updated
    /// event: the typed envelope's JSON value rides the bus, so the
    /// stream carries the same shape the original's model dump records
    /// and the SSE bridge serialises. `occurred_at` is stamped from
    /// the domain timestamp that triggered the event, not a fresh
    /// clock read, so the event is deterministic under replay.
    fn emit_session_event(
        &self,
        reason: TrackingReason,
        status: TrackingStatus,
        occurred_ts: f64,
        session_id: Option<&str>,
    ) {
        let event = DomainEvent::TrackingSessionUpdated(TrackingSessionUpdated {
            topic: TrackingSessionUpdatedTag,
            event_version: 1,
            occurred_at: to_iso_utc(occurred_ts),
            payload: TrackingSessionUpdatedPayload {
                session_id: session_id.map(str::to_string),
                status,
                reason,
            },
        });
        let value = serde_json::to_value(&event).expect("domain events always serialise");
        self.bus.publish(Topic::TrackingSessionUpdated, &value);
    }

    fn refresh_loot_filter_locked(&self, state: &mut TrackerState) {
        let blacklist: Vec<String> = match &self.providers.loot_filter_blacklist_provider {
            Some(provider) => provider(),
            None => self.providers.loot_filter_blacklist.clone(),
        };
        state.loot_blacklist = normalize_blacklist(Some(blacklist.iter().map(String::as_str)));
    }

    fn clear_mob_state(state: &mut TrackerState) {
        state.current_mob_name.clear();
        state.current_mob_species.clear();
        state.current_mob_maturity.clear();
        state.confirmed_mob_name.clear();
        state.confirmed_mob_species.clear();
        state.confirmed_mob_maturity.clear();
        state.mob_source = None;
    }

    fn set_session_tag(state: &mut TrackerState, tag: &str) {
        state.current_mob_name = tag.to_string();
        state.current_mob_species.clear();
        state.current_mob_maturity.clear();
        state.confirmed_mob_name = tag.to_string();
        state.confirmed_mob_species.clear();
        state.confirmed_mob_maturity.clear();
        state.mob_source = Some("tag");
    }

    fn set_manual_mob_state(state: &mut TrackerState, name: &str, species: &str, maturity: &str) {
        state.current_mob_name = name.to_string();
        state.current_mob_species = species.to_string();
        state.current_mob_maturity = maturity.to_string();
        state.confirmed_mob_name = name.to_string();
        state.confirmed_mob_species = species.to_string();
        state.confirmed_mob_maturity = maturity.to_string();
        state.mob_source = Some("manual");
    }

    fn reset_weapon_runtime_state(state: &mut TrackerState) {
        state.trifecta_weapon_profiles.clear();
        state.weapon_enhancer_states.clear();
        state.active_weapon_state_key = None;
        state.active_weapon_observed_name = None;
        state.last_offensive_tool_name = None;
        state.profile_match_cache.clear();
        state.static_tool_cost_cache.clear();
    }

    /// Load damage signatures + heal tool from the resolved trifecta
    /// configuration. The weapon fields read with inert defaults
    /// where the original indexes (the resolver supplies complete
    /// weapon objects by contract).
    fn load_trifecta_weapon_profiles(
        state: &mut TrackerState,
        trifecta: Option<&Map<String, Value>>,
    ) {
        state.damage_attributor.clear();
        state.active_heal_tool_name = None;
        state.heal_cost_per_use_ped = 0.0;
        state.heal_reload_seconds = 2.5;
        state.heal_amount_min = None;
        state.heal_amount_max = None;
        state.heal_warning_emitted = false;
        state.trifecta_weapon_profiles.clear();
        state.active_weapon_state_key = None;
        state.active_weapon_observed_name = None;

        let Some(trifecta) = trifecta.filter(|map| !map.is_empty()) else {
            return;
        };
        for key in ["small_weapon", "big_weapon"] {
            let Some(weapon) = trifecta.get(key).filter(|value| value_truthy(value)) else {
                continue;
            };
            let name = weapon.get("name").and_then(Value::as_str).unwrap_or("");
            state.damage_attributor.add_weapon_profile(
                name,
                weapon
                    .get("damage_min")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                weapon
                    .get("damage_max")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                weapon
                    .get("total_damage")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                weapon
                    .get("cost_per_shot_ped")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                weapon.get("role").and_then(Value::as_str),
            );
            if let Some(props) = weapon
                .get("weapon_props")
                .filter(|value| value_truthy(value))
            {
                state
                    .trifecta_weapon_profiles
                    .insert(name.to_string(), Arc::new(props.clone()));
            }
        }
        if let Some(heal) = trifecta
            .get("heal_tool")
            .filter(|value| value_truthy(value))
        {
            state.active_heal_tool_name =
                heal.get("name").and_then(Value::as_str).map(str::to_string);
            state.heal_cost_per_use_ped = heal
                .get("cost_per_use_ped")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            state.heal_reload_seconds = heal
                .get("reload_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(2.5);
            state.heal_amount_min = heal.get("heal_min").and_then(Value::as_f64);
            state.heal_amount_max = heal.get("heal_max").and_then(Value::as_f64);
        }
    }

    /// Resolve a tool name to its canonical profile: the trifecta
    /// table first, then the memoised equipment-library lookup.
    fn match_weapon_profile(
        &self,
        state: &mut TrackerState,
        tool_name: &str,
    ) -> Option<(String, Arc<Value>)> {
        // The trifecta table only stores truthy props, so a hit is the
        // original's `if profile:` taken branch.
        if let Some(profile) = state.trifecta_weapon_profiles.get(tool_name) {
            return Some((tool_name.to_string(), profile.clone()));
        }

        if let Some(cached) = state.profile_match_cache.get(tool_name) {
            return cached.clone();
        }

        let resolved = (self.providers.equipment_profile_lookup)(tool_name)
            .filter(|profile| !profile.is_empty());
        let Some(profile) = resolved else {
            state
                .profile_match_cache
                .insert(tool_name.to_string(), None);
            return None;
        };
        // `profile.get("weapon_entity", {}).get("name") or tool_name`.
        let canonical_name = profile
            .get("weapon_entity")
            .and_then(Value::as_object)
            .and_then(|entity| entity.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(tool_name)
            .to_string();
        let matched = Some((canonical_name, Arc::new(Value::Object(profile))));
        state
            .profile_match_cache
            .insert(tool_name.to_string(), matched.clone());
        matched
    }

    /// Resolve (creating if first seen) the enhancer state for a
    /// matched weapon, stamping the active-weapon markers either way.
    fn ensure_weapon_state<'a>(
        &self,
        state: &'a mut TrackerState,
        tool_name: &str,
    ) -> Option<&'a mut DamageEnhancerState> {
        let Some((canonical_name, profile)) = self.match_weapon_profile(state, tool_name) else {
            state.active_weapon_state_key = None;
            state.active_weapon_observed_name = Some(tool_name.to_string());
            return None;
        };
        state
            .weapon_enhancer_states
            .entry(canonical_name.clone())
            .or_insert_with(|| DamageEnhancerState::from_props(&canonical_name, profile));
        state.active_weapon_state_key = Some(canonical_name.clone());
        state.active_weapon_observed_name = Some(tool_name.to_string());
        state.weapon_enhancer_states.get_mut(&canonical_name)
    }

    fn current_cost_for_tool(
        &self,
        state: &mut TrackerState,
        tool_name: &str,
        inferred_cost: f64,
    ) -> f64 {
        if let Some(weapon) = self.ensure_weapon_state(state, tool_name) {
            return weapon.current_cost_ped();
        }
        if inferred_cost > 0.0 {
            return inferred_cost;
        }
        if let Some(cached) = state.static_tool_cost_cache.get(tool_name) {
            return *cached;
        }
        let cost = (self.providers.equipment_cost_lookup)(tool_name);
        state
            .static_tool_cost_cache
            .insert(tool_name.to_string(), cost);
        cost
    }

    /// The accumulator's stats entry for this tool at this cost: an
    /// existing phase within the cost tolerance, or a new phase keyed
    /// `name`, then `name#2`...
    fn tool_stats_for_phase<'a>(
        state: &'a mut TrackerState,
        tool_name: &str,
        cost_per_shot: f64,
    ) -> &'a mut ToolStats {
        let accumulator = state
            .accumulator
            .as_mut()
            .expect("no accumulator available");
        if let Some(index) = accumulator.tool_stats.iter().position(|(_, stats)| {
            stats.tool_name == tool_name && (stats.cost_per_shot - cost_per_shot).abs() < 1e-9
        }) {
            return &mut accumulator.tool_stats[index].1;
        }
        let phase_count = accumulator
            .tool_stats
            .iter()
            .filter(|(_, stats)| stats.tool_name == tool_name)
            .count();
        let key = if phase_count == 0 {
            tool_name.to_string()
        } else {
            format!("{tool_name}#{}", phase_count + 1)
        };
        accumulator
            .tool_stats
            .push((key, ToolStats::new(tool_name, cost_per_shot)));
        &mut accumulator.tool_stats.last_mut().expect("just pushed").1
    }

    /// Accumulate one player attack, including jam/dodge/evade
    /// countered shots.
    fn record_offensive_shot(
        &self,
        state: &mut TrackerState,
        amount: f64,
        is_crit: bool,
        allow_damage_inference: bool,
    ) {
        if state.accumulator.is_none() {
            return;
        }
        {
            let accumulator = state.accumulator.as_mut().expect("checked above");
            accumulator.shots_fired += 1;
            if amount > 0.0 {
                accumulator.damage_dealt += amount;
            }
            if is_crit {
                accumulator.critical_hits += 1;
            }
        }

        let mut inferred_cost = 0.0;
        let mut tool: Option<String> = None;
        if (self.providers.weapon_attribution_trifecta)() {
            if allow_damage_inference {
                let attribution = state.damage_attributor.match_damage(amount, is_crit);
                if attribution.is_none() && !state.trifecta_unmatched_warning_emitted {
                    state.session_warnings.push(
                        "Trifecta attribution: damage fell outside both weapon ranges".to_string(),
                    );
                    state.trifecta_unmatched_warning_emitted = true;
                }
                if let Some(attribution) = attribution {
                    tool = Some(attribution.tool_name);
                    inferred_cost = attribution.cost_per_shot;
                }
            } else {
                tool = state.last_offensive_tool_name.clone();
            }
        } else {
            tool = state.active_hotbar_tool_name.clone();
        }

        if let Some(tool) = &tool {
            state.last_offensive_tool_name = Some(tool.clone());
        }

        // `tool or "Unknown"`: the falsy coercion, so an empty name
        // also keys the fallback entry.
        let tool_key = tool
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("Unknown")
            .to_string();
        let mut current_cost = 0.0;
        if let Some(tool) = &tool {
            current_cost = self.current_cost_for_tool(state, tool, inferred_cost);
        }

        let stats: &mut ToolStats = if let (Some(tool), true) = (&tool, current_cost > 0.0) {
            Self::tool_stats_for_phase(state, tool, current_cost)
        } else {
            let accumulator = state.accumulator.as_mut().expect("checked above");
            if !accumulator
                .tool_stats
                .iter()
                .any(|(key, _)| key == &tool_key)
            {
                accumulator
                    .tool_stats
                    .push((tool_key.clone(), ToolStats::new(&tool_key, 0.0)));
            }
            let index = accumulator
                .tool_stats
                .iter()
                .position(|(key, _)| key == &tool_key)
                .expect("just ensured");
            let entry = &mut accumulator.tool_stats[index].1;
            // The fallback cost resolves only for a still-costless
            // entry, so the provider is not re-read on every shot.
            if entry.cost_per_shot == 0.0 {
                let fallback_cost = if inferred_cost > 0.0 {
                    inferred_cost
                } else {
                    (self.providers.equipment_cost_lookup)(&tool_key)
                };
                if fallback_cost > 0.0 {
                    entry.cost_per_shot = fallback_cost;
                }
            }
            entry
        };
        stats.shots_fired += 1;
        if amount > 0.0 {
            stats.damage_dealt += amount;
        }
        if is_crit {
            stats.critical_hits += 1;
        }
    }

    /// Handle a parsed combat event from chat.log. The whole body
    /// mutates owned in-memory state, so it runs under the guard;
    /// there is no DB write or publish. Defensive incoming events
    /// stay out of the kills model.
    fn on_combat(&self, data: &Value) {
        let mut state = self.lock_state();
        if state.accumulator.is_none() {
            return;
        }

        let event_type = data.get("type").and_then(Value::as_str).unwrap_or("");
        let amount = data.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
        let timestamp = parse_bus_timestamp(data.get("timestamp"));
        // Whether this event actually changed the live session
        // readout: the coalesced tracking.session.updated fires only
        // on a real mutation, so a duplicate self-heal tick or an
        // unhandled event type does not wake listeners for a no-op.
        let mut mutated = false;

        match event_type {
            "damage_dealt" | "critical_hit" => {
                self.record_offensive_shot(&mut state, amount, event_type == "critical_hit", true);
                mutated = true;
            }
            "target_dodge" | "target_evade" | "target_jam" => {
                self.record_offensive_shot(&mut state, 0.0, false, false);
                mutated = true;
            }
            "damage_received" => {
                state
                    .accumulator
                    .as_mut()
                    .expect("checked above")
                    .damage_taken += amount;
                mutated = true;
            }
            "self_heal" => {
                // Deduplicate: tool activations produce multiple heal
                // ticks in chat.log. Use the tool's reload time as the
                // dedup window.
                if let Some(timestamp) = timestamp {
                    let is_new_heal_activation = match state.last_heal_time {
                        None => true,
                        Some(last) => {
                            python_total_seconds(timestamp - last) >= state.heal_reload_seconds
                        }
                    };
                    if is_new_heal_activation {
                        if (self.providers.weapon_attribution_trifecta)()
                            && !heal_amount_matches_trifecta_tool(&state, amount)
                        {
                            return;
                        }
                        if state.active_heal_tool_name.is_none() && !state.heal_warning_emitted {
                            state.session_warnings.push(
                                "Healing detected: no heal tool equipped via hotbar".to_string(),
                            );
                            state.heal_warning_emitted = true;
                        }
                        if state.heal_cost_per_use_ped > 0.0 {
                            state.session_heal_cost += state.heal_cost_per_use_ped;
                        }
                        state.last_heal_time = Some(timestamp);
                        mutated = true;
                    }
                }
            }
            _ => {}
        }

        if mutated {
            state.session_dirty = true;
        }
    }

    /// Handle a loot group from chat.log: creates a Kill record. The
    /// kill is built from the accumulator, the accumulator reset, and
    /// the kill appended to the session under the guard; the kill is
    /// a detached value by then, so the persisting DB write runs
    /// after release.
    fn on_loot(&self, data: &Value) {
        let kill = {
            let mut state = self.lock_state();
            if state.accumulator.is_none() || state.session.is_none() {
                return;
            }

            // A missing key reads as the original's `.get` default; a
            // present non-list raises there (contained, no kill, no
            // fingerprint stamp), so it drops the group here too.
            let empty_items = Vec::new();
            let items_raw = match data.get("items") {
                None => &empty_items,
                Some(Value::Array(items)) => items,
                Some(_) => return,
            };
            let total_ped = data.get("total_ped").and_then(Value::as_f64).unwrap_or(0.0);
            let now =
                parse_bus_timestamp(data.get("timestamp")).unwrap_or_else(|| self.clock.now());
            let now_epoch = naive_to_epoch(now);

            // Loot deduplication (same fingerprint within 2s window).
            let first_item = items_raw
                .first()
                .and_then(|item| item.get("item_name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let fingerprint = (round_half_even(total_ped, 4), items_raw.len(), first_item);
            if state.last_loot_fingerprint.as_ref() == Some(&fingerprint) {
                if let Some(last) = state.last_loot_time {
                    if python_total_seconds(now - last) < LOOT_DEDUP_WINDOW_SECONDS {
                        return;
                    }
                }
            }
            state.last_loot_fingerprint = Some(fingerprint);
            state.last_loot_time = Some(now);
            // Past the dedup guard a Kill is always recorded, so the
            // readout changes.
            state.session_dirty = true;

            let mut items = Vec::new();
            for item in items_raw {
                let name = item.get("item_name").and_then(Value::as_str).unwrap_or("");
                if is_tracked_loot(name, &state.loot_blacklist) {
                    items.push(LootItem {
                        item_name: name.to_string(),
                        quantity: item.get("quantity").and_then(Value::as_i64).unwrap_or(1),
                        value_ped: item.get("value_ped").and_then(Value::as_f64).unwrap_or(0.0),
                        is_enhancer_shrapnel: item
                            .get("is_enhancer_shrapnel")
                            .is_some_and(value_truthy),
                    });
                }
            }
            let filtered_total_ped = round_half_even(
                items
                    .iter()
                    .filter(|item| !item.is_enhancer_shrapnel)
                    .map(|item| item.value_ped)
                    .sum(),
                4,
            );

            // Snapshot mob/tag from manual configuration.
            let mob_name = if state.confirmed_mob_name.is_empty() {
                "Unknown".to_string()
            } else {
                state.confirmed_mob_name.clone()
            };

            let session_id = state.session.as_ref().expect("checked above").id.clone();
            let mob_species = state.confirmed_mob_species.clone();
            let mob_maturity = state.confirmed_mob_maturity.clone();
            let accumulator = state.accumulator.as_mut().expect("checked above");
            let kill = Kill {
                id: uuid::Uuid::new_v4().to_string(),
                session_id,
                mob_name,
                mob_species,
                mob_maturity,
                timestamp: now_epoch,
                shots_fired: accumulator.shots_fired,
                damage_dealt: accumulator.damage_dealt,
                damage_taken: accumulator.damage_taken,
                critical_hits: accumulator.critical_hits,
                cost_ped: accumulator.weapon_cost(),
                enhancer_cost: accumulator.enhancer_cost,
                loot_total_ped: filtered_total_ped,
                loot_items: items,
                tool_stats: std::mem::take(&mut accumulator.tool_stats),
                is_global: false,
                is_hof: false,
            };

            // Reset accumulator for next kill (tool_stats moved into
            // the kill above, exactly the original's shallow copy
            // followed by a fresh dict).
            state.accumulator.as_mut().expect("checked above").reset();

            // Append the finalised kill to the session; the list tail
            // doubles as the original's `_last_kill` alias.
            state
                .session
                .as_mut()
                .expect("checked above")
                .kills
                .push(kill.clone());
            kill
        };

        // Persist outside the guard: `kill` is a detached value and
        // the lock is never held across SQLite.
        self.persist_kill(&kill);
    }

    /// Handle hotbar-driven weapon tool change: merges any 'Unknown'
    /// tool stats into the real tool when first detected.
    fn on_tool_changed(&self, data: &Value) {
        let mut state = self.lock_state();
        if (self.providers.weapon_attribution_trifecta)() {
            return;
        }
        let Some(tool_name) = data
            .get("tool_name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
        else {
            return;
        };
        state.active_hotbar_tool_name = Some(tool_name.clone());
        if state.accumulator.is_none() {
            return;
        }

        let current_cost = self.current_cost_for_tool(&mut state, &tool_name, 0.0);

        // Merge "Unknown" stats into the real tool on first
        // identification.
        let unknown = {
            let accumulator = state.accumulator.as_mut().expect("checked above");
            accumulator
                .tool_stats
                .iter()
                .position(|(key, _)| key == "Unknown")
                .map(|index| accumulator.tool_stats.remove(index).1)
        };
        if let Some(unknown) = unknown {
            let real: &mut ToolStats = if current_cost > 0.0 {
                Self::tool_stats_for_phase(&mut state, &tool_name, current_cost)
            } else {
                let accumulator = state.accumulator.as_mut().expect("checked above");
                if !accumulator
                    .tool_stats
                    .iter()
                    .any(|(key, _)| key == &tool_name)
                {
                    accumulator
                        .tool_stats
                        .push((tool_name.clone(), ToolStats::new(&tool_name, 0.0)));
                }
                let index = accumulator
                    .tool_stats
                    .iter()
                    .position(|(key, _)| key == &tool_name)
                    .expect("just ensured");
                &mut accumulator.tool_stats[index].1
            };
            real.shots_fired += unknown.shots_fired;
            real.damage_dealt += unknown.damage_dealt;
            real.critical_hits += unknown.critical_hits;
        }
    }

    /// Handle hotbar-driven heal tool equip.
    fn on_heal_tool_changed(&self, data: &Value) {
        if (self.providers.weapon_attribution_trifecta)() {
            return;
        }
        let name = data
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_string);
        let cost = data
            .get("cost_per_use_ped")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let reload_seconds = data
            .get("reload_seconds")
            .and_then(Value::as_f64)
            .unwrap_or(2.5);

        let mut state = self.lock_state();
        state.active_heal_tool_name = name;
        state.heal_cost_per_use_ped = cost;
        state.heal_reload_seconds = reload_seconds;
        state.heal_amount_min = None;
        state.heal_amount_max = None;
        state.heal_warning_emitted = false;
    }

    /// Handle a global/HoF event from chat.log: tags the most
    /// recently created kill (globals arrive shortly after loot). The
    /// in-memory tag lands under the guard, capturing the values the
    /// DB writes need; the UPDATE/INSERT run after release.
    fn on_global(&self, data: &Value) {
        let (session_id, kill_id, target_is_hof, event_type, mob_or_item, value_ped, ts) = {
            let mut state = self.lock_state();
            if state.session.is_none() {
                return;
            }

            // Filter for own player.
            let player = data.get("player").and_then(Value::as_str).unwrap_or("");
            if self.providers.player_name.is_empty()
                || player.to_lowercase() != self.providers.player_name.to_lowercase()
            {
                return;
            }

            state.session_dirty = true;
            let session_id = state.session.as_ref().expect("checked above").id.clone();
            let event_type = data
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // `data.get("creature") or data.get("item") or "Unknown"`:
            // the falsy chain, so an empty creature falls through.
            let mob_or_item = [data.get("creature"), data.get("item")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .find(|name| !name.is_empty())
                .unwrap_or("Unknown")
                .to_string();
            let value_ped = data.get("value").and_then(Value::as_f64).unwrap_or(0.0);
            let is_hof = matches!(event_type.as_str(), "hof_kill" | "hof_item");
            let ts = parse_bus_timestamp(data.get("timestamp"))
                .map(naive_to_epoch)
                .unwrap_or_else(|| naive_to_epoch(self.clock.now()));

            // Tag the most recently created kill (staleness check:
            // within 5s). The kills tail is the original's
            // `_last_kill` alias.
            let mut kill_id: Option<String> = None;
            let mut target_is_hof = false;
            if let Some(target) = state
                .session
                .as_mut()
                .expect("checked above")
                .kills
                .last_mut()
            {
                if (ts - target.timestamp).abs() < GLOBAL_CORRELATION_WINDOW_SECONDS {
                    target.is_global = true;
                    if is_hof {
                        target.is_hof = true;
                    }
                    kill_id = Some(target.id.clone());
                    target_is_hof = target.is_hof;
                }
            }
            (
                session_id,
                kill_id,
                target_is_hof,
                event_type,
                mob_or_item,
                value_ped,
                ts,
            )
        };

        let result = self.block_on(async {
            let mut tx = self.pool.begin().await?;
            if let Some(kill_id) = &kill_id {
                sqlx::query("UPDATE kills SET is_global = 1, is_hof = ? WHERE id = ?")
                    .bind(i64::from(target_is_hof))
                    .bind(kill_id)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query(
                "INSERT INTO notable_events \
                 (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&session_id)
            .bind(&kill_id)
            .bind(&event_type)
            .bind(&mob_or_item)
            .bind(value_ped)
            .bind(ts)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        });
        // A persistence failure is contained like the original's
        // handler exception: the in-memory tag stands.
        let _ = result;
    }

    /// Handle an enhancer break event: update enhancer state for
    /// future shots. There is no DB write or publish.
    fn on_enhancer_break(&self, data: &Value) {
        let mut state = self.lock_state();
        if state.accumulator.is_none() {
            return;
        }

        let enhancer_name = data
            .get("enhancer_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let item_name = data.get("item_name").and_then(Value::as_str).unwrap_or("");
        // The original narrows with `isinstance(remaining, int)`; a
        // fractional or missing count falls back to the
        // decrement-one-slot path.
        let remaining = data.get("remaining").and_then(Value::as_i64);

        let applies = {
            let weapon = state
                .active_weapon_state_key
                .as_ref()
                .and_then(|key| state.weapon_enhancer_states.get(key));
            match weapon {
                Some(weapon) => {
                    !weapon.stacks.is_empty()
                        && enhancer_name.to_lowercase().contains("damage")
                        && break_matches_active_weapon(&state, item_name)
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
        state.session_dirty = true;
        let key = state
            .active_weapon_state_key
            .clone()
            .expect("checked above");
        state
            .weapon_enhancer_states
            .get_mut(&key)
            .expect("checked above")
            .apply_break(remaining);
    }

    /// Coalesce a settled tick's mutations into one domain event.
    /// Subscribed only while a session is active; fires only when the
    /// tick actually changed the live session readout, stamped with
    /// the tick's own timestamp (already on the tick's loot/combat
    /// events) or the injected clock when the tick carries none.
    fn on_tick_flushed(&self, data: &Value) {
        // Read/reset the dirty flag under the guard; publish after
        // release so a subscriber never runs while this tracker holds
        // its lock.
        let session_id = {
            let mut state = self.lock_state();
            let Some(session) = state.session.as_ref() else {
                return;
            };
            if !state.session_dirty {
                return;
            }
            let session_id = session.id.clone();
            state.session_dirty = false;
            session_id
        };
        // The original's three-way stamp: a datetime-equivalent string
        // takes its instant, anything else present goes through
        // `float()` (an unparseable value raises there, contained with
        // the dirty flag already consumed: no event), and an absent
        // timestamp falls back to the injected clock.
        let raw_ts = data.get("timestamp");
        let occurred_ts = match raw_ts {
            None | Some(Value::Null) => naive_to_epoch(self.clock.now()),
            Some(value) => match parse_bus_timestamp(Some(value)) {
                Some(instant) => naive_to_epoch(instant),
                None => match value {
                    Value::String(text) => match text.trim().parse::<f64>() {
                        Ok(numeric) => numeric,
                        Err(_) => return,
                    },
                    other => match other.as_f64() {
                        Some(numeric) => numeric,
                        None => return,
                    },
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

    /// Write a finalised kill to the database: the kill row, the
    /// per-tool stats (`INSERT OR REPLACE` keyed on the tool name, so
    /// among same-name phases the last written wins, as the
    /// original's insertion-ordered iteration does), and the loot
    /// items, under one commit.
    fn persist_kill(&self, kill: &Kill) {
        let result = self.block_on(async {
            let mut tx = self.pool.begin().await?;
            sqlx::query(
                "INSERT OR REPLACE INTO kills \
                 (id, session_id, mob_name, mob_species, mob_maturity, \
                  timestamp, shots_fired, damage_dealt, damage_taken, \
                  critical_hits, cost_ped, enhancer_cost, \
                  loot_total_ped, is_global, is_hof) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&kill.id)
            .bind(&kill.session_id)
            .bind(&kill.mob_name)
            .bind(&kill.mob_species)
            .bind(&kill.mob_maturity)
            .bind(kill.timestamp)
            .bind(kill.shots_fired)
            .bind(kill.damage_dealt)
            .bind(kill.damage_taken)
            .bind(kill.critical_hits)
            .bind(kill.cost_ped)
            .bind(kill.enhancer_cost)
            .bind(kill.loot_total_ped)
            .bind(i64::from(kill.is_global))
            .bind(i64::from(kill.is_hof))
            .execute(&mut *tx)
            .await?;

            for (_, stats) in &kill.tool_stats {
                sqlx::query(
                    "INSERT OR REPLACE INTO kill_tool_stats \
                     (kill_id, tool_name, shots_fired, damage_dealt, \
                      critical_hits, cost_per_shot) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&kill.id)
                .bind(&stats.tool_name)
                .bind(stats.shots_fired)
                .bind(stats.damage_dealt)
                .bind(stats.critical_hits)
                .bind(stats.cost_per_shot)
                .execute(&mut *tx)
                .await?;
            }

            for item in &kill.loot_items {
                sqlx::query(
                    "INSERT INTO kill_loot_items \
                     (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&kill.id)
                .bind(&item.item_name)
                .bind(item.quantity)
                .bind(item.value_ped)
                .bind(i64::from(item.is_enhancer_shrapnel))
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        });
        // Contained like the original's handler exception.
        let _ = result;
    }

    /// Session-end margin on non-enhancer Shrapnel loot (1%, the
    /// trade-terminal conversion premium), recorded as a markup
    /// ledger gain.
    async fn create_shrapnel_ledger_entry(
        &self,
        session_id: &str,
        end_time: NaiveDateTime,
    ) -> Result<(), DbError> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(kli.value_ped), 0) \
             FROM kill_loot_items kli \
             JOIN kills k ON kli.kill_id = k.id \
             WHERE k.session_id = ? AND kli.item_name = 'Shrapnel' \
             AND COALESCE(kli.is_enhancer_shrapnel, 0) = 0 \
             AND kli.deactivated_at IS NULL",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        let shrapnel_ped = decoded_f64(&row, 0);
        if shrapnel_ped <= 0.0 {
            return Ok(());
        }
        let margin = round_half_even(shrapnel_ped * 0.01, 4);
        sqlx::query(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(naive_isoformat(end_time))
        .bind("markup")
        .bind("Shrapnel Conversion")
        .bind(margin)
        .bind("convert")
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Session-end rebate on enhancer-break Shrapnel (full TT value
    /// returned by breaks), recorded as a markup ledger gain.
    async fn create_enhancer_rebate_ledger_entry(
        &self,
        session_id: &str,
        end_time: NaiveDateTime,
    ) -> Result<(), DbError> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(kli.value_ped), 0) \
             FROM kill_loot_items kli \
             JOIN kills k ON kli.kill_id = k.id \
             WHERE k.session_id = ? AND COALESCE(kli.is_enhancer_shrapnel, 0) = 1 \
             AND kli.deactivated_at IS NULL",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        let rebate = decoded_f64(&row, 0);
        if rebate <= 0.0 {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(naive_isoformat(end_time))
        .bind("markup")
        .bind("Enhancer Shrapnel Rebate")
        .bind(round_half_even(rebate, 4))
        .bind("enhancer")
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Trifecta direct-heal attribution uses the configured heal
/// interval.
fn heal_amount_matches_trifecta_tool(state: &TrackerState, amount: f64) -> bool {
    match (state.heal_amount_min, state.heal_amount_max) {
        (Some(min), Some(max)) => min <= amount && amount <= max,
        _ => true,
    }
}

/// Whether a break's item name names the active weapon (either the
/// canonical or the observed hotbar spelling), compared on lowercased
/// alphanumerics in either containment direction.
fn break_matches_active_weapon(state: &TrackerState, item_name: &str) -> bool {
    let Some(weapon) = state
        .active_weapon_state_key
        .as_ref()
        .and_then(|key| state.weapon_enhancer_states.get(key))
    else {
        return false;
    };
    if item_name.is_empty() {
        return false;
    }
    let normalise = |raw: &str| -> String {
        raw.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let item_norm = normalise(item_name);
    let tool_norm = normalise(&weapon.tool_name);
    let observed_norm = normalise(state.active_weapon_observed_name.as_deref().unwrap_or(""));
    !item_norm.is_empty()
        && (tool_norm.contains(&item_norm)
            || item_norm.contains(&tool_norm)
            || (!observed_norm.is_empty()
                && (observed_norm.contains(&item_norm) || item_norm.contains(&observed_norm))))
}

/// Python truthiness for the wire values the original's falsy checks
/// guard (null/false/0/""/[]/{} are falsy).
fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|inner| inner != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Bus payload timestamps are the watcher's isoformat strings (whole
/// seconds; a fractional suffix is tolerated for symmetry with the
/// harness normaliser); the original receives `datetime` objects on
/// its in-process bus.
fn parse_bus_timestamp(value: Option<&Value>) -> Option<NaiveDateTime> {
    let raw = value?.as_str()?;
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

/// `timedelta.total_seconds()`.
fn python_total_seconds(delta: chrono::TimeDelta) -> f64 {
    delta
        .num_microseconds()
        .map(|micros| micros as f64 / 1e6)
        .unwrap_or_else(|| delta.num_seconds() as f64)
}

/// CPython `datetime.fromtimestamp`'s split of an epoch float into
/// whole seconds and half-even-rounded microseconds.
fn epoch_to_parts(epoch: f64) -> (i64, u32) {
    let mut secs = epoch.trunc() as i64;
    let mut micros = round_half_even((epoch - epoch.trunc()) * 1e6, 0) as i64;
    if micros >= 1_000_000 {
        secs += 1;
        micros -= 1_000_000;
    } else if micros < 0 {
        secs -= 1;
        micros += 1_000_000;
    }
    (secs, micros as u32)
}

/// The original's naive-local `datetime.timestamp()` (fold=0): the
/// earliest interpretation for ambiguous instants; an instant inside
/// a DST gap resolves through the neighbouring hour's offset.
fn naive_to_epoch(instant: NaiveDateTime) -> f64 {
    let resolved = match instant.and_local_timezone(chrono::Local) {
        chrono::LocalResult::Single(instant) => Some(instant),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => (instant + chrono::TimeDelta::hours(1))
            .and_local_timezone(chrono::Local)
            .earliest()
            .map(|shifted| shifted - chrono::TimeDelta::hours(1)),
    };
    resolved
        .map(|instant| {
            instant.timestamp() as f64 + f64::from(instant.timestamp_subsec_micros()) / 1e6
        })
        .unwrap_or(0.0)
}

/// The original's naive-local `datetime.fromtimestamp()`.
fn epoch_to_naive(epoch: f64) -> NaiveDateTime {
    let (secs, micros) = epoch_to_parts(epoch);
    chrono::DateTime::from_timestamp(secs, micros * 1_000)
        .map(|instant| instant.with_timezone(&chrono::Local).naive_local())
        .unwrap_or_default()
}

/// `datetime.isoformat()` for naive instants (ledger dates, the
/// readout's started_at): microseconds only when non-zero.
fn naive_isoformat(instant: NaiveDateTime) -> String {
    if instant.and_utc().timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

/// `backend/core/domain_events.to_iso_utc`: render an epoch float as
/// `datetime.fromtimestamp(ts, tz=UTC).isoformat()` does (the `T`
/// separator, microseconds only when non-zero, `+00:00` suffix).
fn to_iso_utc(ts: f64) -> String {
    let (secs, micros) = epoch_to_parts(ts);
    let instant = chrono::DateTime::from_timestamp(secs, micros * 1_000).unwrap_or_default();
    if micros == 0 {
        format!("{}+00:00", instant.format("%Y-%m-%dT%H:%M:%S"))
    } else {
        format!("{}+00:00", instant.format("%Y-%m-%dT%H:%M:%S%.6f"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::db::Db;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    struct Rig {
        _dir: tempfile::TempDir,
        runtime: tokio::runtime::Runtime,
        bus: Arc<EventBus>,
        clock: Arc<MockClock>,
        pool: SqlitePool,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db = runtime
            .block_on(Db::open(&dir.path().join("entropia_orme.db")))
            .unwrap();
        let pool = db.pool().clone();
        Rig {
            _dir: dir,
            runtime,
            bus: Arc::new(EventBus::new()),
            clock: Arc::new(MockClock::new(None, 0.0)),
            pool,
        }
    }

    impl Rig {
        fn tracker(&self, providers: Providers) -> Arc<HuntTracker> {
            HuntTracker::new(
                self.bus.clone(),
                self.pool.clone(),
                self.runtime.handle().clone(),
                self.clock.clone(),
                providers,
            )
            .unwrap()
        }

        fn capture(&self) -> Arc<StdMutex<Vec<(Topic, Value)>>> {
            let captured = Arc::new(StdMutex::new(Vec::new()));
            let sink = captured.clone();
            self.bus.add_tap(move |topic, data| {
                sink.lock().unwrap().push((topic, data.clone()));
            });
            captured
        }

        fn scalar_f64(&self, sql: &'static str, binds: &[&str]) -> f64 {
            let binds: Vec<String> = binds.iter().map(|bind| bind.to_string()).collect();
            self.runtime.block_on(async {
                let mut query = sqlx::query(sql);
                for bind in binds {
                    query = query.bind(bind);
                }
                let row = query.fetch_one(&self.pool).await.unwrap();
                decoded_f64(&row, 0)
            })
        }

        fn scalar_i64(&self, sql: &'static str, binds: &[&str]) -> i64 {
            let binds: Vec<String> = binds.iter().map(|bind| bind.to_string()).collect();
            self.runtime.block_on(async {
                let mut query = sqlx::query(sql);
                for bind in binds {
                    query = query.bind(bind);
                }
                let row = query.fetch_one(&self.pool).await.unwrap();
                row.try_get::<i64, _>(0).unwrap()
            })
        }

        fn execute(&self, sql: &'static str) {
            self.runtime.block_on(async {
                sqlx::query(sql).execute(&self.pool).await.unwrap();
            });
        }
    }

    fn naive(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").unwrap()
    }

    fn updated_events(captured: &StdMutex<Vec<(Topic, Value)>>) -> Vec<Value> {
        captured
            .lock()
            .unwrap()
            .iter()
            .filter(|(topic, _)| *topic == Topic::TrackingSessionUpdated)
            .map(|(_, data)| data.clone())
            .collect()
    }

    #[test]
    fn session_lifecycle_round_trip() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_cost_lookup: Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 }),
            ..Providers::default()
        });
        let captured = rig.capture();

        assert!(!tracker.is_tracking());
        assert!(tracker.stop_session().unwrap().is_none());
        assert!(!rig.bus.has_subscribers(Topic::Combat));

        let session = tracker.start_session().unwrap();
        assert!(tracker.is_tracking());
        assert!(rig.bus.has_subscribers(Topic::Combat));
        let start_ts = naive_to_epoch(naive("2026-01-01T00:00:00"));
        assert_eq!(
            rig.scalar_f64(
                "SELECT started_at FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            start_ts
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT is_active FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            1
        );

        // Accumulate one kill with both shrapnel kinds, plus dangling
        // shots after it.
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Rifle"}));
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 30.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({
                "type": "loot",
                "timestamp": "2026-01-01T00:00:02",
                "items": [
                    {"item_name": "Animal Hide", "quantity": 1, "value_ped": 4.5,
                     "is_enhancer_shrapnel": false},
                    {"item_name": "Shrapnel", "quantity": 50, "value_ped": 0.5,
                     "is_enhancer_shrapnel": false},
                    {"item_name": "Shrapnel", "quantity": 10, "value_ped": 0.1,
                     "is_enhancer_shrapnel": true},
                ],
                "total_ped": 5.1,
            }),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 7.5, "timestamp": "2026-01-01T00:00:03"}),
        );

        // A skill gain qualifies the session for a summary.
        rig.runtime.block_on(async {
            sqlx::query(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, 1.0, 'Rifle', 1.0, 0.5)",
            )
            .bind(&session.id)
            .execute(&rig.pool)
            .await
            .unwrap();
        });

        rig.clock.advance(10.0).unwrap();
        let stopped = tracker.stop_session().unwrap().unwrap();
        assert_eq!(stopped.id, session.id);
        assert_eq!(stopped.kills.len(), 1);
        assert_eq!(stopped.dangling_cost, 0.05);
        assert!(!tracker.is_tracking());
        assert!(!rig.bus.has_subscribers(Topic::Combat));

        let end_ts = naive_to_epoch(naive("2026-01-01T00:00:10"));
        assert_eq!(
            rig.scalar_f64(
                "SELECT ended_at FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            end_ts
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT is_active FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            0
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            0.05
        );

        // Ledger gains: the enhancer rebate at full value, the
        // conversion margin at 1%, both rounded half-even to 4.
        assert_eq!(
            rig.scalar_f64(
                "SELECT amount FROM ledger_entries WHERE tag = 'enhancer' \
                 AND description = 'Enhancer Shrapnel Rebate'",
                &[],
            ),
            0.1
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT amount FROM ledger_entries WHERE tag = 'convert' \
                 AND description = 'Shrapnel Conversion'",
                &[],
            ),
            0.005
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM session_summaries WHERE session_id = ?",
                &[&session.id],
            ),
            1
        );

        // Producer events after the stop reach nothing.
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:11",
                    "items": [], "total_ped": 0.0}),
        );
        assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 1);

        // The lifecycle's domain events: started then stopped.
        let updated = updated_events(&captured);
        assert_eq!(updated.len(), 1 + 1);
        assert_eq!(updated[0]["payload"]["reason"], "started");
        assert_eq!(updated[0]["payload"]["status"], "active");
        assert_eq!(updated[0]["occurred_at"], to_iso_utc(start_ts));
        assert_eq!(updated[1]["payload"]["reason"], "stopped");
        assert_eq!(updated[1]["payload"]["status"], "idle");
        assert_eq!(updated[1]["occurred_at"], to_iso_utc(end_ts));
    }

    #[test]
    fn start_while_tracking_stops_the_prior_session() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        let captured = rig.capture();

        let first = tracker.start_session().unwrap();
        rig.clock.advance(5.0).unwrap();
        let second = tracker.start_session().unwrap();
        assert_ne!(first.id, second.id);

        assert_eq!(
            rig.scalar_i64(
                "SELECT is_active FROM tracking_sessions WHERE id = ?",
                &[&first.id],
            ),
            0
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT is_active FROM tracking_sessions WHERE id = ?",
                &[&second.id],
            ),
            1
        );

        // The second start's event order: the prior session's stop
        // lands before the new session's start.
        let topics: Vec<Topic> = captured
            .lock()
            .unwrap()
            .iter()
            .map(|(topic, _)| *topic)
            .collect();
        assert_eq!(
            topics,
            vec![
                Topic::SessionStarted,
                Topic::TrackingSessionUpdated,
                Topic::SessionStopped,
                Topic::TrackingSessionUpdated,
                Topic::SessionStarted,
                Topic::TrackingSessionUpdated,
            ]
        );
    }

    #[test]
    fn recovery_closes_crash_orphaned_sessions() {
        let rig = rig();
        rig.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
             VALUES ('orphan', 1000.0, 1, 'mob')",
        );
        rig.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('k1', 'orphan', 'Atrox', '', '', 1500.0, 3, 30.0, 0.0, 0, \
             0.15, 0.0, 80.0, 0, 0)",
        );
        rig.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 500, 50.0, 0)",
        );
        rig.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 300, 30.0, 1)",
        );
        rig.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES ('k1', 'Rifle', 3, 30.0, 0, 0.05)",
        );
        rig.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('orphan', 1100.0, 'Rifle', 1.0, 0.5)",
        );

        let _tracker = rig.tracker(Providers::default());

        assert_eq!(
            rig.scalar_i64(
                "SELECT is_active FROM tracking_sessions WHERE id = 'orphan'",
                &[],
            ),
            0
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT ended_at FROM tracking_sessions WHERE id = 'orphan'",
                &[],
            ),
            1500.0
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT amount FROM ledger_entries WHERE tag = 'convert'",
                &[],
            ),
            0.5
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT amount FROM ledger_entries WHERE tag = 'enhancer'",
                &[],
            ),
            30.0
        );
        let expected_date = naive_isoformat(epoch_to_naive(1500.0));
        let date: String = rig.runtime.block_on(async {
            sqlx::query("SELECT date FROM ledger_entries WHERE tag = 'convert'")
                .fetch_one(&rig.pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        });
        assert_eq!(date, expected_date);
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM session_summaries WHERE session_id = 'orphan'",
                &[],
            ),
            1
        );
    }

    #[test]
    fn loot_creates_and_persists_kills_with_filtering() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_cost_lookup: Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 }),
            ..Providers::default()
        });
        let session = tracker.start_session().unwrap();
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Rifle"}));
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 30.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "critical_hit", "amount": 10.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "target_dodge", "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_received", "amount": 5.0,
                    "timestamp": "2026-01-01T00:00:01"}),
        );

        rig.bus.publish(
            Topic::LootGroup,
            &json!({
                "type": "loot",
                "timestamp": "2026-01-01T00:00:02",
                "items": [
                    {"item_name": "Animal Hide", "quantity": 1, "value_ped": 4.5,
                     "is_enhancer_shrapnel": false},
                    {"item_name": "Universal Ammo", "quantity": 20, "value_ped": 0.2,
                     "is_enhancer_shrapnel": false},
                    {"item_name": "Shrapnel", "quantity": 10, "value_ped": 0.1,
                     "is_enhancer_shrapnel": true},
                ],
                "total_ped": 4.8,
            }),
        );

        let kill_id: String = rig.runtime.block_on(async {
            sqlx::query("SELECT id FROM kills WHERE session_id = ?")
                .bind(&session.id)
                .fetch_one(&rig.pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        });
        assert_eq!(
            rig.scalar_i64("SELECT shots_fired FROM kills WHERE id = ?", &[&kill_id]),
            3
        );
        assert_eq!(
            rig.scalar_f64("SELECT damage_dealt FROM kills WHERE id = ?", &[&kill_id]),
            40.0
        );
        assert_eq!(
            rig.scalar_f64("SELECT damage_taken FROM kills WHERE id = ?", &[&kill_id]),
            5.0
        );
        assert_eq!(
            rig.scalar_i64("SELECT critical_hits FROM kills WHERE id = ?", &[&kill_id],),
            1
        );
        assert_eq!(
            rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill_id]),
            0.05 * 3.0
        );
        // The blacklisted ammo never lands; the enhancer shrapnel
        // lands as an item but stays out of the loot total.
        assert_eq!(
            rig.scalar_f64("SELECT loot_total_ped FROM kills WHERE id = ?", &[&kill_id],),
            4.5
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM kill_loot_items WHERE kill_id = ?",
                &[&kill_id],
            ),
            2
        );
        let mob: String = rig.runtime.block_on(async {
            sqlx::query("SELECT mob_name FROM kills WHERE id = ?")
                .bind(&kill_id)
                .fetch_one(&rig.pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        });
        assert_eq!(mob, "Unknown");
        assert_eq!(
            rig.scalar_i64(
                "SELECT shots_fired FROM kill_tool_stats WHERE kill_id = ? \
                 AND tool_name = 'Rifle'",
                &[&kill_id],
            ),
            3
        );
        assert_eq!(
            rig.scalar_f64("SELECT timestamp FROM kills WHERE id = ?", &[&kill_id],),
            naive_to_epoch(naive("2026-01-01T00:00:02"))
        );

        // The accumulator reset: an immediate second group carries
        // zero shots.
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:04",
                    "items": [{"item_name": "Mud", "quantity": 1, "value_ped": 0.03,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 0.03}),
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT shots_fired FROM kills WHERE session_id = ? AND id != ?",
                &[&session.id, &kill_id],
            ),
            0
        );
    }

    #[test]
    fn loot_dedup_inside_the_window_only() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        tracker.start_session().unwrap();

        let group = |ts: &str| {
            json!({"type": "loot", "timestamp": ts,
                   "items": [{"item_name": "Animal Hide", "quantity": 1, "value_ped": 1.0,
                              "is_enhancer_shrapnel": false}],
                   "total_ped": 1.0})
        };
        rig.bus
            .publish(Topic::LootGroup, &group("2026-01-01T00:00:02"));
        // Identical fingerprint inside the strict 2s window: dropped.
        rig.bus
            .publish(Topic::LootGroup, &group("2026-01-01T00:00:03"));
        assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 1);
        // Exactly the window: recorded (the comparison is strict).
        rig.bus
            .publish(Topic::LootGroup, &group("2026-01-01T00:00:04"));
        assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 2);
        // A different fingerprint inside the window: recorded.
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:05",
                    "items": [{"item_name": "Mud", "quantity": 1, "value_ped": 1.0,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 1.0}),
        );
        assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 3);

        // A present non-list items payload drops the group entirely
        // (the original raises there, contained: no kill).
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:09",
                    "items": null, "total_ped": 1.0}),
        );
        assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 3);
    }

    #[test]
    fn snapshot_aggregates_and_rounds_the_readout() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_cost_lookup: Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 }),
            player_name: "Hero".to_string(),
            ..Providers::default()
        });

        let idle = tracker.snapshot().unwrap();
        assert!(idle.active.is_none());
        assert_eq!(idle.current_tool, None);

        let session = tracker.start_session().unwrap();
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Rifle"}));
        rig.bus.publish(
            Topic::ActiveHealToolChanged,
            &json!({"tool_name": "FAP", "cost_per_use_ped": 0.02, "reload_seconds": 2.5}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 30.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "critical_hit", "amount": 10.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "target_dodge", "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:02",
                    "items": [{"item_name": "Animal Hide", "quantity": 1, "value_ped": 5.0,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 5.0}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 20.0, "timestamp": "2026-01-01T00:00:03"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:04",
                    "items": [{"item_name": "Mud", "quantity": 1, "value_ped": 0.03,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 0.03}),
        );
        // In-flight accumulator damage after the latest kill.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 7.5, "timestamp": "2026-01-01T00:00:05"}),
        );
        // Two counted heals (the second exactly at the reload bound).
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 12.0, "timestamp": "2026-01-01T00:00:05"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 12.0,
                    "timestamp": "2026-01-01T00:00:07.500000"}),
        );
        // A global correlated to the latest kill.
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "global_kill", "player": "hero", "creature": "Atrox",
                    "value": 12.0, "timestamp": "2026-01-01T00:00:05"}),
        );
        rig.runtime.block_on(async {
            sqlx::query(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, 1.0, 'Rifle', 1.0, 1.0), (?, 2.0, 'Rifle', 1.0, 0.25)",
            )
            .bind(&session.id)
            .bind(&session.id)
            .execute(&rig.pool)
            .await
            .unwrap();
        });

        rig.clock.advance(60.0).unwrap();
        let readout = tracker.snapshot().unwrap();
        assert_eq!(readout.current_tool.as_deref(), Some("Rifle"));
        let active = readout.active.unwrap();
        assert_eq!(active.session_id, session.id);
        assert_eq!(active.started_at, "2026-01-01T00:00:00");
        assert_eq!(active.kill_count, 2);
        assert_eq!(active.elapsed, 60);
        assert_eq!(active.cost, 0.29);
        assert_eq!(active.returns, 5.03);
        assert_eq!(active.pes, 1.25);
        assert_eq!(active.net, 4.74);
        assert_eq!(active.return_rate, 17.3448);
        assert_eq!(active.damage_dealt_total, 60.0);
        assert_eq!(active.weapon_damage_dealt, 67.5);
        assert_eq!(active.weapon_cost, 0.25);
        assert_eq!(active.shots_fired_total, 4);
        assert_eq!(active.critical_hits_total, 1);
        assert_eq!(active.max_damage, 40.0);
        assert_eq!(active.globals_count, 1);
        assert_eq!(active.hofs_count, 0);
        assert_eq!(active.latest_kill_loot, Some(0.03));
        assert_eq!(active.multiplier_last, Some(0.6));
        assert_eq!(active.multiplier_avg, Some(16.9667));
        assert_eq!(active.multiplier_max, Some(33.3333));
        assert_eq!(active.multiplier_history, vec![33.3333, 0.6]);
        assert_eq!(active.cumulative_net_history, vec![4.82, 4.79]);
        assert_eq!(active.current_mob, None);
        assert_eq!(active.mob_source, None);
        assert_eq!(active.mob_entry_mode, "mob");
        assert_eq!(active.notable_event_rows.len(), 1);
        let row = &active.notable_event_rows[0];
        assert_eq!(row.0, "global_kill");
        assert_eq!(row.1, "Atrox");
        assert_eq!(row.2, 12.0);
        assert_eq!(row.3, Some(naive_to_epoch(naive("2026-01-01T00:00:05"))));
        assert!(active.warnings.is_empty());

        // The session heal cost reached the session row on stop.
        tracker.stop_session().unwrap();
        assert_eq!(
            rig.scalar_f64(
                "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
                &[&session.id],
            ),
            0.04
        );
    }

    #[test]
    fn unknown_tool_stats_merge_on_identification() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_cost_lookup: Arc::new(|name| if name == "Pistol" { 0.02 } else { 0.0 }),
            ..Providers::default()
        });
        tracker.start_session().unwrap();

        // Shots before any tool is known accumulate under "Unknown".
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 9.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "critical_hit", "amount": 4.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Pistol"}));
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 6.0, "timestamp": "2026-01-01T00:00:02"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:03",
                    "items": [], "total_ped": 0.0}),
        );

        let rows: Vec<(String, i64, f64, i64, f64)> = rig.runtime.block_on(async {
            sqlx::query(
                "SELECT tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot \
                 FROM kill_tool_stats",
            )
            .fetch_all(&rig.pool)
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row.try_get(0).unwrap(),
                    row.try_get(1).unwrap(),
                    decoded_f64(row, 2),
                    row.try_get(3).unwrap(),
                    decoded_f64(row, 4),
                )
            })
            .collect()
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("Pistol".to_string(), 3, 19.0, 1, 0.02));
    }

    #[test]
    fn phased_tool_stats_split_on_cost_change() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        tracker.start_session().unwrap();

        let mut state = tracker.state.lock().unwrap();
        HuntTracker::tool_stats_for_phase(&mut state, "Rifle", 0.05).shots_fired += 1;
        // Within the tolerance: the same phase.
        HuntTracker::tool_stats_for_phase(&mut state, "Rifle", 0.05 + 1e-12).shots_fired += 1;
        // A real cost change: a second phase keyed `Rifle#2`.
        HuntTracker::tool_stats_for_phase(&mut state, "Rifle", 0.04).shots_fired += 1;
        // A third: `Rifle#3`; a different tool keeps its bare key.
        HuntTracker::tool_stats_for_phase(&mut state, "Rifle", 0.03).shots_fired += 1;
        HuntTracker::tool_stats_for_phase(&mut state, "Pistol", 0.02).shots_fired += 1;
        // A cost difference of exactly the tolerance opens a phase:
        // the comparison is strict (2e-9 - 1e-9 is exactly 1e-9).
        HuntTracker::tool_stats_for_phase(&mut state, "Laser", 1e-9).shots_fired += 1;
        HuntTracker::tool_stats_for_phase(&mut state, "Laser", 2e-9).shots_fired += 1;

        let keys: Vec<(String, String, i64)> = state
            .accumulator
            .as_ref()
            .unwrap()
            .tool_stats
            .iter()
            .map(|(key, stats)| (key.clone(), stats.tool_name.clone(), stats.shots_fired))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("Rifle".to_string(), "Rifle".to_string(), 2),
                ("Rifle#2".to_string(), "Rifle".to_string(), 1),
                ("Rifle#3".to_string(), "Rifle".to_string(), 1),
                ("Pistol".to_string(), "Pistol".to_string(), 1),
                ("Laser".to_string(), "Laser".to_string(), 1),
                ("Laser#2".to_string(), "Laser".to_string(), 1),
            ]
        );
    }

    #[test]
    fn heal_ticks_dedup_by_reload_and_warn_without_tool() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        tracker.start_session().unwrap();

        // No heal tool equipped: the warning lands once, no cost.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 10.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 10.0, "timestamp": "2026-01-01T00:00:09"}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(
                state.session_warnings,
                vec!["Healing detected: no heal tool equipped via hotbar".to_string()]
            );
            assert_eq!(state.session_heal_cost, 0.0);
        }

        rig.bus.publish(
            Topic::ActiveHealToolChanged,
            &json!({"tool_name": "FAP", "cost_per_use_ped": 0.03, "reload_seconds": 5.0}),
        );
        // Counted; then inside the 5s reload window (deduped); then at
        // the bound (counted: the comparison admits equality).
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 10.0, "timestamp": "2026-01-01T00:00:20"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 10.0, "timestamp": "2026-01-01T00:00:24"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 10.0, "timestamp": "2026-01-01T00:00:25"}),
        );
        let state = tracker.state.lock().unwrap();
        assert_eq!(state.session_heal_cost, 0.06);
        assert_eq!(state.session_warnings.len(), 1, "the warning fires once");
    }

    #[test]
    fn globals_correlate_within_the_window() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            player_name: "  Hero  ".to_string(),
            ..Providers::default()
        });
        let session = tracker.start_session().unwrap();

        let loot = |ts: &str, value: f64| {
            json!({"type": "loot", "timestamp": ts,
                   "items": [{"item_name": "Animal Hide", "quantity": 1, "value_ped": value,
                              "is_enhancer_shrapnel": false}],
                   "total_ped": value})
        };
        rig.bus
            .publish(Topic::LootGroup, &loot("2026-01-01T00:00:02", 1.0));
        // The wrong player never lands.
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "global_kill", "player": "Villain", "creature": "Atrox",
                    "value": 8.0, "timestamp": "2026-01-01T00:00:03"}),
        );
        assert_eq!(
            rig.scalar_i64("SELECT COUNT(*) FROM notable_events", &[]),
            0
        );
        // Case-insensitive match (the configured name is stripped at
        // construction); a HoF inside the window tags the kill.
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "hof_kill", "player": "HERO", "creature": "Atrox",
                    "value": 120.0, "timestamp": "2026-01-01T00:00:04"}),
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM kills WHERE is_global = 1 AND is_hof = 1",
                &[],
            ),
            1
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM notable_events WHERE kill_id IS NOT NULL",
                &[],
            ),
            1
        );

        // A stale global (past the 5s window) records the notable
        // event with no kill correlation.
        rig.bus
            .publish(Topic::LootGroup, &loot("2026-01-01T00:00:10", 2.0));
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "global_kill", "player": "Hero", "item": "Rare Thing",
                    "value": 50.0, "timestamp": "2026-01-01T00:00:16"}),
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM notable_events WHERE kill_id IS NULL \
                 AND mob_or_item = 'Rare Thing'",
                &[],
            ),
            1
        );
        assert_eq!(
            rig.scalar_i64("SELECT COUNT(*) FROM kills WHERE is_global = 1", &[]),
            1
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM notable_events WHERE session_id = ?",
                &[&session.id],
            ),
            2
        );

        // An empty configured player name disables correlation.
        let unnamed = rig.tracker(Providers::default());
        unnamed.start_session().unwrap();
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "global_kill", "player": "", "creature": "Atrox",
                    "value": 1.0, "timestamp": "2026-01-01T00:00:20"}),
        );
        assert_eq!(
            rig.scalar_i64("SELECT COUNT(*) FROM notable_events", &[]),
            2
        );
    }

    #[test]
    fn enhancer_breaks_filter_and_deplete_stacks() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_profile_lookup: Arc::new(|name| {
                (name == "Rifle").then(|| {
                    let profile = json!({
                        "damage_enhancers": 2,
                        "weapon_entity": {"name": "Rifle Prime"},
                    });
                    profile.as_object().unwrap().clone()
                })
            }),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Rifle"}));
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(
                state.active_weapon_state_key.as_deref(),
                Some("Rifle Prime")
            );
            assert_eq!(
                state.weapon_enhancer_states["Rifle Prime"].stacks,
                vec![100, 100]
            );
        }

        // A non-damage enhancer never applies; a damage break naming
        // a different item never applies.
        rig.bus.publish(
            Topic::EnhancerBreak,
            &json!({"type": "enhancer_break", "enhancer_name": "Accuracy Enhancer 5",
                    "item_name": "Rifle Prime", "remaining": 150}),
        );
        rig.bus.publish(
            Topic::EnhancerBreak,
            &json!({"type": "enhancer_break", "enhancer_name": "Damage Enhancer 5",
                    "item_name": "Sword", "remaining": 150}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(
                state.weapon_enhancer_states["Rifle Prime"].stacks,
                vec![100, 100]
            );
        }

        // A matching break with a remaining count redistributes,
        // front-loading the remainder; one without decrements the
        // last positive slot. The match admits the observed hotbar
        // spelling and lowercased-alphanumeric containment.
        rig.bus.publish(
            Topic::EnhancerBreak,
            &json!({"type": "enhancer_break", "enhancer_name": "Damage Enhancer 5",
                    "item_name": "rifle-prime", "remaining": 151}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(
                state.weapon_enhancer_states["Rifle Prime"].stacks,
                vec![76, 75]
            );
        }
        rig.bus.publish(
            Topic::EnhancerBreak,
            &json!({"type": "enhancer_break", "enhancer_name": "damage enh",
                    "item_name": "Rifle"}),
        );
        let state = tracker.state.lock().unwrap();
        assert_eq!(
            state.weapon_enhancer_states["Rifle Prime"].stacks,
            vec![76, 74]
        );
    }

    #[test]
    fn damage_enhancer_state_arithmetic() {
        let props = Arc::new(json!({"damage_enhancers": 3.7}));
        let mut state = DamageEnhancerState::from_props("Rifle", props);
        assert_eq!(state.stacks, vec![100, 100, 100], "int() truncates");
        assert_eq!(state.active_slots(), 3);

        state.set_total(7);
        assert_eq!(state.stacks, vec![3, 2, 2], "the remainder front-loads");
        state.set_total(-5);
        assert_eq!(state.stacks, vec![0, 0, 0], "totals clamp at zero");

        state.set_total(2);
        assert_eq!(state.stacks, vec![1, 1, 0]);
        assert_eq!(state.active_slots(), 2);
        // A break with no remaining decrements the last positive slot
        // and reports the depletion.
        assert!(state.apply_break(None));
        assert_eq!(state.stacks, vec![1, 0, 0]);
        assert!(
            state.apply_break(Some(3)),
            "redistribution re-activating slots reports the change"
        );
        assert_eq!(state.stacks, vec![1, 1, 1]);

        let mut slotless = DamageEnhancerState::from_props("Bare", Arc::new(json!({})));
        assert_eq!(slotless.stacks, Vec::<i64>::new());
        assert!(!slotless.apply_break(Some(50)), "no slots, no change");

        let negative =
            DamageEnhancerState::from_props("Neg", Arc::new(json!({"damage_enhancers": -2})));
        assert_eq!(negative.stacks, Vec::<i64>::new());
    }

    #[test]
    fn trifecta_attribution_and_heal_filtering() {
        let rig = rig();
        let trifecta = json!({
            "small_weapon": {"name": "Pistol", "damage_min": 5.0, "damage_max": 10.0,
                             "total_damage": 0.0, "cost_per_shot_ped": 0.05,
                             "role": "small_weapon"},
            "big_weapon": {"name": "Cannon", "damage_min": 20.0, "damage_max": 40.0,
                           "total_damage": 0.0, "cost_per_shot_ped": 0.2,
                           "role": "big_weapon"},
            "heal_tool": {"name": "FAP", "cost_per_use_ped": 0.02, "reload_seconds": 2.5,
                          "heal_min": 10.0, "heal_max": 20.0},
        });
        let tracker = rig.tracker(Providers {
            weapon_attribution_trifecta: Arc::new(|| true),
            trifecta_resolver: Arc::new(move || Some(trifecta.as_object().unwrap().clone())),
            ..Providers::default()
        });
        tracker.start_session().unwrap();

        // Hotbar-driven changes are ignored in trifecta mode.
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Sword"}));
        rig.bus.publish(
            Topic::ActiveHealToolChanged,
            &json!({"tool_name": "Other", "cost_per_use_ped": 9.9}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(state.active_hotbar_tool_name, None);
            assert_eq!(state.active_heal_tool_name.as_deref(), Some("FAP"));
            assert_eq!(state.heal_cost_per_use_ped, 0.02);
        }

        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 7.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        // Unmatched damage warns once and lands under "Unknown".
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 0.5, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 0.5, "timestamp": "2026-01-01T00:00:01"}),
        );
        // A critical inside the big weapon's regular band prefers the
        // big regular explanation.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "critical_hit", "amount": 25.0, "timestamp": "2026-01-01T00:00:02"}),
        );
        // A countered shot attributes to the last offensive tool.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "target_jam", "timestamp": "2026-01-01T00:00:02"}),
        );
        {
            let state = tracker.state.lock().unwrap();
            let stats: Vec<(String, i64, f64)> = state
                .accumulator
                .as_ref()
                .unwrap()
                .tool_stats
                .iter()
                .map(|(key, stats)| (key.clone(), stats.shots_fired, stats.cost_per_shot))
                .collect();
            assert_eq!(
                stats,
                vec![
                    ("Pistol".to_string(), 1, 0.05),
                    ("Unknown".to_string(), 2, 0.0),
                    ("Cannon".to_string(), 2, 0.2),
                ]
            );
            assert_eq!(
                state.session_warnings,
                vec!["Trifecta attribution: damage fell outside both weapon ranges".to_string()]
            );
        }

        // The trifecta heal band filters mismatched heal amounts
        // entirely (no dedup stamp, no cost).
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 50.0, "timestamp": "2026-01-01T00:00:03"}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(state.session_heal_cost, 0.0);
            assert_eq!(state.last_heal_time, None);
        }
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "self_heal", "amount": 15.0, "timestamp": "2026-01-01T00:00:04"}),
        );
        let state = tracker.state.lock().unwrap();
        assert_eq!(state.session_heal_cost, 0.02);
    }

    #[test]
    fn tag_and_manual_mob_rules() {
        let rig = rig();

        // No session: every command refuses.
        let tracker = rig.tracker(Providers::default());
        assert_eq!(
            tracker.set_manual_tag("Foo"),
            Err(TrackerCommandError::NoActiveSession)
        );
        assert_eq!(
            tracker.set_manual_mob("Atrox", "Atrox", "Young"),
            Err(TrackerCommandError::NoActiveSession)
        );

        // Tag mode: the configured tag is stripped and stamps kills;
        // manual mob locking refuses; empty tags refuse.
        let tagged = rig.tracker(Providers {
            mob_tracking_mode: Arc::new(|| "tag".to_string()),
            mob_tracking_tag: Arc::new(|| "  Team Hunt \u{1c}".to_string()),
            ..Providers::default()
        });
        let session = tagged.start_session().unwrap();
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:02",
                    "items": [], "total_ped": 0.0}),
        );
        let mob: String = rig.runtime.block_on(async {
            sqlx::query("SELECT mob_name FROM kills WHERE session_id = ?")
                .bind(&session.id)
                .fetch_one(&rig.pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        });
        assert_eq!(mob, "Team Hunt");
        assert_eq!(
            tagged.set_manual_mob("Atrox", "Atrox", "Young"),
            Err(TrackerCommandError::TagModeLocksMob)
        );
        assert_eq!(
            tagged.set_manual_tag("   "),
            Err(TrackerCommandError::EmptyTag)
        );
        tagged.set_manual_tag(" Solo Run ").unwrap();
        {
            let state = tagged.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Solo Run");
            assert_eq!(state.session_mob_tracking_tag, "Solo Run");
            assert_eq!(state.mob_source, Some("tag"));
        }
        assert_eq!(tagged.release_current_mob().as_deref(), Some("Solo Run"));
        tagged.stop_session().unwrap();

        // Mob mode: the manual provider stamps "<maturity> <species>"
        // at start; tag setting refuses; release clears.
        let manual = rig.tracker(Providers {
            manual_mob: Arc::new(|| Some(("Atrox".to_string(), "Young".to_string()))),
            ..Providers::default()
        });
        manual.start_session().unwrap();
        assert_eq!(
            manual.set_manual_tag("Foo"),
            Err(TrackerCommandError::NotTagMode)
        );
        {
            let state = manual.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Young Atrox");
            assert_eq!(state.confirmed_mob_species, "Atrox");
            assert_eq!(state.confirmed_mob_maturity, "Young");
            assert_eq!(state.mob_source, Some("manual"));
        }
        manual.set_manual_mob("Old Atrox", "Atrox", "Old").unwrap();
        {
            let state = manual.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Old Atrox");
        }
        assert_eq!(manual.release_current_mob().as_deref(), Some("Old Atrox"));
        assert_eq!(manual.release_current_mob(), None);
        manual.stop_session().unwrap();

        // Manual entry disabled: the command refuses; a maturity-less
        // manual mob displays the bare species.
        let disabled = rig.tracker(Providers {
            manual_mob_entry_enabled: Arc::new(|| false),
            ..Providers::default()
        });
        disabled.start_session().unwrap();
        assert_eq!(
            disabled.set_manual_mob("Atrox", "Atrox", ""),
            Err(TrackerCommandError::ManualEntryDisabled)
        );
        disabled.stop_session().unwrap();
        let bare = rig.tracker(Providers {
            manual_mob: Arc::new(|| Some(("Atrox".to_string(), String::new()))),
            ..Providers::default()
        });
        bare.start_session().unwrap();
        {
            let state = bare.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Atrox");
        }
        bare.stop_session().unwrap();
    }

    #[test]
    fn reload_config_transitions_manual_mob_and_heal_state() {
        let rig = rig();
        let scripted_mob: Arc<StdMutex<Option<(String, String)>>> = Arc::new(StdMutex::new(Some(
            ("Atrox".to_string(), "Young".to_string()),
        )));
        let provider_view = scripted_mob.clone();
        let tracker = rig.tracker(Providers {
            manual_mob: Arc::new(move || provider_view.lock().unwrap().clone()),
            ..Providers::default()
        });

        // Idle reload only refreshes the loot filter.
        tracker.reload_config();
        assert!(!tracker.is_tracking());

        tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::ActiveHealToolChanged,
            &json!({"tool_name": "FAP", "cost_per_use_ped": 0.03, "reload_seconds": 5.0}),
        );
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Young Atrox");
            assert_eq!(state.heal_cost_per_use_ped, 0.03);
        }

        // The provider switching mobs re-stamps; switching to None
        // clears a manual stamp; the non-trifecta branch resets the
        // heal scalars.
        *scripted_mob.lock().unwrap() = Some(("Feffoid".to_string(), String::new()));
        tracker.reload_config();
        {
            let state = tracker.state.lock().unwrap();
            assert_eq!(state.confirmed_mob_name, "Feffoid");
            assert_eq!(state.heal_cost_per_use_ped, 0.0);
            assert_eq!(state.heal_reload_seconds, 2.5);
        }
        *scripted_mob.lock().unwrap() = None;
        tracker.reload_config();
        let state = tracker.state.lock().unwrap();
        assert_eq!(state.confirmed_mob_name, "");
        assert_eq!(state.mob_source, None);
    }

    #[test]
    fn tick_flushed_coalesces_dirty_mutations() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        let captured = rig.capture();
        let session = tracker.start_session().unwrap();

        // A clean tick wakes nothing.
        rig.bus.publish(
            Topic::TickFlushed,
            &json!({"timestamp": "2026-01-01T00:00:01"}),
        );
        assert_eq!(updated_events(&captured).len(), 1, "only the start event");

        // A mutating event then a tick: one update stamped with the
        // tick's own instant.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 5.0, "timestamp": "2026-01-01T00:00:02"}),
        );
        rig.bus.publish(
            Topic::TickFlushed,
            &json!({"timestamp": "2026-01-01T00:00:02"}),
        );
        let events = updated_events(&captured);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            json!({
                "type": "tracking.session.updated",
                "event_version": 1,
                "occurred_at": to_iso_utc(naive_to_epoch(naive("2026-01-01T00:00:02"))),
                "payload": {"sessionId": session.id, "status": "active", "reason": "updated"},
            })
        );

        // The dirty flag resets: the next tick is silent again.
        rig.bus.publish(
            Topic::TickFlushed,
            &json!({"timestamp": "2026-01-01T00:00:03"}),
        );
        assert_eq!(updated_events(&captured).len(), 2);

        // A numeric tick timestamp passes straight through; a null
        // one falls back to the injected clock.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 5.0, "timestamp": "2026-01-01T00:00:04"}),
        );
        rig.bus
            .publish(Topic::TickFlushed, &json!({"timestamp": 1735680000.0}));
        let events = updated_events(&captured);
        assert_eq!(events[2]["occurred_at"], "2024-12-31T21:20:00+00:00");

        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 5.0, "timestamp": "2026-01-01T00:00:05"}),
        );
        rig.bus
            .publish(Topic::TickFlushed, &json!({"timestamp": null}));
        let events = updated_events(&captured);
        assert_eq!(
            events[3]["occurred_at"],
            to_iso_utc(naive_to_epoch(naive("2026-01-01T00:00:00"))),
            "the frozen mock clock stamps the fallback"
        );

        // An unparseable timestamp drops the event (the original's
        // float() raise, contained) with the dirty flag consumed; a
        // numeric string passes through float() instead.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 5.0, "timestamp": "2026-01-01T00:00:06"}),
        );
        rig.bus
            .publish(Topic::TickFlushed, &json!({"timestamp": "garbage"}));
        assert_eq!(updated_events(&captured).len(), 4);
        rig.bus.publish(
            Topic::TickFlushed,
            &json!({"timestamp": "2026-01-01T00:00:07"}),
        );
        assert_eq!(
            updated_events(&captured).len(),
            4,
            "the dropped event consumed the dirty flag"
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 5.0, "timestamp": "2026-01-01T00:00:08"}),
        );
        rig.bus
            .publish(Topic::TickFlushed, &json!({"timestamp": "1735680000.5"}));
        let events = updated_events(&captured);
        assert_eq!(events[4]["occurred_at"], "2024-12-31T21:20:00.500000+00:00");
    }

    #[test]
    fn session_event_wire_shape_matches_the_python_model_dump() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        let captured = rig.capture();
        let session = tracker.start_session().unwrap();

        let events = updated_events(&captured);
        let start_ts = naive_to_epoch(naive("2026-01-01T00:00:00"));
        assert_eq!(
            events[0],
            json!({
                "type": "tracking.session.updated",
                "event_version": 1,
                "occurred_at": to_iso_utc(start_ts),
                "payload": {"sessionId": session.id, "status": "active", "reason": "started"},
            })
        );
        let captured_topics: Vec<Topic> = captured
            .lock()
            .unwrap()
            .iter()
            .map(|(topic, _)| *topic)
            .collect();
        assert!(captured_topics.contains(&Topic::TrackingSessionUpdated));
    }

    #[test]
    fn helper_pins() {
        assert_eq!(to_iso_utc(1735680000.0), "2024-12-31T21:20:00+00:00");
        assert_eq!(to_iso_utc(1735680000.5), "2024-12-31T21:20:00.500000+00:00");

        let whole = naive("2026-01-01T00:00:05");
        assert_eq!(naive_isoformat(whole), "2026-01-01T00:00:05");
        let fractional =
            NaiveDateTime::parse_from_str("2026-01-01T00:00:05.250000", "%Y-%m-%dT%H:%M:%S%.f")
                .unwrap();
        assert_eq!(naive_isoformat(fractional), "2026-01-01T00:00:05.250000");

        assert_eq!(
            parse_bus_timestamp(Some(&json!("2026-01-01T00:00:05"))),
            Some(whole)
        );
        assert_eq!(
            parse_bus_timestamp(Some(&json!("2026-01-01T00:00:05.5"))),
            NaiveDateTime::parse_from_str("2026-01-01T00:00:05.5", "%Y-%m-%dT%H:%M:%S%.f").ok()
        );
        assert_eq!(parse_bus_timestamp(Some(&json!("garbage"))), None);
        assert_eq!(parse_bus_timestamp(Some(&json!(12.5))), None);
        assert_eq!(parse_bus_timestamp(None), None);

        let delta = naive("2026-01-01T00:00:05") - naive("2026-01-01T00:00:02");
        assert_eq!(python_total_seconds(delta), 3.0);
        let negative = naive("2026-01-01T00:00:02") - naive("2026-01-01T00:00:05");
        assert_eq!(python_total_seconds(negative), -3.0);

        // The naive epoch round-trip holds in the host zone.
        let instant = naive("2026-06-15T12:30:45");
        assert_eq!(epoch_to_naive(naive_to_epoch(instant)), instant);

        assert!(value_truthy(&json!(true)));
        assert!(value_truthy(&json!(1.5)));
        assert!(value_truthy(&json!("x")));
        assert!(value_truthy(&json!([0])));
        assert!(value_truthy(&json!({"k": 0})));
        assert!(!value_truthy(&json!(null)));
        assert!(!value_truthy(&json!(false)));
        assert!(!value_truthy(&json!(0)));
        assert!(!value_truthy(&json!("")));
        assert!(!value_truthy(&json!([])));
        assert!(!value_truthy(&json!({})));
    }
    #[test]
    fn snapshot_prices_enhancer_cost_and_skips_costless_multipliers() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        tracker.start_session().unwrap();

        let loot = |ts: &str, name: &str, value: f64| {
            json!({"type": "loot", "timestamp": ts,
                   "items": [{"item_name": name, "quantity": 1, "value_ped": value,
                              "is_enhancer_shrapnel": false}],
                   "total_ped": value})
        };
        rig.bus
            .publish(Topic::LootGroup, &loot("2026-01-01T00:00:02", "Hide", 2.0));
        let readout = tracker.snapshot().unwrap();
        let active = readout.active.unwrap();
        // A costless kill: no rate, no multipliers (a >= admission
        // would divide by zero into infinities).
        assert_eq!(active.cost, 0.0);
        assert_eq!(active.return_rate, 0.0);
        assert_eq!(active.multiplier_last, None);
        assert_eq!(active.multiplier_avg, None);
        assert_eq!(active.multiplier_max, None);
        assert!(active.multiplier_history.is_empty());

        // Enhancer cost flows from the accumulator into the kill and
        // the live readout arithmetic.
        tracker
            .lock_state()
            .accumulator
            .as_mut()
            .unwrap()
            .enhancer_cost = 0.25;
        rig.bus
            .publish(Topic::LootGroup, &loot("2026-01-01T00:00:05", "Mud", 1.0));
        tracker
            .lock_state()
            .accumulator
            .as_mut()
            .unwrap()
            .enhancer_cost = 0.5;
        let active = tracker.snapshot().unwrap().active.unwrap();
        assert_eq!(active.cost, 0.75);
        assert_eq!(active.returns, 3.0);
        assert_eq!(active.net, 2.25);
        assert_eq!(active.return_rate, 4.0);
        assert_eq!(active.cumulative_net_history, vec![2.0, 2.75]);

        // The unresolved enhancer cost is the dangling remainder.
        let stopped = tracker.stop_session().unwrap().unwrap();
        assert_eq!(stopped.dangling_cost, 0.5);
    }

    #[test]
    fn inferred_cost_outranks_the_equipment_lookup() {
        let rig = rig();
        let trifecta = json!({
            "small_weapon": {"name": "Pistol", "damage_min": 5.0, "damage_max": 10.0,
                             "total_damage": 0.0, "cost_per_shot_ped": 0.05,
                             "role": "small_weapon"},
            "big_weapon": {"name": "Cannon", "damage_min": 20.0, "damage_max": 40.0,
                           "total_damage": 0.0, "cost_per_shot_ped": 0.2,
                           "role": "big_weapon"},
        });
        let tracker = rig.tracker(Providers {
            weapon_attribution_trifecta: Arc::new(|| true),
            trifecta_resolver: Arc::new(move || Some(trifecta.as_object().unwrap().clone())),
            equipment_cost_lookup: Arc::new(|_| 0.9),
            ..Providers::default()
        });
        tracker.start_session().unwrap();

        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 7.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "critical_hit", "amount": 25.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        // The countered shot carries no inferred cost, so the static
        // equipment cost prices it: a new phase of the last tool.
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "target_jam", "timestamp": "2026-01-01T00:00:02"}),
        );
        let state = tracker.lock_state();
        let stats: Vec<(String, f64, i64)> = state
            .accumulator
            .as_ref()
            .unwrap()
            .tool_stats
            .iter()
            .map(|(key, stats)| (key.clone(), stats.cost_per_shot, stats.shots_fired))
            .collect();
        assert_eq!(
            stats,
            vec![
                ("Pistol".to_string(), 0.05, 1),
                ("Cannon".to_string(), 0.2, 1),
                ("Cannon#2".to_string(), 0.9, 1),
            ]
        );
    }

    #[test]
    fn the_unknown_entry_backfills_its_cost_once() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_cost_lookup: Arc::new(|_| 0.7),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 9.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 6.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:02",
                    "items": [], "total_ped": 0.0}),
        );
        let kill_id: String = rig.runtime.block_on(async {
            sqlx::query("SELECT id FROM kills")
                .fetch_one(&rig.pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap()
        });
        assert_eq!(
            rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill_id]),
            1.4
        );
        assert_eq!(
            rig.scalar_f64(
                "SELECT cost_per_shot FROM kill_tool_stats WHERE kill_id = ? \
                 AND tool_name = 'Unknown'",
                &[&kill_id],
            ),
            0.7
        );
    }

    #[test]
    fn a_costless_tool_merges_unknown_into_its_bare_entry() {
        let rig = rig();
        let tracker = rig.tracker(Providers::default());
        tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 9.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "Stick"}));
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 6.0, "timestamp": "2026-01-01T00:00:02"}),
        );
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:03",
                    "items": [], "total_ped": 0.0}),
        );
        let rows: Vec<(String, i64, f64)> = rig.runtime.block_on(async {
            sqlx::query("SELECT tool_name, shots_fired, damage_dealt FROM kill_tool_stats")
                .fetch_all(&rig.pool)
                .await
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row.try_get(0).unwrap(),
                        row.try_get(1).unwrap(),
                        decoded_f64(row, 2),
                    )
                })
                .collect()
        });
        assert_eq!(rows, vec![("Stick".to_string(), 2, 15.0)]);
    }

    #[test]
    fn break_matching_admits_every_containment_direction() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            equipment_profile_lookup: Arc::new(|name| {
                (name == "MyGun").then(|| {
                    json!({"damage_enhancers": 1, "weapon_entity": {"name": "Blast Master"}})
                        .as_object()
                        .unwrap()
                        .clone()
                })
            }),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        rig.bus
            .publish(Topic::ActiveToolChanged, &json!({"tool_name": "MyGun"}));

        let break_event = |item: &str| {
            json!({"type": "enhancer_break", "enhancer_name": "Damage Enhancer 5",
                   "item_name": item})
        };
        let stacks = |tracker: &HuntTracker| {
            tracker.lock_state().weapon_enhancer_states["Blast Master"]
                .stacks
                .clone()
        };
        // The canonical name contains the item; the item contains the
        // canonical name; the observed hotbar name contains the item;
        // the item contains the observed name. Each direction matches.
        rig.bus.publish(Topic::EnhancerBreak, &break_event("Blast"));
        assert_eq!(stacks(&tracker), vec![99]);
        rig.bus
            .publish(Topic::EnhancerBreak, &break_event("Blast Master Deluxe"));
        assert_eq!(stacks(&tracker), vec![98]);
        rig.bus.publish(Topic::EnhancerBreak, &break_event("Gun"));
        assert_eq!(stacks(&tracker), vec![97]);
        rig.bus
            .publish(Topic::EnhancerBreak, &break_event("MyGun Deluxe"));
        assert_eq!(stacks(&tracker), vec![96]);
        // No containment in any direction: ignored.
        rig.bus.publish(Topic::EnhancerBreak, &break_event("Sword"));
        assert_eq!(stacks(&tracker), vec![96]);

        // Stopping the session clears the weapon runtime wholesale.
        tracker.stop_session().unwrap();
        let state = tracker.lock_state();
        assert_eq!(state.active_weapon_state_key, None);
        assert!(state.weapon_enhancer_states.is_empty());
        assert_eq!(state.active_weapon_observed_name, None);
    }

    #[test]
    fn recovery_zero_timestamp_kills_fall_back_to_the_start() {
        let rig = rig();
        rig.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
             VALUES ('orphan2', 2000.0, 1, 'mob')",
        );
        rig.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('kz', 'orphan2', 'Atrox', '', '', 0.0, 1, 1.0, 0.0, 0, \
             0.1, 0.0, 1.0, 0, 0)",
        );
        let _tracker = rig.tracker(Providers::default());
        assert_eq!(
            rig.scalar_f64(
                "SELECT ended_at FROM tracking_sessions WHERE id = 'orphan2'",
                &[],
            ),
            2000.0,
            "a zero kill timestamp is falsy there, not a real maximum"
        );
    }

    #[test]
    fn reload_config_in_tag_mode_never_consults_the_manual_provider() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            mob_tracking_mode: Arc::new(|| "tag".to_string()),
            mob_tracking_tag: Arc::new(|| "Team".to_string()),
            manual_mob: Arc::new(|| Some(("Atrox".to_string(), "Young".to_string()))),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        tracker.reload_config();
        let state = tracker.lock_state();
        assert_eq!(state.confirmed_mob_name, "Team");
        assert_eq!(state.mob_source, Some("tag"));
    }

    #[test]
    fn the_session_tag_stamps_only_in_tag_mode_with_a_real_tag() {
        let rig = rig();
        // A configured tag outside tag mode never stamps.
        let mob_mode = rig.tracker(Providers {
            mob_tracking_tag: Arc::new(|| "Sneaky".to_string()),
            manual_mob_entry_enabled: Arc::new(|| false),
            ..Providers::default()
        });
        mob_mode.start_session().unwrap();
        {
            let state = mob_mode.lock_state();
            assert_eq!(state.confirmed_mob_name, "");
            assert_eq!(state.mob_source, None);
        }
        mob_mode.stop_session().unwrap();

        // Tag mode with an all-blank tag has nothing to stamp.
        let blank = rig.tracker(Providers {
            mob_tracking_mode: Arc::new(|| "tag".to_string()),
            mob_tracking_tag: Arc::new(|| "   ".to_string()),
            ..Providers::default()
        });
        blank.start_session().unwrap();
        let state = blank.lock_state();
        assert_eq!(state.confirmed_mob_name, "");
        assert_eq!(state.mob_source, None);
    }

    #[test]
    fn the_blacklist_provider_refreshes_at_session_start() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            loot_filter_blacklist_provider: Some(Arc::new(|| vec!["Mud".to_string()])),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:02",
                    "items": [{"item_name": "Mud", "quantity": 1, "value_ped": 1.0,
                               "is_enhancer_shrapnel": false},
                              {"item_name": "Hide", "quantity": 1, "value_ped": 2.0,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 3.0}),
        );
        assert_eq!(
            rig.scalar_f64("SELECT loot_total_ped FROM kills", &[]),
            2.0,
            "the provider's blacklist drops Mud"
        );
        assert_eq!(
            rig.scalar_i64("SELECT COUNT(*) FROM kill_loot_items", &[]),
            1
        );
    }

    #[test]
    fn command_error_messages_match_the_original() {
        assert_eq!(
            TrackerCommandError::NoActiveSession.to_string(),
            "No active session"
        );
        assert_eq!(
            TrackerCommandError::NotTagMode.to_string(),
            "Active session is not in tag mode"
        );
        assert_eq!(
            TrackerCommandError::EmptyTag.to_string(),
            "Tag cannot be empty"
        );
        assert_eq!(
            TrackerCommandError::TagModeLocksMob.to_string(),
            "Tag mode sessions do not allow manual mob locking"
        );
        assert_eq!(
            TrackerCommandError::ManualEntryDisabled.to_string(),
            "Manual mob entry is not enabled for this session"
        );
    }

    #[test]
    fn enhancer_state_prices_through_the_cost_engine() {
        let props: Arc<Value> = Arc::new(json!({
            "weapon_entity": {"economy": {"decay": 0.05, "ammo_burn": 200}},
            "damage_enhancers": 2,
        }));
        let mut state = DamageEnhancerState::from_props("Rifle", props.clone());
        let priced = |slots: i64| {
            cost_per_shot_from_props(&props, Some(slots))["totalCostPerUse"]
                .as_f64()
                .unwrap()
                / 100.0
        };
        let two_slots = priced(2);
        assert!(two_slots > 0.0);
        assert_eq!(state.current_cost_ped(), two_slots);
        assert_eq!(
            state.current_cost_ped(),
            two_slots,
            "the cached read agrees"
        );
        state.set_total(1);
        assert_eq!(
            state.current_cost_ped(),
            priced(1),
            "a stack change reprices at the new active count"
        );
    }

    #[test]
    fn epoch_helpers_carry_and_keep_fractions() {
        assert_eq!(epoch_to_parts(5.0), (5, 0));
        assert_eq!(epoch_to_parts(2.25), (2, 250_000));
        assert_eq!(
            epoch_to_parts(1.999_999_9),
            (2, 0),
            "microsecond round-up carries into the seconds"
        );
        assert_eq!(
            epoch_to_parts(-0.25),
            (-1, 750_000),
            "negative fractions borrow a second"
        );

        let base = naive("2026-06-15T12:30:45");
        let fractional =
            NaiveDateTime::parse_from_str("2026-06-15T12:30:45.250000", "%Y-%m-%dT%H:%M:%S%.f")
                .unwrap();
        let delta = naive_to_epoch(fractional) - naive_to_epoch(base);
        assert!((delta - 0.25).abs() < 1e-9);
        assert_eq!(epoch_to_naive(naive_to_epoch(fractional)), fractional);
    }
    #[test]
    fn a_zero_priced_weapon_state_still_prefers_the_inferred_cost() {
        let rig = rig();
        let trifecta = json!({
            "small_weapon": {"name": "Pistol", "damage_min": 5.0, "damage_max": 10.0,
                             "total_damage": 0.0, "cost_per_shot_ped": 0.05,
                             "role": "small_weapon",
                             "weapon_props": {"weapon_entity": {"economy": {
                                 "decay": 0, "ammo_burn": 0}}}},
        });
        let tracker = rig.tracker(Providers {
            weapon_attribution_trifecta: Arc::new(|| true),
            trifecta_resolver: Arc::new(move || Some(trifecta.as_object().unwrap().clone())),
            equipment_cost_lookup: Arc::new(|_| 0.3),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::Combat,
            &json!({"type": "damage_dealt", "amount": 7.0, "timestamp": "2026-01-01T00:00:01"}),
        );
        let state = tracker.lock_state();
        let (key, stats) = &state.accumulator.as_ref().unwrap().tool_stats[0];
        assert_eq!(key, "Pistol");
        assert_eq!(
            stats.cost_per_shot, 0.05,
            "the attribution's cost backfills ahead of the equipment lookup"
        );
    }

    #[test]
    fn a_global_at_the_exact_window_bound_is_not_correlated() {
        let rig = rig();
        let tracker = rig.tracker(Providers {
            player_name: "Hero".to_string(),
            ..Providers::default()
        });
        let session = tracker.start_session().unwrap();
        rig.bus.publish(
            Topic::LootGroup,
            &json!({"type": "loot", "timestamp": "2026-01-01T00:00:20",
                    "items": [{"item_name": "Hide", "quantity": 1, "value_ped": 1.0,
                               "is_enhancer_shrapnel": false}],
                    "total_ped": 1.0}),
        );
        rig.bus.publish(
            Topic::Global,
            &json!({"type": "global_kill", "player": "Hero", "creature": "Atrox",
                    "value": 9.0, "timestamp": "2026-01-01T00:00:25"}),
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM kills WHERE session_id = ? AND is_global = 1",
                &[&session.id],
            ),
            0,
            "the five-second window is strict"
        );
        assert_eq!(
            rig.scalar_i64(
                "SELECT COUNT(*) FROM notable_events WHERE session_id = ? \
                 AND kill_id IS NULL",
                &[&session.id],
            ),
            1
        );
    }

    #[test]
    fn reload_clears_a_manual_stamp_once_entry_disables() {
        let rig = rig();
        let enabled = Arc::new(StdMutex::new(true));
        let provider_view = enabled.clone();
        let tracker = rig.tracker(Providers {
            manual_mob_entry_enabled: Arc::new(move || *provider_view.lock().unwrap()),
            manual_mob: Arc::new(|| Some(("Atrox".to_string(), "Young".to_string()))),
            ..Providers::default()
        });
        tracker.start_session().unwrap();
        assert_eq!(tracker.lock_state().confirmed_mob_name, "Young Atrox");

        *enabled.lock().unwrap() = false;
        tracker.reload_config();
        let state = tracker.lock_state();
        assert_eq!(state.confirmed_mob_name, "");
        assert_eq!(state.mob_source, None);
    }
}
