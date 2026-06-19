"""Deterministic authoring-discipline lint, scoped to newly authored lines.

Two mechanical authoring rules, enforced as cheap deterministic lint rather than
eyeballed in review:

- **No em dashes** (U+2014) in newly authored content (comments, in-code
  strings, docs, in-app copy).
- **UK spelling** in newly authored prose (docs and code comments).

Both rules are **diff-scoped**: they inspect only the lines a change *adds*,
never the whole tree. This is deliberate, and load-bearing. The tree carries
pre-existing US spellings and em dashes that predate the discipline; normalising
them drive-by is explicitly out of scope, so a whole-tree gate could not run
green without churning content it should not touch, and a lint floor only ever
ratchets up. Checking added lines only binds new content without forcing a sweep
of the old.

Scope differs by rule, for a reason:

- The em-dash ban applies to every added line in a non-exempt file. U+2014 is
  never part of code syntax, so an added em dash is always authored content (a
  comment, a string, prose).
- The UK-spelling check applies only to added lines in **prose contexts**:
  Markdown / plain-text docs, and comment-only lines in code. A blanket US->UK
  check over code would be unworkable, because ``color`` (CSS), ``behavior``
  (DOM API), ``center`` (CSS value), ``license`` (the ``package.json`` field),
  and ``serialize`` / ``initialize`` (ordinary identifiers) are legitimate
  US-spelled *code* tokens, not authoring slips. Restricting to prose keeps the
  check honest and false-positive-free; in-app copy in string literals stays
  under the em-dash net but is left to review for UK spelling.

Run against a commit range (CI) or the staged / working-tree diff (local
pre-commit), mirroring ``check_golden_ratification``::

    python -m backend.scripts.check_authoring_lint                 # staged vs HEAD
    python -m backend.scripts.check_authoring_lint --range origin/main..HEAD
    python -m backend.scripts.check_authoring_lint --warn-only     # never exit non-zero
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# The em dash, written as an escape rather than the literal glyph so this guard's
# own source does not trip the very rule it enforces.
EM_DASH = chr(0x2014)

# Repository-relative path patterns exempt from BOTH rules: the licence texts
# (whose canonical wording is fixed and may contain em dashes), vendored /
# dependency trees and their lockfiles (not authored here), and binary or
# generated artefacts (no meaningful authored lines). Matched against the
# POSIX-normalised path.
_EXEMPT_PATTERNS = (
    re.compile(r"(^|/)LICENSE(\.[^/]*)?$", re.IGNORECASE),
    re.compile(r"(^|/)THIRD-PARTY-NOTICES(\.[^/]*)?$", re.IGNORECASE),
    re.compile(r"(^|/)node_modules/"),
    re.compile(r"(^|/)package-lock\.json$"),
    re.compile(r"(^|/)Cargo\.lock$"),
    re.compile(r"\.db$", re.IGNORECASE),
    # Byte-verbatim third-party model assets (the OCR decode alphabet
    # legitimately contains em dashes as recognisable characters).
    re.compile(r"(^|/)backend/assets/models/"),
    # Generated, not hand-authored: the OpenAPI snapshot, the coverage matrix,
    # and the frontend API types generated from that snapshot (their doc
    # comments mirror the spec's description strings verbatim by design).
    re.compile(r"(^|/)backend/tests/expected/openapi\.snapshot\.json$"),
    re.compile(r"(^|/)backend/testing/COVERAGE\.md$"),
    re.compile(r"(^|/)frontend/src/lib/api/schema\.d\.ts$"),
    # Generated demo-output goldens: the curated demo endpoints' responses,
    # pinned byte-for-byte from the oracle and embedded via include_str! in the
    # demo golden test. They carry mock user-entered data (notes, descriptions)
    # that deliberately mimics real input (em dashes and all), so they are
    # captured fixtures, not hand-authored content.
    re.compile(r"(^|/)frontend/src-tauri/eo-http/resources/demo_goldens/"),
)

# Prose file types whose every line is authored prose for the UK-spelling check.
_PROSE_SUFFIXES = (".md", ".markdown", ".txt", ".rst")

# Leading tokens that mark an added line as a comment, used to extend the
# UK-spelling check into code comments without parsing each language. Trailing
# comments (``x = 1  # note``) and string literals are deliberately NOT matched:
# the conservative comment-only heuristic keeps the check free of false
# positives on code that merely contains a US-spelled identifier.
_COMMENT_PREFIXES = ("#", "//", "/*", "*", "<!--", "--", ";")

# Curated US -> UK spelling map. Deliberately conservative: it omits words that
# double as code, CSS, or config tokens (``color``/``center``/``license`` and
# the ``-ize`` identifier verbs such as ``serialize`` / ``initialize``), so even
# within a prose context a comment that references such an identifier is not
# flagged. The map is a floor, not a dictionary; extend it as real slips appear.
US_TO_UK = {
    "behavior": "behaviour",
    "behaviors": "behaviours",
    "behavioral": "behavioural",
    "customize": "customise",
    "customizes": "customises",
    "customized": "customised",
    "customizing": "customising",
    "customization": "customisation",
    "customizations": "customisations",
    "organize": "organise",
    "organizes": "organises",
    "organized": "organised",
    "organizing": "organising",
    "organization": "organisation",
    "organizations": "organisations",
    "analyze": "analyse",
    "analyzes": "analyses",
    "analyzed": "analysed",
    "analyzing": "analysing",
    "optimize": "optimise",
    "optimizes": "optimises",
    "optimized": "optimised",
    "optimizing": "optimising",
    "optimization": "optimisation",
    "optimizations": "optimisations",
    "recognize": "recognise",
    "recognized": "recognised",
    "recognizes": "recognises",
    "recognizing": "recognising",
    "summarize": "summarise",
    "summarized": "summarised",
    "summarizes": "summarises",
    "summarizing": "summarising",
    "minimize": "minimise",
    "minimized": "minimised",
    "minimizes": "minimises",
    "minimizing": "minimising",
    "maximize": "maximise",
    "maximized": "maximised",
    "maximizes": "maximises",
    "maximizing": "maximising",
    "prioritize": "prioritise",
    "prioritized": "prioritised",
    "prioritizes": "prioritises",
    "prioritizing": "prioritising",
    "emphasize": "emphasise",
    "emphasized": "emphasised",
    "emphasizes": "emphasises",
    "emphasizing": "emphasising",
    "categorize": "categorise",
    "categorized": "categorised",
    "categorizes": "categorises",
    "categorizing": "categorising",
    "capitalize": "capitalise",
    "capitalized": "capitalised",
    "capitalizes": "capitalises",
    "capitalizing": "capitalising",
    "catalog": "catalogue",
    "catalogs": "catalogues",
    "favor": "favour",
    "favors": "favours",
    "favored": "favoured",
    "favorite": "favourite",
    "favorites": "favourites",
    "honor": "honour",
    "honors": "honours",
    "honored": "honoured",
    "defense": "defence",
    "offense": "offence",
    "fulfill": "fulfil",
    "fulfills": "fulfils",
    "canceled": "cancelled",
    "canceling": "cancelling",
    "labeled": "labelled",
    "labeling": "labelling",
    "modeled": "modelled",
    "modeling": "modelling",
    "traveled": "travelled",
    "traveling": "travelling",
}

# One alternation over the US spellings, word-bounded and case-insensitive.
_US_SPELLING_RE = re.compile(
    r"\b(" + "|".join(sorted(US_TO_UK, key=len, reverse=True)) + r")\b",
    re.IGNORECASE,
)


def is_exempt(path: str) -> bool:
    """True when a repo-relative path is exempt from both authoring rules."""
    posix = path.replace("\\", "/")
    return any(pattern.search(posix) for pattern in _EXEMPT_PATTERNS)


def is_prose_context(path: str, line: str) -> bool:
    """True when an added line should be checked for UK spelling.

    Prose files (Markdown, plain text) are prose throughout; in code, only a
    comment-only line counts, so a US-spelled identifier in real code is never
    flagged.
    """
    posix = path.replace("\\", "/").lower()
    if posix.endswith(_PROSE_SUFFIXES):
        return True
    stripped = line.lstrip()
    return stripped.startswith(_COMMENT_PREFIXES)


@dataclass(frozen=True)
class AddedLine:
    """One line a diff adds: its file, new-file line number, and text."""

    path: str
    lineno: int
    text: str


@dataclass(frozen=True)
class Finding:
    """A single authoring-rule violation on an added line."""

    path: str
    lineno: int
    rule: str  # "em-dash" or "uk-spelling"
    detail: str


def _run_git(args: list[str], repo_root: Path) -> str:
    # Force UTF-8 decoding: git emits diff content as UTF-8, and the lines this
    # guard inspects carry non-ASCII characters (the em dash U+2014 is the whole
    # point). Relying on the platform default (cp1252 on Windows) would mangle
    # U+2014 into three bytes and the em-dash rule would silently never fire.
    result = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        check=True,
    )
    return result.stdout


_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")


def added_lines(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> list[AddedLine]:
    """Return the lines the diff adds, with their new-file line numbers.

    With ``commit_range`` the diff is that range; without it, the staged plus
    working-tree change against HEAD (the about-to-commit surface a pre-commit
    hook sees). Parsed from ``git diff -U0`` so only added lines are inspected,
    never surrounding context.
    """
    args = ["diff", "-U0", "--no-color"]
    args.append(commit_range if commit_range else "HEAD")
    diff = _run_git(args, repo_root)

    findings: list[AddedLine] = []
    current_path: str | None = None
    new_lineno = 0
    for raw in diff.splitlines():
        if raw.startswith("+++ "):
            target = raw[4:].strip()
            # "+++ b/path" for a tracked change; "+++ /dev/null" for a deletion.
            current_path = target[2:] if target.startswith("b/") else None
            continue
        if raw.startswith("@@"):
            match = _HUNK_RE.match(raw)
            if match:
                new_lineno = int(match.group(1))
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            if current_path is not None:
                findings.append(AddedLine(current_path, new_lineno, raw[1:]))
            new_lineno += 1
    return findings


# A Markdown inline-code span: a run of backticks closed by an equal run. Its
# contents are a code literal, not prose, so the UK-spelling check strips these
# before matching. This is what lets documentation cite a US-spelled code token
# (``the `behavior` DOM property``) without the doc tripping its own rule.
_INLINE_CODE_RE = re.compile(r"(`+)(?:(?!\1).)+\1")


def _strip_inline_code(text: str) -> str:
    """Blank out Markdown inline-code spans so only prose is spell-checked."""
    return _INLINE_CODE_RE.sub(" ", text)


def scan(lines: list[AddedLine]) -> list[Finding]:
    """Apply both authoring rules to a list of added lines."""
    findings: list[Finding] = []
    for line in lines:
        if is_exempt(line.path):
            continue
        if EM_DASH in line.text:
            findings.append(
                Finding(
                    path=line.path,
                    lineno=line.lineno,
                    rule="em-dash",
                    detail=(
                        "em dash (U+2014); use a colon, semicolon, parentheses, "
                        "or a comma instead"
                    ),
                )
            )
        if is_prose_context(line.path, line.text):
            prose = _strip_inline_code(line.text)
            for match in _US_SPELLING_RE.finditer(prose):
                us = match.group(1)
                uk = US_TO_UK[us.lower()]
                findings.append(
                    Finding(
                        path=line.path,
                        lineno=line.lineno,
                        rule="uk-spelling",
                        detail=f"US spelling '{us}'; use UK spelling '{uk}'",
                    )
                )
    return findings


def evaluate(
    repo_root: Path,
    *,
    commit_range: str | None = None,
) -> list[Finding]:
    """Inspect the diff and return every authoring-rule finding.

    Pure of any process exit: the CLI or a test turns the findings into an exit
    code or an assertion.
    """
    return scan(added_lines(repo_root, commit_range=commit_range))


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

    findings = evaluate(args.repo_root, commit_range=args.commit_range)

    if not findings:
        print(
            "check-authoring-lint: no em-dash or UK-spelling issues in the added lines."
        )
        return 0

    print(
        "check-authoring-lint: authoring-discipline issues on newly added "
        "lines.\n\n"
        "These rules apply only to lines this change adds; pre-existing content "
        "is out of scope. Fix the flagged lines (or, for the em-dash rule, "
        "reword with a colon / semicolon / parentheses / comma).\n",
        file=sys.stderr,
    )
    for finding in findings:
        print(
            f"  {finding.path}:{finding.lineno}: [{finding.rule}] {finding.detail}",
            file=sys.stderr,
        )

    if args.warn_only:
        print(
            "\ncheck-authoring-lint: --warn-only set; exiting 0 despite the "
            "findings above.",
            file=sys.stderr,
        )
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
