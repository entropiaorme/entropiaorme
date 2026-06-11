"""Python faithfulness leg for the ``.yml``-family equivalence mirrors.

Proves each committed JSON mirror (under
``backend/testing/equivalence/yml_family/``) faithfully equals its
pytest-regressions ``.yml`` pin and is the current normalised form, so the Rust
runner (``eo-wire/tests/yml_family.rs``) inherits the ``.yml`` projections
mechanically rather than relying on the rarely-run phase-boundary smoke.

Regenerate the mirrors with ``python -m backend.testing.equivalence.yml_family``
after a ``.yml`` golden moves.
"""

from __future__ import annotations

import json

import pytest

from backend.testing.equivalence.yml_family import (
    MIRROR_DIR,
    YML_GOLDENS,
    load_yml,
    mirror_text,
)

_STEMS = sorted(YML_GOLDENS)


@pytest.mark.parametrize("stem", _STEMS)
def test_mirror_is_current_normalised_form(stem: str) -> None:
    """The committed mirror equals a fresh render of its ``.yml`` pin."""
    expected = mirror_text(load_yml(YML_GOLDENS[stem]))
    actual = (MIRROR_DIR / f"{stem}.json").read_text(encoding="utf-8")
    assert actual == expected, (
        f"{stem} mirror is stale; regenerate with "
        "`python -m backend.testing.equivalence.yml_family`"
    )


@pytest.mark.parametrize("stem", _STEMS)
def test_mirror_faithfully_equals_yml_pin(stem: str) -> None:
    """The mirror's data structurally equals the ``.yml`` pin.

    These projections carry no volatile fields, so normalisation is identity:
    the mirror is the pinned projection in the runner's canonical JSON form.
    """
    mirror_data = json.loads((MIRROR_DIR / f"{stem}.json").read_text(encoding="utf-8"))
    assert mirror_data == load_yml(YML_GOLDENS[stem])
