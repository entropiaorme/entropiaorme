"""Domain-event schema snapshot drift detection.

The discriminated-union envelope in ``backend/core/domain_events.py`` is the
canonical wire contract for every event bridged across the IPC seam (over the
SSE stream and re-emitted onto the Tauri event bus). It is also the contract a
future Rust emitter must reproduce byte-for-byte: a ``#[serde(tag = "type")]``
enum whose JSON shape matches this one is, by construction, frontend-compatible.

A silent shift in that shape (a renamed field, a relaxed type, a dropped event
version) is exactly the kind of regression that survives every unit test until a
frontend listener or the Rust port hits it. This test pins it two ways:

- the generated JSON Schema of the whole union, snapshotted against a tracked
  golden (the ``test_openapi_drift`` recipe), so a structural change must be
  deliberately ratified;
- a serialised example of a representative envelope, asserted against the exact
  wire dict, so the *value-level* contract (``occurred_at`` is an ISO-8601-UTC
  string, payload keys are camelCase) is pinned, not just the schema metadata.

Regenerate the schema golden with ``pytest --update-fingerprints`` (the flag
registered in the backend-root conftest) after reviewing the surfaced diff.
"""

from __future__ import annotations

import difflib
import json
from pathlib import Path

import pytest

from backend.core.domain_events import (
    DomainEventAdapter,
    ScanStatusChanged,
    ScanStatusChangedPayload,
    TrackingSessionUpdated,
    TrackingSessionUpdatedPayload,
    to_iso_utc,
)

EXPECTED_PATH = Path(__file__).parent / "expected" / "event_schemas.snapshot.json"


def _canonical_json(payload: dict) -> str:
    """Render ``payload`` as canonical sorted JSON for stable diffing.

    ``sort_keys`` is load-bearing: ``json_schema()`` field ordering can shift
    across the 3.11 and 3.14 CI interpreters, so sorting is what keeps the
    golden stable rather than spuriously diffing per interpreter.
    """
    return json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


@pytest.fixture
def update_fingerprints(request) -> bool:
    """Re-export the backend-wide ``--update-fingerprints`` flag locally."""
    return bool(request.config.getoption("--update-fingerprints"))


def test_event_schema_snapshot_matches_golden(update_fingerprints: bool) -> None:
    """The generated domain-event JSON Schema equals the tracked golden.

    Regenerate with ``pytest --update-fingerprints`` after reviewing the
    surfaced diff; never auto-ratify a regression.
    """
    schema = DomainEventAdapter.json_schema()
    actual_text = _canonical_json(schema)

    if update_fingerprints:
        prior_text = (
            EXPECTED_PATH.read_text(encoding="utf-8") if EXPECTED_PATH.exists() else ""
        )
        if prior_text != actual_text:
            diff = "".join(
                difflib.unified_diff(
                    prior_text.splitlines(keepends=True),
                    actual_text.splitlines(keepends=True),
                    fromfile="event_schemas.snapshot.json (golden)",
                    tofile="event_schemas.snapshot.json (this run)",
                )
            )
            # Surface the diff so a ratification is deliberate. ``pytest -s``
            # shows this print directly under the test name.
            print("\n--- Event schema snapshot update ---")
            print(diff)
            print("--- End event schema snapshot update ---\n")
        EXPECTED_PATH.parent.mkdir(parents=True, exist_ok=True)
        # newline="\n" so a Windows regen writes LF directly, matching the repo's
        # `*.json eol=lf` policy rather than emitting CRLF in text mode.
        EXPECTED_PATH.write_text(actual_text, encoding="utf-8", newline="\n")
        return

    assert EXPECTED_PATH.exists(), (
        f"Event schema golden missing at {EXPECTED_PATH}; "
        "rerun with --update-fingerprints to generate the first golden."
    )

    expected_text = EXPECTED_PATH.read_text(encoding="utf-8")
    if expected_text == actual_text:
        return

    diff = "".join(
        difflib.unified_diff(
            expected_text.splitlines(keepends=True),
            actual_text.splitlines(keepends=True),
            fromfile="event_schemas.snapshot.json (golden)",
            tofile="event_schemas.snapshot.json (this run)",
            n=3,
        )
    )
    pytest.fail(
        "Domain-event schema diverged from golden. Diff:\n\n"
        + diff
        + "\n\nRerun with `pytest --update-fingerprints` (and review the "
        "diff above) if the new schema is the intended new golden."
    )


def test_event_envelope_serialises_to_expected_wire() -> None:
    """Pin the value-level wire contract of a representative envelope.

    This is the strongest acceptance: it proves the actual bytes a frontend
    listener (and the Rust port) sees, not just the schema metadata. A change to
    field casing or the timestamp encoding fails here even if the schema's shape
    is unaffected.
    """
    envelope = TrackingSessionUpdated(
        occurred_at=to_iso_utc(1735680000.0),
        payload=TrackingSessionUpdatedPayload(
            sessionId="session-abc",
            status="active",
            reason="started",
        ),
    )

    assert envelope.model_dump(mode="json") == {
        "type": "tracking.session.updated",
        "event_version": 1,
        "occurred_at": "2024-12-31T21:20:00+00:00",
        "payload": {
            "sessionId": "session-abc",
            "status": "active",
            "reason": "started",
        },
    }

    # Round-trips back through the union adapter to the same typed model.
    restored = DomainEventAdapter.validate_python(envelope.model_dump(mode="json"))
    assert isinstance(restored, TrackingSessionUpdated)
    assert restored.payload.sessionId == "session-abc"


def test_scan_status_changed_serialises_to_expected_wire() -> None:
    """Pin the value-level wire contract of the scan domain event.

    The second union member must serialise to the same closed, camelCase,
    ISO-8601-UTC envelope shape as the first, and the discriminator must select
    it (not tracking) on the way back. This proves the actual bytes a frontend
    listener (and the Rust port) sees for the new topic, not just the schema.
    """
    envelope = ScanStatusChanged(
        occurred_at=to_iso_utc(1735680000.0),
        payload=ScanStatusChangedPayload(phase="processing"),
    )

    assert envelope.model_dump(mode="json") == {
        "type": "scan.status.changed",
        "event_version": 1,
        "occurred_at": "2024-12-31T21:20:00+00:00",
        "payload": {
            "phase": "processing",
        },
    }

    restored = DomainEventAdapter.validate_python(envelope.model_dump(mode="json"))
    assert isinstance(restored, ScanStatusChanged)
    assert restored.payload.phase == "processing"
