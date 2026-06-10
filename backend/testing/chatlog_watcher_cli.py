"""Line-server oracle exposing the chat.log watcher pipeline.

Each request carries a list of raw chat.log lines (and optional fixed
quest-suppression indexes). The oracle streams them through the real
``ChatlogWatcher`` against a temporary file with a recording bus tap,
drains, stops, and replies with the ordered (topic, payload) sequence
the bus observed; datetimes render as their string form. The native
port's differential drives the same lines through its watcher and
compares sequences byte-for-byte. Part of the equivalence oracle
surface; never imported by production code.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher


def _run(request: dict) -> list:
    lines = request["lines"]
    suppress = request.get("suppress")

    quest_filter = None
    if suppress is not None:

        def quest_filter(mission_name, loot_items, skill_gains):  # noqa: ARG001
            return suppress

    recorded: list[tuple[str, object]] = []
    bus = EventBus()
    bus.add_tap(lambda topic, data: recorded.append((topic, data)))

    with tempfile.TemporaryDirectory() as tmp:
        chatlog = Path(tmp) / "chat_replay.log"
        chatlog.touch()
        watcher = ChatlogWatcher(bus, chatlog, quest_reward_filter=quest_filter)
        watcher.start()
        try:
            with chatlog.open("a", encoding="utf-8") as handle:
                for line in lines:
                    handle.write(line + "\n")
            watcher.wait_until_drained(len(lines))
        finally:
            watcher.stop()

    return [{"topic": topic, "payload": payload} for topic, payload in recorded]


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request = json.loads(line)
        result = _run(request)
        sys.stdout.write(
            json.dumps(result, sort_keys=True, ensure_ascii=False, default=str) + "\n"
        )
        sys.stdout.flush()


if __name__ == "__main__":
    main()
