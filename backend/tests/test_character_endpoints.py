"""Unit cover for the character skills + codex endpoints.

Drives ``get_skills`` and ``get_codex`` against an in-memory calibration store
through the service-locator seam, so the calibrated-level read paths have
direct coverage.
"""

from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from backend.db.app_database import AppDatabase
from backend.routers import character


def _game_data(professions=None):
    """A game-data stub whose ``get_entities`` answers per kind."""
    tables = {"professions": professions or []}
    return SimpleNamespace(get_entities=lambda kind: tables.get(kind, []))


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


# ── Prospect request-validation guards (raise before any service lookup) ──────


@pytest.mark.parametrize(
    "kwargs",
    [
        {"target_level": 0.0},  # non-positive target
        {"target_level": 10.0, "markup_uplift": -1.0},  # negative uplift
        {"target_level": 10.0, "slice_type": "bogus"},  # unknown slice type
        {"target_level": 10.0, "slice_type": "tag"},  # missing slice value
    ],
)
def test_prospect_rejects_invalid_request(kwargs):
    with pytest.raises(HTTPException) as exc:
        character.get_character_prospect(profession="Laser Pistoleer", **kwargs)
    assert exc.value.status_code == 422


def test_prospect_unknown_profession_returns_error_shape(monkeypatch):
    monkeypatch.setattr(
        character, "get_services", lambda: SimpleNamespace(game_data=_game_data())
    )

    result = character.get_character_prospect(profession="Nope", target_level=10.0)

    assert result["error"] == "Profession 'Nope' not found"
    assert result["rows"] == []
    assert result["warnings"] == []


# ── Optimizer not-found shapes ────────────────────────────────────────────────


def test_profession_optimizer_unknown_profession_returns_error(monkeypatch):
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(game_data=_game_data([{"name": "Other"}])),
    )

    result = character.get_profession_optimizer(profession="Nope")

    assert result["error"] == "Profession 'Nope' not found"
    assert result["skills"] == []
    assert result["attributes"] == []


def test_path_optimizer_requires_exactly_one_target():
    # Neither target_level nor ped_budget.
    with pytest.raises(HTTPException) as exc_neither:
        character.get_profession_path_optimizer(profession="Laser Pistoleer")
    assert exc_neither.value.status_code == 422

    # Both at once.
    with pytest.raises(HTTPException) as exc_both:
        character.get_profession_path_optimizer(
            profession="Laser Pistoleer", target_level=10.0, ped_budget=5.0
        )
    assert exc_both.value.status_code == 422


def test_path_optimizer_unknown_profession_returns_error(monkeypatch):
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(game_data=_game_data([{"name": "Other"}])),
    )

    result = character.get_profession_path_optimizer(
        profession="Nope", target_level=10.0
    )

    assert result["error"] == "Profession 'Nope' not found"
    assert result["allocations"] == []


# ── Read-path endpoints (calibrated DB + profession/skill catalogue) ──────────

_PROFESSIONS = [
    {
        "name": "Laser Pistoleer",
        "category": "Combat",
        "skills": [{"skill": {"name": "Handgun"}, "weight": 100}],
    }
]


def test_get_character_stats_returns_hp_and_top_professions(app_db, monkeypatch):
    _seed_calibration(app_db, "Health", 100.0)
    _seed_calibration(app_db, "Handgun", 100.0)
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=_game_data(_PROFESSIONS)),
    )

    result = character.get_character_stats()

    assert result["hp"] == 100
    assert result["topProfessions"]
    top = result["topProfessions"][0]
    assert top["name"] == "Laser Pistoleer"
    assert top["level"] > 0
    assert top["category"] == "Combat"


def test_get_professions_returns_levels_with_anchor(app_db, monkeypatch):
    _seed_calibration(app_db, "Handgun", 100.0, source="scan")
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=_game_data(_PROFESSIONS)),
    )

    result = character.get_professions()

    assert result
    prof = result[0]
    assert prof["name"] == "Laser Pistoleer"
    assert prof["level"] > 0
    assert prof["anchorLevel"] is not None  # a source='scan' row is the anchor
    assert prof["gainSinceAnchor"] == 0.0  # believed-current equals the anchor


def test_get_professions_empty_without_catalogue(app_db, monkeypatch):
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=_game_data([])),
    )

    assert character.get_professions() == []


def test_get_calibration_reports_status_after_a_scan(app_db, monkeypatch):
    _seed_calibration(app_db, "Handgun", 50.0)  # scanned_at far in the past
    monkeypatch.setattr(
        character, "get_services", lambda: SimpleNamespace(app_db=app_db)
    )

    result = character.get_calibration()

    assert result["calibrated"] is True
    assert result["lastCalibration"] is not None
    assert result["stale"] is True  # the seeded timestamp is ancient


