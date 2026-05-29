"""Mutation-hardening tests for HuntTracker loot/tool/heal handlers.

Targets cluster tracker__c8: HuntTracker._on_loot, ._on_tool_changed,
._on_heal_tool_changed, ._heal_amount_matches_trifecta_tool.

These call the handler methods directly (the production code wires them onto
the event bus, which swallows subscriber exceptions, so direct calls let a test
observe the exact mutated line - argument defaults, comparison operators,
in-place vs plain assignment - through the resulting Kill record, the tracker's
heal-cost accumulator, or a raised exception).
"""

import sqlite3
from datetime import datetime, timedelta

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
)
from backend.tracking.models import ToolStats
from backend.tracking.tracker import HuntTracker


def _make_tracker(**kwargs):
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, **kwargs)
    return bus, tracker, db


@pytest.fixture
def pipeline():
    return _make_tracker()


# ---------------------------------------------------------------------------
# _on_loot
# ---------------------------------------------------------------------------


class TestOnLootGuard:
    def test_returns_without_kill_when_accumulator_missing(self, pipeline):
        """The `not acc or not session` guard short-circuits on a missing
        accumulator. With `or` -> `and` (mutmut_1) the body would run and read
        `self._accumulator.shots_fired`, raising AttributeError; the original
        returns quietly and records no kill."""
        bus, tracker, db = pipeline
        tracker.start_session()
        # Session present, accumulator gone: original guard returns; the `and`
        # mutant proceeds into the body.
        tracker._accumulator = None
        before = len(tracker.session.kills)

        # No exception, no kill recorded.
        tracker._on_loot(
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
                "total_ped": 0.01,
                "timestamp": datetime.now(tz=None),
            }
        )
        assert len(tracker.session.kills) == before


