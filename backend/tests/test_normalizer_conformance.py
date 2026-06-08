"""Python leg of the cross-language Normalizer conformance table.

The committed fixture ``backend/testing/equivalence/normalizer_conformance.json``
is the shared contract: this leg asserts the Python oracle reproduces every
``expected`` value in it, and that the fixture is in sync with the authored
cases (so a stale fixture cannot pass). The Rust leg
(``eo-wire/tests/conformance.rs``) asserts the native normaliser reproduces the
same ``expected`` bytes from the same fixture; both legs green is the
cross-language guarantee.
"""

from __future__ import annotations

import json

import pytest

from backend.testing.equivalence.table import (
    FIXTURE_PATH,
    build_table,
    serialize_table,
)
from backend.testing.normalize_cli import normalize_compact

_FIXTURE = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))


def test_committed_fixture_is_in_sync_with_cases() -> None:
    """The committed fixture must equal a fresh build from the authored cases.

    If this fails, the cases changed without regenerating the fixture: run
    ``python -m backend.testing.equivalence.table``. Guarding freshness here is
    what lets the Rust leg trust the committed bytes.
    """
    assert FIXTURE_PATH.read_text(encoding="utf-8") == serialize_table(build_table())


@pytest.mark.parametrize("case", _FIXTURE, ids=[c["name"] for c in _FIXTURE])
def test_python_leg_reproduces_expected(case: dict) -> None:
    """The Python oracle normalises each input to the committed ``expected``."""
    assert normalize_compact(case["input"]) == case["expected"]
