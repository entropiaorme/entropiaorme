"""Hunt tracker — central coordinator for the tracking engine.

Subscribes to event bus, accumulates combat stats, creates kill records
on loot events, persists to SQLite.

Kills model: shots accumulate with cost. When a loot group arrives,
that's a kill — snapshot the accumulator, stamp the configured mob/tag, persist,
reset. Deaths are invisible (shots keep accumulating). Session ends
with unresolved shots → dangling cost.
"""

from __future__ import annotations

import logging
import sqlite3
import sys
import time as _time
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Literal

from backend.core.domain_events import (
    TOPIC_TRACKING_SESSION_UPDATED,
    TrackingSessionUpdated,
    TrackingSessionUpdatedPayload,
    to_iso_utc,
)
from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
    EVENT_TICK_FLUSHED,
)
from backend.services.cost_engine import cost_per_shot_from_props
from backend.tracking.loot_filter import is_tracked_loot, normalize_blacklist
from backend.tracking.models import Kill, LootItem, ToolStats, TrackingSession
from backend.tracking.schema import init_tracking_tables
from backend.tracking.tool_inference import DamageAttributor

log = logging.getLogger(__name__)


@dataclass
class _Accumulator:
    """Tracks combat stats since the last kill (or session start)."""

    shots_fired: int = 0
    damage_dealt: float = 0.0
    damage_taken: float = 0.0
    critical_hits: int = 0
    enhancer_cost: float = 0.0
    tool_stats: dict[str, ToolStats] = field(default_factory=dict)

    def reset(self) -> None:
        self.shots_fired = 0
        self.damage_dealt = 0.0
        self.damage_taken = 0.0
        self.critical_hits = 0
        self.enhancer_cost = 0.0
        self.tool_stats = {}

    @property
    def weapon_cost(self) -> float:
        return sum(ts.cost_per_shot * ts.shots_fired for ts in self.tool_stats.values())

    @property
    def total_cost(self) -> float:
        return self.weapon_cost + self.enhancer_cost


@dataclass
class _DamageEnhancerState:
    """Per-weapon damage-enhancer state within the current session."""

    tool_name: str
    props: dict
    stacks: list[int] = field(default_factory=list)
    _cached_cost_ped: float | None = field(default=None, init=False, repr=False)

    @classmethod
    def from_props(cls, tool_name: str, props: dict) -> _DamageEnhancerState:
        configured = max(0, int(props.get("damage_enhancers", 0) or 0))
        return cls(tool_name=tool_name, props=props, stacks=[100] * configured)

    @property
    def active_slots(self) -> int:
        return sum(1 for stack in self.stacks if stack > 0)

    def set_total(self, total: int) -> None:
        total = max(0, total)
        slot_count = len(self.stacks)
        if slot_count == 0:
            return
        per_slot = total // slot_count
        remainder = total % slot_count
        self.stacks = [
            per_slot + (1 if idx < remainder else 0) for idx in range(slot_count)
        ]
        self._cached_cost_ped = None

    def apply_break(self, remaining: int | None = None) -> bool:
        """Apply one break and return True when a slot is fully depleted."""
        old_active = self.active_slots
        if remaining is not None and self.stacks:
            self.set_total(remaining)
        else:
            for idx in range(len(self.stacks) - 1, -1, -1):
                if self.stacks[idx] > 0:
                    self.stacks[idx] -= 1
                    self._cached_cost_ped = None
                    break
        return old_active != self.active_slots

    def current_cost_ped(self) -> float:
        if self._cached_cost_ped is None:
            self._cached_cost_ped = (
                cost_per_shot_from_props(
                    self.props,
                    damage_enhancers=self.active_slots,
                )["totalCostPerUse"]
                / 100.0
            )
        return self._cached_cost_ped