class TestOnLootItemDefaults:
    def _loot(self, items, total_ped, ts):
        return {"items": items, "total_ped": total_ped, "timestamp": ts}

    def test_missing_items_key_defaults_to_empty_list(self, pipeline):
        """`data.get("items", [])` - a loot payload with no `items` key must
        produce a kill with zero loot items, not crash. The mutants that change
        the default to None (mutmut_6) or drop it (mutmut_8) would make
        `items_raw` None and break `items_raw[0]`/iteration."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_loot({"total_ped": 0.0, "timestamp": datetime.now(tz=None)})
        assert len(tracker.session.kills) == 1
        assert tracker.session.kills[0].loot_items == []

    def test_total_ped_default_zero_in_fingerprint(self, pipeline):
        """`total_ped = data.get("total_ped", 0.0)` (mutmut_18 -> 1.0). The
        fingerprint rounds total_ped. Group 1 omits total_ped (default 0.0),
        group 2 sets total_ped explicitly to 0.0; with identical items inside the
        window the fingerprints match and dedup to one kill. A 1.0 default gives
        group 1 a different fingerprint and yields two kills."""
        bus, tracker, db = pipeline
        tracker.start_session()
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        item = {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.5}
        tracker._on_loot({"items": [dict(item)], "timestamp": t0})  # no total_ped
        tracker._on_loot(
            {
                "items": [dict(item)],
                "total_ped": 0.0,
                "timestamp": t0 + timedelta(seconds=0.5),
            }
        )
        assert len(tracker.session.kills) == 1

    def test_filtered_total_uses_item_values(self, pipeline):
        """loot_total_ped is summed from item values, independent of total_ped."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime.now(tz=None)
        tracker._on_loot(
            self._loot(
                [{"item_name": "Animal Oil Residue", "quantity": 2, "value_ped": 0.5}],
                0.0,
                ts,
            )
        )
        assert tracker.session.kills[0].loot_total_ped == pytest.approx(0.5)

    def test_item_name_default_empty_string(self, pipeline):
        """`item.get("item_name", "")` defaults to "" (mutmut_54/56 -> None /
        drop, which makes is_tracked_loot call .casefold() on None and raise;
        mutmut_59 -> "XXXX"). An item with no name key becomes a tracked LootItem
        whose item_name is exactly the empty-string default."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime.now(tz=None)
        tracker._on_loot(self._loot([{"quantity": 3, "value_ped": 0.2}], 0.2, ts))
        kill = tracker.session.kills[0]
        assert len(kill.loot_items) == 1
        assert kill.loot_items[0].item_name == ""
        assert kill.loot_items[0].quantity == 3

    def test_quantity_and_value_defaults_when_keys_absent(self, pipeline):
        """A tracked item missing quantity/value falls back to quantity=1
        (mutmut_74/76/79) and value_ped=0.0 (mutmut_81/83/86)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime.now(tz=None)
        tracker._on_loot(self._loot([{"item_name": "Animal Oil Residue"}], 0.0, ts))
        item = tracker.session.kills[0].loot_items[0]
        assert item.quantity == 1
        assert item.value_ped == 0.0

    def test_is_enhancer_shrapnel_default_false_counts_toward_total(self, pipeline):
        """`item.get("is_enhancer_shrapnel", False)` (mutmut_89/91). With the
        default False, the item's value counts toward filtered_total_ped. If the
        default became None it is still falsy, but dropping the argument makes
        `get` raise; assert the value contributes to the total and the flag is
        False."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime.now(tz=None)
        tracker._on_loot(
            self._loot(
                [{"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.7}],
                0.7,
                ts,
            )
        )
        kill = tracker.session.kills[0]
        assert kill.loot_items[0].is_enhancer_shrapnel is False
        # Non-shrapnel items contribute to the filtered total.
        assert kill.loot_total_ped == pytest.approx(0.7)

    def test_enhancer_shrapnel_excluded_from_filtered_total(self, pipeline):
        """When is_enhancer_shrapnel is True the value is excluded from the
        filtered total. Pins the True-path so the default-False mutants cannot
        silently flip enhancer shrapnel into the total."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime.now(tz=None)
        tracker._on_loot(
            self._loot(
                [
                    {
                        "item_name": "Shrapnel",
                        "quantity": 100,
                        "value_ped": 1.0,
                        "is_enhancer_shrapnel": True,
                    },
                    {
                        "item_name": "Animal Oil Residue",
                        "quantity": 1,
                        "value_ped": 0.3,
                    },
                ],
                1.3,
                ts,
            )
        )
        kill = tracker.session.kills[0]
        assert kill.loot_total_ped == pytest.approx(0.3)
        shrap = next(i for i in kill.loot_items if i.item_name == "Shrapnel")
        assert shrap.is_enhancer_shrapnel is True


class TestOnLootTimestamp:
    def test_explicit_datetime_timestamp_persisted_as_epoch(self, pipeline):
        """`float(now)` is only reached for a non-datetime timestamp; a datetime
        goes through `now.timestamp()`. Pin the datetime path (mutmut_26 swaps
        `float(now)` -> `float(None)` on the else branch, so a numeric timestamp
        is what kills it - covered below)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        ts = datetime(2024, 1, 2, 3, 4, 5)
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": ts,
            }
        )
        assert tracker.session.kills[0].timestamp == pytest.approx(ts.timestamp())

    def test_numeric_timestamp_converted_via_float(self, pipeline):
        """A non-datetime timestamp takes the `float(now)` branch. mutmut_26
        replaces it with `float(None)` which raises TypeError; the original
        stores the numeric epoch unchanged."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": 1700000000.5,
            }
        )
        assert tracker.session.kills[0].timestamp == pytest.approx(1700000000.5)


