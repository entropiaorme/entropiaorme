"""Mutation-hardening tests for ``backend.services.session_summary``.

Targeted, example-based kills for the surviving mutants in cluster
``session_summary__c1`` (the ``compute_session_summary`` reducer plus the
``load_prospect_sessions`` / ``_row_to_prospect_dict`` read path).

Each test seeds an in-memory database with a deterministic, fully-specified
session and asserts the exact numeric / structural value the mutated line
would change. The seeds avoid the property-suite's generated ranges so a
single concrete example pins each arithmetic and branch decision.
"""

from __future__ import annotations

import sqlite3
import uuid

import pytest

from backend.services.character_calc import ATTRIBUTE_SKILLS
from backend.services.session_summary import (
    compute_session_summary,
    load_prospect_sessions,
    write_session_summary,
)
from backend.tracking.schema import init_tracking_tables

_ATTRIBUTES = sorted(ATTRIBUTE_SKILLS)


def _fresh_db() -> sqlite3.Connection:
    conn = sqlite3.connect(":memory:")
    init_tracking_tables(conn)
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS skill_gains (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id    TEXT NOT NULL,
            timestamp     REAL NOT NULL,
            skill_name    TEXT NOT NULL,
            amount        REAL NOT NULL,
            ped_value     REAL
        );
        """
    )
    conn.commit()
    return conn


def _insert_session(
    conn: sqlite3.Connection,
    session_id: str,
    started_at: float,
    ended_at: float,
    *,
    armour_cost: float = 0.0,
    heal_cost: float = 0.0,
    dangling_cost: float = 0.0,
) -> None:
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, 0, ?, ?, ?)",
        (session_id, started_at, ended_at, armour_cost, heal_cost, dangling_cost),
    )


def _insert_kill(
    conn: sqlite3.Connection,
    kill_id: str,
    session_id: str,
    *,
    mob_name: str | None,
    species: str = "",
    maturity: str = "",
    timestamp: float = 0.0,
    enhancer_cost: float = 0.0,
    loot_total_ped: float = 0.0,
) -> None:
    conn.execute(
        "INSERT INTO kills "
        "(id, session_id, mob_name, mob_species, mob_maturity, timestamp, "
        "enhancer_cost, loot_total_ped) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            kill_id,
            session_id,
            mob_name,
            species,
            maturity,
            timestamp,
            enhancer_cost,
            loot_total_ped,
        ),
    )


def _insert_tool_stat(
    conn: sqlite3.Connection,
    kill_id: str,
    tool_name: str | None,
    *,
    shots_fired: int,
    cost_per_shot: float,
) -> None:
    conn.execute(
        "INSERT INTO kill_tool_stats "
        "(kill_id, tool_name, shots_fired, cost_per_shot) VALUES (?, ?, ?, ?)",
        (kill_id, tool_name, shots_fired, cost_per_shot),
    )


def _insert_skill_gain(
    conn: sqlite3.Connection,
    session_id: str,
    skill_name: str,
    amount: float,
    ped_value: float | None,
) -> None:
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, ?, ?)",
        (session_id, skill_name, amount, ped_value),
    )


def _seed_basic(
    conn: sqlite3.Connection,
    sid: str,
    *,
    started_at: float = 100.0,
    ended_at: float = 3700.0,
    armour_cost: float = 0.0,
    heal_cost: float = 0.0,
    dangling_cost: float = 0.0,
) -> str:
    """One qualifying session: one kill, one tool shot, one positive skill gain."""
    _insert_session(
        conn,
        sid,
        started_at=started_at,
        ended_at=ended_at,
        armour_cost=armour_cost,
        heal_cost=heal_cost,
        dangling_cost=dangling_cost,
    )
    kill_id = uuid.uuid4().hex
    _insert_kill(
        conn, kill_id, sid, mob_name="Atrox", loot_total_ped=12.5, enhancer_cost=2.0
    )
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=3.0)
    conn.commit()
    return kill_id


# ---------------------------------------------------------------------------
# Exact-value smoke kill: pins the whole arithmetic surface of one session so
# any single off-by-operator/constant mutation in the final reduce changes a
# value asserted below.
# ---------------------------------------------------------------------------


def test_exact_summary_values_for_fully_specified_session():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    # started=100, ended=3700 -> duration 1.0h.
    # weapon = 10 * 0.5 = 5.0, enhancer = 2.0, armour=1.0, heal=0.25, dangling=0.75
    # cycled = 5 + 2 + 1 + 0.25 + 0.75 = 9.0
    _insert_session(
        conn, sid, started_at=100.0, ended_at=3700.0,
        armour_cost=1.0, heal_cost=0.25, dangling_cost=0.75,
    )
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", loot_total_ped=12.5, enhancer_cost=2.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=3.0)
    _insert_skill_gain(conn, sid, "Wounding", amount=0.02, ped_value=4.0)
    _insert_skill_gain(conn, sid, "Health", amount=2.5, ped_value=None)
    conn.commit()

    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["id"] == sid
    assert s["startedAt"] == 100.0
    assert s["endedAt"] == 3700.0
    assert s["durationHours"] == pytest.approx(1.0)
    assert s["armourCost"] == pytest.approx(1.0)
    assert s["healCost"] == pytest.approx(0.25)
    assert s["danglingCost"] == pytest.approx(0.75)
    assert s["weaponCost"] == pytest.approx(5.0)
    assert s["enhancerCost"] == pytest.approx(2.0)
    assert s["kills"] == 1
    assert s["lootTt"] == pytest.approx(12.5)
    assert s["regularSkillPed"] == {"Anatomy": 3.0, "Wounding": 4.0}
    assert s["attributeLevels"] == {"Health": 2.5}
    assert s["regularSkillTt"] == pytest.approx(7.0)
    assert s["attributeLevelsTotal"] == pytest.approx(2.5)
    assert s["cycledPed"] == pytest.approx(9.0)


# ---------------------------------------------------------------------------
# cycled_ped must SUM all five cost components (weapon + enhancer + armour +
# heal + dangling). A mutation dropping or altering one component changes the
# total. Each component is isolated so each summand is independently pinned.
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "armour,heal,dangling,enhancer,cost_per_shot,shots,expected",
    [
        (0.0, 0.0, 0.0, 0.0, 0.5, 10, 5.0),   # weapon only
        (0.0, 0.0, 0.0, 3.0, 0.0, 10, 3.0),   # enhancer only
        (7.0, 0.0, 0.0, 0.0, 0.0, 10, 7.0),   # armour only
        (0.0, 9.0, 0.0, 0.0, 0.0, 10, 9.0),   # heal only
        (0.0, 0.0, 4.0, 0.0, 0.0, 10, 4.0),   # dangling only
        (1.0, 2.0, 3.0, 4.0, 0.5, 10, 15.0),  # all five
    ],
)
def test_cycled_ped_is_sum_of_all_cost_components(
    armour, heal, dangling, enhancer, cost_per_shot, shots, expected
):
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(
        conn, sid, started_at=0.0, ended_at=3600.0,
        armour_cost=armour, heal_cost=heal, dangling_cost=dangling,
    )
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=enhancer)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=shots, cost_per_shot=cost_per_shot)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()

    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["cycledPed"] == pytest.approx(expected)


# ---------------------------------------------------------------------------
# Qualifying filters: cycled_ped <= 0  OR  duration_hours <= 0  -> None.
# And: regular_skill_tt <= 0 AND attribute_total <= 0 -> None.
# ---------------------------------------------------------------------------


def test_zero_cycled_ped_returns_none():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    # All cost components zero -> cycled_ped == 0 -> None.
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=0.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    assert compute_session_summary(conn, sid) is None


def test_zero_duration_returns_none():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    # started == ended -> duration 0 -> None despite positive cycled_ped.
    _insert_session(conn, sid, started_at=500.0, ended_at=500.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=5.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    assert compute_session_summary(conn, sid) is None


def test_positive_cycled_and_duration_qualifies():
    """The boundary the filter rejects must NOT reject a genuinely positive run."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_basic(conn, sid)
    assert compute_session_summary(conn, sid) is not None


