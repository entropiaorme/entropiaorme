"""Guard against silently ratifying a regression through the goldens workflow.

Several e2e suites assert against committed golden files (the per-scenario
event-stream fingerprint and DB-state snapshot, the per-endpoint HTTP-response
goldens, the OpenAPI spec snapshot, the ``pytest-regressions`` consistency
goldens) plus the generated service coverage matrix. Regenerating those files
re-ratifies whatever the pipeline currently produces, so a regression that has
crept into the production code can be locked in as the new "expected" output
simply by running the regeneration workflow and committing the result.

This guard makes such a ratification deliberate rather than accidental, and in
range mode (the pull-request gate) also tied to an independent sign-off:

- The goldens-regeneration commit-message marker (the ``test: regenerate
  goldens`` subject prefix documented in ``TESTING.md``) must appear on the
  relevant commit(s). This proves the ratification was *conscious*.
- In range mode it additionally requires a recorded independent-review verdict:
  a report committed to ``backend/testing/ratifications/<slug>.md`` in the same
  range, carrying an ``ORACLE-RATIFICATION`` block whose ``VERDICT`` is
  ``ratification-sound``. This supplies the judgement the marker cannot: that an
  independent party (not the change's author) confirmed the diff is a genuine
  behaviour change rather than a swept regression. The artefact must be added or
  modified *in the same range*, so a sound verdict from a prior regeneration
  cannot bless a fresh golden change. As an additional cross-check, the verdict's
  ``goldens:`` field must name every changed golden set, so a verdict recorded
  for one set cannot bless another.

The marker alone proves consciousness, not correctness; the recorded verdict is
the missing correctness sign-off. The artefact lives deliberately outside any
``expected/`` directory so the guard never classifies its own evidence as a
golden and recurses.

Staged / working-tree mode (the pre-commit invocation) stays marker-only and
advisory: there is no committed verdict to inspect yet, so it surfaces the
golden diff for review rather than blocking on a verdict it cannot see. The
range-mode pull-request gate is where the verdict requirement bites.

CI cannot prove the independent review actually happened; it can only
presence-check and parse the committed verdict. That residual gap is closed by
the report being reviewer-visible (CodeRabbit / a human) and by the human merge
to ``main``, not by this guard.

A change that touches no golden file is ignored entirely, so the guard is inert
for ordinary work.

Run it directly against a commit range or the staged / working-tree diff, or
import :func:`evaluate` to drive the same logic from a test::

    python -m backend.scripts.check_golden_ratification            # staged vs HEAD
    python -m backend.scripts.check_golden_ratification --staged   # staged vs HEAD
    python -m backend.scripts.check_golden_ratification --range origin/main..HEAD
    python -m backend.scripts.check_golden_ratification --warn-only # never exit non-zero
"""

from __future__ import annotations

import argparse
import contextlib
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# The documented goldens-regeneration commit-message marker (see the
# "Commit-message convention" section in TESTING.md). Matched case-insensitively
# against any line of the commit message, so the subject prefix
# ``test: regenerate goldens ...`` satisfies it; the trailing text naming the
# regenerated sets is free-form.
MARKER = re.compile(r"\btest:\s*regenerate\s+goldens\b", re.IGNORECASE)

# Repository-relative path prefixes / patterns that count as golden files. A
# committed change to any of these re-ratifies expected output, so it must carry
# the marker. Kept as a small, readable set of predicates rather than a single
# opaque regex so a reviewer can see exactly what is guarded.
_EXPECTED_SEGMENT = "/expected/"


def is_golden_path(path: str) -> bool:
    """True when a repository-relative path is a committed golden file.

    Covers the per-scenario goldens (anything under a ``backend/tests`` tree's
    ``expected/`` directory: the fingerprint, the DB-state snapshot, and the
    HTTP-response goldens), the OpenAPI spec snapshot, the
    ``pytest-regressions`` consistency goldens (the ``.yml`` files beside the
    ``test_consistency_*`` modules), and the generated coverage matrix.
    """
    posix = path.replace("\\", "/")
    if posix.startswith("backend/tests/") and _EXPECTED_SEGMENT in posix:
        return True
    if posix == "backend/tests/expected/openapi.snapshot.json":
        return True
    if posix.startswith("backend/tests/e2e/test_consistency_") and posix.endswith(
        ".yml"
    ):
        return True
    return posix == "backend/testing/COVERAGE.md"


