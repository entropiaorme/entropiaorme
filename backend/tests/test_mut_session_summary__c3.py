"""Mutation-hardening tests for ``load_prospect_sessions`` (cluster c3).

The surviving mutants in this cluster all mutate the *casing* of the two SQL
statements inside ``load_prospect_sessions`` (the ``missing``-detection
LEFT JOIN and the final ``SELECT ... FROM session_summaries``): keywords and
identifiers are flipped to lower- or upper-case (e.g. ``SELECT`` -> ``select``,
``session_summaries`` -> ``SESSION_SUMMARIES``).

SQLite treats SQL keywords *and* unquoted identifiers (table, column and alias
names) case-insensitively, so those mutations are behaviourally inert. The
tests below pin every observable behaviour of the function none the less:

* the lazy-rebuild branch (``missing`` non-empty -> ``write_session_summary``
  is invoked and the row is materialised, with version-staleness honoured),
* the read branch's exact column-to-key projection (positional SELECT order
  feeding ``_row_to_prospect_dict``),
* the no-rebuild fast path.

If any casing mutant were *not* equivalent, the rebuild or the projection would
diverge and one of these assertions would fire; they all pass against the real
source, which is the empirical confirmation of equivalence recorded for the
campaign.
"""

from __future__ import annotations

import sqlite3
import uuid

from backend.services.character_calc import ATTRIBUTE_SKILLS
from backend.services.session_summary import (
    SUMMARY_VERSION,
    load_prospect_sessions,
)
from backend.tracking.schema import init_tracking_tables

_REGULAR = "Laser Weaponry Technology"
_ATTR = sorted(ATTRIBUTE_SKILLS)[0]


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


def _seed_qualifying_session(
    conn: sqlite3.Connection,
    session_id: str,
    *,
    started_at: float = 0.0,
    ended_at: float = 3600.0,
    weapon_cost: float = 2.5,
    loot_total_ped: float = 7.25,
    regular_ped: float = 3.0,
    attr_amount: float = 4.0,
) -> None:
    """Seed one session that passes compute's qualifying filters."""
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, 0, 0, 0, 0)",
        (session_id, started_at, ended_at),
    )
    kill_id = uuid.uuid4().hex
    conn.execute(
        "INSERT INTO kills "
        "(id, session_id, mob_name, mob_species, mob_maturity, timestamp, "
        "enhancer_cost, loot_total_ped) "
        "VALUES (?, ?, 'Atrox', 'Atrox', 'Young', 0.0, 0.0, ?)",
        (kill_id, session_id, loot_total_ped),
    )
    conn.execute(
        "INSERT INTO kill_tool_stats "
        "(kill_id, tool_name, shots_fired, cost_per_shot) VALUES (?, 'Opalo', 1, ?)",
        (kill_id, weapon_cost),
    )
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, 0.01, ?)",
        (session_id, _REGULAR, regular_ped),
    )
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, ?, NULL)",
        (session_id, _ATTR, attr_amount),
    )
    conn.commit()


def test_lazy_rebuild_materialises_missing_row() -> None:
    """A qualifying session with no summary row must be rebuilt on read.

    Exercises the ``missing`` LEFT-JOIN query (mutants 7,8,10,11,13,14,16,17,
    19,20): if that query were broken the session would not be detected as
    missing, ``write_session_summary`` would not run, and no row would come
    back from the read.
    """
    conn = _fresh_db()
    sid = "sess-rebuild"
    _seed_qualifying_session(conn, sid)

    # Precondition: nothing materialised yet.
    assert conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0] == 0

    rows = load_prospect_sessions(conn)

    assert len(rows) == 1
    assert rows[0]["id"] == sid
    # The rebuild persisted a row at the current version.
    persisted = conn.execute(
        "SELECT summary_version FROM session_summaries WHERE session_id = ?", (sid,)
    ).fetchone()
    assert persisted is not None
    assert persisted[0] == SUMMARY_VERSION


def test_stale_version_row_is_rebuilt() -> None:
    """A pre-existing row at an older summary_version must be recomputed.

    Pins the ``ss.summary_version < ?`` arm of the missing query (mutants
    19,20 touch that line). A stale placeholder carries deliberately wrong
    aggregates; after the read they must be replaced by the recomputed values.
    """
    conn = _fresh_db()
    sid = "sess-stale"
    _seed_qualifying_session(conn, sid, weapon_cost=2.5)

    # Insert a stale-version row with bogus values.
    conn.execute(
        "INSERT INTO session_summaries ("
        "session_id, summary_version, started_at, ended_at, duration_hours, "
        "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
        "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
        "regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, "
        "dominant_weapon, computed_at) "
        "VALUES (?, ?, 0, 0, 0, 999, 999.0, 999.0, 0, 0, 0, 0, 999.0, '{}', '{}', "
        "999.0, 999.0, NULL, NULL, NULL, 0)",
        (sid, SUMMARY_VERSION - 1),
    )
    conn.commit()

    rows = load_prospect_sessions(conn)

    assert len(rows) == 1
    row = rows[0]
    # Recomputed, not the stale 999s.
    assert row["kills"] == 1
    assert row["weaponCost"] == 2.5
    assert row["cycledPed"] == 2.5
    assert (
        conn.execute(
            "SELECT summary_version FROM session_summaries WHERE session_id = ?",
            (sid,),
        ).fetchone()[0]
        == SUMMARY_VERSION
    )


