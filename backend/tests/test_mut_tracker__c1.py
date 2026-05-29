"""Mutation-killing tests for the tracker__c1 cluster.

Targets four private HuntTracker helpers that resolve a weapon profile, build
and cache per-weapon damage-enhancer state, compute the current cost for a tool,
and select/create the per-phase ToolStats bucket:

    HuntTracker._match_weapon_profile
    HuntTracker._ensure_weapon_state
    HuntTracker._current_cost_for_tool
    HuntTracker._tool_stats_for_phase

These are internal, but they carry the cost-attribution arithmetic of the
tracking engine and are reached only through stateful event replay in the
existing suites, which leaves their reducers' edges unmutated. Here we drive the
real production methods directly on a HuntTracker built against in-memory SQLite
and injected lookups, and assert the exact behaviour each mutation breaks.
"""

from __future__ import annotations

import sqlite3

import pytest

from backend.core.event_bus import EventBus
from backend.tracking.tracker import HuntTracker, _Accumulator
from backend.tracking.models import ToolStats

# A weapon profile whose canonical weapon name differs from the observed tool
# name, so name-resolution mutations become visible. 2 configured damage
# enhancers make the enhancer-aware cost depend on the live stack state.
PROFILE = {
    "weapon_entity": {
        "name": "CanonicalGun",
        "economy": {"decay": 1.0, "ammo_burn": 200.0},
    },
    "damage_enhancers": 2,
}
OBSERVED = "ObservedGun"
CANONICAL = "CanonicalGun"


