"""Scenario replay helpers.

A scripted or recorded scenario lives in a directory whose
``chat_replay.log`` carries one chat.log line per event. The harness
boots a ``ChatlogWatcher`` against a temp file, then the test streams
the scenario's lines into that file; the watcher's real tail loop
reads them as if the game were writing live.

These helpers cover the streaming + draining beats so individual
scenario tests stay short.
"""

from __future__ import annotations

from pathlib import Path

from backend.services.chatlog_watcher import ChatlogWatcher

# Ceiling on a drain wait. The condition wait returns the instant the watcher
# is genuinely idle, so this is only reached when a watcher never drains (a
# bug), never a routine wait duration. Generous enough to absorb worst-case
# thread-scheduling latency on a saturated parallel runner.
DRAIN_TIMEOUT_S = 10.0


def replay_scenario(scenario_dir: Path, chatlog_path: Path) -> None:
    """Stream the scenario's chat replay into ``chatlog_path``.

    Lines are written + flushed one at a time so the watcher's tail
    loop reads them through its normal ``readline()`` path. The
    scenario's timestamps drive tracker behaviour; wall-clock spacing
    between writes is unimportant for tick-boundary correctness
    because the tick boundary is derived from the parsed timestamp,
    not from when the line landed on disk.
    """

    source = scenario_dir / "chat_replay.log"
    lines = source.read_text(encoding="utf-8").splitlines(keepends=True)

    with chatlog_path.open("a", encoding="utf-8") as sink:
        for line in lines:
            sink.write(line)
            sink.flush()


def wait_for_drain(
    watcher: ChatlogWatcher,
    chatlog_path: Path,
    *,
    timeout: float = DRAIN_TIMEOUT_S,
) -> None:
    """Block until ``watcher`` has tailed every line currently in
    ``chatlog_path`` and flushed its final idle tick.

    The watcher seeks to end-of-file when it starts, so the cumulative line
    count it reports equals the number of lines a scenario has appended to the
    (initially empty) chatlog. This waits on that condition rather than
    sleeping a fixed interval: it converges the instant the tail loop is
    genuinely idle, and stays correct when heavy parallel load delays the
    watcher thread. ``TimeoutError`` propagates if the watcher never drains.
    """

    target_lines = len(chatlog_path.read_text(encoding="utf-8").splitlines())
    watcher.wait_until_drained(target_lines, timeout=timeout)
