"""Mutation-hardening tests for HuntTracker._on_enhancer_break (cluster tracker__c10).

The break handler reads the event payload, filters to the *active* weapon's
damage-enhancer state, and depletes a slot. The only behaviour observable
through a public surface is the resulting per-shot cost of the active weapon
(``kill.tool_stats[...].cost_per_shot``, persisted to the DB): a depleted
enhancer slot lowers the cost of the *next* shot. We also assert the
"slot depleted" INFO log via ``caplog``.

Each test exercises one decision in the handler so a mutation that flips it is
caught either by a wrong cost on the post-break shot or by the log fingerprint.
"""

import logging
import sqlite3

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_LOOT_GROUP,
)
from backend.tracking.tracker import HuntTracker, _DamageEnhancerState


def _enhancer_props(slots: int) -> dict:
    """A weapon props payload carrying ``slots`` damage enhancers."""
    return {
        "weapon_entity": {
            "damage": {"impact": 10.0},
            "economy": {"decay": 1.0, "ammo_burn": 50},
        },
        "weapon_markup": 100,
        "damage_enhancers": slots,
    }


def _make_tracker(slots: int = 2):
    """A tracker whose only profiled weapon ``MyGun`` carries ``slots`` enhancers."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    profile = _enhancer_props(slots)
    tracker = HuntTracker(
        bus,
        db,
        equipment_profile_lookup=lambda name: profile if name == "MyGun" else None,
        equipment_cost_lookup=lambda _name: 0.5,
    )
    return bus, tracker, db


def _arm(bus, tracker):
    """Equip MyGun (arms the enhancer state) and start a session."""
    tracker.start_session()
    bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "MyGun"})


def _shot(bus):
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": None}
    )


def _break(bus, **overrides):
    payload = {
        "enhancer_name": "Weapon Damage Enhancer",
        "item_name": "MyGun",
        "remaining": 1,
        "shrapnel_ped": 0.5,
    }
    payload.update(overrides)
    bus.publish(EVENT_ENHANCER_BREAK, payload)


def _finalize_and_cost(bus, tracker):
    """Finalise the in-progress kill via loot and return MyGun's cost_per_shot."""
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.5}],
            "total_ped": 0.5,
            "timestamp": None,
        },
    )
    result = tracker.stop_session()
    assert result is not None
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert "MyGun" in kill.tool_stats
    return kill.tool_stats["MyGun"].cost_per_shot


def _active_slots(tracker):
    """Active-slot count of the (single) armed weapon state."""
    states = list(tracker._weapon_enhancer_states.values())
    assert len(states) == 1
    return states[0].active_slots


# --------------------------------------------------------------------------
# Reference costs by active_slots (anchors the cost-based kills below).
# --------------------------------------------------------------------------


def _cost_for_active(slots_total: int, active: int) -> float:
    s = _DamageEnhancerState.from_props("MyGun", _enhancer_props(slots_total))
    s.set_total({2: 200, 1: 1, 0: 0}[active])
    return s.current_cost_ped()


COST_2 = _cost_for_active(2, 2)
COST_1 = _cost_for_active(2, 1)
COST_0 = _cost_for_active(2, 0)


def test_reference_costs_are_distinct():
    # Pre-condition for every cost-based kill: the three slot counts that the
    # break can land on give three distinct per-shot costs.
    assert COST_2 > COST_1 > COST_0
    assert abs(COST_2 - COST_1) > 1e-6
    assert abs(COST_1 - COST_0) > 1e-6


# --------------------------------------------------------------------------
# mutmut_1: `if not self._accumulator` -> `if self._accumulator`
# --------------------------------------------------------------------------


