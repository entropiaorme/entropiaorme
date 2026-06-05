"""Tests for the version-stamp parity guard.

The guard (``backend/scripts/check_version_stamps.py``) asserts the three app
version stamps (``package.json``, ``Cargo.toml``, ``tauri.conf.json``) carry an
identical version string, and deliberately excludes the independent
``CURRENT_TOS_VERSION``. These tests drive the logic against synthetic trees and
add one live assertion over the real repository, so a future bump that updates
some stamps but not all fails the suite as well as the gate.
"""

from __future__ import annotations

from pathlib import Path

from backend.scripts import check_version_stamps as stamps


def _write_stamps(
    root: Path,
    *,
    package: str,
    cargo: str,
    tauri: str,
) -> None:
    (root / "frontend").mkdir(parents=True, exist_ok=True)
    (root / "frontend/src-tauri/entropia-orme").mkdir(parents=True, exist_ok=True)
    (root / stamps.PACKAGE_JSON).write_text(
        '{\n  "name": "x",\n  "version": "' + package + '"\n}\n',
        encoding="utf-8",
    )
    (root / stamps.CARGO_TOML).write_text(
        '[workspace]\nmembers = ["x"]\n\n[workspace.package]\nversion = "'
        + cargo
        + '"\nedition = "2021"\n',
        encoding="utf-8",
    )
    (root / stamps.TAURI_CONF).write_text(
        '{\n  "productName": "X",\n  "version": "' + tauri + '"\n}\n',
        encoding="utf-8",
    )


def test_matching_stamps_pass(tmp_path: Path, capsys) -> None:
    _write_stamps(tmp_path, package="0.2.0", cargo="0.2.0", tauri="0.2.0")

    result = stamps.evaluate(tmp_path)
    assert result.ok
    assert result.versions == {"0.2.0"}

    assert stamps.main(["--repo-root", str(tmp_path)]) == 0
    assert "agree at 0.2.0" in capsys.readouterr().out


def test_one_stamp_lagging_fails(tmp_path: Path, capsys) -> None:
    # A bump that updated package.json and tauri.conf.json but not Cargo.toml.
    _write_stamps(tmp_path, package="0.2.0", cargo="0.1.0", tauri="0.2.0")

    result = stamps.evaluate(tmp_path)
    assert not result.ok
    assert result.versions == {"0.1.0", "0.2.0"}

    assert stamps.main(["--repo-root", str(tmp_path)]) == 1
    err = capsys.readouterr().err
    assert "disagree" in err
    assert stamps.CARGO_TOML in err


def test_each_stamp_format_is_parsed(tmp_path: Path) -> None:
    _write_stamps(tmp_path, package="1.2.3", cargo="4.5.6", tauri="7.8.9")
    read = stamps.read_stamps(tmp_path)
    assert read[stamps.PACKAGE_JSON] == "1.2.3"
    assert read[stamps.CARGO_TOML] == "4.5.6"
    assert read[stamps.TAURI_CONF] == "7.8.9"


def test_repository_version_stamps_are_in_lockstep() -> None:
    """The real tree must always satisfy the parity rule (a live guard)."""
    result = stamps.evaluate(stamps.REPO_ROOT)
    assert result.ok, f"version stamps disagree: {result.stamps}"
