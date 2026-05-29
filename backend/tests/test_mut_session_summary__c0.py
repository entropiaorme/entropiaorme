"""Mutation-kill tests for ``backend.services.session_summary.compute_session_summary``.

Dedicated, example-based tests that pin the exact arithmetic, SQL filters,
aggregation, dominance thresholds, and qualifying logic of
``compute_session_summary``. Each test seeds an in-memory database with a
hand-built session and asserts a precise scalar/structural value, so a single
operator/constant/boundary mutation flips the asserted value and is killed.

The summary is a cache of derived state whose source of truth is the tracking
tables, so we seed the tables directly (mirroring
``test_session_summary_properties``) rather than driving the live tracker.
"""

from __future__ import annotations

import sqlite3
import uuid

import pytest

from backend.services.character_calc import ATTRIBUTE_SKILLS
from backend.services.session_summary import (
    compute_session_summary,
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
    ended_at: float | None,
    *,
    is_active: int = 0,
    armour_cost: float = 0.0,
    heal_cost: float = 0.0,
    dangling_cost: float = 0.0,
) -> None:
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        (
            session_id,
            started_at,
            ended_at,
            is_active,
            armour_cost,
            heal_cost,
            dangling_cost,
        ),
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
    shots_fired: float,
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


_REGULAR_SKILL = "Laser Weaponry Technology"
_ATTR = _ATTRIBUTES[0]
_TOOL = "Sollomate Opalo"
_MOB = "Atrox"


def _add_qualifying_skill(conn: sqlite3.Connection, sid: str, ped: float = 5.0) -> None:
    """One positive regular-skill gain so the session passes the gains filter."""
    _insert_skill_gain(conn, sid, _REGULAR_SKILL, amount=0.01, ped_value=ped)


# --- kills count: int(kill_totals[0] or 0/1) (m41) ---


def test_kills_zero_when_no_kills():
    """A qualifying session with no kill rows reports kills == 0.

    Kills the ``or 1`` mutant (m41): COUNT(*) is 0, and the fallback would
    surface 1. cycled PED comes from armour_cost so the session still
    qualifies without any kills.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["kills"] == 0


def test_kills_count_matches_rows():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    for _ in range(3):
        _insert_kill(conn, uuid.uuid4().hex, sid, mob_name=_MOB)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["kills"] == 3


# --- enhancer_cost: float(kill_totals[2] or/and ...) (m49, m51) ---


def test_enhancer_cost_preserved_when_positive():
    """enhancer_cost surfaces the real SUM (kills the ``and 0.0`` mutant m49)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0)
    _insert_kill(conn, uuid.uuid4().hex, sid, mob_name=_MOB, enhancer_cost=7.5)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["enhancerCost"] == pytest.approx(7.5)


def test_enhancer_cost_zero_when_none():
    """No enhancer cost yields 0.0 (kills the ``or 1.0`` mutant m51).

    cycled PED is supplied by weapon cost, so the session qualifies even
    though enhancer cost is zero.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0)
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name=_MOB, enhancer_cost=0.0)
    _insert_tool_stat(conn, kid, _TOOL, shots_fired=1, cost_per_shot=2.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["enhancerCost"] == 0.0


# --- weapon_cost: float(weapon_row[0] or 1.0) (m73) ---


def test_weapon_cost_zero_when_no_tool_stats():
    """No tool-stat rows yields weaponCost == 0.0 (kills ``or 1.0`` mutant m73).

    Qualification is carried by armour_cost.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=4.0)
    _insert_kill(conn, uuid.uuid4().hex, sid, mob_name=_MOB)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["weaponCost"] == 0.0


def test_weapon_cost_is_cost_per_shot_times_shots():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0)
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name=_MOB)
    _insert_tool_stat(conn, kid, _TOOL, shots_fired=4, cost_per_shot=0.5)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["weaponCost"] == pytest.approx(2.0)


# --- dominant mob / tag computation ---
#
# Helpers build sessions whose top mob group crosses the dominance threshold,
# letting tests pin the mob-vs-tag branch and the exact dominant name.


