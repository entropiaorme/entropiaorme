"""Tests for the no-bare-setInterval frontend lint.

The lint is the frontend half of the polling/orphan enforcement pair (its
backend twin is ``test_supervised_workers``). These tests assert it is green
against the live tree AND that it has teeth: a planted bare ``setInterval`` or a
planted retired-event reference turns it red, the sanctioned helper module is
exempt, and the whole-tree ``git ls-files`` enumeration finds a pre-existing
offender (not just a newly added one).
"""

from __future__ import annotations

import subprocess

import pytest

from backend.scripts.check_no_bare_setinterval import (
    REPO_ROOT,
    SETINTERVAL_HOME,
    Finding,
    evaluate,
    main,
    scan_text,
)


def _rules(findings: list[Finding]) -> set[str]:
    return {f.rule for f in findings}


def _init_repo_with_violation(repo) -> None:
    """Stage one tracked frontend source file containing a bare setInterval."""
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    src = repo / "frontend" / "src" / "routes"
    src.mkdir(parents=True)
    (src / "+page.svelte").write_text("setInterval(fn, 1000);\n", encoding="utf-8")
    subprocess.run(
        ["git", "add", "frontend/src/routes/+page.svelte"], cwd=repo, check=True
    )


def test_frontend_tree_is_clean() -> None:
    """The live frontend source has no bare setInterval or retired-event refs."""
    findings = evaluate(REPO_ROOT)
    assert findings == [], "Frontend polling-discipline violations:\n" + "\n".join(
        f"  {f.path}:{f.lineno} [{f.rule}] {f.detail}" for f in findings
    )


def test_scan_flags_bare_setinterval_outside_home() -> None:
    findings = scan_text(
        "frontend/src/routes/+page.svelte",
        "const t = setInterval(fn, 1000);\n",
    )
    assert "bare-setinterval" in _rules(findings)


def test_scan_allows_setinterval_in_the_helper_home() -> None:
    findings = scan_text(SETINTERVAL_HOME, "\ttimer = setInterval(run, intervalMs);\n")
    assert "bare-setinterval" not in _rules(findings)


def test_scan_flags_setinterval_with_space_before_paren() -> None:
    """The whitespace variant ``setInterval (fn)`` does not evade Rule A."""
    findings = scan_text(
        "frontend/src/routes/+page.svelte", "setInterval (fn, 1000);\n"
    )
    assert "bare-setinterval" in _rules(findings)


def test_scan_flags_retired_event_reference() -> None:
    # Rule B applies everywhere, including the helper home.
    for path in ("frontend/src/lib/realtime/eventRelay.ts", SETINTERVAL_HOME):
        findings = scan_text(path, "void emit('tracking-state-changed', {});\n")
        assert "legacy-event" in _rules(findings), path


def test_scan_is_clean_on_compliant_text() -> None:
    findings = scan_text(
        "frontend/src/routes/quests/+page.svelte",
        "return useVisiblePoll(refresh, { intervalMs: 1000 });\n",
    )
    assert findings == []


def test_evaluate_catches_planted_violations_whole_tree(tmp_path) -> None:
    """End-to-end: the git-tracked whole-tree scan flags a planted offender.

    Proves the lint is whole-tree (it flags a file it never saw added in a diff)
    and that the git enumeration + read path work together.
    """
    repo = tmp_path
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    src = repo / "frontend" / "src" / "routes"
    src.mkdir(parents=True)
    (src / "+page.svelte").write_text(
        "const t = setInterval(fn, 1000);\nvoid emit('tracking-state-changed', {});\n",
        encoding="utf-8",
    )
    # A node_modules file with the same token must NOT be scanned: it is left
    # untracked (and node_modules is gitignored in the real repo), so
    # ``git ls-files`` never reports it, regardless of its suffix.
    junk = repo / "frontend" / "src" / "node_modules"
    junk.mkdir(parents=True)
    (junk / "vendor.js").write_text("setInterval(x, 1);\n", encoding="utf-8")
    # Stage so git ls-files reports the tracked source (a commit is not needed).
    subprocess.run(
        ["git", "add", "frontend/src/routes/+page.svelte"], cwd=repo, check=True
    )

    findings = evaluate(repo)
    assert _rules(findings) == {"bare-setinterval", "legacy-event"}
    assert all(f.path == "frontend/src/routes/+page.svelte" for f in findings)


def test_evaluate_scans_js_family_sources(tmp_path) -> None:
    """A bare setInterval in a tracked .js module under frontend/src is caught.

    Vite / SvelteKit bundle .js alongside .svelte/.ts, so the lint must not be
    blind to a poll hiding in a first-class .js-family module.
    """
    repo = tmp_path
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    lib = repo / "frontend" / "src" / "lib"
    lib.mkdir(parents=True)
    (lib / "legacy.js").write_text("setInterval(fn, 1000);\n", encoding="utf-8")
    subprocess.run(["git", "add", "frontend/src/lib/legacy.js"], cwd=repo, check=True)

    findings = evaluate(repo)
    assert any(f.rule == "bare-setinterval" for f in findings)


def test_evaluate_skips_deleted_but_tracked_file(tmp_path) -> None:
    """An unstaged on-disk deletion is skipped, not a crash and not a finding.

    ``git ls-files`` enumerates the index, so a tracked file deleted from the
    working tree is still listed; it carries no live content to scan.
    """
    _init_repo_with_violation(tmp_path)
    (tmp_path / "frontend" / "src" / "routes" / "+page.svelte").unlink()

    assert evaluate(tmp_path) == []


def test_evaluate_fails_loudly_on_unreadable_tracked_file(tmp_path) -> None:
    """A read failure other than on-disk absence propagates.

    A guard that silently skipped an unreadable source could return a false
    clean; reading a directory in place of the tracked file raises an
    ``OSError`` that is not ``FileNotFoundError`` on every platform.
    """
    _init_repo_with_violation(tmp_path)
    target = tmp_path / "frontend" / "src" / "routes" / "+page.svelte"
    target.unlink()
    target.mkdir()

    with pytest.raises(OSError):
        evaluate(tmp_path)


def test_main_returns_zero_when_clean(tmp_path, capsys) -> None:
    """The CLI exits 0 and reports clean against a tree with no offenders."""
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    assert main(["--repo-root", str(tmp_path)]) == 0
    assert "no bare setInterval" in capsys.readouterr().out


def test_main_returns_one_on_violations(tmp_path, capsys) -> None:
    """The CLI exits 1 and lists the offenders on a violating tree."""
    _init_repo_with_violation(tmp_path)
    assert main(["--repo-root", str(tmp_path)]) == 1
    assert "polling-discipline violations" in capsys.readouterr().err


def test_main_warn_only_exits_zero_despite_violations(tmp_path) -> None:
    """--warn-only reports but exits 0 even when offenders are present."""
    _init_repo_with_violation(tmp_path)
    assert main(["--repo-root", str(tmp_path), "--warn-only"]) == 0