def test_no_positive_skill_or_attribute_returns_none():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=5.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    # A skill gain exists (so has_gains passes) but ped_value <= 0 and not an
    # attribute -> regular_skill_tt == 0 and attribute_total == 0 -> None.
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=0.0)
    conn.commit()
    assert compute_session_summary(conn, sid) is None


def test_attribute_only_session_qualifies():
    """attribute_total > 0 alone is enough to qualify even with no regular ped."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=5.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=0.0)
    _insert_skill_gain(conn, sid, "Strength", amount=1.5, ped_value=None)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["regularSkillTt"] == pytest.approx(0.0)
    assert s["attributeLevelsTotal"] == pytest.approx(1.5)


def test_tiny_attribute_total_below_one_still_qualifies():
    """A session with no regular ped and 0 < attribute_total <= 1 must qualify.

    The qualifying filter is ``regular_skill_tt <= 0 and attribute_total <= 0``;
    the comparison must be ``<= 0`` exactly. A mutation widening the bound (e.g.
    ``<= 1``) would wrongly discard a session whose only progress is a small
    attribute gain.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name="Atrox", enhancer_cost=5.0)
    _insert_tool_stat(conn, kill_id, "Opalo", shots_fired=10, cost_per_shot=0.5)
    # No regular ped (ped_value 0) so regular_skill_tt == 0; a single small
    # attribute gain of 0.25 gives attribute_total == 0.25, which is in (0, 1].
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=0.0)
    _insert_skill_gain(conn, sid, "Strength", amount=0.25, ped_value=None)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None, "session with a small positive attribute gain must qualify"
    assert s["regularSkillTt"] == pytest.approx(0.0)
    assert s["attributeLevelsTotal"] == pytest.approx(0.25)


