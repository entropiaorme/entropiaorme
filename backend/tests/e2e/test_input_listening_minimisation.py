"""Input-listening minimisation policy gate.

The recorded ``hunt_with_skill_scan`` bundle (kept local-by-default,
gitignored, not on the public surface) carries real keystroke
fixtures. This test reads its ``keystrokes.jsonl`` and asserts every
captured key is admitted by the production listener allow-lists
(HOTBAR_SLOT_KEYS ∪ ``{"space"}``), making the input-listening
minimisation policy verifiable from real fixtures rather than
trusted by inspection.

Skipped on environments without the bundle (fresh checkouts, public
CI, other contributors); the gate fires only where real keystroke
data exists.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from backend.services.hotbar_listener import HOTBAR_SLOT_KEYS

# Production listener allow-lists, named so a future allow-list
# expansion (e.g. F-key bindings, configurable hotbar surfaces)
# is one source of truth that this test reads from.
ALLOWED_KEYS = HOTBAR_SLOT_KEYS | {"space"}

RECORDED_BUNDLE = (
    Path(__file__).parent
    / "corpus"
    / "recorded"
    / "hunt_with_skill_scan"
    / "keystrokes.jsonl"
)


def test_recorded_keystrokes_are_within_input_listening_allowlist() -> None:
    """Every recorded keystroke must lie within the listener allow-lists.

    A failure here indicates either an out-of-policy capture (the
    recorder grew a tap onto a listener whose allow-list does not
    constrain it) or a real-world allow-list expansion that the
    policy needs to ratify deliberately.
    """
    if not RECORDED_BUNDLE.exists():
        pytest.skip(
            "Recorded bundle absent (local-by-default fixture); "
            f"path: {RECORDED_BUNDLE}"
        )

    records = [
        json.loads(line)
        for line in RECORDED_BUNDLE.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert records, "recorded bundle present but empty"

    out_of_policy = sorted(
        {record["key"] for record in records if record["key"] not in ALLOWED_KEYS}
    )
    assert not out_of_policy, (
        "Recorded keystrokes outside the production allow-lists: "
        f"{out_of_policy}. Either the recorder grew an out-of-policy "
        "tap, or the allow-list policy needs deliberate expansion."
    )


def test_recorded_keystrokes_kinds_are_press_or_release() -> None:
    """Every record carries a recognised edge kind.

    A drift here means a recorder format change — the fingerprint
    contract caught the change before any consumer broke.
    """
    if not RECORDED_BUNDLE.exists():
        pytest.skip(
            "Recorded bundle absent (local-by-default fixture); "
            f"path: {RECORDED_BUNDLE}"
        )

    records = [
        json.loads(line)
        for line in RECORDED_BUNDLE.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    unrecognised = sorted(
        {
            record["kind"]
            for record in records
            if record["kind"] not in {"press", "release"}
        }
    )
    assert not unrecognised, f"Unrecognised kinds in recorded bundle: {unrecognised}"
