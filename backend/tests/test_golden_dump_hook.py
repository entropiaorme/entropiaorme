"""The golden harness's cross-implementation dump hook.

With ``EO_DB_DUMP_DIR`` set, the harness materialises the scenario's
final database and the shared normaliser's symbol tables at snapshot
time, the inputs the native replay comparison consumes.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

import pytest

from backend.testing.golden import GoldenAssertionFailure, GoldenSet


def test_dump_hook_materialises_db_and_symbols(tmp_path: Path, monkeypatch) -> None:
    scenario_dir = tmp_path / "scenario_under_test"
    scenario_dir.mkdir()
    dump_dir = tmp_path / "dumps"
    monkeypatch.setenv("EO_DB_DUMP_DIR", str(dump_dir))

    db = sqlite3.connect(":memory:")
    db.execute(
        "CREATE TABLE tracking_sessions (id TEXT, started_at REAL, "
        "ended_at REAL, is_active INTEGER, heal_cost REAL, dangling_cost REAL)"
    )
    db.execute(
        "INSERT INTO tracking_sessions VALUES "
        "('123e4567-e89b-12d3-a456-426614174000', 1738220402.5, NULL, 1, NULL, NULL)"
    )
    # The live pipeline always commits before a snapshot; an open write
    # transaction would stall the backup just as it would stall any
    # second reader.
    db.commit()

    goldens = GoldenSet(scenario_dir)
    # No goldens exist for the throwaway scenario; the dump must land
    # before the missing-golden failure surfaces.
    with pytest.raises(GoldenAssertionFailure):
        goldens.assert_matches(db)

    dumped = dump_dir / "scenario_under_test.db"
    assert dumped.exists()
    copy = sqlite3.connect(dumped)
    row = copy.execute("SELECT id, started_at FROM tracking_sessions").fetchone()
    assert row == ("123e4567-e89b-12d3-a456-426614174000", 1738220402.5)
    copy.close()

    symbols = json.loads(
        (dump_dir / "scenario_under_test.symbols.json").read_text(encoding="utf-8")
    )
    assert symbols["uuids"] == {"123e4567-e89b-12d3-a456-426614174000": "<UUID_1>"}
    assert "1738220402.5" in symbols["timestamps"]


def test_no_dump_without_the_env(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.delenv("EO_DB_DUMP_DIR", raising=False)
    scenario_dir = tmp_path / "quiet_scenario"
    scenario_dir.mkdir()
    db = sqlite3.connect(":memory:")
    with pytest.raises(GoldenAssertionFailure):
        GoldenSet(scenario_dir).assert_matches(db)
    assert not list(tmp_path.glob("**/*.db"))
