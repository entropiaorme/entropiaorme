"""Property-based tests for codex category data and rank derivations.

Covers ``backend.data.codex_categories``: skill-to-category mapping, rank
cycling, rank cost / reward formulas, and the 25-row breakdown builder.

Also covers the service surface ``backend.services.codex_service``: the
claim-rank validation gate, the side-effect boundary of calibrate, the
meta-claim constant-reward contract, and the dense recommend-rank ordering
returned by the skill-options recommender.
"""

import tempfile
import uuid
from pathlib import Path

import pytest
from hypothesis import assume, given
from hypothesis import strategies as st

from backend.data.codex_categories import (
    CODEX_MULTIPLIERS,
    CODEX_SKILL_CATEGORIES,
    REWARD_DIVISORS,
    build_rank_breakdown,
    get_category_for_rank,
    get_codex_category,
    get_rank_cost,
    get_reward_ped,
    is_cat4_rank,
)
from backend.db.app_database import AppDatabase
from backend.services.codex_service import CodexService

_ALL_SKILLS = sorted({s for skills in CODEX_SKILL_CATEGORIES.values() for s in skills})
_BASE_COST = st.floats(
    min_value=0.0, max_value=1e6, allow_nan=False, allow_infinity=False
)


# --- category membership ---


def test_categories_are_disjoint():
    seen: set[str] = set()
    for skills in CODEX_SKILL_CATEGORIES.values():
        for skill in skills:
            assert skill not in seen
            seen.add(skill)


@given(st.sampled_from(_ALL_SKILLS))
def test_catalogued_skill_maps_back_to_its_category(skill):
    cat = get_codex_category(skill)
    assert cat is not None
    assert skill in CODEX_SKILL_CATEGORIES[cat]


@given(st.text())
def test_unknown_skill_has_no_category(name):
    assume(name not in _ALL_SKILLS)
    assert get_codex_category(name) is None


# --- rank cycling ---


@given(st.integers(min_value=1, max_value=200))
def test_category_for_rank_is_mod5_periodic(rank):
    mod = rank % 5
    expected = "cat1" if mod in (1, 2) else "cat2" if mod in (3, 4) else "cat3"
    assert get_category_for_rank(rank) == expected
    assert get_category_for_rank(rank) == get_category_for_rank(rank + 5)


@given(
    st.integers(min_value=1, max_value=25),
    st.sampled_from([None, "MobLooter", "Crafter"]),
)
def test_is_cat4_rank_only_on_mob_looter_5_15_25(rank, codex_type):
    result = is_cat4_rank(rank, codex_type)
    assert result == (codex_type == "MobLooter" and rank % 10 == 5)
    if result:
        assert rank in (5, 15, 25)


# --- rank cost / reward ---


@given(st.integers(min_value=1, max_value=25), _BASE_COST)
def test_rank_cost_is_linear_and_rank1_equals_base(rank, base):
    assert get_rank_cost(1, base) == pytest.approx(base)
    assert get_rank_cost(rank, base) == pytest.approx(
        CODEX_MULTIPLIERS[rank - 1] * base
    )


@given(st.integers(min_value=1, max_value=24), _BASE_COST)
def test_rank_cost_is_non_decreasing_in_rank(rank, base):
    assert get_rank_cost(rank + 1, base) + 1e-9 >= get_rank_cost(rank, base)


@given(st.integers(min_value=1, max_value=25), _BASE_COST)
def test_reward_is_non_increasing_across_categories(rank, base):
    r1 = get_reward_ped(rank, base, "cat1")
    r2 = get_reward_ped(rank, base, "cat2")
    r3 = get_reward_ped(rank, base, "cat3")
    r4 = get_reward_ped(rank, base, "cat4")
    # Larger divisor (cat1 < cat2 < cat3 < cat4) yields a smaller reward.
    assert r1 + 1e-9 >= r2 >= r3 - 1e-9
    assert r3 + 1e-9 >= r4


@given(
    st.integers(min_value=1, max_value=25),
    st.sampled_from(sorted(REWARD_DIVISORS)),
    _BASE_COST,
)
def test_reward_matches_cost_over_divisor(rank, category, base):
    expected = get_rank_cost(rank, base) / REWARD_DIVISORS[category]
    assert get_reward_ped(rank, base, category) == pytest.approx(expected, abs=1e-4)


# --- breakdown builder ---


