"""Mutation-hardening tests for the tracker__c3 cluster.

Targets ``HuntTracker._record_shot_perf`` (the combat perf-window
accumulator/flush) and ``HuntTracker.reload_config`` (trifecta/heal/mob
state refresh after a config change).

Both methods are exercised against the real ``backend.tracking.tracker``
module with an in-memory SQLite DB and an in-process event bus, with no
device, OS, or GPU dependency. Time is driven through a fake injected for
the ``backend.tracking.tracker._time`` module alias so the perf-window
flush is fully deterministic.
"""

import logging
import sqlite3

import pytest

from backend.core.event_bus import EventBus
from backend.tracking import tracker as tracker_mod
from backend.tracking.tracker import HuntTracker

# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------


class _FakeTime:
    """Deterministic stand-in for the ``time`` module alias.

    ``monotonic`` returns whatever value is queued; ``perf_counter`` advances
    by a fixed step on each call so shot-second accumulation is predictable.
    """

    def __init__(self, monotonic_value: float = 0.0):
        self._monotonic = monotonic_value
        self._perf = 0.0
        self.perf_step = 0.0

    def set_monotonic(self, value: float) -> None:
        self._monotonic = value

    def monotonic(self) -> float:
        return self._monotonic

    def perf_counter(self) -> float:
        value = self._perf
        self._perf += self.perf_step
        return value


@pytest.fixture
def fake_time(monkeypatch):
    ft = _FakeTime()
    monkeypatch.setattr(tracker_mod, "_time", ft)
    return ft


@pytest.fixture
def tracker():
    db = sqlite3.connect(":memory:", check_same_thread=False)
    return HuntTracker(EventBus(), db)


@pytest.fixture
def debug_records():
    """Force DEBUG and capture every record emitted on the tracker logger."""
    records: list[logging.LogRecord] = []

    class _H(logging.Handler):
        def emit(self, record):  # noqa: D401
            records.append(record)

    handler = _H()
    handler.setLevel(logging.DEBUG)
    previous_level = tracker_mod.log.level
    tracker_mod.log.addHandler(handler)
    tracker_mod.log.setLevel(logging.DEBUG)
    try:
        yield records
    finally:
        tracker_mod.log.removeHandler(handler)
        tracker_mod.log.setLevel(previous_level)


# ---------------------------------------------------------------------------
# _record_shot_perf : per-shot accumulation (window NOT yet elapsed)
# ---------------------------------------------------------------------------


def test_record_shot_perf_increments_counters_within_window(tracker, fake_time):
    """One call within the window bumps each counter by exactly one and adds
    the shot's wall time. Kills the increment/assignment mutants on the
    accumulation lines (count, unknown, miss, shot-seconds)."""
    fake_time.set_monotonic(1000.0)
    tracker._perf_window_started = 1000.0  # elapsed == 0 -> no flush
    tracker._perf_shot_count = 0
    tracker._perf_unknown_tool_shots = 0
    tracker._perf_inference_misses = 0
    tracker._perf_shot_seconds = 0.0

    # perf_counter() called once inside; shot_started passed as 5.0, the
    # internal call returns 7.0 -> adds exactly 2.0 seconds.
    fake_time._perf = 7.0
    tracker._record_shot_perf(True, True, 5.0)

    assert tracker._perf_shot_count == 1
    assert tracker._perf_unknown_tool_shots == 1
    assert tracker._perf_inference_misses == 1
    assert tracker._perf_shot_seconds == pytest.approx(2.0)


def test_record_shot_perf_does_not_count_unknown_or_miss_when_false(tracker, fake_time):
    """unknown_tool / inference_miss False -> those counters stay put.
    Pins that the increments are gated behind their flags."""
    fake_time.set_monotonic(2000.0)
    tracker._perf_window_started = 2000.0
    tracker._perf_shot_count = 0
    tracker._perf_unknown_tool_shots = 0
    tracker._perf_inference_misses = 0
    tracker._perf_shot_seconds = 0.0

    fake_time._perf = 0.0
    tracker._record_shot_perf(False, False, 0.0)

    assert tracker._perf_shot_count == 1
    assert tracker._perf_unknown_tool_shots == 0
    assert tracker._perf_inference_misses == 0


def test_record_shot_perf_accumulates_across_calls(tracker, fake_time):
    """Two within-window calls accumulate (catches ``= 1`` vs ``+= 1`` and
    sign-flip mutants on every accumulator)."""
    fake_time.set_monotonic(3000.0)
    tracker._perf_window_started = 3000.0
    tracker._perf_shot_count = 0
    tracker._perf_unknown_tool_shots = 0
    tracker._perf_inference_misses = 0
    tracker._perf_shot_seconds = 0.0

    fake_time._perf = 0.0
    fake_time.perf_step = 0.0
    tracker._record_shot_perf(True, True, 0.0)
    tracker._record_shot_perf(True, True, -1.0)  # internal perf_counter() == 0.0

    assert tracker._perf_shot_count == 2
    assert tracker._perf_unknown_tool_shots == 2
    assert tracker._perf_inference_misses == 2
    # second call adds 0.0 - (-1.0) = 1.0
    assert tracker._perf_shot_seconds == pytest.approx(1.0)


