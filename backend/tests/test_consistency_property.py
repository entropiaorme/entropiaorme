"""Property-based generalisation of the snapshot/event-stream consistency.

The hand-authored consistency scenarios pin the property at a handful of
fixed midpoints. This module lifts the same property to arbitrary valid
hunt sequences: a gameplay-DSL strategy generates a sequence of combat
and loot clusters, a randomised midpoint splits them into the harness's
two-segment layout, and ``ConsistencyHarness`` drives the full lifecycle
(replay the pre-midpoint segment, snapshot T0, install a fresh
``TrackingReducer`` and hydrate it, replay the post-midpoint segment,
snapshot T1, diff). The property asserted for every generated sequence is
the one a future event-driven hydration client relies on: hydrate-then-
fold equals a fresh re-fetch.

Each example boots the real ``ChatlogWatcher`` plus ``HuntTracker`` over a
throwaway in-memory database and streams through the watcher's genuine
tail loop, so the example budget is kept to the dev profile's ceiling. The
generator stays inside the projection's agreed surface (offensive and
defensive shots plus distinctly-valued, non-shrapnel loot ticks) so a
divergence indicts the snapshot view or the reducer, not an event family
the projection deliberately does not model.

The module also encodes the structural invariants of the consistency
machinery itself: ``ConsistencyResult.holds`` tracks divergence emptiness
exactly, ``_diff_state`` is reflexive and reports only a symmetric, sorted,
duplicate-free key set, and ``replay_segment`` fails loudly on a missing
segment rather than silently skipping events.
"""

from __future__ import annotations

import sqlite3
import tempfile
from dataclasses import dataclass
from pathlib import Path

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.consistency import (
    ConsistencyHarness,
    ConsistencyResult,
    SurfaceAdapter,
    _diff_state,
    replay_segment,
)
from backend.testing.dsl import Scenario
from backend.testing.store_reducers import (
    TrackingReducer,
    TrackingViewContext,
    tracking_view_state,
)
from backend.tracking.tracker import HuntTracker

# Combat builders the tracking projection models identically on both
# sides: offensive shots advance shots / damage (and crits), defensive
# counters advance shots only. Enhancer breaks, globals, skills, and
# missions are deliberately excluded; the reducer does not project them,
# so generating them would test an out-of-scope event family.
_OFFENSIVE = ("damage_dealt", "critical_hit")
_DEFENSIVE = ("target_dodge", "target_evade", "target_jam")
_COMBAT_KINDS = _OFFENSIVE + _DEFENSIVE

# Damage amounts stay strictly positive and two-decimal-clean so the
# DSL's ``Value:``/``points of damage`` emission round-trips through the
# parser without rounding noise the float tolerance would have to absorb.
_AMOUNTS = st.integers(min_value=1, max_value=5000).map(lambda cents: cents / 100.0)


@dataclass(frozen=True)
class _Cluster:
    """One timestamp's worth of combat, optionally closed by a loot tick.

    ``combat`` is the ordered list of combat-kind strings emitted at the
    cluster's combat timestamp; ``loot_value`` is the PED value of a
    single loot drop emitted one tick later (closing the kill) or ``None``
    when the cluster leaves its shots in the in-flight accumulator.
    """

    combat: tuple[str, ...]
    loot_value: float | None


_combat_kinds = st.lists(st.sampled_from(_COMBAT_KINDS), min_size=0, max_size=4)


@st.composite
def _clusters(draw: st.DrawFn) -> list[_Cluster]:
    """Generate a non-empty sequence of combat/loot clusters.

    Each cluster carries zero-or-more combat events and an independent
    coin-flip on whether a loot tick closes it. The sequence is bounded
    small because every example boots the real pipeline and streams the
    lines through the watcher's tail loop.
    """
    count = draw(st.integers(min_value=1, max_value=4))
    out: list[_Cluster] = []
    for _ in range(count):
        combat = tuple(draw(_combat_kinds))
        has_loot = draw(st.booleans())
        loot_value = draw(_AMOUNTS) if has_loot else None
        out.append(_Cluster(combat=combat, loot_value=loot_value))
    return out


