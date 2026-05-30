"""Mutation-hardening tests for HuntTracker._record_offensive_shot.

Drives the offensive-shot accumulator directly (no event bus) so every
branch of the method is exercised deterministically:

  * the global accumulator counters (shots_fired / damage_dealt / critical_hits);
  * the trifecta attribution path: matched, unmatched (with the one-shot
    session warning and its dedup flag), and the cost/tool it yields;
  * the hotbar-tool fallback path (non-trifecta);
  * the allow_damage_inference=False (jam/dodge/evade) path that reuses the
    last offensive tool name;
  * the per-tool ToolStats counters and the phase-key routing;
  * the fallback cost selection (inferred vs equipment lookup);
  * the DEBUG-gated perf instrumentation (_record_shot_perf), which is reached
    only when the module logger is at DEBUG. perf_counter is monkeypatched to a
    deterministic clock so the accumulated perf seconds are assertable.

These all import the real backend.tracking.tracker module under test.
"""

import sqlite3

import pytest

from backend.core.event_bus import EventBus
from backend.tracking import tracker as tracker_mod
from backend.tracking.tracker import HuntTracker

# --------------------------------------------------------------------------
# Construction helpers
# --------------------------------------------------------------------------


# A weapon props payload whose damage-enhancer cost model yields a small,
# deterministic per-shot cost (0.015 PED) that differs from any equipment-cost
# lookup value. This lets a test tell the weapon-state cost path apart from the
# static equipment-lookup / else-branch fallback path.
_ENH_WEAPON_NAME = "EnhWeapon"
_ENH_WEAPON_PROPS = {
    "weapon_entity": {
        "name": _ENH_WEAPON_NAME,
        "damage": {"impact": 10.0},
        "economy": {"decay": 1.0, "ammo_burn": 50},
    },
    "weapon_markup": 100,
    "damage_enhancers": 0,
}
_ENH_WEAPON_COST_PED = 0.015


def _make_tracker(
    *, trifecta=False, equipment_cost=0.0, cost_lookup=None, profile_lookup=None
):
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(
        bus,
        db,
        equipment_cost_lookup=(cost_lookup or (lambda _: equipment_cost)),
        equipment_profile_lookup=(profile_lookup or (lambda _: None)),
        weapon_attribution_trifecta_provider=lambda: trifecta,
    )
    tracker.start_session()
    return tracker


def _shot(tracker, *, amount, is_crit=False, allow_damage_inference=True):
    tracker._record_offensive_shot(
        amount=amount,
        is_crit=is_crit,
        allow_damage_inference=allow_damage_inference,
    )


def _arm_trifecta(tracker, *, name, lo, hi, cost):
    """Register a single weapon damage profile for attribution."""
    tracker._damage_attributor.add_weapon_profile(
        name=name,
        min_damage=lo,
        max_damage=hi,
        cost_per_shot=cost,
    )


# --------------------------------------------------------------------------
# Global accumulator counters (mutmut_9 region and shots_fired)
# --------------------------------------------------------------------------


class TestGlobalAccumulator:
    def test_shots_and_damage_and_crit_accumulate(self):
        t = _make_tracker()
        _shot(t, amount=10.0, is_crit=False)
        _shot(t, amount=15.0, is_crit=True)
        acc = t.current_accumulator
        assert acc.shots_fired == 2
        assert acc.damage_dealt == pytest.approx(25.0)
        assert acc.critical_hits == 1

    def test_zero_damage_shot_still_counts_but_adds_no_damage(self):
        # Kills mutmut_9 (amount > 0 -> amount >= 0 is equivalent on amount==0,
        # but a *positive* amount must be the only thing that adds damage): a
        # negative/zero amount must not change damage_dealt while shots_fired
        # still advances. Pin the positive-add directly.
        t = _make_tracker()
        _shot(t, amount=0.0)
        acc = t.current_accumulator
        assert acc.shots_fired == 1
        assert acc.damage_dealt == 0.0

    def test_negative_amount_does_not_add_damage(self):
        # mutmut_9: `amount > 0` -> `amount >= 0` is equivalent at 0, but the
        # guard must reject negatives. A defensive negative amount must never
        # decrease damage. (Also pins the global-counter guard, not just ts.)
        t = _make_tracker()
        _shot(t, amount=-5.0)
        assert t.current_accumulator.damage_dealt == 0.0
        assert t.current_accumulator.shots_fired == 1


