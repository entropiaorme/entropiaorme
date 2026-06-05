"""Backend determinism guard: the no-ambient-time lint.

One whole-tree rule over the backend production source: no module under the
production packages may read the ambient clock. Every wall-clock or monotonic
read must flow through the injected ``backend.testing.clock.Clock`` seam
(constructed once at the composition root in ``backend/main.py``), so replay
scenarios can drive deterministic instants through every output-reaching
timestamp. A golden-stability check alone cannot enforce this: a clock-coupled
surface that carries no golden passes vacuously, so this static zero-site
assertion is the enforcement.

Forbidden reads (matched on the AST, so strings and comments never
false-positive, and ``import time as _time``-style aliases are resolved):

- ``time.time()``, ``time.monotonic()``, ``time.perf_counter()`` (and
  importing those callables directly via ``from time import ...``);
- ``datetime.now()`` / ``datetime.utcnow()`` on the class or module path
  (including ``default_factory=datetime.now``-style bare references);
- ``date.today()``.

``time.sleep`` is deliberately NOT forbidden: sleeping schedules work but
produces no value, so it cannot leak wall-clock state into an output.

A site with a genuinely justified ambient read carries a same-line pragma
``# ambient-time: allowed (<reason>)`` with a non-empty reason; the guard
fails on a bare pragma. The tree ships with zero pragmas.

This lint is WHOLE-TREE rather than diff-scoped: the tree was driven to zero
ambient reads, so the guarantee is "zero anywhere", not merely "no new ones".
The source set is the ``git ls-files``-tracked ``.py`` files under the
production packages (tracked-only and deterministic).

Stdlib-only by design. Run from the repo root::

    python -m backend.scripts.check_ambient_time
    python -m backend.scripts.check_ambient_time --warn-only
"""

from __future__ import annotations

import argparse
import ast
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# Production packages under the zero-ambient-reads guarantee. The testing
# package is excluded by construction: it is where the Clock seam itself
# (RealClock's stdlib delegation) legitimately lives.
SCAN_ROOTS = (
    "backend/services",
    "backend/routers",
    "backend/core",
    "backend/tracking",
    "backend/db",
    "backend/main.py",
    "backend/dependencies.py",
)

# (module, attribute) tails that constitute an ambient clock read. The tail is
# matched against the resolved dotted chain, so ``datetime.datetime.now`` and
# ``dt.datetime.now`` (module alias) both reduce to ``('datetime', 'now')``.
_FORBIDDEN_TAILS = {
    ("time", "time"),
    ("time", "monotonic"),
    ("time", "perf_counter"),
    ("datetime", "now"),
    ("datetime", "utcnow"),
    ("date", "today"),
}

# Callables that must not be imported directly off the ``time`` module: a
# ``from time import monotonic`` would otherwise hide the read behind a bare
# name the attribute scan cannot see.
_FORBIDDEN_TIME_IMPORTS = {"time", "monotonic", "perf_counter"}

# Same-line escape hatch. The reason is mandatory: a pragma without one fails.
_PRAGMA_RE = re.compile(r"#\s*ambient-time:\s*allowed\s*\((?P<reason>[^)]*)\)")


@dataclass(frozen=True)
class Finding:
    """A single guard violation: file, 1-based line number, detail."""

    path: str
    lineno: int
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
    """Repo-relative tracked ``.py`` paths under the production packages."""
    out = _run_git(["ls-files", "--", *SCAN_ROOTS], repo_root)
    return [line for line in out.splitlines() if line.endswith(".py")]


def _dotted_chain(node: ast.AST) -> list[str] | None:
    """Resolve an ``ast.Attribute`` chain into its dotted name parts.

    ``a.b.c`` -> ``['a', 'b', 'c']``; returns None when the chain roots in a
    call or subscript (``x().now`` is not a module/class clock read).
    """
    parts: list[str] = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
        parts.reverse()
        return parts
    return None