def test_break_with_remaining_depletes_to_one_slot():
    """A break carrying remaining=1 on a 2-slot weapon redistributes to one
    active slot; the next shot is billed at the one-slot cost.

    Kills mutmut_1 (guard inversion would skip the break entirely, leaving the
    two-slot cost), and the core depletion path."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, remaining=1)
    assert _active_slots(tracker) == 1
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_1) < 1e-9
    assert abs(cost - COST_2) > 1e-9


def test_break_with_no_session_is_silent_noop():
    """With no active session, current_accumulator is None and a break is a
    silent no-op (no raise, no kill row). The killing direction of mutmut_1
    (guard inversion) is the session-active path, covered by
    test_break_with_remaining_depletes_to_one_slot: with the guard inverted a
    live session would skip the break, leaving the two-slot cost."""
    bus, tracker, db = _make_tracker(2)
    assert tracker.current_accumulator is None
    _break(bus, remaining=1)  # must not raise
    assert tracker.current_accumulator is None


# --------------------------------------------------------------------------
# mutmut_10..17: enhancer_name = data.get("enhancer_name", "")
# enhancer_name feeds  `"damage" not in enhancer_name.lower()`.
# --------------------------------------------------------------------------


def test_non_damage_enhancer_name_does_not_deplete():
    """An enhancer whose name lacks 'damage' must be ignored (the guard
    `"damage" not in enhancer_name.lower()` returns early). Cost stays at the
    two-slot value."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, enhancer_name="Weapon Accuracy Enhancer", remaining=1)
    assert _active_slots(tracker) == 2
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_2) < 1e-9


def test_damage_enhancer_name_case_insensitive_depletes():
    """A name that only matches 'damage' case-insensitively still depletes.
    Kills mutants that change the literal to 'DAMAGE'/'XXdamageXX' or switch
    .lower()->.upper(), and the None/empty-default reads of enhancer_name."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, enhancer_name="Weapon DAMAGE Enhancer", remaining=1)
    assert _active_slots(tracker) == 1
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_1) < 1e-9


def test_missing_enhancer_name_uses_empty_default_and_is_ignored():
    """When the payload omits enhancer_name the default "" is used; "" has no
    'damage', so the break is ignored. mutmut_10 (enhancer_name=None) would
    raise on None.lower(); mutmut_12/14 (default None) likewise; mutmut_13
    (get("") wrong key) yields "" -> ignored too but with no break."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    # Omit enhancer_name entirely.
    bus.publish(
        EVENT_ENHANCER_BREAK,
        {"item_name": "MyGun", "remaining": 1, "shrapnel_ped": 0.5},
    )
    assert _active_slots(tracker) == 2  # unchanged: "" has no 'damage'
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_2) < 1e-9


# --------------------------------------------------------------------------
# mutmut_18..25: item_name = data.get("item_name", "")
# item_name feeds _break_matches_active_weapon(item_name).
# --------------------------------------------------------------------------


def test_break_for_other_item_does_not_deplete_active_weapon():
    """A break whose item_name does not match the active weapon is ignored
    (`not self._break_matches_active_weapon(item_name)`)."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, item_name="Some Unrelated Pistol", remaining=1)
    assert _active_slots(tracker) == 2
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_2) < 1e-9


def test_break_matching_item_name_depletes():
    """A break whose item_name matches the active weapon depletes a slot.
    Kills the wrong-key / None / uppercase item_name reads, which would make
    the match fail (mutants 18-25) and skip the break."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, item_name="MyGun", remaining=1)
    assert _active_slots(tracker) == 1
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_1) < 1e-9


def test_missing_item_name_uses_empty_default_and_is_ignored():
    """Omitting item_name -> default "" -> _break_matches_active_weapon("")
    returns False -> ignored. mutmut_18 (item_name=None) makes the match
    raise/behave differently."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    bus.publish(
        EVENT_ENHANCER_BREAK,
        {
            "enhancer_name": "Weapon Damage Enhancer",
            "remaining": 1,
            "shrapnel_ped": 0.5,
        },
    )
    assert _active_slots(tracker) == 2
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_2) < 1e-9


# --------------------------------------------------------------------------
# mutmut_26..29, 53, 54: remaining = data.get("remaining"); apply_break(...)
# --------------------------------------------------------------------------


def test_integer_remaining_redistributes_total_not_single_decrement():
    """remaining=1 on a 2-slot weapon redistributes total to one active slot
    (active_slots 2 -> 1). If `remaining` is lost (mutants 26-29: None / wrong
    key) or apply_break is passed None (mutmut_54), the code takes the single
    decrement path which keeps active_slots at 2."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, remaining=1)
    assert _active_slots(tracker) == 1
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_1) < 1e-9
    assert abs(cost - COST_2) > 1e-9


