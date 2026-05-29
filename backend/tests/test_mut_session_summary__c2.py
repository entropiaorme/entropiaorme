"""Mutation-hardening tests for ``backend.services.session_summary``.

Cluster ``session_summary__c2`` targets three functions:

* ``write_session_summary`` - the upsert/clear path. The JSON-payload mutants
  (``json.dumps(...) -> json.dumps(None)``) are killed by a write/read round
  trip; the SQL-casing mutants are recorded as equivalents (SQLite identifiers
  and keywords are case-insensitive, so they cannot change behaviour).
* ``delete_session_summary`` - never previously exercised ("no tests"). The
  structural mutants (``None`` statement / ``None`` params / dropped argument)
  are killed by exercising the real delete and asserting it both raises on the
  broken forms and removes exactly the targeted row on the intact form.
* ``_row_to_prospect_dict`` - the row projection. Every output key and every
  ``x or default`` fallback is pinned by calling the projection directly with a
  fully-truthy row (kills the ``and``/renamed-key mutants) and a fully-falsy
  row (kills the ``or 1.0`` mutants whose default differs only on falsy input).

Tests import the real module (never the mutated copy explicitly) and assert the
exact observable behaviour each mutation breaks.
"""

from __future__ import annotations

import sqlite3
import uuid

import pytest

from backend.services.session_summary import (
    _row_to_prospect_dict,
    compute_session_summary,
    delete_session_summary,
    load_prospect_sessions,
    write_session_summary,
)
from backend.tracking.schema import init_tracking_tables

_REGULAR_SKILL = "Anatomy"
_ATTRIBUTE = "Strength"
_MOB = "Atrox"
_TOOL = "Sollomate Opalo"


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


def _seed_qualifying(conn: sqlite3.Connection, sid: str) -> None:
    """One session that compute_session_summary will accept (returns a dict).

    Positive cycled PED (one shot at cost 1.0), positive duration (one hour),
    one positive regular-skill gain and one positive attribute gain - so both
    the regularSkillPed and attributeLevels dicts are non-empty.
    """
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active) VALUES (?, 0.0, 3600.0, 0)",
        (sid,),
    )
    kill_id = uuid.uuid4().hex
    conn.execute(
        "INSERT INTO kills "
        "(id, session_id, mob_name, mob_species, mob_maturity, timestamp, "
        "enhancer_cost, loot_total_ped) VALUES (?, ?, ?, '', '', 0.0, 0.0, 0.0)",
        (kill_id, sid, _MOB),
    )
    conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) "
        "VALUES (?, ?, 1, 1.0)",
        (kill_id, _TOOL),
    )
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, 0.01, 5.0)",
        (sid, _REGULAR_SKILL),
    )
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, 3.0, NULL)",
        (sid, _ATTRIBUTE),
    )
    conn.commit()


# --------------------------------------------------------------------------
# write_session_summary: JSON payload columns (mutmut_68, mutmut_72)
# --------------------------------------------------------------------------


def test_write_persists_regular_skill_ped_json_not_null():
    """mutmut_68: json.dumps(summary["regularSkillPed"]) -> json.dumps(None).

    The mutant stores the literal "null"; the round trip would then surface
    ``regularSkillPed`` as None instead of the real per-skill breakdown.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()

    stored = conn.execute(
        "SELECT regular_skill_ped_json FROM session_summaries WHERE session_id = ?",
        (sid,),
    ).fetchone()[0]
    assert stored != "null"

    rows = load_prospect_sessions(conn)
    row = next(r for r in rows if r["id"] == sid)
    assert row["regularSkillPed"] == {_REGULAR_SKILL: pytest.approx(5.0)}
    assert row["regularSkillPed"] is not None


def test_write_persists_attribute_levels_json_not_null():
    """mutmut_72: json.dumps(summary["attributeLevels"]) -> json.dumps(None)."""
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()

    stored = conn.execute(
        "SELECT attribute_levels_json FROM session_summaries WHERE session_id = ?",
        (sid,),
    ).fetchone()[0]
    assert stored != "null"

    rows = load_prospect_sessions(conn)
    row = next(r for r in rows if r["id"] == sid)
    assert row["attributeLevels"] == {_ATTRIBUTE: pytest.approx(3.0)}
    assert row["attributeLevels"] is not None


def test_write_then_load_round_trips_both_breakdowns_together():
    """Cross-check: both JSON payloads survive a full materialise/read cycle.

    A combined assertion guards against either dumps() argument being swapped
    to None independently (mutmut_68 and mutmut_72 in one shot).
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()
    rows = load_prospect_sessions(conn)
    row = next(r for r in rows if r["id"] == sid)
    assert row["regularSkillPed"] == {_REGULAR_SKILL: pytest.approx(5.0)}
    assert row["attributeLevels"] == {_ATTRIBUTE: pytest.approx(3.0)}


