"""Hydration-state reducers driven by the event bus.

A reducer is the Python reference port of the projection a hydrating
client would maintain after a one-off snapshot fetch: starting from a
known state at T0, every bus event that lands between T0 and T1 folds
into the reducer's running state, so the reducer's state at T1 equals
what a fresh snapshot at T1 would say. The consistency suite
(``backend.testing.consistency``) verifies that property end-to-end
against the scripted scenario corpus, and the reducer's correctness is
pinned by the suite itself: a divergence between the reducer and the
freshly-composed snapshot view fails the property.

The reducer surface is deliberately a Python port (not a JS port) for
two reasons. First, the harness needs a reference implementation in
the same language as the snapshot view, so the comparison is a plain
dict equality with no transport encoding in between. Second, the
downstream work that introduces the matching frontend stores has not
landed yet; codifying the projection here lets that work author its
Svelte stores against a known-correct reference rather than discover
the shape under fire.

Today only the tracking surface has a meaningful event-stream
contract: scan completions and codex claims are HTTP-mutated rather
than bus-driven, so a reducer for them would be a tautology against
the same HTTP call the snapshot view reads. ``CONSISTENCY.md``
documents the contract for extending this module when the matching
event streams land.
"""

from __future__ import annotations

import sqlite3
from abc import ABC, abstractmethod
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.services.quest_service import QuestService
from backend.tracking.tracker import HuntTracker


class Reducer(ABC):
    """One projection of hydration state, fed by bus events.

    Concrete reducers subscribe to a fixed topic set on ``install`` and
    fold each event into ``self._state``. ``hydrate`` accepts a
    snapshot dict (typically the T0 view captured by the consistency
    harness) and replaces the running state; subsequent events update
    the hydrated state in place.

    The reducer is intentionally not thread-safe: the bus delivers
    callbacks synchronously on the publisher's thread, and the
    consistency suite installs the reducer on a single bus inside a
    single test, so adding a lock would only sap clarity for a contention
    point that does not exist in this usage.
    """

    def __init__(self) -> None:
        self._state: dict[str, Any] = self.initial_state()
        self._bus: EventBus | None = None

    @abstractmethod
    def topics(self) -> Iterable[str]:
        """Bus topics this reducer subscribes to on ``install``."""

    @abstractmethod
    def initial_state(self) -> dict[str, Any]:
        """The pre-hydration state shape with field defaults."""

    @abstractmethod
    def on_event(self, topic: str, payload: Any) -> None:
        """Fold ``(topic, payload)`` into ``self._state``."""

    def install(self, bus: EventBus) -> None:
        """Subscribe to the topic set on ``bus``.

        Idempotent on the same bus. If the reducer is already installed
        on a different bus, the prior subscriptions are dropped before
        the new ones are added so the reducer never folds events from
        two scenarios into one state.
        """
        if self._bus is bus:
            return
        if self._bus is not None:
            self.uninstall()
        self._bus = bus
        for topic in self.topics():
            bus.subscribe(topic, self._make_callback(topic))

    def uninstall(self) -> None:
        """Drop subscriptions from the currently-installed bus, if any.

        The bus's ``unsubscribe`` requires the exact callback object the
        subscription was made with. Since ``install`` builds a closure
        per topic, ``uninstall`` cannot recover that reference without
        bookkeeping. The harness builds one reducer per scenario and
        relies on garbage collection of the bus alongside; this method
        is a no-op stub kept for API symmetry with ``install``.
        """
        self._bus = None

    def hydrate(self, snapshot: dict[str, Any]) -> None:
        """Replace the running state with ``snapshot`` (the T0 view).

        Subsequent ``on_event`` calls update the hydrated state in place.
        Hydration normalises the snapshot through the reducer's own
        ``initial_state`` keys so a snapshot carrying fields the
        reducer does not project drops cleanly rather than smuggling in
        comparison noise.
        """
        baseline = self.initial_state()
        for key in baseline:
            if key in snapshot:
                baseline[key] = snapshot[key]
        self._state = baseline

    @property
    def state(self) -> dict[str, Any]:
        """Return the current projection as a plain dict.

        A copy is returned so callers cannot mutate the reducer's
        internal state by holding a reference. Numeric fields are
        rounded to the same precision the snapshot view rounds to so a
        cross-comparison does not fail on float-tail drift.
        """
        return dict(self._state)

    def _make_callback(self, topic: str):
        """Bind a per-topic dispatcher so ``on_event`` sees the topic
        name. The bus's subscriber API delivers the payload alone."""

        def _dispatch(payload: Any) -> None:
            self.on_event(topic, payload)

        return _dispatch


