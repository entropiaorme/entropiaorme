"""Frontend polling/orphan-regrowth guard: the no-bare-setInterval lint.

Two whole-tree rules over the frontend source, enforcing the visibility-aware
polling discipline so the hidden-window-polling smell (and the retired
window-to-window tracking event) cannot grow back:

- **Rule A (single-home setInterval):** the raw ``setInterval(`` token may
  appear ONLY in the sanctioned helper module
  ``frontend/src/lib/realtime/useVisiblePoll.ts``. Every other timer-driven loop
  must route through ``useVisiblePoll`` (or its ``windowGeometryPoll`` variant),
  which clears the timer while its surface is hidden.
- **Rule B (no legacy lifecycle event):** the string ``tracking-state-changed``
  must not appear anywhere in the frontend source. That window-to-window event
  was retired in favour of the typed ``tracking:session:updated`` topic; a
  re-introduction is a regression.

This lint is WHOLE-TREE rather than diff-scoped: the tree was driven to zero
offending sites, so the guarantee is "zero anywhere", not merely "no new ones".
The source set is the ``git ls-files``-tracked compiled-source files (``.svelte``,
``.ts``, and the ``.js`` family) under ``frontend/src`` (tracked-only and
deterministic, never descending into ``node_modules`` or build output).

Stdlib-only by design, so CI's pre-commit job runs it without the project
virtual environment. Run from the repo root::

    python -m backend.scripts.check_no_bare_setinterval
    python -m backend.scripts.check_no_bare_setinterval --warn-only
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# The sole module permitted to hold a raw setInterval: the visibility-gated
# polling helper that every other timer must route through.
SETINTERVAL_HOME = "frontend/src/lib/realtime/useVisiblePoll.ts"

# A bare timer call. Tolerates whitespace before the paren (``setInterval (``),
# which is valid JS that no formatter in the toolchain would normalise away; the
# leading word boundary still matches a qualified ``window.setInterval(`` form.
SETINTERVAL_RE = re.compile(r"\bsetInterval\s*\(")

# The retired window-to-window tracking lifecycle event, superseded by the typed
# ``tracking:session:updated`` topic. Must not reappear in the frontend.
LEGACY_EVENT = "tracking-state-changed"

# Scanned source: every tracked compiled-source file under the frontend tree.
# Covers what Vite / SvelteKit bundle, so a poll cannot hide in a .js-family
# module that a .svelte/.ts-only scan would miss.
_SCAN_ROOT = "frontend/src"
_SCAN_SUFFIXES = (".svelte", ".ts", ".js", ".mjs", ".cjs", ".jsx", ".tsx")


@dataclass(frozen=True)
class Finding:
    """A single lint violation: file, 1-based line number, rule, detail."""

    path: str
    lineno: int
    rule: str  # "bare-setinterval" or "legacy-event"
    detail: str


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


def tracked_sources(repo_root: Path) -> list[str]:
    """Repo-relative tracked compiled-source paths under ``frontend/src``.

    ``git ls-files`` is the enumeration, so the scan is tracked-only and
    deterministic and never descends into ``node_modules`` or build artefacts.
    """
    out = _run_git(["ls-files", "--", _SCAN_ROOT], repo_root)
    return [line for line in out.splitlines() if line.endswith(_SCAN_SUFFIXES)]


def scan_text(path: str, text: str) -> list[Finding]:
    """Apply both whole-tree rules to one file's text."""
    findings: list[Finding] = []
    posix = path.replace("\\", "/")
    is_home = posix == SETINTERVAL_HOME
    for lineno, line in enumerate(text.splitlines(), start=1):
        if not is_home and SETINTERVAL_RE.search(line):
            findings.append(
                Finding(
                    path=posix,
                    lineno=lineno,
                    rule="bare-setinterval",
                    detail=(
                        f"bare setInterval outside {SETINTERVAL_HOME}; route the "
                        "poll through useVisiblePoll (or windowGeometryPoll)"
                    ),
                )
            )
        if LEGACY_EVENT in line:
            findings.append(
                Finding(
                    path=posix,
                    lineno=lineno,
                    rule="legacy-event",
                    detail=(
                        f"reference to the retired '{LEGACY_EVENT}' event; use the "
                        "typed 'tracking:session:updated' topic instead"
                    ),
                )
            )
    return findings


def evaluate(repo_root: Path) -> list[Finding]:
    """Scan the tracked frontend source and return every finding.

    Pure of any process exit: the CLI or a test turns findings into an exit code
    or an assertion.
    """
    findings: list[Finding] = []
    for path in tracked_sources(repo_root):
        try:
            text = (repo_root / path).read_text(encoding="utf-8", errors="replace")
        except FileNotFoundError:
            # `git ls-files` enumerates the index, so a tracked file deleted
            # from the working tree (an unstaged deletion mid-edit) is
            # legitimately absent on disk and carries no live content to scan.
            continue
        # Any other read failure (permissions, I/O) must fail loudly: a guard
        # that silently skips an unreadable source can return a false clean.
        findings.extend(scan_text(path, text))
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--warn-only",
        action="store_true",
        help="print findings but always exit 0",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to inspect (default: the project root)",
    )
    args = parser.parse_args(argv)

    findings = evaluate(args.repo_root)

    if not findings:
        print(
            "check-no-bare-setinterval: no bare setInterval or retired "
            "tracking-event references in the frontend source."
        )
        return 0

    print(
        "check-no-bare-setinterval: frontend polling-discipline violations.\n\n"
        f"Every timer-driven loop must route through {SETINTERVAL_HOME} "
        "(useVisiblePoll / windowGeometryPoll), and the retired "
        f"'{LEGACY_EVENT}' event must not reappear. Offenders:\n",
        file=sys.stderr,
    )
    for finding in findings:
        print(
            f"  {finding.path}:{finding.lineno}: [{finding.rule}] {finding.detail}",
            file=sys.stderr,
        )

    if args.warn_only:
        print(
            "\ncheck-no-bare-setinterval: --warn-only set; exiting 0 despite the "
            "findings above.",
            file=sys.stderr,
        )
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
