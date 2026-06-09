"""Tests for ChatlogWatcher tick buffering, loot grouping, and quest suppression.

Verifies that:
  • Loot items sharing the same timestamp are grouped into a single EVENT_LOOT_GROUP.
  • Tick buffering correctly groups events by timestamp.
  • Mission completion triggers quest reward suppression when a filter is installed.
"""

import logging
from datetime import datetime

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_LOOT_GROUP,
    EVENT_MISSION_RECEIVED,
    EVENT_SKILL_GAIN,
)
from backend.services.chatlog_watcher import PERF_LOG_INTERVAL_S, ChatlogWatcher
from backend.testing.clock import MockClock


def _make_loot_line(ts: str, item: str, value: str) -> str:
    """Build a valid chat.log loot line."""
    return f"{ts} [System] [] You received {item} Value: {value} PED"


def _make_loot_qty_line(ts: str, item: str, qty: int, value: str) -> str:
    """Build a valid chat.log loot line with quantity."""
    return f"{ts} [System] [] You received {item} x ({qty}) Value: {value} PED"


def _make_damage_line(ts: str, amount: str) -> str:
    return f"{ts} [System] [] You inflicted {amount} points of damage"


def _make_skill_line(ts: str, amount: str, skill: str) -> str:
    return f"{ts} [System] [] You have gained {amount} {skill}"


def _make_enhancer_break_line(
    ts: str, enhancer: str, item: str, remaining: int, shrapnel: str
) -> str:
    return (
        f"{ts} [System] [] Your enhancer {enhancer} on your {item} broke. "
        f"You have {remaining} enhancers remaining on the item. "
        f"You received {shrapnel} PED Shrapnel."
    )


def _make_mission_complete_line(ts: str, name: str) -> str:
    return f"{ts} [System] [] Mission completed ({name})"


def _make_mission_received_line(ts: str, name: str) -> str:
    return f"{ts} [System] [] New Mission received ({name})"


