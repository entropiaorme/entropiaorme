"""Mutation-hardening tests for CodexService (cluster codex_service__c0).

Targets get_all_species, get_species_ranks, claim_rank. Uses an in-memory
SQLite AppDatabase and a minimal GameDataStore stand-in, mirroring the
fixtures in test_codex_service.py but written to pin the exact behaviour each
surviving mutant breaks.
"""

import logging
from typing import cast

import pytest

from backend.data.codex_categories import (
    CODEX_SKILL_CATEGORIES,
    get_category_for_rank,
)
from backend.db.app_database import AppDatabase
from backend.services.codex_service import CodexService
from backend.services.game_data_store import GameDataStore


class _StubGameData:
    """Minimal GameDataStore stand-in for tests."""

    def __init__(self) -> None:
        self._data: dict[str, list[dict]] = {}

    def store(self, endpoint: str, entities: list[dict]) -> None:
        self._data[endpoint] = entities

    def get_entities(self, endpoint: str) -> list[dict]:
        return self._data.get(endpoint, [])


def _make_mob(species_name: str, base_cost, codex_type: str = "Mob", **extra) -> dict:
    species = {
        "name": species_name,
        "codex_base_cost": base_cost,
        "codex_type": codex_type,
    }
    species.update(extra)
    return {"id": 1, "name": f"{species_name} Young", "species": species}


@pytest.fixture
def app_db(tmp_path):
    return AppDatabase(tmp_path / "test_app.db")


@pytest.fixture
def game_data() -> _StubGameData:
    store = _StubGameData()
    store.store(
        "mobs",
        [
            _make_mob("Atrox", 100, "MobLooter"),
            _make_mob("Feffoid", 50, "Mob"),
            {**_make_mob("Atrox", 100, "MobLooter"), "name": "Atrox Old"},
        ],
    )
    store.store(
        "skills",
        [
            {"name": "Aim", "hp_increase": 1600},
            {"name": "Rifle", "hp_increase": 500},
            {"name": "Anatomy", "hp_increase": 0},
        ],
    )
    return store


@pytest.fixture
def service(app_db, game_data):
    return CodexService(app_db, game_data)


# ── get_all_species ──────────────────────────────────────────────────────────


def test_get_all_species_skips_mob_without_species(app_db):
    """A mob with no 'species' key (or falsy species) must be skipped, not crash."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            {"id": 1, "name": "Headless mob"},  # no 'species'
            {"id": 2, "name": "Empty", "species": None},  # falsy species
            _make_mob("Atrox", 100, "MobLooter"),
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    names = [s["name"] for s in svc.get_all_species()]
    assert names == ["Atrox"]


def test_get_all_species_skips_empty_name(app_db):
    """Species with empty/missing name must be skipped."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            _make_mob("", 100, "MobLooter"),  # empty name
            {"id": 2, "name": "x", "species": {"codex_base_cost": 7}},  # missing name
            _make_mob("Feffoid", 50, "Mob"),
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    names = [s["name"] for s in svc.get_all_species()]
    assert names == ["Feffoid"]


def test_get_all_species_dedup_keeps_first(app_db):
    """Duplicate species name is deduped: only one entry, first base_cost wins."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            _make_mob("Atrox", 100, "MobLooter"),
            {**_make_mob("Atrox", 999, "Mob"), "name": "Atrox Old"},
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    species = svc.get_all_species()
    assert len(species) == 1
    assert species[0]["name"] == "Atrox"
    assert species[0]["baseCost"] == 100
    assert species[0]["codexType"] == "MobLooter"


def test_get_all_species_skips_missing_base_cost(app_db):
    """Species with codex_base_cost None must be skipped entirely."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            {"id": 1, "name": "x", "species": {"name": "NoCost", "codex_type": "Mob"}},
            _make_mob("Feffoid", 50, "Mob"),
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    names = [s["name"] for s in svc.get_all_species()]
    assert names == ["Feffoid"]


