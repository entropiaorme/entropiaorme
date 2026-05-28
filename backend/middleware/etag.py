"""Strong-ETag + Cache-Control + conditional-GET middleware.

Applied to GET responses under the hydration prefixes (``/api/tracking``,
``/api/scan``, ``/api/quests``, ``/api/codex``) so the frontend can skip
re-rendering when a polled snapshot has not moved. The value is a
strong ETag (``"<sha256-hex>"``) computed over the serialised response
body, so equal bodies yield equal ETags regardless of the route or the
process that produced them. ``Cache-Control: no-cache`` mandates
revalidation: the body skip is the goal, not network skip.

A request whose ``If-None-Match`` header matches the freshly-computed
ETag is answered with ``304 Not Modified`` and an empty body, carrying
the ETag + Cache-Control headers per RFC 7232 §4.1. Non-matching or
absent ``If-None-Match`` yields the original 2xx response with the
ETag + Cache-Control headers added.

The middleware is intentionally narrow: only successful 2xx GETs under
the four hydration prefixes are touched. Non-GETs, non-2xx, and routes
outside the four prefixes pass through unchanged so the substrate does
not silently widen its remit.
"""

from __future__ import annotations

import hashlib
from collections.abc import AsyncIterable, Awaitable, Callable, Iterable

from fastapi import FastAPI
from fastapi.routing import APIRoute
from starlette.requests import Request
from starlette.responses import Response
from starlette.types import ASGIApp

ETAG_PREFIXES: tuple[str, ...] = (
    "/api/tracking",
    "/api/scan",
    "/api/quests",
    "/api/codex",
)

CACHE_CONTROL_VALUE = "no-cache"


def path_is_in_etag_scope(path: str) -> bool:
    """Whether ``path`` falls under one of the hydration prefixes the
    ETag middleware covers."""
    return any(
        path == prefix or path.startswith(prefix + "/") for prefix in ETAG_PREFIXES
    )


def compute_strong_etag(body: bytes) -> str:
    """Return the strong ETag value (quoted hex) for ``body``."""
    return f'"{hashlib.sha256(body).hexdigest()}"'


def if_none_match_matches(header_value: str | None, current_etag: str) -> bool:
    """Return whether ``If-None-Match`` indicates the client already holds
    the resource representation identified by ``current_etag``.

    Implements RFC 7232 §3.2 + §2.3.2 weak-comparison semantics:

    - An absent or empty header is a non-match.
    - The wildcard ``*`` matches any current representation.
    - Otherwise the header is a comma-separated list of entity-tags
      (``"opaque"`` or ``W/"opaque"``); the request matches if any
      candidate's opaque part equals ``current_etag``'s opaque part.
      Weak/strong prefix is ignored by the weak-comparison function the
      RFC mandates for conditional GETs, so a client sending
      ``W/"<hex>"`` against a server-strong ``"<hex>"`` still gets 304.

    Whitespace between list items and around the ``W/`` prefix is
    tolerated; clients in the wild produce all three of
    ``"a","b"``, ``"a", "b"``, ``W/"a", "b"``.
    """
    if not header_value:
        return False
    stripped = header_value.strip()
    if stripped == "*":
        return True
    current_opaque = current_etag.removeprefix("W/").strip()
    for raw in stripped.split(","):
        candidate = raw.strip()
        if not candidate:
            continue
        candidate_opaque = candidate.removeprefix("W/").strip()
        if candidate_opaque == current_opaque:
            return True
    return False


async def _read_streaming_body(
    iterator: AsyncIterable[str | bytes | memoryview],
) -> bytes:
    """Drain a Starlette ``body_iterator`` into a single ``bytes``.

    Handles the streaming-response case where ``call_next`` yields
    chunks rather than a fully-buffered body. Starlette's iterator
    yields ``str``, ``bytes``, or ``memoryview`` depending on the
    handler shape; all three are folded into a single ``bytes``.
    """
    body = bytearray()
    async for chunk in iterator:
        if isinstance(chunk, str):
            body.extend(chunk.encode("utf-8"))
        else:
            body.extend(chunk)
    return bytes(body)


_HEADERS_TO_DROP = {"content-length", "etag", "cache-control"}


def _filtered_headers(headers: Iterable[tuple[str, str]]) -> dict[str, str]:
    """Copy ``headers`` minus the entries the middleware owns.

    Drops ``Content-Length`` so the rebuilt ``Response`` recomputes it
    from the in-hand body, and drops any handler-set ``ETag`` /
    ``Cache-Control`` so the middleware's values win.
    """
    return {k: v for k, v in headers if k.lower() not in _HEADERS_TO_DROP}


async def etag_dispatch(
    request: Request,
    call_next: Callable[[Request], Awaitable[Response]],
) -> Response:
    """The middleware body: wrap eligible GETs with ETag + 304 handling.

    Eligibility: GET method, request path under one of ``ETAG_PREFIXES``,
    response status 2xx. Anything else is passed through unchanged.
    """
    response = await call_next(request)

    if request.method != "GET" or not path_is_in_etag_scope(request.url.path):
        return response
    if not (200 <= response.status_code < 300):
        return response

    # Starlette's BaseHTTPMiddleware wraps the inner response in a
    # streaming form regardless of the handler's return type, but its
    # concrete class is a private ``_StreamingResponse`` that does not
    # extend ``StreamingResponse`` in every Starlette version. Probe
    # for the ``body_iterator`` attribute (the only thing the
    # middleware needs) so the check stays compatible across versions
    # rather than coupling to the class hierarchy.
    body_iterator = getattr(response, "body_iterator", None)
    if body_iterator is None:
        return response
    body = await _read_streaming_body(body_iterator)
    etag_value = compute_strong_etag(body)
    headers = _filtered_headers(response.headers.items())
    headers["ETag"] = etag_value
    headers["Cache-Control"] = CACHE_CONTROL_VALUE

    if if_none_match_matches(request.headers.get("if-none-match"), etag_value):
        # 304 carries the same validator + cross-cutting headers a 200
        # response would (per RFC 7232 §4.1), so CORS / Vary / Date
        # added by inner middleware remain available to the client.
        # Drop Content-Type since there is no body to describe.
        not_modified_headers = {
            k: v for k, v in headers.items() if k.lower() != "content-type"
        }
        return Response(status_code=304, headers=not_modified_headers)

    return Response(
        content=body,
        status_code=response.status_code,
        headers=headers,
        media_type=response.media_type,
    )


def install_etag_middleware(app: FastAPI) -> None:
    """Wire ``etag_dispatch`` into ``app`` as an HTTP middleware."""
    app.middleware("http")(etag_dispatch)


def covered_get_routes(app: ASGIApp | FastAPI) -> list[str]:
    """Enumerate every GET route whose path lies in the ETag scope.

    Used by the route-coverage test (every hydration-prefix GET must
    have the contract). Sorted for deterministic test output.
    """
    seen: set[str] = set()
    routes = getattr(app, "routes", [])
    for route in routes:
        if not isinstance(route, APIRoute):
            continue
        methods = route.methods or set()
        if "GET" not in {m.upper() for m in methods}:
            continue
        if path_is_in_etag_scope(route.path):
            seen.add(route.path)
    return sorted(seen)
