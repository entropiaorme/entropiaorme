"""Tests for CodexService: in-memory DB, stub game-data catalogue."""

import pytest

from backend.db.app_database import AppDatabase
from backend.services.codex_service import CodexService


class _StubGameData:
    """Minimal GameDataStore stand-in for tests."""

    def __init__(self) -> None:
        self._data: dict[str, list[dict]] = {}

    def store(self, endpoint: str, entities: list[dict]) -> None:
        self._data[endpoint] = entities

    def get_entities(self, endpoint: str) -> list[dict]:
        return self._data.get(endpoint, [])


def _make_mob(species_name: str, base_cost: float, codex_type: str = "Mob") -> dict:
    """Build a minimal mob entity matching the catalogue shape."""
    return {
        "id": 1,
        "name": f"{species_name} Young",
        "species": {
            "name": species_name,
            "codex_base_cost": base_cost,
            "codex_type": codex_type,
        },
    }


def _make_skill(name: str, hp_increase: float) -> dict:
    """Build a minimal skill entity matching the catalogue shape."""
    return {"name": name, "hp_increase": hp_increase}


@pytest.fixture
def app_db(tmp_path):
    db = AppDatabase(tmp_path / "test_app.db")
    return db


@pytest.fixture
def game_data() -> _StubGameData:
    store = _StubGameData()
    mobs = [
        _make_mob("Atrox", 100, "MobLooter"),
        _make_mob("Feffoid", 50, "Mob"),
        # Duplicate maturity for Atrox; should be deduped
        {**_make_mob("Atrox", 100, "MobLooter"), "name": "Atrox Old"},
    ]
    skills = [
        _make_skill("Aim", 1600),
        _make_skill("Rifle", 500),
        _make_skill("Anatomy", 0),
    ]
    store.store("mobs", mobs)
    store.store("skills", skills)
    return store


@pytest.fixture
def service(app_db, game_data):
    return CodexService(app_db, game_data)


# ── get_all_species ─────────────────────────────────────────────────────────


def test_get_all_species_deduped(service):
    species = service.get_all_species()
    names = [s["name"] for s in species]
    assert names.count("Atrox") == 1
    assert "Feffoid" in names


def test_get_all_species_has_fields(service):
    species = service.get_all_species()
    atrox = next(s for s in species if s["name"] == "Atrox")
    assert atrox["baseCost"] == 100
    assert atrox["codexType"] == "MobLooter"
    assert atrox["currentRank"] == 0
    assert atrox["nextRank"] == 1
    assert atrox["nextCategory"] == "cat1"
    assert atrox["nextCost"] == 100.0  # multiplier[0]=1 × 100


def test_get_all_species_sort_order(service):
    """Rank desc then name asc."""
    # Calibrate Feffoid to rank 3
    service.calibrate("Feffoid", 3)
    species = service.get_all_species()
    # Feffoid (rank 3) should come before Atrox (rank 0)
    names = [s["name"] for s in species]
    assert names.index("Feffoid") < names.index("Atrox")


# ── get_species_ranks ───────────────────────────────────────────────────────


def test_get_species_ranks_returns_25(service):
    result = service.get_species_ranks("Atrox")
    assert result is not None
    assert len(result["ranks"]) == 25
    assert result["currentRank"] == 0


def test_get_species_ranks_unknown(tmp_path):
    """Unknown species returns None."""
    app_db = AppDatabase(tmp_path / "empty_app.db")
    # _StubGameData is a deliberate minimal stand-in for the heavy GameDataStore.
    svc = CodexService(app_db, _StubGameData())  # type: ignore[arg-type]
    assert svc.get_species_ranks("Nonexistent") is None
    app_db.close()


def test_get_species_ranks_marks_next(service):
    result = service.get_species_ranks("Atrox")
    assert result["ranks"][0]["isNext"] is True  # rank 1
    assert result["ranks"][1]["isNext"] is False


# ── claim_rank ──────────────────────────────────────────────────────────────


def test_claim_rank_success(service):
    result = service.claim_rank("Atrox", 1, "Aim")  # cat1 skill
    assert result["rank"] == 1
    assert result["skillName"] == "Aim"
    # Deterministic reward: get_rank_cost(1, 100) = 100, cat1 divisor 200 → 0.5 PED.
    # Pins the divisor/multiplier selection so a mutant that keeps a positive
    # value but corrupts the arithmetic is caught.
    assert result["pedValue"] == 0.5

    # Verify progress updated
    species = service.get_all_species()
    atrox = next(s for s in species if s["name"] == "Atrox")
    assert atrox["currentRank"] == 1


