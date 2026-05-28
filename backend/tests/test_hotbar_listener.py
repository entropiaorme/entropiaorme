"""Unit tests for ``HotbarListener`` driven by ``MockKeystrokeSource``.

The listener's job under the seam:

1. Gate the keystroke source's lifecycle on (capability toggle) AND
   (tracking session active).
2. On a ``press`` event for a hotbar slot key, fire the resolver and
   publish the right tool-change event.
3. Pass every press through the optional recorder tap.

Tests drive the listener with a ``MockKeystrokeSource`` and an in-memory
``EventBus``, capturing published events and recorded taps without
touching ``pynput``.
"""

from __future__ import annotations

from datetime import datetime

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.services.hotbar_listener import HOTBAR_SLOT_KEYS, HotbarListener
from backend.testing.keystroke_source import MockKeystrokeSource

_TS = datetime(2026, 5, 28, 12, 0, 0)


def _bus_recorder(bus: EventBus, topic: str) -> list[dict]:
    """Subscribe a list-collector to ``topic`` and return the list."""
    seen: list[dict] = []
    bus.subscribe(topic, lambda payload: seen.append(payload))
    return seen


def _make_resolver(mapping: dict[str, tuple[str, float, str, float]]):
    """Build a resolver returning ``mapping[slot]`` (None for misses)."""

    def _resolve(slot: str):
        return mapping.get(slot)

    return _resolve


def test_source_does_not_start_until_capability_and_session_both_active() -> None:
    """The source stays stopped until both gate inputs are true."""
    bus = EventBus()
    source = MockKeystrokeSource()
    listener = HotbarListener(bus, keystroke_source=source)
    assert not listener.is_running

    listener.set_hotbar_hooks_enabled(True)
    assert not listener.is_running  # capability on, but no session

    bus.publish(EVENT_SESSION_STARTED, {})
    assert listener.is_running  # both on

    bus.publish(EVENT_SESSION_STOPPED, {})
    assert not listener.is_running

    bus.publish(EVENT_SESSION_STARTED, {})
    listener.set_hotbar_hooks_enabled(False)
    assert not listener.is_running


def test_weapon_slot_press_publishes_active_tool_changed() -> None:
    """A press on a weapon slot publishes ``EVENT_ACTIVE_TOOL_CHANGED``."""
    bus = EventBus()
    source = MockKeystrokeSource()
    resolver = _make_resolver({"1": ("Sollomate Opalo", 0.50, "weapon", 1.6)})
    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    seen = _bus_recorder(bus, EVENT_ACTIVE_TOOL_CHANGED)
    source.inject("1", _TS, "press")
    # _resolve_hotbar_slot runs off-thread; spin briefly until the publish lands.
    _spin_until(lambda: len(seen) == 1)

    assert seen == [{"tool_name": "Sollomate Opalo", "source": "hotbar:1"}]


def test_heal_slot_press_publishes_active_heal_tool_changed() -> None:
    """A press on a heal slot publishes ``EVENT_ACTIVE_HEAL_TOOL_CHANGED``."""
    bus = EventBus()
    source = MockKeystrokeSource()
    resolver = _make_resolver({"2": ("FAP-5", 0.30, "healing", 2.5)})
    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    seen = _bus_recorder(bus, EVENT_ACTIVE_HEAL_TOOL_CHANGED)
    source.inject("2", _TS, "press")
    _spin_until(lambda: len(seen) == 1)

    assert seen == [
        {
            "tool_name": "FAP-5",
            "cost_per_use_ped": 0.30,
            "reload_seconds": 2.5,
            "source": "hotbar:2",
        }
    ]


def test_consumable_slot_press_does_not_switch_active_tool() -> None:
    """Consumables are one-off actions; no tool-change events fire."""
    bus = EventBus()
    source = MockKeystrokeSource()
    resolver = _make_resolver({"3": ("CDF Cola", 0.10, "consumable", 0.0)})
    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    weapon_seen = _bus_recorder(bus, EVENT_ACTIVE_TOOL_CHANGED)
    heal_seen = _bus_recorder(bus, EVENT_ACTIVE_HEAL_TOOL_CHANGED)
    source.inject("3", _TS, "press")
    # Negative assertion: nothing arrives. Give the off-thread path a moment.
    _spin_briefly()

    assert weapon_seen == []
    assert heal_seen == []


def test_non_hotbar_key_press_is_ignored() -> None:
    """Keys outside HOTBAR_SLOT_KEYS never reach the resolver."""
    bus = EventBus()
    source = MockKeystrokeSource()
    seen_slots: list[str] = []

    def resolver(slot: str):
        seen_slots.append(slot)
        return None

    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    source.inject("a", _TS, "press")
    source.inject("space", _TS, "press")
    source.inject("f1", _TS, "press")
    _spin_briefly()

    assert seen_slots == []


def test_release_events_are_ignored() -> None:
    """Only press events drive the resolver; releases never do."""
    bus = EventBus()
    source = MockKeystrokeSource()
    seen_slots: list[str] = []

    def resolver(slot: str):
        seen_slots.append(slot)
        return None

    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    source.inject("1", _TS, "release")
    _spin_briefly()

    assert seen_slots == []


def test_recorder_tap_receives_every_hotbar_press() -> None:
    """The recorder-tap surface still copies hotbar presses into the recorder."""
    bus = EventBus()
    source = MockKeystrokeSource()
    listener = HotbarListener(bus, keystroke_source=source)
    listener.set_hotbar_hooks_enabled(True)
    bus.publish(EVENT_SESSION_STARTED, {})

    taps: list[tuple[str, str]] = []
    listener.set_key_tap(lambda key, kind: taps.append((key, kind)))

    source.inject("1", _TS, "press")
    source.inject("0", _TS, "press")
    source.inject("a", _TS, "press")  # filtered out
    _spin_briefly()

    assert taps == [("1", "press"), ("0", "press")]

    listener.clear_key_tap()
    source.inject("2", _TS, "press")
    _spin_briefly()
    assert taps == [("1", "press"), ("0", "press")]


def test_injected_events_outside_gate_window_are_no_ops() -> None:
    """Presses arriving while the source is stopped never fire the resolver."""
    bus = EventBus()
    source = MockKeystrokeSource()
    seen_slots: list[str] = []

    def resolver(slot: str):
        seen_slots.append(slot)
        return None

    # Constructed for its side effect of subscribing the resolver callback
    # to the source; never explicitly enabled, so the gate stays closed.
    HotbarListener(bus, keystroke_source=source, hotbar_resolver=resolver)
    # Capability off, session off: source never starts; the mock drops injects.
    source.inject("1", _TS, "press")
    _spin_briefly()
    assert seen_slots == []


def test_hotbar_slot_keys_cover_number_row_zero_through_nine() -> None:
    """Module constant pins the number-row slot vocabulary."""
    assert {"0", "1", "2", "3", "4", "5", "6", "7", "8", "9"} == HOTBAR_SLOT_KEYS


# ----------------------------------------------------------------------
# Threading helpers
# ----------------------------------------------------------------------


def _spin_until(predicate, *, timeout_s: float = 2.0) -> None:
    """Block until ``predicate()`` is true, or raise after ``timeout_s``."""
    import time

    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    raise AssertionError("predicate did not become true within timeout")


def _spin_briefly() -> None:
    """Give off-thread dispatch a small window to either fire or not."""
    import time

    time.sleep(0.05)
