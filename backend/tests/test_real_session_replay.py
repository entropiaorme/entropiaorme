"""Tests for the real-session offline replay cross-check.

Exercises the cross-check against the committed synthetic fixture bundle (no
real data): the Python-oracle replay-from-snapshot must reproduce the ratified
golden db_state over the codex/quest/skill + skill_gains catalogue, the replay
must be deterministic, the headline skill_gains coverage must be non-vacuous,
and a deliberately mutated reference must be DETECTED (the harness has real
detection power, not a vacuous pass).
"""

from __future__ import annotations

import copy
import json
import shutil
from pathlib import Path

import pytest

from backend.testing.real_session_replay import (
    EXPECTED_GOLDEN,
    capture_db_state,
    cross_check,
    main,
    replay_bundle,
)

FIXTURE = Path(__file__).resolve().parents[1] / "testing" / "fixtures" / "soak_replay"


def test_fixture_cross_check_is_clean() -> None:
    """The replay reproduces the committed golden: zero db_state divergence."""
    assert main(["--bundle", str(FIXTURE)]) == 0


def test_replay_is_deterministic() -> None:
    """Two replays of the same bundle yield identical normalised db_state
    (per-run UUIDs/instants symbolise to the same encounter-order tokens)."""
    assert replay_bundle(FIXTURE) == replay_bundle(FIXTURE)


def test_skill_gains_coverage_is_non_vacuous() -> None:
    """The segment actually writes the headline db_state-silent table."""
    state = replay_bundle(FIXTURE)
    assert state["skill_gains"], "the segment must produce skill_gains rows"
    assert any(
        row["skill_name"] == "Laser Weaponry Technology" for row in state["skill_gains"]
    )
    # The skill with a starting calibration also appends a chatlog-source point.
    assert any(row["source"] == "chatlog" for row in state["skill_calibrations"])


def test_negative_control_detects_a_mutated_reference() -> None:
    """A single mutated reference row MUST be flagged, and the unmutated
    reference MUST stay clean (no false positives)."""
    state = replay_bundle(FIXTURE)
    golden = json.loads((FIXTURE / EXPECTED_GOLDEN).read_text(encoding="utf-8"))

    mutated = copy.deepcopy(golden)
    mutated["skill_gains"][0]["amount"] = 999.0
    diverged = {table for table, message in cross_check(state, mutated) if message}
    assert "skill_gains" in diverged, "the cross-check failed to detect a mutation"

    assert all(message is None for _, message in cross_check(state, golden)), (
        "the cross-check false-positived against the clean reference"
    )


def test_capture_db_state_reads_the_silent_write_catalogue() -> None:
    """The reference-DB capture path reads the silent-write catalogue."""
    state = capture_db_state(FIXTURE / "starting_db.sqlite")
    assert "skill_gains" in state
    assert "codex_claims" in state


def test_update_writes_the_expected_golden(tmp_path) -> None:
    """The --update path first-pins the expected golden."""
    out = tmp_path / "expected.json"
    assert main(["--bundle", str(FIXTURE), "--expected", str(out), "--update"]) == 0
    assert out.is_file()
    assert json.loads(out.read_text(encoding="utf-8"))["skill_gains"]


def test_reference_db_path_detects_divergence() -> None:
    """The --reference-db path captures the reference and diffs: the starting
    DB lacks the replay's writes, so the cross-check reports divergence."""
    rc = main(
        [
            "--bundle",
            str(FIXTURE),
            "--reference-db",
            str(FIXTURE / "starting_db.sqlite"),
        ]
    )
    assert rc == 1


def test_missing_starting_db_raises(tmp_path) -> None:
    with pytest.raises(FileNotFoundError):
        replay_bundle(tmp_path)


def test_no_reference_returns_usage_error(tmp_path) -> None:
    """A bundle with neither a committed golden nor --reference-db/--update
    exits with the usage code."""
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    shutil.copy(FIXTURE / "starting_db.sqlite", bundle / "starting_db.sqlite")
    shutil.copy(FIXTURE / "chat_replay.log", bundle / "chat_replay.log")
    assert main(["--bundle", str(bundle)]) == 2
