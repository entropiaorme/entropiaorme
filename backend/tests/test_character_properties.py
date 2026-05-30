"""Property-based tests for the character router's analytic surfaces.

Covers ``backend.routers.character``:

* ``_prospect_sample`` derives a forecast sample from a list of completed
  session-summary rows. Its per-PED and per-hour rates are arithmetic
  identities over the summed columns, and the empty-input case must yield
  exact-zero rates with no division.
* ``_build_prospect_result`` is a no-op when the requested target level is at
  or below the current profession level: it forecasts no extra cycling and
  performs no skill projection.
* ``get_character_stats`` reports HP straight from the scanned Health level and
  returns the top professions, capped, positive, and sorted descending.

The first two are pure functions exercised directly over generated session
dicts and profession entities. The stats endpoint reads calibrations from an
app database and the profession catalogue from game data, so it is driven over
a fresh in-memory calibration table and a small generated catalogue with
``get_services`` patched, which spans the same code path the live endpoint runs
without booting the app lifespan.
"""

from __future__ import annotations

import sqlite3
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import backend.routers.character as character
from backend.routers.character import (
    _build_prospect_result,
    _prospect_sample,
    get_character_stats,
)
from backend.services.character_calc import ATTRIBUTE_SKILLS, profession_level

_REGULAR_SKILLS = ["Laser Weaponry Technology", "Anatomy", "Dexterity", "Wounding"]
_ATTRIBUTES = sorted(ATTRIBUTE_SKILLS)

_NONNEG = st.floats(
    min_value=0.0, max_value=100000.0, allow_nan=False, allow_infinity=False
)
_POSITIVE = st.floats(
    min_value=0.0001, max_value=100000.0, allow_nan=False, allow_infinity=False
)
_COUNT = st.integers(min_value=0, max_value=50)


# A breakdown maps a skill/attribute name to a non-negative PED or level total.
_breakdown = st.dictionaries(
    keys=st.sampled_from(_REGULAR_SKILLS),
    values=_NONNEG,
    max_size=len(_REGULAR_SKILLS),
)
_attr_breakdown = st.dictionaries(
    keys=st.sampled_from(_ATTRIBUTES),
    values=_NONNEG,
    max_size=len(_ATTRIBUTES),
)


@st.composite
def _session_row(draw) -> dict:
    """One completed session summary in the shape ``_prospect_sample`` consumes.

    Mirrors the column set ``backend.services.session_summary`` materialises:
    each numeric column is a non-negative finite total, and the per-name
    breakdowns are the source of the regular-skill and attribute aggregates.
    """
    return {
        "kills": draw(_COUNT),
        "durationHours": draw(_NONNEG),
        "cycledPed": draw(_NONNEG),
        "lootTt": draw(_NONNEG),
        "regularSkillTt": draw(_NONNEG),
        "attributeLevelsTotal": draw(_NONNEG),
        "regularSkillPed": draw(_breakdown),
        "attributeLevels": draw(_attr_breakdown),
    }


_session_list = st.lists(_session_row(), min_size=0, max_size=8)


# --- _prospect_sample: per-PED and per-hour rates are arithmetic identities ---


@given(_session_list)
def test_prospect_sample_rates_are_exact_quotients(sessions):
    # Each rate is the rounded quotient of the same summed operands the sample
    # exposes, guarded by the positive-denominator precondition. Where the
    # denominator is non-positive the code takes the literal-0.0 else-branch,
    # which the identity does not constrain.
    sample = _prospect_sample(sessions)

    if sample["hours"] > 0:
        assert sample["cycledPerHour"] == pytest.approx(
            round(sample["cycledPed"] / sample["hours"], 4)
        )
        assert sample["lootPerHour"] == pytest.approx(
            round(sample["lootTt"] / sample["hours"], 4)
        )
    if sample["cycledPed"] > 0:
        assert sample["returnRate"] == pytest.approx(
            round(sample["lootTt"] / sample["cycledPed"], 4)
        )
        assert sample["pesPerPed"] == pytest.approx(
            round(sample["pes"] / sample["cycledPed"], 6)
        )
        assert sample["lootTtPerPed"] == pytest.approx(
            round(sample["lootTt"] / sample["cycledPed"], 6)
        )


@given(_session_list)
def test_prospect_sample_rates_are_zero_when_denominator_is_non_positive(sessions):
    # When a denominator is non-positive the rate falls to the literal 0.0
    # else-branch rather than dividing, so no ZeroDivisionError can arise.
    sample = _prospect_sample(sessions)

    if not sample["hours"] > 0:
        assert sample["cycledPerHour"] == 0.0
        assert sample["lootPerHour"] == 0.0
    if not sample["cycledPed"] > 0:
        assert sample["returnRate"] == 0.0
        assert sample["pesPerPed"] == 0.0
        assert sample["lootTtPerPed"] == 0.0


def test_prospect_sample_empty_input_yields_exact_zero_rates():
    # The empty sample is the canonical degenerate case: every sum is zero, so
    # all five rates take the 0.0 else-branch and no division executes.
    sample = _prospect_sample([])

    assert sample["sessions"] == 0
    assert sample["kills"] == 0
    assert sample["cycledPed"] == 0.0
    assert sample["hours"] == 0.0
    for rate in (
        "cycledPerHour",
        "lootPerHour",
        "returnRate",
        "pesPerPed",
        "lootTtPerPed",
    ):
        assert sample[rate] == 0.0
    assert sample["skillShares"] == {}
    assert sample["attributeRates"] == {}


