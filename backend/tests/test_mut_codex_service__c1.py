"""Mutation-hardening tests for CodexService cluster codex_service__c1.

Targets surviving / no-test / timeout mutants in:
  CodexService.calibrate, .get_skill_options, .meta_claim,
  .get_meta_attributes, ._find_species, ._get_skill_level

Uses an in-memory-ish SQLite (tmp file) AppDatabase plus a stub GameDataStore,
mirroring backend/tests/test_codex_service.py.
"""

from typing import cast

import pytest

from backend.data.codex_categories import CODEX_SKILL_CATEGORIES
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


def _make_mob(species_name: str, base_cost: float, codex_type: str = "Mob") -> dict:
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
    return {"name": name, "hp_increase": hp_increase}


@pytest.fixture
def app_db(tmp_path):
    return AppDatabase(tmp_path / "test_app.db")


@pytest.fixture
def game_data() -> _StubGameData:
    store = _StubGameData()
    mobs = [
        _make_mob("Atrox", 100, "MobLooter"),
        _make_mob("Feffoid", 50, "Mob"),
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


# Reward PED for rank-1 cat1 on base-cost 100 is deterministically 0.5.
RANK1_CAT1_PED = 0.5


def _cal(app_db, skill_name: str, level: float, when: float = 1000.0) -> None:
    """Insert a skill/attribute calibration row directly."""
    app_db.conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, 'scan', ?)",
        (skill_name, level, when),
    )
    app_db.conn.commit()


# ── calibrate: error-message mutants (6,7,8,9) ───────────────────────────────


def test_calibrate_rejects_out_of_range_with_exact_message(service):
    """Pins the exact ValueError text so ValueError(None) / casing / wording
    mutants on the guard message are caught (calibrate mutmut_6,7,8,9)."""
    for bad in (-1, 26):
        with pytest.raises(ValueError) as exc:
            service.calibrate("Atrox", bad)
        assert str(exc.value) == "Rank must be 0-25"


def test_calibrate_accepts_boundaries(service):
    """0 and 25 are valid (guard is strict <0 / >25)."""
    assert service.calibrate("Atrox", 0)["rank"] == 0
    assert service.calibrate("Atrox", 25)["rank"] == 25


# ── meta_claim: error message (2) + log message (17-23) ──────────────────────


def test_meta_claim_invalid_attribute_message(service):
    """Pins the ValueError text on the attribute guard (meta_claim mutmut_2:
    ValueError(None) → message 'None')."""
    with pytest.raises(ValueError, match="is not an attribute"):
        service.meta_claim("Notanattr")


def test_meta_claim_logs_exact_message(service, caplog):
    """The INFO log line is the only observable for the log.* mutations
    (meta_claim mutmut_17,18,19,20,21,22,23). Assert the fully formatted
    record text. mutmut_17/23 make getMessage() raise → the list comp below
    raises → test fails → mutant killed."""
    import logging

    caplog.set_level(logging.INFO, logger="backend.services.codex_service")
    service.meta_claim("Strength")

    messages = [
        r.getMessage()
        for r in caplog.records
        if r.name == "backend.services.codex_service"
    ]
    assert "Codex meta claim: Strength (1.00 PES)" in messages


# ── get_meta_attributes: structure + calibrated level (all 16 'no tests') ────


def test_get_meta_attributes_shape_and_level(service, app_db):
    """Drives get_meta_attributes (previously untested). Calibrating Strength
    to 100.46 pins level rounding to 1 dp (=100.5) and the dict keys/structure.

    Kills get_meta_attributes mutmut_1 (result=None → AttributeError),
    _2 (sorted(None)), _3 (level=None), _4 (_get_skill_level(None)),
    _5 (append(None)), _6/_7 (name key), _8/_9/_10 (currentLevel key),
    _11 (round(None,1)), _12 (round(level,None)→int 100), _13 (round(1)),
    _14 (round(level)→100), _15 (round(level,2)→100.46), _16 (inverted guard)."""
    _cal(app_db, "Strength", 100.46)

    result = service.get_meta_attributes()

    assert isinstance(result, list)
    assert len(result) == 6
    names = [r["name"] for r in result]
    assert names == sorted(
        ["Agility", "Health", "Intelligence", "Psyche", "Stamina", "Strength"]
    )
    for r in result:
        assert set(r.keys()) == {"name", "currentLevel"}

    strength = next(r for r in result if r["name"] == "Strength")
    assert strength["currentLevel"] == 100.5

    # Uncalibrated attributes report None (pins the is-not-None guard, _16).
    agility = next(r for r in result if r["name"] == "Agility")
    assert agility["currentLevel"] is None