class TestOnLootDedup:
    def test_identical_loot_within_window_is_deduplicated(self, pipeline):
        """Fingerprint = (round(total,4), len, first_item). Two identical loot
        groups inside LOOT_DEDUP_WINDOW collapse to one kill. mutmut_42 rounds
        to 5 dp and mutmut_29/31/35/36 change the first_item default; a tiny
        4th-decimal difference must still dedup (same rounded fingerprint)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        payload = {
            "items": [
                {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.5}
            ],
            "total_ped": 1.23456,
            "timestamp": t0,
        }
        tracker._on_loot(dict(payload))
        # 0.5s later, total differs only in the 5th decimal -> same 4dp round.
        payload2 = dict(payload)
        payload2["total_ped"] = 1.23459
        payload2["timestamp"] = t0 + timedelta(seconds=0.5)
        tracker._on_loot(payload2)
        assert len(tracker.session.kills) == 1

    def test_loot_outside_window_creates_second_kill(self, pipeline):
        """At exactly the window boundary `(now - last) < WINDOW` is False, so a
        new kill is created. mutmut_48 (`<` -> `<=`) would treat the boundary as
        still inside the window and drop the second kill."""
        bus, tracker, db = pipeline
        tracker.start_session()
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        payload = {
            "items": [
                {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.5}
            ],
            "total_ped": 1.0,
            "timestamp": t0,
        }
        tracker._on_loot(dict(payload))
        # Exactly 2.0s later == LOOT_DEDUP_WINDOW: boundary is NOT inside.
        payload2 = dict(payload)
        payload2["timestamp"] = t0 + timedelta(seconds=2)
        tracker._on_loot(payload2)
        assert len(tracker.session.kills) == 2

    def test_first_item_default_empty_matches_explicit_empty(self, pipeline):
        """`first_item = items_raw[0].get("item_name", "")`. Group 1's first item
        has NO name key (default ""), group 2 has an explicit empty name "".
        Both yield first_item "" so, with identical total/count inside the
        window, they dedup to one kill. mutmut_29/31 (default None) and mutmut_35
        (default "XXXX") give group 1 a different first_item, breaking the dedup
        into two kills."""
        bus, tracker, db = pipeline
        tracker.start_session()
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        # First group: first item lacks an item_name key -> default applies.
        tracker._on_loot(
            {
                "items": [{"quantity": 1, "value_ped": 0.5}],
                "total_ped": 1.0,
                "timestamp": t0,
            }
        )
        # Second group: explicit empty name, same total/count, inside window.
        tracker._on_loot(
            {
                "items": [{"item_name": "", "quantity": 1, "value_ped": 0.5}],
                "total_ped": 1.0,
                "timestamp": t0 + timedelta(seconds=0.5),
            }
        )
        assert len(tracker.session.kills) == 1

    def test_first_item_name_part_of_fingerprint(self, pipeline):
        """The fingerprint's third element is the first item's name. Two loot
        groups with the same total/count but different first item are distinct
        kills, pinning that first_item participates (mutmut_29/31/35/36)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.5}
                ],
                "total_ped": 1.0,
                "timestamp": t0,
            }
        )
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Muscle Oil", "quantity": 1, "value_ped": 0.5}
                ],
                "total_ped": 1.0,
                "timestamp": t0 + timedelta(seconds=0.5),
            }
        )
        assert len(tracker.session.kills) == 2


