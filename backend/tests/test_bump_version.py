"""Tests for the lock-step version-bump helper.

The helper (``backend/scripts/bump_version.py``) rewrites the same three app
version stamps the parity guard governs, surgically (version token only) so
formatting and unrelated content survive, and self-verifies through the guard's
own ``evaluate``. These tests drive it against a synthetic tree: they prove the
round-trip the guard expects (bump leaves all three in lock-step), that the
``[workspace.package]`` edit does not touch a ``[workspace.dependencies]`` pin,
that JSON formatting is preserved, and that a non-semver argument is rejected.
"""

from __future__ import annotations

from pathlib import Path

from backend.scripts import bump_version
from backend.scripts import check_version_stamps as stamps


def _write_tree(root: Path, version: str) -> None:
    """A minimal repo tree with the three stamps plus a dependency pin to guard."""
    (root / "frontend").mkdir(parents=True, exist_ok=True)
    (root / "frontend/src-tauri/entropia-orme").mkdir(parents=True, exist_ok=True)
    # Tab-indented, like the real package.json, so format preservation is testable.
    (root / stamps.PACKAGE_JSON).write_text(
        '{\n\t"name": "x",\n\t"private": true,\n\t"version": "' + version + '"\n}\n',
        encoding="utf-8",
    )
    # A [workspace.dependencies] pin carrying its own version = "..." that must
    # survive untouched (the scoping guard).
    (root / stamps.CARGO_TOML).write_text(
        "[workspace]\n"
        'members = ["x"]\n\n'
        "[workspace.package]\n"
        f'version = "{version}"\n'
        'edition = "2021"\n\n'
        "[workspace.dependencies]\n"
        'serde = { version = "1.0.0" }\n',
        encoding="utf-8",
    )
    (root / stamps.TAURI_CONF).write_text(
        '{\n  "productName": "X",\n  "version": "' + version + '"\n}\n',
        encoding="utf-8",
    )


def test_bump_sets_all_three_in_lockstep(tmp_path: Path) -> None:
    _write_tree(tmp_path, "0.1.0")

    bump_version.set_version(tmp_path, "0.2.0")

    result = stamps.evaluate(tmp_path)
    assert result.ok
    assert result.versions == {"0.2.0"}


def test_bump_does_not_touch_dependency_pin(tmp_path: Path) -> None:
    _write_tree(tmp_path, "0.1.0")

    bump_version.set_version(tmp_path, "0.2.0")

    cargo = (tmp_path / stamps.CARGO_TOML).read_text(encoding="utf-8")
    # The [workspace.package] version moved; the [workspace.dependencies] pin did not.
    assert 'version = "0.2.0"' in cargo
    assert 'serde = { version = "1.0.0" }' in cargo


def test_bump_preserves_json_formatting(tmp_path: Path) -> None:
    _write_tree(tmp_path, "0.1.0")

    bump_version.set_version(tmp_path, "0.2.0")

    package = (tmp_path / stamps.PACKAGE_JSON).read_text(encoding="utf-8")
    assert '\t"name": "x"' in package  # tab indentation intact
    assert '"private": true' in package  # sibling keys intact
    assert '"version": "0.2.0"' in package


def test_main_round_trip(tmp_path: Path, capsys) -> None:
    _write_tree(tmp_path, "0.1.0")

    assert bump_version.main(["0.2.0", "--repo-root", str(tmp_path)]) == 0
    assert "set to 0.2.0" in capsys.readouterr().out
    assert stamps.evaluate(tmp_path).versions == {"0.2.0"}

    # Round back to the original; the helper is idempotent in either direction.
    assert bump_version.main(["0.1.0", "--repo-root", str(tmp_path)]) == 0
    assert stamps.evaluate(tmp_path).versions == {"0.1.0"}


def test_bump_preserves_lf_line_endings(tmp_path: Path) -> None:
    # The repo normalises to LF (.gitattributes eol=lf); the bump must not
    # introduce CRLF (text-mode write does on Windows without newline="").
    _write_tree(tmp_path, "0.1.0")

    bump_version.set_version(tmp_path, "0.2.0")

    for rel in (stamps.PACKAGE_JSON, stamps.CARGO_TOML, stamps.TAURI_CONF):
        assert b"\r\n" not in (tmp_path / rel).read_bytes(), f"{rel} gained CRLF"


def test_invalid_version_is_rejected(tmp_path: Path, capsys) -> None:
    _write_tree(tmp_path, "0.1.0")

    assert bump_version.main(["not-a-version", "--repo-root", str(tmp_path)]) == 2
    assert "not a valid semver" in capsys.readouterr().err
    # The tree is untouched by a rejected bump.
    assert stamps.evaluate(tmp_path).versions == {"0.1.0"}