# ── _find_species: pre-target mob without species (mutmut_10) ────────────────


def test_find_species_skips_mob_without_species(app_db):
    """A mob lacking a 'species' key must be skipped (continue), not abort the
    scan (break). Kills _find_species mutmut_10 (continue → break): the target
    species sits AFTER a species-less mob, so 'break' would never reach it."""
    gd = _StubGameData()
    gd.store(
        "mobs",
        [
            {"id": 1, "name": "Ghost", "species": None},  # no species → skip
            _make_mob("Daikiba", 80, "Mob"),  # target, only reachable via continue
        ],
    )
    gd.store("skills", [])
    svc = CodexService(app_db, cast(GameDataStore, gd))

    ranks = svc.get_species_ranks("Daikiba")
    assert ranks is not None
    assert ranks["baseCost"] == 80
    assert ranks["codexType"] == "Mob"


# ── get_skill_options: category / field-key mutants ──────────────────────────


def test_get_skill_options_cat4_category_label(service):
    """Rank-5 MobLooter cat4 skills must carry category 'cat4' (kills gso
    mutmut_43 'XXcat4XX' and _44 'CAT4' on the cat4 entry tuple)."""
    options = service.get_skill_options("Atrox", 5)
    cat4_names = set(CODEX_SKILL_CATEGORIES["cat4"])
    cat4_opts = [o for o in options if o["skillName"] in cat4_names]
    assert cat4_opts  # rank-5 MobLooter exposes cat4
    for o in cat4_opts:
        assert o["category"] == "cat4"


def test_get_skill_options_dict_keys_present(service):
    """Every option exposes the canonical field keys (kills gso mutmut_150/_151
    'category', _152/_153/_154 'rewardPed')."""
    options = service.get_skill_options("Atrox", 1)
    assert options
    for o in options:
        assert "category" in o
        assert "rewardPed" in o
        assert o["category"] == "cat1"
        assert o["rewardPed"] == RANK1_CAT1_PED


# ── get_skill_options: profession 'skills' default + weight defaults ─────────


def test_get_skill_options_profession_without_skills_key(service, game_data):
    """A matched profession that lacks a 'skills' key must yield zero weights,
    not crash. Kills gso mutmut_55 (default []→None) and _57 (default removed):
    `for se in None` would raise TypeError."""
    game_data.store(
        "professions",
        [{"name": "Empty Prof"}],  # matches by name, no 'skills' key
    )
    options = service.get_skill_options("Atrox", 1, profession="Empty Prof")
    assert options
    assert all(o["professionWeight"] == 0 for o in options)


def test_get_skill_options_zero_weight_skill_stays_zero(service, game_data):
    """A profession skill with weight 0 keeps professionWeight 0 and
    profContribution 0.0, and is left unranked. Kills gso mutmut_78
    (`se.get('weight') or 0` → `or 1`), _128 (`weight > 0` → `> 1`)."""
    game_data.store(
        "professions",
        [
            {
                "name": "ZeroProf",
                "skills": [
                    {"skill": {"name": "Aim"}, "weight": 0},
                ],
            }
        ],
    )
    options = service.get_skill_options("Atrox", 1, profession="ZeroProf")
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["professionWeight"] == 0
    assert aim["profContribution"] == 0.0
    assert aim["recommendRank"] is None


def test_get_skill_options_weight_one_contributes(service, game_data):
    """A profession skill with weight exactly 1 must contribute (>0 guard) and
    be ranked. Kills gso mutmut_128 (`weight > 0` → `weight > 1`: weight-1 skill
    would wrongly fall to the 0.0 branch and stay unranked)."""
    game_data.store(
        "professions",
        [
            {
                "name": "OneProf",
                "skills": [
                    {"skill": {"name": "Aim"}, "weight": 1},
                ],
            }
        ],
    )
    options = service.get_skill_options("Atrox", 1, profession="OneProf")
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["professionWeight"] == 1
    assert aim["profContribution"] > 0.0
    assert aim["recommendRank"] == 1


