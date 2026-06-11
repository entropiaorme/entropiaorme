"""Bridge the three out-of-``expected/`` pytest-regressions ``.yml`` goldens
into the cross-language equivalence runner.

``test_hotbar_slot_use`` and ``test_spacebar_scan_capture`` (the listener
bus-stream pins) and ``test_quest_automation_with_playlist_match`` (the
automation pin) sit outside the ``expected/`` corpus the Rust runner reads, so
an implementer grading against the ``cargo test`` gate alone would not inherit
them. This module emits a canonical-JSON mirror of each ``.yml``-pinned
projection (the projection run through the shared Normalizer and serialised the
way the DB-state golden is) so:

- the Rust runner asserts the native normaliser + serialiser reproduce each
  pinned projection byte-for-byte (``eo-wire/tests/yml_family.rs``);
- the Python leg asserts each mirror faithfully equals its ``.yml`` pin
  (``test_equivalence_yml_family.py``).

These projections carry no volatile fields, so normalisation is identity over
them; the mirror is therefore the pinned projection in the runner's canonical
JSON form, and the cross-language assertion is that both languages render it
identically. Regenerate with ``python -m backend.testing.equivalence.yml_family``
after a ``.yml`` golden moves.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import yaml

from backend.testing.fingerprint import Normalizer

_E2E = Path(__file__).resolve().parents[2] / "tests" / "e2e"
MIRROR_DIR = Path(__file__).parent / "yml_family"

# scenario key -> committed .yml golden produced by the named pytest-regressions
# test. The key is the mirror filename stem.
YML_GOLDENS: dict[str, Path] = {
    "hotbar_slot_use": _E2E
    / "test_hotbar_slot_use"
    / "test_hotbar_slot_use_drives_listener_via_keystroke_source.yml",
    "spacebar_scan_capture": _E2E
    / "test_spacebar_scan_capture"
    / "test_spacebar_scan_capture_drives_listener_via_keystroke_source.yml",
    "quest_automation_with_playlist_match": _E2E
    / "test_quest_automation_with_playlist_match"
    / "test_quest_automation_resolves_session_to_playlist_exact_match.yml",
}


def load_yml(path: Path) -> Any:
    """Parse a committed ``.yml`` golden into its plain Python data."""
    return yaml.safe_load(path.read_text(encoding="utf-8"))


def mirror_text(data: Any) -> str:
    """Render ``data`` as the canonical JSON mirror (normalised, indent-2,
    sorted keys, trailing newline) the runner compares against."""
    normalised = Normalizer().normalize(data)
    return json.dumps(normalised, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


def write_mirrors() -> None:
    """Regenerate every committed mirror from its ``.yml`` pin."""
    MIRROR_DIR.mkdir(parents=True, exist_ok=True)
    for stem, yml_path in YML_GOLDENS.items():
        (MIRROR_DIR / f"{stem}.json").write_text(
            mirror_text(load_yml(yml_path)), encoding="utf-8", newline="\n"
        )


if __name__ == "__main__":
    write_mirrors()
    print(f"Wrote {len(YML_GOLDENS)} .yml-family mirrors to {MIRROR_DIR}")
