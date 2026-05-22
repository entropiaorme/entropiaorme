"""Event-stream fingerprint emitter and shared normaliser.

The fingerprint subscribes to every event published on a test bus,
applies stable normalisation (UUIDs to sequential ``<UUID_N>`` symbols,
timestamps to sequential ``<TS_N>`` tokens, floats rounded to 4 dp,
dict keys lexically sorted), and serialises the result as JSONL: one
line per event in publish order. The normaliser is shared with
``db_snapshot.capture`` so a UUID seen first on the bus and then in a
DB column resolves to the same symbol across both surfaces, which
keeps the diff output cross-referenceable.

The format is intentionally language-agnostic. A future Rust backend
publishing the same logical event sequence yields byte-identical
fingerprints, which is the "Python as oracle" A/B story
``rust-backend-migration`` rides on top of.
"""

from __future__ import annotations

import json
import re
from datetime import datetime
from pathlib import Path
from typing import Any, Callable

from backend.core.event_bus import EventBus

UUID_PATTERN = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
ISO_PATTERN = re.compile(
    r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}"
)
# Heuristic epoch-second window for treating bare floats as timestamps:
# spans 2001-09 through year ~2603, well clear of any plausible
# monetary or counter value the harness scenarios produce.
EPOCH_MIN = 1_000_000_000.0
EPOCH_MAX = 20_000_000_000.0

FLOAT_PRECISION = 4


class Normalizer:
    """Stable canonicalisation shared across fingerprint and DB snapshot.

    The same UUID or timestamp string maps to the same symbol regardless
    of which surface (bus payload or DB column) surfaces it first. The
    symbol tables reset per-scenario so symbols start from 1 each run;
    that keeps goldens portable across machines and detached from any
    process-global UUID counter.
    """

    def __init__(self) -> None:
        self._uuids: dict[str, str] = {}
        self._timestamps: dict[Any, str] = {}

    def normalize(self, value: Any) -> Any:
        """Return the canonical form of ``value`` (recursive walk)."""
        return self._walk(value)

    def reset(self) -> None:
        """Drop all symbol assignments; the next normalise call starts
        from ``<UUID_1>`` / ``<TS_1>`` again."""
        self._uuids.clear()
        self._timestamps.clear()

    def _walk(self, value: Any) -> Any:
        if value is None or isinstance(value, bool):
            return value
        if isinstance(value, datetime):
            return self._symbol_for_timestamp(value.isoformat())
        if isinstance(value, str):
            if UUID_PATTERN.match(value):
                return self._symbol_for_uuid(value)
            if ISO_PATTERN.match(value):
                return self._symbol_for_timestamp(value)
            return value
        if isinstance(value, int):
            return value
        if isinstance(value, float):
            if EPOCH_MIN <= value <= EPOCH_MAX:
                return self._symbol_for_timestamp(value)
            return round(value, FLOAT_PRECISION)
        if isinstance(value, dict):
            return {key: self._walk(value[key]) for key in sorted(value.keys())}
        if isinstance(value, (list, tuple)):
            return [self._walk(item) for item in value]
        # Unknown types fall back to repr so the fingerprint stays
        # stringifiable; surfaces a clear marker on any drift toward an
        # unhandled value shape rather than silently losing it.
        return repr(value)

    def _symbol_for_uuid(self, value: str) -> str:
        if value not in self._uuids:
            self._uuids[value] = f"<UUID_{len(self._uuids) + 1}>"
        return self._uuids[value]

    def _symbol_for_timestamp(self, value: Any) -> str:
        if value not in self._timestamps:
            self._timestamps[value] = f"<TS_{len(self._timestamps) + 1}>"
        return self._timestamps[value]


class FingerprintRecorder:
    """Captures every event published on a bus by wrapping ``publish``.

    Production handlers keep using the bus as normal: the wrapper
    records the ``(topic, payload)`` pair, then forwards to the
    original ``publish`` so subscribers run unchanged. The recorder
    stores raw payloads and defers normalisation to ``serialize`` so a
    test that wants to inspect the live event list can do so against
    the original objects.
    """

    def __init__(self, normalizer: Normalizer) -> None:
        self._normalizer = normalizer
        self._events: list[tuple[str, Any]] = []
        self._original_publish: Callable[..., None] | None = None
        self._bus: EventBus | None = None

    def install(self, bus: EventBus) -> None:
        """Attach the recorder to ``bus`` by shadowing its publish
        method. Idempotent: re-installing on the same bus is a no-op."""
        if self._bus is bus and self._original_publish is not None:
            return
        self._bus = bus
        self._original_publish = bus.publish

        def wrapped(event_type: str, data: Any = None) -> None:
            self._events.append((event_type, data))
            assert self._original_publish is not None
            self._original_publish(event_type, data)

        bus.publish = wrapped  # type: ignore[method-assign]

    def uninstall(self) -> None:
        """Restore the bus's original publish method."""
        if self._bus is not None and self._original_publish is not None:
            self._bus.publish = self._original_publish  # type: ignore[method-assign]
        self._bus = None
        self._original_publish = None

    @property
    def events(self) -> list[tuple[str, Any]]:
        """Return a copy of the raw recorded events."""
        return list(self._events)

    def serialize(self) -> str:
        """Render the recorded events as canonical JSONL.

        Each line is ``{"topic": <event_type>, "payload": <normalised>}``
        with keys sorted; events appear in publish order. The trailing
        newline is present whenever any events were recorded so the
        file ends predictably for tooling that walks line counts.
        """
        lines: list[str] = []
        for topic, payload in self._events:
            entry = {
                "topic": topic,
                "payload": self._normalizer.normalize(payload),
            }
            lines.append(json.dumps(entry, sort_keys=True, ensure_ascii=False))
        if not lines:
            return ""
        return "\n".join(lines) + "\n"

    def write(self, path: Path) -> None:
        """Persist the serialised fingerprint to ``path`` (parents
        created on demand)."""
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.serialize(), encoding="utf-8")