# Where the author commits the independent ratification report alongside a
# golden change. Deliberately NOT under any ``expected/`` directory, so
# ``is_golden_path`` never classifies a report as a golden (which would make the
# guard demand a ratification artefact for its own ratification artefact).
_RATIFICATIONS_PREFIX = "backend/testing/ratifications/"


def is_ratification_artifact(path: str) -> bool:
    """True when a repo-relative path is a committed ratification report.

    Reports live under ``backend/testing/ratifications/`` as ``<slug>.md`` and
    carry the ``ORACLE-RATIFICATION`` verdict block the range-mode gate requires
    alongside a golden change.
    """
    posix = path.replace("\\", "/")
    return posix.startswith(_RATIFICATIONS_PREFIX) and posix.endswith(".md")


# The three verdict values a ratification report can carry; only
# ``ratification-sound`` satisfies the gate. Matched on its own line, beneath the
# ``ORACLE-RATIFICATION`` header, so prose elsewhere in the report that merely
# mentions a verdict value does not count as the verdict.
_VERDICT_VALUES = (
    "ratification-sound",
    "regression-suspected",
    "needs-user-judgement",
)
_HEADER_RE = re.compile(r"ORACLE-RATIFICATION\b", re.IGNORECASE)
_VERDICT_RE = re.compile(
    r"^[^\S\n]*VERDICT:[^\S\n]*(" + "|".join(_VERDICT_VALUES) + r")[^\S\n]*$",
    re.IGNORECASE | re.MULTILINE,
)
_GOLDENS_RE = re.compile(
    r"^[^\S\n]*goldens:[^\S\n]*(.+?)[^\S\n]*$", re.IGNORECASE | re.MULTILINE
)
_RANGE_FIELD_RE = re.compile(
    r"^[^\S\n]*range:[^\S\n]*(.+?)[^\S\n]*$", re.IGNORECASE | re.MULTILINE
)


@dataclass(frozen=True)
class RatificationVerdict:
    """A parsed ``ORACLE-RATIFICATION`` verdict block from one report."""

    path: str
    verdict: str
    goldens: tuple[str, ...]
    range: str | None

    @property
    def is_sound(self) -> bool:
        return self.verdict == "ratification-sound"


def parse_ratification_artifact(path: str, text: str) -> RatificationVerdict | None:
    """Parse the verdict block out of a ratification report, or ``None``.

    Returns ``None`` when the text carries no ``ORACLE-RATIFICATION`` header or
    no recognised ``VERDICT:`` line beneath it, so a malformed or placeholder
    report does not register as any verdict (and so cannot satisfy the gate).
    """
    header = _HEADER_RE.search(text)
    if not header:
        return None
    block = text[header.start() :]
    verdict_m = _VERDICT_RE.search(block)
    if not verdict_m:
        return None
    goldens_m = _GOLDENS_RE.search(block)
    goldens: tuple[str, ...] = ()
    if goldens_m:
        goldens = tuple(tok for tok in re.split(r"[,\s]+", goldens_m.group(1)) if tok)
    range_m = _RANGE_FIELD_RE.search(block)
    return RatificationVerdict(
        path=path,
        verdict=verdict_m.group(1).lower(),
        goldens=goldens,
        range=range_m.group(1).strip() if range_m else None,
    )