def test_get_calibration_uncalibrated_when_empty(app_db, monkeypatch):
    monkeypatch.setattr(
        character, "get_services", lambda: SimpleNamespace(app_db=app_db)
    )

    assert character.get_calibration() == {
        "calibrated": False,
        "lastCalibration": None,
        "stale": True,
    }


def test_hp_optimizer_runs_over_calibrated_skills(app_db, monkeypatch):
    _seed_calibration(app_db, "Health", 100.0)
    skills = [{"name": "Anatomy", "hp_increase": 0.1}]
    game_data = SimpleNamespace(
        get_entities=lambda kind: {"skills": skills}.get(kind, [])
    )
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=game_data),
    )

    result = character.get_hp_optimizer()

    assert isinstance(result, dict)
    # Health is the only calibrated skill (level 100 -> HP 80); Anatomy is a
    # regular skill at level 0, so it ranks alone in skills with no attributes.
    assert result["currentHp"] == pytest.approx(80.0, abs=1e-2)
    assert [s["name"] for s in result["skills"]] == ["Anatomy"]
    assert result["skills"][0]["levelsPerHp"] == pytest.approx(0.1, abs=1e-3)
    assert result["attributes"] == []


# ── Prospect forecast (drives the sample + projection machinery) ──────────────

_PROSPECT_SESSIONS = [
    {
        "kills": 50,
        "durationHours": 2.0,
        "cycledPed": 200.0,
        "lootTt": 180.0,
        "regularSkillTt": 10.0,
        "attributeLevelsTotal": 0.0,
        "regularSkillPed": {"Handgun": 10.0},
        "attributeLevels": {},
        "dominantTag": "PvE",
        "dominantMob": "Atrox",
        "dominantWeapon": "Sollomate",
    }
]


def test_prospect_forecasts_a_reachable_target(app_db, monkeypatch):
    _seed_calibration(app_db, "Handgun", 50.0)  # current Laser Pistoleer level 0.5
    monkeypatch.setattr(
        character, "_load_prospect_sessions", lambda _db: list(_PROSPECT_SESSIONS)
    )
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=_game_data(_PROFESSIONS)),
    )

    result = character.get_character_prospect(
        profession="Laser Pistoleer", target_level=0.6, markup_uplift=0.5
    )

    assert result["currentLevel"] == 0.5
    assert result["projectedCycledPed"] > 0
    assert result["rows"]  # per-skill projection rows
    assert result["speculativeLootTt"] is not None  # markup_uplift > 0 branch


def test_prospect_options_list_observed_slices(app_db, monkeypatch):
    monkeypatch.setattr(
        character, "_load_prospect_sessions", lambda _db: list(_PROSPECT_SESSIONS)
    )
    monkeypatch.setattr(
        character, "get_services", lambda: SimpleNamespace(app_db=app_db)
    )

    result = character.get_character_prospect_options()

    assert any(opt["value"] == "Atrox" for opt in result["mobs"])
    assert any(opt["value"] == "PvE" for opt in result["tags"])
    assert any(opt["value"] == "Sollomate" for opt in result["weapons"])


def test_prospect_high_target_exercises_the_search(app_db, monkeypatch):
    """A target above the current level drives the doubling/bisection search,
    exercising the iterative projection and pinning the forecast it converges on."""
    _seed_calibration(app_db, "Handgun", 50.0)
    monkeypatch.setattr(
        character, "_load_prospect_sessions", lambda _db: list(_PROSPECT_SESSIONS)
    )
    monkeypatch.setattr(
        character,
        "get_services",
        lambda: SimpleNamespace(app_db=app_db, game_data=_game_data(_PROFESSIONS)),
    )

    result = character.get_character_prospect(
        profession="Laser Pistoleer", target_level=1.0
    )

    assert result["currentLevel"] == 0.5
    # Reaching Laser Pistoleer level 1.0 from 0.5 needs only ~1.2 PED of cycling
    # with this sample, so the bisection converges well inside the observed
    # sample (200 PED) and emits no long-extrapolation warning. Pin the
    # converged forecast so a mutant breaking the search or the hours/rows
    # derivation cannot survive behind a key-existence check.
    assert result["projectedCycledPed"] == pytest.approx(1.2, abs=1e-2)
    assert result["projectedHours"] == pytest.approx(0.01, abs=1e-3)
    assert result["projectedHours"] > 0
    assert result["rows"]  # per-skill projection rows
    assert "Long extrapolation" not in " ".join(result["warnings"])