def test_get_all_species_base_cost_zero_is_kept(app_db):
    """codex_base_cost of 0 is a real value (only None is skipped)."""
    gd = _StubGameData()
    gd.store("mobs", [_make_mob("ZeroCost", 0, "Mob")])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    species = svc.get_all_species()
    assert len(species) == 1
    assert species[0]["name"] == "ZeroCost"
    assert species[0]["baseCost"] == 0


def test_get_all_species_codex_type_none_when_missing(app_db):
    """codex_type defaults to None (via .get) when absent."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [{"id": 1, "name": "x", "species": {"name": "Plain", "codex_base_cost": 10}}],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    species = svc.get_all_species()
    assert species[0]["codexType"] is None


def test_get_all_species_rank_defaults_zero(service):
    """A species with no codex_progress row defaults to rank 0."""
    atrox = next(s for s in service.get_all_species() if s["name"] == "Atrox")
    assert atrox["currentRank"] == 0
    assert atrox["nextRank"] == 1


def test_get_all_species_next_fields_for_rank_zero(service):
    """At rank 0: nextRank=1, nextCategory=cat1, nextCost=baseCost*mult[0]/... ."""
    atrox = next(s for s in service.get_all_species() if s["name"] == "Atrox")
    assert atrox["nextRank"] == 1
    assert atrox["nextCategory"] == "cat1"
    # get_rank_cost(1, 100) = CODEX_MULTIPLIERS[0]=1 * 100 = 100
    assert atrox["nextCost"] == 100.0


def test_get_all_species_next_rank_at_24(service):
    """At rank 24, nextRank is 25 (rank < 25 boundary)."""
    service.calibrate("Atrox", 24)
    atrox = next(s for s in service.get_all_species() if s["name"] == "Atrox")
    assert atrox["currentRank"] == 24
    assert atrox["nextRank"] == 25
    assert atrox["nextCategory"] is not None
    assert atrox["nextCost"] is not None


def test_get_all_species_rank_25_has_no_next(service):
    """At rank 25 (== 25, not < 25): nextRank/Category/Cost all None."""
    service.calibrate("Atrox", 25)
    atrox = next(s for s in service.get_all_species() if s["name"] == "Atrox")
    assert atrox["currentRank"] == 25
    assert atrox["nextRank"] is None
    assert atrox["nextCategory"] is None
    assert atrox["nextCost"] is None


def test_get_all_species_next_cost_rounded_2dp(app_db):
    """nextCost is rounded to 2 decimals."""
    gd = _StubGameData()
    # base_cost chosen so cost/divisor has >2 decimals before rounding.
    # rank 5 -> cat3 (divisor 640), mult[4]=6. Use rank 4 here via calibrate.
    gd.store("mobs", [_make_mob("Odd", 33.333, "Mob")])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    sp = svc.get_all_species()[0]
    # next_cost uses get_rank_cost (mult*base), rounded to 2dp.
    # rank 1 mult=1 -> 33.333 -> round 2dp = 33.33
    assert sp["nextCost"] == 33.33


def test_get_all_species_sort_rank_desc_then_name_asc(app_db):
    """Sort key is (-currentRank, name): rank desc, then name asc within a rank."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            _make_mob("Zeta", 10, "Mob"),
            _make_mob("Alpha", 10, "Mob"),
            _make_mob("Mid", 10, "Mob"),
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    svc.calibrate("Mid", 5)
    names = [s["name"] for s in svc.get_all_species()]
    # Mid (rank 5) first; then Alpha, Zeta (rank 0) in name-asc order.
    assert names == ["Mid", "Alpha", "Zeta"]


def test_get_all_species_empty_when_no_mobs(app_db):
    svc = CodexService(app_db, cast(GameDataStore, _StubGameData()))
    assert svc.get_all_species() == []


# ── get_species_ranks ─────────────────────────────────────────────────────────


