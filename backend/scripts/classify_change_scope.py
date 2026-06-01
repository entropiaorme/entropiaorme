"""Classify a change as documentation-only or code, to gate the expensive CI jobs.

The per-pull-request CI gate runs a Windows backend test matrix and a frontend
build, and a labelled full-tier suite gates the merge. A change that touches only
documentation (Markdown) needs none of that: there is no code to test and no
frontend to build. This guard inspects the set of files a change touches and
emits a single ``code`` flag that the workflows read to decide whether to run
those jobs.

The flag is deliberately conservative. ``code=false`` (documentation-only: skip
the expensive jobs) is emitted only when *every* changed path is a Markdown file.
Any other path (source, tests, configuration, the workflow files themselves, a
lockfile, an image) yields ``code=true`` and the full gate runs. The safe failure
direction is to run the suite, so an empty change set, a non-pull-request event,
and any classification doubt all resolve to ``code=true``.

The flag gates *required* checks, so the workflows pair it with a fail-closed
aggregator: a documentation-only skip passes the gate, but a detection that did
not run cleanly fails it, so a misfire can never let an untested code change
through.

Run from a workflow on a pull request, reading the range from the event env::

    EVENT_NAME=pull_request PR_BASE_SHA=<base> PR_HEAD_SHA=<head> \\
        python -m backend.scripts.classify_change_scope

or locally against an explicit range::

    python -m backend.scripts.classify_change_scope --range origin/main..HEAD

It writes ``code=true`` / ``code=false`` to the ``GITHUB_OUTPUT`` file when that
environment variable is set (so a workflow step exposes it as a job output), and
always prints the verdict for the run log.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# Repository-relative path suffixes that count as documentation. A change whose
# every touched path matches one of these needs neither the backend test matrix
# nor the frontend build. Intentionally minimal (only Markdown): anything else,
# including plain-text notes, images, and the workflow files, is treated as code
# so the suite runs, which is the safe direction.
_DOCS_SUFFIXES = (".md",)


def is_docs_path(path: str) -> bool:
    """True when a repo-relative path is documentation (Markdown)."""
    return path.replace("\\", "/").lower().endswith(_DOCS_SUFFIXES)


def is_docs_only(paths: list[str]) -> bool:
    """True when a change touches at least one path and all of them are docs.

    An empty set is deliberately *not* documentation-only: a change with no
    detectable files resolves to code, so the suite runs rather than being
    skipped on a degenerate diff.
    """
    return bool(paths) and all(is_docs_path(p) for p in paths)


def _run_git(args: list[str], repo_root: Path) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        check=True,
    )
    return result.stdout


def changed_paths(repo_root: Path, commit_range: str) -> list[str]:
    """The repo-relative paths a commit range touches (added / modified / deleted)."""
    out = _run_git(["diff", "--name-only", commit_range], repo_root)
    return [line.strip() for line in out.splitlines() if line.strip()]


def classify(repo_root: Path, *, commit_range: str | None) -> bool:
    """Return True when the expensive CI jobs should run (the change is code).

    A pull-request range is classified by its file set; without a range (a push
    or any non-pull-request event, or a pull request that did not supply both
    SHAs) the jobs run unconditionally.
    """
    if commit_range is None:
        return True
    return not is_docs_only(changed_paths(repo_root, commit_range))


def _range_from_env() -> str | None:
    """Derive the pull request's base..head range from the workflow event env.

    Returns ``None`` for any non-pull-request event, or a pull request missing
    either SHA, so the caller runs the jobs unconditionally.
    """
    if os.environ.get("EVENT_NAME") != "pull_request":
        return None
    base = os.environ.get("PR_BASE_SHA")
    head = os.environ.get("PR_HEAD_SHA")
    if base and head:
        return f"{base}..{head}"
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--range",
        dest="commit_range",
        default=None,
        help=(
            "classify this commit range (e.g. origin/main..HEAD); default is the "
            "pull-request range read from the EVENT_NAME / PR_BASE_SHA / "
            "PR_HEAD_SHA environment variables"
        ),
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to inspect (default: the project root)",
    )
    args = parser.parse_args(argv)

    commit_range = args.commit_range or _range_from_env()
    code = classify(args.repo_root, commit_range=commit_range)
    value = "true" if code else "false"

    if commit_range is None:
        print(
            "classify-change-scope: no pull-request range to inspect; "
            f"code={value} (run the full gate)."
        )
    else:
        print(f"classify-change-scope: range {commit_range}; code={value}.")

    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(f"code={value}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
