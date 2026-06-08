"""Metamorphic-property suite for the replay harness.

A metamorphic property relates the output of one run to the output of a
second run whose input is a controlled transformation of the first, when
the transformation is one the system is supposed to be blind to. These
relations need no hand-computed oracle: the first run is the oracle for
the second. They are deliberately harness-driven and deterministic so
they double as the comparison contract a future Python-vs-Rust
differential oracle would ride on (feed both implementations the same
transformed inputs; assert the same metamorphic relation holds for each).

Two themes carry the suite:

* Reorder-invariance. Reordering causally-independent events (combat
  shots inside one kill window; loot items inside one group) leaves the
  externally-observable result unchanged. Combat reorder is asserted on
  the full normalised database snapshot; loot-item reorder is asserted
  on the order-insensitive projections (per-kill totals and the loot
  multiset), because the persisted ``kill_loot_items`` rows intentionally
  retain arrival order and so are not snapshot-identical under a
  within-group permutation.
* Replay-idempotence and replay-stability. Re-snapshotting a quiescent
  surface yields no divergence; replaying the same scenario in two fresh
  pipelines yields a byte-identical snapshot; and splitting a scenario
  into two replay segments yields the same snapshot as replaying the
  concatenation in one. At the parser layer, mapping is pure: duplicating
  a line N times yields N identical events, and ``parse_file`` distributes
  over concatenation.

Each relation was verified to hold against the current code before being
encoded; relations the production code does not satisfy (for example a
within-group loot reorder being snapshot-identical) are asserted at the
projection level the code genuinely respects, with the reason noted
inline rather than over-claimed.
"""

from __future__ import annotations

import sqlite3
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.core.event_bus import EventBus
from backend.services.chatlog_parser import parse_file, parse_line
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.consistency import _diff_state, replay_segment
from backend.testing.db_snapshot import capture
from backend.testing.dsl import Scenario
from backend.testing.fingerprint import Normalizer
from backend.testing.replay import (
    _group_by_tick,
    _stream_ticks,
    replay_scenario,
    wait_for_drain,
)
from backend.testing.store_reducers import (
    TrackingViewContext,
    tracking_view_state,
)
from backend.tracking.tracker import HuntTracker

# --- pipeline harness ------------------------------------------------------

_EPOCH = "2024-01-01 10:00:00"


@contextmanager
def _pipeline() -> Iterator[
    tuple[ChatlogWatcher, HuntTracker, Path, sqlite3.Connection]
]:
    """Boot a self-contained replay pipeline over a throwaway chatlog.

    Each metamorphic relation compares two or more independent runs, so
    every run needs its own fresh bus, tracker, in-memory DB, and watcher
    rather than the shared per-test fixtures. The watcher's real tail loop
    is started here and stopped on exit so the relation exercises the same
    file-write to SQLite path the live game drives.
    """
    tmp = Path(tempfile.mkdtemp(prefix="eo_metamorphic_"))
    chatlog = tmp / "chat.log"
    chatlog.touch()
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db)
    watcher = ChatlogWatcher(bus, chatlog)
    watcher.start()
    try:
        yield watcher, tracker, chatlog, db
    finally:
        watcher.stop()
        db.close()


def _snapshot_scenario(build: Callable[[Scenario], None]) -> dict[str, Any]:
    """Replay a freshly-built scenario through a pipeline and snapshot it.

    ``build`` populates a :class:`Scenario` whose lines are streamed
    through the real watcher between ``start_session`` and ``stop_session``.
    The returned value is the normalised database snapshot, the same
    surface the golden-file regression suite diffs against.
    """
    with _pipeline() as (watcher, tracker, chatlog, db):
        scenario_dir = chatlog.parent / "scenario"
        scenario = Scenario("metamorphic").at(_EPOCH)
        build(scenario)
        scenario.write(scenario_dir)

        tracker.start_session()
        replay_scenario(scenario_dir, chatlog)
        wait_for_drain(watcher, chatlog)
        tracker.stop_session()

        return capture(db, Normalizer())


def _kill_totals(snapshot: dict[str, Any]) -> list[tuple[Any, ...]]:
    """Order-insensitive projection of the per-kill headline numbers.

    Drops the per-kill UUID (varies per run) and sorts so the projection
    is invariant to the order kills happen to be returned in. This is the
    surface a loot-item reorder must preserve even though the raw
    ``kill_loot_items`` row order does not.
    """
    return sorted(
        (
            kill["shots_fired"],
            kill["damage_dealt"],
            kill["critical_hits"],
            kill["loot_total_ped"],
        )
        for kill in snapshot["kills"]
    )