def test_get_species_ranks_unknown_returns_none(app_db):
    svc = CodexService(app_db, cast(GameDataStore, _StubGameData()))
    assert svc.get_species_ranks("Nope") is None


def test_get_species_ranks_basic_shape(service):
    result = service.get_species_ranks("Atrox")
    assert result is not None
    assert result["speciesName"] == "Atrox"
    assert result["baseCost"] == 100
    assert result["codexType"] == "MobLooter"
    assert result["currentRank"] == 0
    assert len(result["ranks"]) == 25
    assert [r["rank"] for r in result["ranks"]] == list(range(1, 26))


def test_get_species_ranks_current_rank_defaults_zero(service):
    """No codex_progress row -> currentRank 0 and rank 1 isNext."""
    result = service.get_species_ranks("Atrox")
    assert result["currentRank"] == 0
    assert result["ranks"][0]["isNext"] is True
    assert result["ranks"][1]["isNext"] is False


def test_get_species_ranks_current_rank_from_db(service):
    service.calibrate("Atrox", 7)
    result = service.get_species_ranks("Atrox")
    assert result["currentRank"] == 7
    # isNext marks exactly rank == current_rank + 1 == 8 (index 7).
    next_flags = [r["rank"] for r in result["ranks"] if r["isNext"]]
    assert next_flags == [8]


def test_get_species_ranks_claim_fields_unclaimed(service):
    """With no claims: claimed False, claimedSkill/Ped None for every rank."""
    result = service.get_species_ranks("Atrox")
    for item in result["ranks"]:
        assert item["claimed"] is False
        assert item["claimedSkill"] is None
        assert item["claimedPed"] is None


def test_get_species_ranks_claim_fields_after_claim(service, app_db):
    """A claimed rank surfaces claimed=True with the recorded skill and ped."""
    service.claim_rank("Atrox", 1, "Aim")
    result = service.get_species_ranks("Atrox")
    rank1 = result["ranks"][0]
    assert rank1["claimed"] is True
    assert rank1["claimedSkill"] == "Aim"
    assert rank1["claimedPed"] == pytest.approx(0.5)
    # An unclaimed rank stays empty.
    rank2 = result["ranks"][1]
    assert rank2["claimed"] is False
    assert rank2["claimedSkill"] is None
    assert rank2["claimedPed"] is None


def test_get_species_ranks_passes_codex_type_for_cat4(service):
    """build_rank_breakdown must receive the species codex_type so a MobLooter
    flags the cat4 bonus on rank 5; passing None there drops the bonus."""
    result = service.get_species_ranks("Atrox")  # Atrox is MobLooter
    rank5 = result["ranks"][4]
    assert rank5["rank"] == 5
    assert rank5["cat4Bonus"] is True
    assert rank5["cat4RewardPed"] is not None
    assert rank5["cat4Skills"]  # non-empty


def test_get_species_ranks_regular_mob_has_no_cat4(service):
    """A regular Mob (Feffoid) must NOT flag a cat4 bonus on rank 5."""
    result = service.get_species_ranks("Feffoid")
    rank5 = result["ranks"][4]
    assert rank5["cat4Bonus"] is False
    assert rank5["cat4RewardPed"] is None
    assert rank5["cat4Skills"] == []


