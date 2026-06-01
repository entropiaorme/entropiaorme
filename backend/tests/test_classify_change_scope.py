"""Tests for the change-scope classifier that gates the expensive CI jobs.

The guard (``backend/scripts/classify_change_scope.py``) decides whether a change
needs the backend test matrix, the frontend build, and the full-tier merge gate,
or whether it is documentation-only and those jobs can be skipped. The
conservative direction is the point, so these tests pin it precisely:

- documentation-only means *every* changed path is Markdown; one non-Markdown
  path makes the whole change code;
- an empty change set, and the absence of a pull-request range, both resolve to
  code (run the suite), never to a skip;
- ``main`` writes the verdict to the ``GITHUB_OUTPUT`` file a workflow reads.

The pure predicate tests assert the rule directly; the git-backed tests drive the
diff against a throwaway repository, mirroring ``test_authoring_lint``.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from backend.scripts import classify_change_scope as scope


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
    _write(root, "README.md", "# Project\n\nBaseline prose.\n")
    _write(root, "backend/app.py", "VALUE = 1\n")
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "base")
    return root


def _base(repo: Path) -> str:
    return _git(repo, "rev-parse", "HEAD").strip()


# --- the path predicates (pure, no git) -----------------------------------


def test_is_docs_path_recognises_markdown() -> None:
    assert scope.is_docs_path("README.md")
    assert scope.is_docs_path("backend/testing/COVERAGE.md")
    # Case-insensitive on the suffix; backslashes normalise to POSIX.
    assert scope.is_docs_path("docs\\Guide.MD")


def test_is_docs_path_rejects_non_markdown() -> None:
    assert not scope.is_docs_path("backend/app.py")
    assert not scope.is_docs_path(".github/workflows/ci.yml")
    assert not scope.is_docs_path("frontend/src/App.svelte")
    # A plain-text note is not Markdown: code, so the suite runs.
    assert not scope.is_docs_path("NOTES.txt")


def test_is_docs_only_all_markdown() -> None:
    assert scope.is_docs_only(["README.md", "docs/guide.md"])


def test_is_docs_only_any_non_markdown_is_code() -> None:
    assert not scope.is_docs_only(["README.md", "backend/app.py"])


def test_is_docs_only_empty_is_not_docs_only() -> None:
    # A degenerate empty change set resolves to code, not a skip.
    assert not scope.is_docs_only([])


# --- the git-backed classifier --------------------------------------------


def test_docs_only_change_classifies_as_docs(repo: Path) -> None:
    base = _base(repo)
    _write(repo, "README.md", "# Project\n\nUpdated prose.\n")
    _git(repo, "commit", "-aqm", "docs")
    head = _git(repo, "rev-parse", "HEAD").strip()
    assert scope.classify(repo, commit_range=f"{base}..{head}") is False


def test_code_change_classifies_as_code(repo: Path) -> None:
    base = _base(repo)
    _write(repo, "backend/app.py", "VALUE = 2\n")
    _git(repo, "commit", "-aqm", "code")
    head = _git(repo, "rev-parse", "HEAD").strip()
    assert scope.classify(repo, commit_range=f"{base}..{head}") is True


def test_mixed_change_classifies_as_code(repo: Path) -> None:
    base = _base(repo)
    _write(repo, "README.md", "# Project\n\nUpdated prose.\n")
    _write(repo, "backend/app.py", "VALUE = 2\n")
    _git(repo, "commit", "-aqm", "mixed")
    head = _git(repo, "rev-parse", "HEAD").strip()
    assert scope.classify(repo, commit_range=f"{base}..{head}") is True


def test_no_range_classifies_as_code(repo: Path) -> None:
    # No pull-request range (e.g. a push to main): run the suite.
    assert scope.classify(repo, commit_range=None) is True


# --- the workflow entry point ---------------------------------------------


def test_main_writes_docs_verdict_to_github_output(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    base = _base(repo)
    _write(repo, "README.md", "# Project\n\nUpdated prose.\n")
    _git(repo, "commit", "-aqm", "docs")
    head = _git(repo, "rev-parse", "HEAD").strip()

    output = tmp_path / "github_output"
    monkeypatch.setenv("GITHUB_OUTPUT", str(output))
    rc = scope.main(["--range", f"{base}..{head}", "--repo-root", str(repo)])

    assert rc == 0
    assert "code=false" in output.read_text(encoding="utf-8")


def test_main_writes_code_verdict_to_github_output(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    base = _base(repo)
    _write(repo, "backend/app.py", "VALUE = 2\n")
    _git(repo, "commit", "-aqm", "code")
    head = _git(repo, "rev-parse", "HEAD").strip()

    output = tmp_path / "github_output"
    monkeypatch.setenv("GITHUB_OUTPUT", str(output))
    rc = scope.main(["--range", f"{base}..{head}", "--repo-root", str(repo)])

    assert rc == 0
    assert "code=true" in output.read_text(encoding="utf-8")