def test_current_version_row_is_not_rebuilt() -> None:
    """A current-version row is the fast path: read it back verbatim.

    A current-version placeholder with sentinel values must NOT be recomputed
    (it is not "missing"), so the sentinels survive the read. This pins that
    the missing query's version comparison excludes current rows.
    """
    conn = _fresh_db()
    sid = "sess-fresh"
    _seed_qualifying_session(conn, sid)

    conn.execute(
        "INSERT INTO session_summaries ("
        "session_id, summary_version, started_at, ended_at, duration_hours, "
        "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
        "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
        "regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, "
        "dominant_weapon, computed_at) "
        "VALUES (?, ?, 11, 22, 0.5, 42, 13.0, 1.0, 0, 0, 0, 0, 1.0, '{}', '{}', "
        "0, 0, NULL, NULL, NULL, 0)",
        (sid, SUMMARY_VERSION),
    )
    conn.commit()

    rows = load_prospect_sessions(conn)

    assert len(rows) == 1
    # Sentinel kept => no rebuild happened for the current-version row.
    assert rows[0]["kills"] == 42
    assert rows[0]["startedAt"] == 11.0
    assert rows[0]["endedAt"] == 22.0


def test_read_projects_every_column_to_its_key() -> None:
    """The final SELECT's column order must map 1:1 onto the prospect dict.

    Exercises the read query (mutants 28,29,31,33,35,37,39,40). A row with a
    distinct value per column proves the positional SELECT order matches
    ``_row_to_prospect_dict``'s unpacking; any reorder/typo would scramble it.
    """
    conn = _fresh_db()
    sid = "sess-project"
    # No tracking_sessions row -> no rebuild; we control the summary row fully.
    conn.execute(
        "INSERT INTO session_summaries ("
        "session_id, summary_version, started_at, ended_at, duration_hours, "
        "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
        "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
        "regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, "
        "dominant_weapon, computed_at) "
        "VALUES (?, ?, 100.0, 200.0, 1.5, 7, 12.0, 3.0, 0.5, 0.25, 0.75, 0.125, "
        "4.7, ?, ?, 6.0, 8.0, 'Atrox', 'tag-x', 'Opalo', 0)",
        (
            sid,
            SUMMARY_VERSION,
            '{"Laser Weaponry Technology": 6.0}',
            '{"Strength": 8.0}',
        ),
    )
    conn.commit()

    rows = load_prospect_sessions(conn)

    assert len(rows) == 1
    row = rows[0]
    assert row == {
        "id": sid,
        "startedAt": 100.0,
        "endedAt": 200.0,
        "durationHours": 1.5,
        "kills": 7,
        "lootTt": 12.0,
        "weaponCost": 3.0,
        "enhancerCost": 0.5,
        "armourCost": 0.25,
        "healCost": 0.75,
        "danglingCost": 0.125,
        "cycledPed": 4.7,
        "regularSkillPed": {"Laser Weaponry Technology": 6.0},
        "attributeLevels": {"Strength": 8.0},
        "regularSkillTt": 6.0,
        "attributeLevelsTotal": 8.0,
        "dominantMob": "Atrox",
        "dominantTag": "tag-x",
        "dominantWeapon": "Opalo",
    }


def test_empty_database_returns_empty_list() -> None:
    """No sessions, no summaries -> empty list (no spurious rebuild)."""
    conn = _fresh_db()
    assert load_prospect_sessions(conn) == []


def test_active_session_without_summary_is_not_rebuilt() -> None:
    """Sessions still active (ended_at NULL) are excluded from the rebuild.

    Pins the ``WHERE s.ended_at IS NOT NULL`` filter of the missing query
    (mutants 13,14 touch that clause).
    """
    conn = _fresh_db()
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active) VALUES ('active', 0.0, NULL, 1)",
    )
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES ('active', 0.0, ?, 0.01, 3.0)",
        (_REGULAR,),
    )
    conn.commit()

    rows = load_prospect_sessions(conn)

    assert rows == []
    assert conn.execute("SELECT COUNT(*) FROM session_summaries").fetchone()[0] == 0