def _seed_dominant_mob_session(
    conn: sqlite3.Connection,
    sid: str,
    *,
    top_count: int,
    other_count: int,
    species: str,
    maturity: str,
) -> None:
    """Top mob ``_MOB`` appears top_count times; a different mob other_count.

    The top mob carries the given species/maturity. cycled PED via armour.
    """
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=5.0)
    ts = 0.0
    for _ in range(top_count):
        _insert_kill(
            conn,
            uuid.uuid4().hex,
            sid,
            mob_name=_MOB,
            species=species,
            maturity=maturity,
            timestamp=ts,
        )
        ts += 1.0
    for _ in range(other_count):
        _insert_kill(
            conn,
            uuid.uuid4().hex,
            sid,
            mob_name="Daikiba",
            species="Daikiba",
            maturity="Old",
            timestamp=ts,
        )
        ts += 1.0
    _add_qualifying_skill(conn, sid)
    conn.commit()


def test_dominant_mob_set_for_species_tagged_top_mob():
    """A clearly dominant species-tagged mob populates dominantMob.

    Kills m74 (mob_rows=None), m99/m108 (count-zeroing), m112 (dominant_mob
    forced None) and m111 (or->and on species/maturity, since only species
    is set here).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=4, other_count=1, species="Atrox", maturity=""
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantMob"] == _MOB
    assert summary["dominantTag"] is None


def test_dominant_tag_set_for_untagged_top_mob():
    """A dominant mob with no species/maturity populates dominantTag.

    Kills m113 (dominant_tag forced None) and reinforces m74/m99/m108.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=4, other_count=1, species="", maturity=""
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantTag"] == _MOB
    assert summary["dominantMob"] is None


def test_dominant_mob_when_only_maturity_present():
    """Only maturity set (no species) still routes to dominantMob.

    Kills m111: orig ``species or maturity`` is truthy, the ``and`` mutant is
    falsy and would mis-route to dominantTag.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=4, other_count=1, species="", maturity="Young"
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantMob"] == _MOB
    assert summary["dominantTag"] is None


def test_single_kill_is_dominant():
    """A lone known kill is dominant (ratio 1.0).

    Kills m103 (total_known > 1) and m148-style boundary on the mob side: with
    exactly one kill, total_known == 1 and the dominant branch must still run.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=1, other_count=0, species="", maturity=""
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantTag"] == _MOB


def test_no_dominant_below_threshold_mob():
    """A 1-of-2 split (ratio 0.5) yields no dominant mob.

    Kills m106 (``/`` -> ``*``: 1*2 >= 0.6 would wrongly mark dominant) and
    m146-analogue is on the weapon side. Here orig 1/2 = 0.5 < 0.6.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=1, other_count=1, species="", maturity=""
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantTag"] is None
    assert summary["dominantMob"] is None


def test_dominant_mob_at_exact_threshold():
    """A 3-of-5 split is exactly 0.6 and counts as dominant.

    Kills m110: ``>=`` -> ``>`` would drop the exact-threshold case.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_dominant_mob_session(
        conn, sid, top_count=3, other_count=2, species="", maturity=""
    )
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantTag"] == _MOB


def test_unknown_mob_excluded_from_dominance():
    """Kills named 'Unknown' are excluded by the SQL filter.

    Kills m86/m87 ('Unknown' literal recase): with the literal lowered/uppered
    the 'Unknown' rows would slip through and dilute the 4-of-4 Atrox dominance
    (4 of 8 = 0.5 < 0.6), flipping dominantTag from Atrox to None.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=5.0)
    ts = 0.0
    for _ in range(4):
        _insert_kill(conn, uuid.uuid4().hex, sid, mob_name=_MOB, timestamp=ts)
        ts += 1.0
    for _ in range(4):
        _insert_kill(conn, uuid.uuid4().hex, sid, mob_name="Unknown", timestamp=ts)
        ts += 1.0
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    # Only the 4 Atrox kills are "known", so Atrox is 4/4 = fully dominant.
    assert summary["dominantTag"] == _MOB


# --- dominant weapon computation ---


def _seed_tool_session(
    conn: sqlite3.Connection,
    sid: str,
    tools: list[tuple[str, int]],
) -> None:
    """One kill per (tool_name, shots) pair; cycled PED via armour_cost."""
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=5.0)
    for tool_name, shots in tools:
        kid = uuid.uuid4().hex
        _insert_kill(conn, kid, sid, mob_name=_MOB)
        _insert_tool_stat(conn, kid, tool_name, shots_fired=shots, cost_per_shot=0.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()


def test_dominant_weapon_set_when_dominant():
    """A weapon firing 80% of shots is the dominant weapon.

    Kills m114 (tool_rows=None), m141/m151 (shot-zeroing), m154 (forced None).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 8), ("CB5 Regular", 2)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] == _TOOL