# --------------------------------------------------------------------------
# Hotbar-tool fallback path (non-trifecta): tool = active hotbar tool
# --------------------------------------------------------------------------


class TestHotbarFallbackPath:
    def test_active_hotbar_tool_is_attributed_and_costed(self):
        t = _make_tracker(trifecta=False, equipment_cost=0.50)
        t._active_hotbar_tool_name = "Weapon A"
        _shot(t, amount=10.0)
        stats = t.current_accumulator.tool_stats
        # tool routed under its own key with the equipment cost (mutmut_46/47/48
        # /52/55/60/61/62/63/77/78/79 region: cost lookup and phase routing).
        assert "Weapon A" in stats
        ts = stats["Weapon A"]
        assert ts.tool_name == "Weapon A"
        assert ts.shots_fired == 1
        assert ts.damage_dealt == pytest.approx(10.0)
        assert ts.cost_per_shot == pytest.approx(0.50)
        # last offensive tool remembered for the jam/dodge reuse path.
        assert t._last_offensive_tool_name == "Weapon A"

    def test_no_tool_routes_under_unknown_with_no_cost(self):
        # tool is None -> tool_key "Unknown", cost branch skipped (mutmut_48
        # invert, mutmut_46/47 current_cost default). equipment_cost would be
        # applied only via the fallback branch; here it is 0.
        t = _make_tracker(trifecta=False, equipment_cost=0.0)
        t._active_hotbar_tool_name = None
        _shot(t, amount=10.0)
        stats = t.current_accumulator.tool_stats
        assert "Unknown" in stats
        ts = stats["Unknown"]
        assert ts.tool_name == "Unknown"
        assert ts.shots_fired == 1
        assert ts.damage_dealt == pytest.approx(10.0)
        assert ts.cost_per_shot == 0.0
        # tool was None: last offensive tool must NOT be overwritten
        # (mutmut_39 invert / mutmut_40 set None).
        assert t._last_offensive_tool_name is None

    def test_unknown_tool_uses_equipment_fallback_cost_keyed_by_name(self):
        # tool is None, current_cost stays 0 -> else branch creates Unknown
        # ToolStats then back-fills cost via _equipment_cost_lookup(tool_key).
        # The lookup is name-aware so passing the WRONG key (mutmut_78:
        # _equipment_cost_lookup(None)) yields 0.0 instead of 0.25.
        def cost_lookup(name):
            return {"Unknown": 0.25}.get(name, 0.0)

        t = _make_tracker(trifecta=False, cost_lookup=cost_lookup)
        t._active_hotbar_tool_name = None
        _shot(t, amount=10.0)
        ts = t.current_accumulator.tool_stats["Unknown"]
        # _equipment_cost_lookup("Unknown") == 0.25 must land as cost_per_shot;
        # _equipment_cost_lookup(None) would be 0.0.
        assert ts.cost_per_shot == pytest.approx(0.25)

    def test_last_offensive_tool_remembered_only_when_present(self):
        # mutmut_39/40: when a tool is present it is stored as the last
        # offensive tool; a later None shot must not clear it.
        t = _make_tracker(trifecta=False)
        t._active_hotbar_tool_name = "Rifle"
        _shot(t, amount=5.0)
        assert t._last_offensive_tool_name == "Rifle"
        t._active_hotbar_tool_name = None
        _shot(t, amount=5.0)
        # still Rifle (None did not overwrite)
        assert t._last_offensive_tool_name == "Rifle"


# --------------------------------------------------------------------------
# Trifecta attribution path: matched / unmatched
# --------------------------------------------------------------------------