def _loot_multiset(snapshot: dict[str, Any]) -> list[tuple[Any, ...]]:
    """Order-insensitive multiset of persisted loot items.

    The kill id is dropped (per-run UUID) so the comparison is purely the
    bag of ``(name, quantity, value, is_shrapnel)`` rows, which a
    within-group reorder must leave untouched even as it permutes the
    stored row order.
    """
    return sorted(
        (
            item["item_name"],
            item["quantity"],
            item["value_ped"],
            item["is_enhancer_shrapnel"],
        )
        for item in snapshot["kill_loot_items"]
    )


# --- reorder-invariance ----------------------------------------------------

# Three causally-independent shots inside a single kill window: two plain
# damage lines and one critical, each on its own tick so the parser sees
# distinct timestamps. The loot tick at the end closes the accumulated
# window into one kill. Reordering these shots must not change anything the
# tracker persists, because shots / damage / criticals fold additively and
# the kill row stamps the loot tick's timestamp, not any shot's.
_COMBAT_SHOTS: tuple[Callable[[Scenario], None], ...] = (
    lambda s: s.combat.damage_dealt(10.0),
    lambda s: s.combat.critical_hit(35.0),
    lambda s: s.combat.damage_dealt(20.0),
)

_COMBAT_PERMUTATIONS = (
    (0, 1, 2),
    (2, 1, 0),
    (1, 2, 0),
    (2, 0, 1),
)


def _build_single_kill(order: tuple[int, ...]) -> Callable[[Scenario], None]:
    """Build a one-kill scenario whose combat shots follow ``order``."""

    def build(scenario: Scenario) -> None:
        for index in order:
            _COMBAT_SHOTS[index](scenario)
            scenario.tick()
        scenario.loot.received("Shrapnel", 5.00)
        scenario.loot.received("Animal Muscle Oil", 0.12)
        scenario.tick()

    return build


@pytest.mark.parametrize("order", _COMBAT_PERMUTATIONS)
def test_combat_reorder_within_kill_window_yields_identical_snapshot(
    order: tuple[int, ...],
) -> None:
    """Permuting independent shots in one kill window is snapshot-invariant.

    The canonical (source) order is the oracle; every permutation must
    reproduce its full normalised snapshot. A genuine divergence here would
    mean the tracker's per-kill accumulation is order-sensitive, which it
    must not be.
    """
    canonical = _snapshot_scenario(_build_single_kill(_COMBAT_PERMUTATIONS[0]))
    permuted = _snapshot_scenario(_build_single_kill(order))
    assert permuted == canonical


def test_combat_reorder_is_non_vacuous() -> None:
    """Guard the reorder relation against trivially passing on an empty DB.

    If a future edit broke the pipeline so no kill ever persisted, the
    snapshot-equality assertions above would still hold (empty == empty).
    Pin that the canonical run actually produced the one expected kill with
    the summed totals so the relation stays a real property.
    """
    snapshot = _snapshot_scenario(_build_single_kill(_COMBAT_PERMUTATIONS[0]))
    assert len(snapshot["kills"]) == 1
    kill = snapshot["kills"][0]
    assert kill["shots_fired"] == 3
    assert kill["critical_hits"] == 1
    assert kill["damage_dealt"] == pytest.approx(65.0)
    assert kill["loot_total_ped"] == pytest.approx(5.12)


# Two loot items inside one group; swapping their arrival order. The group
# resolves one kill whose total and item bag are order-insensitive, but the
# persisted kill_loot_items rows keep arrival order, so this relation is
# asserted on the projections, not on the full snapshot.
def _build_loot_pair(reversed_order: bool) -> Callable[[Scenario], None]:
    items = [("Hide", 3.0), ("Animal Oil", 2.0)]
    if reversed_order:
        items = list(reversed(items))

    def build(scenario: Scenario) -> None:
        scenario.combat.damage_dealt(10.0)
        scenario.tick()
        for name, value in items:
            scenario.loot.received(name, value)
        scenario.tick()

    return build


def test_loot_item_reorder_preserves_kill_totals_and_loot_multiset() -> None:
    """Swapping loot items within one group preserves the order-insensitive
    projections.

    The full snapshot is intentionally NOT compared: ``kill_loot_items`` is
    ordered by insertion and so legitimately differs in row order under a
    within-group permutation. The kill-total projection and the loot
    multiset are the surfaces that must hold, and they pin the
    item-partition-invariance of returns that a differential oracle would
    check.
    """
    forward = _snapshot_scenario(_build_loot_pair(reversed_order=False))
    swapped = _snapshot_scenario(_build_loot_pair(reversed_order=True))

    assert _kill_totals(swapped) == _kill_totals(forward)
    assert _loot_multiset(swapped) == _loot_multiset(forward)

    # Non-vacuous: one kill, two items, the documented rolled-up total.
    assert len(forward["kills"]) == 1
    assert forward["kills"][0]["loot_total_ped"] == pytest.approx(5.0)
    assert len(forward["kill_loot_items"]) == 2