@given(
    st.floats(min_value=0.01, max_value=1000.0, allow_nan=False, allow_infinity=False),
    st.sampled_from([None, "MobLooter"]),
)
def test_build_rank_breakdown_is_25_consistent_rows(base, codex_type):
    rows = build_rank_breakdown(base, codex_type)
    assert len(rows) == 25
    for expected_rank, row in enumerate(rows, start=1):
        assert row["rank"] == expected_rank
        assert row["category"] == get_category_for_rank(expected_rank)
        assert row["cost"] == pytest.approx(
            round(get_rank_cost(expected_rank, base), 2)
        )
        assert row["cat4Bonus"] == is_cat4_rank(expected_rank, codex_type)
        # The skills list is a fresh copy, never an alias of the module data.
        assert row["skills"] == CODEX_SKILL_CATEGORIES[row["category"]]
        assert row["skills"] is not CODEX_SKILL_CATEGORIES[row["category"]]
        if row["cat4Bonus"]:
            assert row["cat4RewardPed"] is not None
            assert row["cat4Skills"] == list(CODEX_SKILL_CATEGORIES["cat4"])
        else:
            assert row["cat4RewardPed"] is None
            assert row["cat4Skills"] == []


# --- service surface: backend.services.codex_service ---
#
# These properties drive a real CodexService over an in-process AppDatabase
# (sqlite) and a minimal in-memory game-data stand-in, the same way the
# example-based service tests do. Each example gets a fresh database so writes
# from one generated input never leak into the next.

_MOB_LOOTER = "PropLooter"
_REGULAR_MOB = "PropMob"
_BASE_COST_VALUE = 100.0
_PROFESSION = "Prop Profession"

# All catalogued codex skills, so every category a rank can map to is populated.
_ALL_CODEX_SKILLS = sorted(
    {s for skills in CODEX_SKILL_CATEGORIES.values() for s in skills}
)


class _StubGameData:
    """Minimal GameDataStore stand-in: returns canned entity lists."""

    def __init__(self, data):
        self._data = data

    def get_entities(self, endpoint):
        return self._data.get(endpoint, [])


def _stub_game_data():
    def mob(species_name, codex_type):
        return {
            "id": 1,
            "name": f"{species_name} Young",
            "species": {
                "name": species_name,
                "codex_base_cost": _BASE_COST_VALUE,
                "codex_type": codex_type,
            },
        }

    # Give every codex skill a non-zero profession weight and a distinct
    # hp_increase so both the profession and hp recommend-rank branches have a
    # non-empty relevant set to order.
    profession_skills = [
        {"skill": {"name": name}, "weight": (i % 7) + 1}
        for i, name in enumerate(_ALL_CODEX_SKILLS)
    ]
    skills = [
        {"name": name, "hp_increase": float((i % 5) + 1) * 100.0}
        for i, name in enumerate(_ALL_CODEX_SKILLS)
    ]
    return _StubGameData(
        {
            "mobs": [mob(_MOB_LOOTER, "MobLooter"), mob(_REGULAR_MOB, "Mob")],
            "professions": [{"name": _PROFESSION, "skills": profession_skills}],
            "skills": skills,
        }
    )


class _fresh_service:
    """Context manager yielding a CodexService over a throwaway sqlite file.

    Each generated input gets its own database and the file is torn down on
    exit, so writes never leak between examples (a function-scoped fixture
    would be shared across @given inputs, which is why we manage it inline).
    """

    def __enter__(self):
        self._dir = tempfile.TemporaryDirectory()
        path = Path(self._dir.name) / f"codex_{uuid.uuid4().hex}.db"
        self.db = AppDatabase(path)
        # _StubGameData is a minimal stand-in for the heavy GameDataStore.
        svc = CodexService(self.db, _stub_game_data())
        return svc, self.db

    def __exit__(self, *exc):
        self.db.close()
        self._dir.cleanup()
        return False


def _valid_skills_for(rank, codex_type):
    category = get_category_for_rank(rank)
    valid = set(CODEX_SKILL_CATEGORIES[category])
    if is_cat4_rank(rank, codex_type):
        valid |= set(CODEX_SKILL_CATEGORIES["cat4"])
    return valid


def _count_rows(db, table, where=""):
    return db.conn.execute(f"SELECT COUNT(*) AS n FROM {table} {where}").fetchone()["n"]


# claim_rank: the validation gate only admits a skill that belongs to the
# rank's category (plus cat4 on a MobLooter cat4 rank), and a rejected claim
# writes nothing.


@given(
    species=st.sampled_from([_MOB_LOOTER, _REGULAR_MOB]),
    rank=st.integers(min_value=1, max_value=25),
    skill=st.sampled_from(_ALL_CODEX_SKILLS),
)
def test_claim_rank_skill_must_match_category(species, rank, skill):
    with _fresh_service() as (svc, db):
        codex_type = "MobLooter" if species == _MOB_LOOTER else "Mob"
        # claim_rank requires the rank to be exactly current_rank + 1.
        if rank > 1:
            svc.calibrate(species, rank - 1)
        valid = _valid_skills_for(rank, codex_type)

        claims_before = _count_rows(
            db, "codex_claims", f"WHERE species_name = '{species}'"
        )
        if skill in valid:
            result = svc.claim_rank(species, rank, skill)
            assert result["skillName"] == skill
            # The accepted skill is exactly one drawn from the rank's category set.
            assert skill in valid
            assert (
                _count_rows(db, "codex_claims", f"WHERE species_name = '{species}'")
                == claims_before + 1
            )
        else:
            progress_before = _count_rows(db, "codex_progress")
            with pytest.raises(ValueError):
                svc.claim_rank(species, rank, skill)
            # A rejected claim is inert: no claim row, no progress mutation.
            assert (
                _count_rows(db, "codex_claims", f"WHERE species_name = '{species}'")
                == claims_before
            )
            assert _count_rows(db, "codex_progress") == progress_before


