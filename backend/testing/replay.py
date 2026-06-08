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

from collections.abc import Iterator
from pathlib import Path

from backend.services.chatlog_parser import LINE_RE
from backend.services.chatlog_watcher import ChatlogWatcher

# Ceiling on a drain wait. The condition wait returns the instant the watcher
# is genuinely idle, so this is only reached when a watcher never drains (a
# bug), never a routine wait duration. Generous enough to absorb worst-case
# thread-scheduling latency on a saturated parallel runner.
DRAIN_TIMEOUT_S = 10.0


def _tick_key(line: str) -> str | None:
    """The watcher's tick-grouping key for ``line``: its parsed timestamp.

    The watcher buckets events into one app tick per chat.log timestamp
    (second resolution), so two lines share a tick iff their leading
    timestamp tokens are equal. Reusing the parser's own ``LINE_RE`` here
    guarantees the harness groups by exactly the key the watcher splits on.
    Lines with no recognised timestamp never start or extend a tick, so they
    key to ``None`` and stream on their own.
    """
    matched = LINE_RE.match(line.strip())
    return matched.group(1) if matched else None


def _stream_ticks(lines: list[str], chatlog_path: Path) -> None:
    """Append ``lines`` to ``chatlog_path``, flushing once per timestamp tick.

    The watcher closes the current tick whenever its tail loop reaches
    end-of-file, so if it caught up to a writer mid-tick — between two lines
    sharing one timestamp — it would flush a partial tick and the trailing
    same-second line would land in a fresh (wrong) tick, splitting one app
    tick into two. Under parallel load that interleaving is exactly what made
    the metamorphic combat-reorder property flake. Writing each timestamp
    group in a single flush makes a tick the atomic streaming unit, so the
    watcher can never observe end-of-file in the middle of one; ticks still
    stream incrementally (one flush each), so the real tail-read path is
    exercised as before, and the result is now independent of reader/writer
    interleaving as the tick-boundary contract always intended.
    """
    with chatlog_path.open("a", encoding="utf-8") as sink:
        for group in _group_by_tick(lines):
            sink.write("".join(group))
            sink.flush()


def _group_by_tick(lines: list[str]) -> Iterator[list[str]]:
    """Yield runs of consecutive lines sharing one tick key.

    A timestamped line opens a group that absorbs any following untimestamped
    lines (tick-neutral: an untimestamped line neither opens nor closes a tick,
    so it rides with the line before it) and every further line carrying the
    same timestamp; a new timestamp starts a new group. Leading untimestamped
    lines, having no tick to join, stream on their own. Chat.log timestamps are
    monotonic non-decreasing, so same-tick lines are always consecutive and a
    group never has to reach back across an intervening different second.
    """
    group: list[str] = []
    current: str | None = None
    for line in lines:
        key = _tick_key(line)
        if group and key is not None and key != current:
            yield group
            group = []
        group.append(line)
        if key is not None:
            current = key
    if group:
        yield group


def replay_scenario(scenario_dir: Path, chatlog_path: Path) -> None:
    """Stream the scenario's chat replay into ``chatlog_path``.

    Lines stream through the watcher's normal ``readline()`` tail path, one
    flush per timestamp tick (see :func:`_stream_ticks`). The scenario's
    timestamps drive tracker behaviour; wall-clock spacing between ticks is
    unimportant for tick-boundary correctness because the tick boundary is
    derived from the parsed timestamp, not from when a line landed on disk —
    an invariant the per-tick flush is what actually upholds.
    """

    source = scenario_dir / "chat_replay.log"
    lines = source.read_text(encoding="utf-8").splitlines(keepends=True)
    _stream_ticks(lines, chatlog_path)


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