def test_remaining_zero_redistributes_to_zero_slots():
    """remaining=0 redistributes to zero active slots; the next shot is billed
    at the no-enhancer cost. Distinguishes the redistribute path (-> 0 slots)
    from the single-decrement path (-> 1 slot)."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, remaining=0)
    assert _active_slots(tracker) == 0
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_0) < 1e-9


def test_non_integer_remaining_falls_back_to_single_decrement():
    """A non-int remaining (e.g. None) takes the single-decrement branch:
    active_slots stays 2 (a full slot at 100 only drops to 99). This pins the
    `isinstance(remaining, int)` guard and apply_break wiring (mutmut_53/54)."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, remaining=None)
    # Decrement path: one stack 100 -> 99, still active. No slot lost.
    assert _active_slots(tracker) == 2
    _shot(bus)
    cost = _finalize_and_cost(bus, tracker)
    assert abs(cost - COST_2) < 1e-9


def test_apply_break_actually_called_changes_state():
    """mutmut_53 sets slot_changed = None and never calls state.apply_break,
    so the enhancer state would be untouched. A remaining=1 break MUST change
    active_slots from 2 to 1."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    before = _active_slots(tracker)
    assert before == 2
    _break(bus, remaining=1)
    after = _active_slots(tracker)
    assert after == 1
    assert before != after


# --------------------------------------------------------------------------
# mutmut_41..52: state resolution and the guard boolean structure.
# --------------------------------------------------------------------------


def test_break_with_no_active_weapon_is_noop():
    """With no active weapon resolved (no profile match), state is None and the
    break returns early. mutmut_41 (state=None) and mutmut_45 (is None -> is not
    None) hinge on this."""
    bus, tracker, db = _make_tracker(2)
    tracker.start_session()
    # Equip an unprofiled tool: _ensure_weapon_state clears the active key.
    bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "NoProfileGun"})
    assert tracker._active_weapon_state() is None
    _break(bus, item_name="NoProfileGun", remaining=1)
    # No enhancer state was armed for an unprofiled weapon.
    assert tracker._weapon_enhancer_states == {}


def test_active_weapon_present_break_applies():
    """The positive path: an armed, matching, damage break depletes. Anchors
    mutmut_45 (is None -> is not None would skip when state IS present) and the
    or/and regroupings (42-44) which all must still permit this clean break."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    assert tracker._active_weapon_state() is not None
    _break(bus, remaining=1)
    assert _active_slots(tracker) == 1


def test_zero_enhancer_weapon_break_is_noop_not_crash():
    """A weapon with zero configured enhancers has empty stacks; the guard
    `not state.stacks` returns early. mutmut_46 (`not state.stacks` ->
    `state.stacks`) would proceed into apply_break on an empty stack list and
    must be caught: active_slots stays 0 and cost stays the no-enhancer cost."""
    bus, tracker, db = _make_tracker(0)
    _arm(bus, tracker)
    state = tracker._active_weapon_state()
    assert state is not None
    assert state.stacks == []
    _break(bus, remaining=1)
    assert state.active_slots == 0


# --------------------------------------------------------------------------
# mutmut_42/43/44: or -> and regroupings of the early-return guard.
# Each regrouping changes which payloads are filtered. We probe the guard with
# inputs that the real (all-`or`) guard rejects but a regrouping would accept,
# or vice versa, observed via whether the break depletes.
# --------------------------------------------------------------------------