# ---------------------------------------------------------------------------
# Dominant-mob / dominant-tag dominance threshold (>= 0.6) and the
# species/maturity split between dominant_mob and dominant_tag.
# ---------------------------------------------------------------------------


def test_dominant_mob_set_when_species_present_and_over_threshold():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    # 7 Atrox (species present) + 3 Daikiba -> 0.7 >= 0.6 -> dominant.
    for i in range(7):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Atrox", species="Atrox", maturity="Young", timestamp=float(i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=1.0)
    for i in range(3):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Daikiba", species="Daikiba", maturity="Old", timestamp=float(100 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["dominantMob"] == "Atrox"
    assert s["dominantTag"] is None


def test_dominant_tag_set_when_no_species_and_over_threshold():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    # 7 of a blank-species/maturity mob -> dominant_tag branch.
    for i in range(7):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Mystery", species="", maturity="", timestamp=float(i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=1.0)
    for i in range(3):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Other", species="", maturity="", timestamp=float(100 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["dominantMob"] is None
    assert s["dominantTag"] == "Mystery"


def test_no_dominance_below_threshold():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    # 5 vs 5 -> top fraction 0.5 < 0.6 -> neither set.
    for i in range(5):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Atrox", species="Atrox", maturity="Young", timestamp=float(i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=1.0)
    for i in range(5):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Daikiba", species="Daikiba", maturity="Old", timestamp=float(100 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["dominantMob"] is None
    assert s["dominantTag"] is None