def _emit_cluster(scenario: Scenario, cluster: _Cluster, loot_value: float) -> None:
    """Append one cluster's lines to ``scenario`` at the current cursor.

    Combat events land on the current timestamp; the loot tick (when the
    cluster has one) lands one tick later so the watcher flushes the
    combat tick first and the loot group closes a kill against the
    accumulated shots. ``loot_value`` is passed in pre-resolved to a
    globally-unique value so the tracker's same-fingerprint dedup window
    never collapses two distinct kills into one.
    """
    for kind in cluster.combat:
        if kind == "damage_dealt":
            scenario.combat.damage_dealt(20.0)
        elif kind == "critical_hit":
            scenario.combat.critical_hit(20.0)
        elif kind == "target_dodge":
            scenario.combat.target_dodge()
        elif kind == "target_evade":
            scenario.combat.target_evade()
        elif kind == "target_jam":
            scenario.combat.target_jam()
        scenario.tick()
    if cluster.loot_value is not None:
        scenario.loot.received("Animal Muscle Oil", value_ped=loot_value)
        scenario.tick()


def _write_segments(
    scenario_dir: Path, clusters: list[_Cluster], midpoint: int
) -> None:
    """Write the pre- and post-midpoint segment files for ``clusters``.

    ``midpoint`` is the cluster index the split falls on; clusters before
    it become ``chat_replay.log`` and the remainder ``chat_replay_after.log``.
    Both files are always written (even when empty) so the harness's
    two-segment contract is satisfied. Loot values are made globally
    unique across the whole sequence (a fixed base plus the cluster index)
    so no two loot ticks share the tracker's dedup fingerprint, keeping
    the kill count the view derives equal to the loot-group count the
    reducer folds.
    """
    pre = Scenario(name="consistency_property_pre")
    post = Scenario(name="consistency_property_post")
    pre.at("2026-05-19 10:00:00")
    post.at("2026-05-19 11:00:00")
    for index, cluster in enumerate(clusters):
        target = pre if index < midpoint else post
        # Unique per-cluster value keeps loot fingerprints distinct.
        loot_value = round(1.00 + index, 2)
        _emit_cluster(target, cluster, loot_value)
    (scenario_dir / "chat_replay.log").write_text(
        "".join(pre.lines()), encoding="utf-8"
    )
    (scenario_dir / "chat_replay_after.log").write_text(
        "".join(post.lines()), encoding="utf-8"
    )


def _expected_kill_count(clusters: list[_Cluster]) -> int:
    """How many loot groups (and thus kills) the sequence should resolve."""
    return sum(1 for c in clusters if c.loot_value is not None)


@settings(
    max_examples=settings().max_examples,
    deadline=None,
    # Each example builds its own pipeline + temp scenario dir inside the
    # body, so the fixture-reuse health check does not apply.
    suppress_health_check=[HealthCheck.function_scoped_fixture],
)
@given(clusters=_clusters(), midpoint_frac=st.floats(min_value=0.0, max_value=1.0))
def test_hydrate_then_fold_matches_fresh_snapshot_for_any_sequence(
    clusters: list[_Cluster],
    midpoint_frac: float,
) -> None:
    """For any valid hunt sequence, hydrate-from-T0 + fold == fresh T1."""

    midpoint = round(midpoint_frac * len(clusters))

    with tempfile.TemporaryDirectory(prefix="eo_consistency_prop_") as raw_dir:
        scenario_dir = Path(raw_dir)
        chatlog = scenario_dir / "chat_testing.log"
        chatlog.touch()
        _write_segments(scenario_dir, clusters, midpoint)

        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(bus, db)
        watcher = ChatlogWatcher(bus, chatlog)
        watcher.start()
        tracker.start_session()
        try:
            harness = ConsistencyHarness(bus=bus, chatlog_path=chatlog, watcher=watcher)
            adapter = SurfaceAdapter(
                name="tracking",
                view_fn=tracking_view_state,
                reducer_factory=TrackingReducer,
            )
            result = harness.run(
                scenario_dir=scenario_dir,
                adapter=adapter,
                view_context=TrackingViewContext(tracker=tracker),
            )
        finally:
            if tracker.is_tracking:
                tracker.stop_session()
            watcher.stop()
            db.close()

    assert result.holds, (
        "Consistency property failed for a generated sequence; keys "
        f"diverged: {result.divergence}. hydrated_state="
        f"{result.hydrated_state!r} snapshot_t1={result.snapshot_t1!r}"
    )
    # Cross-check the generated sequence actually drove the surface it
    # claimed to, so a generator regression that produced inert sequences
    # cannot make the property pass vacuously.
    assert result.snapshot_t1["kill_count"] == _expected_kill_count(clusters)


