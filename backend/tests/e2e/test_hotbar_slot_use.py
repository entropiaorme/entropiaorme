"""Acceptance test for the ``hotbar_slot_use`` scenario.

Wires a ``HotbarListener`` with a ``MockKeystrokeSource`` onto the
harness pipeline and a small in-memory hotbar resolver, then injects
the scenario's three recorded hotbar presses (slots ``1`` / ``2`` /
``3``: weapon / heal / consumable) and pins the resulting bus event
stream via ``pytest-regressions``. The consumable slot is the negative
assertion: it resolves but publishes nothing.
"""

from __future__ import annotations

import json
from collections.abc import Iterable
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_ACTIVE_HEAL_TOOL_CHANGED,
    EVENT_ACTIVE_TOOL_CHANGED,
)
from backend.services.hotbar_listener import HotbarListener
from backend.testing.keystroke_source import MockKeystrokeSource
from backend.testing.replay import replay_scenario, wait_for_drain


def _hotbar_resolver(slot: str):
    """Map a hotbar slot to a (name, cost, item_type, reload_s) tuple.

    Mirrors the production resolver's contract; ``None`` indicates an
    unbound slot. Three slots are bound: weapon, heal, consumable.
    """
    return {
        "1": ("Sollomate Opalo", 0.50, "weapon", 1.6),
        "2": ("FAP-5", 0.30, "healing", 2.5),
        "3": ("CDF Cola", 0.10, "consumable", 0.0),
    }.get(slot)


def _load_keystrokes(scenario: Path) -> list[dict]:
    """Read the scenario's ``keystrokes.jsonl`` into press/release records."""
    path = scenario / "keystrokes.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    return [json.loads(line) for line in lines if line.strip()]


def _record_bus(bus: EventBus, topics: Iterable[str]) -> list[dict]:
    """Subscribe a list-collector to ``topics`` and return the list."""
    seen: list[dict] = []

    def _make_handler(topic: str):
        def _handler(payload):
            seen.append({"topic": topic, "payload": payload})

        return _handler

    for topic in topics:
        bus.subscribe(topic, _make_handler(topic))
    return seen


def test_hotbar_slot_use_drives_listener_via_keystroke_source(
    e2e_pipeline,
    corpus_root: Path,
    in_memory_db,
    data_regression,
) -> None:
    """The three recorded hotbar presses produce the expected bus
    event stream: weapon → ACTIVE_TOOL_CHANGED, heal →
    ACTIVE_HEAL_TOOL_CHANGED, consumable → no event.
    """

    bus, tracker, _watcher, chatlog = e2e_pipeline
    scenario = corpus_root / "scripted" / "hotbar_slot_use"

    # Capture every tool-change publication, in order, before the
    # listener is wired so subscription is in place when its callbacks
    # run.
    seen = _record_bus(
        bus,
        topics=(EVENT_ACTIVE_TOOL_CHANGED, EVENT_ACTIVE_HEAL_TOOL_CHANGED),
    )

    # Wire the seam under test onto the harness pipeline.
    source = MockKeystrokeSource()
    listener = HotbarListener(
        bus,
        keystroke_source=source,
        hotbar_resolver=_hotbar_resolver,
    )
    listener.set_hotbar_hooks_enabled(True)

    tracker.start_session()
    # Session-started published; capability already on; source now running.
    assert listener.is_running

    # Drive the chat side first so the hunt context is registered, then
    # inject the recorded keystrokes in offset_s order.
    replay_scenario(scenario, chatlog)
    keystrokes = _load_keystrokes(scenario)
    for record in keystrokes:
        from datetime import datetime

        ts = datetime.fromisoformat(record["wall"])
        source.inject(record["key"], ts, record["kind"])

    wait_for_drain()
    # Listener's resolver runs off-thread; give it a brief moment to land.
    _spin_until(lambda: len(seen) >= 2)
    result = tracker.stop_session()

    # Chat-side sanity: two damage shots + one loot tick = one kill.
    assert len(result.kills) == 1
    kill = result.kills[0]
    assert kill.shots_fired == 2
    assert kill.damage_dealt == pytest.approx(35.0)
    assert kill.loot_total_ped == pytest.approx(5.00)

    # Bus side: weapon press → ACTIVE_TOOL_CHANGED; heal press →
    # ACTIVE_HEAL_TOOL_CHANGED; consumable press → no event (negative
    # assertion encoded by the goldens containing two entries, not three).
    assert len(seen) == 2
    data_regression.check(_normalise(seen))


def _normalise(events: list[dict]) -> list[dict]:
    """Render the captured event stream in stable, golden-friendly form.

    Each entry becomes a flat dict with deterministically-ordered keys
    so ``data_regression``'s YAML output is human-readable and the diff
    surface is tight when the listener contract drifts.
    """
    out: list[dict] = []
    for event in events:
        payload = dict(event["payload"])
        entry = {"topic": event["topic"]}
        # Sort payload keys for stable YAML emission.
        for key in sorted(payload):
            entry[key] = payload[key]
        out.append(entry)
    return out


def _spin_until(predicate, *, timeout_s: float = 2.0) -> None:
    """Block until ``predicate()`` is true, or raise after ``timeout_s``."""
    import time

    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    raise AssertionError("predicate did not become true within timeout")