def scan_source(path: str, text: str) -> list[Finding]:
    """Apply the no-ambient-time rule to one file's source text."""
    posix = path.replace("\\", "/")
    try:
        tree = ast.parse(text)
    except SyntaxError as exc:  # a broken production file must fail loudly
        return [Finding(posix, exc.lineno or 0, f"unparseable source: {exc.msg}")]

    lines = text.splitlines()

    # Local aliases for the time/datetime modules and for the datetime/date
    # classes, so aliased reads (``import time as _time``;
    # ``from datetime import datetime as dt``) resolve to their true tails.
    module_aliases: dict[str, str] = {}
    class_aliases: dict[str, str] = {}

    findings: list[Finding] = []

    def _allowed(lineno: int) -> str | None:
        """Return the pragma reason when the line carries a valid pragma."""
        if 1 <= lineno <= len(lines):
            match = _PRAGMA_RE.search(lines[lineno - 1])
            if match:
                return match.group("reason").strip()
        return None

    def _flag(lineno: int, detail: str) -> None:
        reason = _allowed(lineno)
        if reason == "":
            findings.append(
                Finding(
                    posix,
                    lineno,
                    "ambient-time pragma carries no reason; justify the "
                    "exception inside the parentheses",
                )
            )
        elif reason is None:
            findings.append(Finding(posix, lineno, detail))

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name in ("time", "datetime"):
                    module_aliases[alias.asname or alias.name] = alias.name
        elif isinstance(node, ast.ImportFrom):
            if node.module == "time":
                for alias in node.names:
                    if alias.name in _FORBIDDEN_TIME_IMPORTS:
                        _flag(
                            node.lineno,
                            f"'from time import {alias.name}' imports an ambient "
                            "clock read; inject a Clock instead",
                        )
            elif node.module == "datetime":
                for alias in node.names:
                    if alias.name in ("datetime", "date"):
                        class_aliases[alias.asname or alias.name] = alias.name

    for node in ast.walk(tree):
        if not isinstance(node, ast.Attribute):
            continue
        chain = _dotted_chain(node)
        if chain is None or len(chain) < 2:
            continue
        # Resolve a module alias at the chain root (``_time.monotonic`` ->
        # ``time.monotonic``; ``dt.datetime.now`` -> ``datetime.datetime.now``).
        root = chain[0]
        if root in module_aliases:
            chain = [module_aliases[root], *chain[1:]]
        elif root in class_aliases:
            chain = [class_aliases[root], *chain[1:]]
        tail = (chain[-2], chain[-1])
        if tail in _FORBIDDEN_TAILS and chain[0] in ("time", "datetime", "date"):
            # The root must resolve to the canonical module/class name: an
            # arbitrary object chain that merely ends in a forbidden pair
            # (``record.date.today``) is not a clock read. Aliased roots were
            # already rewritten to their canonical names above.
            _flag(
                node.lineno,
                f"ambient clock read '{'.'.join(chain)}'; route it through the "
                "injected backend.testing.clock.Clock",
            )

    return findings


def evaluate(repo_root: Path) -> list[Finding]:
    """Scan the tracked production source and return every finding."""
    findings: list[Finding] = []
    for path in tracked_sources(repo_root):
        try:
            text = (repo_root / path).read_text(encoding="utf-8", errors="replace")
        except FileNotFoundError:
            # `git ls-files` enumerates the index, so a tracked file deleted
            # from the working tree is legitimately absent and carries no
            # live content to scan.
            continue
        findings.extend(scan_source(path, text))
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
            "check-ambient-time: no ambient clock reads in the backend "
            "production source."
        )
        return 0

    print(
        "check-ambient-time: ambient clock reads in production code.\n\n"
        "Every wall-clock or monotonic read must flow through the injected "
        "backend.testing.clock.Clock (constructed at the composition root) "
        "so replayed scenarios stay deterministic. Offenders:\n",
        file=sys.stderr,
    )
    for finding in findings:
        print(f"  {finding.path}:{finding.lineno}: {finding.detail}", file=sys.stderr)

    if args.warn_only:
        print(
            "\ncheck-ambient-time: --warn-only set; exiting 0 despite the "
            "findings above.",
            file=sys.stderr,
        )
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