class HuntTracker:
    """Central tracking coordinator.

    Subscribes to chat.log and hotbar events on the event bus, accumulates
    combat stats, creates kill records on loot events, persists to SQLite.
    """

    LOOT_DEDUP_WINDOW = timedelta(seconds=2)

    def __init__(
        self,
        event_bus: EventBus,
        db_conn: sqlite3.Connection,
        equipment_cost_lookup: Callable[[str], float] | None = None,
        equipment_profile_lookup: Callable[[str], dict | None] | None = None,
        player_name: str = "",
        enhancer_tt_lookup: Callable[[str], float] | None = None,
        loot_filter_blacklist: list[str] | None = None,
        loot_filter_blacklist_provider: Callable[[], list[str]] | None = None,
        weapon_attribution_trifecta_provider: Callable[[], bool] | None = None,
        mob_tracking_mode_provider: Callable[[], str] | None = None,
        mob_tracking_tag_provider: Callable[[], str] | None = None,
        manual_mob_entry_enabled_provider: Callable[[], bool] | None = None,
        manual_mob_provider: Callable[[], tuple[str, str] | None] | None = None,
        trifecta_resolver: Callable[[], dict | None] | None = None,
        now_fn: Callable[[], datetime] | None = None,
    ):
        self._event_bus = event_bus
        self._db = db_conn
        # Time source for session-boundary timestamps. Injected so replay
        # tests can drive deterministic session start/stop instants; defaults
        # to wall-clock time, leaving production behaviour unchanged.
        self._now_fn = now_fn or (lambda: datetime.now(tz=None))
        self._equipment_cost_lookup = equipment_cost_lookup or (lambda _: 0.0)
        self._equipment_profile_lookup = equipment_profile_lookup or (lambda _: None)
        self._player_name = player_name.strip()
        self._enhancer_tt_lookup = enhancer_tt_lookup
        self._loot_filter_blacklist = list(loot_filter_blacklist or [])
        self._loot_filter_blacklist_provider = loot_filter_blacklist_provider
        self._weapon_attribution_trifecta_provider = (
            weapon_attribution_trifecta_provider or (lambda: False)
        )
        self._mob_tracking_mode_provider = mob_tracking_mode_provider or (lambda: "mob")
        self._mob_tracking_tag_provider = mob_tracking_tag_provider or (lambda: "")
        self._manual_mob_entry_enabled_provider = manual_mob_entry_enabled_provider or (
            lambda: True
        )
        self._manual_mob_provider = manual_mob_provider or (lambda: None)
        self._trifecta_resolver = trifecta_resolver or (lambda: None)
        self._damage_attributor = DamageAttributor()
        self._profile_match_cache: dict[str, tuple[str, dict] | None] = {}
        self._static_tool_cost_cache: dict[str, float] = {}

        self._active_hotbar_tool_name: str | None = None
        self._active_heal_tool_name: str | None = None
        self._heal_cost_per_use_ped: float = 0.0
        self._heal_reload_seconds: float = 2.5
        self._heal_amount_min: float | None = None
        self._heal_amount_max: float | None = None
        self._trifecta_weapon_profiles: dict[str, dict] = {}
        self._weapon_enhancer_states: dict[str, _DamageEnhancerState] = {}
        self._active_weapon_state_key: str | None = None
        self._active_weapon_observed_name: str | None = None
        self._last_offensive_tool_name: str | None = None

        # Ensure tracking tables exist
        init_tracking_tables(self._db)

        # Recover any sessions left open by a crash
        self._recover_orphaned_sessions()

        # Session state
        self._session: TrackingSession | None = None
        self._accumulator: _Accumulator | None = None
        # Set by the P&L handlers when a tick mutates the live session readout,
        # cleared when the coalesced tracking.session.updated is emitted at the
        # settled-tick boundary. Lets one domain event stand for a tick's worth
        # of low-level mutations rather than one per raw mutation.
        self._session_dirty = False
        self._loot_blacklist = normalize_blacklist(loot_filter_blacklist)
        self._refresh_loot_filter()

        # Current configured mob/tag snapshot for kill stamping.
        self._current_mob_name: str = ""
        self._current_mob_species: str = ""
        self._current_mob_maturity: str = ""

        # Confirmed mob/tag. Kills use this for stamping.
        self._confirmed_mob_name: str = ""
        self._confirmed_mob_species: str = ""
        self._confirmed_mob_maturity: str = ""
        self._mob_source: str | None = None  # "manual" | "tag"
        self._session_mob_tracking_mode: str = "mob"
        self._session_mob_tracking_tag: str = ""

        # Heal deduplication: a single tool activation produces multiple chat.log ticks
        self._last_heal_time: datetime | None = None

        # Loot dedup
        self._last_loot_fingerprint: tuple | None = None
        self._last_loot_time: datetime | None = None

        # For global/HoF correlation
        self._last_kill: Kill | None = None
        self._trifecta_unmatched_warning_emitted = False
        self._perf_window_started = _time.monotonic()
        self._perf_shot_count = 0
        self._perf_unknown_tool_shots = 0
        self._perf_inference_misses = 0
        self._perf_shot_seconds = 0.0
        self._perf_cost_lookup_seconds = 0.0

        # Demo-mode priming hook: env-var-gated, dev-only. Production
        # bundles include demo_seed for the /demo/* router's parallel
        # HuntTracker, but this real-tracker hook stays gated off in
        # frozen builds so an end user setting ENTROPIAORME_DEMO_SCENARIO
        # cannot prime their real tracker.
        if not getattr(sys, "frozen", False):
            try:
                from backend.scripts.demo_seed.live_injection import (
                    maybe_prime_tracker_from_env,
                )

                maybe_prime_tracker_from_env(self)
            except ImportError:
                pass

    # ------------------------------------------------------------------
    # Startup recovery
    # ------------------------------------------------------------------

    def _recover_orphaned_sessions(self) -> None:
        """Close sessions left open by a crash (is_active=1 with no in-memory state).

        For each orphaned session:
        - Set ended_at to the latest kill timestamp, or started_at if none
        - Compute heal_cost from session-level (already persisted on session row)
        - Create shrapnel ledger entry
        - Mark is_active=0
        """
        rows = self._db.execute(
            "SELECT id, started_at FROM tracking_sessions WHERE is_active = 1"
        ).fetchall()
        if not rows:
            return

        for row in rows:
            session_id = row[0]
            started_at = row[1]

            # Compute end time from latest kill, fall back to session start
            kill_row = self._db.execute(
                "SELECT MAX(timestamp) FROM kills WHERE session_id = ?",
                (session_id,),
            ).fetchone()
            ended_at = kill_row[0] if kill_row and kill_row[0] else started_at

            # Close the session
            self._db.execute(
                "UPDATE tracking_sessions SET ended_at = ?, is_active = 0 WHERE id = ?",
                (ended_at, session_id),
            )

            # Create shrapnel ledger entry (same logic as normal stop)
            end_dt = datetime.fromtimestamp(ended_at)
            self._create_enhancer_rebate_ledger_entry(session_id, end_dt)
            self._create_shrapnel_ledger_entry(session_id, end_dt)

            from backend.services.session_summary import write_session_summary

            write_session_summary(self._db, session_id)

            self._db.commit()

            # Count how many kills were preserved
            count_row = self._db.execute(
                "SELECT COUNT(*) FROM kills WHERE session_id = ?",
                (session_id,),
            ).fetchone()
            kill_count = count_row[0] if count_row else 0

            log.warning(
                "Recovered orphaned session %s: %d kills preserved, "
                "in-progress accumulator at crash time was lost",
                session_id[:8],
                kill_count,
            )

    # ------------------------------------------------------------------
    # Session lifecycle
    # ------------------------------------------------------------------

    @property
    def is_tracking(self) -> bool:
        return self._session is not None

    @property
    def session(self) -> TrackingSession | None:
        return self._session

    @property
    def current_accumulator(self) -> _Accumulator | None:
        return self._accumulator

    def _is_weapon_attribution_trifecta(self) -> bool:
        return self._weapon_attribution_trifecta_provider()

    def _is_manual_mob_entry_enabled(self) -> bool:
        return self._manual_mob_entry_enabled_provider()

    def is_session_tag_mode(self) -> bool:
        return self._session_mob_tracking_mode == "tag"

    def _reset_weapon_runtime_state(self) -> None:
        self._trifecta_weapon_profiles = {}
        self._weapon_enhancer_states = {}
        self._active_weapon_state_key = None
        self._active_weapon_observed_name = None
        self._last_offensive_tool_name = None
        self._profile_match_cache.clear()
        self._static_tool_cost_cache.clear()

    def _active_weapon_state(self) -> _DamageEnhancerState | None:
        if self._active_weapon_state_key is None:
            return None
        return self._weapon_enhancer_states.get(self._active_weapon_state_key)

    def _match_weapon_profile(self, tool_name: str) -> tuple[str, dict] | None:
        profile = self._trifecta_weapon_profiles.get(tool_name)
        if profile:
            return tool_name, profile

        if tool_name in self._profile_match_cache:
            return self._profile_match_cache[tool_name]

        profile = self._equipment_profile_lookup(tool_name)
        if not profile:
            self._profile_match_cache[tool_name] = None
            return None
        canonical_name = profile.get("weapon_entity", {}).get("name") or tool_name
        match = (canonical_name, profile)
        self._profile_match_cache[tool_name] = match
        return match

    def _ensure_weapon_state(self, tool_name: str) -> _DamageEnhancerState | None:
        match = self._match_weapon_profile(tool_name)
        if not match:
            self._active_weapon_state_key = None
            self._active_weapon_observed_name = tool_name
            return None

        canonical_name, profile = match
        state = self._weapon_enhancer_states.get(canonical_name)
        if state is None:
            state = _DamageEnhancerState.from_props(canonical_name, profile)
            self._weapon_enhancer_states[canonical_name] = state
        self._active_weapon_state_key = canonical_name
        self._active_weapon_observed_name = tool_name
        return state

    def _current_cost_for_tool(
        self, tool_name: str, inferred_cost: float = 0.0
    ) -> float:
        state = self._ensure_weapon_state(tool_name)
        if state is not None:
            return state.current_cost_ped()
        if inferred_cost > 0:
            return inferred_cost
        cached = self._static_tool_cost_cache.get(tool_name)
        if cached is not None:
            return cached
        cost = self._equipment_cost_lookup(tool_name)
        self._static_tool_cost_cache[tool_name] = cost
        return cost

    def _tool_stats_for_phase(self, tool_name: str, cost_per_shot: float) -> ToolStats:
        if not self._accumulator:
            raise RuntimeError("No accumulator available")

        for stats in self._accumulator.tool_stats.values():
            if stats.tool_name != tool_name:
                continue
            if abs(stats.cost_per_shot - cost_per_shot) < 1e-9:
                return stats

        phase_count = sum(
            1
            for stats in self._accumulator.tool_stats.values()
            if stats.tool_name == tool_name
        )
        key = tool_name if phase_count == 0 else f"{tool_name}#{phase_count + 1}"
        stats = ToolStats(tool_name=tool_name, cost_per_shot=cost_per_shot)
        self._accumulator.tool_stats[key] = stats
        return stats

    def _record_offensive_shot(
        self,
        *,
        amount: float,
        is_crit: bool,
        allow_damage_inference: bool,
    ) -> None:
        """Accumulate one player attack, including jam/dodge/evade countered shots."""
        if not self._accumulator:
            return
        debug_perf = log.isEnabledFor(logging.DEBUG)
        shot_started = _time.perf_counter() if debug_perf else 0.0

        self._accumulator.shots_fired += 1
        if amount > 0:
            self._accumulator.damage_dealt += amount
        if is_crit:
            self._accumulator.critical_hits += 1

        inferred_cost = 0.0
        tool = None
        if self._is_weapon_attribution_trifecta():
            if allow_damage_inference:
                attribution = self._damage_attributor.match_damage(
                    amount, critical=is_crit
                )
                if attribution is None and not self._trifecta_unmatched_warning_emitted:
                    msg = "Trifecta attribution: damage fell outside both weapon ranges"
                    self._session_warnings.append(msg)
                    self._trifecta_unmatched_warning_emitted = True
                if attribution is not None:
                    tool = attribution.tool_name
                    inferred_cost = attribution.cost_per_shot
            else:
                tool = self._last_offensive_tool_name
        else:
            tool = self._active_hotbar_tool_name

        if tool is not None:
            self._last_offensive_tool_name = tool

        tool_key = tool or "Unknown"
        current_cost = 0.0
        if tool is not None:
            lookup_started = _time.perf_counter() if debug_perf else 0.0
            current_cost = self._current_cost_for_tool(
                tool, inferred_cost=inferred_cost
            )
            if debug_perf:
                self._perf_cost_lookup_seconds += _time.perf_counter() - lookup_started

        phase_key = tool_key
        if tool is not None and current_cost > 0:
            ts = self._tool_stats_for_phase(tool, current_cost)
        else:
            if phase_key not in self._accumulator.tool_stats:
                self._accumulator.tool_stats[phase_key] = ToolStats(tool_name=tool_key)
            ts = self._accumulator.tool_stats[phase_key]
            if ts.cost_per_shot == 0.0:
                fallback_cost = (
                    inferred_cost
                    if inferred_cost > 0
                    else self._equipment_cost_lookup(tool_key)
                )
                if fallback_cost > 0:
                    ts.cost_per_shot = fallback_cost
        ts.shots_fired += 1
        if amount > 0:
            ts.damage_dealt += amount
        if is_crit:
            ts.critical_hits += 1
        if debug_perf:
            self._record_shot_perf(
                tool is None,
                self._is_weapon_attribution_trifecta()
                and allow_damage_inference
                and tool is None,
                shot_started,
            )

    def _record_shot_perf(
        self, unknown_tool: bool, inference_miss: bool, shot_started: float
    ) -> None:
        self._perf_shot_count += 1
        if unknown_tool:
            self._perf_unknown_tool_shots += 1
        if inference_miss:
            self._perf_inference_misses += 1
        self._perf_shot_seconds += _time.perf_counter() - shot_started

        now = _time.monotonic()
        elapsed = now - self._perf_window_started
        if elapsed < 15.0:
            return

        shots = self._perf_shot_count
        avg_shot_ms = (self._perf_shot_seconds / shots * 1000.0) if shots else 0.0
        avg_cost_lookup_ms = (
            (self._perf_cost_lookup_seconds / shots * 1000.0) if shots else 0.0
        )
        log.debug(
            "Tracker combat perf: %.1fs shots=%d unknown=%d inference_misses=%d avg_shot_ms=%.3f avg_cost_lookup_ms=%.3f",
            elapsed,
            shots,
            self._perf_unknown_tool_shots,
            self._perf_inference_misses,
            avg_shot_ms,
            avg_cost_lookup_ms,
        )
        self._perf_window_started = now
        self._perf_shot_count = 0
        self._perf_unknown_tool_shots = 0
        self._perf_inference_misses = 0
        self._perf_shot_seconds = 0.0
        self._perf_cost_lookup_seconds = 0.0

    def reload_config(self) -> None:
        """Refresh trifecta-attribution state after config changes."""
        self._refresh_loot_filter()
        if not self._session:
            return
        if self._is_weapon_attribution_trifecta():
            self._load_trifecta_weapon_profiles()
        else:
            self._damage_attributor.clear()
            self._active_heal_tool_name = None
            self._heal_cost_per_use_ped = 0.0
            self._heal_reload_seconds = 2.5
            self._heal_amount_min = None
            self._heal_amount_max = None
            self._heal_warning_emitted = False
            self._reset_weapon_runtime_state()

        if self.is_session_tag_mode():
            return

        if self._is_manual_mob_entry_enabled():
            manual_mob = self._manual_mob_provider()
            if manual_mob is None:
                if self._mob_source == "manual":
                    self._clear_mob_state()
                return
            species, maturity = manual_mob
            display = f"{maturity} {species}" if maturity else species
            self._set_manual_mob_state(display, species, maturity)
            return

        if self._mob_source == "manual":
            self._clear_mob_state()

    def start_session(self) -> TrackingSession:
        """Start a new tracking session."""
        if self._session:
            self.stop_session()

        self._refresh_loot_filter()

        session_mob_tracking_mode = self._mob_tracking_mode_provider()
        session_mob_tracking_tag = self._mob_tracking_tag_provider().strip()

        session_id = str(uuid.uuid4())
        self._session = TrackingSession(id=session_id)
        self._session.start_time = self._now_fn()
        self._accumulator = _Accumulator()

        self._active_hotbar_tool_name = None
        self._last_heal_time = None
        self._session_heal_cost = 0.0
        self._heal_warning_emitted = False
        self._session_warnings: list[str] = []
        self._last_kill = None
        self._last_loot_fingerprint = None
        self._last_loot_time = None
        self._confirmed_mob_name = ""
        self._confirmed_mob_species = ""
        self._confirmed_mob_maturity = ""
        self._mob_source = None
        self._session_mob_tracking_mode = session_mob_tracking_mode
        self._session_mob_tracking_tag = session_mob_tracking_tag
        self._clear_mob_state()
        self._trifecta_unmatched_warning_emitted = False
        self._damage_attributor.clear()
        self._reset_weapon_runtime_state()

        if self._is_weapon_attribution_trifecta():
            self._load_trifecta_weapon_profiles()

        if self.is_session_tag_mode() and self._session_mob_tracking_tag:
            self._set_session_tag(self._session_mob_tracking_tag)
        elif self._is_manual_mob_entry_enabled():
            manual_mob = self._manual_mob_provider()
            if manual_mob is not None:
                species, maturity = manual_mob
                display = f"{maturity} {species}" if maturity else species
                self._set_manual_mob_state(display, species, maturity)

        # Subscribe to events
        self._event_bus.subscribe(EVENT_COMBAT, self._on_combat)
        self._event_bus.subscribe(EVENT_LOOT_GROUP, self._on_loot)
        self._event_bus.subscribe(EVENT_ACTIVE_TOOL_CHANGED, self._on_tool_changed)
        self._event_bus.subscribe(
            EVENT_ACTIVE_HEAL_TOOL_CHANGED, self._on_heal_tool_changed
        )
        self._event_bus.subscribe(EVENT_GLOBAL, self._on_global)
        self._event_bus.subscribe(EVENT_ENHANCER_BREAK, self._on_enhancer_break)
        self._event_bus.subscribe(EVENT_TICK_FLUSHED, self._on_tick_flushed)

        # Persist session start. `mob_tracking_mode` records the input
        # mode the session was captured under so post-hoc UI surfaces
        # can choose label vocabulary; the value never mutates after
        # session start.
        self._db.execute(
            "INSERT INTO tracking_sessions "
            "(id, started_at, is_active, mob_tracking_mode) "
            "VALUES (?, ?, 1, ?)",
            (
                session_id,
                self._session.start_time.timestamp(),
                self._session_mob_tracking_mode,
            ),
        )
        self._db.commit()

        log.info("Session started: %s", session_id[:8])
        self._event_bus.publish(EVENT_SESSION_STARTED, {"session_id": session_id})
        self._session_dirty = False
        self._emit_session_event(
            "started", "active", self._session.start_time.timestamp()
        )
        return self._session

    def _refresh_loot_filter(self) -> None:
        blacklist = self._loot_filter_blacklist
        if self._loot_filter_blacklist_provider is not None:
            blacklist = self._loot_filter_blacklist_provider()
        self._loot_blacklist = normalize_blacklist(blacklist)

    def _load_trifecta_weapon_profiles(self) -> None:
        """Load damage signatures + heal tool from configured trifecta."""
        self._damage_attributor.clear()
        self._active_heal_tool_name = None
        self._heal_cost_per_use_ped = 0.0
        self._heal_reload_seconds = 2.5
        self._heal_amount_min = None
        self._heal_amount_max = None
        self._heal_warning_emitted = False
        self._trifecta_weapon_profiles = {}
        self._active_weapon_state_key = None
        self._active_weapon_observed_name = None

        trifecta = self._trifecta_resolver()
        if not trifecta:
            return

        for key in ("small_weapon", "big_weapon"):
            weapon = trifecta.get(key)
            if not weapon:
                continue
            self._damage_attributor.add_weapon_profile(
                name=weapon["name"],
                min_damage=weapon["damage_min"],
                max_damage=weapon["damage_max"],
                base_damage=weapon["total_damage"],
                cost_per_shot=weapon["cost_per_shot_ped"],
                role=weapon.get("role"),
            )
            if weapon.get("weapon_props"):
                self._trifecta_weapon_profiles[weapon["name"]] = weapon["weapon_props"]

        heal_tool = trifecta.get("heal_tool")
        if heal_tool:
            self._active_heal_tool_name = heal_tool["name"]
            self._heal_cost_per_use_ped = heal_tool["cost_per_use_ped"]
            self._heal_reload_seconds = heal_tool["reload_seconds"]
            self._heal_amount_min = heal_tool.get("heal_min")
            self._heal_amount_max = heal_tool.get("heal_max")

    def stop_session(self) -> TrackingSession | None:
        """Stop the active session, compute dangling cost."""
        if not self._session:
            return None

        # Compute dangling cost from unresolved accumulator
        dangling_cost = 0.0
        if self._accumulator:
            dangling_cost = self._accumulator.total_cost

        # Unsubscribe
        self._event_bus.unsubscribe(EVENT_COMBAT, self._on_combat)
        self._event_bus.unsubscribe(EVENT_LOOT_GROUP, self._on_loot)
        self._event_bus.unsubscribe(EVENT_ACTIVE_TOOL_CHANGED, self._on_tool_changed)
        self._event_bus.unsubscribe(
            EVENT_ACTIVE_HEAL_TOOL_CHANGED, self._on_heal_tool_changed
        )
        self._event_bus.unsubscribe(EVENT_GLOBAL, self._on_global)
        self._event_bus.unsubscribe(EVENT_ENHANCER_BREAK, self._on_enhancer_break)
        self._event_bus.unsubscribe(EVENT_TICK_FLUSHED, self._on_tick_flushed)

        # Finalise session
        self._session.end_time = self._now_fn()
        self._session.dangling_cost = dangling_cost
        self._db.execute(
            "UPDATE tracking_sessions SET ended_at = ?, is_active = 0, "
            "heal_cost = ?, dangling_cost = ? WHERE id = ?",
            (
                self._session.end_time.timestamp(),
                self._session_heal_cost,
                dangling_cost,
                self._session.id,
            ),
        )

        # Auto-generate ledger gains derived from persisted loot rows.
        self._create_enhancer_rebate_ledger_entry(
            self._session.id, self._session.end_time
        )
        self._create_shrapnel_ledger_entry(self._session.id, self._session.end_time)

        from backend.services.session_summary import write_session_summary

        write_session_summary(self._db, self._session.id)

        self._db.commit()

        session = self._session
        log.info(
            "Session stopped: %s (%d kills, %.2f PED dangling cost)",
            session.id[:8],
            len(session.kills),
            dangling_cost,
        )
        self._event_bus.publish(EVENT_SESSION_STOPPED, {"session_id": session.id})
        self._emit_session_event(
            "stopped",
            "idle",
            session.end_time.timestamp() if session.end_time else None,
        )

        # Cleanup
        self._session = None
        self._accumulator = None
        self._active_hotbar_tool_name = None
        self._last_kill = None
        self._reset_weapon_runtime_state()
        self._clear_mob_state()

        return session

    def _clear_mob_state(self) -> None:
        self._current_mob_name = ""
        self._current_mob_species = ""
        self._current_mob_maturity = ""
        self._confirmed_mob_name = ""
        self._confirmed_mob_species = ""
        self._confirmed_mob_maturity = ""
        self._mob_source = None

    def _set_session_tag(self, tag: str) -> None:
        self._current_mob_name = tag
        self._current_mob_species = ""
        self._current_mob_maturity = ""
        self._confirmed_mob_name = tag
        self._confirmed_mob_species = ""
        self._confirmed_mob_maturity = ""
        self._mob_source = "tag"

    def _set_manual_mob_state(self, mob_name: str, species: str, maturity: str) -> None:
        self._current_mob_name = mob_name
        self._current_mob_species = species
        self._current_mob_maturity = maturity
        self._confirmed_mob_name = mob_name
        self._confirmed_mob_species = species
        self._confirmed_mob_maturity = maturity
        self._mob_source = "manual"

    def set_manual_tag(self, tag: str) -> None:
        """Immediately set the active free-text tag for tag-mode kill stamping."""
        if not self._session:
            raise RuntimeError("No active session")
        if not self.is_session_tag_mode():
            raise RuntimeError("Active session is not in tag mode")

        cleaned = tag.strip()
        if not cleaned:
            raise ValueError("Tag cannot be empty")

        self._session_mob_tracking_tag = cleaned
        self._set_session_tag(cleaned)

    def set_manual_mob(self, mob_name: str, species: str, maturity: str) -> None:
        """Immediately set the active mob for manual kill stamping."""
        if not self._session:
            raise RuntimeError("No active session")
        if self.is_session_tag_mode():
            raise RuntimeError("Tag mode sessions do not allow manual mob locking")
        if not self._is_manual_mob_entry_enabled():
            raise RuntimeError("Manual mob entry is not enabled for this session")

        self._set_manual_mob_state(mob_name, species, maturity)

    def release_current_mob(self) -> str | None:
        """Clear the current/confirmed mob state."""
        released = self._confirmed_mob_name or self._current_mob_name or None
        self._clear_mob_state()
        return released

    def _create_shrapnel_ledger_entry(
        self, session_id: str, end_time: datetime
    ) -> None:
        """Create a ledger entry for the shrapnel→ammo conversion margin.

        Shrapnel looted during a session is crafted into ammo at a 100:101 rate,
        yielding 1% margin. This is a personal economic decision (ledger) not a
        raw loot mechanic (Cycled), so it flows through the ledger as a gain.
        """
        row = self._db.execute(
            "SELECT COALESCE(SUM(kli.value_ped), 0) "
            "FROM kill_loot_items kli "
            "JOIN kills k ON kli.kill_id = k.id "
            "WHERE k.session_id = ? AND kli.item_name = 'Shrapnel' "
            "AND COALESCE(kli.is_enhancer_shrapnel, 0) = 0 "
            "AND kli.deactivated_at IS NULL",
            (session_id,),
        ).fetchone()
        shrapnel_ped = row[0] if row else 0.0
        if shrapnel_ped <= 0:
            return

        margin_ped = round(shrapnel_ped * 0.01, 4)
        date_str = end_time.isoformat()
        entry_id = str(uuid.uuid4())
        self._db.execute(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (
                entry_id,
                date_str,
                "markup",
                "Shrapnel Conversion",
                margin_ped,
                "convert",
            ),
        )
        log.info(
            "Shrapnel conversion: %.2f PED shrapnel → %.4f PED margin",
            shrapnel_ped,
            margin_ped,
        )

    def _create_enhancer_rebate_ledger_entry(
        self, session_id: str, end_time: datetime
    ) -> None:
        """Create a ledger entry for enhancer shrapnel refunded during the session."""
        row = self._db.execute(
            "SELECT COALESCE(SUM(kli.value_ped), 0) "
            "FROM kill_loot_items kli "
            "JOIN kills k ON kli.kill_id = k.id "
            "WHERE k.session_id = ? AND COALESCE(kli.is_enhancer_shrapnel, 0) = 1 "
            "AND kli.deactivated_at IS NULL",
            (session_id,),
        ).fetchone()
        rebate_ped = row[0] if row else 0.0
        if rebate_ped <= 0:
            return

        entry_id = str(uuid.uuid4())
        self._db.execute(
            "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (
                entry_id,
                end_time.isoformat(),
                "markup",
                "Enhancer Shrapnel Rebate",
                round(rebate_ped, 4),
                "enhancer",
            ),
        )
        log.info("Enhancer rebate: %.4f PED shrapnel refunded", rebate_ped)

    # ------------------------------------------------------------------
    # Event handlers
    # ------------------------------------------------------------------

    def _emit_session_event(
        self,
        reason: Literal["started", "updated", "stopped"],
        status: Literal["active", "idle"],
        occurred_ts: float | None,
    ) -> None:
        """Publish the coarse, frontend-facing tracking.session.updated event.

        A typed ``DomainEvent`` instance (not a dict) is published, so the bus
        carries the same shape the SSE bridge serialises and a future Rust
        emitter reproduces. ``occurred_at`` is stamped from the domain timestamp
        that triggered the event (the tick time, or the session start/stop
        time), not a fresh clock read: those values already exist in the
        scenario's event stream and DB columns, so the event is deterministic
        under replay and reuses the harness's existing timestamp symbols rather
        than minting wall-clock ones.
        """
        session_id = self._session.id if self._session else None
        self._event_bus.publish(
            TOPIC_TRACKING_SESSION_UPDATED,
            TrackingSessionUpdated(
                occurred_at=to_iso_utc(occurred_ts),
                payload=TrackingSessionUpdatedPayload(
                    sessionId=session_id,
                    status=status,
                    reason=reason,
                ),
            ),
        )

    def _on_tick_flushed(self, data: dict) -> None:
        """Coalesce a settled tick's mutations into one domain event.

        Subscribed only while a session is active. Emits a single
        ``tracking.session.updated`` when the tick actually changed the live
        session readout (the P&L handlers set ``_session_dirty``), so a tick of
        unrelated chat traffic does not wake every frontend listener. The
        event is stamped with the tick's own timestamp, which already appears
        on the tick's loot/combat events.
        """
        if self._session is None or not self._session_dirty:
            return
        self._session_dirty = False
        raw_ts = data.get("timestamp")
        occurred_ts = (
            raw_ts.timestamp()
            if isinstance(raw_ts, datetime)
            else (float(raw_ts) if raw_ts is not None else None)
        )
        self._emit_session_event("updated", "active", occurred_ts)

    def _on_combat(self, data: dict) -> None:
        """Handle a parsed combat event from chat.log."""
        if not self._accumulator:
            return

        event_type = data.get("type", "")
        amount = data.get("amount", 0.0)
        timestamp = data.get("timestamp")
        # Whether this event actually changed the live session readout. The
        # coalesced tracking.session.updated fires only on a real mutation, so a
        # duplicate self-heal tick or an unhandled event type does not wake
        # listeners for a no-op.
        mutated = False

        if event_type in ("damage_dealt", "critical_hit"):
            self._record_offensive_shot(
                amount=amount,
                is_crit=(event_type == "critical_hit"),
                allow_damage_inference=True,
            )
            mutated = True

        elif event_type in ("target_dodge", "target_evade", "target_jam"):
            self._record_offensive_shot(
                amount=0.0,
                is_crit=False,
                allow_damage_inference=False,
            )
            mutated = True

        elif event_type == "damage_received":
            self._accumulator.damage_taken += amount
            mutated = True

        # Kept as nested ifs rather than one combined condition so each
        # event-type branch dispatches on a single comparison.
        elif event_type == "self_heal":  # noqa: SIM102
            # Deduplicate: tool activations produce multiple heal ticks in chat.log.
            # Use the tool's reload time as the dedup window.
            if timestamp:
                is_new_heal_activation = (
                    self._last_heal_time is None
                    or (timestamp - self._last_heal_time).total_seconds()
                    >= self._heal_reload_seconds
                )
                if is_new_heal_activation:
                    if (
                        self._is_weapon_attribution_trifecta()
                        and not self._heal_amount_matches_trifecta_tool(amount)
                    ):
                        return
                    if (
                        self._active_heal_tool_name is None
                        and not self._heal_warning_emitted
                    ):
                        msg = "Healing detected — no heal tool equipped via hotbar"
                        self._session_warnings.append(msg)
                        self._heal_warning_emitted = True
                        log.warning(
                            "Healing detected with no heal tool equipped via hotbar"
                        )
                    if self._heal_cost_per_use_ped > 0:
                        self._session_heal_cost += self._heal_cost_per_use_ped
                    self._last_heal_time = timestamp
                    mutated = True

        if mutated:
            self._session_dirty = True

        # Defensive incoming events stay out of the kills model.

    def _on_loot(self, data: dict) -> None:
        """Handle a loot group from chat.log — creates a Kill record."""
        if not self._accumulator or not self._session:
            return

        items_raw = data.get("items", [])
        total_ped = data.get("total_ped", 0.0)
        timestamp = data.get("timestamp")
        now = timestamp or datetime.now(tz=None)
        now_epoch = now.timestamp() if isinstance(now, datetime) else float(now)

        # Loot deduplication (same fingerprint within 2s window)
        first_item = items_raw[0].get("item_name", "") if items_raw else ""
        fingerprint = (round(total_ped, 4), len(items_raw), first_item)
        if (
            self._last_loot_fingerprint == fingerprint
            and self._last_loot_time is not None
            and (now - self._last_loot_time) < self.LOOT_DEDUP_WINDOW
        ):
            return
        self._last_loot_fingerprint = fingerprint
        self._last_loot_time = now
        # Past the dedup guard a Kill is always recorded, so the readout changes.
        self._session_dirty = True

        # Filter items
        items = []
        for item in items_raw:
            name = item.get("item_name", "")
            if is_tracked_loot(name, self._loot_blacklist):
                items.append(
                    LootItem(
                        item_name=name,
                        quantity=item.get("quantity", 1),
                        value_ped=item.get("value_ped", 0.0),
                        is_enhancer_shrapnel=bool(
                            item.get("is_enhancer_shrapnel", False)
                        ),
                    )
                )
        filtered_total_ped = round(
            sum(item.value_ped for item in items if not item.is_enhancer_shrapnel),
            4,
        )

        # Snapshot mob/tag from manual configuration.
        mob_name = self._confirmed_mob_name or "Unknown"
        mob_species = self._confirmed_mob_species
        mob_maturity = self._confirmed_mob_maturity

        # Create Kill from accumulator
        kill = Kill(
            id=str(uuid.uuid4()),
            session_id=self._session.id,
            mob_name=mob_name,
            mob_species=mob_species,
            mob_maturity=mob_maturity,
            timestamp=now_epoch,
            shots_fired=self._accumulator.shots_fired,
            damage_dealt=self._accumulator.damage_dealt,
            damage_taken=self._accumulator.damage_taken,
            critical_hits=self._accumulator.critical_hits,
            cost_ped=self._accumulator.weapon_cost,
            enhancer_cost=self._accumulator.enhancer_cost,
            loot_total_ped=filtered_total_ped,
            loot_items=items,
            tool_stats=dict(self._accumulator.tool_stats),
        )

        # Reset accumulator for next kill
        self._accumulator.reset()

        # Persist and finalise
        self._session.kills.append(kill)
        self._last_kill = kill
        self._persist_kill(kill)

        log.debug(
            "Kill recorded: %s %.2f PED loot → %s",
            kill.mob_name,
            filtered_total_ped,
            kill.id[:8],
        )

    def _on_tool_changed(self, data: dict) -> None:
        """Handle hotbar-driven weapon tool change.

        Merges any 'Unknown' tool stats into the real tool when first detected.
        """
        if self._is_weapon_attribution_trifecta():
            return
        tool_name = data.get("tool_name")
        if not tool_name:
            return
        self._active_hotbar_tool_name = tool_name
        if not self._accumulator:
            return

        current_cost = self._current_cost_for_tool(tool_name)

        # Merge "Unknown" stats into the real tool on first identification
        unknown = self._accumulator.tool_stats.pop("Unknown", None)
        if unknown:
            if current_cost > 0:
                real = self._tool_stats_for_phase(tool_name, current_cost)
            else:
                if tool_name not in self._accumulator.tool_stats:
                    self._accumulator.tool_stats[tool_name] = ToolStats(
                        tool_name=tool_name
                    )
                real = self._accumulator.tool_stats[tool_name]
            real.shots_fired += unknown.shots_fired
            real.damage_dealt += unknown.damage_dealt
            real.critical_hits += unknown.critical_hits

    def _on_heal_tool_changed(self, data: dict) -> None:
        """Handle hotbar-driven heal tool equip."""
        if self._is_weapon_attribution_trifecta():
            return
        name = data.get("tool_name")
        cost = data.get("cost_per_use_ped", 0.0)
        reload_s = data.get("reload_seconds", 2.5)

        self._active_heal_tool_name = name
        self._heal_cost_per_use_ped = cost
        self._heal_reload_seconds = reload_s
        self._heal_amount_min = None
        self._heal_amount_max = None
        self._heal_warning_emitted = False

        log.info(
            "Heal tool equipped: %s (cost=%.4f PED, reload=%.1fs)",
            name,
            cost,
            reload_s,
        )

    def _heal_amount_matches_trifecta_tool(self, amount: float) -> bool:
        """Trifecta direct-heal attribution uses the configured heal interval."""
        if self._heal_amount_min is None or self._heal_amount_max is None:
            return True
        return self._heal_amount_min <= amount <= self._heal_amount_max

    def _break_matches_active_weapon(self, item_name: str) -> bool:
        state = self._active_weapon_state()
        if state is None:
            return False
        if not item_name:
            return False
        item_norm = "".join(ch.lower() for ch in item_name if ch.isalnum())
        tool_norm = "".join(ch.lower() for ch in state.tool_name if ch.isalnum())
        observed_norm = "".join(
            ch.lower()
            for ch in (self._active_weapon_observed_name or "")
            if ch.isalnum()
        )
        return bool(
            item_norm
            and (
                item_norm in tool_norm
                or tool_norm in item_norm
                or (
                    observed_norm
                    and (item_norm in observed_norm or observed_norm in item_norm)
                )
            )
        )

    def _on_global(self, data: dict) -> None:
        """Handle a global/HoF event from chat.log.

        Tags the most recently created kill (globals arrive shortly after loot).
        """
        if not self._session:
            return

        # Filter for own player
        player = data.get("player", "")
        if not self._player_name or player.lower() != self._player_name.lower():
            return

        self._session_dirty = True
        event_type = data.get("type", "")
        mob_or_item = data.get("creature") or data.get("item") or "Unknown"
        value_ped = data.get("value", 0.0)
        is_hof = event_type in ("hof_kill", "hof_item")
        timestamp = data.get("timestamp")
        ts = timestamp.timestamp() if timestamp else datetime.now(tz=None).timestamp()

        # Tag the most recently created kill (staleness check: within 5s)
        kill_id = None
        target = self._last_kill
        if target and abs(ts - target.timestamp) < 5.0:
            target.is_global = True
            if is_hof:
                target.is_hof = True
            kill_id = target.id
            self._db.execute(
                "UPDATE kills SET is_global = 1, is_hof = ? WHERE id = ?",
                (int(target.is_hof), target.id),
            )
            log.info(
                "Global/HoF correlated: %s %.2f PED → kill %s",
                event_type,
                value_ped,
                target.id[:8],
            )
        else:
            log.warning(
                "Global/HoF with no recent kill to correlate: %s %.2f PED",
                event_type,
                value_ped,
            )

        self._db.execute(
            """INSERT INTO notable_events
               (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (self._session.id, kill_id, event_type, mob_or_item, value_ped, ts),
        )
        self._db.commit()

    def _on_enhancer_break(self, data: dict) -> None:
        """Handle an enhancer break event — update enhancer state for future shots."""
        if not self._accumulator:
            return

        shrapnel_ped = data.get("shrapnel_ped", 0.0)
        enhancer_name = data.get("enhancer_name", "")
        item_name = data.get("item_name", "")
        remaining = data.get("remaining")

        log.debug(
            "Enhancer break: %s — shrapnel=%.2f, remaining=%s",
            enhancer_name,
            shrapnel_ped,
            remaining,
        )

        state = self._active_weapon_state()
        if (
            state is None
            or not state.stacks
            or "damage" not in enhancer_name.lower()
            or not self._break_matches_active_weapon(item_name)
        ):
            return

        # The break applies to the active weapon, so the readout reflects it; an
        # ignored break (filtered out above) leaves the session unchanged.
        self._session_dirty = True
        slot_changed = state.apply_break(
            remaining if isinstance(remaining, int) else None
        )
        if slot_changed:
            log.info(
                "Damage enhancer slot depleted on %s: %d active slot(s) remain",
                state.tool_name,
                state.active_slots,
            )

    # ------------------------------------------------------------------
    # Persistence
    # ------------------------------------------------------------------

    def _persist_kill(self, kill: Kill) -> None:
        """Write a finalised kill to the database."""
        self._db.execute(
            """INSERT OR REPLACE INTO kills
               (id, session_id, mob_name, mob_species, mob_maturity,
                timestamp, shots_fired, damage_dealt, damage_taken,
                critical_hits, cost_ped, enhancer_cost,
                loot_total_ped, is_global, is_hof)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                kill.id,
                kill.session_id,
                kill.mob_name,
                kill.mob_species,
                kill.mob_maturity,
                kill.timestamp,
                kill.shots_fired,
                kill.damage_dealt,
                kill.damage_taken,
                kill.critical_hits,
                kill.cost_ped,
                kill.enhancer_cost,
                kill.loot_total_ped,
                int(kill.is_global),
                int(kill.is_hof),
            ),
        )

        # Tool stats
        for stats in kill.tool_stats.values():
            self._db.execute(
                """INSERT OR REPLACE INTO kill_tool_stats
                   (kill_id, tool_name, shots_fired, damage_dealt,
                    critical_hits, cost_per_shot)
                   VALUES (?, ?, ?, ?, ?, ?)""",
                (
                    kill.id,
                    stats.tool_name,
                    stats.shots_fired,
                    stats.damage_dealt,
                    stats.critical_hits,
                    stats.cost_per_shot,
                ),
            )

        # Loot items
        for item in kill.loot_items:
            self._db.execute(
                """INSERT INTO kill_loot_items
                   (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel)
                   VALUES (?, ?, ?, ?, ?)""",
                (
                    kill.id,
                    item.item_name,
                    item.quantity,
                    item.value_ped,
                    int(item.is_enhancer_shrapnel),
                ),
            )

        self._db.commit()