class TestTrifectaMatched:
    def test_matched_attribution_sets_tool_and_inferred_cost(self):
        # mutmut_19 (skip match), 20/21 (arg swap), 22/23 (drop arg -> TypeError),
        # 34 (invert is not None), 35 (tool None), 36 (inferred None).
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0, is_crit=False)
        stats = t.current_accumulator.tool_stats
        assert "Laser" in stats
        ts = stats["Laser"]
        assert ts.tool_name == "Laser"
        assert ts.shots_fired == 1
        assert ts.damage_dealt == pytest.approx(10.0)
        # inferred cost from the attribution drives cost_per_shot (current_cost
        # branch with current_cost > 0 -> _tool_stats_for_phase).
        assert ts.cost_per_shot == pytest.approx(0.40)
        # matched -> tool remembered
        assert t._last_offensive_tool_name == "Laser"
        # matched -> no unmatched warning emitted
        assert t._session_warnings == []

    def test_crit_flag_changes_attribution_range(self):
        # mutmut_21 (critical=is_crit -> critical=None) and mutmut_23 (the
        # critical kwarg dropped -> defaults to False): both turn a crit shot
        # into a non-crit match. A weapon with regular range [8, 12] has crit
        # range [16, 36] (min*2 .. max*3). An amount of 24.0 matches ONLY under
        # the critical bounds, so:
        #   is_crit=True  -> matched -> "Laser", no warning
        #   is_crit=False -> unmatched -> "Unknown", warning
        # If the crit flag is forced off (the mutants), the crit shot becomes
        # unmatched and routes to "Unknown" with a session warning.
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=24.0, is_crit=True)
        assert set(t.current_accumulator.tool_stats.keys()) == {"Laser"}
        assert t._session_warnings == []

    def test_non_crit_at_crit_only_amount_is_unmatched(self):
        # The mirror of the above: a non-crit shot at 24.0 must NOT match, so the
        # crit-flag distinction is real (guards against the crit test passing for
        # the wrong reason).
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=24.0, is_crit=False)
        assert set(t.current_accumulator.tool_stats.keys()) == {"Unknown"}
        assert t._session_warnings == [
            "Trifecta attribution: damage fell outside both weapon ranges"
        ]

    def test_amount_argument_drives_attribution(self):
        # mutmut_20: match_damage(None, ...) would break attribution entirely.
        # A 10.0 hit inside [8, 12] must attribute to Laser.
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0, is_crit=False)
        assert set(t.current_accumulator.tool_stats.keys()) == {"Laser"}

    def test_matched_inferred_cost_wins_over_equipment_lookup(self):
        # mutmut_55: dropping inferred_cost=inferred_cost from the
        # _current_cost_for_tool call falls back to the equipment lookup. Make
        # the equipment lookup return a DIFFERENT (also positive) cost so the
        # phase-routed cost differs: real -> inferred 0.40; mutant -> lookup 0.70.
        def cost_lookup(name):
            return {"Laser": 0.70}.get(name, 0.0)

        t = _make_tracker(trifecta=True, cost_lookup=cost_lookup)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0, is_crit=False)
        ts = t.current_accumulator.tool_stats["Laser"]
        assert ts.cost_per_shot == pytest.approx(0.40)


class TestTrifectaUnmatched:
    def test_unmatched_emits_single_session_warning(self):
        # mutmut_24 (and->or), 25 (is None->is not None), 26 (drop not),
        # 27/28/29/30 (msg text), 31 (append None), 32 (flag None), 33 (flag
        # False -> dedup broken -> two warnings).
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        # 100.0 is outside the 8..12 range -> no attribution.
        _shot(t, amount=100.0)
        assert t._session_warnings == [
            "Trifecta attribution: damage fell outside both weapon ranges"
        ]
        assert t._trifecta_unmatched_warning_emitted is True
        # The dedup flag must suppress a second warning (kills mutmut_33: flag
        # set to False; mutmut_32: flag set to None still truthy but the
        # exact-string list-equality below also pins append behaviour).
        _shot(t, amount=200.0)
        assert t._session_warnings == [
            "Trifecta attribution: damage fell outside both weapon ranges"
        ]

    def test_unmatched_flag_starts_false_and_is_truthy_after(self):
        # mutmut_32: `_trifecta_unmatched_warning_emitted = None`. The flag must
        # be exactly True after the first unmatched shot (None would be falsy and
        # let a second warning through; also pins the literal).
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        assert t._trifecta_unmatched_warning_emitted is False
        _shot(t, amount=100.0)
        assert t._trifecta_unmatched_warning_emitted is True
        assert len(t._session_warnings) == 1

    def test_matched_shot_does_not_warn(self):
        # mutmut_25/26: inverting the warning condition would warn on a *matched*
        # shot. A clean match must leave the warning list empty.
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0)
        assert t._session_warnings == []
        assert t._trifecta_unmatched_warning_emitted is False


