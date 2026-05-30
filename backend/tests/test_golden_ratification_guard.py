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


# The fixture's golden (a basic_hunt scenario DB-state snapshot) and the path the
# matching independent ratification report is committed to.
_GOLDEN = "backend/tests/e2e/corpus/scripted/basic_hunt/expected/db_state.json"
_ARTIFACT = "backend/testing/ratifications/basic-hunt-totals.md"


def _ratification(
    *,
    verdict: str = "ratification-sound",
    goldens: str = "basic_hunt fingerprint.jsonl, db_state.json",
    rng: str = "HEAD~1..HEAD",
) -> str:
    """The text of an independent ratification report carrying a verdict block.

    Only the trailing fenced ``ORACLE-RATIFICATION`` block is load-bearing for
    the guard; the preamble stands in for the report's findings narrative.
    """
    return (
        "# Ratification: basic-hunt totals change\n\n"
        "Reviewed the golden diff against the code change: the new totals are\n"
        "the intended rounding change, and every diff element is accounted for.\n\n"
        "```\n"
        "ORACLE-RATIFICATION\n"
        f"range: {rng}\n"
        f"goldens: {goldens}\n"
        f"VERDICT: {verdict}\n"
        "```\n"
    )


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


def test_marked_and_ratified_regeneration_passes(repo: Path, capsys) -> None:
    """Range mode: marker + an in-range sound verdict naming the set passes."""
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _write(repo, _ARTIFACT, _ratification())
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
    assert result.has_sound_verdict
    assert not result.unblessed_sets
    assert result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 0
    assert "signed off" in capsys.readouterr().out


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


# --- the ratification-artefact predicate and parser (pure, no git) ---------


@pytest.mark.parametrize(
    "path",
    [
        "backend/testing/ratifications/basic-hunt.md",
        "backend/testing/ratifications/2026-05-30-openapi-snapshot.md",
        # Windows-style separators normalise to the same verdict.
        "backend\\testing\\ratifications\\win.md",
    ],
)
def test_ratification_artifacts_are_recognised(path: str) -> None:
    assert guard.is_ratification_artifact(path)


@pytest.mark.parametrize(
    "path",
    [
        "backend/testing/ratifications/notes.txt",  # not a .md report
        "backend/testing/ratifications.md",  # the dir name, not a file under it
        "backend/testing/COVERAGE.md",
        "backend/tests/expected/openapi.snapshot.json",
        # A report must never be classified as a golden, or the guard would
        # demand a ratification artefact for its own ratification artefact.
        "backend/tests/e2e/corpus/scripted/h/expected/ratification.md",
    ],
)
def test_non_ratification_paths_are_ignored(path: str) -> None:
    assert not guard.is_ratification_artifact(path)


def test_ratification_path_is_never_a_golden() -> None:
    assert not guard.is_golden_path(_ARTIFACT)


def test_parse_extracts_verdict_goldens_and_range() -> None:
    text = (
        "preamble naming a VERDICT in prose should not count\n\n"
        "```\nORACLE-RATIFICATION\n"
        "range: base..head\ngoldens: basic_hunt, openapi\n"
        "VERDICT: ratification-sound\n```\n"
    )
    verdict = guard.parse_ratification_artifact("p", text)
    assert verdict is not None
    assert verdict.is_sound
    assert verdict.range == "base..head"
    assert verdict.goldens == ("basic_hunt", "openapi")


def test_parse_returns_none_without_a_complete_block() -> None:
    assert guard.parse_ratification_artifact("p", "no block here at all") is None
    # A fenced block with the header but no recognised VERDICT line: a malformed
    # / placeholder report registers as no verdict, so it cannot satisfy the gate.
    assert (
        guard.parse_ratification_artifact(
            "p", "```\nORACLE-RATIFICATION\nrange: x\n```\n"
        )
        is None
    )


def test_parse_ignores_a_verdict_outside_the_fence() -> None:
    # A VERDICT line in the report's prose (outside the fenced block) does not
    # count: only the fenced ORACLE-RATIFICATION block is read.
    text = (
        "The reviewer wrote VERDICT: ratification-sound in passing, but the real\n"
        "block is malformed below.\n\n"
        "```\nnot the verdict block\n```\n"
    )
    assert guard.parse_ratification_artifact("p", text) is None


def test_parse_reads_an_unsound_verdict() -> None:
    text = "```\nORACLE-RATIFICATION\ngoldens: x\nVERDICT: regression-suspected\n```\n"
    verdict = guard.parse_ratification_artifact("p", text)
    assert verdict is not None
    assert not verdict.is_sound
    assert verdict.verdict == "regression-suspected"


# --- the two new range-mode failure paths, end to end through git ----------