def test_record_shot_perf_shot_seconds_uses_difference_not_sum(tracker, fake_time):
    """``perf_counter() - shot_started`` (not ``+``): a large shot_started
    yields a small/negative delta, never the sum."""
    fake_time.set_monotonic(3500.0)
    tracker._perf_window_started = 3500.0
    tracker._perf_shot_seconds = 0.0
    fake_time._perf = 10.0
    tracker._record_shot_perf(False, False, 4.0)
    assert tracker._perf_shot_seconds == pytest.approx(6.0)  # 10 - 4, not 14


def test_record_shot_perf_within_window_does_not_flush(
    tracker, fake_time, debug_records
):
    """elapsed < 15 -> no log, no reset. Catches ``elapsed = now + started``
    (always huge -> would flush) and the early-return removal."""
    records = debug_records
    fake_time.set_monotonic(5000.0)
    tracker._perf_window_started = 4990.0  # elapsed == 10 < 15
    tracker._perf_shot_count = 4
    tracker._record_shot_perf(False, False, 0.0)

    assert tracker._perf_shot_count == 5  # incremented, NOT reset to 0
    assert records == []  # no flush log emitted
    assert tracker._perf_window_started == 4990.0  # window NOT advanced


# ---------------------------------------------------------------------------
# _record_shot_perf : window flush (elapsed >= 15)
# ---------------------------------------------------------------------------


def _seed_for_flush(tracker, fake_time, *, now, window_started):
    fake_time.set_monotonic(now)
    tracker._perf_window_started = window_started
    tracker._perf_shot_count = 5
    tracker._perf_unknown_tool_shots = 2
    tracker._perf_inference_misses = 1
    tracker._perf_shot_seconds = 0.6
    tracker._perf_cost_lookup_seconds = 0.12
    fake_time._perf = 0.0
    fake_time.perf_step = 0.0


def test_record_shot_perf_flush_resets_all_counters(tracker, fake_time, debug_records):
    """elapsed >= 15 -> emits the debug record and resets every counter.
    Kills the reset-line mutants (= None / = 1 / = 1.0) and the window
    advance."""
    records = debug_records
    _seed_for_flush(tracker, fake_time, now=10_000.0, window_started=9_980.0)  # 20s

    tracker._record_shot_perf(False, False, 0.0)

    assert len(records) == 1
    assert tracker._perf_shot_count == 0
    assert tracker._perf_unknown_tool_shots == 0
    assert tracker._perf_inference_misses == 0
    assert tracker._perf_shot_seconds == 0.0
    assert tracker._perf_cost_lookup_seconds == 0.0
    assert tracker._perf_window_started == 10_000.0  # advanced to ``now``


def test_record_shot_perf_flush_log_message_and_args(tracker, fake_time, debug_records):
    """The flush log carries the exact template and the exact substitution
    args. Kills the message-string mutants and the per-arg ``None`` /
    dropped-argument mutants on the ``log.debug`` call."""
    records = debug_records
    _seed_for_flush(tracker, fake_time, now=20_000.0, window_started=19_980.0)  # 20s

    tracker._record_shot_perf(True, True, 0.0)
    # this call also bumps count 5->6, unknown 2->3, misses 1->2 before flush

    assert len(records) == 1
    rec = records[0]
    assert rec.msg == (
        "Tracker combat perf: %.1fs shots=%d unknown=%d inference_misses=%d "
        "avg_shot_ms=%.3f avg_cost_lookup_ms=%.3f"
    )
    assert rec.args is not None
    assert len(rec.args) == 6
    # arg0: elapsed (20s). arg1: shots (6). arg2: unknown (3). arg3: misses (2).
    assert rec.args[0] == pytest.approx(20.0)
    assert rec.args[1] == 6
    assert rec.args[2] == 3
    assert rec.args[3] == 2
    # arg4/arg5 are the per-shot averages: 0.6/6*1000 == 100.0, 0.12/6*1000 == 20.0.
    assert rec.args[4] == pytest.approx(100.0)
    assert rec.args[5] == pytest.approx(20.0)
    # The fully-formatted message must render without error (catches a dropped
    # positional argument or a None substituted into a %d/%f slot).
    assert rec.getMessage() == (
        "Tracker combat perf: 20.0s shots=6 unknown=3 inference_misses=2 "
        "avg_shot_ms=100.000 avg_cost_lookup_ms=20.000"
    )