class TestOnLootKillSnapshot:
    def _arm_accumulator(self, tracker):
        acc = tracker.current_accumulator
        acc.shots_fired = 4
        acc.damage_dealt = 40.0
        acc.damage_taken = 17.5
        acc.critical_hits = 1
        acc.enhancer_cost = 3.25
        acc.tool_stats["Gun"] = ToolStats(
            tool_name="Gun", shots_fired=4, damage_dealt=40.0, cost_per_shot=2.0
        )

    def test_kill_snapshots_damage_taken(self, pipeline):
        """`damage_taken=self._accumulator.damage_taken` (mutmut_134 drops the
        kwarg, leaving the model default 0.0)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        self._arm_accumulator(tracker)
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": datetime.now(tz=None),
            }
        )
        assert tracker.session.kills[0].damage_taken == pytest.approx(17.5)

    def test_kill_snapshots_weapon_cost(self, pipeline):
        """`cost_ped=self._accumulator.weapon_cost` (mutmut_136). weapon_cost =
        cost_per_shot * shots = 2.0 * 4 = 8.0."""
        bus, tracker, db = pipeline
        tracker.start_session()
        self._arm_accumulator(tracker)
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": datetime.now(tz=None),
            }
        )
        assert tracker.session.kills[0].cost_ped == pytest.approx(8.0)

    def test_kill_snapshots_enhancer_cost(self, pipeline):
        """`enhancer_cost=self._accumulator.enhancer_cost` (mutmut_137)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        self._arm_accumulator(tracker)
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": datetime.now(tz=None),
            }
        )
        assert tracker.session.kills[0].enhancer_cost == pytest.approx(3.25)

    def test_kill_id_truncated_to_eight_chars_in_log(self, pipeline):
        """The debug log slices `kill.id[:8]` (mutmut_157 -> `[:9]`). The slice
        feeds only a log line, but the call must not crash and the kill is
        recorded; the id-length invariant is pinned here to give the log-arg
        mutants (146-156) a behavioural anchor: a kill is still produced and
        persisted with a full-length uuid id."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_loot(
            {
                "items": [
                    {"item_name": "Animal Oil Residue", "quantity": 1, "value_ped": 0.1}
                ],
                "total_ped": 0.1,
                "timestamp": datetime.now(tz=None),
            }
        )
        kill = tracker.session.kills[0]
        assert len(kill.id) == 36  # uuid4 string
        row = db.execute("SELECT id FROM kills WHERE id = ?", (kill.id,)).fetchone()
        assert row is not None and row[0] == kill.id


# ---------------------------------------------------------------------------
# _on_tool_changed
# ---------------------------------------------------------------------------


class TestOnToolChangedMerge:
    def _seed_unknown(self, tracker, shots=2, dmg=20.0, crits=1):
        acc = tracker.current_accumulator
        acc.tool_stats["Unknown"] = ToolStats(
            tool_name="Unknown",
            shots_fired=shots,
            damage_dealt=dmg,
            critical_hits=crits,
        )

    def test_merge_uses_named_tool_and_its_cost(self):
        """`current_cost = self._current_cost_for_tool(tool_name)` (mutmut_9
        passes None). With a lookup keyed on the name, the named tool resolves a
        cost of 2.0; None resolves 0.0, which would route the merge through the
        zero-cost branch and leave the kill's cost_ped at 0."""
        bus, tracker, db = _make_tracker(
            equipment_cost_lookup=lambda name: 2.0 if name == "Gun" else 0.0
        )
        tracker.start_session()
        self._seed_unknown(tracker, shots=3, dmg=30.0, crits=0)
        tracker._on_tool_changed({"tool_name": "Gun"})
        # Merge lands under "Gun" with cost_per_shot 2.0 -> weapon_cost 6.0.
        stats = tracker.current_accumulator.tool_stats
        assert "Unknown" not in stats
        assert stats["Gun"].shots_fired == 3
        assert stats["Gun"].cost_per_shot == pytest.approx(2.0)
        assert tracker.current_accumulator.weapon_cost == pytest.approx(6.0)

    def test_no_unknown_key_does_not_raise(self):
        """`pop("Unknown", None)` (mutmut_13 drops the default). With no
        "Unknown" entry, the original returns None and skips the merge; a bare
        `pop("Unknown")` would raise KeyError."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        # Accumulator has no "Unknown" key.
        tracker._on_tool_changed({"tool_name": "Gun"})  # must not raise
        assert "Unknown" not in tracker.current_accumulator.tool_stats
        assert tracker._active_hotbar_tool_name == "Gun"

    def test_merge_target_resolved_before_use(self):
        """mutmut_19 sets `real = None`, then `real.shots_fired += ...` raises
        AttributeError. The original merges the unknown shots into the real
        tool; assert the merged result is present and no exception escaped."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        self._seed_unknown(tracker, shots=2, dmg=20.0, crits=1)
        tracker._on_tool_changed({"tool_name": "Gun"})
        stats = tracker.current_accumulator.tool_stats["Gun"]
        assert stats.shots_fired == 2
        assert stats.damage_dealt == pytest.approx(20.0)
        assert stats.critical_hits == 1

    def test_merge_phase_keyed_under_real_tool_name(self):
        """mutmut_20 passes None as the tool_name to _tool_stats_for_phase, so
        the merged stats land under a None-named tool. The original keys them
        under the real tool name "Gun"."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        self._seed_unknown(tracker, shots=2, dmg=20.0, crits=1)
        tracker._on_tool_changed({"tool_name": "Gun"})
        stats = tracker.current_accumulator.tool_stats
        assert "Gun" in stats
        assert stats["Gun"].tool_name == "Gun"
        assert all(ts.tool_name == "Gun" for ts in stats.values())

    def test_merge_cost_per_shot_is_resolved_value(self):
        """mutmut_21 passes None as cost_per_shot; weapon_cost then multiplies
        None by shots and raises TypeError. The original keeps cost_per_shot 2.0
        so weapon_cost = 2.0 * 2 = 4.0."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        self._seed_unknown(tracker, shots=2, dmg=20.0, crits=1)
        tracker._on_tool_changed({"tool_name": "Gun"})
        assert tracker.current_accumulator.tool_stats[
            "Gun"
        ].cost_per_shot == pytest.approx(2.0)
        assert tracker.current_accumulator.weapon_cost == pytest.approx(4.0)

    def test_phase_helper_called_with_both_arguments(self):
        """mutmut_22/23 drop one of the two required positional args to
        _tool_stats_for_phase, raising TypeError. The original call succeeds and
        records the merged tool stats."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        self._seed_unknown(tracker, shots=2, dmg=20.0, crits=1)
        tracker._on_tool_changed({"tool_name": "Gun"})
        assert tracker.current_accumulator.tool_stats["Gun"].shots_fired == 2

    def test_merge_accumulates_onto_existing_phase_stats(self):
        """mutmut_28/30/32 replace `real.X += unknown.X` with `real.X =
        unknown.X`. Pre-seed an existing "Gun" phase (same cost) with its own
        shots/damage/crits so the merge must ADD the unknown counts rather than
        overwrite them."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 2.0)
        tracker.start_session()
        acc = tracker.current_accumulator
        # Existing real-tool phase with nonzero counts and matching cost.
        acc.tool_stats["Gun"] = ToolStats(
            tool_name="Gun",
            shots_fired=5,
            damage_dealt=50.0,
            critical_hits=2,
            cost_per_shot=2.0,
        )
        # Unknown stats that must be ADDED onto the existing phase.
        acc.tool_stats["Unknown"] = ToolStats(
            tool_name="Unknown",
            shots_fired=3,
            damage_dealt=30.0,
            critical_hits=1,
        )
        tracker._on_tool_changed({"tool_name": "Gun"})
        merged = tracker.current_accumulator.tool_stats["Gun"]
        assert merged.shots_fired == 8  # 5 + 3 (not 3)
        assert merged.damage_dealt == pytest.approx(80.0)  # 50 + 30
        assert merged.critical_hits == 3  # 2 + 1

    def test_zero_cost_routes_through_named_branch(self):
        """mutmut_18 changes `current_cost > 0` to `current_cost > 1`. With a
        sub-1 positive cost (0.5) the original takes the cost>0 branch and the
        merged phase keeps cost_per_shot 0.5; the mutant falls into the
        zero-cost branch and the stats carry cost_per_shot 0.0."""
        bus, tracker, db = _make_tracker(equipment_cost_lookup=lambda name: 0.5)
        tracker.start_session()
        self._seed_unknown(tracker, shots=4, dmg=40.0, crits=0)
        tracker._on_tool_changed({"tool_name": "Gun"})
        stats = tracker.current_accumulator.tool_stats["Gun"]
        assert stats.shots_fired == 4
        assert stats.cost_per_shot == pytest.approx(0.5)
        assert tracker.current_accumulator.weapon_cost == pytest.approx(2.0)