def _make_tracker(*, cost=0.5, profile_for=OBSERVED, profile=PROFILE):
    """Build a HuntTracker over in-memory SQLite with injected lookups."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()

    def profile_lookup(name):
        return profile if name == profile_for else None

    return HuntTracker(
        bus,
        db,
        equipment_cost_lookup=lambda _name: cost,
        equipment_profile_lookup=profile_lookup,
    )


# ---------------------------------------------------------------------------
# _match_weapon_profile
# ---------------------------------------------------------------------------


def test_match_uses_preloaded_trifecta_profile():
    """A profile already in _trifecta_weapon_profiles short-circuits the lookup.

    Kills mut_1 (trifecta get -> None) and mut_2 (get(None) instead of get(name)):
    both make the preloaded branch miss, so the tool would fall through to the
    (here empty) equipment lookup and return None.
    """
    t = _make_tracker(profile_for="never")
    t._trifecta_weapon_profiles = {OBSERVED: PROFILE}
    match = t._match_weapon_profile(OBSERVED)
    assert match is not None
    name, prof = match
    assert name == OBSERVED
    assert prof is PROFILE


def test_match_resolves_via_equipment_lookup_and_canonical_name():
    """Equipment lookup yields a profile; canonical name comes from weapon_entity.

    Kills mut_4 (lookup result discarded -> None), mut_5 (lookup(None)),
    mut_8 (canonical_name = None), mut_9 (`and` instead of `or`), mut_19
    (match = None). With a hit the method must return (CanonicalGun, profile).
    """
    t = _make_tracker()
    match = t._match_weapon_profile(OBSERVED)
    assert match is not None
    name, prof = match
    assert name == CANONICAL
    assert prof is PROFILE


def test_match_canonical_name_falls_back_to_tool_name_when_unnamed():
    """When the profile has no weapon name, canonical falls back to tool_name.

    Reinforces mut_9 (`name and tool_name` would yield None when name is missing):
    the `or tool_name` fallback must produce the observed tool name.
    """
    profile = {"weapon_entity": {"economy": {"decay": 1.0}}, "damage_enhancers": 0}
    t = _make_tracker(profile=profile)
    match = t._match_weapon_profile(OBSERVED)
    assert match is not None
    name, prof = match
    assert name == OBSERVED


def test_match_caches_miss_as_none_not_empty_string():
    """A lookup miss is cached as exactly None and returned as None on re-query.

    Kills mut_7 (cache miss stored as "" instead of None): the second call hits
    the cache and would return "" rather than None.
    """
    t = _make_tracker(profile_for="never")
    assert t._match_weapon_profile("Missing") is None
    # Stored value must be None (not a falsy "").
    assert t._profile_match_cache["Missing"] is None
    assert t._match_weapon_profile("Missing") is None


def test_match_reads_name_from_weapon_entity_keys():
    """Canonical name extraction reads profile['weapon_entity']['name'].

    Kills the key-string and structure mutations:
      mut_10 weapon_entity.get(None)
      mut_11 profile.get(None, {})
      mut_12 profile.get('weapon_entity', None) (AttributeError when absent)
      mut_14 profile.get('weapon_entity', )    (AttributeError when absent)
      mut_15 'XXweapon_entityXX'
      mut_16 'WEAPON_ENTITY'
      mut_17 'XXnameXX'
      mut_18 'NAME'
    All of these would fail to read CanonicalGun (returning the tool name or
    raising), so the canonical name must be CanonicalGun.
    """
    t = _make_tracker()
    name, _prof = t._match_weapon_profile(OBSERVED)
    assert name == CANONICAL


def test_match_default_dict_guards_missing_weapon_entity():
    """A profile lacking weapon_entity must not raise; canonical -> tool_name.

    Kills mut_12 and mut_14 (default replaced by None -> .get on None raises):
    the production `{}` default keeps the chained .get safe.
    """
    profile = {"damage_enhancers": 0}  # no weapon_entity key
    t = _make_tracker(profile=profile)
    match = t._match_weapon_profile(OBSERVED)
    assert match is not None
    name, _prof = match
    assert name == OBSERVED


def test_match_caches_hit_and_returns_it_on_re_query():
    """A successful match is cached and re-returned identically.

    Kills mut_20 (cache stores None instead of the match): the first call is
    correct, but the second call hits the cache and would return None.
    """
    t = _make_tracker()
    first = t._match_weapon_profile(OBSERVED)
    assert first is not None
    assert t._profile_match_cache[OBSERVED] == first
    second = t._match_weapon_profile(OBSERVED)
    assert second is not None
    assert second[0] == CANONICAL


# ---------------------------------------------------------------------------
# _ensure_weapon_state
# ---------------------------------------------------------------------------


def test_ensure_builds_state_for_matched_weapon():
    """A matched weapon yields a damage-enhancer state with the canonical name.

    Kills mut_1 (match = None -> early None return), mut_2 (match(None)),
    mut_9 (`is not None` inverts so state stays None -> returns None),
    mut_10 (state = None after build -> returns None), mut_11 (from_props(None,..)
    -> wrong tool_name), mut_22 of from_props path indirectly.
    """
    t = _make_tracker()
    state = t._ensure_weapon_state(OBSERVED)
    assert state is not None
    assert state.tool_name == CANONICAL
    assert state.current_cost_ped() == pytest.approx(0.036)


def test_ensure_clears_active_key_and_records_observed_on_miss():
    """On a profile miss the active key resets to None and observed name is set.

    Kills mut_4 (active key set to "" instead of None) and mut_5 (observed name
    set to None instead of tool_name).
    """
    t = _make_tracker(profile_for="never")
    result = t._ensure_weapon_state("MysteryTool")
    assert result is None
    assert t._active_weapon_state_key is None
    assert t._active_weapon_observed_name == "MysteryTool"


def test_ensure_caches_state_and_preserves_it_across_calls():
    """The built state is cached by canonical name and reused on the next call.

    Kills mut_7 (get -> None forces rebuild), mut_8 (get(None) forces rebuild),
    mut_15 (dict entry stored as None forces rebuild). A second ensure must
    return the very same object, preserving live enhancer-stack mutation.
    """
    t = _make_tracker()
    state = t._ensure_weapon_state(OBSERVED)
    # Mutate live state so a rebuild from props would be detectable.
    state.set_total(50)
    marked_cost = state.current_cost_ped()
    assert t._weapon_enhancer_states[CANONICAL] is state

    again = t._ensure_weapon_state(OBSERVED)
    assert again is state
    assert again.current_cost_ped() == marked_cost


def test_ensure_sets_active_key_and_observed_name_on_match():
    """After a match the active key is the canonical name; observed is the tool.

    Kills mut_16 (active key set to None -> _active_weapon_state() returns None)
    and mut_17 (observed name set to None instead of tool_name).
    """
    t = _make_tracker()
    state = t._ensure_weapon_state(OBSERVED)
    assert t._active_weapon_state_key == CANONICAL
    assert t._active_weapon_observed_name == OBSERVED
    assert t._active_weapon_state() is state


# ---------------------------------------------------------------------------
# _current_cost_for_tool
# ---------------------------------------------------------------------------


def test_cost_prefers_live_weapon_state():
    """A live weapon state wins over inferred cost and static lookup.

    Kills mut_2 (state = None -> ignores state) and mut_3 (ensure(None)).
    The enhancer-aware state cost (0.036) must be returned, not the 0.5 lookup.
    """
    t = _make_tracker(cost=0.5)
    cost = t._current_cost_for_tool(OBSERVED, inferred_cost=0.0)
    assert cost == pytest.approx(0.036)


def test_cost_default_inferred_is_zero_so_lookup_runs():
    """With no state and no inferred cost, the static lookup result is returned.

    Guards the default-argument contract of inferred_cost (it must be falsy so
    control reaches the equipment cost lookup, returning 0.5). NOTE: the matching
    mutant (mut_1, default 0.0 -> 1.0) is an equivalent under mutmut's trampoline:
    the dispatcher wrapper keeps the original 0.0 default and always forwards
    inferred_cost positionally, so the mutated default on the mutant body is dead
    and unobservable. The assertion still documents the intended behaviour.
    """
    t = _make_tracker(profile_for="never", cost=0.5)
    assert t._current_cost_for_tool("Plain") == pytest.approx(0.5)


def test_cost_uses_inferred_only_when_strictly_positive():
    """Inferred cost is used only when strictly greater than zero.

    Kills mut_5 (`>= 0` -> returns 0.0 inferred instead of lookup) and mut_6
    (`> 1` -> ignores a 0.5 inferred and falls through to lookup).
    """
    # inferred_cost == 0.0: must fall through to lookup (0.9), not return 0.0.
    t0 = _make_tracker(profile_for="never", cost=0.9)
    assert t0._current_cost_for_tool("Plain", inferred_cost=0.0) == pytest.approx(0.9)

    # inferred_cost == 0.5 (between 0 and 1): must be used, not the lookup (0.9).
    t1 = _make_tracker(profile_for="never", cost=0.9)
    assert t1._current_cost_for_tool("Plain", inferred_cost=0.5) == pytest.approx(0.5)


def test_cost_caches_static_lookup_result():
    """The static lookup runs once and the cached value is reused thereafter.

    Kills mut_7 (cached = None forces re-lookup) and mut_8 (cache.get(None)),
    and mut_12 (cache stores None forcing re-lookup). The lookup is counted; a
    second query must not re-invoke it and must return the original value.
    """
    calls = {"n": 0}

    def counting_lookup(_name):
        calls["n"] += 1
        return 0.5

    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    t = HuntTracker(
        bus,
        db,
        equipment_cost_lookup=counting_lookup,
        equipment_profile_lookup=lambda _n: None,
    )
    assert t._current_cost_for_tool("Plain") == pytest.approx(0.5)
    assert calls["n"] == 1
    assert t._static_tool_cost_cache["Plain"] == pytest.approx(0.5)
    # Second query is served from cache; lookup count stays at 1.
    assert t._current_cost_for_tool("Plain") == pytest.approx(0.5)
    assert calls["n"] == 1


def test_cost_lookup_receives_tool_name():
    """The static lookup is keyed by the tool name, not None.

    Kills mut_11 (_equipment_cost_lookup(None)): the value returned depends on
    the tool name passed to the lookup.
    """
    seen = []

    def name_lookup(name):
        seen.append(name)
        return 0.7 if name == "Plain" else 0.0

    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    t = HuntTracker(
        bus,
        db,
        equipment_cost_lookup=name_lookup,
        equipment_profile_lookup=lambda _n: None,
    )
    assert t._current_cost_for_tool("Plain") == pytest.approx(0.7)
    assert seen == ["Plain"]


# ---------------------------------------------------------------------------
# _tool_stats_for_phase
# ---------------------------------------------------------------------------


def test_phase_requires_accumulator_with_exact_message():
    """Without an accumulator the method raises with the exact message.

    Kills mut_2 (RuntimeError(None)), mut_3 ('XXNo accumulator availableXX'),
    mut_4 ('no accumulator available'), mut_5 ('NO ACCUMULATOR AVAILABLE').
    """
    t = _make_tracker()
    assert t._accumulator is None
    with pytest.raises(RuntimeError) as excinfo:
        t._tool_stats_for_phase("Gun", 0.5)
    assert str(excinfo.value) == "No accumulator available"


def test_phase_returns_existing_stats_for_same_tool_and_cost():
    """A matching tool name + equal cost reuses the existing ToolStats object.

    Kills mut_6 (`==` inverts the skip so matches are skipped), mut_8
    (abs(None) -> TypeError on a match), mut_9 (`+` instead of `-` so equal
    costs never match), mut_22 (tool_name=None on the created stats).
    """
    t = _make_tracker()
    t._accumulator = _Accumulator()
    first = t._tool_stats_for_phase("Gun", 0.5)
    assert first.tool_name == "Gun"
    again = t._tool_stats_for_phase("Gun", 0.5)
    assert again is first
    assert list(t._accumulator.tool_stats.keys()) == ["Gun"]


def test_phase_skips_non_matching_tool_without_breaking_scan():
    """A non-matching tool earlier in the dict must not abort the scan.

    Kills mut_7 (`continue` -> `break`): with a non-matching entry first, a
    break would stop before the matching entry and create a duplicate bucket.
    """
    t = _make_tracker()
    t._accumulator = _Accumulator()
    # Seed a non-matching tool first, then the target tool at equal cost.
    t._accumulator.tool_stats["Other"] = ToolStats(tool_name="Other", cost_per_shot=0.5)
    target = ToolStats(tool_name="Gun", cost_per_shot=0.5)
    t._accumulator.tool_stats["Gun"] = target
    result = t._tool_stats_for_phase("Gun", 0.5)
    assert result is target
    assert list(t._accumulator.tool_stats.keys()) == ["Other", "Gun"]


def test_phase_cost_tolerance_is_tight():
    """A different cost for the same tool spawns a new phase bucket.

    Kills mut_11 (tolerance widened to ~1.0 so 0.5 vs 1.0 would falsely match).
    """
    t = _make_tracker()
    t._accumulator = _Accumulator()
    first = t._tool_stats_for_phase("Gun", 0.5)
    second = t._tool_stats_for_phase("Gun", 1.0)
    assert second is not first
    assert "Gun#2" in t._accumulator.tool_stats


def test_phase_key_uses_count_of_matching_tools_plus_one():
    """The new phase key is tool#<matching_count + 1>.

    Kills mut_14 (count uses 2 per match -> Gun#3), mut_15 (counts non-matching
    tools), mut_19 (count - 1 -> Gun#0), mut_20 (count + 2 -> Gun#3). With one
    existing matching entry the second distinct-cost bucket must be 'Gun#2'.
    """
    t = _make_tracker()
    t._accumulator = _Accumulator()
    # One unrelated tool (mut_15 would miscount it) plus one matching entry.
    t._accumulator.tool_stats["Unrelated"] = ToolStats(
        tool_name="Unrelated", cost_per_shot=0.1
    )
    t._tool_stats_for_phase("Gun", 0.5)  # creates key "Gun", matching_count base
    t._tool_stats_for_phase("Gun", 0.9)  # distinct cost -> new phase bucket
    keys = list(t._accumulator.tool_stats.keys())
    assert "Gun#2" in keys
    assert "Gun#0" not in keys
    assert "Gun#3" not in keys
