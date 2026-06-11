"""Line-server oracle exposing the hotbar listener pipeline.

Each request scripts a session: the capability toggle, session
start/stop events, and injected keystrokes flow through the real
``HotbarListener`` with a ``MockKeystrokeSource`` and a scripted
resolver; the reply is the ordered (topic, payload) sequence the bus
observed plus the keystroke-tap record. The native port's differential
drives the same script through its listener and compares replies
byte-for-byte. Part of the equivalence oracle surface; never imported
by production code.

The scripted resolver maps slots to fixed tuples: "1" resolves to a
weapon, "2" to a healing tool, "3" to a consumable, everything else to
an empty slot.
"""

from __future__ import annotations

import json
import sys
from datetime import UTC, datetime

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_SESSION_STARTED, EVENT_SESSION_STOPPED
from backend.services.hotbar_listener import HotbarListener
from backend.testing.keystroke_source import MockKeystrokeSource
from backend.testing.stdio import pin_utf8_line_protocol


def _resolver(slot: str):
    return {
        "1": ("Opalo", 0.05, "weapon", 0.0),
        "2": ("Healer", 0.088, "healing", 2.5),
        "3": ("Snack", 0.01, "consumable", 0.0),
    }.get(slot)


def _run(request: dict) -> dict:
    recorded: list[dict] = []
    taps: list[list[str]] = []
    bus = EventBus()
    bus.add_tap(lambda topic, data: recorded.append({"topic": topic, "payload": data}))

    source = MockKeystrokeSource()
    listener = HotbarListener(bus, keystroke_source=source, hotbar_resolver=_resolver)
    listener.set_key_tap(lambda key, kind: taps.append([key, kind]))

    import time

    def await_tool_events(count: int) -> None:
        """Park until ``count`` tool events have landed (bounded), so a
        script can serialise resolutions that would otherwise race on
        the original's per-press worker threads."""
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline:
            tool_count = sum(
                1
                for entry in recorded
                if entry["topic"] in ("active_tool_changed", "active_heal_tool_changed")
            )
            if tool_count >= count:
                return
            time.sleep(0.01)

    timestamp = datetime(2026, 5, 19, 10, 0, 0, tzinfo=UTC)
    for step in request["steps"]:
        kind = step[0]
        if kind == "toggle":
            listener.set_hotbar_hooks_enabled(bool(step[1]))
        elif kind == "session":
            bus.publish(
                EVENT_SESSION_STARTED if step[1] else EVENT_SESSION_STOPPED, None
            )
        elif kind == "key":
            source.inject(step[1], timestamp, step[2])
        elif kind == "await":
            await_tool_events(int(step[1]))

    listener.stop()

    return {"stream": recorded, "taps": taps, "running": listener.is_running}


def main() -> None:
    pin_utf8_line_protocol()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        result = _run(json.loads(line))
        sys.stdout.write(json.dumps(result, sort_keys=True, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