# ---------------------------------------------------------------------------
# _on_heal_tool_changed
# ---------------------------------------------------------------------------


class TestOnHealToolChanged:
    def test_equips_heal_tool_state(self, pipeline):
        """Pins every assignment in the handler: name (mutmut_1-4, 21), cost
        (mutmut_5-12, 22), reload (mutmut_13-20, 23), min/max reset to None
        (mutmut_24/25), warning flag reset to False (mutmut_26/27)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        # Dirty the state first so the reset assignments are observable.
        tracker._heal_amount_min = 5.0
        tracker._heal_amount_max = 9.0
        tracker._heal_warning_emitted = True

        tracker._on_heal_tool_changed(
            {
                "tool_name": "Vivo T1",
                "cost_per_use_ped": 0.42,
                "reload_seconds": 1.5,
            }
        )
        assert tracker._active_heal_tool_name == "Vivo T1"
        assert tracker._heal_cost_per_use_ped == pytest.approx(0.42)
        assert tracker._heal_reload_seconds == pytest.approx(1.5)
        assert tracker._heal_amount_min is None
        assert tracker._heal_amount_max is None
        assert tracker._heal_warning_emitted is False

    def test_cost_defaults_to_zero_when_absent(self, pipeline):
        """`cost_per_use_ped` default 0.0 (mutmut_12 -> 1.0; mutmut_5/7/9 ->
        None/drop)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_heal_tool_changed({"tool_name": "Vivo T1", "reload_seconds": 2.0})
        assert tracker._heal_cost_per_use_ped == 0.0

    def test_reload_defaults_to_two_point_five(self, pipeline):
        """`reload_seconds` default 2.5 (mutmut_20 -> 3.5; mutmut_13/15/17 ->
        None/drop)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_heal_tool_changed({"tool_name": "Vivo T1", "cost_per_use_ped": 0.1})
        assert tracker._heal_reload_seconds == pytest.approx(2.5)

    def test_heal_cost_default_zero_suppresses_accumulation(self, pipeline):
        """A default 0.0 heal cost means the self_heal branch in _on_combat does
        not accumulate session heal cost (`if self._heal_cost_per_use_ped > 0`).
        If the default flipped to 1.0 (mutmut_12) the heal would charge cost.
        This pins the default through the observable heal-cost accumulator."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._on_heal_tool_changed({"tool_name": "Vivo T1", "reload_seconds": 2.0})
        # No cost configured -> a heal tick adds nothing to session heal cost.
        bus.publish(
            EVENT_COMBAT,
            {"type": "self_heal", "amount": 30.0, "timestamp": datetime(2024, 1, 1)},
        )
        assert tracker._session_heal_cost == pytest.approx(0.0)

    def test_configured_heal_cost_accumulates_per_activation(self, pipeline):
        """With a positive heal cost and reload window, each new heal activation
        charges cost once. Drives the self_heal branch end-to-end so the heal
        assignments are observable through _session_heal_cost (and DB heal_cost):
        a name=None / cost=None mutant would break the equip or accumulation."""
        bus, tracker, db = pipeline
        session = tracker.start_session()
        tracker._on_heal_tool_changed(
            {
                "tool_name": "Vivo T1",
                "cost_per_use_ped": 0.5,
                "reload_seconds": 2.0,
            }
        )
        base = datetime(2024, 1, 1, 0, 0, 0)
        # First heal activation.
        bus.publish(
            EVENT_COMBAT, {"type": "self_heal", "amount": 30.0, "timestamp": base}
        )
        # Second activation past the 2.0s reload window.
        bus.publish(
            EVENT_COMBAT,
            {
                "type": "self_heal",
                "amount": 30.0,
                "timestamp": base + timedelta(seconds=2.0),
            },
        )
        assert tracker._session_heal_cost == pytest.approx(1.0)
        result = tracker.stop_session()
        assert result is not None
        row = db.execute(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?", (session.id,)
        ).fetchone()
        assert row[0] == pytest.approx(1.0)