class TestLootGrouping:
    """Test that the watcher groups loot items by timestamp."""

    def _make_watcher(self):
        bus = EventBus()
        events = []
        bus.subscribe(EVENT_LOOT_GROUP, lambda d: events.append(("loot", d)))
        bus.subscribe(EVENT_COMBAT, lambda d: events.append(("combat", d)))
        bus.subscribe(EVENT_SKILL_GAIN, lambda d: events.append(("skill", d)))
        bus.subscribe(
            EVENT_ENHANCER_BREAK, lambda d: events.append(("enhancer_break", d))
        )
        bus.subscribe(
            EVENT_MISSION_RECEIVED, lambda d: events.append(("mission_received", d))
        )
        # Use a dummy path; we won't call start(), just _process_line directly
        watcher = ChatlogWatcher(bus, "dummy.log")
        return watcher, events

    def test_single_loot_item_flushed_on_idle(self):
        """A single loot line produces one group with one item after flush."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_loot_line("2026-03-27 10:00:00", "Animal Muscle Oil", "0.12")
        )
        assert len(events) == 0  # Not flushed yet

        watcher._flush_tick()
        assert len(events) == 1
        assert events[0][0] == "loot"
        assert len(events[0][1]["items"]) == 1
        assert events[0][1]["items"][0]["item_name"] == "Animal Muscle Oil"
        assert events[0][1]["total_ped"] == 0.12

    def test_multiple_items_same_timestamp_grouped(self):
        """Multiple loot lines with the same timestamp → one group."""
        watcher, events = self._make_watcher()
        ts = "2026-03-27 10:00:00"

        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 342, "3.42"))
        watcher._process_line(_make_loot_line(ts, "Animal Muscle Oil", "0.12"))
        watcher._process_line(_make_loot_line(ts, "Animal Oil Residue", "0.03"))
        watcher._flush_tick()

        assert len(events) == 1
        group = events[0][1]
        assert len(group["items"]) == 3
        assert group["total_ped"] == pytest.approx(3.57)
        names = [i["item_name"] for i in group["items"]]
        assert names == ["Shrapnel", "Animal Muscle Oil", "Animal Oil Residue"]

    def test_different_timestamps_produce_separate_groups(self):
        """Loot lines with different timestamps → separate groups."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_loot_qty_line("2026-03-27 10:00:00", "Shrapnel", 100, "1.00")
        )
        watcher._process_line(
            _make_loot_line("2026-03-27 10:00:00", "Animal Oil Residue", "0.05")
        )
        # Different timestamp → flushes first group, starts second
        watcher._process_line(
            _make_loot_qty_line("2026-03-27 10:00:05", "Shrapnel", 200, "2.00")
        )
        watcher._flush_tick()

        assert len(events) == 2
        assert len(events[0][1]["items"]) == 2
        assert events[0][1]["total_ped"] == pytest.approx(1.05)
        assert len(events[1][1]["items"]) == 1
        assert events[1][1]["total_ped"] == pytest.approx(2.00)

    def test_non_loot_event_at_different_timestamp_flushes_tick(self):
        """A combat event at a new timestamp flushes the previous tick."""
        watcher, events = self._make_watcher()
        ts = "2026-03-27 10:00:00"

        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 50, "0.50"))
        watcher._process_line(_make_loot_line(ts, "Blazar Fragment", "1.20"))
        assert len(events) == 0  # Still pending

        # Combat at new timestamp flushes previous tick
        watcher._process_line(_make_damage_line("2026-03-27 10:00:01", "10.5"))
        watcher._flush_tick()
        assert len(events) == 2  # loot group + combat event
        assert events[0][0] == "loot"
        assert len(events[0][1]["items"]) == 2
        assert events[0][1]["total_ped"] == pytest.approx(1.70)
        assert events[1][0] == "combat"

    def test_skill_event_same_timestamp_grouped_with_loot(self):
        """Loot + skill at same timestamp are in the same tick, emitted together."""
        watcher, events = self._make_watcher()
        ts = "2026-03-27 10:00:00"

        watcher._process_line(_make_loot_line(ts, "Animal Hide", "0.08"))
        watcher._process_line(_make_skill_line(ts, "0.12", "Bravado"))
        watcher._flush_tick()

        assert len(events) == 2
        assert events[0][0] == "loot"
        assert events[1][0] == "skill"

    def test_unparseable_line_does_not_break_tick(self):
        """Chat messages from other channels don't split a tick.

        This fixes a prior issue where Rookie/Local messages interleaving
        with System events at the same timestamp would break loot grouping.
        """
        watcher, events = self._make_watcher()
        ts = "2026-03-27 10:00:00"

        watcher._process_line(_make_loot_line(ts, "Shrapnel", "0.50"))
        # Interleaving chat message at same timestamp: should NOT break tick
        watcher._process_line(f"{ts} [Rookie] [SomePlayer] hello world")
        watcher._process_line(_make_loot_line(ts, "Animal Oil Residue", "0.10"))
        watcher._flush_tick()

        assert len(events) == 1
        assert len(events[0][1]["items"]) == 2  # Both items in one group

    def test_flush_when_empty_is_noop(self):
        """Flushing with no pending events publishes nothing."""
        watcher, events = self._make_watcher()
        watcher._flush_tick()
        assert len(events) == 0

    def test_double_flush_is_noop(self):
        """Second flush after already flushed publishes nothing."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_loot_line("2026-03-27 10:00:00", "Shrapnel", "0.50")
        )
        watcher._flush_tick()
        assert len(events) == 1

        watcher._flush_tick()
        assert len(events) == 1  # No second event

    def test_quantity_preserved_in_group(self):
        """Item quantity from chat.log is preserved in the grouped event."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_loot_qty_line("2026-03-27 10:00:00", "Shrapnel", 4762, "47.62")
        )
        watcher._flush_tick()

        item = events[0][1]["items"][0]
        assert item["item_name"] == "Shrapnel"
        assert item["quantity"] == 4762
        assert item["value_ped"] == 47.62

    def test_timestamp_preserved_in_group(self):
        """The group's timestamp matches the chat.log loot timestamp."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_loot_line("2026-03-27 10:00:00", "Shrapnel", "1.00")
        )
        watcher._flush_tick()

        assert events[0][1]["timestamp"] == datetime(2026, 3, 27, 10, 0, 0)

    def test_double_shrapnel_same_timestamp(self):
        """Two shrapnel lines with same timestamp → one group, two items.

        Entropia can emit the same item name twice in one kill (e.g. two
        separate shrapnel stacks). Both must appear in the group.
        """
        watcher, events = self._make_watcher()
        ts = "2026-03-27 10:00:00"

        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 4762, "47.62"))
        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 5690, "56.90"))
        watcher._flush_tick()

        assert len(events) == 1
        group = events[0][1]
        assert len(group["items"]) == 2
        assert group["total_ped"] == pytest.approx(104.52)

    def test_interleaved_loot_combat_loot(self):
        """Loot → combat (new ts) → loot produces two separate loot groups."""
        watcher, events = self._make_watcher()

        watcher._process_line(_make_loot_line("2026-03-27 10:00:00", "Hide", "0.08"))
        watcher._process_line(_make_damage_line("2026-03-27 10:00:01", "15.0"))
        watcher._process_line(_make_loot_line("2026-03-27 10:00:05", "Wool", "0.03"))
        watcher._flush_tick()

        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 2
        assert loot_events[0][1]["items"][0]["item_name"] == "Hide"
        assert loot_events[1][1]["items"][0]["item_name"] == "Wool"

    def test_enhancer_break_marks_matching_shrapnel_and_emits_first(self):
        watcher, events = self._make_watcher()
        ts = "2026-04-05 09:12:27"

        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 8000, "0.80"))
        watcher._process_line(
            _make_enhancer_break_line(
                ts,
                "T1 Weapon Damage Enhancer",
                "Electric Attack Nanochip 9",
                3,
                "0.80",
            )
        )
        watcher._flush_tick()

        assert [event[0] for event in events] == ["enhancer_break", "loot"]
        loot_item = events[1][1]["items"][0]
        assert loot_item["item_name"] == "Shrapnel"
        assert loot_item["is_enhancer_shrapnel"] is True

    def test_only_matching_shrapnel_stack_is_flagged(self):
        watcher, events = self._make_watcher()
        ts = "2026-04-05 09:12:27"

        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 8000, "0.80"))
        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 17317, "1.73"))
        watcher._process_line(
            _make_enhancer_break_line(
                ts,
                "T1 Weapon Damage Enhancer",
                "Electric Attack Nanochip 9",
                3,
                "0.80",
            )
        )
        watcher._flush_tick()

        items = events[1][1]["items"]
        assert [item["is_enhancer_shrapnel"] for item in items] == [True, False]
        # The flagged stack must be the one whose value matches the refund;
        # pin both values so a mutant cannot flip the flag onto the wrong stack.
        assert items[0]["value_ped"] == pytest.approx(0.80)
        assert items[0]["is_enhancer_shrapnel"] is True
        assert items[1]["value_ped"] == pytest.approx(1.73)
        assert items[1]["is_enhancer_shrapnel"] is False

    def test_mission_received_event_emitted(self):
        """New Mission received line emits EVENT_MISSION_RECEIVED."""
        watcher, events = self._make_watcher()

        watcher._process_line(
            _make_mission_received_line("2026-03-27 10:00:00", "ARIS - Daily Hunting 1")
        )
        watcher._flush_tick()

        mr_events = [e for e in events if e[0] == "mission_received"]
        assert len(mr_events) == 1
        assert mr_events[0][1]["mission_name"] == "ARIS - Daily Hunting 1"


class TestQuestRewardSuppression:
    """Test that quest reward items are filtered from loot/skill events."""

    def _make_watcher_with_filter(self, filter_fn):
        bus = EventBus()
        events = []
        bus.subscribe(EVENT_LOOT_GROUP, lambda d: events.append(("loot", d)))
        bus.subscribe(EVENT_COMBAT, lambda d: events.append(("combat", d)))
        bus.subscribe(EVENT_SKILL_GAIN, lambda d: events.append(("skill", d)))
        watcher = ChatlogWatcher(bus, "dummy.log", quest_reward_filter=filter_fn)
        return watcher, events

    def test_ped_reward_suppressed_from_loot(self):
        """Quest reward item matched by PED value is suppressed; mob loot remains."""

        def fake_filter(mission_name, loot_items, skill_gains):
            # Simulate: quest has 1.5 PED reward
            for i, item in enumerate(loot_items):
                if abs(item["value"] - 1.5) < 0.02:
                    return {"suppress_loot_index": i, "suppress_skill_index": None}
            return None

        watcher, events = self._make_watcher_with_filter(fake_filter)
        ts = "2026-03-25 12:54:57"

        # Quest reward: Universal Ammo 1.5 PED
        watcher._process_line(_make_loot_qty_line(ts, "Universal Ammo", 15000, "1.50"))
        # Mission complete
        watcher._process_line(
            _make_mission_complete_line(ts, "Paneleon Hunter (repeatable)")
        )
        # Mob loot
        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 825, "0.0825"))
        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 7247, "0.7247"))
        watcher._process_line(_make_loot_line(ts, "Animal Eye Oil", "0.80"))
        watcher._flush_tick()

        # Should have: loot group (minus UA) + mission_complete
        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1
        items = loot_events[0][1]["items"]
        assert len(items) == 3  # Shrapnel, Shrapnel, Animal Eye Oil (UA suppressed)
        names = [i["item_name"] for i in items]
        assert "Universal Ammo" not in names
        # Pin order and per-item values, not just the sum: a mutant that drops a
        # different item (or keeps Universal Ammo) can still hit the same total.
        assert names == ["Shrapnel", "Shrapnel", "Animal Eye Oil"]
        assert items[0]["value_ped"] == pytest.approx(0.0825)
        assert items[1]["value_ped"] == pytest.approx(0.7247)
        assert items[2]["value_ped"] == pytest.approx(0.80)
        assert loot_events[0][1]["total_ped"] == pytest.approx(0.0825 + 0.7247 + 0.80)

    def test_zero_ped_reward_suppresses_lowest_value(self):
        """0 PED quest reward: suppress the lowest-value item (badge/token)."""

        def fake_filter(mission_name, loot_items, skill_gains):
            # Simulate: quest has 0 PED reward → suppress lowest-value item
            if loot_items:
                min_idx = min(
                    range(len(loot_items)), key=lambda i: loot_items[i]["value"]
                )
                return {"suppress_loot_index": min_idx, "suppress_skill_index": None}
            return None

        watcher, events = self._make_watcher_with_filter(fake_filter)
        ts = "2026-03-25 12:54:57"

        # Badge at 0 PED (quest reward)
        watcher._process_line(_make_loot_qty_line(ts, "A.R.C. Faction Badge", 3, "0"))
        watcher._process_line(
            _make_mission_complete_line(
                ts, "Atlas Haven Imperium Ranger Hunt! (repeatable)"
            )
        )
        # Mob loot
        watcher._process_line(_make_loot_qty_line(ts, "Shrapnel", 825, "0.0825"))
        watcher._process_line(_make_loot_line(ts, "Animal Eye Oil", "0.80"))
        watcher._flush_tick()

        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1
        items = loot_events[0][1]["items"]
        assert len(items) == 2  # Badge suppressed
        names = [i["item_name"] for i in items]
        assert "A.R.C. Faction Badge" not in names

    def test_skill_reward_suppressed(self):
        """Skill quest reward: first skill gain in tick suppressed."""

        def fake_filter(mission_name, loot_items, skill_gains):
            if skill_gains:
                return {"suppress_loot_index": None, "suppress_skill_index": 0}
            return None

        watcher, events = self._make_watcher_with_filter(fake_filter)
        ts = "2026-03-25 12:54:57"

        watcher._process_line(
            _make_skill_line(ts, "0.5000", "Laser Weaponry Technology")
        )
        watcher._process_line(_make_mission_complete_line(ts, "Skill Quest"))
        # Mob loot at same timestamp
        watcher._process_line(_make_loot_line(ts, "Shrapnel", "0.50"))
        watcher._flush_tick()

        skill_events = [e for e in events if e[0] == "skill"]
        assert len(skill_events) == 0  # Suppressed

        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1  # Mob loot still emitted

    def test_no_filter_installed_all_events_pass(self):
        """Without a quest_reward_filter, all events pass through unchanged."""
        watcher, events = self._make_watcher_with_filter(None)
        ts = "2026-03-25 12:54:57"

        watcher._process_line(_make_loot_qty_line(ts, "Universal Ammo", 15000, "1.50"))
        watcher._process_line(_make_mission_complete_line(ts, "Some Quest"))
        watcher._process_line(_make_loot_line(ts, "Shrapnel", "0.50"))
        watcher._flush_tick()

        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1
        items = loot_events[0][1]["items"]
        assert len(items) == 2  # Both items pass through
        assert {i["item_name"] for i in items} == {"Universal Ammo", "Shrapnel"}
        assert loot_events[0][1]["total_ped"] == pytest.approx(2.0)

    def test_filter_returns_none_all_events_pass(self):
        """Filter returning None means no suppression."""
        watcher, events = self._make_watcher_with_filter(lambda *_: None)
        ts = "2026-03-25 12:54:57"

        watcher._process_line(_make_loot_qty_line(ts, "Universal Ammo", 15000, "1.50"))
        watcher._process_line(_make_mission_complete_line(ts, "Unknown Quest"))
        watcher._process_line(_make_loot_line(ts, "Shrapnel", "0.50"))
        watcher._flush_tick()

        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1
        assert len(loot_events[0][1]["items"]) == 2

    def test_mob_loot_at_next_second_not_suppressed(self):
        """Loot at a different timestamp from mission complete is never suppressed."""

        def fake_filter(mission_name, loot_items, skill_gains):
            # Would suppress lowest value, but only items in the SAME tick
            if loot_items:
                min_idx = min(
                    range(len(loot_items)), key=lambda i: loot_items[i]["value"]
                )
                return {"suppress_loot_index": min_idx, "suppress_skill_index": None}
            return None

        watcher, events = self._make_watcher_with_filter(fake_filter)

        # Tick 1: badge + mission complete (same ts)
        watcher._process_line(
            _make_loot_qty_line(
                "2026-03-25 13:14:33", "A.R.C. Faction Badge", 10, "0.0001"
            )
        )
        watcher._process_line(
            _make_mission_complete_line(
                "2026-03-25 13:14:33", "Island Biome Hunt! (repeatable)"
            )
        )
        # Tick 2: mob loot at next second
        watcher._process_line(_make_loot_line("2026-03-25 13:14:34", "Wool", "0.20"))
        watcher._process_line(
            _make_loot_qty_line("2026-03-25 13:14:34", "Shrapnel", 209, "0.0209")
        )
        watcher._flush_tick()

        loot_events = [e for e in events if e[0] == "loot"]
        # Tick 1 loot: badge suppressed → no loot event (empty after suppression)
        # Tick 2 loot: Wool + Shrapnel → one loot event
        assert len(loot_events) == 1
        assert len(loot_events[0][1]["items"]) == 2  # Wool + Shrapnel, unsuppressed
        names = [i["item_name"] for i in loot_events[0][1]["items"]]
        assert "Wool" in names
        assert "Shrapnel" in names

    def test_filter_exception_does_not_crash_watcher(self):
        """If the filter callback raises, events still pass through."""

        def exploding_filter(mission_name, loot_items, skill_gains):
            raise RuntimeError("boom")

        watcher, events = self._make_watcher_with_filter(exploding_filter)
        ts = "2026-03-25 12:54:57"

        watcher._process_line(_make_loot_line(ts, "Shrapnel", "0.50"))
        watcher._process_line(_make_mission_complete_line(ts, "Some Quest"))
        watcher._flush_tick()

        # Events should still be emitted despite exception
        loot_events = [e for e in events if e[0] == "loot"]
        assert len(loot_events) == 1


class TestDrainSeams:
    """The condition-drain seams the replay helpers depend on.

    These pin the watcher contract the e2e/recorder drain helpers rely on:
    ``start()`` is ready to tail before it returns, ``wait_until_drained``
    blocks on the tail loop's progress rather than a clock, and a watcher that
    never reaches the requested line count raises rather than passing silently.
    """

    def _write(self, path, lines: list[str]) -> None:
        with path.open("a", encoding="utf-8") as sink:
            for line in lines:
                sink.write(line)
                sink.flush()

    def test_start_is_ready_to_tail_before_returning(self, tmp_path):
        """Lines written immediately after start() are tailed, not missed.

        The readiness barrier means start() has opened the file and seeked to
        end by the time it returns, so a write that lands right after cannot
        slip past a not-yet-seeked watcher.
        """
        chatlog = tmp_path / "chat.log"
        chatlog.touch()
        bus = EventBus()
        loot: list = []
        bus.subscribe(EVENT_LOOT_GROUP, loot.append)
        watcher = ChatlogWatcher(bus, chatlog)
        watcher.start()
        try:
            line = _make_loot_line("2026-03-27 10:00:00", "Animal Muscle Oil", "0.12")
            self._write(chatlog, [line])
            watcher.wait_until_drained(1, timeout=5.0)
        finally:
            watcher.stop()

        assert watcher.lines_seen == 1
        assert not watcher.has_pending_tick
        assert len(loot) == 1
        # Pin the parsed payload, not just the count: a mutant that drops the
        # value or mislabels the item would keep the count but corrupt the event.
        assert loot[0]["timestamp"] == datetime(2026, 3, 27, 10, 0, 0)
        assert loot[0]["items"][0]["item_name"] == "Animal Muscle Oil"
        assert loot[0]["total_ped"] == 0.12

    def test_wait_until_drained_raises_when_target_unreached(self, tmp_path):
        """A watcher that never reads the requested lines raises TimeoutError.

        A drain that the watcher cannot satisfy is a bug to surface, never a
        wait to sleep through; the helper raises rather than returning.
        """
        chatlog = tmp_path / "chat.log"
        chatlog.touch()
        watcher = ChatlogWatcher(EventBus(), chatlog)
        watcher.start()
        try:
            # The message must surface both the unmet target and the actual
            # progress so a stuck drain is diagnosable from the traceback alone.
            with pytest.raises(
                TimeoutError, match=r"did not drain to 1 line.*within 0\.3s.*read 0"
            ):
                # Nothing is ever written, so line one never arrives.
                watcher.wait_until_drained(1, timeout=0.3)
        finally:
            watcher.stop()

    def test_wait_until_drained_times_out_with_a_frozen_injected_clock(self, tmp_path):
        """A frozen injected clock cannot turn a failed drain into a hang.

        The drain deadline runs on the watcher's own real, advancing clock,
        not the injected one, so a deterministic-replay harness that injects a
        frozen clock still gets a TimeoutError rather than an infinite wait
        (a frozen monotonic stream would otherwise hold ``remaining`` above
        zero forever).
        """
        chatlog = tmp_path / "chat.log"
        chatlog.touch()
        # MockClock().monotonic() never advances: routed through the deadline
        # arithmetic it would freeze the timeout. The dedicated timeout clock
        # must ignore it and still expire.
        watcher = ChatlogWatcher(EventBus(), chatlog, clock=MockClock())
        watcher.start()
        try:
            with pytest.raises(
                TimeoutError, match=r"did not drain to 1 line.*within 0\.3s"
            ):
                watcher.wait_until_drained(1, timeout=0.3)
        finally:
            watcher.stop()

    def test_start_does_not_hang_when_readiness_times_out(self, tmp_path):
        """If the readiness signal never fires, start() warns and returns.

        The barrier must not turn a slow or failed startup into a hang; the
        wait is bounded and start() proceeds (the watcher thread is still
        live) rather than blocking the caller indefinitely.
        """

        class _NeverReady:
            def clear(self):
                pass

            def set(self):
                pass

            def wait(self, timeout=None):
                return False

        chatlog = tmp_path / "chat.log"
        chatlog.touch()
        watcher = ChatlogWatcher(EventBus(), chatlog)
        watcher._ready = _NeverReady()  # type: ignore[assignment]
        watcher.start()
        try:
            assert watcher.is_running
        finally:
            watcher.stop()

    def test_pending_tick_reflects_buffer_state(self):
        """has_pending_tick tracks the tick buffer across process and flush."""
        bus = EventBus()
        bus.subscribe(EVENT_LOOT_GROUP, lambda _d: None)
        watcher = ChatlogWatcher(bus, "dummy.log")

        assert not watcher.has_pending_tick
        watcher._process_line(
            _make_loot_line("2026-03-27 10:00:00", "Animal Muscle Oil", "0.12")
        )
        assert watcher.has_pending_tick
        watcher._flush_tick()
        assert not watcher.has_pending_tick

    def test_drain_counter_survives_debug_perf_window_reset(self, caplog):
        """``lines_seen`` keeps counting across a DEBUG perf-window reset.

        The perf summary zeroes its window counters every interval when DEBUG
        logging is enabled; the cumulative counter the drain helpers compare
        against must be immune to that reset, or ``wait_until_drained`` would
        silently stop converging the moment verbose logging is switched on.
        """
        clock = MockClock()
        watcher = ChatlogWatcher(EventBus(), "dummy.log", clock=clock)
        caplog.set_level(logging.DEBUG, logger="backend.services.chatlog_watcher")

        watcher._process_line("not a chat line\n")
        assert watcher.lines_seen == 1

        # Step the injected clock past the perf interval so the next processed
        # line triggers the window reset, then keep feeding lines.
        clock.advance(PERF_LOG_INTERVAL_S + 1.0)
        watcher._process_line("not a chat line\n")
        watcher._process_line("not a chat line\n")

        # The reset must actually have fired for this test to mean anything.
        assert any("Chatlog watcher perf" in rec.message for rec in caplog.records)
        assert watcher.lines_seen == 3
