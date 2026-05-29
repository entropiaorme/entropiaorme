"""Mutation-hardening tests for HuntTracker._load_trifecta_weapon_profiles.

Cluster tracker__c5. These tests drive the trifecta-profile loader directly
with a controlled ``trifecta_resolver`` and assert every state field it sets,
the early-return on an empty trifecta, the per-weapon loop (both keys, the
``not weapon`` skip, the damage-attributor population, and the conditional
weapon_props capture), and the heal-tool block (every assigned field plus the
falsy guard). Each assertion pins one mutated line so the surviving mutants are
driven to a deterministic kill.
"""

import sqlite3

from backend.core.event_bus import EventBus
from backend.tracking.tracker import HuntTracker


def _make_tracker(trifecta):
    """Build a HuntTracker whose trifecta_resolver yields ``trifecta``."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    return HuntTracker(bus, db, trifecta_resolver=lambda: trifecta)


def _full_trifecta():
    """A trifecta with two weapons (props on each) and a heal tool."""
    return {
        "small_weapon": {
            "name": "Small Blaster",
            "damage_min": 10.0,
            "damage_max": 20.0,
            "total_damage": 18.0,
            "cost_per_shot_ped": 0.05,
            "role": "small",
            "weapon_props": {"amp": "104", "tier": 3},
        },
        "big_weapon": {
            "name": "Big Cannon",
            "damage_min": 90.0,
            "damage_max": 130.0,
            "total_damage": 120.0,
            "cost_per_shot_ped": 0.42,
            "role": "big",
            "weapon_props": {"amp": "115", "tier": 7},
        },
        "heal_tool": {
            "name": "Healer Mk.III",
            "cost_per_use_ped": 0.33,
            "reload_seconds": 1.75,
            "heal_min": 40.0,
            "heal_max": 60.0,
        },
    }


def _dirty_state(tracker):
    """Pre-load non-default values so a mutant that skips a reset shows up."""
    tracker._damage_attributor.add_weapon_profile(
        name="Stale Weapon", min_damage=1.0, max_damage=2.0
    )
    tracker._active_heal_tool_name = "Stale Heal"
    tracker._heal_cost_per_use_ped = 9.99
    tracker._heal_reload_seconds = 99.0
    tracker._heal_amount_min = 11.0
    tracker._heal_amount_max = 22.0
    tracker._heal_warning_emitted = True
    tracker._trifecta_weapon_profiles = {"Stale": {"x": 1}}
    tracker._active_weapon_state_key = "stale-key"
    tracker._active_weapon_observed_name = "stale-name"


# ---------------------------------------------------------------------------
# Reset block: every field is reset to its documented default before the
# resolver is consulted. Pre-dirty each field, then load an EMPTY trifecta so
# only the reset block runs (the early return fires) and the assertions pin the
# reset values exactly.
# ---------------------------------------------------------------------------


def test_reset_block_clears_all_state_before_empty_return():
    tracker = _make_tracker(trifecta=None)
    _dirty_state(tracker)

    tracker._load_trifecta_weapon_profiles()

    # _damage_attributor.clear() must have wiped the stale profile.
    assert tracker._damage_attributor._profiles == {}
    assert tracker._active_heal_tool_name is None
    assert tracker._heal_cost_per_use_ped == 0.0
    assert tracker._heal_reload_seconds == 2.5
    assert tracker._heal_amount_min is None
    assert tracker._heal_amount_max is None
    assert tracker._heal_warning_emitted is False
    assert tracker._trifecta_weapon_profiles == {}
    assert tracker._active_weapon_state_key is None
    assert tracker._active_weapon_observed_name is None


def test_reset_heal_reload_default_is_exactly_2_5():
    # Pins the literal 2.5 (guards a numeric mutation of the reload default).
    tracker = _make_tracker(trifecta=None)
    tracker._heal_reload_seconds = 123.0
    tracker._load_trifecta_weapon_profiles()
    assert tracker._heal_reload_seconds == 2.5


def test_reset_heal_cost_default_is_exactly_zero():
    tracker = _make_tracker(trifecta=None)
    tracker._heal_cost_per_use_ped = 7.0
    tracker._load_trifecta_weapon_profiles()
    assert tracker._heal_cost_per_use_ped == 0.0


# ---------------------------------------------------------------------------
# Early return: a falsy trifecta means none of the weapon/heal state changes.
# ---------------------------------------------------------------------------


def test_empty_trifecta_returns_without_loading_weapons_or_heal():
    tracker = _make_tracker(trifecta={})  # falsy -> early return
    tracker._load_trifecta_weapon_profiles()
    assert tracker._damage_attributor._profiles == {}
    assert tracker._trifecta_weapon_profiles == {}
    assert tracker._active_heal_tool_name is None


def test_none_trifecta_returns_without_loading():
    tracker = _make_tracker(trifecta=None)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._damage_attributor._profiles == {}
    assert tracker._active_heal_tool_name is None


# ---------------------------------------------------------------------------
# Weapon loop: BOTH keys ("small_weapon", "big_weapon") are iterated, each
# present weapon is added to the damage attributor with exactly its fields, and
# weapon_props are captured only when present.
# ---------------------------------------------------------------------------


def test_both_weapon_keys_are_loaded_into_damage_attributor():
    tracker = _make_tracker(_full_trifecta())
    tracker._load_trifecta_weapon_profiles()

    profiles = tracker._damage_attributor._profiles
    assert set(profiles) == {"Small Blaster", "Big Cannon"}


def test_small_weapon_profile_fields_are_exact():
    tracker = _make_tracker(_full_trifecta())
    tracker._load_trifecta_weapon_profiles()

    p = tracker._damage_attributor._profiles["Small Blaster"]
    assert p.name == "Small Blaster"
    assert p.min_damage == 10.0
    assert p.max_damage == 20.0
    assert p.base_damage == 18.0  # from total_damage, not max_damage
    assert p.cost_per_shot == 0.05
    assert p.role == "small"


def test_big_weapon_profile_fields_are_exact():
    tracker = _make_tracker(_full_trifecta())
    tracker._load_trifecta_weapon_profiles()

    p = tracker._damage_attributor._profiles["Big Cannon"]
    assert p.name == "Big Cannon"
    assert p.min_damage == 90.0
    assert p.max_damage == 130.0
    assert p.base_damage == 120.0
    assert p.cost_per_shot == 0.42
    assert p.role == "big"


def test_weapon_field_mapping_is_not_swapped():
    # Distinct values per slot so a min/max/base/cost reorder is caught.
    trifecta = {
        "small_weapon": {
            "name": "W",
            "damage_min": 3.0,
            "damage_max": 7.0,
            "total_damage": 5.0,
            "cost_per_shot_ped": 1.25,
            "role": "r",
            "weapon_props": {"k": "v"},
        }
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    p = tracker._damage_attributor._profiles["W"]
    assert (p.min_damage, p.max_damage, p.base_damage, p.cost_per_shot) == (
        3.0,
        7.0,
        5.0,
        1.25,
    )


def test_weapon_props_captured_when_present():
    tracker = _make_tracker(_full_trifecta())
    tracker._load_trifecta_weapon_profiles()
    assert tracker._trifecta_weapon_profiles == {
        "Small Blaster": {"amp": "104", "tier": 3},
        "Big Cannon": {"amp": "115", "tier": 7},
    }


def test_weapon_props_skipped_when_falsy():
    # weapon_props absent / empty -> no entry recorded for that weapon, but the
    # weapon is still added to the damage attributor.
    trifecta = {
        "small_weapon": {
            "name": "NoProps",
            "damage_min": 1.0,
            "damage_max": 2.0,
            "total_damage": 1.5,
            "cost_per_shot_ped": 0.1,
            "role": "small",
            # no weapon_props key at all
        },
        "big_weapon": {
            "name": "EmptyProps",
            "damage_min": 5.0,
            "damage_max": 6.0,
            "total_damage": 5.5,
            "cost_per_shot_ped": 0.2,
            "role": "big",
            "weapon_props": {},  # falsy -> skipped
        },
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()

    assert set(tracker._damage_attributor._profiles) == {"NoProps", "EmptyProps"}
    assert tracker._trifecta_weapon_profiles == {}


def test_missing_weapon_slot_is_skipped_not_errored():
    # Only the big weapon present; the small slot is absent (the `not weapon`
    # continue must fire without touching the damage attributor for it).
    trifecta = {
        "big_weapon": {
            "name": "OnlyBig",
            "damage_min": 50.0,
            "damage_max": 70.0,
            "total_damage": 65.0,
            "cost_per_shot_ped": 0.3,
            "role": "big",
            "weapon_props": {"a": 1},
        }
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert set(tracker._damage_attributor._profiles) == {"OnlyBig"}
    assert tracker._trifecta_weapon_profiles == {"OnlyBig": {"a": 1}}


def test_falsy_weapon_value_is_skipped():
    # Slot present but value falsy (None) -> skipped.
    trifecta = {
        "small_weapon": None,
        "big_weapon": {
            "name": "B",
            "damage_min": 1.0,
            "damage_max": 2.0,
            "total_damage": 1.5,
            "cost_per_shot_ped": 0.1,
            "role": "big",
            "weapon_props": {"z": 9},
        },
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert set(tracker._damage_attributor._profiles) == {"B"}


def test_role_defaults_to_none_when_absent():
    trifecta = {
        "small_weapon": {
            "name": "NoRole",
            "damage_min": 1.0,
            "damage_max": 2.0,
            "total_damage": 1.5,
            "cost_per_shot_ped": 0.1,
            # no "role"
            "weapon_props": {"k": 1},
        }
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._damage_attributor._profiles["NoRole"].role is None


# ---------------------------------------------------------------------------
# Heal-tool block: present heal tool sets every field; falsy heal tool leaves
# the reset defaults in place.
# ---------------------------------------------------------------------------


def test_heal_tool_fields_all_set_exactly():
    tracker = _make_tracker(_full_trifecta())
    tracker._load_trifecta_weapon_profiles()
    assert tracker._active_heal_tool_name == "Healer Mk.III"
    assert tracker._heal_cost_per_use_ped == 0.33
    assert tracker._heal_reload_seconds == 1.75
    assert tracker._heal_amount_min == 40.0
    assert tracker._heal_amount_max == 60.0


def test_heal_tool_fields_not_swapped():
    # min/max distinct and reload/cost distinct so a reorder is caught.
    trifecta = {
        "heal_tool": {
            "name": "HT",
            "cost_per_use_ped": 2.0,
            "reload_seconds": 3.0,
            "heal_min": 4.0,
            "heal_max": 5.0,
        }
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._heal_cost_per_use_ped == 2.0
    assert tracker._heal_reload_seconds == 3.0
    assert tracker._heal_amount_min == 4.0
    assert tracker._heal_amount_max == 5.0


def test_heal_amount_defaults_to_none_when_absent():
    trifecta = {
        "heal_tool": {
            "name": "HT2",
            "cost_per_use_ped": 1.0,
            "reload_seconds": 2.0,
            # no heal_min / heal_max
        }
    }
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._active_heal_tool_name == "HT2"
    assert tracker._heal_amount_min is None
    assert tracker._heal_amount_max is None


def test_no_heal_tool_keeps_reset_defaults():
    # Trifecta with a weapon but no heal tool -> heal stays at reset defaults.
    trifecta = {
        "small_weapon": {
            "name": "W",
            "damage_min": 1.0,
            "damage_max": 2.0,
            "total_damage": 1.5,
            "cost_per_shot_ped": 0.1,
            "role": "small",
            "weapon_props": {"k": 1},
        }
    }
    tracker = _make_tracker(trifecta)
    # Pre-dirty heal fields to ensure the block being skipped leaves defaults.
    tracker._active_heal_tool_name = "stale"
    tracker._load_trifecta_weapon_profiles()
    assert tracker._active_heal_tool_name is None
    assert tracker._heal_cost_per_use_ped == 0.0
    assert tracker._heal_reload_seconds == 2.5
    assert tracker._heal_amount_min is None
    assert tracker._heal_amount_max is None


def test_falsy_heal_tool_value_keeps_defaults():
    trifecta = {"heal_tool": None}
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._active_heal_tool_name is None
    assert tracker._heal_reload_seconds == 2.5


def test_empty_dict_heal_tool_value_keeps_defaults():
    trifecta = {"heal_tool": {}}
    tracker = _make_tracker(trifecta)
    tracker._load_trifecta_weapon_profiles()
    assert tracker._active_heal_tool_name is None