def test_get_species_ranks_claims_scoped_to_species(app_db):
    """Claims for one species must not bleed into another species' breakdown."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [_make_mob("Atrox", 100, "MobLooter"), _make_mob("Feffoid", 50, "Mob")],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    svc.claim_rank("Atrox", 1, "Aim")
    feff = svc.get_species_ranks("Feffoid")
    assert feff is not None
    assert all(not item["claimed"] for item in feff["ranks"])


# ── claim_rank ─────────────────────────────────────────────────────────────────


def test_claim_rank_unknown_species_raises(service):
    with pytest.raises(ValueError, match="not found in game-data catalogue"):
        service.claim_rank("Ghost", 1, "Aim")


def test_claim_rank_must_be_next(service):
    with pytest.raises(ValueError, match="Expected rank 1, got 2"):
        service.claim_rank("Atrox", 2, "Aim")


def test_claim_rank_next_after_progress(service):
    service.calibrate("Atrox", 4)
    # rank 5 is next; rank 6 / rank 4 are wrong.
    with pytest.raises(ValueError, match="Expected rank 5, got 6"):
        service.claim_rank("Atrox", 6, "Alertness")
    with pytest.raises(ValueError, match="Expected rank 5, got 4"):
        service.claim_rank("Atrox", 4, "Alertness")


def test_claim_rank_invalid_skill_for_category(service):
    """A cat2 skill is not valid for a cat1 rank."""
    with pytest.raises(ValueError, match="not valid for rank 1"):
        service.claim_rank("Atrox", 1, "Clubs")


def test_claim_rank_cat4_not_valid_on_cat1_rank(service):
    with pytest.raises(ValueError, match="not valid"):
        service.claim_rank("Atrox", 1, "Zoology")


def test_claim_rank_success_returns_payload(service):
    result = service.claim_rank("Atrox", 1, "Aim")
    assert result == {
        "speciesName": "Atrox",
        "rank": 1,
        "skillName": "Aim",
        "pedValue": 0.5,
    }


def test_claim_rank_persists_claim_row(service, app_db):
    service.claim_rank("Feffoid", 1, "Rifle")
    rows = app_db.conn.execute(
        "SELECT species_name, rank, skill_name, ped_value, kind FROM codex_claims "
        "WHERE species_name = 'Feffoid'"
    ).fetchall()
    assert len(rows) == 1
    r = rows[0]
    assert r["species_name"] == "Feffoid"
    assert r["rank"] == 1
    assert r["skill_name"] == "Rifle"
    assert r["ped_value"] > 0
    assert r["kind"] == "rank"


def test_claim_rank_updates_progress(service, app_db):
    service.claim_rank("Atrox", 1, "Aim")
    row = app_db.conn.execute(
        "SELECT current_rank FROM codex_progress WHERE species_name = 'Atrox'"
    ).fetchone()
    assert row["current_rank"] == 1
    # second claim advances to 2
    service.claim_rank("Atrox", 2, "Aim")
    row = app_db.conn.execute(
        "SELECT current_rank FROM codex_progress WHERE species_name = 'Atrox'"
    ).fetchone()
    assert row["current_rank"] == 2


def test_claim_rank_cat4_skill_allowed_on_rank5_moblooter(service):
    for i in range(1, 5):
        cat = get_category_for_rank(i)
        skill = CODEX_SKILL_CATEGORIES[cat][0]
        service.claim_rank("Atrox", i, skill)
    result = service.claim_rank("Atrox", 5, "Zoology")
    assert result["skillName"] == "Zoology"
    assert result["rank"] == 5


def test_claim_rank_cat4_skill_uses_cat4_divisor(app_db):
    """A cat4 skill on a cat4 rank is rewarded with the cat4 divisor (1000),
    not the base category divisor."""
    from backend.data.codex_categories import get_reward_ped

    gd = _StubGameData()
    gd.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    gd.store("skills", [])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    for i in range(1, 5):
        cat = get_category_for_rank(i)
        svc.claim_rank("Atrox", i, CODEX_SKILL_CATEGORIES[cat][0])
    result = svc.claim_rank("Atrox", 5, "Zoology")
    # rank 5 base category is cat3; cat4 divisor differs from cat3.
    expected_cat4 = get_reward_ped(5, 100, "cat4")
    assert result["pedValue"] == expected_cat4
    assert expected_cat4 != get_reward_ped(5, 100, "cat3")


def test_claim_rank_non_cat4_skill_uses_category_divisor(app_db):
    """A regular cat3 skill on rank 5 uses the cat3 divisor, not cat4."""
    from backend.data.codex_categories import get_reward_ped

    gd = _StubGameData()
    gd.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    for i in range(1, 5):
        cat = get_category_for_rank(i)
        svc.claim_rank("Atrox", i, CODEX_SKILL_CATEGORIES[cat][0])
    result = svc.claim_rank("Atrox", 5, "Alertness")  # cat3 skill
    assert result["pedValue"] == get_reward_ped(5, 100, "cat3")


def test_claim_rank_updates_skill_calibration_when_calibrated(service, app_db):
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, 'scan', ?)",
        ("Aim", 500.0, 1000.0),
    )
    app_db.conn.commit()
    service.claim_rank("Atrox", 1, "Aim")
    rows = app_db.conn.execute(
        "SELECT level, source FROM skill_calibrations WHERE skill_name = 'Aim' "
        "ORDER BY scanned_at DESC"
    ).fetchall()
    assert len(rows) == 2
    assert rows[0]["source"] == "codex"
    assert rows[0]["level"] > 500.0


def test_claim_rank_no_calibration_when_skill_uncalibrated(service, app_db):
    """If the skill has no prior calibration, no codex calibration row is added."""
    service.claim_rank("Atrox", 1, "Aim")
    rows = app_db.conn.execute(
        "SELECT * FROM skill_calibrations WHERE skill_name = 'Aim'"
    ).fetchall()
    assert rows == []


def test_claim_rank_new_level_is_sum(app_db):
    """new_level = current_level + levels_for_tt_value(current_level, ped)."""
    from backend.data.tt_value_curve import levels_for_tt_value

    gd = _StubGameData()
    gd.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, 'scan', ?)",
        ("Aim", 500.0, 1000.0),
    )
    app_db.conn.commit()
    result = svc.claim_rank("Atrox", 1, "Aim")
    ped = result["pedValue"]
    expected = 500.0 + levels_for_tt_value(500.0, ped)
    row = app_db.conn.execute(
        "SELECT level FROM skill_calibrations WHERE skill_name = 'Aim' "
        "AND source = 'codex' ORDER BY scanned_at DESC LIMIT 1"
    ).fetchone()
    assert row["level"] == pytest.approx(expected)


def test_claim_rank_does_not_exceed_via_progress(service):
    """Claiming beyond current+1 is always rejected as wrong rank."""
    service.calibrate("Atrox", 1)
    with pytest.raises(ValueError, match="Expected rank 2"):
        service.claim_rank("Atrox", 3, "Aim")


def test_claim_rank_26_rejected_with_max_message(service):
    """When current_rank is 25, the only 'next' rank is 26, which must be
    rejected by the rank>25 guard with the exact 'Maximum rank is 25' message
    (not a downstream IndexError, not a mangled message)."""
    service.calibrate("Atrox", 25)
    with pytest.raises(ValueError, match="^Maximum rank is 25$"):
        service.claim_rank("Atrox", 26, "Aim")


def test_claim_rank_logs_exact_message(service, caplog):
    """The success path logs an INFO record whose rendered message is exactly
    'Codex claim: <species> rank <n> → <skill> (<ped> PES)'. Pins the log
    format string and its %-argument list (corruptions render differently or
    fail to format)."""
    caplog.set_level(logging.INFO, logger="backend.services.codex_service")
    service.claim_rank("Atrox", 1, "Aim")

    claim_records = [
        r
        for r in caplog.records
        if r.name == "backend.services.codex_service" and r.levelno == logging.INFO
    ]
    assert claim_records, "expected an INFO log record from claim_rank"
    # getMessage() renders msg % args; a broken format string / arg list either
    # renders a different string or raises here - both fail the test.
    rendered = [r.getMessage() for r in claim_records]
    assert "Codex claim: Atrox rank 1 → Aim (0.5000 PES)" in rendered