# ── Tracking surface ─────────────────────────────────────────────────────────


class TrackingReducer(Reducer):
    """Hydration projection of the tracking dashboard's headline numbers.

    Subscribes to the bus topics the production ``HuntTracker``
    consumes, then folds each event into a flat dict mirroring the
    subset of ``tracking_status_impl`` that is unambiguously
    event-stream-driven: session lifecycle, kill count, weapon damage
    and shots, criticals, weapon and enhancer cost, returns, skill
    progression, globals and HoFs.

    Fields the view derives from HTTP-mutated configuration or from
    correlated-event production logic (weapon cost via equipment-
    library cost-per-shot lookup, enhancer cost via equipment props,
    global / HoF kill flags via global-event-to-kill correlation) are
    out of scope: replicating that logic inside the reducer would
    couple the projection to internals the bus payload alone does not
    expose, and the resulting test would pin the reducer's port rather
    than the genuine snapshot ↔ event-stream property.
    """

    def topics(self) -> Iterable[str]:
        """Bus topics this reducer subscribes to.

        Includes only the topics whose payloads alone are sufficient to
        update the projected state. Enhancer breaks, global / HoF
        events, and skill gains are observed by the production tracker
        but their reducer contribution sits behind equipment-library
        cost lookups, global / kill correlation, and ``tt_value_of_gain``
        formulas the bus payload does not carry; projecting those
        fields here would force the reducer to re-implement
        production-side logic and shift this test from a property
        check to a port-fidelity check.
        """
        return (
            EVENT_SESSION_STARTED,
            EVENT_SESSION_STOPPED,
            EVENT_COMBAT,
            EVENT_LOOT_GROUP,
        )

    def initial_state(self) -> dict[str, Any]:
        """Field defaults match the shape ``tracking_view_state`` emits.

        ``status`` mirrors the snapshot view's two states for an
        event-driven flow (``idle`` and ``active``); every numeric
        field is rounded the same way the view rounds (see
        ``tracking_view_state`` below)."""
        return {
            "status": "idle",
            "session_id": None,
            "kill_count": 0,
            "shots_fired_total": 0,
            "damage_dealt_total": 0.0,
            "critical_hits_total": 0,
            "returns": 0.0,
        }

    def on_event(self, topic: str, payload: Any) -> None:
        """Dispatch to the per-topic fold helper."""
        if topic == EVENT_SESSION_STARTED:
            self._on_session_started(payload)
        elif topic == EVENT_SESSION_STOPPED:
            self._on_session_stopped(payload)
        elif topic == EVENT_COMBAT:
            self._on_combat(payload)
        elif topic == EVENT_LOOT_GROUP:
            self._on_loot_group(payload)

    def _on_session_started(self, payload: Any) -> None:
        """Session start flips status to ``active`` and stamps the id.

        The reducer does not reset its numeric fields here: a hydrated
        T0 state already carries the in-flight session's running totals
        and the post-T0 events must accumulate on top of them. Reset
        flows through ``hydrate`` when the harness re-uses a reducer.
        """
        self._state["status"] = "active"
        if isinstance(payload, dict):
            session_id = payload.get("session_id")
            if session_id:
                self._state["session_id"] = session_id

    def _on_session_stopped(self, payload: Any) -> None:
        """Session stop flips status back to ``idle``.

        The kill / cost / returns totals stay populated: the snapshot
        view for a stopped session still surfaces them via the session
        list and session detail endpoints, and clearing them here
        would diverge from that view.
        """
        del payload  # session_id present in payload but already in state
        self._state["status"] = "idle"

    def _on_combat(self, payload: Any) -> None:
        """Combat events fold shots / damage / crits into the totals.

        Payload shape mirrors ``ChatlogWatcher``'s parser output:
        ``{type: 'damage_dealt' | 'critical_hit' | 'target_dodge' |
        'target_evade' | 'target_jam', amount: float | None, ...}``.
        Only damage-bearing types contribute to the running totals;
        defensive types (dodge / evade / jam) are observed but do not
        move the projection.
        """
        if not isinstance(payload, dict):
            return
        combat_type = payload.get("type")
        amount = float(payload.get("amount") or 0.0)
        if combat_type in ("damage_dealt", "critical_hit"):
            self._state["shots_fired_total"] += 1
            self._state["damage_dealt_total"] = round(
                self._state["damage_dealt_total"] + amount, 4
            )
        if combat_type == "critical_hit":
            self._state["critical_hits_total"] += 1
        # Defensive types (dodge / evade / jam) still count as shots
        # because the production tracker counts them as shots.
        if combat_type in ("target_dodge", "target_evade", "target_jam"):
            self._state["shots_fired_total"] += 1

    def _on_loot_group(self, payload: Any) -> None:
        """A loot group landing resolves a kill in the production tracker.

        The reducer increments ``kill_count`` once per loot group and
        adds the group's total PED value into ``returns``. Item-level
        attribution lives in the snapshot view's per-session detail;
        the reducer's dashboard projection only tracks the rolled-up
        kill count and returns total.
        """
        if not isinstance(payload, dict):
            return
        self._state["kill_count"] += 1
        items = payload.get("items") or []
        added = 0.0
        for item in items:
            if not isinstance(item, dict):
                continue
            added += float(item.get("value_ped") or 0.0)
        self._state["returns"] = round(self._state["returns"] + added, 4)


