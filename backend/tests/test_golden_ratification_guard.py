"""Tests for the golden-ratification guard.

The guard (``backend/scripts/check_golden_ratification.py``) exists to close the
suite's foremost failure mode: a developer running the goldens-regeneration
workflow and silently ratifying a regression. These tests drive the guard
against a throwaway git repository so the git-backed diff and commit-message
inspection are exercised end to end, and assert the three behaviours that
matter:

- a golden-only change lacking the documented marker fails (exit non-zero);
- a properly-marked regeneration commit passes;
- a change that touches no golden file is ignored.

The repository is built under ``tmp_path`` with an isolated identity, so the
test neither reads nor mutates the real working tree.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from backend.scripts import check_golden_ratification as guard


def _git(repo: Path, *args: str) -> str:
    """Run git inside ``repo`` with a deterministic, isolated identity."""
    env = {
        "GIT_AUTHOR_NAME": "Test",
        "GIT_AUTHOR_EMAIL": "test@example.invalid",
        "GIT_COMMITTER_NAME": "Test",
        "GIT_COMMITTER_EMAIL": "test@example.invalid",
        # Keep the throwaway repo from inheriting any ambient global config.
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
    """An initialised git repo on a known base commit with one golden file."""
    root = tmp_path / "repo"
    root.mkdir()
    _git(root, "init", "-q", "-b", "main")
    # A representative golden plus an ordinary source file, committed as the base
    # so later changes diff cleanly against HEAD.
    _write(
        root,
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        '{"sessions": 1}\n',
    )
    _write(root, "backend/services/cost_engine.py", "VALUE = 1\n")
    _git(root, "add", "-A")
    _git(root, "commit", "-q", "-m", "base")
    return root


# --- the path predicate (pure, no git) ------------------------------------


@pytest.mark.parametrize(
    "path",
    [
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/fingerprint.jsonl",
        "backend/tests/e2e/corpus/scripted/h/expected/http_responses/GET_x.json",
        "backend/tests/expected/openapi.snapshot.json",
        "backend/tests/e2e/test_consistency_tracking_hunt_midpoint/x.yml",
        "backend/testing/COVERAGE.md",
        # Windows-style separators normalise to the same verdict.
        "backend\\tests\\expected\\openapi.snapshot.json",
    ],
)
def test_golden_paths_are_recognised(path: str) -> None:
    assert guard.is_golden_path(path)


@pytest.mark.parametrize(
    "path",
    [
        "backend/services/cost_engine.py",
        "backend/tests/test_cost_engine.py",
        "backend/testing/dsl.py",
        "TESTING.md",
        "backend/tests/e2e/test_consistency_tracking_hunt_midpoint.py",
        # An 'expected' directory outside the test tree is not a golden.
        "backend/services/expected/thing.json",
    ],
)
def test_non_golden_paths_are_ignored(path: str) -> None:
    assert not guard.is_golden_path(path)


# --- the three headline behaviours, end to end through git ----------------


def test_unmarked_golden_change_fails(repo: Path, capsys) -> None:
    """A goldens-only change without the marker must fail (exit non-zero)."""
    _write(
        repo,
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        '{"sessions": 99}\n',
    )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "feat: tweak the hunt totals")

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert result.touches_goldens
    assert not result.has_marker
    assert not result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 1
    err = capsys.readouterr().err
    assert "without the documented" in err
    assert "db_state.json" in err


def test_marked_regeneration_passes(repo: Path, capsys) -> None:
    """A regeneration commit carrying the documented marker passes."""
    _write(
        repo,
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        '{"sessions": 99}\n',
    )
    _git(repo, "add", "-A")
    _git(
        repo,
        "commit",
        "-q",
        "-m",
        "test: regenerate goldens for the hunt-totals change\n\n"
        "Regenerated: basic_hunt db_state.json.",
    )

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert result.touches_goldens
    assert result.has_marker
    assert result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 0
    assert "ratification is deliberate" in capsys.readouterr().out


def test_non_golden_change_is_ignored(repo: Path, capsys) -> None:
    """A change touching no golden file passes without needing the marker."""
    _write(repo, "backend/services/cost_engine.py", "VALUE = 2\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "fix: bump the cost constant")

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert not result.touches_goldens
    assert result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 0
    assert "nothing to guard" in capsys.readouterr().out


# --- supporting CLI surfaces ----------------------------------------------


def test_warn_only_never_exits_non_zero(repo: Path, capsys) -> None:
    """``--warn-only`` surfaces the diff but always exits 0."""
    _write(
        repo,
        "backend/tests/expected/openapi.snapshot.json",
        '{"openapi": "3.1.0"}\n',
    )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "feat: change the schema")

    code = guard.main(
        ["--range", "HEAD~1..HEAD", "--repo-root", str(repo), "--warn-only"]
    )
    assert code == 0
    err = capsys.readouterr().err
    assert "without the documented" in err
    assert "--warn-only set" in err


def test_staged_diff_against_head_detects_unmarked_golden(repo: Path) -> None:
    """The default (no range) inspects the working-tree diff against HEAD."""
    # Change a golden but do not commit: the staged / working-tree path applies.
    _write(
        repo,
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        '{"sessions": 7}\n',
    )

    result = guard.evaluate(repo)
    assert result.touches_goldens
    # With no commit yet and no prepared message naming the marker, this is the
    # unratified case the pre-commit invocation must catch.
    assert not result.ok


def test_staged_diff_honours_prepared_commit_message(repo: Path) -> None:
    """A prepared COMMIT_EDITMSG carrying the marker ratifies a staged change."""
    _write(
        repo,
        "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json",
        '{"sessions": 7}\n',
    )
    (repo / ".git" / "COMMIT_EDITMSG").write_text(
        "test: regenerate goldens for the staged hunt change\n",
        encoding="utf-8",
    )

    result = guard.evaluate(repo)
    assert result.touches_goldens
    assert result.has_marker
    assert result.ok


def test_staged_and_range_are_mutually_exclusive(repo: Path) -> None:
    with pytest.raises(SystemExit):
        guard.main(["--staged", "--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
