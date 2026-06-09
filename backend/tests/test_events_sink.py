"""Tests for the test-mode events.jsonl bus sink.

Pin the contract the external comparator relies on: one JSON line per
publish in publish order across every topic, payloads reduced to wire
form, each line flushed as it is written (progress is observable
mid-run), no interleaving under concurrent publishers, and a closed
sink detaches cleanly.
"""

import json
import threading
from datetime import datetime

from pydantic import BaseModel

from backend.core.event_bus import EventBus
from backend.testing.events_sink import EventsJsonlSink


def _lines(path) -> list[dict]:
    text = path.read_text(encoding="utf-8")
    return [json.loads(line) for line in text.splitlines()]


class _Envelope(BaseModel):
    kind: str
    occurred_at: datetime


def test_one_line_per_publish_in_order_across_topics(tmp_path):
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    try:
        # No subscriber on either topic: the sink must still observe both.
        bus.publish("combat", {"damage": 12.5})
        bus.publish("loot_group", {"total_ped": 0.12})
    finally:
        sink.close()

    assert _lines(sink.path) == [
        {"topic": "combat", "payload": {"damage": 12.5}},
        {"topic": "loot_group", "payload": {"total_ped": 0.12}},
    ]


def test_payloads_reduce_to_wire_form(tmp_path):
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    try:
        instant = datetime(2026, 3, 27, 10, 0, 0)
        bus.publish("tick_flushed", {"timestamp": instant})
        bus.publish(
            "tracking.session.updated",
            _Envelope(kind="session", occurred_at=instant),
        )
    finally:
        sink.close()

    lines = _lines(sink.path)
    assert lines[0]["payload"] == {"timestamp": "2026-03-27T10:00:00"}
    assert lines[1]["payload"] == {
        "kind": "session",
        "occurred_at": "2026-03-27T10:00:00",
    }


def test_lines_are_canonical_json(tmp_path):
    """Keys are sorted and non-ASCII text is written raw, not escaped."""
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    try:
        bus.publish("loot_group", {"item_name": "Lysté", "amount": 1})
    finally:
        sink.close()

    raw = sink.path.read_text(encoding="utf-8")
    assert raw == (
        '{"payload": {"amount": 1, "item_name": "Lysté"}, "topic": "loot_group"}\n'
    )


def test_each_line_is_flushed_as_written(tmp_path):
    """An external reader sees every published line before the sink closes."""
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    try:
        bus.publish("combat", {"n": 1})
        # Read through a separate handle while the sink is still open.
        assert _lines(sink.path) == [{"topic": "combat", "payload": {"n": 1}}]
    finally:
        sink.close()


def test_concurrent_publishers_do_not_interleave(tmp_path):
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    per_thread = 50

    def _publish(worker: int) -> None:
        for i in range(per_thread):
            bus.publish("combat", {"worker": worker, "i": i})

    threads = [threading.Thread(target=_publish, args=(w,)) for w in range(4)]
    try:
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
    finally:
        sink.close()

    lines = _lines(sink.path)  # raises on any torn/interleaved line
    assert len(lines) == 4 * per_thread
    # Per-thread order is preserved within the global linearisation.
    for worker in range(4):
        own = [ln["payload"]["i"] for ln in lines if ln["payload"]["worker"] == worker]
        assert own == list(range(per_thread))


def test_close_detaches_and_is_idempotent(tmp_path):
    bus = EventBus()
    sink = EventsJsonlSink(tmp_path / "events.jsonl")
    sink.install(bus)
    bus.publish("combat", {"n": 1})

    sink.close()
    bus.publish("combat", {"n": 2})
    sink.close()

    assert _lines(sink.path) == [{"topic": "combat", "payload": {"n": 1}}]
