"""Enforce per-file mutation-score floors over a cargo-mutants run.

cargo-mutants writes ``mutants.out/outcomes.json`` with one outcome per
mutant. This script reads it and:

- prints a per-file summary of the campaign (caught / missed / unviable
  and the resulting score);
- enforces the per-file floors below, failing (exit 1) when any scored
  file falls under its floor;
- holds every file *without* a floor entry to the strictest bar (any
  missed mutant fails), so new code starts at full strength and only
  gains a floor once its measured score is deliberately adopted.

Scoring matches the Python campaign's conventions: a mutant counts as
caught when a test failed on it or the mutated build timed out; missed
mutants count against the score; unviable mutants (the mutation does
not compile) leave the denominator entirely.

Floors only ever ratchet up. The values below are the adoption-time
staging floors, a shade under the score measured when the gate was
wired; each ratchets to its module's inherited floor from the Python
campaign (`nightly.yml` FLOORS) when the corresponding service port
completes, with remaining survivors killed or individually annotated at
that point.

Usage (from ``frontend/src-tauri`` after a campaign)::

    python3 ../../backend/scripts/rust_mutation_floors.py \
        --outcomes mutants.out/outcomes.json
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

#: file (workspace-relative, as cargo-mutants reports it) -> floor %.
FLOORS: dict[str, float] = {
    # The inherited floor from backend.services.cost_engine, adopted at
    # the unit's port completion (measured 97.1 after the hardening
    # pass). Known equivalent mutants, annotated rather than chased: the
    # three falsy-guard replacements (economy/num_or_zero/sum_damage),
    # where the falsy input and the empty default produce identical
    # downstream values by construction.
    "eo-services/src/cost_engine.rs": 92.0,
    # Measured 96.4. The two residual survivors are equivalent on the
    # shipped curve data only: consecutive integer anchors make the
    # interpolation divisor exactly 1, so the /, * and % variants
    # coincide for the in-segment fraction. The bisection's comparison
    # strictness and top-anchor guard are NOT equivalent and are pinned
    # by oracle-valued tests instead.
    "eo-services/src/tt_value_curve.rs": 92.0,
    # Oracle-side comparison plumbing (not a ported service): staged at
    # measured strength; ratchet as the comparison surface hardens.
    "eo-wire/src/normalizer.rs": 81.0,
    "eo-wire/src/http_fingerprint.rs": 97.0,
}


def score_outcomes(outcomes_path: Path) -> dict[str, dict[str, int]]:
    """Per-file caught/missed/timeout/unviable counts from outcomes.json."""
    data = json.loads(outcomes_path.read_text(encoding="utf-8"))
    per_file: dict[str, dict[str, int]] = defaultdict(
        lambda: {"caught": 0, "missed": 0, "timeout": 0, "unviable": 0}
    )
    for outcome in data["outcomes"]:
        scenario = outcome["scenario"]
        if scenario == "Baseline":
            continue
        counts = per_file[scenario["Mutant"]["file"]]
        summary = outcome["summary"]
        if summary == "CaughtMutant":
            counts["caught"] += 1
        elif summary == "MissedMutant":
            counts["missed"] += 1
        elif summary == "Timeout":
            counts["timeout"] += 1
        elif summary == "Unviable":
            counts["unviable"] += 1
        else:
            raise SystemExit(f"unrecognised outcome summary: {summary!r}")
    return per_file


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--outcomes",
        type=Path,
        default=Path("mutants.out/outcomes.json"),
        help="path to a cargo-mutants outcomes.json",
    )
    args = parser.parse_args(argv)

    per_file = score_outcomes(args.outcomes)
    if not per_file:
        print("no mutants in the campaign output; nothing to score")
        return 1

    failures: list[str] = []
    print(f"{'file':45s} {'caught':>6s} {'missed':>6s} {'score':>7s} {'floor':>9s}")
    for file, counts in sorted(per_file.items()):
        caught = counts["caught"] + counts["timeout"]
        denominator = caught + counts["missed"]
        score = 100.0 * caught / denominator if denominator else 100.0
        floor = FLOORS.get(file)
        bar = f"{floor:.1f}" if floor is not None else "no-missed"
        print(f"{file:45s} {caught:6d} {counts['missed']:6d} {score:7.1f} {bar:>9s}")
        if floor is not None:
            if score < floor:
                failures.append(f"{file}: score {score:.1f} below floor {floor:.1f}")
        elif counts["missed"]:
            failures.append(
                f"{file}: {counts['missed']} missed mutant(s) and no adopted floor"
            )

    # A floor whose file produced no scored mutants is a silently vacuous
    # gate (a rename or deletion would otherwise pass unnoticed).
    for file, floor in sorted(FLOORS.items()):
        if file not in per_file:
            failures.append(
                f"{file}: adopted floor {floor:.1f} but no scored mutants "
                "(renamed or removed? update the floor map)"
            )

    if failures:
        print("\nmutation floors violated:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print("\nall mutation floors hold")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
