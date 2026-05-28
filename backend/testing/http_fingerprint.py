"""HTTP request/response fingerprint emitter for scripted scenarios.

For each endpoint a scenario exercises, the fingerprinter records a
canonical (request, normalised-response) pair and asserts it against
a per-endpoint golden file under
``<scenario>/expected/http_responses/<endpoint_id>.json``. The
``--update-fingerprints`` pytest flag (already registered by the
backend-root conftest) flips the workflow into write mode for
deliberate golden ratification.

The fingerprint reuses the shared ``Normalizer`` so a UUID or
timestamp the bus surfaced first resolves to the same symbol when it later
appears in an HTTP response body; that keeps the HTTP goldens
cross-referenceable with the per-scenario fingerprint.jsonl + db_state.json.

The response header projection is intentionally narrow: only the
substrate's own headers (``Content-Type``, ``Cache-Control``,
``ETag``) are pinned. ETag is recorded as a presence-and-shape
sentinel (``"<STRONG_ETAG>"``) rather than its literal hex, because
the hex hashes a body that contains UUIDs and timestamps which
change per run; the body's own normalisation pins identity, and the
header pins the substrate is engaged.

Binary bodies (e.g. ``image/png`` from ``/api/scan/skills/capture/{page}``)
are projected as ``{"_binary": true, "byte_length": N}``; pinning byte
length lets the goldens catch a content-shape regression without
embedding the raw bytes.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from backend.testing.fingerprint import Normalizer

STRONG_ETAG_RE = re.compile(r'^"[0-9a-f]{64}"$')

# Inline UUID pattern (anchored to a path segment); the ``UUID_PATTERN``
# imported above is the full-string match used by the body walker.
_UUID_IN_PATH_RE = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
)

# The header projection: only these are written into the golden.
# Anything else (Server, Date, CORS) varies across environments and
# would make the goldens noisy without strengthening the contract.
PROJECTED_HEADERS: tuple[str, ...] = (
    "content-type",
    "cache-control",
    "etag",
)

ETAG_SENTINEL = "<STRONG_ETAG>"

# A tracking session's ``duration`` is ``ended_at - started_at`` in whole
# seconds, both captured from wall-clock at session start / stop, so its value
# reflects how long the test happened to run (its replay-drain latency) rather
# than any contract the response should pin. The sibling ``startTime`` /
# ``endTime`` are already symbolised by the shared Normalizer; this projects
# the derived delta the same way so the golden stays deterministic regardless
# of how quickly the watcher drained the scenario.
SESSION_DURATION_SENTINEL = "<SESSION_DURATION>"


@dataclass(frozen=True)
class HttpRequest:
    """The request side of a captured (request, response) pair."""

    method: str
    path: str
    query: dict[str, Any] = field(default_factory=dict)
    headers: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class HttpResponse:
    """The response side of a captured pair, post-normalisation."""

    status_code: int
    headers: dict[str, str]
    body: Any


class HttpFingerprintMismatch(AssertionError):
    """Raised when a captured fingerprint diverges from its golden.

    Carries the endpoint_id and a diff-ready expected/actual pair so a
    downstream renderer can surface the divergence without re-parsing
    the formatted message.
    """

    def __init__(self, endpoint_id: str, expected: Any, actual: Any) -> None:
        self.endpoint_id = endpoint_id
        self.expected = expected
        self.actual = actual
        expected_text = json.dumps(
            expected, indent=2, sort_keys=True, ensure_ascii=False
        )
        actual_text = json.dumps(actual, indent=2, sort_keys=True, ensure_ascii=False)
        super().__init__(
            f"HTTP fingerprint mismatch for endpoint {endpoint_id!r}.\n\n"
            f"--- expected (golden) ---\n{expected_text}\n\n"
            f"--- actual (this run) ---\n{actual_text}\n\n"
            "Rerun with `pytest --update-fingerprints` (and review the diff) "
            "if the new output is the intended new golden."
        )


def normalise_etag(value: str | None) -> str | None:
    """Project an ETag header into the golden's canonical form.

    A strong-format ETag (``"<sha256-hex>"``) is replaced by the
    sentinel ``"<STRONG_ETAG>"``; that pins the substrate is engaged
    without binding the golden to the hex of a body that contains
    per-run UUIDs and timestamps. Any other shape (a weak ETag, an
    empty string, or a malformed value) is kept verbatim so an
    unexpected shape surfaces as a divergence rather than silently
    coercing into the sentinel.
    """
    if value is None:
        return None
    if STRONG_ETAG_RE.match(value):
        return ETAG_SENTINEL
    return value


def project_headers(raw_headers: dict[str, str]) -> dict[str, str]:
    """Filter ``raw_headers`` down to the projection the golden tracks."""
    lowered = {k.lower(): v for k, v in raw_headers.items()}
    projected: dict[str, str] = {}
    for name in PROJECTED_HEADERS:
        if name not in lowered:
            continue
        value = lowered[name]
        if name == "etag":
            etag_projection = normalise_etag(value)
            if etag_projection is not None:
                projected[name] = etag_projection
        else:
            projected[name] = value
    return projected


def normalise_path(path: str, normalizer: Normalizer) -> str:
    """Replace each UUID segment in ``path`` with its ``<UUID_N>`` symbol.

    Path-side normalisation must share the body's symbol table so the
    same session UUID surfacing in ``/api/tracking/session/<id>`` and
    in the response body's ``"sessionId"`` resolves to the same symbol.
    """

    def _replace(match: re.Match[str]) -> str:
        result = normalizer.normalize(match.group(0))
        return result if isinstance(result, str) else match.group(0)

    return _UUID_IN_PATH_RE.sub(_replace, path)


def normalise_body(
    raw_body: bytes,
    content_type: str | None,
    normalizer: Normalizer,
) -> Any:
    """Render the response body into the golden's canonical form.

    JSON bodies are parsed and walked through the shared
    ``Normalizer`` so UUIDs / timestamps land on the same symbols
    the per-scenario fingerprint already uses. Empty bodies are emitted as
    ``null``. Anything else is treated as binary and projected as
    ``{"_binary": true, "byte_length": N}`` so the byte-shape is
    pinned without embedding the raw bytes (which would defeat the
    UUID / timestamp normalisation rationale for everything else).
    """
    if not raw_body:
        return None
    if content_type and "application/json" in content_type.lower():
        decoded = json.loads(raw_body.decode("utf-8"))
        return _project_session_duration(normalizer.normalize(decoded))
    return {"_binary": True, "byte_length": len(raw_body)}


def _project_session_duration(value: Any) -> Any:
    """Symbolise the wall-clock session ``duration`` to a stable sentinel.

    Walks the normalised body and replaces any numeric ``duration`` with
    ``SESSION_DURATION_SENTINEL``. Across the hydration surface the goldens
    capture, ``duration`` is only ever a tracking session's wall-clock length
    (in the sessions list and in session detail's summary), so a value-level
    pin would encode replay-drain latency rather than a contract; the sentinel
    keeps the presence-and-shape assertion without the non-determinism. A
    future endpoint that emits a genuinely deterministic ``duration`` would
    surface the sentinel here and can refine this projection then.
    """
    if isinstance(value, dict):
        projected = {k: _project_session_duration(v) for k, v in value.items()}
        if isinstance(projected.get("duration"), int | float) and not isinstance(
            projected.get("duration"), bool
        ):
            projected["duration"] = SESSION_DURATION_SENTINEL
        return projected
    if isinstance(value, list):
        return [_project_session_duration(item) for item in value]
    return value


@dataclass(frozen=True)
class HttpCapture:
    """One (request, normalised-response) pair recorded for a scenario."""

    request: HttpRequest
    response: HttpResponse

    def to_golden_dict(self) -> dict[str, Any]:
        """Render this capture as the dict the golden file holds.

        Keys are sorted in the serialised JSON, so two captures of
        the same response produce byte-identical golden text.
        """
        return {
            "request": {
                "method": self.request.method,
                "path": self.request.path,
                "query": dict(sorted(self.request.query.items())),
            },
            "response": {
                "status_code": self.response.status_code,
                "headers": dict(sorted(self.response.headers.items())),
                "body": self.response.body,
            },
        }


class HttpFingerprinter:
    """Capture HTTP requests against per-endpoint goldens for a scenario.

    The fingerprinter is bound to one ``scenario_dir`` for its lifetime;
    each ``capture`` call writes to (or compares against)
    ``<scenario_dir>/expected/http_responses/<endpoint_id>.json``. The
    normaliser is shared with the caller's existing fingerprint
    substrate so the symbol table is consistent across the per-scenario
    event-stream fingerprint, DB snapshot, and HTTP captures.
    """

    def __init__(
        self,
        scenario_dir: Path,
        normalizer: Normalizer,
        *,
        update: bool = False,
    ) -> None:
        self.scenario_dir = scenario_dir
        self.expected_dir = scenario_dir / "expected" / "http_responses"
        self.normalizer = normalizer
        self._update = update
        self._captured: list[str] = []

    @property
    def update_mode(self) -> bool:
        """True when the run was invoked with ``--update-fingerprints``."""
        return self._update

    @property
    def captured_endpoint_ids(self) -> tuple[str, ...]:
        """The endpoint_ids captured so far this run (in invocation order)."""
        return tuple(self._captured)

    def capture(
        self,
        response: Any,
        *,
        endpoint_id: str,
        request_method: str,
        request_path: str,
        request_query: dict[str, Any] | None = None,
    ) -> HttpCapture:
        """Normalise ``response`` and assert it against the endpoint's golden.

        ``response`` is a ``httpx.Response`` (TestClient yields these);
        the fingerprinter reads ``status_code``, ``headers``, and
        ``content`` so any compatible response object works. Returns
        the captured ``HttpCapture`` so callers can layer extra
        assertions on top (status range, body shape) without re-doing
        the normalisation.
        """
        content_type = response.headers.get("content-type")
        headers = project_headers(dict(response.headers))
        body = normalise_body(response.content, content_type, self.normalizer)
        normalised_path = normalise_path(request_path, self.normalizer)

        capture = HttpCapture(
            request=HttpRequest(
                method=request_method,
                path=normalised_path,
                query=dict(request_query or {}),
            ),
            response=HttpResponse(
                status_code=response.status_code,
                headers=headers,
                body=body,
            ),
        )

        self._captured.append(endpoint_id)
        golden_path = self.expected_dir / f"{endpoint_id}.json"
        actual = capture.to_golden_dict()

        if self._update:
            self._write_golden(golden_path, actual)
            return capture

        if not golden_path.exists():
            raise HttpFingerprintMismatch(
                endpoint_id=endpoint_id,
                expected=None,
                actual=actual,
            )

        expected = json.loads(golden_path.read_text(encoding="utf-8"))
        if expected != actual:
            raise HttpFingerprintMismatch(
                endpoint_id=endpoint_id,
                expected=expected,
                actual=actual,
            )
        return capture

    def _write_golden(self, path: Path, payload: dict[str, Any]) -> None:
        """Serialise ``payload`` to ``path`` as canonical sorted JSON."""
        path.parent.mkdir(parents=True, exist_ok=True)
        text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
        path.write_text(text, encoding="utf-8")