def test_range_marker_without_verdict_fails(repo: Path, capsys) -> None:
    """Golden + marker but no recorded sound verdict in the range must fail."""
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens for the hunt change")

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert result.touches_goldens
    assert result.has_marker
    assert not result.ratification_artifacts
    assert not result.has_sound_verdict
    assert not result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 1
    assert "ratification-sound" in capsys.readouterr().err


def test_range_unsound_verdict_fails(repo: Path) -> None:
    """An in-range artefact whose verdict is not sound does not satisfy the gate."""
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _write(repo, _ARTIFACT, _ratification(verdict="regression-suspected"))
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens (review flagged it)")

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert result.touches_goldens
    assert result.has_marker
    # The artefact was found and parsed, but it is not a sound verdict.
    assert result.ratification_artifacts == (_ARTIFACT,)
    assert not result.has_sound_verdict
    assert not result.ok


def test_range_stale_verdict_does_not_bless_a_new_change(repo: Path, capsys) -> None:
    """A sound verdict must not satisfy a later same-range golden change.

    Commit A is a properly ratified regeneration. Commit B is a fresh golden
    change carrying the marker but no fresh artefact. A range covering only B
    fails (the artefact is out of range); the range covering both ALSO fails,
    because the verdict from A reviewed the earlier state, not B's later edit.
    """
    # Commit A: a fully ratified regeneration.
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _write(repo, _ARTIFACT, _ratification())
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens (A)")
    # Commit B: a new golden change, marker present, no fresh artefact.
    _write(repo, _GOLDEN, '{"sessions": 123}\n')
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens (B)")

    only_b = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert only_b.touches_goldens
    assert only_b.has_marker
    assert not only_b.ratification_artifacts  # A's artefact is out of range
    assert not only_b.has_sound_verdict
    assert not only_b.ok

    # The wider range includes A's artefact, but B changed the golden after it,
    # so the verdict is stale and the gate still fails.
    both = guard.evaluate(repo, commit_range="HEAD~2..HEAD")
    assert both.ratification_artifacts == (_ARTIFACT,)
    assert both.has_sound_verdict
    assert both.artifact_stale
    assert not both.ok

    code = guard.main(["--range", "HEAD~2..HEAD", "--repo-root", str(repo)])
    assert code == 1
    assert "later in this range than the verdict" in capsys.readouterr().err


def test_range_refreshed_verdict_blesses_the_latest_change(repo: Path) -> None:
    """Refreshing the artefact alongside the later golden change passes.

    The same A-then-B shape as the stale test, except commit B re-commits the
    ratification report, so the sound verdict is no earlier than the last golden
    change and the gate is satisfied.
    """
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _write(repo, _ARTIFACT, _ratification())
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens (A)")
    # Commit B: a further golden change WITH a refreshed report.
    _write(repo, _GOLDEN, '{"sessions": 123}\n')
    _write(repo, _ARTIFACT, _ratification(goldens="basic_hunt db_state.json refreshed"))
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens (B, re-reviewed)")

    both = guard.evaluate(repo, commit_range="HEAD~2..HEAD")
    assert both.has_sound_verdict
    assert not both.artifact_stale
    assert not both.unblessed_sets
    assert both.ok


def test_range_verdict_naming_another_set_does_not_bless_it(repo: Path, capsys) -> None:
    """A sound verdict for set A cannot bless a co-changed set B it does not name."""
    other = "backend/tests/e2e/corpus/scripted/advanced_raid/expected/db_state.json"
    _write(repo, _GOLDEN, '{"sessions": 99}\n')
    _write(repo, other, '{"sessions": 5}\n')
    # The verdict names only basic_hunt; advanced_raid co-changed but is unnamed.
    _write(repo, _ARTIFACT, _ratification(goldens="basic_hunt db_state.json"))
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "test: regenerate goldens for two scenarios")

    result = guard.evaluate(repo, commit_range="HEAD~1..HEAD")
    assert result.has_sound_verdict
    assert "advanced_raid" in result.unblessed_sets
    assert "basic_hunt" not in result.unblessed_sets
    assert not result.ok

    code = guard.main(["--range", "HEAD~1..HEAD", "--repo-root", str(repo)])
    assert code == 1
    assert "does not name every changed golden set" in capsys.readouterr().err


def test_staged_mode_stays_marker_only_despite_no_verdict(repo: Path) -> None:
    """Staged / working-tree mode is advisory: marker alone satisfies it.

    The verdict requirement is the range-mode pull-request gate's; the pre-commit
    invocation cannot see a committed verdict, so it stays marker-only.
    """
    _write(repo, _GOLDEN, '{"sessions": 7}\n')
    (repo / ".git" / "COMMIT_EDITMSG").write_text(
        "test: regenerate goldens for the staged hunt change\n",
        encoding="utf-8",
    )

    result = guard.evaluate(repo)
    assert result.touches_goldens
    assert result.has_marker
    assert not result.range_mode
    assert result.ok