# --- replay-idempotence and replay-stability -------------------------------


def _build_two_kill_hunt(scenario: Scenario) -> None:
    """A two-kill hunt with a critical and a multi-item loot group.

    Reused by the determinism and segment-split relations so they compare
    the same non-trivial surface.
    """
    scenario.combat.damage_dealt(12.0)
    scenario.tick()
    scenario.combat.critical_hit(30.0)
    scenario.tick()
    scenario.loot.received("Shrapnel", 5.0)
    scenario.loot.received("Hide", 1.5)
    scenario.tick()
    scenario.tick()
    scenario.combat.damage_dealt(22.0)
    scenario.tick()
    scenario.loot.received("Animal Oil", 2.0)
    scenario.tick()


def test_replaying_the_same_scenario_twice_yields_identical_snapshot() -> None:
    """Replay-stability: two fresh pipelines, one scenario, one snapshot.

    The normalised snapshot drops per-run UUIDs and timestamps to stable
    symbols, so a deterministic pipeline must reproduce it byte-for-byte
    across runs. This is the baseline a Python-vs-Rust differential oracle
    extends: same input, same normalised snapshot, regardless of which
    implementation produced it.
    """
    first = _snapshot_scenario(_build_two_kill_hunt)
    second = _snapshot_scenario(_build_two_kill_hunt)
    assert first == second
    assert len(first["kills"]) == 2


def test_segment_split_replay_equals_single_concatenated_replay() -> None:
    """The midpoint split is a test marker, not a semantic boundary.

    Streaming a scenario as ``chat_replay.log`` + ``chat_replay_after.log``
    through two ``replay_segment`` calls must land the same snapshot as
    streaming the concatenation in one pass. A divergence would mean the
    drain-and-resume boundary the consistency suite relies on perturbs
    tracker state, which it must not.
    """
    pre = Scenario("pre").at(_EPOCH)
    pre.combat.damage_dealt(12.0)
    pre.tick()
    pre.loot.received("Shrapnel", 5.0)
    pre.tick()
    pre_text = "".join(pre.lines())

    post = Scenario("post").at("2024-01-01 10:00:05")
    post.combat.critical_hit(30.0)
    post.tick()
    post.loot.received("Animal Oil", 2.0)
    post.tick()
    post_text = "".join(post.lines())

    with _pipeline() as (watcher, tracker, chatlog, db):
        scenario_dir = chatlog.parent / "split"
        scenario_dir.mkdir(parents=True, exist_ok=True)
        (scenario_dir / "chat_replay.log").write_text(pre_text, encoding="utf-8")
        (scenario_dir / "chat_replay_after.log").write_text(post_text, encoding="utf-8")
        tracker.start_session()
        replay_segment(scenario_dir, "chat_replay.log", chatlog, watcher)
        replay_segment(scenario_dir, "chat_replay_after.log", chatlog, watcher)
        tracker.stop_session()
        split_snapshot = capture(db, Normalizer())

    with _pipeline() as (watcher, tracker, chatlog, db):
        scenario_dir = chatlog.parent / "concat"
        scenario_dir.mkdir(parents=True, exist_ok=True)
        (scenario_dir / "chat_replay.log").write_text(
            pre_text + post_text, encoding="utf-8"
        )
        tracker.start_session()
        replay_scenario(scenario_dir, chatlog)
        wait_for_drain(watcher, chatlog)
        tracker.stop_session()
        concat_snapshot = capture(db, Normalizer())

    assert split_snapshot == concat_snapshot
    assert len(concat_snapshot["kills"]) == 2