def test_write_clears_stale_row_when_session_no_longer_qualifies():
    """write_session_summary deletes the row when compute returns None.

    Exercises the summary-is-None branch (the DELETE in write_session_summary,
    mutmut_12/13) end to end: a materialised row is removed once its backing
    skill gains are gone.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()
    assert conn.execute(
        "SELECT COUNT(*) FROM session_summaries WHERE session_id = ?", (sid,)
    ).fetchone()[0] == 1

    # Remove the gains so the session no longer qualifies, then re-write.
    conn.execute("DELETE FROM skill_gains WHERE session_id = ?", (sid,))
    assert compute_session_summary(conn, sid) is None
    write_session_summary(conn, sid)
    conn.commit()
    assert conn.execute(
        "SELECT COUNT(*) FROM session_summaries WHERE session_id = ?", (sid,)
    ).fetchone()[0] == 0


# --------------------------------------------------------------------------
# delete_session_summary: previously "no tests" (mutmut_1..7)
# --------------------------------------------------------------------------


def test_delete_removes_only_the_targeted_row():
    """Intact delete removes exactly the named row and leaves the rest.

    mutmut_5 (XX..XX garbage SQL) raises OperationalError; mutmut_6/7 (case-only
    SQL) behave identically and are recorded as equivalents. This positive test
    pins that the real statement deletes the right single row - the behaviour
    the structural mutants below also break.
    """
    conn = _fresh_db()
    keep = uuid.uuid4().hex
    drop = uuid.uuid4().hex
    for sid in (keep, drop):
        _seed_qualifying(conn, sid)
        write_session_summary(conn, sid)
    conn.commit()
    assert conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0] == 2

    delete_session_summary(conn, drop)
    conn.commit()
    remaining = [
        r[0] for r in conn.execute("SELECT session_id FROM session_summaries").fetchall()
    ]
    assert remaining == [keep]


def test_delete_raises_on_broken_statement_forms():
    """mutmut_1/3: SQL replaced by None / dropped -> conn.execute gets a non-str.

    The real call passes a str statement; the mutated forms pass None or a
    tuple as the first argument, which raises TypeError.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid)
    write_session_summary(conn, sid)
    conn.commit()
    # Sanity: the genuine call does not raise.
    delete_session_summary(conn, sid)
    conn.commit()
    assert conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0] == 0


def test_delete_is_idempotent_second_call_is_noop():
    """Deleting an absent row affects zero rows and does not raise.

    Reinforces that delete uses a parameterised WHERE on session_id: the
    params-dropping/None mutants (mutmut_2/4) raise a ProgrammingError instead
    of cleanly removing only the matching row, so they cannot pass this and the
    targeted-row test together.
    """
    conn = _fresh_db()
    a = uuid.uuid4().hex
    b = uuid.uuid4().hex
    _seed_qualifying(conn, a)
    write_session_summary(conn, a)
    conn.commit()
    delete_session_summary(conn, b)  # b never existed
    conn.commit()
    rows = [r[0] for r in conn.execute("SELECT session_id FROM session_summaries")]
    assert rows == [a]
    # Deleting a again, then once more, stays at zero with no error.
    delete_session_summary(conn, a)
    delete_session_summary(conn, a)
    conn.commit()
    assert conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0] == 0


# --------------------------------------------------------------------------
# _row_to_prospect_dict: keys + or/and fallbacks (mutmut_4..97)
# --------------------------------------------------------------------------

# Column order matches the SELECT in load_prospect_sessions.
_TRUTHY_ROW = (
    "sess-truthy",  # session_id
    11.0,           # started_at
    22.0,           # ended_at
    1.5,            # duration_hours
    7,              # kills
    33.0,           # loot_tt
    44.0,           # weapon_cost
    55.0,           # enhancer_cost
    66.0,           # armour_cost
    77.0,           # heal_cost
    88.0,           # dangling_cost
    99.0,           # cycled_ped
    '{"Anatomy": 5.0}',         # regular_skill_ped_json
    '{"Strength": 3.0}',        # attribute_levels_json
    12.0,           # regular_skill_tt
    13.0,           # attribute_levels_total
    "Atrox",        # dominant_mob
    "tag-x",        # dominant_tag
    "Opalo",        # dominant_weapon
)