# --- _build_prospect_result: a target at or below current is a no-op ---


def _profession(weights: dict[str, float]) -> dict:
    return {
        "name": "P",
        "skills": [{"skill": {"name": n}, "weight": w} for n, w in weights.items()],
    }


_skill_levels = st.dictionaries(
    keys=st.sampled_from(_REGULAR_SKILLS + _ATTRIBUTES),
    values=st.floats(
        min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False
    ),
    max_size=5,
)
_weights = st.dictionaries(
    keys=st.sampled_from(_REGULAR_SKILLS + _ATTRIBUTES),
    values=st.floats(
        min_value=0.0, max_value=10.0, allow_nan=False, allow_infinity=False
    ),
    max_size=5,
)


@given(
    skill_levels=_skill_levels,
    weights=_weights,
    sample_sessions=_session_list,
    target_drop=st.floats(
        min_value=0.0, max_value=50.0, allow_nan=False, allow_infinity=False
    ),
    markup_uplift=st.floats(
        min_value=0.0, max_value=2.0, allow_nan=False, allow_infinity=False
    ),
)
def test_prospect_below_target_forecasts_no_cycling_or_projection(
    skill_levels, weights, sample_sessions, target_drop, markup_uplift
):
    # Precondition: target_level <= current_level drives the no-op branch.
    profession = _profession(weights)
    current_level = profession_level(skill_levels, profession)
    target_level = current_level - target_drop

    sample = _prospect_sample(sample_sessions)
    result = _build_prospect_result(
        "P",
        profession,
        skill_levels,
        target_level,
        sample,
        "global",
        None,
        markup_uplift,
    )

    # The three forecast outputs are derived solely from a hard-coded
    # zero projected-cycled-PED, so no sample content can perturb them.
    assert result["projectedCycledPed"] == 0.0
    assert result["expectedLootTt"] == 0.0
    assert result["expectedNetTtBurn"] == 0.0

    # No projection is performed: every reported row shows a zero gain and an
    # end level equal to the current level (numeric equality, the qualified
    # reading of "projected_levels equals the input skill_levels").
    for row in result["rows"]:
        assert row["projectedGain"] == 0.0
        assert row["projectedEndLevel"] == pytest.approx(row["currentLevel"])


# --- get_character_stats: HP mirrors the Health level; top professions hold ---


class _FakeAppDb:
    """Minimal stand-in for the app database the stats endpoint reads.

    Exposes the ``lock`` (a real re-entrant lock used as a context manager)
    and ``conn`` (a row-factory connection) the calibration query relies on.
    """

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn
        self.lock = threading.RLock()


class _FakeGameData:
    def __init__(self, professions: list[dict]):
        self._professions = professions

    def get_entities(self, kind: str) -> list[dict]:
        return self._professions if kind == "professions" else []


class _FakeServices:
    def __init__(self, app_db: _FakeAppDb, game_data: _FakeGameData):
        self.app_db = app_db
        self.game_data = game_data


def _calibration_db(levels: dict[str, float]) -> _FakeAppDb:
    conn = sqlite3.connect(":memory:")
    conn.row_factory = sqlite3.Row
    conn.executescript(
        """
        CREATE TABLE skill_calibrations (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_name    TEXT NOT NULL,
            level         REAL NOT NULL,
            source        TEXT NOT NULL,
            scanned_at    REAL NOT NULL DEFAULT 0
        );
        """
    )
    for name, level in levels.items():
        conn.execute(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
            "VALUES (?, ?, 'scan', 0)",
            (name, level),
        )
    conn.commit()
    return _FakeAppDb(conn)


# Distinct profession names so each catalogue entry is independent.
_PROFESSION_NAMES = ["Laser Pistoleer", "Animal Tamer", "Healer", "Miner", "Crafter"]


@settings(max_examples=80)
@given(
    health_level=st.floats(
        min_value=0.0, max_value=2000.0, allow_nan=False, allow_infinity=False
    ),
    catalogue=st.lists(
        st.tuples(
            st.sampled_from(_PROFESSION_NAMES),
            _weights,
        ),
        max_size=6,
    ),
    skill_levels=_skill_levels,
)
def test_character_stats_hp_and_top_professions(health_level, catalogue, skill_levels):
    # health_level seeds the Health row; the endpoint reports HP as
    # int(Health). The catalogue is a set of professions whose levels derive
    # from the generated skill snapshot.
    levels = dict(skill_levels)
    levels["Health"] = health_level

    seen: set[str] = set()
    professions: list[dict] = []
    for name, weights in catalogue:
        if name in seen:
            continue
        seen.add(name)
        prof = _profession(weights)
        prof["name"] = name
        prof["category"] = "General"
        professions.append(prof)

    services = _FakeServices(_calibration_db(levels), _FakeGameData(professions))

    original = character.get_services
    character.get_services = lambda: services  # type: ignore[assignment,return-value]
    try:
        result = get_character_stats()
    finally:
        character.get_services = original

    assert result["hp"] == int(health_level)

    top = result["topProfessions"]
    assert len(top) <= 5
    for entry in top:
        assert entry["level"] > 0
    levels_seq = [entry["level"] for entry in top]
    assert all(a >= b for a, b in zip(levels_seq, levels_seq[1:], strict=False))