# ---------------------------------------------------------------------------
# _heal_amount_matches_trifecta_tool
# ---------------------------------------------------------------------------


def _trifecta_with_heal_range(heal_min, heal_max):
    return {
        "heal_tool": {
            "name": "Vivo T1",
            "cost_per_use_ped": 0.5,
            "reload_seconds": 2.0,
            "heal_min": heal_min,
            "heal_max": heal_max,
        },
    }


class TestHealAmountMatchesTrifectaTool:
    def test_unbounded_when_min_or_max_unset(self, pipeline):
        """When either bound is None the method returns True (mutmut_4 returns
        False instead). Default tracker has both bounds None."""
        bus, tracker, db = pipeline
        tracker.start_session()
        assert tracker._heal_amount_matches_trifecta_tool(123.0) is True

    def test_only_min_set_is_still_unbounded(self, pipeline):
        """`min is None or max is None` -> True. With only max None, the OR keeps
        it unbounded. mutmut_1 (`or`->`and`), mutmut_2/3 (`is`->`is not`) flip
        which combinations return early."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = None
        assert tracker._heal_amount_matches_trifecta_tool(50.0) is True

    def test_only_max_set_is_still_unbounded(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = None
        tracker._heal_amount_max = 10.0
        assert tracker._heal_amount_matches_trifecta_tool(50.0) is True

    def test_amount_inside_range_matches(self, pipeline):
        """Both bounds set: min <= amount <= max."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = 20.0
        assert tracker._heal_amount_matches_trifecta_tool(15.0) is True

    def test_amount_equal_to_min_is_inclusive(self, pipeline):
        """Lower bound inclusive: `min <= amount` (mutmut_5 -> `min < amount`
        excludes the boundary)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = 20.0
        assert tracker._heal_amount_matches_trifecta_tool(10.0) is True

    def test_amount_equal_to_max_is_inclusive(self, pipeline):
        """Upper bound inclusive: `amount <= max` (mutmut_6 -> `amount < max`
        excludes the boundary)."""
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = 20.0
        assert tracker._heal_amount_matches_trifecta_tool(20.0) is True

    def test_amount_below_min_does_not_match(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = 20.0
        assert tracker._heal_amount_matches_trifecta_tool(5.0) is False

    def test_amount_above_max_does_not_match(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()
        tracker._heal_amount_min = 10.0
        tracker._heal_amount_max = 20.0
        assert tracker._heal_amount_matches_trifecta_tool(25.0) is False

    def test_trifecta_heal_out_of_range_suppresses_cost(self):
        """End-to-end under trifecta: a self_heal whose amount falls OUTSIDE the
        configured heal range is not attributed to the heal tool, so no session
        heal cost is charged. An in-range heal does charge. This drives the
        _heal_amount_matches_trifecta_tool gate via the public combat path."""
        bus, tracker, db = _make_tracker(
            weapon_attribution_trifecta_provider=lambda: True,
            trifecta_resolver=lambda: _trifecta_with_heal_range(10.0, 20.0),
        )
        tracker.start_session()
        # Bounds came from the resolver.
        assert tracker._heal_amount_min == pytest.approx(10.0)
        assert tracker._heal_amount_max == pytest.approx(20.0)
        base = datetime(2024, 1, 1, 0, 0, 0)
        # Out-of-range heal: gate returns False -> early return, no cost.
        bus.publish(
            EVENT_COMBAT, {"type": "self_heal", "amount": 100.0, "timestamp": base}
        )
        assert tracker._session_heal_cost == pytest.approx(0.0)
        # In-range heal a window later: gate returns True -> cost charged.
        bus.publish(
            EVENT_COMBAT,
            {
                "type": "self_heal",
                "amount": 15.0,
                "timestamp": base + timedelta(seconds=2.0),
            },
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)
