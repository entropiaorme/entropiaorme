"""Unit tests for the HTTP fingerprint projection helpers.

These pin the pure normalisation functions the fingerprinter composes:
ETag sentinelisation, the narrow header projection, path-UUID
symbolisation, body decoding (JSON / empty / binary), and the wall-clock
session-duration projection.
"""

from __future__ import annotations

import json

from backend.testing.fingerprint import Normalizer
from backend.testing.http_fingerprint import (
    ETAG_SENTINEL,
    SESSION_DURATION_SENTINEL,
    HttpFingerprintMismatch,
    _project_session_duration,
    normalise_body,
    normalise_etag,
    normalise_path,
    project_headers,
)

_STRONG_ETAG = '"' + "a" * 64 + '"'


def test_normalise_etag_variants():
    assert normalise_etag(_STRONG_ETAG) == ETAG_SENTINEL
    assert normalise_etag("not-an-etag") == "not-an-etag"
    assert normalise_etag(None) is None


def test_project_headers_keeps_only_the_projection():
    projected = project_headers(
        {
            "Content-Type": "application/json",
            "Cache-Control": "no-cache",
            "ETag": _STRONG_ETAG,
            "Server": "uvicorn",
            "Date": "today",
        }
    )
    assert projected == {
        "content-type": "application/json",
        "cache-control": "no-cache",
        "etag": ETAG_SENTINEL,
    }


def test_project_headers_skips_absent_headers():
    assert project_headers({"content-type": "text/plain"}) == {
        "content-type": "text/plain"
    }


def test_normalise_path_symbolises_uuids():
    norm = Normalizer()
    uuid = "12345678-1234-1234-1234-1234567890ab"
    out = normalise_path(f"/api/tracking/session/{uuid}", norm)
    assert uuid not in out
    assert normalise_path("/api/tracking/snapshot", norm) == "/api/tracking/snapshot"


def test_normalise_body_json_empty_and_binary():
    norm = Normalizer()
    assert normalise_body(b"", "application/json", norm) is None
    body = normalise_body(b'{"a": 1}', "application/json", norm)
    assert body == {"a": 1}
    binary = normalise_body(b"\x89PNG\r\n", "image/png", norm)
    assert binary == {"_binary": True, "byte_length": 6}


def test_project_session_duration_only_numeric_duration():
    projected = _project_session_duration(
        {"summary": {"duration": 5, "kills": 3}, "items": [{"duration": 2.0}]}
    )
    assert projected["summary"]["duration"] == SESSION_DURATION_SENTINEL
    assert projected["items"][0]["duration"] == SESSION_DURATION_SENTINEL
    assert projected["summary"]["kills"] == 3
    # Booleans are not numeric durations and must be left alone.
    assert _project_session_duration({"duration": True}) == {"duration": True}
    # Scalars pass through unchanged.
    assert _project_session_duration("plain") == "plain"


def test_mismatch_message_names_the_endpoint():
    exc = HttpFingerprintMismatch("GET_x", {"a": 1}, {"a": 2})
    text = str(exc)
    assert "GET_x" in text
    assert exc.expected == {"a": 1}
    assert exc.actual == {"a": 2}
    assert json.dumps(exc.actual)  # actual is JSON-serialisable