def test_guard_rejects_when_only_name_fails_but_others_pass():
    """Real guard: state present, stacks present, item matches, but the
    enhancer name lacks 'damage' -> the `"damage" not in ...` term is True ->
    early return. mutmut_42 turns the last two terms into an `and`, so a
    non-damage name with a matching item would no longer short-circuit and
    the break would wrongly apply. Active slots must stay 2."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(bus, enhancer_name="Accuracy Enhancer", item_name="MyGun", remaining=1)
    assert _active_slots(tracker) == 2


def test_guard_rejects_when_only_item_match_fails():
    """Real guard: damage name present but item does not match the active
    weapon -> `not _break_matches_active_weapon` is True -> early return.
    Probes mutmut_42 (name/match `and`) and mutmut_51/52 from the other side."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    _break(
        bus,
        enhancer_name="Weapon Damage Enhancer",
        item_name="DifferentWeapon",
        remaining=1,
    )
    assert _active_slots(tracker) == 2


# --------------------------------------------------------------------------
# mutmut_55..63: the "slot depleted" INFO log (fires only when slot_changed).
# Verified via caplog: message present + correctly formatted with tool name.
# --------------------------------------------------------------------------


def test_slot_depleted_logs_info_with_tool_name_and_count(caplog):
    """When a break depletes a slot, an INFO line names the weapon and the
    remaining active-slot count. Kills the log message/arg mutants (55-63)
    by asserting the rendered message contents, which forces %-formatting."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    with caplog.at_level(logging.INFO, logger="backend.tracking.tracker"):
        _break(bus, remaining=1)
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.INFO]
    depleted = [m for m in msgs if "slot depleted" in m.lower()]
    assert depleted, f"expected a slot-depleted INFO log, got: {msgs}"
    line = depleted[0]
    assert "MyGun" in line  # state.tool_name arg (mutmut_56/59)
    assert "1 active slot" in line  # state.active_slots arg (mutmut_57/60)
    # Exact prefix pins the literal (mutmut_61/62/63: XX.., lower, UPPER).
    assert line.startswith("Damage enhancer slot depleted on MyGun:")


def test_no_slot_depleted_log_when_slot_not_lost(caplog):
    """A single-decrement break that does NOT cross a slot to zero must not
    emit the depleted log (slot_changed is False)."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    with caplog.at_level(logging.INFO, logger="backend.tracking.tracker"):
        _break(bus, remaining=None)  # decrement 100 -> 99, no slot lost
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.INFO]
    assert not [m for m in msgs if "slot depleted" in m.lower()]


# --------------------------------------------------------------------------
# mutmut_30..40: the log.debug(...) at the top of the handler.
# Captured at DEBUG level so %-formatting executes and the message renders.
# --------------------------------------------------------------------------