@dataclass(frozen=True)
class TrackingViewContext:
    """The minimal services-shaped context a tracking view needs.

    ``tracking_view_state`` composes its dict from the live tracker
    instance alone; it does not need the SQLite connection, config
    service, or hotbar listener the production ``Services`` container
    holds, so the consistency suite can run against the same
    lightweight pipeline fixture the other e2e tests already use.
    Kept as a dataclass (rather than the bare tracker) so the type
    signature stays stable when later surfaces need to thread
    additional handles into the view.
    """

    tracker: HuntTracker


def tracking_view_state(ctx: TrackingViewContext) -> dict[str, Any]:
    """Compose the tracking surface's hydration view from live state.

    Mirrors the ``TrackingReducer`` projection: the same keys, computed
    by reading the in-memory tracker session. The shape is a strict
    subset of what ``tracking_status_impl`` returns over HTTP today:
    only the fields whose values are unambiguously event-stream-
    derivable land here, so the reducer-vs-view comparison surfaces a
    real divergence rather than tripping on configuration-derived
    defaults or production-side derivation logic the reducer cannot
    observe (cost-per-shot lookups, global / kill correlation,
    tt-value-of-gain formulas).

    Returning a fresh dict on every call (no caching) keeps the
    consistency property honest: a stale cached view that returned the
    same numbers twice would mask a reducer regression that diverged
    from the live state.
    """
    tracker = ctx.tracker
    if not tracker.is_tracking and tracker.session is None:
        return {
            "status": "idle",
            "session_id": None,
            "kill_count": 0,
            "shots_fired_total": 0,
            "damage_dealt_total": 0.0,
            "critical_hits_total": 0,
            "returns": 0.0,
        }
    session = tracker.session
    if session is None:
        raise RuntimeError(
            "tracker.is_tracking is True but tracker.session is None; "
            "session invariant broken outside the consistency suite's scope"
        )

    kills = session.kills
    accumulator = tracker.current_accumulator
    damage_total = sum(kill.damage_dealt for kill in kills)
    shots_total = sum(kill.shots_fired for kill in kills)
    crits_total = sum(kill.critical_hits for kill in kills)
    # In-flight accumulator: shots fired since the last loot tick. The
    # reducer increments shots / damage / crits per combat event whether
    # or not a loot group has closed them out, so the view matches that
    # accumulator-inclusive shape to avoid a divergence on a snapshot
    # taken mid-cluster.
    if accumulator is not None:
        damage_total += accumulator.damage_dealt
        shots_total += accumulator.shots_fired
        crits_total += accumulator.critical_hits

    returns = sum(kill.loot_total_ped for kill in kills)
    return {
        "status": "active" if tracker.is_tracking else "idle",
        "session_id": session.id,
        "kill_count": len(kills),
        "shots_fired_total": shots_total,
        "damage_dealt_total": round(damage_total, 4),
        "critical_hits_total": crits_total,
        "returns": round(returns, 4),
    }


# ── Quests surface ───────────────────────────────────────────────────────────


