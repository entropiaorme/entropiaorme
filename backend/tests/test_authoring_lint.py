"""Tests for the diff-scoped authoring-discipline lint.

The guard (``backend/scripts/check_authoring_lint.py``) enforces two mechanical
authoring rules on newly added lines only: no em dashes (U+2014) in authored
content, and UK spelling in authored prose. The scope is the point, so these
tests pin it precisely:

- the em-dash ban covers every added line in a non-exempt file;
- the UK-spelling check fires only in prose contexts (docs and comment-only
  lines), never on a US-spelled identifier in real code;
- both rules see only the lines a change *adds*: a pre-existing em dash or US
  spelling in an untouched part of an edited file is never re-flagged.

The pure ``scan`` tests assert the rule logic directly; the git-backed tests
drive the diff parser end to end against a throwaway repository.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from backend.scripts import check_authoring_lint as lint
from backend.scripts.check_authoring_lint import AddedLine

EM = lint.EM_DASH


def _git(repo: Path, *args: str) -> str:
    env = {
        "GIT_AUTHOR_NAME": "Test",
        "GIT_AUTHOR_EMAIL": "test@example.invalid",
        "GIT_COMMITTER_NAME": "Test",
        "GIT_COMMITTER_EMAIL": "test@example.invalid",
        "GIT_CONFIG_GLOBAL": str(repo / ".gitconfig-none"),
        "GIT_CONFIG_SYSTEM": str(repo / ".gitconfig-none"),
    }
    result = subprocess.run(
        ["git", *args],
        cwd=repo,
        capture_output=True,
        text=True,
        check=True,
        env={**os.environ, **env},
    )
    return result.stdout


def _write(repo: Path, rel: str, content: str) -> None:
    target = repo / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    _git(root, "init", "-q", "-b", "main")
    _write(root, "README.md", "# Project\n\nClean baseline prose.\n")
    _write(root, "backend/app.py", "VALUE = 1\n")
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "base")
    return root


# --- the path / context predicates (pure, no git) -------------------------


@pytest.mark.parametrize(
    "path",
    [
        "LICENSE",
        "LICENSE.md",
        "THIRD-PARTY-NOTICES.md",
        "frontend/node_modules/pkg/index.js",
        "frontend/package-lock.json",
        "data/demo/entropia_orme.db",
        "backend/tests/expected/openapi.snapshot.json",
        "backend/testing/COVERAGE.md",
        "backend\\testing\\COVERAGE.md",
    ],
)
def test_exempt_paths(path: str) -> None:
    assert lint.is_exempt(path)


@pytest.mark.parametrize(
    "path",
    [
        "README.md",
        "backend/services/cost_engine.py",
        "frontend/src/app.css",
        "TESTING.md",
    ],
)
def test_non_exempt_paths(path: str) -> None:
    assert not lint.is_exempt(path)


def test_prose_context_for_docs_and_comments() -> None:
    assert lint.is_prose_context("README.md", "any text at all")
    assert lint.is_prose_context("notes.txt", "plain prose")
    assert lint.is_prose_context("backend/app.py", "# a python comment")
    assert lint.is_prose_context("frontend/x.ts", "// a ts comment")
    assert lint.is_prose_context("frontend/x.svelte", "<!-- markup comment -->")
    # Real code (not a comment) in a code file is NOT a prose context.
    assert not lint.is_prose_context("backend/app.py", "color = compute()")
    assert not lint.is_prose_context("frontend/x.ts", "const behavior = 'smooth'")


# --- the scan logic (pure, synthetic AddedLine lists) ---------------------


def test_em_dash_flagged_in_any_non_exempt_file() -> None:
    findings = lint.scan([AddedLine("backend/app.py", 10, f"x = 1  # note {EM} aside")])
    assert len(findings) == 1
    assert findings[0].rule == "em-dash"
    assert findings[0].lineno == 10


def test_em_dash_not_flagged_in_exempt_file() -> None:
    assert lint.scan([AddedLine("LICENSE", 1, f"a {EM} b")]) == []


def test_uk_spelling_flagged_in_markdown() -> None:
    findings = lint.scan([AddedLine("README.md", 3, "We customize the behavior here.")])
    rules = {f.rule for f in findings}
    details = " ".join(f.detail for f in findings)
    assert rules == {"uk-spelling"}
    assert "customise" in details
    assert "behaviour" in details


def test_uk_spelling_flagged_in_code_comment() -> None:
    findings = lint.scan([AddedLine("backend/app.py", 5, "# normalize then organize")])
    # `organize` is in the curated map; the identifier-style `normalize` is not.
    details = [f.detail for f in findings if f.rule == "uk-spelling"]
    assert any("organise" in d for d in details)
    assert all("normalise" not in d for d in details)


def test_uk_spelling_skips_inline_code_spans() -> None:
    # A US-spelled token inside backticks is a code literal, not prose, so a doc
    # can cite it without tripping the rule.
    assert lint.scan([AddedLine("README.md", 1, "The `behavior` prop is fine.")]) == []
    # The same token in bare prose is a real slip.
    flagged = lint.scan([AddedLine("README.md", 1, "The behavior is wrong.")])
    assert any(f.rule == "uk-spelling" for f in flagged)
    # reST-style double-backtick spans are stripped too.
    assert lint.scan([AddedLine("notes.md", 1, "Use ``organize`` with care.")]) == []


def test_uk_spelling_not_flagged_in_real_code() -> None:
    # A US-spelled identifier in actual code (not a comment) must not be flagged.
    assert lint.scan([AddedLine("frontend/x.ts", 9, "const behavior = 'smooth'")]) == []
    assert lint.scan([AddedLine("frontend/x.css", 2, "color: red;")]) == []


def test_clean_line_has_no_findings() -> None:
    assert lint.scan([AddedLine("README.md", 1, "Perfectly fine British prose.")]) == []


# --- end to end through git -----------------------------------------------


def test_added_em_dash_and_spelling_fail(repo: Path, capsys) -> None:
    _write(
        repo,
        "README.md",
        f"# Project\n\nClean baseline prose.\nWe customize it {EM} thoroughly.\n",
    )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "docs: expand the readme")

    findings = lint.evaluate(repo, commit_range="HEAD~1..HEAD")
    rules = sorted({f.rule for f in findings})
    assert rules == ["em-dash", "uk-spelling"]

    code = lint.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 1
    err = capsys.readouterr().err
    assert "em-dash" in err
    assert "customise" in err


def test_clean_change_passes(repo: Path, capsys) -> None:
    _write(repo, "README.md", "# Project\n\nClean baseline prose.\nMore good prose.\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "docs: add a line")

    assert lint.evaluate(repo, commit_range="HEAD~1..HEAD") == []
    code = lint.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 0
    assert "no em-dash or UK-spelling issues" in capsys.readouterr().out


def test_preexisting_violations_are_out_of_scope(repo: Path) -> None:
    """A pre-existing em dash / US spelling in an untouched region is not flagged.

    This is the whole point of diff-scoping: editing a file that already
    contains old violations must not resurface them.
    """
    # Commit a file riddled with old violations.
    _write(
        repo,
        "legacy.md",
        f"Old prose with an em dash {EM} and we customize behavior here.\n",
    )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "docs: legacy content (pre-discipline)")
    # A later commit appends one clean line, touching nothing old.
    _write(
        repo,
        "legacy.md",
        f"Old prose with an em dash {EM} and we customize behavior here.\n"
        "A brand new and perfectly clean line.\n",
    )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "docs: append a clean line")

    # Only the appended line is in scope, and it is clean.
    assert lint.evaluate(repo, commit_range="HEAD~1..HEAD") == []
    # The commit that first introduced the violations does flag them, though.
    introduced = lint.evaluate(repo, commit_range="HEAD~2..HEAD~1")
    assert sorted({f.rule for f in introduced}) == ["em-dash", "uk-spelling"]


def test_warn_only_never_exits_non_zero(repo: Path, capsys) -> None:
    _write(repo, "README.md", f"# Project\n\nBaseline.\nWe customize {EM} it.\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "docs: tweak")

    code = lint.main(
        ["--range", "HEAD~1..HEAD", "--repo-root", str(repo), "--warn-only"]
    )
    assert code == 0
    err = capsys.readouterr().err
    assert "customise" in err
    assert "--warn-only set" in err


def test_staged_working_tree_diff_is_scanned(repo: Path) -> None:
    """With no range, the working-tree change against HEAD is the surface."""
    _write(repo, "README.md", f"# Project\n\nBaseline.\nUncommitted {EM} dash.\n")
    findings = lint.evaluate(repo)
    assert any(f.rule == "em-dash" for f in findings)