def test_re_snapshotting_a_quiescent_tracking_surface_is_idempotent() -> None:
    """Re-fetching a surface with no intervening events yields no divergence.

    The hydration contract treats a snapshot as a stable read; capturing
    the tracking view twice with nothing happening in between must produce
    the identical projection. ``_diff_state`` returning ``[]`` is the
    harness's own equality predicate, so the relation is phrased in its
    terms.
    """
    with _pipeline() as (watcher, tracker, chatlog, _db):
        scenario_dir = chatlog.parent / "quiescent"
        scenario = Scenario("quiescent").at(_EPOCH)
        scenario.combat.damage_dealt(10.0)
        scenario.tick()
        scenario.loot.received("Shrapnel", 5.0)
        scenario.tick()
        scenario.write(scenario_dir)

        tracker.start_session()
        replay_scenario(scenario_dir, chatlog)
        wait_for_drain(watcher, chatlog)

        context = TrackingViewContext(tracker=tracker)
        first_view = tracking_view_state(context)
        second_view = tracking_view_state(context)
        tracker.stop_session()

    assert _diff_state(first_view, second_view) == []
    # Non-vacuous: the surface actually carried the one kill, so the
    # idempotence is over a populated state rather than an empty one.
    assert first_view["kill_count"] == 1


# --- parser-layer replay-idempotence (pure mapping) ------------------------

# Single-word, letters-only names sidestep every delimiter the parser keys
# on (no spaces, brackets, "Value:" or " x (" sequences), so a generated
# name can never accidentally re-classify the line.
_NAME = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
    min_size=1,
    max_size=12,
)
_AMOUNT = st.floats(
    min_value=0.0, max_value=99999.0, allow_nan=False, allow_infinity=False
)


def _damage_line(amount: float) -> str:
    return f"{_EPOCH} [System] [] You inflicted {amount} points of damage\n"


def _loot_line(name: str, value: float) -> str:
    return f"{_EPOCH} [System] [] You received {name} Value: {value:.2f} PED\n"


def _project(events: list) -> list[tuple[Any, Any]]:
    """Project parsed events to (type, data) so two parses compare by value."""
    return [(event.type, repr(event.data)) for event in events]


@given(st.text(max_size=120), st.integers(min_value=2, max_value=6))
def test_parse_line_is_a_pure_repeatable_mapping(line: str, repeats: int) -> None:
    """Re-parsing the same line yields the identical result every time.

    ``parse_line`` is a deterministic function of its input, so replaying a
    segment that repeats one line N times produces N pairwise-identical
    results: the duplication-idempotence-of-mapping that makes a doubled
    replay segment a doubled-but-otherwise-identical stream. The repeatability
    holds whether the line is recognised (all the same event) or not (all
    ``None``), so it is asserted over arbitrary text.
    """
    results = [parse_line(line) for _ in range(repeats)]
    first = results[0]
    if first is None:
        assert all(result is None for result in results)
    else:
        assert all(result is not None for result in results)
        projected = {
            (result.type, repr(result.data)) for result in results if result is not None
        }
        assert len(projected) == 1


def test_repeated_recognised_line_yields_n_identical_events() -> None:
    """Non-vacuous companion: a recognised line duplicated N times parses to
    N identical events, so the duplication relation is exercised on a real
    event and not only on the all-``None`` branch.
    """
    line = _damage_line(12.5).rstrip("\n")
    events = [parse_line(line) for _ in range(4)]
    assert all(event is not None for event in events)
    projected = {
        (event.type, repr(event.data)) for event in events if event is not None
    }
    assert len(projected) == 1


@given(_AMOUNT, _NAME, _AMOUNT)
def test_parse_file_distributes_over_concatenation(
    damage: float, loot_name: str, loot_value: float
) -> None:
    """``parse_file(A + B) == parse_file(A) + parse_file(B)``.

    Each line is parsed in isolation with no cross-line state, so parsing a
    concatenated log equals concatenating the per-segment parses. This is the
    parser-layer statement of segment-split invariance and pins line
    independence for the differential oracle. (Generated content is
    constrained to lines the parser recognises and whose timestamps are
    in-range, so the totality carve-out the parser does not guarantee on
    out-of-range timestamps is out of scope here.)
    """
    segment_a = _damage_line(damage)
    segment_b = _loot_line(loot_name, loot_value)

    tmp = Path(tempfile.mkdtemp(prefix="eo_metamorphic_parse_"))
    path_a = tmp / "a.log"
    path_b = tmp / "b.log"
    path_ab = tmp / "ab.log"
    path_a.write_text(segment_a, encoding="utf-8")
    path_b.write_text(segment_b, encoding="utf-8")
    path_ab.write_text(segment_a + segment_b, encoding="utf-8")

    parsed_a = parse_file(path_a)
    parsed_b = parse_file(path_b)
    parsed_ab = parse_file(path_ab)

    assert _project(parsed_ab) == _project(parsed_a) + _project(parsed_b)


# --- feed tick-atomicity (the determinism the combat-reorder property needs) -