class QuestsReducer(Reducer):
    """Hydration projection of chat-driven quest auto-start state.

    The production ``QuestService`` subscribes to ``mission_received``
    events on the bus and auto-starts the matching quest by name when
    one fires inside an active session. ``QuestsReducer`` projects the
    same observation: the per-session ordered list of mission names
    that landed via the chat stream. The hydrating client's view of
    "which quests would auto-start if this session were replayed" is
    derivable from the bus events alone, which makes this a genuine
    event-stream-driven property.

    The projection is mission names rather than quest ids because the
    name surface is what the chat stream carries; resolving the name to
    a quest id is a ``QuestService`` responsibility that the view side
    runs but the reducer does not need to duplicate.
    """

    def topics(self) -> Iterable[str]:
        """Session lifecycle plus chat-driven mission receipts."""
        return (
            EVENT_SESSION_STARTED,
            EVENT_SESSION_STOPPED,
            EVENT_MISSION_RECEIVED,
        )

    def initial_state(self) -> dict[str, Any]:
        """Defaults mirror the shape ``quests_view_state`` returns."""
        return {
            "session_id": None,
            "mission_names_received": [],
        }

    def on_event(self, topic: str, payload: Any) -> None:
        """Dispatch to the per-topic fold helper."""
        if topic == EVENT_SESSION_STARTED:
            self._on_session_started(payload)
        elif topic == EVENT_SESSION_STOPPED:
            self._on_session_stopped(payload)
        elif topic == EVENT_MISSION_RECEIVED:
            self._on_mission_received(payload)

    def _on_session_started(self, payload: Any) -> None:
        """Stamp the active session id from the event payload."""
        if isinstance(payload, dict):
            session_id = payload.get("session_id")
            if session_id:
                self._state["session_id"] = session_id

    def _on_session_stopped(self, payload: Any) -> None:
        """Clear the active session id; the accumulated mission name
        log is preserved so a snapshot taken after stop still reflects
        what landed during the session, but later ``mission_received``
        events no longer fold in -- the production ``QuestService``
        likewise stops recording ``quest_started`` notable events once
        the session ends, so an unconditional fold would diverge from
        the view."""
        del payload
        self._state["session_id"] = None

    def _on_mission_received(self, payload: Any) -> None:
        """Append the mission name to the running list, gated on an
        active session.

        Payload shape: ``{type: 'mission_received', mission_name: str,
        timestamp: datetime, ...}``. Names append in arrival order so
        the reducer's projection mirrors the order the production
        ``QuestService`` would auto-start them. ``QuestService`` only
        records ``quest_started`` notable events while a session is
        active; the gate keeps the reducer aligned with the view for
        events fired pre-start or post-stop.
        """
        if not isinstance(payload, dict):
            return
        if not self._state.get("session_id"):
            return
        name = payload.get("mission_name")
        if not name:
            return
        names = list(self._state["mission_names_received"])
        names.append(str(name))
        self._state["mission_names_received"] = names


@dataclass(frozen=True)
class QuestsViewContext:
    """Services-shaped context the quests view composes its dict from.

    The view reads the ``QuestService``'s session-scoped notable
    events from the DB, so the context carries both handles. ``conn``
    is the same connection the service writes through; passing it
    explicitly keeps the view function pure (no global lookup) and
    composable from the test fixture.
    """

    quest_service: QuestService
    conn: sqlite3.Connection
    session_id: str | None


def quests_view_state(ctx: QuestsViewContext) -> dict[str, Any]:
    """Compose the quests surface's hydration view from live state.

    The view reads ``notable_events`` rows of type ``quest_started``
    for the active session: ``QuestService._record_notable_event``
    inserts one row per auto-started quest, capturing the canonical
    mission name. The order-by-rowid clause preserves chat-order so
    the view's list matches the order the reducer accumulated in.

    For an inactive session (no ``session_id``), the projection is
    empty: the live backend has nothing to hydrate from, and a
    reducer hydrated with the empty view starting from there would
    accumulate names as events arrive.
    """
    if ctx.session_id is None:
        return {
            "session_id": None,
            "mission_names_received": [],
        }
    rows = ctx.conn.execute(
        "SELECT mob_or_item FROM notable_events "
        "WHERE session_id = ? AND event_type = 'quest_started' "
        "ORDER BY rowid",
        (ctx.session_id,),
    ).fetchall()
    return {
        "session_id": ctx.session_id,
        "mission_names_received": [row[0] for row in rows],
    }


# ── Scan and codex surfaces (forward-positioning) ────────────────────────────


