"""Unit cover for the character skills + codex endpoints.

Drives ``get_skills`` and ``get_codex`` against an in-memory calibration store
through the service-locator seam, so the calibrated-level read paths have
direct coverage.
"""

from pathlib import Path
from types import SimpleNamespace

import pytest

from backend.db.app_database import AppDatabase
from backend.routers import character


def _seed_calibration(db, skill_name, level, source="scan"):
    db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, ?, ?)",
        (skill_name, level, source, 1000.0),
    )
    db.conn.commit()


@pytest.fixture
def app_db(tmp_path: Path):
    return AppDatabase(tmp_path / "character.db")


def test_get_skills_returns_calibrated_rows(app_db, monkeypatch):
    _seed_calibration(app_db, "Handgun", 50.0)
    game_data = SimpleNamespace(
        get_entities=lambda kind: {
            "skills": [{"name": "Handgun", "category": {"name": "Combat"}}],
            "skill_ranks": [
                {
                    "table": {
                        "rows": [
                            {"name": "Rookie", "skill": 0},
                            {"name": "Master", "skill": 100},
                        ]
                    }
                }
            ],
        }.get(kind, [])
    )
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=game_data),
    )

    result = character.get_skills()

    assert len(result) == 1
    row = result[0]
    assert row["name"] == "Handgun"
    assert row["level"] == 50.0
    assert row["anchorLevel"] == 50.0
    assert row["gainSinceAnchor"] == 0.0
    assert row["category"] == "Combat"
    assert row["rankName"] == "Rookie"


def test_get_skills_empty_when_no_calibrations(app_db, monkeypatch):
    game_data = SimpleNamespace(get_entities=lambda kind: [])
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=game_data),
    )

    assert character.get_skills() == []


def test_get_codex_predicts_rewards_for_codex_skills(app_db, monkeypatch):
    _seed_calibration(app_db, "Handgun", 50.0)  # cat1 (divisor 200)
    _seed_calibration(app_db, "Mining", 30.0)  # not a codex skill -> excluded
    game_data = SimpleNamespace(get_entities=lambda kind: [])
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=game_data),
    )

    result = character.get_codex()

    assert [row["skillName"] for row in result] == ["Handgun"]
    handgun = result[0]
    assert handgun["currentLevel"] == 50.0
    assert handgun["nextRewardValue"] == round(50.0 / 200, 2)
    assert handgun["progress"] == round((50.0 % 200) / 200, 4)
