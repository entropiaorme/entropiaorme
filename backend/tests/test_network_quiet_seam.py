"""Network-quiet seam: the consolidated snapshot plus the SSE push are enough
for a hydrate-and-subscribe client to stay current without polling.

This is the backend, no-frontend-in-the-loop half of the dashboard's
poll-removal acceptance. It models the dashboard's data flow against the real app
on a loopback uvicorn server wrapped in a request-recording ASGI layer: hydrate
the snapshot once, open the event stream, and on each pushed frame re-read the
snapshot (the store's loop), driving genuine state mutations through the
production producer (``HuntTracker.start_session`` / ``stop_session``).

It asserts three properties together:

- the push fires: a state mutation delivers a typed ``tracking.session.updated``
  frame to the open stream;
- the snapshot is live: the re-read the frame triggers reflects the mutation
  (active after start, idle with an empty feed after stop), so the frame plus one
  snapshot read carry the whole state change with no polling;
- the request signature is network-quiet: across the exchange the server sees
  only the snapshot hydrations and the single event stream, never a read of the
  three collapsed endpoints (``/status``, ``/live``, ``/recent-events``) or the
  un-collapsed ``/sessions``.

The frontend's adherence to this loop is the residual UAT the backend harness
cannot reach; what is mechanised here is that the seam supports it.

A real loopback server is used rather than the in-process ASGI transport for the
same reason as the SSE seam test: ``httpx.ASGITransport`` buffers the whole
response body, so it cannot exercise an unbounded stream.
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import tempfile
import threading
import time
from collections.abc import AsyncIterator, Iterator
from typing import Any

import httpx
import pytest
import uvicorn

import backend.main as main_module
from backend.dependencies import get_services
from backend.main import app

REQUEST_HEADERS = {"Origin": "tauri://localhost"}

# Per-read timeout: a frame arrives within a couple of reads of its trigger, so
# this bound turns a dropped-frame or stream-buffering regression into a fast
# failure rather than a hang.
_READ_TIMEOUT = 5.0
_MAX_LINES = 50
_STARTUP_TIMEOUT = 15.0

_COLLAPSED_ENDPOINTS = {
    "/api/tracking/status",
    "/api/tracking/live",
    "/api/tracking/recent-events",
}


class _RequestLog:
    """Records the (method, path) of every HTTP request the server handles."""

    def __init__(self) -> None:
        self.requests: list[tuple[str, str]] = []

    def get_paths(self, path: str) -> list[tuple[str, str]]:
        return [(m, p) for (m, p) in self.requests if m == "GET" and p == path]


def _recording_asgi(inner: Any, log: _RequestLog) -> Any:
    """Wrap an ASGI app, recording each HTTP request before delegating.

    Only ``http`` scopes are logged; ``lifespan`` (and any other) scopes pass
    straight through, so the production startup the inner app performs on boot is
    unchanged.
    """

    async def wrapped(scope: dict[str, Any], receive: Any, send: Any) -> None:
        if scope["type"] == "http":
            log.requests.append((scope["method"], scope["path"]))
        await inner(scope, receive, send)

    return wrapped


@pytest.fixture
def recording_live_server() -> Iterator[tuple[int, _RequestLog]]:
    """Run the real app on a loopback uvicorn server behind a request recorder.

    Mirrors the SSE seam fixture (kernel-assigned ephemeral port, host-allow-list
    management, temporary data dir, production lifespan) with one addition: the
    app is wrapped in a recording ASGI layer so the test can assert the exact
    request signature the server saw.
    """
    data_dir = tempfile.mkdtemp(prefix="eo_netquiet_seam_")
    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = data_dir

    log = _RequestLog()
    config = uvicorn.Config(
        _recording_asgi(app, log),
        host="127.0.0.1",
        port=0,
        log_level="warning",
        lifespan="on",
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

        # Read the port uvicorn actually bound (port=0 -> kernel-assigned).
        port = server.servers[0].sockets[0].getsockname()[1]
        added_hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
        main_module.ALLOWED_API_HOSTS |= added_hosts

        yield port, log
    finally:
        server.should_exit = True
        thread.join(timeout=10)
        main_module.ALLOWED_API_HOSTS -= added_hosts
        shutil.rmtree(data_dir, ignore_errors=True)
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir


async def _next_frame(lines: AsyncIterator[str]) -> dict[str, Any]:
    """Read the stream until the next data frame; return its topic and envelope.

    Comment lines (heartbeats, the opening ``: ready``) and blanks are skipped.
    """
    topic: str | None = None
    for _ in range(_MAX_LINES):
        raw = await asyncio.wait_for(anext(lines), _READ_TIMEOUT)
        if raw.startswith("event:"):
            topic = raw[len("event:") :].strip()
        elif raw.startswith("data:"):
            payload = json.loads(raw[len("data:") :].strip())
            return {"topic": topic, "envelope": payload}
    raise AssertionError("no data frame arrived within the line budget")


async def _drive_dashboard_flow(port: int) -> None:
    """Model the dashboard's hydrate-and-subscribe loop against the live server."""
    base_url = f"http://127.0.0.1:{port}"
    async with httpx.AsyncClient(
        base_url=base_url, timeout=10.0, headers=REQUEST_HEADERS
    ) as client:
        # Hydrate on mount: one snapshot read, idle to begin with.
        first = await client.get("/api/tracking/snapshot")
        assert first.status_code == 200
        assert first.json()["status"] == "idle"

        # Subscribe to the push stream.
        async with client.stream("GET", "/api/events") as response:
            assert response.status_code == 200
            assert response.headers["content-type"].startswith("text/event-stream")
            lines = response.aiter_lines()
            # The opening comment flushes only after the connection registers with
            # the hub, so the event triggered next is not raced.
            assert await asyncio.wait_for(anext(lines), _READ_TIMEOUT) == ": ready"

            # Mutate (start): the push delivers a started frame...
            get_services().tracker.start_session()
            started = await _next_frame(lines)
            assert started["topic"] == "tracking.session.updated"
            assert started["envelope"]["payload"]["status"] == "active"
            assert started["envelope"]["payload"]["reason"] == "started"

            # ...and the snapshot re-read the frame triggers reflects it, so no
            # poll is needed to learn the session went active.
            active = await client.get("/api/tracking/snapshot")
            assert active.status_code == 200
            assert active.json()["status"] == "active"

            # Mutate (stop): a stopped frame, then an idle snapshot with the
            # activity feed cleared.
            get_services().tracker.stop_session()
            stopped = await _next_frame(lines)
            assert stopped["topic"] == "tracking.session.updated"
            assert stopped["envelope"]["payload"]["reason"] == "stopped"

            idle = await client.get("/api/tracking/snapshot")
            assert idle.status_code == 200
            idle_body = idle.json()
            assert idle_body["status"] == "idle"
            assert idle_body["recentEvents"] == []


def test_dashboard_data_flow_reads_no_collapsed_endpoint(
    recording_live_server: tuple[int, _RequestLog],
) -> None:
    """A hydrate-and-subscribe client stays current on the snapshot plus the push
    alone: the server sees the snapshot hydrations and the single event stream,
    and never a read of the collapsed status / live / recent-events endpoints."""
    port, log = recording_live_server
    asyncio.run(_drive_dashboard_flow(port))

    snapshot_reads = log.get_paths("/api/tracking/snapshot")
    event_streams = log.get_paths("/api/events")
    collapsed = [(m, p) for (m, p) in log.requests if p in _COLLAPSED_ENDPOINTS]
    sessions = log.get_paths("/api/tracking/sessions")

    # One stream, opened once.
    assert len(event_streams) == 1, log.requests
    # Exactly the hydrations the loop issues: one on mount, one per pushed frame
    # (start, stop).
    assert len(snapshot_reads) == 3, log.requests
    # The collapsed trio is never read, and neither is the un-collapsed sessions
    # endpoint the dashboard no longer fetches.
    assert collapsed == [], f"collapsed endpoints must not be read: {collapsed}"
    assert sessions == [], (
        f"sessions must not be read by the dashboard flow: {sessions}"
    )