def test_get_skill_options_no_profession_weight_zero(service):
    """With no profession, every weight is the default 0 (kills gso mutmut_117
    `weight_map.get(name, 0)` → `1`: would make all skills weight 1 and ranked).
    Also pins gso mutmut_129 (`else 0.0` → `1.0`): unweighted contribution is
    exactly 0.0."""
    options = service.get_skill_options("Atrox", 1)
    assert options
    for o in options:
        assert o["professionWeight"] == 0
        assert o["profContribution"] == 0.0
        assert o["recommendRank"] is None


# ── get_skill_options: levels_for_tt_value start-level default (gso_111) ─────


def test_get_skill_options_uncalibrated_uses_level_zero(service):
    """An uncalibrated skill feeds level 0 into the TT curve (`current_level or 0`).
    Kills gso mutmut_111 (`or 0` → `or 1`): level-0 reward buys 312.0 levels
    (rounded), level-1 would buy 311.0 (round of 310.995)."""
    options = service.get_skill_options("Atrox", 1)
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["currentLevel"] is None
    assert aim["levelsGained"] == 312.0


# ── get_skill_options: profContribution arithmetic (121,123,124,125) ─────────


def test_get_skill_options_prof_contribution_formula(service, game_data, app_db):
    """Pins profContribution = round(levelsGained * weight / 10000, 6).
    Aim calibrated to 5000 (levelsGained 3.33), weight 50 → 0.016665.
    Kills gso mutmut_121 (round(6)=6), _123 (*10000), _124 (/weight), _125 (/10001)."""
    _cal(app_db, "Aim", 5000.0)
    game_data.store(
        "professions",
        [{"name": "P", "skills": [{"skill": {"name": "Aim"}, "weight": 50}]}],
    )
    options = service.get_skill_options("Atrox", 1, profession="P")
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["currentLevel"] == 5000.0
    assert aim["levelsGained"] == 3.33
    assert aim["profContribution"] == 0.016665


# ── get_skill_options: currentLevel & levelsGained rounding ──────────────────


def test_get_skill_options_current_level_rounded_one_dp(service, app_db):
    """currentLevel rounds to 1 dp. Aim calibrated 100.46 → 100.5.
    Kills gso mutmut_159 (round(_,None)→int 100), _161 (round(_)→100),
    _162 (round(_,2)→100.46)."""
    _cal(app_db, "Aim", 100.46)
    options = service.get_skill_options("Atrox", 1)
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["currentLevel"] == 100.5


def test_get_skill_options_levels_gained_rounded_two_dp(service, app_db):
    """levelsGained rounds to 2 dp. Aim calibrated 5000 → 3.33.
    Kills gso mutmut_168 (round(_,None)→int 3), _170 (round(_)→3),
    _171 (round(_,3)→3.333)."""
    _cal(app_db, "Aim", 5000.0)
    options = service.get_skill_options("Atrox", 1)
    aim = next(o for o in options if o["skillName"] == "Aim")
    assert aim["levelsGained"] == 3.33


# ── get_skill_options: hp_increase handling (101,141,142,144,182,184,185,187) ─


@pytest.fixture
def hp_game_data() -> "_StubGameData":
    """Game-data with crafted hp_increase values on cat1 skills."""
    store = _StubGameData()
    store.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    store.store(
        "skills",
        [
            {"name": "Aim", "hp_increase": 1600},  # integer hp
            {"name": "Rifle", "hp_increase": 12.344},  # fractional → rounding probes
            {"name": "Anatomy", "hp_increase": 0.5},  # 0<h<=1 → guard probes
            {"name": "Dexterity"},  # missing hp_increase → None
        ],
    )
    return store


@pytest.fixture
def hp_service(app_db, hp_game_data):
    return CodexService(app_db, hp_game_data)


def test_get_skill_options_hp_increase_rounding_and_guards(hp_service):
    """Pins hpIncrease field rounding/guards.
    Rifle hp=12.344 → round 2dp 12.34 (kills gso mutmut_182 round(_,None)→int 12,
    _184 round(_)→12, _185 round(_,3)→12.344).
    Anatomy hp=0.5 (>0) → hpIncrease 0.5 reported (kills gso mutmut_187 guard
    `>0`→`>1`: would report None).
    Dexterity hp missing → hp_map default 0.0 → hpIncrease None (kills gso
    mutmut_101 default 0.0→1.0: would report 1.0)."""
    options = hp_service.get_skill_options("Atrox", 1, target="hp")
    rifle = next(o for o in options if o["skillName"] == "Rifle")
    anatomy = next(o for o in options if o["skillName"] == "Anatomy")
    dexterity = next(o for o in options if o["skillName"] == "Dexterity")

    assert rifle["hpIncrease"] == 12.34
    assert anatomy["hpIncrease"] == 0.5
    assert dexterity["hpIncrease"] is None


