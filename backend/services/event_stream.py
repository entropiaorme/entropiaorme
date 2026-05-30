"""SSE fan-out hub: the bridge from the in-process bus to webview clients.

The backend's :class:`~backend.core.event_bus.EventBus` carries typed
:mod:`~backend.core.domain_events` envelopes between in-process subscribers.
This hub is the single seam that forwards the coarse, frontend-facing subset of
those events out of the Python process: it subscribes to the domain topics on
the bus and exposes their serialised frames to any number of connected
``GET /api/events`` server-sent-event streams. The relay in the main webview
re-emits each frame onto the Tauri event bus, so every window receives the push
without polling.

The hard part is the thread boundary. ``EventBus.publish`` runs synchronously on
whatever thread mutated state: for the tick-coalesced ``tracking.session.updated``
event that is the chatlog-watcher's OS thread, not the uvicorn event-loop thread
the SSE generators await on. The hub crosses that boundary in one place and one
direction:

- ``_on_domain_event`` runs on the *publisher* thread. It serialises the
  envelope to its wire JSON there (pure CPU, thread-safe) and hops the finished
  frame onto the loop via ``loop.call_soon_threadsafe``.
- ``_dispatch`` runs on the *loop* thread. It assigns the frame sequence number
  and fans the frame out to every connection's queue.

Because the connection registry, the sequence counter, and the per-connection
queues are touched only on the loop thread (``register`` / ``unregister`` are
called from the async endpoint, ``_dispatch`` via ``call_soon_threadsafe``),
they need no lock of their own.

This maps to a Rust ``tokio::sync::broadcast`` channel at port time: the bus
callback is the sender side, each SSE connection holds a receiver, and the
bounded per-connection queue with drop-oldest is exactly broadcast's lag-drop
behaviour for a receiver that falls behind. There is deliberately no app-lifetime
background task here: each connection's drain is owned by its request lifecycle
(the Starlette/uvicorn response), which maps onto an ``axum`` per-connection
response stream rather than a supervised long-lived task.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
from typing import Any

from pydantic import BaseModel

from backend.core.domain_events import TOPIC_TRACKING_SESSION_UPDATED
from backend.core.event_bus import EventBus

log = logging.getLogger(__name__)

# Coarse, frontend-facing domain topics forwarded across the SSE bridge. Each is
# published on the bus as a typed DomainEvent instance (see domain_events.py);
# the legacy low-level EVENT_* topics stay intra-backend and are deliberately
# not forwarded. Further domain topics join this tuple as they are added.
DOMAIN_TOPICS: tuple[str, ...] = (TOPIC_TRACKING_SESSION_UPDATED,)

# Per-connection queue bound. A stalled or slow webview reader cannot grow
# memory without limit: once the queue is full the oldest frame is dropped. Under
# the push-to-pull contract a frame is only an invalidation signal, so a dropped
# frame is self-healing: the next frame the reader does receive triggers a
# snapshot re-hydration that reflects every intervening change.
DEFAULT_MAX_QUEUE = 256


def _offer(queue: asyncio.Queue[str], frame: str) -> None:
    """Enqueue ``frame``, dropping the oldest entry first if the queue is full.

    Drop-oldest (not drop-newest) is correct under push-to-pull: the newest
    frame is the one that triggers the freshest hydration, so it must never be
    the one discarded. Runs only on the event-loop thread, so the full-check then
    put is race-free.
    """
    while queue.full():
        with contextlib.suppress(asyncio.QueueEmpty):
            queue.get_nowait()
    queue.put_nowait(frame)


class EventStreamHub:
    """Fan-out broker from the synchronous EventBus to SSE connections."""

    def __init__(
        self, event_bus: EventBus, *, max_queue: int = DEFAULT_MAX_QUEUE
    ) -> None:
        self._event_bus = event_bus
        self._max_queue = max_queue
        self._loop: asyncio.AbstractEventLoop | None = None
        self._connections: set[asyncio.Queue[str]] = set()
        self._seq = 0
        self._subscribed = False

    def bind_loop(self, loop: asyncio.AbstractEventLoop) -> None:
        """Capture the event loop the SSE generators run on and subscribe to the
        domain topics. Called once from the app lifespan, which runs on that
        loop. Idempotent so a re-entered lifespan (test reuse) does not
        double-subscribe.
        """
        self._loop = loop
        if not self._subscribed:
            for topic in DOMAIN_TOPICS:
                self._event_bus.subscribe(topic, self._on_domain_event)
            self._subscribed = True

    def close(self) -> None:
        """Unsubscribe from the bus and drop all connections.

        Called on app shutdown so a late publish cannot hop a frame onto a
        closing loop, and so the bus does not retain a reference to this hub.
        """
        if self._subscribed:
            for topic in DOMAIN_TOPICS:
                self._event_bus.unsubscribe(topic, self._on_domain_event)
            self._subscribed = False
        self._connections.clear()
        self._loop = None

    # ── Connection registry (loop thread only) ──

    def register(self) -> asyncio.Queue[str]:
        """Create and track a bounded queue for one SSE connection."""
        queue: asyncio.Queue[str] = asyncio.Queue(maxsize=self._max_queue)
        self._connections.add(queue)
        return queue

    def unregister(self, queue: asyncio.Queue[str]) -> None:
        """Stop tracking a connection's queue (on disconnect)."""
        self._connections.discard(queue)

    @property
    def connection_count(self) -> int:
        """Number of live SSE connections (observability + tests)."""
        return len(self._connections)

    # ── Bus → loop hop ──

    def _on_domain_event(self, envelope: Any) -> None:
        """EventBus subscriber: runs on the publisher thread.

        Serialises the typed envelope to its wire JSON here, then schedules the
        fan-out on the loop. A non-model payload on a domain topic would be a
        programming error upstream (the domain topics carry typed instances by
        construction), so it is logged and dropped rather than forwarded as an
        untyped frame.
        """
        if not isinstance(envelope, BaseModel):
            log.error(
                "Domain topic carried a non-model payload (%s); dropped",
                type(envelope).__name__,
            )
            return
        topic = getattr(envelope, "type", None)
        if not isinstance(topic, str):
            log.error("Domain envelope lacks a string 'type' discriminator; dropped")
            return
        data_json = envelope.model_dump_json()
        loop = self._loop
        if loop is None:
            return
        # A closed loop (shutdown race) raises RuntimeError; the frame is then
        # dropped, which is safe under push-to-pull.
        with contextlib.suppress(RuntimeError):
            loop.call_soon_threadsafe(self._dispatch, topic, data_json)

    def _dispatch(self, topic: str, data_json: str) -> None:
        """Build the SSE frame and fan it out to every connection.

        Runs only on the loop thread. The sequence id is monotonic across the
        process so a client can reason about gaps; the ``event`` field carries
        the domain topic so the relay can route without parsing the body.
        """
        self._seq += 1
        frame = f"id: {self._seq}\nevent: {topic}\ndata: {data_json}\n\n"
        for queue in self._connections:
            _offer(queue, frame)