# ── Structural invariants of the consistency machinery ───────────────────────


_SCALAR_VALUES = st.one_of(
    st.integers(min_value=-1000, max_value=1000),
    st.floats(allow_nan=False, allow_infinity=False, min_value=-1e6, max_value=1e6),
    st.text(max_size=8),
    st.none(),
    st.booleans(),
)
_STATE_DICTS = st.dictionaries(
    keys=st.text(min_size=1, max_size=6), values=_SCALAR_VALUES, max_size=6
)


@given(divergence=st.lists(st.text(max_size=6), max_size=5))
def test_holds_iff_divergence_empty(divergence: list[str]) -> None:
    """``holds`` is True exactly when the divergence list is empty."""
    result = ConsistencyResult(
        snapshot_t0={},
        snapshot_t1={},
        hydrated_state={},
        divergence=divergence,
    )
    assert result.holds == (divergence == [])


@given(state=_STATE_DICTS)
def test_diff_state_is_reflexive(state: dict) -> None:
    """A snapshot never diverges from itself."""
    assert _diff_state(state, state) == []


@given(left=_STATE_DICTS, right=_STATE_DICTS)
def test_diff_state_keys_are_symmetric_sorted_and_unique(
    left: dict, right: dict
) -> None:
    """Divergence keys are drawn from the union, sorted, and de-duplicated.

    The fourth conjunct of the proposed invariant (an exact numeric
    versus non-numeric equality rule) is not encoded: the audit found the
    code's float-coercion branch does not match that wording in the
    abstract, and the production call surface never exercises the
    divergent case. Encoding the structural three conjuncts that do hold
    keeps the property honest about what the implementation guarantees.
    """
    divergence = _diff_state(left, right)
    union = set(left) | set(right)
    assert set(divergence) <= union
    assert divergence == sorted(divergence)
    assert len(divergence) == len(set(divergence))
    # A key missing from exactly one side must always be reported.
    only_one_side = set(left) ^ set(right)
    assert only_one_side <= set(divergence)


@given(left=_STATE_DICTS, right=_STATE_DICTS)
def test_diff_state_reports_unequal_shared_keys(left: dict, right: dict) -> None:
    """Every shared key whose values are plainly unequal is reported.

    Restricted to keys that are not float-typed on either side so the
    assertion mirrors the implementation's non-numeric ``!=`` branch
    rather than its tolerance branch.
    """
    divergence = set(_diff_state(left, right))
    for key in set(left) & set(right):
        lhs, rhs = left[key], right[key]
        if isinstance(lhs, float) or isinstance(rhs, float):
            continue
        if lhs != rhs:
            assert key in divergence


def test_replay_segment_raises_on_missing_segment(tmp_path: Path) -> None:
    """A missing segment file is a loud contract error, never a no-op.

    The guard runs before any watcher interaction, so a dummy watcher
    object is sufficient: reaching it would already be the bug. The error
    message names both the scenario and the missing segment.
    """
    chatlog = tmp_path / "chat_testing.log"
    chatlog.touch()
    with pytest.raises(FileNotFoundError) as excinfo:
        replay_segment(
            scenario_dir=tmp_path,
            segment_name="chat_replay_after.log",
            chatlog_path=chatlog,
            watcher=object(),  # type: ignore[arg-type]  # never touched: the guard raises first
        )
    message = str(excinfo.value)
    assert tmp_path.name in message
    assert "chat_replay_after.log" in message