def test_get_skill_options_hp_gain_formula_and_guards(hp_service):
    """Pins hpGain = round(levelsGained / hpIncrease, 6) with the >0 guard.
    Aim uncalibrated raw levels_gained 311.995, hp 1600 → 0.194997.
    Kills gso mutmut_141 (`/` → `*`: 499192.0), _142 (round 7dp),
    _144 (guard `>0`→`>1`: Anatomy hp=0.5 would drop hpGain to 0.0)."""
    options = hp_service.get_skill_options("Atrox", 1, target="hp")
    aim = next(o for o in options if o["skillName"] == "Aim")
    anatomy = next(o for o in options if o["skillName"] == "Anatomy")

    # hpGain uses the raw (unrounded) levels_gained 311.995 / 1600.
    assert aim["hpGain"] == 0.194997
    # Anatomy hp_increase 0.5 (>0) → hpGain computed, non-zero.
    assert anatomy["hpGain"] > 0.0


# ── get_skill_options: HP-target secondary sort key (gso_206) ────────────────


def test_get_skill_options_hp_sort_tiebreak_does_not_crash(app_db):
    """HP-mode tiebreak uses `currentLevel if not None else inf`. Construct a
    hpGain tie between an uncalibrated and a calibrated skill so the secondary
    key is exercised. Kills gso mutmut_206 (`is not None` → `is None`): the
    mutant compares None against float('inf') and raises TypeError."""
    gd = _StubGameData()
    gd.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    # ped buys 311.995 levels at level 0, 3.333 at level 5000.
    # hpGain == 1.0 for both when hp_increase equals levelsGained.
    gd.store(
        "skills",
        [
            {"name": "Aim", "hp_increase": 311.995},  # uncalibrated → hpGain 1.0
            {"name": "Rifle", "hp_increase": 3.333},  # calibrated 5000 → hpGain 1.0
        ],
    )
    svc = CodexService(app_db, cast(GameDataStore, gd))
    _cal(app_db, "Rifle", 5000.0)

    options = svc.get_skill_options("Atrox", 1, target="hp")
    aim = next(o for o in options if o["skillName"] == "Aim")
    rifle = next(o for o in options if o["skillName"] == "Rifle")
    assert aim["hpGain"] == 1.0
    assert rifle["hpGain"] == 1.0
    # orig: finite level (Rifle) sorts before inf (Aim) on the hpGain tie.
    names_in_order = [o["skillName"] for o in options]
    assert names_in_order.index("Rifle") < names_in_order.index("Aim")


# ── get_skill_options: profession secondary sort key (gso_219) ───────────────


def test_get_skill_options_profession_weight_tiebreak(app_db):
    """profession-mode tiebreak sorts equal-contribution skills by DESCENDING
    weight (`-professionWeight`). Construct a profContribution tie (0.062399)
    with distinct weights: Aim weight 2 @ level 0, Rifle weight 3 @ level 192.
    Kills gso mutmut_219 (`-weight` → `+weight`): flips the tie order, so the
    higher-weight skill (Rifle) would no longer rank first."""
    gd = _StubGameData()
    gd.store("mobs", [_make_mob("Atrox", 100, "MobLooter")])
    gd.store("skills", [])
    svc = CodexService(app_db, cast(GameDataStore, gd))
    _cal(app_db, "Rifle", 192.0)
    gd.store(
        "professions",
        [
            {
                "name": "TieProf",
                "skills": [
                    {"skill": {"name": "Aim"}, "weight": 2},
                    {"skill": {"name": "Rifle"}, "weight": 3},
                ],
            }
        ],
    )

    options = svc.get_skill_options("Atrox", 1, profession="TieProf")
    aim = next(o for o in options if o["skillName"] == "Aim")
    rifle = next(o for o in options if o["skillName"] == "Rifle")
    # Tie on contribution, distinct weights.
    assert aim["profContribution"] == rifle["profContribution"] == 0.062399
    assert aim["professionWeight"] == 2
    assert rifle["professionWeight"] == 3
    # orig: higher weight (Rifle) ranks first on the contribution tie.
    assert rifle["recommendRank"] == 1
    assert aim["recommendRank"] == 2