def test_record_shot_perf_flush_avg_values_are_scaled_per_shot(
    tracker, fake_time, debug_records
):
    """avg_*_ms == seconds / shots * 1000. Kills the */÷ swap, the ÷-vs-* on
    the 1000 factor, and the 1000->1001 constant mutants on both averages."""
    records = debug_records
    fake_time.set_monotonic(30_000.0)
    tracker._perf_window_started = 29_980.0  # 20s
    tracker._perf_shot_count = 3  # +1 inside -> shots == 4
    tracker._perf_unknown_tool_shots = 0
    tracker._perf_inference_misses = 0
    tracker._perf_shot_seconds = 0.8
    tracker._perf_cost_lookup_seconds = 0.4
    fake_time._perf = 0.0
    fake_time.perf_step = 0.0

    tracker._record_shot_perf(False, False, 0.0)

    rec = records[0]
    shots = 4
    assert rec.args[1] == shots
    # 0.8 / 4 * 1000 == 200.0 ; 0.4 / 4 * 1000 == 100.0
    assert rec.args[4] == pytest.approx(200.0)
    assert rec.args[5] == pytest.approx(100.0)


def test_record_shot_perf_flush_zero_shots_uses_zero_average(
    tracker, fake_time, debug_records
):
    """When shots == 0 the averages fall back to 0.0 (not 1.0). Kills the
    ``else 0.0`` -> ``else 1.0`` mutants on both averages."""
    records = debug_records
    fake_time.set_monotonic(40_000.0)
    tracker._perf_window_started = 39_980.0  # 20s
    tracker._perf_shot_count = -1  # +1 inside -> shots == 0 (falsy)
    tracker._perf_unknown_tool_shots = 0
    tracker._perf_inference_misses = 0
    tracker._perf_shot_seconds = 0.5
    tracker._perf_cost_lookup_seconds = 0.5
    fake_time._perf = 0.0
    fake_time.perf_step = 0.0

    tracker._record_shot_perf(False, False, 0.0)

    rec = records[0]
    assert rec.args[1] == 0
    assert rec.args[4] == 0.0
    assert rec.args[5] == 0.0


def test_record_shot_perf_flush_boundary_at_exactly_15(
    tracker, fake_time, debug_records
):
    """elapsed == 15.0 must flush (``< 15.0`` is False). Kills ``<`` -> ``<=``."""
    records = debug_records
    _seed_for_flush(tracker, fake_time, now=50_015.0, window_started=50_000.0)  # 15.0s
    tracker._record_shot_perf(False, False, 0.0)
    assert len(records) == 1
    assert tracker._perf_shot_count == 0  # flushed


def test_record_shot_perf_no_flush_just_below_15(tracker, fake_time, debug_records):
    """elapsed == 15.5 with a ``< 15.0`` threshold flushes; a ``< 16.0``
    mutant would suppress it. Kills the 15.0 -> 16.0 constant mutant."""
    records = debug_records
    _seed_for_flush(tracker, fake_time, now=60_015.5, window_started=60_000.0)  # 15.5s
    tracker._record_shot_perf(False, False, 0.0)
    assert len(records) == 1  # orig flushes at 15.5; ``< 16.0`` would not


def test_record_shot_perf_does_not_crash_on_flush(tracker, fake_time, debug_records):
    """A normal flush computes ``now`` and ``elapsed`` as real numbers and
    completes. Kills ``now = None`` / ``elapsed = None`` (both raise on the
    subtraction / comparison)."""
    records = debug_records
    _seed_for_flush(tracker, fake_time, now=70_020.0, window_started=70_000.0)  # 20s
    tracker._record_shot_perf(False, False, 0.0)  # must not raise
    assert len(records) == 1


# ---------------------------------------------------------------------------
# reload_config
# ---------------------------------------------------------------------------


def _make_tracker(
    *,
    trifecta=False,
    mode="mob",
    manual_enabled=True,
    manual_mob=None,
):
    db = sqlite3.connect(":memory:", check_same_thread=False)
    return HuntTracker(
        EventBus(),
        db,
        weapon_attribution_trifecta_provider=lambda: trifecta,
        mob_tracking_mode_provider=lambda: mode,
        mob_tracking_tag_provider=lambda: "",
        manual_mob_entry_enabled_provider=lambda: manual_enabled,
        manual_mob_provider=lambda: manual_mob,
    )


def test_reload_config_no_session_is_a_noop():
    """Without a session ``reload_config`` returns before touching heal
    state. Kills ``if not self._session`` -> ``if self._session``."""
    t = _make_tracker(trifecta=False, manual_enabled=True, manual_mob=None)
    t._active_heal_tool_name = "Sentinel"
    t._heal_cost_per_use_ped = 9.9
    assert not t.is_tracking
    t.reload_config()
    # The else-branch reset never ran, so the sentinels survive untouched.
    assert t._active_heal_tool_name == "Sentinel"
    assert t._heal_cost_per_use_ped == 9.9