def test_dominance_exactly_at_threshold_is_inclusive():
    """0.6 exactly must count (>=, not >)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    # 6 Atrox + 4 Daikiba -> exactly 0.6.
    for i in range(6):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Atrox", species="Atrox", maturity="Young", timestamp=float(i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=1.0)
    for i in range(4):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Daikiba", species="Daikiba", maturity="Old", timestamp=float(100 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["dominantMob"] == "Atrox"


def test_unknown_and_null_mobs_excluded_from_dominance():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    # 3 real Atrox, 7 'Unknown' + NULL -> Unknown/NULL are filtered, so the
    # known set is 100% Atrox -> dominant_mob Atrox.
    for i in range(3):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Atrox", species="Atrox", maturity="Young", timestamp=float(i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=1.0)
    for i in range(4):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name="Unknown", timestamp=float(100 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    for i in range(3):
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name=None, timestamp=float(200 + i))
        _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["dominantMob"] == "Atrox"


# ---------------------------------------------------------------------------
# Dominant weapon dominance threshold over summed shots, and Unknown/NULL
# tool filtering.
# ---------------------------------------------------------------------------


def test_dominant_weapon_set_over_threshold():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kid1 = uuid.uuid4().hex
    _insert_kill(conn, kid1, sid, mob_name="Atrox", timestamp=0.0)
    _insert_tool_stat(conn, kid1, "Opalo", shots_fired=70, cost_per_shot=0.1)
    kid2 = uuid.uuid4().hex
    _insert_kill(conn, kid2, sid, mob_name="Atrox", timestamp=1.0)
    _insert_tool_stat(conn, kid2, "Breer", shots_fired=30, cost_per_shot=0.1)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    # 70/100 = 0.7 >= 0.6 -> Opalo dominant.
    assert s["dominantWeapon"] == "Opalo"


def test_dominant_weapon_none_below_threshold():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kid1 = uuid.uuid4().hex
    _insert_kill(conn, kid1, sid, mob_name="Atrox", timestamp=0.0)
    _insert_tool_stat(conn, kid1, "Opalo", shots_fired=50, cost_per_shot=0.1)
    kid2 = uuid.uuid4().hex
    _insert_kill(conn, kid2, sid, mob_name="Atrox", timestamp=1.0)
    _insert_tool_stat(conn, kid2, "Breer", shots_fired=50, cost_per_shot=0.1)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=2.0)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    # 50/100 = 0.5 < 0.6 -> no dominant weapon.
    assert s["dominantWeapon"] is None


# ---------------------------------------------------------------------------
# Rounding to 4 decimals for the three rounded fields.
# ---------------------------------------------------------------------------


def test_rounded_fields_round_to_four_decimals():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name="Atrox", enhancer_cost=0.123456789)
    _insert_tool_stat(conn, kid, "Opalo", shots_fired=1, cost_per_shot=0.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=1.123456789)
    _insert_skill_gain(conn, sid, "Strength", amount=2.987654321, ped_value=None)
    conn.commit()
    s = compute_session_summary(conn, sid)
    assert s is not None
    assert s["regularSkillTt"] == pytest.approx(round(1.123456789, 4))
    assert s["regularSkillTt"] == 1.1235
    assert s["attributeLevelsTotal"] == 2.9877
    assert s["cycledPed"] == round(0.123456789, 4)
    # The breakdown dict retains the unrounded per-skill value.
    assert s["regularSkillPed"]["Anatomy"] == pytest.approx(1.123456789)


# ---------------------------------------------------------------------------
# load_prospect_sessions / _row_to_prospect_dict round-trip (mutants 310-313
# live in the read path). Persist via write_session_summary then read back.
# ---------------------------------------------------------------------------


def test_load_prospect_sessions_round_trips_all_fields():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(
        conn, sid, started_at=100.0, ended_at=3700.0,
        armour_cost=1.0, heal_cost=0.25, dangling_cost=0.75,
    )
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name="Atrox", species="Atrox", maturity="Young",
                 loot_total_ped=12.5, enhancer_cost=2.0)
    _insert_tool_stat(conn, kid, "Opalo", shots_fired=10, cost_per_shot=0.5)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=3.0)
    _insert_skill_gain(conn, sid, "Health", amount=2.5, ped_value=None)
    conn.commit()

    rows = load_prospect_sessions(conn)
    assert len(rows) == 1
    r = rows[0]
    assert r["id"] == sid
    assert r["startedAt"] == 100.0
    assert r["endedAt"] == 3700.0
    assert r["durationHours"] == pytest.approx(1.0)
    assert r["kills"] == 1
    assert r["lootTt"] == pytest.approx(12.5)
    assert r["weaponCost"] == pytest.approx(5.0)
    assert r["enhancerCost"] == pytest.approx(2.0)
    assert r["armourCost"] == pytest.approx(1.0)
    assert r["healCost"] == pytest.approx(0.25)
    assert r["danglingCost"] == pytest.approx(0.75)
    assert r["cycledPed"] == pytest.approx(9.0)
    assert r["regularSkillPed"] == {"Anatomy": 3.0}
    assert r["attributeLevels"] == {"Health": 2.5}
    assert r["regularSkillTt"] == pytest.approx(3.0)
    assert r["attributeLevelsTotal"] == pytest.approx(2.5)
    assert r["dominantMob"] == "Atrox"
    assert r["dominantTag"] is None
    # dominantWeapon: single tool -> 100% -> dominant.
    assert r["dominantWeapon"] == "Opalo"


def test_load_prospect_sessions_lazily_rebuilds_missing_rows():
    """No write_session_summary call: load must materialise the missing row."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_basic(conn, sid)
    # Nothing in session_summaries yet.
    pre = conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0]
    assert pre == 0
    rows = load_prospect_sessions(conn)
    assert [r["id"] for r in rows] == [sid]
    post = conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0]
    assert post == 1


def test_write_then_load_no_duplicate_materialisation():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_basic(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()
    rows = load_prospect_sessions(conn)
    assert len(rows) == 1
    assert rows[0]["id"] == sid
