"""OpenAPI snapshot drift detection.

FastAPI's ``/openapi.json`` is the canonical HTTP contract: every
schemathesis run, code-generator pass, and frontend client builds off
it. A silent shift in that spec (e.g. a route's response_model
changes, a parameter constraint relaxes, a new operation slips in
unannounced) is exactly the kind of regression that survives every
unit test until a client hits it in production.

This test snapshots the live ``app.openapi()`` against a tracked
golden under ``backend/tests/expected/openapi.snapshot.json``. The
default mode asserts equality; ``--update-fingerprints`` (the flag
already registered in the backend-root conftest) flips into write
mode for deliberate ratification, with the diff surfaced so the
change is deliberate rather than mechanical.

The spec is captured directly from ``app.openapi()`` (the same surface
``/openapi.json`` serves) rather than through an HTTP call so the test
does not depend on the lifespan or a TestClient context.
"""

from __future__ import annotations

import difflib
import json
from pathlib import Path

import pytest

from backend.main import app

EXPECTED_PATH = Path(__file__).parent / "expected" / "openapi.snapshot.json"


def _canonical_json(payload: dict) -> str:
    """Render ``payload`` as canonical sorted JSON for stable diffing."""
    return json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


@pytest.fixture
def update_fingerprints(request) -> bool:
    """Re-export the backend-wide ``--update-fingerprints`` flag locally."""
    return bool(request.config.getoption("--update-fingerprints"))


def test_openapi_snapshot_matches_golden(update_fingerprints: bool) -> None:
    """The live OpenAPI spec equals the tracked golden.

    Regenerate with ``pytest --update-fingerprints`` after reviewing
    the surfaced diff; never auto-ratify a regression.
    """
    spec = app.openapi()
    actual_text = _canonical_json(spec)

    if update_fingerprints:
        prior_text = (
            EXPECTED_PATH.read_text(encoding="utf-8") if EXPECTED_PATH.exists() else ""
        )
        if prior_text != actual_text:
            diff = "".join(
                difflib.unified_diff(
                    prior_text.splitlines(keepends=True),
                    actual_text.splitlines(keepends=True),
                    fromfile="openapi.snapshot.json (golden)",
                    tofile="openapi.snapshot.json (this run)",
                )
            )
            # Surface the diff so a ratification is deliberate rather
            # than mechanical. The print is deliberate (not logging):
            # ``pytest -s`` shows it directly under the test name.
            print("\n--- OpenAPI snapshot update ---")
            print(diff)
            print("--- End OpenAPI snapshot update ---\n")
        EXPECTED_PATH.parent.mkdir(parents=True, exist_ok=True)
        # newline="\n" so a Windows regen writes LF directly, matching the repo's
        # `*.json eol=lf` policy rather than emitting CRLF in text mode.
        EXPECTED_PATH.write_text(actual_text, encoding="utf-8", newline="\n")
        return

    assert EXPECTED_PATH.exists(), (
        f"OpenAPI golden missing at {EXPECTED_PATH}; "
        "rerun with --update-fingerprints to generate the first golden."
    )

    expected_text = EXPECTED_PATH.read_text(encoding="utf-8")
    if expected_text == actual_text:
        return

    diff = "".join(
        difflib.unified_diff(
            expected_text.splitlines(keepends=True),
            actual_text.splitlines(keepends=True),
            fromfile="openapi.snapshot.json (golden)",
            tofile="openapi.snapshot.json (this run)",
            n=3,
        )
    )
    pytest.fail(
        "OpenAPI spec diverged from golden. Diff:\n\n"
        + diff
        + "\n\nRerun with `pytest --update-fingerprints` (and review the "
        "diff above) if the new spec is the intended new golden."
    )


def test_openapi_get_surface_carries_expected_prefixes() -> None:
    """A sanity invariant on the spec: the four hydration prefixes
    are still present, so a refactor that silently drops a router
    surfaces here before the golden test reads the smaller spec.
    """
    spec = app.openapi()
    paths = set(spec.get("paths", {}))
    required = {
        "/api/tracking/snapshot",
        "/api/scan/skills/status",
        "/api/quests",
        "/api/codex/species",
    }
    missing = required - paths
    assert not missing, f"OpenAPI spec is missing required hydration paths: {missing}"
