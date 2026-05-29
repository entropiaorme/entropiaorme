"""Guard against silently ratifying a regression through the goldens workflow.

Several e2e suites assert against committed golden files (the per-scenario
event-stream fingerprint and DB-state snapshot, the per-endpoint HTTP-response
goldens, the OpenAPI spec snapshot, the ``pytest-regressions`` consistency
goldens) plus the generated service coverage matrix. Regenerating those files
re-ratifies whatever the pipeline currently produces, so a regression that has
crept into the production code can be locked in as the new "expected" output
simply by running the regeneration workflow and committing the result.

This guard makes such a ratification deliberate rather than accidental: when a
change touches one or more golden files, it requires the goldens-regeneration
commit-message marker (the ``test: regenerate goldens`` subject prefix
documented in ``TESTING.md``) on the relevant commit(s). A goldens change that
carries the marker passes (the diff is reviewed like any other change); a
goldens change that lacks it fails (or hard-warns), surfacing the golden diff
for deliberate review before it can land.

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


@dataclass(frozen=True)
class Evaluation:
    """Outcome of inspecting a diff for unratified golden changes."""

    golden_paths: tuple[str, ...]
    has_marker: bool
    messages: tuple[str, ...]

    @property
    def touches_goldens(self) -> bool:
        return bool(self.golden_paths)

    @property
    def ok(self) -> bool:
        """A clean result: no goldens touched, or touched with the marker."""
        return (not self.touches_goldens) or self.has_marker


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


def evaluate(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> Evaluation:
    """Inspect the diff and classify it against the ratification rule.

    Pure of any process exit: it returns an :class:`Evaluation` the caller (the
    CLI or a test) turns into an exit code or an assertion.
    """
    paths = changed_paths(repo_root, commit_range=commit_range)
    goldens = tuple(sorted(p for p in paths if is_golden_path(p)))
    messages = commit_messages(repo_root, commit_range=commit_range)
    return Evaluation(
        golden_paths=goldens,
        has_marker=_has_marker(messages),
        messages=tuple(messages),
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
        print(
            "check-golden-ratification: golden change carries the "
            "'test: regenerate goldens' marker; ratification is deliberate. "
            "Files:"
        )
        for path in result.golden_paths:
            print(f"  {path}")
        return 0

    listing = "\n".join(f"  {path}" for path in result.golden_paths)
    diff = _golden_diff(
        args.repo_root, result.golden_paths, commit_range=args.commit_range
    )
    print(
        "check-golden-ratification: golden files changed without the documented "
        "'test: regenerate goldens' commit-message marker.\n\n"
        "Regenerating a golden re-ratifies whatever the pipeline currently "
        "produces, so an unmarked golden change can silently lock in a "
        "regression. Either:\n"
        "  - this is a deliberate re-ratification: record it with a commit whose "
        "subject is 'test: regenerate goldens ...' naming the regenerated sets "
        "(see TESTING.md), or\n"
        "  - this is an unintended golden move: revert it and fix the underlying "
        "change so the goldens hold.\n\n"
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