# The watcher closes an app tick whenever its tail loop reaches end-of-file (a
# tick is closed when the timestamp advances OR the file goes idle). So the
# replay feed must never let the watcher reach end-of-file in the middle of a
# same-timestamp tick: that flushes a partial tick and lands the trailing
# same-second line in a fresh, wrong tick, splitting one kill into two. Under
# parallel load that interleaving is what made the combat-reorder property
# above flake (a per-line feed let the tail thread reach end-of-file in the gap
# between the two same-second loot lines). The feed groups same-timestamp lines
# into one write so a tick is the atomic streaming unit; these guards pin that
# grouping and the no-split property it buys, with a deterministic companion
# proving the hazard is real so the no-split assertion is non-vacuous.


def _same_second_loot_pair() -> list[str]:
    """Two loot lines sharing one timestamp: one parser tick, one feed write."""
    s = Scenario("pair").at(_EPOCH)
    s.loot.received("Shrapnel", 5.00)
    s.loot.received("Animal Muscle Oil", 0.12)
    return s.lines()


def _one_kill_with_same_second_loot() -> list[str]:
    """One combat shot then a same-second two-item loot group: one kill."""
    s = Scenario("kill").at(_EPOCH)
    s.combat.damage_dealt(10.0)
    s.tick()
    s.loot.received("Shrapnel", 5.00)
    s.loot.received("Animal Muscle Oil", 0.12)
    s.tick()
    return s.lines()


def test_group_by_tick_keeps_same_timestamp_lines_in_one_group() -> None:
    a, b = _same_second_loot_pair()
    assert list(_group_by_tick([a, b])) == [[a, b]]


def test_group_by_tick_splits_on_timestamp_change() -> None:
    combat, loot1, _loot2 = _one_kill_with_same_second_loot()
    # Combat at 10:00:00, loot at 10:00:01: distinct ticks, distinct groups.
    assert list(_group_by_tick([combat, loot1])) == [[combat], [loot1]]


def test_group_by_tick_attaches_untimestamped_continuation_to_current_tick() -> None:
    a, b = _same_second_loot_pair()
    cont = "    wrapped continuation with no timestamp\n"
    # A line the parser cannot timestamp neither opens nor closes a tick, so it
    # rides with the tick currently streaming rather than forcing a flush.
    assert list(_group_by_tick([a, cont, b])) == [[a, cont, b]]


def test_group_by_tick_streams_leading_untimestamped_line_alone() -> None:
    cont = "no timestamp here\n"
    combat = _one_kill_with_same_second_loot()[0]
    # With no tick to join, a leading untimestamped line streams on its own and
    # the first timestamped line opens the first real tick.
    assert list(_group_by_tick([cont, combat])) == [[cont], [combat]]


def test_group_by_tick_is_empty_for_no_lines() -> None:
    assert list(_group_by_tick([])) == []


def test_feed_keeps_same_second_loot_in_one_kill() -> None:
    """The tick-atomic feed lands both same-second loot items in one kill.

    The two loot lines share a timestamp, so they belong to one app tick and
    one kill. The production :func:`_stream_ticks` writes that tick in a single
    flush, so the watcher can never reach end-of-file between the two lines.
    """
    with _pipeline() as (watcher, tracker, chatlog, db):
        tracker.start_session()
        _stream_ticks(_one_kill_with_same_second_loot(), chatlog)
        wait_for_drain(watcher, chatlog)
        tracker.stop_session()
        snapshot = capture(db, Normalizer())

    assert len(snapshot["kills"]) == 1
    assert len(snapshot["kill_loot_items"]) == 2


def test_idle_boundary_between_same_second_lines_splits_the_tick() -> None:
    """Detection-power companion: an idle boundary mid-tick splits the kill.

    Draining between the two same-second loot writes forces the watcher to
    reach end-of-file mid-tick (the exact interleaving a per-line feed allowed
    under parallel load), so it closes the first loot line into one kill and the
    second into a fresh, empty-combat kill. This pins that the no-split
    guarantee above is non-vacuous: the hazard is real, and any feed that lets
    the watcher idle mid-tick reintroduces it.
    """
    combat, loot1, loot2 = _one_kill_with_same_second_loot()
    with _pipeline() as (watcher, tracker, chatlog, db):
        tracker.start_session()
        # Each same-second loot line written and drained separately, so the
        # watcher idles (and flushes) between them: the splitting schedule.
        for chunk in ([combat], [loot1], [loot2]):
            _stream_ticks(chunk, chatlog)
            wait_for_drain(watcher, chatlog)
        tracker.stop_session()
        snapshot = capture(db, Normalizer())

    assert len(snapshot["kills"]) == 2