def golden_set_key(path: str) -> str:
    """A coarse identifier for the golden 'set' a changed golden path belongs to.

    Used by the optional cross-check that a sound verdict names every changed
    set. Scenario goldens key on their scenario directory; the standalone
    goldens (the OpenAPI snapshot, the consistency goldens, the coverage matrix)
    key on a fixed category token the report's ``goldens:`` field will name.
    """
    posix = path.replace("\\", "/")
    if posix == "backend/tests/expected/openapi.snapshot.json":
        return "openapi"
    if posix == "backend/testing/COVERAGE.md":
        return "coverage"
    if posix.startswith("backend/tests/e2e/test_consistency_"):
        return "consistency"
    scenario = re.search(r"/([^/]+)/expected/", posix)
    if scenario:
        return scenario.group(1)
    return posix


def _normalise(text: str) -> str:
    """Lowercase and strip non-alphanumerics, so ``basic-hunt`` == ``basic_hunt``."""
    return re.sub(r"[^a-z0-9]+", "", text.lower())


def _verdict_covers(verdict: RatificationVerdict, set_key: str) -> bool:
    """True when the verdict's ``goldens:`` field references ``set_key``.

    Comparison is alphanumeric-insensitive and bidirectional-substring, so a
    report naming ``basic_hunt`` (or ``basic-hunt``, or ``basic_hunt
    fingerprint.jsonl``) covers the ``basic_hunt`` scenario. A verdict that
    records no ``goldens:`` field covers nothing, so this cross-check can only
    ever tighten the gate, never silently pass an unnamed set.
    """
    key = _normalise(set_key)
    if not key:
        return False
    for token in verdict.goldens:
        norm = _normalise(token)
        if norm and (key in norm or norm in key):
            return True
    return False


@dataclass(frozen=True)
class Evaluation:
    """Outcome of inspecting a diff for unratified golden changes."""

    golden_paths: tuple[str, ...]
    has_marker: bool
    messages: tuple[str, ...]
    range_mode: bool
    ratification_artifacts: tuple[str, ...] = ()
    sound_verdicts: tuple[RatificationVerdict, ...] = ()
    unblessed_sets: tuple[str, ...] = ()

    @property
    def touches_goldens(self) -> bool:
        return bool(self.golden_paths)

    @property
    def has_sound_verdict(self) -> bool:
        return bool(self.sound_verdicts)

    @property
    def ok(self) -> bool:
        """A clean result against the ratification rule.

        No goldens touched is always clean. In staged / working-tree mode the
        rule is marker-only and advisory (no committed verdict exists yet to
        inspect). In range mode (the pull-request gate) a golden change must
        carry the marker AND a sound, in-range verdict that names every changed
        set.
        """
        if not self.touches_goldens:
            return True
        if not self.range_mode:
            return self.has_marker
        return self.has_marker and self.has_sound_verdict and not self.unblessed_sets


def _run_git(args: list[str], repo_root: Path) -> str:
    """Run a git command under ``repo_root`` and return its stdout."""
    result = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout


def changed_paths(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> list[str]:
    """Return the repository-relative paths changed by the diff under inspection.

    With ``commit_range`` (e.g. ``origin/main..HEAD``), the names come from
    ``git diff --name-only <range>``. Without it, the staged-plus-working-tree
    change against HEAD is used (``git diff --name-only HEAD``), which covers the
    about-to-commit case a pre-commit hook sees.
    """
    if commit_range:
        out = _run_git(["diff", "--name-only", commit_range], repo_root)
    else:
        out = _run_git(["diff", "--name-only", "HEAD"], repo_root)
    return [line for line in out.splitlines() if line.strip()]


def commit_messages(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> list[str]:
    """Return the commit message bodies relevant to the diff under inspection.

    For a ``commit_range`` the messages are every commit in the range; for the
    staged / working-tree case there is no commit yet, so the marker is sought in
    the prepared message file if a commit is in flight, falling back to the tip
    commit's message (the amend / fixup case). The caller treats an empty list as
    "no marker present".
    """
    if commit_range:
        out = _run_git(["log", "--format=%B%x00", commit_range], repo_root)
        return [block for block in out.split("\x00") if block.strip()]

    # No commit yet: a pre-commit hook can pass the prepared message via the
    # COMMIT_EDITMSG file; otherwise fall back to the tip commit (an amend that
    # is regenerating goldens in place).
    prepared = repo_root / ".git" / "COMMIT_EDITMSG"
    messages: list[str] = []
    if prepared.exists():
        text = prepared.read_text(encoding="utf-8", errors="replace").strip()
        if text:
            messages.append(text)
    if not messages:
        with contextlib.suppress(subprocess.CalledProcessError):
            messages.append(_run_git(["log", "-1", "--format=%B"], repo_root).strip())
    return [m for m in messages if m]


def _has_marker(messages: list[str]) -> bool:
    return any(MARKER.search(message) for message in messages)


def _range_tip(commit_range: str) -> str:
    """The right-hand side of a ``A..B`` / ``A...B`` range (a bare ref is itself).

    The ratifying content of an in-range artefact is its content at the tip of
    the range, so the verdict is tied to the range rather than to whatever the
    working tree happens to hold.
    """
    tip = re.split(r"\.\.\.?", commit_range)[-1].strip()
    return tip or "HEAD"


def _file_at(repo_root: Path, ref: str, path: str) -> str | None:
    """The text of ``path`` at ``ref``, or ``None`` if it does not exist there.

    A ``None`` result means the artefact was deleted by the range tip (so it is
    not an added/modified in-range artefact and does not count).
    """
    try:
        return _run_git(["show", f"{ref}:{path}"], repo_root)
    except subprocess.CalledProcessError:
        return None


def evaluate(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> Evaluation:
    """Inspect the diff and classify it against the ratification rule.

    Pure of any process exit: it returns an :class:`Evaluation` the caller (the
    CLI or a test) turns into an exit code or an assertion. In range mode it also
    locates the ratification artefacts changed in the range, parses their verdict
    blocks, and works out which (if any) changed golden sets a sound verdict
    fails to name.
    """
    paths = changed_paths(repo_root, commit_range=commit_range)
    goldens = tuple(sorted(p for p in paths if is_golden_path(p)))
    messages = commit_messages(repo_root, commit_range=commit_range)
    has_marker = _has_marker(messages)
    range_mode = commit_range is not None

    artifact_paths: tuple[str, ...] = ()
    sound_verdicts: tuple[RatificationVerdict, ...] = ()
    unblessed_sets: tuple[str, ...] = ()

    if commit_range is not None:
        tip = _range_tip(commit_range)
        found: list[str] = []
        verdicts: list[RatificationVerdict] = []
        for path in paths:
            if not is_ratification_artifact(path):
                continue
            text = _file_at(repo_root, tip, path)
            if text is None:
                continue  # deleted by the tip: not an added/modified artefact
            found.append(path)
            verdict = parse_ratification_artifact(path, text)
            if verdict is not None:
                verdicts.append(verdict)
        artifact_paths = tuple(sorted(found))
        sound_verdicts = tuple(v for v in verdicts if v.is_sound)
        if goldens and sound_verdicts:
            keys = {golden_set_key(g) for g in goldens}
            unblessed_sets = tuple(
                sorted(
                    key
                    for key in keys
                    if not any(_verdict_covers(v, key) for v in sound_verdicts)
                )
            )

    return Evaluation(
        golden_paths=goldens,
        has_marker=has_marker,
        messages=tuple(messages),
        range_mode=range_mode,
        ratification_artifacts=artifact_paths,
        sound_verdicts=sound_verdicts,
        unblessed_sets=unblessed_sets,
    )


def _golden_diff(
    repo_root: Path,
    paths: tuple[str, ...],
    *,
    commit_range: str | None = None,
) -> str:
    """Return the unified diff for the golden paths, for the review surface."""
    if not paths:
        return ""
    args = ["diff"]
    if commit_range:
        args.append(commit_range)
    else:
        args.append("HEAD")
    args.append("--")
    args.extend(paths)
    try:
        return _run_git(args, repo_root)
    except subprocess.CalledProcessError:
        return ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--range",
        dest="commit_range",
        default=None,
        help=(
            "inspect this commit range (e.g. origin/main..HEAD); default is the "
            "staged / working-tree diff against HEAD"
        ),
    )
    parser.add_argument(
        "--staged",
        action="store_true",
        help="inspect the staged / working-tree diff against HEAD (the default)",
    )
    parser.add_argument(
        "--warn-only",
        action="store_true",
        help="hard-warn on the review surface but always exit 0",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to inspect (default: the project root)",
    )
    args = parser.parse_args(argv)

    if args.staged and args.commit_range:
        parser.error("--staged and --range are mutually exclusive")

    result = evaluate(args.repo_root, commit_range=args.commit_range)

    if not result.touches_goldens:
        print("check-golden-ratification: no golden files touched; nothing to guard.")
        return 0

    if result.ok:
        if result.range_mode:
            print(
                "check-golden-ratification: golden change carries the "
                "'test: regenerate goldens' marker and a recorded independent "
                "'ratification-sound' verdict naming the changed sets; "
                "ratification is deliberate and signed off."
            )
            print("Goldens:")
            for path in result.golden_paths:
                print(f"  {path}")
            print("Ratification artefacts:")
            for path in result.ratification_artifacts:
                print(f"  {path}")
        else:
            print(
                "check-golden-ratification: golden change carries the "
                "'test: regenerate goldens' marker; ratification is deliberate "
                "(staged / working-tree mode is marker-only; the independent "
                "ratification verdict is enforced by the pull-request gate). "
                "Files:"
            )
            for path in result.golden_paths:
                print(f"  {path}")
        return 0

    listing = "\n".join(f"  {path}" for path in result.golden_paths)
    diff = _golden_diff(
        args.repo_root, result.golden_paths, commit_range=args.commit_range
    )
    if not result.has_marker:
        print(
            "check-golden-ratification: golden files changed without the "
            "documented 'test: regenerate goldens' commit-message marker.\n\n"
            "Regenerating a golden re-ratifies whatever the pipeline currently "
            "produces, so an unmarked golden change can silently lock in a "
            "regression. Either:\n"
            "  - this is a deliberate re-ratification: record it with a commit "
            "whose subject is 'test: regenerate goldens ...' naming the "
            "regenerated sets (see TESTING.md), or\n"
            "  - this is an unintended golden move: revert it and fix the "
            "underlying change so the goldens hold.\n\n"
            f"Changed golden files:\n{listing}\n",
            file=sys.stderr,
        )
    elif not result.has_sound_verdict:
        print(
            "check-golden-ratification: golden files changed with the "
            "'test: regenerate goldens' marker, but no independent "
            "'ratification-sound' verdict was recorded in this range.\n\n"
            "The marker proves the regeneration was conscious; it cannot prove "
            "the diff is correct. An independent reviewer (not the change's "
            "author) must review the golden diff, and the resulting report "
            "(carrying the ORACLE-RATIFICATION ... VERDICT: ratification-sound "
            "block) must be committed to backend/testing/ratifications/<slug>.md "
            "in the same range as the golden change. A verdict artefact from a "
            "prior regeneration does not count: it must be added or modified in "
            "this range.\n\n"
            f"Changed golden files:\n{listing}\n",
            file=sys.stderr,
        )
    else:
        unblessed = "\n".join(f"  {key}" for key in result.unblessed_sets)
        print(
            "check-golden-ratification: a 'ratification-sound' verdict is "
            "present, but it does not name every changed golden set. A verdict "
            "recorded for one set cannot bless another, so each changed set must "
            "appear in the verdict's 'goldens:' field.\n\n"
            f"Golden sets without a matching sound verdict:\n{unblessed}\n\n"
            f"Changed golden files:\n{listing}\n",
            file=sys.stderr,
        )
    if diff:
        print("Golden diff for review:\n", file=sys.stderr)
        print(diff, file=sys.stderr)

    if args.warn_only:
        print(
            "check-golden-ratification: --warn-only set; exiting 0 despite the "
            "unratified golden change.",
            file=sys.stderr,
        )
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
