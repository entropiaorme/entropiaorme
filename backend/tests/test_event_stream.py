"""Unit coverage for the SSE fan-out hub.

Exercises the hub's mechanics in isolation (no HTTP, no lifespan): the bus to
loop hop serialises a typed envelope to its camelCase wire form, fan-out reaches
every connection, a slow connection's queue drops the oldest frame under
overflow rather than growing without bound, and unregistering a connection stops
it receiving further frames. The end-to-end path across a real ``GET /api/events``
stream is covered by ``test_event_stream_seam.py``.

The async scenarios are driven through ``asyncio.run`` so the suite needs no
async-test plugin: the hub binds the loop ``asyncio.run`` creates, and a single
``await asyncio.sleep(0)`` lets the ``call_soon_threadsafe`` callbacks scheduled
by ``bus.publish`` drain before the queue is inspected.
"""

from __future__ import annotations

import asyncio
import json
import threading

import pytest

from backend.core.domain_events import (
    TOPIC_TRACKING_SESSION_UPDATED,
    TrackingSessionUpdated,
    TrackingSessionUpdatedPayload,
)
from backend.core.event_bus import EventBus
from backend.services.event_stream import DEFAULT_MAX_QUEUE, EventStreamHub


def _session_event(session_id: str) -> TrackingSessionUpdated:
    return TrackingSessionUpdated(
        occurred_at="2026-01-01T00:00:00+00:00",
        payload=TrackingSessionUpdatedPayload(
            sessionId=session_id, status="active", reason="started"
        ),
    )


def test_publish_serialises_and_fans_out_to_every_connection() -> None:
    """A typed envelope published on the bus reaches every connection as a
    well-formed SSE frame carrying the camelCase wire JSON."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus)
        hub.bind_loop(asyncio.get_running_loop())
        q1 = hub.register()
        q2 = hub.register()
        assert hub.connection_count == 2

        bus.publish(TOPIC_TRACKING_SESSION_UPDATED, _session_event("sess-1"))
        # Wait for the cross-thread delivery rather than relying on a bare
        # sleep(0) as the synchronisation barrier.
        frame1 = await asyncio.wait_for(q1.get(), 1.0)
        frame2 = await asyncio.wait_for(q2.get(), 1.0)
        assert frame1 == frame2, "both connections must receive the identical frame"

        # SSE frame shape: id, event topic, data, terminating blank line.
        lines = frame1.split("\n")
        assert lines[0].startswith("id: ")
        assert lines[1] == f"event: {TOPIC_TRACKING_SESSION_UPDATED}"
        assert lines[2].startswith("data: ")
        assert frame1.endswith("\n\n")

        envelope = json.loads(lines[2][len("data: ") :])
        assert envelope["type"] == TOPIC_TRACKING_SESSION_UPDATED
        assert envelope["event_version"] == 1
        # camelCase on the wire: the payload key is sessionId, not session_id.
        assert envelope["payload"]["sessionId"] == "sess-1"
        assert envelope["payload"]["status"] == "active"
        assert envelope["payload"]["reason"] == "started"

        hub.close()

    asyncio.run(scenario())


def test_publish_from_another_thread_reaches_the_loop() -> None:
    """A publish on a non-loop thread (the production case: the chatlog-watcher
    OS thread) hops onto the loop via call_soon_threadsafe and reaches the
    connection. This is the hub's load-bearing thread-boundary mechanic."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus)
        hub.bind_loop(asyncio.get_running_loop())
        queue = hub.register()

        # Publish from a separate OS thread, not the loop thread.
        worker = threading.Thread(
            target=bus.publish,
            args=(TOPIC_TRACKING_SESSION_UPDATED, _session_event("from-thread")),
        )
        worker.start()
        worker.join()

        # The frame was scheduled cross-thread; awaiting the queue lets the
        # call_soon_threadsafe callback run and deliver it.
        frame = await asyncio.wait_for(queue.get(), 2.0)
        assert '"sessionId":"from-thread"' in frame

        hub.close()

    asyncio.run(scenario())


def test_slow_connection_drops_oldest_frame_under_overflow() -> None:
    """A connection whose queue fills keeps the newest frames and drops the
    oldest, so a stalled reader cannot grow memory without bound."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus, max_queue=2)
        queue = hub.register()

        # Dispatch four frames without anyone draining; the queue caps at 2.
        for index in range(4):
            hub._dispatch(TOPIC_TRACKING_SESSION_UPDATED, json.dumps({"n": index}))

        assert queue.qsize() == 2
        kept = [json.loads(queue.get_nowait().split("\n")[2][len("data: ") :])["n"]]
        kept.append(json.loads(queue.get_nowait().split("\n")[2][len("data: ") :])["n"])
        # The two newest (2, 3) survive; the two oldest (0, 1) were dropped.
        assert kept == [2, 3]

        hub.close()

    asyncio.run(scenario())


def test_unregistered_connection_receives_no_further_frames() -> None:
    """After unregister, a connection's queue is no longer a fan-out target."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus)
        hub.bind_loop(asyncio.get_running_loop())
        queue = hub.register()
        hub.unregister(queue)
        assert hub.connection_count == 0

        bus.publish(TOPIC_TRACKING_SESSION_UPDATED, _session_event("sess-2"))
        # An unregistered connection must receive nothing: assert the queue
        # stays empty across a real wait window, not just one event-loop tick.
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(queue.get(), 0.1)

        hub.close()

    asyncio.run(scenario())


def test_non_model_payload_on_domain_topic_is_dropped_not_forwarded() -> None:
    """A bare dict on a domain topic is a producer bug; the hub logs and drops
    it rather than forwarding an untyped frame."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus)
        hub.bind_loop(asyncio.get_running_loop())
        queue = hub.register()

        bus.publish(TOPIC_TRACKING_SESSION_UPDATED, {"not": "a model"})
        # A non-model payload must be dropped: nothing arrives within the window.
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(queue.get(), 0.1)

        hub.close()

    asyncio.run(scenario())


def test_close_unsubscribes_from_the_bus() -> None:
    """After close, the bus no longer holds the hub as a subscriber, so a later
    publish does not reach it."""

    async def scenario() -> None:
        bus = EventBus()
        hub = EventStreamHub(bus)
        hub.bind_loop(asyncio.get_running_loop())
        assert bus.has_subscribers(TOPIC_TRACKING_SESSION_UPDATED)

        hub.close()
        assert not bus.has_subscribers(TOPIC_TRACKING_SESSION_UPDATED)

    asyncio.run(scenario())


def test_default_queue_bound_is_positive() -> None:
    """Sanity guard on the module-level default so a refactor cannot zero it."""
    assert DEFAULT_MAX_QUEUE > 0