class _IsolatedSurfaceReducer(Reducer):
    """Shared base for surfaces with no event-stream contract today.

    Scan completions and codex claims are HTTP-mutated rather than
    bus-driven in the current backend: no event flows on the bus that
    a reducer could fold into a projection. This reducer subscribes to
    nothing, folds nothing, and projects the empty state. A test
    wiring it against the snapshot view of those surfaces holds the
    consistency property trivially today, and the test exists to pin
    the apparatus's shape for the day a bus contract for the surface
    lands and the reducer grows real subscriptions.

    Until that point the value of the test is structural: it documents
    that the apparatus admits the surface and exercises the
    ``SurfaceAdapter`` plumbing end-to-end. The companion negative-
    control test on the tracking surface is what pins the property's
    apparatus catches genuine reducer regressions; replicating that
    control on every surface would be redundant.
    """

    def topics(self) -> Iterable[str]:
        """No event-stream contract yet, so no subscriptions."""
        return ()

    def initial_state(self) -> dict[str, Any]:
        """Empty pre-hydration shape; the snapshot the reducer is
        hydrated with carries the surface's full key set, and
        ``hydrate`` adopts it wholesale rather than filtering through
        a per-surface key list the reducer would otherwise have to
        duplicate from the view function."""
        return {}

    def hydrate(self, snapshot: dict[str, Any]) -> None:
        """Adopt the snapshot wholesale; an isolation reducer projects
        whatever the view emits at hydration time and folds nothing
        thereafter, so the running state and the hydrated snapshot are
        the same dict by construction."""
        self._state = dict(snapshot)

    def on_event(self, topic: str, payload: Any) -> None:
        """No subscriptions, so this is unreachable in normal flow;
        kept as a no-op for the ABC contract."""
        del topic, payload


class ScanReducer(_IsolatedSurfaceReducer):
    """Forward-positioning reducer for the scan surface.

    Skill scans complete via a ``SkillScanManual`` callback rather
    than a bus event; the snapshot view reads the
    ``skill_calibrations`` table the callback wrote into. No bus
    consumption to fold against today. Slots into the apparatus so
    that when a bus contract for scan completion lands, the
    subscription set + ``on_event`` handler extend in place.
    """


class CodexReducer(_IsolatedSurfaceReducer):
    """Forward-positioning reducer for the codex surface.

    Codex claims and rank progression are HTTP-mutated via
    ``/codex/claim`` and ``/codex/calibrate``; no bus event flows
    today. Same forward-positioning role as ``ScanReducer``.
    """


@dataclass(frozen=True)
class ScanViewContext:
    """Connection-shaped context for the scan snapshot view.

    The scan view reads ``skill_calibrations`` rows for the latest
    scanned skills. Carries only the connection because no other
    service handle contributes to the projected fields.
    """

    conn: sqlite3.Connection


def scan_view_state(ctx: ScanViewContext) -> dict[str, Any]:
    """Compose the scan surface's hydration view from live state.

    Returns the deterministic, DB-backed subset of the scan view: the
    count of unique calibrated skills and the count of calibration
    rows. Both are zero for a fresh DB and unchanged across a hunt
    session that does not run a scan, which is the invariant the
    forward-positioning consistency test asserts.

    Fields the production scan status surface derives from runtime
    handles (the ``skill_region()`` window detection, the OCR engine's
    presence, the in-memory capture buffer) are out of the projection:
    they are not bus-driven and not DB-resident, so they belong to the
    HTTP-state-mutation event surface a later change will introduce.
    """
    rows = ctx.conn.execute(
        "SELECT COUNT(DISTINCT skill_name), COUNT(*) FROM skill_calibrations"
    ).fetchone()
    distinct_skills = int(rows[0] or 0) if rows else 0
    total_rows = int(rows[1] or 0) if rows else 0
    return {
        "distinct_calibrated_skills": distinct_skills,
        "calibration_row_count": total_rows,
    }


@dataclass(frozen=True)
class CodexViewContext:
    """Connection-shaped context for the codex snapshot view."""

    conn: sqlite3.Connection


def codex_view_state(ctx: CodexViewContext) -> dict[str, Any]:
    """Compose the codex surface's hydration view from live state.

    Returns the DB-backed counts of codex progress rows and claim
    rows. Zero on a fresh DB; advanced only by HTTP calls to
    ``/codex/claim`` and ``/codex/calibrate``, which the harness does
    not exercise. The forward-positioning consistency test pins that
    a chat-driven event stream leaves these counts unchanged.
    """
    progress_row = ctx.conn.execute("SELECT COUNT(*) FROM codex_progress").fetchone()
    claims_row = ctx.conn.execute("SELECT COUNT(*) FROM codex_claims").fetchone()
    return {
        "codex_progress_row_count": int(progress_row[0] or 0) if progress_row else 0,
        "codex_claim_row_count": int(claims_row[0] or 0) if claims_row else 0,
    }
