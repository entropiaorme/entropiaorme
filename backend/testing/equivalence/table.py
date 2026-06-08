"""Build (and regenerate) the committed Normalizer conformance fixture.

``build_table()`` runs every case input from ``conformance_cases.CASES``
through the Python oracle and returns the ``[{name, input, expected}]`` table.
``FIXTURE_PATH`` is the committed JSON both conformance legs assert against.

Regenerate with ``python -m backend.testing.equivalence.table`` after editing
``conformance_cases.CASES``; the Python conformance test fails if the committed
fixture drifts from the cases, so a stale fixture cannot pass silently.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from backend.testing.equivalence.conformance_cases import CASES
from backend.testing.normalize_cli import normalize_compact

FIXTURE_PATH = Path(__file__).parent / "normalizer_conformance.json"


def build_table() -> list[dict[str, Any]]:
    """Return the conformance table: each case with its oracle-normalised form."""
    return [
        {"name": name, "input": value, "expected": normalize_compact(value)}
        for name, value in CASES
    ]


def serialize_table(table: list[dict[str, Any]]) -> str:
    """Render the table as the committed fixture text (LF, trailing newline)."""
    return json.dumps(table, indent=2, ensure_ascii=False) + "\n"


def write_fixture() -> None:
    """Regenerate the committed fixture from the current cases."""
    FIXTURE_PATH.write_text(
        serialize_table(build_table()), encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    write_fixture()
    print(f"Wrote {len(CASES)} conformance cases to {FIXTURE_PATH}")