def test_reload_config_non_trifecta_resets_heal_state():
    """Non-trifecta reload resets every heal field to its default. Pins each
    reset line: tool=None, cost=0.0, reload=2.5, min/max=None, warning=False."""
    t = _make_tracker(
        trifecta=False, manual_enabled=True, manual_mob=("Atrox", "Young")
    )
    t.start_session()
    # Perturb every heal field away from its default.
    t._active_heal_tool_name = "OldHeal"
    t._heal_cost_per_use_ped = 9.9
    t._heal_reload_seconds = 7.7
    t._heal_amount_min = 5.0
    t._heal_amount_max = 50.0
    t._heal_warning_emitted = True

    t.reload_config()

    assert t._active_heal_tool_name is None  # not "" (mutant 2)
    assert t._heal_cost_per_use_ped == 0.0  # not None / 1.0 (mutants 3,4)
    assert t._heal_reload_seconds == 2.5  # not None / 3.5 (mutants 5,6)
    assert t._heal_amount_min is None  # not "" (mutant 7)
    assert t._heal_amount_max is None  # not "" (mutant 8)
    assert t._heal_warning_emitted is False  # not None / True (mutants 9,10)


def test_reload_config_manual_provider_sets_mob_state():
    """Manual entry on + provider yields (species, maturity) -> the mob is
    locked with display ``"{maturity} {species}"`` and the species/maturity
    components are stamped. Kills: provider->None, the ``is None`` flip, the
    unpack->None, display->None, and each positional-arg mutant on
    ``_set_manual_mob_state`` (None substitutions and dropped arguments)."""
    t = _make_tracker(
        trifecta=False, manual_enabled=True, manual_mob=("Atrox", "Young")
    )
    t.start_session()
    # Wipe any state start_session locked, to prove reload_config re-locks it.
    t._clear_mob_state()

    t.reload_config()

    assert t._confirmed_mob_name == "Young Atrox"  # display = maturity + species
    assert t._confirmed_mob_species == "Atrox"  # arg present, not None/dropped
    assert t._confirmed_mob_maturity == "Young"  # arg present, not None/dropped
    assert t._mob_source == "manual"
    assert t.release_current_mob() == "Young Atrox"


def test_reload_config_manual_provider_none_keeps_clean_when_unset():
    """Manual entry on, provider returns None, no prior manual lock: reload
    returns cleanly without unpacking None. Kills the ``is None`` -> ``is not
    None`` flip (which would unpack None and raise)."""
    t = _make_tracker(trifecta=False, manual_enabled=True, manual_mob=None)
    t.start_session()
    t._clear_mob_state()
    assert t._mob_source is None

    t.reload_config()  # must not raise

    assert t._confirmed_mob_name == ""
    assert t._mob_source is None


def test_reload_config_manual_provider_none_clears_prior_manual_lock():
    """Manual entry on, provider returns None, a manual mob was locked: the
    lock is cleared. Kills the inner ``_mob_source == "manual"`` comparison
    mutants (!= and the string-literal mangles)."""
    t = _make_tracker(trifecta=False, manual_enabled=True, manual_mob=None)
    t.start_session()
    t._set_manual_mob_state("Foo Bar", "Bar", "Foo")
    assert t._mob_source == "manual"

    t.reload_config()

    assert t._confirmed_mob_name == ""  # cleared
    assert t._mob_source is None


def test_reload_config_manual_disabled_clears_prior_manual_lock():
    """Manual entry disabled, a manual mob locked: the trailing
    ``if self._mob_source == "manual": clear`` fires. Kills the final
    comparison mutants (!= and the string-literal mangles)."""
    t = _make_tracker(trifecta=False, manual_enabled=False, manual_mob=None)
    t.start_session()
    t._set_manual_mob_state("Foo Bar", "Bar", "Foo")
    assert t._mob_source == "manual"

    t.reload_config()

    assert t._confirmed_mob_name == ""  # trailing branch cleared it
    assert t._mob_source is None


def test_reload_config_manual_disabled_keeps_non_manual_state():
    """Manual entry disabled, no manual lock (source is None): the trailing
    branch must NOT clear anything it didn't set. With a ``!=`` mutant the
    branch would fire on a non-manual source. We assert a tag-set state is
    preserved when source is not 'manual'."""
    t = _make_tracker(trifecta=False, manual_enabled=False, manual_mob=None)
    t.start_session()
    # Simulate a non-manual confirmed state (e.g. set by other config).
    t._set_session_tag("MyTag")
    assert t._mob_source == "tag"

    t.reload_config()

    # ``source == "manual"`` is False, so the tag state is untouched; a ``!=``
    # mutant would clear it.
    assert t._confirmed_mob_name == "MyTag"
    assert t._mob_source == "tag"