# calibrate: writes only codex_progress, sets current_rank to the supplied
# rank, and touches no claim / calibration / ledger table.


@given(
    species=st.sampled_from([_MOB_LOOTER, _REGULAR_MOB]),
    rank=st.integers(min_value=0, max_value=25),
)
def test_calibrate_has_no_side_effects_beyond_progress(species, rank):
    with _fresh_service() as (svc, db):
        result = svc.calibrate(species, rank)
        assert result == {"speciesName": species, "rank": rank}

        row = db.conn.execute(
            "SELECT current_rank FROM codex_progress WHERE species_name = ?",
            (species,),
        ).fetchone()
        assert row is not None
        assert row["current_rank"] == rank

        # Calibrate is a pure rank set: no claim, calibration, or ledger writes.
        assert _count_rows(db, "codex_claims") == 0
        assert _count_rows(db, "skill_calibrations") == 0
        assert _count_rows(db, "ledger_entries") == 0


# meta_claim: a valid attribute yields a constant 1 PES reward and exactly one
# meta codex_claims row of the documented shape, with no skill calibration; an
# invalid attribute raises and writes nothing.

_ATTRIBUTES = sorted(CodexService.ATTRIBUTES)


@given(attribute=st.sampled_from(_ATTRIBUTES))
def test_meta_claim_constant_reward_into_valid_attribute(attribute):
    with _fresh_service() as (svc, db):
        result = svc.meta_claim(attribute)
        assert result == {
            "attributeName": attribute,
            "pedValue": CodexService.META_PED,
        }
        assert result["pedValue"] == 1.0

        rows = db.conn.execute(
            "SELECT kind, attribute_name, species_name, rank, skill_name, ped_value "
            "FROM codex_claims WHERE kind = 'meta'"
        ).fetchall()
        assert len(rows) == 1
        row = rows[0]
        assert row["attribute_name"] == attribute
        assert row["skill_name"] == attribute
        assert row["species_name"] == "__meta__"
        assert row["rank"] == 0
        assert row["ped_value"] == 1.0

        # Meta rewards never touch the skill-calibration curve.
        assert _count_rows(db, "skill_calibrations") == 0


@given(
    name=st.text(),
)
def test_meta_claim_rejects_non_attribute_without_writing(name):
    assume(name not in CodexService.ATTRIBUTES)
    with _fresh_service() as (svc, db):
        with pytest.raises(ValueError):
            svc.meta_claim(name)
        assert _count_rows(db, "codex_claims") == 0


# get_skill_options: the recommendRank values form a dense 1..N over exactly
# the relevant skills (professionWeight > 0 for profession; hpGain > 0 for hp),
# and the returned order is non-increasing in the active objective.


@given(
    species=st.sampled_from([_MOB_LOOTER, _REGULAR_MOB]),
    rank=st.integers(min_value=1, max_value=25),
    target=st.sampled_from(["profession", "hp"]),
    with_profession=st.booleans(),
)
def test_recommend_rank_dense_over_relevant_only(
    species, rank, target, with_profession
):
    with _fresh_service() as (svc, _db):
        profession = _PROFESSION if with_profession else None
        options = svc.get_skill_options(
            species, rank, profession=profession, target=target
        )

        def is_relevant(opt):
            if target == "hp":
                return opt["hpGain"] > 0
            return opt["professionWeight"] > 0

        relevant = [o for o in options if is_relevant(o)]
        non_relevant = [o for o in options if not is_relevant(o)]

        # Exactly the relevant skills carry a recommendRank; the rest are None.
        assert all(o["recommendRank"] is None for o in non_relevant)
        ranks = [o["recommendRank"] for o in relevant]
        # Dense 1..N over the relevant set, in the order they appear.
        assert ranks == list(range(1, len(relevant) + 1))

        # The full list is non-increasing in the active objective, so the
        # relevant subsequence is too.
        objective = (
            (lambda o: o["hpGain"])
            if target == "hp"
            else (lambda o: o["profContribution"])
        )
        values = [objective(o) for o in options]
        assert all(values[i] + 1e-9 >= values[i + 1] for i in range(len(values) - 1))
