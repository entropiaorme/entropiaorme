"""Server-sent-events endpoint: the backend to webview push channel.

``GET /api/events`` is a long-lived ``text/event-stream`` carrying the coarse,
frontend-facing domain events (see :mod:`backend.core.domain_events`). The main
webview opens it once and relays each frame onto the Tauri event bus, replacing
the per-window HTTP polling that earlier discovered backend state changes.

The endpoint sits OUTSIDE the four ETag hydration prefixes by design: the ETag
middleware buffers a whole response body to hash it, which would never return on
an unbounded stream. It is also excluded from the OpenAPI schema: an infinite
event stream is not a request/response operation the spec (or the schemathesis
contract walk, or the generated TS client) can model. The stream contract is
documented in prose in the architecture note instead
(``backend/architecture/README.md``).
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

from fastapi import APIRouter, Request
from starlette.responses import StreamingResponse

from backend.dependencies import get_services

router = APIRouter(prefix="/events", tags=["events"])

# Keep-alive cadence. A comment frame on a quiet stream stops the browser
# EventSource and any intermediary from treating the connection as dead, without
# coupling the cadence to any domain-event rate.
KEEPALIVE_SECONDS = 15.0

# Cache-Control: no-cache stops any intermediary caching the stream;
# X-Accel-Buffering: no disables reverse-proxy buffering so frames flush
# immediately. Content type is set via the response media_type.
SSE_HEADERS = {
    "Cache-Control": "no-cache",
    "Connection": "keep-alive",
    "X-Accel-Buffering": "no",
}


@router.get("", include_in_schema=False)
async def events_stream(request: Request) -> StreamingResponse:
    """Open a long-lived SSE stream of frontend-facing domain events.

    Each frame is an invalidation signal (push-to-pull): the typed envelope
    names which domain surface changed and the webview re-hydrates via the
    matching snapshot GET. The connection registers its queue with the hub
    before the response begins streaming, so an event published the instant
    after the stream opens is delivered rather than raced away.
    """
    hub = get_services().event_stream_hub
    queue = hub.register()

    async def frames() -> AsyncIterator[str]:
        try:
            # Opening comment: flushes the response headers and signals that the
            # connection is registered (the seam test syncs on this line before
            # triggering an event).
            yield ": ready\n\n"
            while True:
                try:
                    frame = await asyncio.wait_for(queue.get(), KEEPALIVE_SECONDS)
                except TimeoutError:
                    if await request.is_disconnected():
                        break
                    yield ": keep-alive\n\n"
                    continue
                yield frame
                if await request.is_disconnected():
                    break
        finally:
            hub.unregister(queue)

    return StreamingResponse(
        frames(), media_type="text/event-stream", headers=SSE_HEADERS
    )