def test_dominant_weapon_none_below_threshold():
    """A 50/50 shot split yields no dominant weapon.

    Kills m146 (and->or: total_shots>0 alone would mark dominant) and m149
    (``/`` -> ``*``: 5*10 >= 0.6 would mark dominant). orig 5/10 = 0.5 < 0.6.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 5), ("CB5 Regular", 5)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] is None


def test_dominant_weapon_at_exact_threshold():
    """A 3-of-5 shot split is exactly 0.6 and is dominant.

    Kills m153: ``>=`` -> ``>`` would drop the exact-threshold weapon.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 3), ("CB5 Regular", 2)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] == _TOOL


def test_single_shot_tool_is_dominant():
    """A lone tool firing one shot is dominant (ratio 1.0).

    Kills m148: total_shots ``> 0`` -> ``> 1`` would drop the single-shot case.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 1)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] == _TOOL


def test_dominant_weapon_none_with_zero_total_shots():
    """A tool row with zero shots leaves total_shots == 0 and no dominant.

    Kills m147: ``total_shots > 0`` -> ``>= 0`` would let the >= branch divide
    by zero (raising), whereas orig short-circuits and returns a summary with
    dominantWeapon None.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 0)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] is None


def test_dominant_weapon_default_is_none_with_no_tool_rows():
    """No qualifying tool rows leaves dominantWeapon as None, not "".

    Kills m137: the default initialiser ``None`` -> ``""`` would surface an
    empty string instead of None. cycled PED via armour_cost, no tool stats.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=5.0)
    _insert_kill(conn, uuid.uuid4().hex, sid, mob_name=_MOB)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] is None


def test_unknown_tool_excluded_from_dominance():
    """Tool rows named 'Unknown' are excluded by the SQL filter.

    Kills m129/m130 ('Unknown' literal recase): the 'Unknown' tool would
    otherwise count toward total shots, diluting the real tool's 4-of-4
    dominance to 4/8 = 0.5 and flipping dominantWeapon to None.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=5.0)
    k1 = uuid.uuid4().hex
    _insert_kill(conn, k1, sid, mob_name=_MOB)
    _insert_tool_stat(conn, k1, _TOOL, shots_fired=4, cost_per_shot=0.0)
    k2 = uuid.uuid4().hex
    _insert_kill(conn, k2, sid, mob_name=_MOB)
    _insert_tool_stat(conn, k2, "Unknown", shots_fired=4, cost_per_shot=0.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] == _TOOL


# --- regular-skill breakdown filter: float(total or 0.0) > 0 (m178, m179) ---


def test_zero_ped_regular_skill_excluded():
    """A regular skill whose summed ped_value is 0 is excluded from the breakdown.

    Kills m178 (``total or 1.0`` in the filter -> a 0 row passes) and m179
    (``> 0`` -> ``>= 0`` -> a 0 row passes). A second positive skill keeps the
    session qualifying.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _insert_skill_gain(conn, sid, _REGULAR_SKILL, amount=0.01, ped_value=5.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=0.0)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    breakdown = summary["regularSkillPed"]
    assert "Anatomy" not in breakdown
    assert breakdown == {_REGULAR_SKILL: pytest.approx(5.0)}


def test_regular_skill_tt_is_sum_of_positive_peds():
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _insert_skill_gain(conn, sid, _REGULAR_SKILL, amount=0.01, ped_value=2.0)
    _insert_skill_gain(conn, sid, "Anatomy", amount=0.01, ped_value=3.0)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["regularSkillTt"] == pytest.approx(5.0)


# --- attribute breakdown filter: float(total or 0.0) > 0 (m196, m197, m198, m199) ---


def test_positive_attribute_included():
    """A positive attribute gain populates attributeLevels.

    Kills m196: ``total and 0.0`` would zero every attribute and drop them all.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    _insert_skill_gain(conn, sid, _ATTR, amount=2.0, ped_value=None)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["attributeLevels"].get(_ATTR) == pytest.approx(2.0)
    assert summary["attributeLevelsTotal"] == pytest.approx(2.0)


def test_zero_amount_attribute_excluded():
    """An attribute whose summed amount is 0 is excluded.

    Kills m197 (``total or 1.0``) and m198 (``> 0`` -> ``>= 0``): both would
    let the 0-amount attribute into the breakdown.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    _insert_skill_gain(conn, sid, _ATTR, amount=2.0, ped_value=None)
    _insert_skill_gain(conn, sid, _ATTRIBUTES[1], amount=0.0, ped_value=None)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert _ATTRIBUTES[1] not in summary["attributeLevels"]
    assert set(summary["attributeLevels"]) == {_ATTR}


def test_small_positive_attribute_included():
    """A 0.5-amount attribute is included (it is > 0).

    Kills m199: ``> 0`` -> ``> 1`` would drop the 0.5 attribute.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    _insert_skill_gain(conn, sid, _ATTR, amount=0.5, ped_value=None)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["attributeLevels"].get(_ATTR) == pytest.approx(0.5)


