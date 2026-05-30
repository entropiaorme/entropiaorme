"""Golden-file load, save, and update workflow for scripted scenarios.

Scenarios under ``backend/tests/e2e/corpus/<flavour>/<name>/`` carry an
``expected/`` subdirectory holding their canonical fingerprint and DB
snapshot. The default test posture asserts against these goldens; the
``--update-fingerprints`` flag flips the same code path into write
mode after surfacing the per-scenario diff so the developer can review
the change before ratifying it.

The biggest failure mode of any golden-file workflow is reflex updates
that ratify a real regression as "the new normal." Three guardrails
push back: the default mode fails on diff, the update mode is gated
behind an explicit CLI flag (``--update-fingerprints``), and the
update path writes only after surfacing the human-readable diff so the
ratification is deliberate rather than mechanical.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from backend.testing.db_snapshot import (
    capture as capture_snapshot,
)
from backend.testing.db_snapshot import (
    serialize as serialize_snapshot,
)
from backend.testing.diff import diff_fingerprint_files, diff_snapshot_dicts
from backend.testing.fingerprint import FingerprintRecorder, Normalizer


class GoldenAssertionFailure(AssertionError):
    """Raised when a scenario's actual output diverges from its goldens.

    Carries the structured per-surface diff in addition to the
    composed message so a downstream tool (a `--report` flag, a CI
    summariser) can render the same data without re-parsing the
    formatted output.
    """

    def __init__(
        self,
        scenario_name: str,
        fingerprint_diff: str | None,
        snapshot_diff: str | None,
    ):
        """Compose the AssertionError message from the structured
        per-surface diffs while keeping the diffs accessible on the
        instance for downstream tooling that wants the raw data."""
        self.scenario_name = scenario_name
        self.fingerprint_diff = fingerprint_diff
        self.snapshot_diff = snapshot_diff
        parts: list[str] = [f"Scenario {scenario_name!r} diverged from goldens."]
        if fingerprint_diff:
            parts.append("")
            parts.append("Fingerprint diff:")
            parts.append(fingerprint_diff)
        if snapshot_diff:
            parts.append("")
            parts.append("DB snapshot diff:")
            parts.append(snapshot_diff)
        parts.append("")
        parts.append(
            "Rerun with `pytest --update-fingerprints` (and review the "
            "surfaced diff) if the new output is the intended new golden."
        )
        super().__init__("\n".join(parts))


class GoldenSet:
    """One scenario's ``expected/`` paired with a fresh recorder.

    Construct one per test; install the recorder on the bus before the
    pipeline starts emitting, then call ``assert_matches`` after the
    pipeline has drained. The shared ``Normalizer`` flows into both
    the fingerprint and the DB snapshot so UUIDs and timestamps stay
    aligned across the two surfaces.
    """

    def __init__(self, scenario_dir: Path, *, update: bool = False) -> None:
        """Set up the recorder + normaliser pair pinned to
        ``scenario_dir/expected/``. ``update=True`` flips this set
        into write mode for the duration of the test."""
        self.scenario_dir = scenario_dir
        self.expected_dir = scenario_dir / "expected"
        self.fingerprint_path = self.expected_dir / "fingerprint.jsonl"
        self.snapshot_path = self.expected_dir / "db_state.json"
        self.normalizer = Normalizer()
        self.recorder = FingerprintRecorder(self.normalizer)
        self._update = update

    @property
    def update_mode(self) -> bool:
        """True when ``--update-fingerprints`` was passed to pytest."""
        return self._update

    def assert_matches(self, db: sqlite3.Connection) -> None:
        """Compare the recorded fingerprint + DB snapshot to goldens.

        Under update mode, surfaces the diff vs the existing golden
        (if any) and writes the new golden afterwards. Under default
        mode, raises ``GoldenAssertionFailure`` on any divergence with
        the structured diff in the message.
        """
        actual_fingerprint = self.recorder.serialize()
        actual_snapshot = capture_snapshot(db, normalizer=self.normalizer)

        if self._update:
            self._update_goldens(actual_fingerprint, actual_snapshot)
            return

        if not self.fingerprint_path.exists() or not self.snapshot_path.exists():
            raise GoldenAssertionFailure(
                scenario_name=self.scenario_dir.name,
                fingerprint_diff=(
                    f"Goldens missing for scenario {self.scenario_dir.name!r}; "
                    "rerun with --update-fingerprints to generate the first "
                    "golden set."
                ),
                snapshot_diff=None,
            )

        expected_fingerprint = self.fingerprint_path.read_text(encoding="utf-8")
        expected_snapshot = json.loads(self.snapshot_path.read_text(encoding="utf-8"))

        fingerprint_diff = diff_fingerprint_files(
            expected_fingerprint, actual_fingerprint
        )
        snapshot_diff = diff_snapshot_dicts(expected_snapshot, actual_snapshot)

        if fingerprint_diff or snapshot_diff:
            raise GoldenAssertionFailure(
                scenario_name=self.scenario_dir.name,
                fingerprint_diff=fingerprint_diff,
                snapshot_diff=snapshot_diff,
            )

    def _update_goldens(
        self,
        fingerprint_text: str,
        snapshot: dict,
    ) -> None:
        """Surface any prior-vs-new diff, then overwrite the golden
        files. Surfacing the diff first is the deliberate guardrail
        against unthinking ratification of regressions."""
        prior_fingerprint = (
            self.fingerprint_path.read_text(encoding="utf-8")
            if self.fingerprint_path.exists()
            else ""
        )
        prior_snapshot = (
            json.loads(self.snapshot_path.read_text(encoding="utf-8"))
            if self.snapshot_path.exists()
            else {}
        )

        fingerprint_diff = diff_fingerprint_files(prior_fingerprint, fingerprint_text)
        snapshot_diff = diff_snapshot_dicts(prior_snapshot, snapshot)

        if fingerprint_diff or snapshot_diff:
            print(
                f"\n--- Golden update: {self.scenario_dir.name} ---",
                flush=True,
            )
            if fingerprint_diff:
                print("Fingerprint diff:", flush=True)
                print(fingerprint_diff, flush=True)
            if snapshot_diff:
                print("DB snapshot diff:", flush=True)
                print(snapshot_diff, flush=True)
            print("--- End golden update ---\n", flush=True)
        else:
            print(
                f"\nGolden update: {self.scenario_dir.name} - no change",
                flush=True,
            )

        self.expected_dir.mkdir(parents=True, exist_ok=True)
        self.fingerprint_path.write_text(fingerprint_text, encoding="utf-8")
        self.snapshot_path.write_text(serialize_snapshot(snapshot), encoding="utf-8")
