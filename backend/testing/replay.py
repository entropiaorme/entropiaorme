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

import time
from pathlib import Path


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


def wait_for_drain(seconds: float = 0.6) -> None:
    """Sleep long enough for the watcher's tail loop to read every
    pending line and idle-flush the final tick.

    The watcher polls at ``TAIL_INTERVAL = 0.1s``. After a write burst
    the loop needs one cycle to read each remaining line and one more
    to observe a quiescent file and flush. The default of 0.6s leaves
    generous headroom over the minimum (~0.2s) so the helper stays
    deterministic under CI scheduling jitter without slowing the fast
    tier noticeably.
    """

    time.sleep(seconds)
