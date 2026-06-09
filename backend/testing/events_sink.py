"""Test-mode event sink: the full bus publish stream as a JSONL file.

Installed by the composition root only under test mode, the sink taps the
event bus and appends one JSON line per publish, in publish order, to a
runtime output file (``events.jsonl`` in the run's data directory). The
stream is a strict superset of the SSE surface, which forwards only the
domain topics: the sink observes every topic that crosses ``publish``.

Payloads are written in raw wire form (``backend.testing.wire``), not the
normalised fingerprint form: an external comparator runs either language's
normaliser over the captured stream, exactly as the committed raw-capture
fixtures are consumed, so the sink stays decoupled from any in-process
symbol table and cannot move a golden.

Writes happen synchronously on the publisher's thread, under the sink's
own lock, flushed per line. Synchronous writes are what let the watcher's
drain barrier extend to the file: when a drain returns, every
watcher-driven event is already on disk. The lock makes the recorded
order a true linearisation despite publishers spanning several threads,
and keeps concurrent lines from interleaving mid-write.
"""

from __future__ import annotations

import json
import threading
from pathlib import Path
from typing import IO, Any

from backend.core.event_bus import EventBus

from .wire import wire


class EventsJsonlSink:
    """Append every bus publish to a JSONL file, one line per event."""

    def __init__(self, path: str | Path):
        self._path = Path(path)
        self._lock = threading.Lock()
        self._bus: EventBus | None = None
        # Line-buffered text handle; the per-line flush below is what makes
        # progress observable to an external reader mid-run.
        self._file: IO[str] | None = self._path.open(
            "w", encoding="utf-8", newline="\n"
        )

    @property
    def path(self) -> Path:
        return self._path

    def install(self, bus: EventBus) -> None:
        """Attach to the bus as a full-stream tap."""
        self._bus = bus
        bus.add_tap(self._on_publish)

    def close(self) -> None:
        """Detach from the bus and close the file. Idempotent."""
        if self._bus is not None:
            self._bus.remove_tap(self._on_publish)
            self._bus = None
        with self._lock:
            if self._file is not None:
                self._file.close()
                self._file = None

    def _on_publish(self, topic: str, payload: Any) -> None:
        # Serialise outside the lock (pure CPU), write under it so lines from
        # concurrent publisher threads can never interleave mid-line.
        line = json.dumps(
            {"payload": wire(payload), "topic": topic},
            sort_keys=True,
            ensure_ascii=False,
        )
        with self._lock:
            if self._file is None:
                return
            self._file.write(line + "\n")
            self._file.flush()