# --- duration: max((ended - started) / 3600.0, 0.0) (m205, m206, m209, m210) ---


def test_duration_hours_is_delta_over_3600():
    """One hour elapsed yields durationHours == 1.0.

    Kills m205 (``/`` -> ``*``: 3600*3600), m209 (``/3600`` -> ``/3601``).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["durationHours"] == 1.0


def test_duration_uses_difference_not_sum():
    """A 1800s window starting at 1800 is 0.5h.

    Kills m206 (``-`` -> ``+``: (3600+1800)/3600 = 1.5) and m210 (the 0.0 clamp
    floor raised to 1.0 would force 1.0).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 1800.0, 3600.0, armour_cost=3.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["durationHours"] == pytest.approx(0.5)


# --- per-cost coalesce: float(cost or 0.0) (m213/m214 armour, m217/m218 heal,
#     m221/m222 dangling) ---


def test_armour_cost_preserved_when_positive():
    """armourCost surfaces the stored value (kills ``and 0.0`` mutant m213)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=6.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["armourCost"] == pytest.approx(6.0)


def test_armour_cost_zero_stays_zero():
    """Zero armour cost stays 0.0 (kills ``or 1.0`` mutant m214).

    Qualification is carried by weapon cost.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=0.0)
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name=_MOB)
    _insert_tool_stat(conn, kid, _TOOL, shots_fired=1, cost_per_shot=2.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["armourCost"] == 0.0


def test_heal_cost_preserved_when_positive():
    """healCost surfaces the stored value (kills ``and 0.0`` mutant m217)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, heal_cost=8.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["healCost"] == pytest.approx(8.0)


def test_heal_cost_zero_stays_zero():
    """Zero heal cost stays 0.0 (kills ``or 1.0`` mutant m218)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=2.0, heal_cost=0.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["healCost"] == 0.0


def test_dangling_cost_preserved_when_positive():
    """danglingCost surfaces the stored value (kills ``and 0.0`` mutant m221)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, dangling_cost=9.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["danglingCost"] == pytest.approx(9.0)


def test_dangling_cost_zero_stays_zero():
    """Zero dangling cost stays 0.0 (kills ``or 1.0`` mutant m222)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, 0.0, 3600.0, armour_cost=2.0, dangling_cost=0.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["danglingCost"] == 0.0


# --- cycled_ped is the sum of all five components (m224, m225, m226, m227) ---


def test_cycled_ped_is_sum_of_all_components():
    """cycledPed sums weapon + enhancer + armour + heal + dangling.

    Distinct positive values are chosen so flipping any single ``+`` to ``-``
    changes the total: kills m227 (weapon-enhancer), m226 (enhancer-armour),
    m225 (armour-heal), m224 (heal-dangling).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    # weapon = 1*10 = 10, enhancer = 20, armour = 4, heal = 8, dangling = 16
    _insert_session(
        conn, sid, 0.0, 3600.0, armour_cost=4.0, heal_cost=8.0, dangling_cost=16.0
    )
    kid = uuid.uuid4().hex
    _insert_kill(conn, kid, sid, mob_name=_MOB, enhancer_cost=20.0)
    _insert_tool_stat(conn, kid, _TOOL, shots_fired=10, cost_per_shot=1.0)
    _add_qualifying_skill(conn, sid)
    conn.commit()
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["weaponCost"] == pytest.approx(10.0)
    assert summary["enhancerCost"] == pytest.approx(20.0)
    assert summary["cycledPed"] == pytest.approx(10.0 + 20.0 + 4.0 + 8.0 + 16.0)


def test_zero_shot_tool_does_not_inflate_total_shots():
    """A 0-shot tool group must not add a phantom shot to the denominator.

    Top tool fires 3 of 5 real shots (exactly the 0.6 threshold), so orig
    marks it dominant. Kills m143 (``r[1] or 1``): counting the 0-shot group
    as 1 makes the denominator 6, dropping the ratio to 0.5 and the dominance.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_tool_session(conn, sid, [(_TOOL, 3), ("CB5 Regular", 0), ("Breer P1a", 2)])
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert summary["dominantWeapon"] == _TOOL
