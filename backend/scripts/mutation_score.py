"""Turn a mutmut campaign's stats into a score, a badge, and a pass/fail gate.

`mutmut export-cicd-stats` writes ``mutants/mutmut-cicd-stats.json`` with the
per-status mutant counts. This script reads that file and:

- prints a human-readable summary of the campaign;
- computes the mutation score (the share of testable mutants the suite caught);
- optionally writes a shields.io endpoint badge (``--badge-out``);
- optionally fails (exit 1) when the score is below a floor (``--fail-under``),
  which is how the nightly campaign ratchets and never silently regresses.

The score counts a mutant as *caught* when a test failed on it (``killed``) or
the mutation broke execution badly enough to time out. Mutants that survived,
that no test exercised, or whose run was merely suspicious all count against the
score: a surviving mutant is a real gap in the suite's assertions. Mutants
excluded with ``# pragma: no mutate`` (``skipped``) and internal-error buckets
(segfault, user interruption) are left out of the denominator entirely.

Usage::

    python -m backend.scripts.mutation_score                 # summary + score
    python -m backend.scripts.mutation_score --badge-out mutation.json
    python -m backend.scripts.mutation_score --fail-under 70
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

_DEFAULT_STATS = Path("mutants/mutmut-cicd-stats.json")


def compute_score(stats: dict[str, int]) -> tuple[float, int, int]:
    """Return (score_percent, caught, considered) from a cicd-stats mapping."""
    killed = stats.get("killed", 0)
    timeout = stats.get("timeout", 0)
    suspicious = stats.get("suspicious", 0)
    survived = stats.get("survived", 0)
    no_tests = stats.get("no_tests", 0)

    caught = killed + timeout
    considered = killed + timeout + suspicious + survived + no_tests
    score = 100.0 * caught / considered if considered else 0.0
    return round(score, 1), caught, considered


def _colour(score: float) -> str:
    """Map a score to a shields.io colour band."""
    for floor, name in (
        (90, "brightgreen"),
        (80, "green"),
        (70, "yellowgreen"),
        (60, "yellow"),
        (50, "orange"),
    ):
        if score >= floor:
            return name
    return "red"


def _badge(score: float) -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "label": "mutation",
        "message": f"{score:.1f}%",
        "color": _colour(score),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "stats",
        nargs="?",
        type=Path,
        default=_DEFAULT_STATS,
        help="path to mutmut-cicd-stats.json (default: %(default)s)",
    )
    parser.add_argument(
        "--badge-out",
        type=Path,
        default=None,
        help="write a shields.io endpoint badge JSON to this path",
    )
    parser.add_argument(
        "--fail-under",
        type=float,
        default=None,
        help="exit 1 if the score is below this floor",
    )
    args = parser.parse_args(argv)

    if not args.stats.exists():
        print(
            f"error: {args.stats} not found; run `mutmut run` then "
            "`mutmut export-cicd-stats` first.",
            file=sys.stderr,
        )
        return 2

    stats = json.loads(args.stats.read_text(encoding="utf-8"))
    score, caught, considered = compute_score(stats)

    print(
        "Mutants: "
        f"{stats.get('killed', 0)} killed, "
        f"{stats.get('survived', 0)} survived, "
        f"{stats.get('timeout', 0)} timeout, "
        f"{stats.get('suspicious', 0)} suspicious, "
        f"{stats.get('no_tests', 0)} no-tests, "
        f"{stats.get('skipped', 0)} skipped"
    )
    print(f"Mutation score: {score:.1f}%  ({caught}/{considered} caught)")

    if args.badge_out is not None:
        args.badge_out.write_text(json.dumps(_badge(score)) + "\n", encoding="utf-8")
        print(f"Wrote badge to {args.badge_out}")

    if args.fail_under is not None and score < args.fail_under:
        print(
            f"FAIL: mutation score {score:.1f}% is below the floor "
            f"{args.fail_under:.1f}%.",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