# --------------------------------------------------------------------------
# allow_damage_inference=False: jam/dodge/evade reuse last offensive tool
# --------------------------------------------------------------------------


class TestNoInferencePath:
    def test_countered_shot_reuses_last_offensive_tool(self):
        # mutmut_37: else branch `tool = self._last_offensive_tool_name` -> None.
        # First land a real attributed shot to set the last offensive tool, then
        # a jam (allow_damage_inference=False) must attribute to that same tool.
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0)  # matched -> last offensive tool = Laser
        _shot(t, amount=0.0, allow_damage_inference=False)  # jam
        ts = t.current_accumulator.tool_stats["Laser"]
        # two shots accrued to Laser: one damaging, one countered.
        assert ts.shots_fired == 2
        assert ts.damage_dealt == pytest.approx(10.0)

    def test_countered_shot_without_prior_tool_is_unknown(self):
        # When there is no prior offensive tool, the jam path yields tool None ->
        # "Unknown" key. Pins that the no-inference branch does not call the
        # attributor (which would otherwise match nothing on amount 0 anyway).
        t = _make_tracker(trifecta=True)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=0.0, allow_damage_inference=False)
        assert "Unknown" in t.current_accumulator.tool_stats
        assert t._session_warnings == []  # no attribution attempted -> no warn


# --------------------------------------------------------------------------
# Per-tool ToolStats counters and phase routing
# --------------------------------------------------------------------------


class TestToolStatsCounters:
    def test_tool_stats_crit_increments_not_assigns(self):
        # mutmut_89: `ts.critical_hits += 1` -> `= 1`. Two crit shots on the same
        # tool must yield 2.
        t = _make_tracker(trifecta=False, equipment_cost=0.10)
        t._active_hotbar_tool_name = "Gun"
        _shot(t, amount=5.0, is_crit=True)
        _shot(t, amount=5.0, is_crit=True)
        ts = t.current_accumulator.tool_stats["Gun"]
        assert ts.critical_hits == 2
        assert ts.shots_fired == 2

    def test_tool_stats_damage_only_on_positive(self):
        # mutmut_85: `amount > 0` -> `>= 0` for ts.damage_dealt is equivalent at
        # zero; pin that a negative amount does not subtract from ts.damage.
        t = _make_tracker(trifecta=False, equipment_cost=0.10)
        t._active_hotbar_tool_name = "Gun"
        _shot(t, amount=8.0)
        _shot(t, amount=-3.0)
        ts = t.current_accumulator.tool_stats["Gun"]
        assert ts.damage_dealt == pytest.approx(8.0)
        assert ts.shots_fired == 2

    def test_cost_zero_tool_routes_to_else_branch_not_phase(self):
        # mutmut_60 (and->or), 61 (tool None), 62 (>0 -> >=0), 63 (>0 -> >1):
        # a tool present but with current_cost == 0 must route through the else
        # branch (plain tool_key entry), NOT _tool_stats_for_phase. With cost 0
        # the key is exactly the tool name and cost_per_shot stays 0.
        t = _make_tracker(trifecta=False, equipment_cost=0.0)
        t._active_hotbar_tool_name = "FreeTool"
        _shot(t, amount=4.0)
        stats = t.current_accumulator.tool_stats
        assert set(stats.keys()) == {"FreeTool"}
        assert stats["FreeTool"].cost_per_shot == 0.0

    def test_positive_cost_tool_routes_through_phase(self):
        # mutmut_62/63: current_cost > 0 must be true for a 0.50 cost so the
        # phase routing (cost-bearing) path is taken; cost lands on the stats.
        t = _make_tracker(trifecta=False, equipment_cost=0.50)
        t._active_hotbar_tool_name = "PaidTool"
        _shot(t, amount=4.0)
        ts = t.current_accumulator.tool_stats["PaidTool"]
        assert ts.cost_per_shot == pytest.approx(0.50)
        assert ts.shots_fired == 1