def test_debug_log_renders_with_all_fields(caplog):
    """The DEBUG trace embeds enhancer_name, shrapnel_ped (%.2f) and remaining.
    Capturing at DEBUG forces the lazy %-format; the arg/message mutants
    (30-40) change the literal or drop/null an arg, altering the rendered text
    or raising during format (logging records the failure, dropping the line)."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        _break(
            bus, enhancer_name="Weapon Damage Enhancer", shrapnel_ped=0.5, remaining=1
        )
    debug_msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.DEBUG]
    rendered = [m for m in debug_msgs if m.startswith("Enhancer break:")]
    assert rendered, f"expected the enhancer-break DEBUG line, got: {debug_msgs}"
    line = rendered[0]
    assert "Weapon Damage Enhancer" in line  # enhancer_name arg (mutmut_31/35)
    assert "shrapnel=0.50" in line  # shrapnel_ped %.2f (mutmut_32/36)
    assert "remaining=1" in line  # remaining arg (mutmut_33/37)


# --------------------------------------------------------------------------
# Direct-handler tests. The event bus swallows handler exceptions, so to catch
# mutations that raise (a None default fed to .lower() or %.2f), and to pin the
# *default value* of payload .get() reads, we invoke _on_enhancer_break
# directly. These bypass only the bus's blanket try/except; the handler logic
# under test is identical.
# --------------------------------------------------------------------------


def _debug_break_message(tracker, caplog, payload):
    """Drive _on_enhancer_break directly at DEBUG and return its rendered line."""
    with caplog.at_level(logging.DEBUG, logger="backend.tracking.tracker"):
        tracker._on_enhancer_break(payload)
    for r in caplog.records:
        if r.levelno == logging.DEBUG and str(r.msg).startswith("Enhancer break:"):
            return r.getMessage()  # forces %-format; raises on a bad arg type
    raise AssertionError("no enhancer-break DEBUG record emitted")


def test_missing_shrapnel_defaults_to_zero_in_debug_log(caplog):
    """shrapnel_ped's default is 0.0: a payload without the key renders
    'shrapnel=0.00'. mutmut_9 (default 1.0) renders 'shrapnel=1.00'; mutmut_4/6
    (default None) raise TypeError during %.2f formatting when the record is
    rendered."""
    bus, tracker, db = _make_tracker(2)
    tracker.start_session()
    line = _debug_break_message(
        tracker,
        caplog,
        {
            "enhancer_name": "Weapon Damage Enhancer",
            "item_name": "MyGun",
            "remaining": 1,
        },  # shrapnel_ped omitted -> default used
    )
    assert "shrapnel=0.00" in line
    assert "shrapnel=1.00" not in line


def test_present_shrapnel_value_is_used_in_debug_log(caplog):
    """A present shrapnel_ped value is rendered verbatim (the default is not
    substituted). Anchors the key/default reads (mutmut_3/5/7/8) from the
    value-present side."""
    bus, tracker, db = _make_tracker(2)
    tracker.start_session()
    line = _debug_break_message(
        tracker,
        caplog,
        {
            "enhancer_name": "Weapon Damage Enhancer",
            "item_name": "MyGun",
            "remaining": 1,
            "shrapnel_ped": 3.25,
        },
    )
    assert "shrapnel=3.25" in line


def test_missing_enhancer_name_default_is_empty_string_not_none():
    """enhancer_name's default is "" (a string), so a payload without the key
    reaches `"damage" not in "".lower()` safely and returns. mutmut_12/14
    (default None) hit `None.lower()` and raise AttributeError. Direct call so
    the raise is not swallowed by the bus."""
    bus, tracker, db = _make_tracker(2)
    _arm(bus, tracker)  # armed + matching item so the .lower() term is reached
    state = tracker._active_weapon_state()
    assert state is not None and state.stacks
    # No enhancer_name key. Real code: "".lower() -> ignored, no raise.
    tracker._on_enhancer_break({"item_name": "MyGun", "remaining": 1})
    # Ignored (the empty name has no 'damage'): slots unchanged, no exception.
    assert state.active_slots == 2


def test_missing_enhancer_name_empty_default_renders_in_debug(caplog):
    """The "" default renders as an empty %s in the debug trace, NOT 'XXXX'.
    Kills mutmut_17 (default 'XXXX')."""
    bus, tracker, db = _make_tracker(2)
    tracker.start_session()
    line = _debug_break_message(
        tracker,
        caplog,
        {"item_name": "MyGun", "remaining": 1, "shrapnel_ped": 0.5},
    )
    assert "XXXX" not in line
    # Empty enhancer name renders as nothing between the colon-space and the
    # opening parenthesis.
    assert line.startswith("Enhancer break:  (")


def test_state_none_guard_short_circuits_without_touching_stacks():
    """When the active weapon state is None the guard's first term returns the
    handler early WITHOUT evaluating `state.stacks`. mutmut_44 regroups
    `state is None or not state.stacks` into `... and ...`, which forces
    `not state.stacks` to be evaluated even when state is None, raising
    AttributeError. Direct call so the raise propagates."""
    bus, tracker, db = _make_tracker(2)
    tracker.start_session()
    # No active weapon equipped -> _active_weapon_state() is None.
    assert tracker._active_weapon_state() is None
    # Real code returns early cleanly; mutmut_44 raises on None.stacks.
    tracker._on_enhancer_break(
        {
            "enhancer_name": "Weapon Damage Enhancer",
            "item_name": "MyGun",
            "remaining": 1,
            "shrapnel_ped": 0.5,
        }
    )  # must not raise