def test_claim_rank_wrong_rank(service):
    with pytest.raises(ValueError, match="Expected rank 1"):
        service.claim_rank("Atrox", 2, "Aim")


def test_claim_rank_invalid_skill(service):
    with pytest.raises(ValueError, match="not valid"):
        service.claim_rank("Atrox", 1, "Zoology")  # cat4 skill, not valid for rank 1


def test_claim_rank_cat4_skill_on_rank_5(service):
    """MobLooter species can claim cat4 skills on rank 5."""
    # Set up ranks 1-4
    for i in range(1, 5):
        cat_skills = {"cat1": "Aim", "cat2": "Clubs"}
        from backend.data.codex_categories import get_category_for_rank

        cat = get_category_for_rank(i)
        service.claim_rank("Atrox", i, cat_skills[cat])

    # Rank 5 should allow cat4 skill
    result = service.claim_rank("Atrox", 5, "Zoology")
    assert result["skillName"] == "Zoology"


def test_claim_rank_persists_codex_claim(service, app_db):
    service.claim_rank("Feffoid", 1, "Rifle")
    rows = app_db.conn.execute(
        "SELECT species_name, rank, skill_name, ped_value, kind FROM codex_claims "
        "WHERE species_name = 'Feffoid'"
    ).fetchall()
    assert len(rows) == 1
    assert rows[0]["species_name"] == "Feffoid"
    assert rows[0]["rank"] == 1
    assert rows[0]["skill_name"] == "Rifle"
    assert rows[0]["ped_value"] > 0
    assert rows[0]["kind"] == "rank"

    # Codex claims must not leak into the activity ledger anymore.
    ledger_rows = app_db.conn.execute(
        "SELECT id FROM ledger_entries WHERE tag = 'codex'"
    ).fetchall()
    assert ledger_rows == []


def test_meta_claim_persists_with_kind_meta(service, app_db):
    result = service.meta_claim("Strength")
    assert result["attributeName"] == "Strength"
    assert result["pedValue"] == 1.0

    rows = app_db.conn.execute(
        "SELECT kind, attribute_name, species_name, rank, skill_name, ped_value "
        "FROM codex_claims WHERE kind = 'meta'"
    ).fetchall()
    assert len(rows) == 1
    assert rows[0]["attribute_name"] == "Strength"
    assert rows[0]["species_name"] == "__meta__"
    assert rows[0]["rank"] == 0
    assert rows[0]["skill_name"] == "Strength"
    assert rows[0]["ped_value"] == 1.0

    # Meta claims must not leak into the activity ledger anymore.
    ledger_rows = app_db.conn.execute(
        "SELECT id FROM ledger_entries WHERE tag = 'codex'"
    ).fetchall()
    assert ledger_rows == []


def test_claim_rank_updates_skill_calibration(service, app_db):
    """If skill is calibrated, claiming should update the level."""
    # Pre-calibrate the skill
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Aim", 500.0, 1000.0),
    )
    app_db.conn.commit()

    service.claim_rank("Atrox", 1, "Aim")

    # Should have a new calibration entry with higher level
    rows = app_db.conn.execute(
        "SELECT level, source FROM skill_calibrations WHERE skill_name = 'Aim' ORDER BY scanned_at DESC"
    ).fetchall()
    assert len(rows) == 2
    assert rows[0]["source"] == "codex"
    assert rows[0]["level"] > 500.0


# ── calibrate ───────────────────────────────────────────────────────────────


def test_calibrate_set_and_get(service):
    service.calibrate("Atrox", 10)
    species = service.get_all_species()
    atrox = next(s for s in species if s["name"] == "Atrox")
    assert atrox["currentRank"] == 10


def test_calibrate_reset_to_zero(service):
    service.calibrate("Atrox", 5)
    service.calibrate("Atrox", 0)
    species = service.get_all_species()
    atrox = next(s for s in species if s["name"] == "Atrox")
    assert atrox["currentRank"] == 0


def test_calibrate_invalid_rank(service):
    with pytest.raises(ValueError):
        service.calibrate("Atrox", 26)


# ── get_skill_options ───────────────────────────────────────────────────────