class TestPhaseVsElseRouting:
    """Distinguish the cost-bearing phase route from the else-branch fallback.

    A weapon profile is supplied via equipment_profile_lookup so
    _current_cost_for_tool returns the weapon-state cost (0.015 PED), while the
    static equipment_cost_lookup returns a DIFFERENT, larger value (0.99). The
    two costs only agree if the wrong path is taken, so cost_per_shot pins which
    branch ran.
    """

    def _tracker(self):
        def profile_lookup(name):
            return _ENH_WEAPON_PROPS if name == _ENH_WEAPON_NAME else None

        def cost_lookup(name):
            # Deliberately != the weapon-state cost and != 0 so any fall-through
            # to the static lookup (or to lookup(None)) is observable.
            return {_ENH_WEAPON_NAME: 0.99, None: 0.0}.get(name, 0.0)

        t = _make_tracker(
            trifecta=False, cost_lookup=cost_lookup, profile_lookup=profile_lookup
        )
        t._active_hotbar_tool_name = _ENH_WEAPON_NAME
        return t

    def test_weapon_state_cost_drives_phase_routing(self):
        # mutmut_52 (_current_cost_for_tool(None) -> no profile -> lookup 0.99),
        # mutmut_61 (tool is None and current_cost>0 -> else branch, fallback
        # lookup 0.99), mutmut_63 (current_cost > 1 -> for 0.015 this is False ->
        # else branch, fallback lookup 0.99). All three would yield 0.99 instead
        # of the weapon-state 0.015.
        t = self._tracker()
        _shot(t, amount=10.0)
        ts = t.current_accumulator.tool_stats[_ENH_WEAPON_NAME]
        assert ts.cost_per_shot == pytest.approx(_ENH_WEAPON_COST_PED)
        assert ts.cost_per_shot != pytest.approx(0.99)


# --------------------------------------------------------------------------
# DEBUG-gated perf instrumentation (_record_shot_perf)
# --------------------------------------------------------------------------


@pytest.fixture
def debug_perf(monkeypatch):
    """Enable DEBUG on the tracker logger and install a deterministic clock.

    perf_counter advances by exactly 1.0 each call; monotonic is frozen so the
    15s perf-window flush never fires. This makes _perf_shot_seconds and
    _perf_cost_lookup_seconds exactly predictable.
    """
    monkeypatch.setattr(tracker_mod.log, "isEnabledFor", lambda level: True)

    counter = {"t": 0.0}

    def fake_perf_counter():
        counter["t"] += 1.0
        return counter["t"]

    monkeypatch.setattr(tracker_mod._time, "perf_counter", fake_perf_counter)
    monkeypatch.setattr(tracker_mod._time, "monotonic", lambda: 0.0)
    return counter