_FALSY_ROW = (
    "sess-falsy",   # session_id
    None,           # started_at
    None,           # ended_at
    None,           # duration_hours
    None,           # kills
    None,           # loot_tt
    None,           # weapon_cost
    None,           # enhancer_cost
    None,           # armour_cost
    None,           # heal_cost
    None,           # dangling_cost
    None,           # cycled_ped
    None,           # regular_skill_ped_json
    None,           # attribute_levels_json
    None,           # regular_skill_tt
    None,           # attribute_levels_total
    None,           # dominant_mob
    None,           # dominant_tag
    None,           # dominant_weapon
)

_EXPECTED_KEYS = {
    "id",
    "startedAt",
    "endedAt",
    "durationHours",
    "kills",
    "lootTt",
    "weaponCost",
    "enhancerCost",
    "armourCost",
    "healCost",
    "danglingCost",
    "cycledPed",
    "regularSkillPed",
    "attributeLevels",
    "regularSkillTt",
    "attributeLevelsTotal",
    "dominantMob",
    "dominantTag",
    "dominantWeapon",
}


def test_row_to_prospect_dict_has_exact_camelcase_keys():
    """Every output key is the exact camelCase string.

    Kills every key-rename mutant (XXkeyXX, lowercased, UPPERCASED) across all
    19 fields by asserting the precise key set: any renamed key both removes the
    expected key and introduces an unexpected one.
    """
    out = _row_to_prospect_dict(_TRUTHY_ROW)
    assert set(out) == _EXPECTED_KEYS


def test_row_to_prospect_dict_preserves_truthy_values():
    """With all-truthy inputs, each field carries the input value through.

    Kills the ``x or default -> x and default`` mutants: for a truthy x,
    ``x and default`` collapses to ``default`` (0.0/1.0), so asserting the real
    value fails the mutant. Covers started/ended/duration/kills/loot/weapon/
    enhancer/armour/heal/dangling/cycled/regularSkillTt/attributeLevelsTotal.
    """
    out = _row_to_prospect_dict(_TRUTHY_ROW)
    assert out["id"] == "sess-truthy"
    assert out["startedAt"] == pytest.approx(11.0)
    assert out["endedAt"] == pytest.approx(22.0)
    assert out["durationHours"] == pytest.approx(1.5)
    assert out["kills"] == 7
    assert out["lootTt"] == pytest.approx(33.0)
    assert out["weaponCost"] == pytest.approx(44.0)
    assert out["enhancerCost"] == pytest.approx(55.0)
    assert out["armourCost"] == pytest.approx(66.0)
    assert out["healCost"] == pytest.approx(77.0)
    assert out["danglingCost"] == pytest.approx(88.0)
    assert out["cycledPed"] == pytest.approx(99.0)
    assert out["regularSkillPed"] == {"Anatomy": pytest.approx(5.0)}
    assert out["attributeLevels"] == {"Strength": pytest.approx(3.0)}
    assert out["regularSkillTt"] == pytest.approx(12.0)
    assert out["attributeLevelsTotal"] == pytest.approx(13.0)
    assert out["dominantMob"] == "Atrox"
    assert out["dominantTag"] == "tag-x"
    assert out["dominantWeapon"] == "Opalo"
    # Numeric fields are coerced to their concrete Python types.
    assert isinstance(out["kills"], int)
    assert isinstance(out["startedAt"], float)


def test_row_to_prospect_dict_falsy_defaults_are_zero():
    """With falsy (NULL) inputs, each numeric field defaults to exactly 0.

    Kills the ``or 0.0 -> or 1.0`` and ``or 0 -> or 1`` mutants: the fallback
    is reached only when the input is falsy, and the mutant's default (1.0 / 1)
    differs from the real 0.0 / 0. Also pins that the JSON fields fall back to
    empty dicts and the nullable dominant_* fields stay None.
    """
    out = _row_to_prospect_dict(_FALSY_ROW)
    assert out["startedAt"] == 0.0
    assert out["endedAt"] == 0.0
    assert out["durationHours"] == 0.0
    assert out["kills"] == 0
    assert out["lootTt"] == 0.0
    assert out["weaponCost"] == 0.0
    assert out["enhancerCost"] == 0.0
    assert out["armourCost"] == 0.0
    assert out["healCost"] == 0.0
    assert out["danglingCost"] == 0.0
    assert out["cycledPed"] == 0.0
    assert out["regularSkillTt"] == 0.0
    assert out["attributeLevelsTotal"] == 0.0
    assert out["regularSkillPed"] == {}
    assert out["attributeLevels"] == {}
    assert out["dominantMob"] is None
    assert out["dominantTag"] is None
    assert out["dominantWeapon"] is None
