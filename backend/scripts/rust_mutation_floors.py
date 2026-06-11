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
    # Measured 96.8. The residual survivors are equivalent: the dead
    # defensive clamp after the rank bisect (the index is always in
    # range; Python carries the same dead clamp), the greedy loops'
    # exact-equality boundary flips (a fractional step of exactly one
    # level prices identically to a whole step on both branches), and
    # the zero-level HP gates (a zero level contributes zero either
    # way), plus the budget loop's 1e-6 epsilon comparison, whose
    # equality case is a measure-zero input spending a budget of
    # exactly the epsilon on the curve's flat start. The cross-language
    # differential drives the same loops against the backend over the
    # real snapshots.
    "eo-services/src/character_calc.rs": 92.0,
    # Measured 97.1. The one residual survivor replaces a no-data
    # extractor's body (an empty map) with the default value, which is
    # the same empty map: equivalent by construction.
    "eo-services/src/chatlog_parser.rs": 92.0,
    # Measured 95.9. The residual survivors are equivalent in
    # behaviour: the unmapped-event filter is defensive parity with the
    # original's identically-dead check (every parsed type is mapped or
    # internally buffered), the pending-tick accessor cannot diverge a
    # drain because the stop path flushes either way, and the refund
    # tolerance's exact-equality flip needs a difference of exactly one
    # billionth, which the value grammar cannot produce.
    "eo-services/src/chatlog_watcher.rs": 92.0,
    # Measured 96.9. The residual survivors are equivalent or
    # environment-bound: the zero-or-positive guards on accumulator
    # adds flip to admitting an exact zero, which adds nothing either
    # way (shot damage, per-tool damage, the zero heal cost, the
    # zero fallback cost); the loot handler's session/accumulator
    # null-checks are invariant-coupled (the fields are set and
    # cleared together under the same guard); the enhancer break's
    # redistribute-on-empty arm equals the decrement loop's no-op on
    # empty stacks; the costless tool merge resolves the same bare
    # entry whichever comparison arm runs, because the Unknown bucket
    # cannot coexist with named entries; and the naive-local epoch
    # helper's DST-gap resolution arm is reachable only when the test
    # host's zone puts the instant inside a spring-forward gap, which
    # no deterministic test can arrange across CI hosts.
    "eo-services/src/tracker.rs": 92.0,
    # Measured 95.6. The two residual survivors are equivalent: a
    # zero known-kill total implies the mob rows were empty, which the
    # enclosing guard already excludes, and a zero shot total makes
    # the dominance share NaN, which fails the threshold exactly as
    # the guard's skip does.
    "eo-services/src/session_summary.rs": 92.0,
    # Measured 93.6. The residual survivors are equivalence-grade:
    # the score-update strictness and the equal-length second pass
    # admit no pinnable distinguishing pair (the alignment detail is
    # not exposed), the empty-side token guard reaches the same zero
    # through the set arithmetic, and the weighted scorer's
    # empty-input and equal-length guards coincide with the layered
    # guards beneath them. Every weighted score is pinned against the
    # original library (twelve curated cases plus direct sub-scorer
    # edges); a 300-case randomised sweep ran at zero divergences.
    "eo-services/src/fuzzy_match.rs": 92.0,
    # Measured 82.5 hermetically. Twenty-five of the thirty-seven
    # survivors are engine-instance mutants (the recognise entry
    # points and their input/output guards), killable only with a
    # live ONNX Runtime session: the ground-truth differential kills
    # them on hosts carrying the runtime and the locally-held bench,
    # which the campaign host and CI deliberately are not. Counting
    # those out, the hermetic surface scores 93.5; the resize is
    # additionally pinned byte-for-byte against the original image
    # library and the whole pipeline is held to the recorded
    # ground-truth rate by the differential.
    "eo-services/src/ocr_engine.rs": 82.0,
    # Measured 93.6. The residuals are the bar-fill estimate's
    # remaining tie arithmetic (the grey conversion is pinned through
    # colour-differentiated columns; vendor builds of the original's
    # image library deviate by one least-significant bit on rounding
    # ties, so finer pins would overfit) and its guard couplings.
    "eo-services/src/skill_panel.rs": 92.0,
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