class TestDebugPerfInstrumentation:
    def test_perf_shot_count_advances_when_debug(self, debug_perf):
        # mutmut_2: debug_perf = None -> perf branch skipped -> count never
        # advances. With DEBUG on the real code must record the shot.
        t = _make_tracker(trifecta=False, equipment_cost=0.10)
        t._active_hotbar_tool_name = "Gun"
        _shot(t, amount=5.0)
        assert t._perf_shot_count == 1
        # shot_started/lookup_started subtractions are deterministic with the
        # fake clock: each is a finite positive number (kills mutmut_4 None ->
        # TypeError, mutmut_49 None -> TypeError).
        assert t._perf_shot_seconds > 0.0
        assert t._perf_cost_lookup_seconds > 0.0

    def test_perf_cost_lookup_accumulates_across_shots(self, debug_perf):
        # mutmut_56 (= instead of +=), 57 (-=), 58 (+ instead of -). With the
        # deterministic clock each shot's cost-lookup delta is exactly 1.0 (one
        # perf_counter tick around the lookup). Two shots -> +=: 2.0; =: 1.0;
        # -=: negative; +: large (two absolute timestamps summed).
        t = _make_tracker(trifecta=False, equipment_cost=0.10)
        t._active_hotbar_tool_name = "Gun"
        _shot(t, amount=5.0)
        _shot(t, amount=5.0)
        # exactly two 1.0 deltas accumulated.
        assert t._perf_cost_lookup_seconds == pytest.approx(2.0)

    def test_perf_unknown_tool_counted_for_none_tool(self, debug_perf):
        # mutmut_92 (first arg None), 98 (tool is not None), 95 (arg dropped ->
        # TypeError). A None-tool shot must increment the unknown-tool counter.
        t = _make_tracker(trifecta=False, equipment_cost=0.0)
        t._active_hotbar_tool_name = None
        _shot(t, amount=5.0)
        assert t._perf_unknown_tool_shots == 1
        assert t._perf_inference_misses == 0

    def test_perf_unknown_tool_not_counted_for_known_tool(self, debug_perf):
        # mutmut_98: `tool is None` -> `tool is not None` would count a known
        # tool as unknown.
        t = _make_tracker(trifecta=False, equipment_cost=0.10)
        t._active_hotbar_tool_name = "Gun"
        _shot(t, amount=5.0)
        assert t._perf_unknown_tool_shots == 0

    def test_perf_inference_miss_counted_on_trifecta_unmatched(self, debug_perf):
        # mutmut_93 (2nd arg None), 96 (2nd arg dropped -> TypeError), 99/100/101
        # (boolean restructure of the inference-miss expression). An unmatched
        # trifecta inference shot (tool ends up None) is an inference miss.
        t = _make_tracker(trifecta=True, equipment_cost=0.0)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=100.0, allow_damage_inference=True)  # unmatched
        assert t._perf_inference_misses == 1
        assert t._perf_unknown_tool_shots == 1  # tool None -> also unknown

    def test_perf_inference_miss_not_counted_when_inference_disabled(self, debug_perf):
        # mutmut_99/100/101: the inference-miss flag is
        # `trifecta AND allow_inference AND tool is None`. A countered shot
        # (allow_damage_inference=False) with no prior tool is unknown but NOT an
        # inference miss. mutmut_100 (or allow_inference) / 99 (or tool is None)
        # / 101 (and tool is not None) all change this.
        t = _make_tracker(trifecta=True, equipment_cost=0.0)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=0.0, allow_damage_inference=False)
        assert t._perf_unknown_tool_shots == 1
        assert t._perf_inference_misses == 0

    def test_perf_inference_miss_not_counted_when_matched(self, debug_perf):
        # mutmut_101: `and tool is None` -> `and tool is not None` would flag a
        # *matched* shot as an inference miss.
        t = _make_tracker(trifecta=True, equipment_cost=0.0)
        _arm_trifecta(t, name="Laser", lo=8.0, hi=12.0, cost=0.40)
        _shot(t, amount=10.0, allow_damage_inference=True)  # matched
        assert t._perf_inference_misses == 0
        assert t._perf_unknown_tool_shots == 0

    def test_perf_no_inference_miss_when_not_trifecta(self, debug_perf):
        # mutmut_100: `trifecta OR allow_inference` would flag a non-trifecta
        # unknown shot (allow_inference True) as an inference miss.
        t = _make_tracker(trifecta=False, equipment_cost=0.0)
        t._active_hotbar_tool_name = None
        _shot(t, amount=5.0, allow_damage_inference=True)
        assert t._perf_unknown_tool_shots == 1
        assert t._perf_inference_misses == 0