def test_get_skill_options_rank1(service):
    options = service.get_skill_options("Atrox", 1)
    # Rank 1 = cat1 skills
    from backend.data.codex_categories import CODEX_SKILL_CATEGORIES

    cat1_names = set(CODEX_SKILL_CATEGORIES["cat1"])
    option_names = {o["skillName"] for o in options}
    assert option_names == cat1_names


def test_get_skill_options_rank5_mob_looter(service):
    """Rank 5 MobLooter should include both cat3 and cat4 skills."""
    options = service.get_skill_options("Atrox", 5)
    from backend.data.codex_categories import CODEX_SKILL_CATEGORIES

    expected = set(CODEX_SKILL_CATEGORIES["cat3"]) | set(CODEX_SKILL_CATEGORIES["cat4"])
    actual = {o["skillName"] for o in options}
    assert actual == expected


def test_get_skill_options_rank5_regular_mob(service):
    """Rank 5 regular Mob should only have cat3 skills."""
    options = service.get_skill_options("Feffoid", 5)
    from backend.data.codex_categories import CODEX_SKILL_CATEGORIES

    expected = set(CODEX_SKILL_CATEGORIES["cat3"])
    actual = {o["skillName"] for o in options}
    assert actual == expected


def test_get_skill_options_with_profession(service, game_data, app_db):
    """Profession contribution should be computed and drive sorting."""
    # Seed a dummy profession
    game_data.store(
        "professions",
        [
            {
                "name": "Laser Sniper (Hit)",
                "skills": [
                    {"skill": {"name": "Aim"}, "weight": 50},
                    {"skill": {"name": "Rifle"}, "weight": 30},
                ],
            }
        ],
    )

    # Calibrate: Aim at high level (diminishing returns), Rifle at low level
    import time

    now = time.time()
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Aim", 5000.0, now),
    )
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Rifle", 100.0, now),
    )
    app_db.conn.commit()

    options = service.get_skill_options("Atrox", 1, profession="Laser Sniper (Hit)")
    aim_opt = next(o for o in options if o["skillName"] == "Aim")
    rifle_opt = next(o for o in options if o["skillName"] == "Rifle")

    # Both should have new fields
    assert aim_opt["professionWeight"] == 50
    assert aim_opt["currentLevel"] == 5000.0
    assert aim_opt["levelsGained"] > 0
    assert aim_opt["profContribution"] > 0
    assert rifle_opt["professionWeight"] == 30
    assert rifle_opt["currentLevel"] == 100.0

    # Rifle at low level should gain MORE levels than Aim at high level
    assert rifle_opt["levelsGained"] > aim_opt["levelsGained"]

    # Despite lower weight, Rifle's contribution may be higher due to more levels gained
    # The sorting should put the higher profContribution first
    ranked = [o for o in options if o["professionWeight"] > 0]
    assert ranked[0]["recommendRank"] == 1
    assert ranked[0]["profContribution"] >= ranked[1]["profContribution"]

    # Non-profession skills should have recommendRank = None
    non_prof = [o for o in options if o["professionWeight"] == 0]
    assert all(o["recommendRank"] is None for o in non_prof)


def test_get_skill_options_with_hp_target(service, app_db):
    """HP mode should rank by expected HP gain from the reward."""
    import time

    now = time.time()
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Aim", 5000.0, now),
    )
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
        ("Rifle", 100.0, now),
    )
    app_db.conn.commit()

    options = service.get_skill_options("Atrox", 1, target="hp")
    aim_opt = next(o for o in options if o["skillName"] == "Aim")
    rifle_opt = next(o for o in options if o["skillName"] == "Rifle")

    assert aim_opt["hpIncrease"] == 1600
    assert rifle_opt["hpIncrease"] == 500
    assert aim_opt["hpGain"] > 0
    assert rifle_opt["hpGain"] > 0
    assert rifle_opt["hpGain"] > aim_opt["hpGain"]

    ranked = [o for o in options if o["recommendRank"] is not None]
    assert ranked[0]["skillName"] == "Rifle"
    assert ranked[0]["recommendRank"] == 1


def test_get_skill_options_with_hp_target_leaves_non_hp_skills_unranked(service):
    """Skills without HpIncrease should stay visible but unranked in HP mode."""
    options = service.get_skill_options("Atrox", 1, target="hp")
    anatomy_opt = next(o for o in options if o["skillName"] == "Anatomy")

    assert anatomy_opt["hpIncrease"] is None
    assert anatomy_opt["hpGain"] == 0
    assert anatomy_opt["recommendRank"] is None
