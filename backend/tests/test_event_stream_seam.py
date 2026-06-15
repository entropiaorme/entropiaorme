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
import shutil
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


@pytest.fixture
def live_server(tmp_path) -> Iterator[int]:
    """Run the real app on a loopback uvicorn server for one test.

    Uvicorn binds an ephemeral port itself (``port=0``) and the actual port is
    read back from the running server's socket after startup, so there is no
    bind-then-release window another process could steal. The host/origin guard
    allows only hosts derived from the configured backend port, so the bound port
    is added to the allow-list for the server's lifetime. The lifespan (and thus
    the event bus, hub, and tracker) is wired by uvicorn itself, so the seam runs
    against production startup.
    """
    data_dir = str(tmp_path)
    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = data_dir

    config = uvicorn.Config(
        app, host="127.0.0.1", port=0, log_level="warning", lifespan="on"
    )
    server = uvicorn.Server(config)
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    added_hosts: set[str] = set()
    try:
        deadline = time.monotonic() + _STARTUP_TIMEOUT
        while not server.started:
            if time.monotonic() > deadline:
                server.should_exit = True
                raise RuntimeError("loopback server did not start in time")
            time.sleep(0.05)

        # Read the port uvicorn actually bound (port=0 -> kernel-assigned), so
        # there is no gap between choosing a port and binding it.
        port = server.servers[0].sockets[0].getsockname()[1]
        added_hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
        main_module.ALLOWED_API_HOSTS |= added_hosts

        yield port
    finally:
        server.should_exit = True
        thread.join(timeout=10)
        main_module.ALLOWED_API_HOSTS -= added_hosts
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir
        # ignore_errors: Windows may briefly hold the SQLite file open
        # past lifespan shutdown via the per-thread connection pool; a
        # leftover temp dir on a stuck handle is preferable to a teardown
        # crash that masks a real test failure.
        shutil.rmtree(data_dir, ignore_errors=True)


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


async def _read_scan_frame(port: int) -> tuple[str | None, str | None]:
    """Open the stream, drive a scan status change, return the first scan frame.

    The real scan verbs gate on the OCR engine and the game window (absent in a
    headless test), so the status change is driven through the producer's owned
    state and outbox directly. The seam under test is that the frame crosses the
    wire; the verb business logic is covered by the producer unit and scan suites.
    """
    base_url = f"http://127.0.0.1:{port}"
    async with (
        httpx.AsyncClient(base_url=base_url, timeout=10.0) as client,
        client.stream("GET", "/api/events", headers=REQUEST_HEADERS) as response,
    ):
        assert response.status_code == 200
        assert response.headers["content-type"].startswith("text/event-stream")

        lines = response.aiter_lines()
        assert await asyncio.wait_for(anext(lines), _READ_TIMEOUT) == ": ready"

        svc = get_services().skill_scan_manual
        try:
            with svc._lock:
                svc._active = True
                svc._captures = [b"page"]
            svc._publish_status()

            topic: str | None = None
            data_payload: str | None = None
            for _ in range(_MAX_LINES):
                raw = await asyncio.wait_for(anext(lines), _READ_TIMEOUT)
                if raw.startswith("event:"):
                    topic = raw[len("event:") :].strip()
                elif raw.startswith("data:"):
                    data_payload = raw[len("data:") :].strip()
                    break
            return topic, data_payload
        finally:
            with svc._lock:
                svc._reset()


def test_scan_status_event_crosses_the_sse_seam(live_server: int) -> None:
    """A scan status change pushes a typed scan.status.changed frame to an open
    SSE stream, proving the spine's second domain topic crosses the same seam as
    the first with the discriminator, version, and phase the wire contract pins."""
    topic, data_payload = asyncio.run(_read_scan_frame(live_server))

    assert topic == "scan.status.changed", (
        f"expected the scan topic on the stream, saw {topic!r}"
    )
    assert data_payload is not None, "no scan data frame arrived on the stream"

    envelope = json.loads(data_payload)
    assert envelope["type"] == "scan.status.changed"
    assert envelope["event_version"] == 1
    assert envelope["occurred_at"] is not None
    assert envelope["payload"]["phase"] == "capturing"
