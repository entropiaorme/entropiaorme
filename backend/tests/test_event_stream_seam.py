"""End-to-end SSE seam: a domain event crosses GET /api/events backend-side.

This is the acceptance gate for the event spine, and the reason the SSE transport
was chosen over a shell-stdout bridge: because the push channel is itself an HTTP
endpoint, the
whole backend half of the seam is mechanically checkable with no frontend in the
loop. The test runs the real app on a loopback uvicorn server, opens the event
stream with a real ``httpx`` client, triggers a genuine
``tracking.session.updated`` through the production producer
(``HuntTracker.start_session``), and asserts the typed envelope arrives over the
wire with the camelCase payload the frontend contract expects.

A real server is used rather than the in-process ASGI transport on purpose:
``httpx.ASGITransport`` (and the sync ``TestClient`` built on it) buffers the
whole response body before returning, so neither can exercise an unbounded
stream; both deadlock on an SSE endpoint. A loopback server is the only harness
that proves the endpoint actually streams frames incrementally over HTTP, which
is the property the bridge depends on. It also exercises the hub's real
cross-thread hop: the producer publishes on the test thread, the bus callback
hops the frame onto the server's event loop via ``call_soon_threadsafe``, and
the frame streams out over the socket.

Every read is wrapped in ``asyncio.wait_for`` so a regression that drops the
frame (or a middleware change that buffers the stream) fails fast rather than
hanging the suite.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import tempfile
import threading
import time
from collections.abc import Iterator

import httpx
import pytest
import uvicorn

import backend.main as main_module
from backend.dependencies import get_services
from backend.main import app

REQUEST_HEADERS = {"Origin": "tauri://localhost"}

# Per-read timeout. The event fires immediately after the trigger, so a frame
# arrives within a couple of reads; this bound turns a dropped-frame or
# stream-buffering regression into a fast failure rather than a hang.
_READ_TIMEOUT = 5.0
_MAX_LINES = 50
_STARTUP_TIMEOUT = 15.0


def _free_loopback_port() -> int:
    """Reserve and release an ephemeral loopback port for the test server."""
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]
    finally:
        probe.close()


@pytest.fixture
def live_server() -> Iterator[int]:
    """Run the real app on a loopback uvicorn server for one test.

    The host/origin guard allows only hosts derived from the configured backend
    port, so the ephemeral port is added to the allow-list for the server's
    lifetime. The lifespan (and thus the event bus, hub, and tracker) is wired by
    uvicorn itself, so the seam runs against production startup.
    """
    port = _free_loopback_port()
    data_dir = tempfile.mkdtemp(prefix="eo_sse_seam_")

    added_hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
    main_module.ALLOWED_API_HOSTS |= added_hosts
    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = data_dir

    config = uvicorn.Config(
        app, host="127.0.0.1", port=port, log_level="warning", lifespan="on"
    )
    server = uvicorn.Server(config)
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    deadline = time.monotonic() + _STARTUP_TIMEOUT
    while not server.started:
        if time.monotonic() > deadline:
            server.should_exit = True
            raise RuntimeError("loopback server did not start in time")
        time.sleep(0.05)

    try:
        yield port
    finally:
        server.should_exit = True
        thread.join(timeout=10)
        main_module.ALLOWED_API_HOSTS -= added_hosts
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir


async def _read_session_frame(port: int) -> tuple[str | None, str | None, str]:
    """Open the stream, trigger a real session start, return the first frame."""
    base_url = f"http://127.0.0.1:{port}"
    async with (
        httpx.AsyncClient(base_url=base_url, timeout=10.0) as client,
        client.stream("GET", "/api/events", headers=REQUEST_HEADERS) as response,
    ):
        assert response.status_code == 200
        assert response.headers["content-type"].startswith("text/event-stream")
        assert response.headers.get("cache-control") == "no-cache"

        lines = response.aiter_lines()
        # Sync on the opening comment: it only flushes after the connection
        # registers with the hub, so the event triggered next is not raced.
        assert await asyncio.wait_for(anext(lines), _READ_TIMEOUT) == ": ready"

        # Trigger a real domain event through the production producer.
        session = get_services().tracker.start_session()
        try:
            topic: str | None = None
            data_payload: str | None = None
            for _ in range(_MAX_LINES):
                raw = await asyncio.wait_for(anext(lines), _READ_TIMEOUT)
                if raw.startswith("event:"):
                    topic = raw[len("event:") :].strip()
                elif raw.startswith("data:"):
                    data_payload = raw[len("data:") :].strip()
                    break
            return topic, data_payload, session.id
        finally:
            get_services().tracker.stop_session()


def test_session_started_event_crosses_the_sse_seam(live_server: int) -> None:
    """Starting a session pushes a typed tracking.session.updated frame to an
    open SSE stream, with the discriminator, version, and camelCase payload the
    wire contract pins."""
    topic, data_payload, session_id = asyncio.run(_read_session_frame(live_server))

    assert topic == "tracking.session.updated", (
        f"expected the session topic on the stream, saw {topic!r}"
    )
    assert data_payload is not None, "no data frame arrived on the stream"

    envelope = json.loads(data_payload)
    assert envelope["type"] == "tracking.session.updated"
    assert envelope["event_version"] == 1
    assert envelope["occurred_at"] is not None
    payload = envelope["payload"]
    assert payload["status"] == "active"
    assert payload["reason"] == "started"
    # camelCase on the wire, and the id matches the session just started.
    assert payload["sessionId"] == session_id
