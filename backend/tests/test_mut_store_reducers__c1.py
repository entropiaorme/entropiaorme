"""Mutation-hardening tests for the snapshot view functions in
``backend.testing.store_reducers``.

This file targets the three DB-backed view composers the consistency
apparatus exposes but the existing reducer-focused suites never drive
directly: ``quests_view_state``, ``scan_view_state`` and
``codex_view_state``. Each view reads a small SQL projection off the
live connection; the tests below stand up the exact table the view
queries in an in-memory SQLite database, seed it with rows that
distinguish the real behaviour from each mutant, and assert the precise
shape, keys, values, and ordering the view returns.

The seed data is chosen to separate mutations that would otherwise hide:
- distinct-vs-total skill counts differ (one skill scanned twice), so a
  view that reads the wrong COUNT column is caught;
- populated tables give non-zero counts, so a dropped query (``rows =
  None``) or a short-circuited ``and 0`` is caught;
- empty tables assert an exact ``0`` (never the ``or 1`` / ``else 1``
  fallbacks a mutant substitutes);
- the quests projection seeds rows in a deliberately non-rowid name
  order and asserts the rowid-ordered result, and seeds a non-matching
  ``event_type`` so the ``'quest_started'`` literal filter is pinned.

These views are pure reads, so the tests construct only an in-memory
``sqlite3.Connection`` with the column subset each query touches; no
production source, service, or device handle is needed.
"""

from __future__ import annotations

import sqlite3
from typing import cast

import pytest

from backend.services.quest_service import QuestService
from backend.testing.store_reducers import (
    CodexViewContext,
    QuestsViewContext,
    ScanViewContext,
    codex_view_state,
    quests_view_state,
    scan_view_state,
)

# --------------------------------------------------------------------------
# Schema helpers: the exact column subset each view's SQL touches.
# --------------------------------------------------------------------------


def _notable_events_conn() -> sqlite3.Connection:
    """Connection carrying the ``notable_events`` columns the quests view reads."""
    conn = sqlite3.connect(":memory:")
    conn.execute(
        """
        CREATE TABLE notable_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT,
            kill_id     INTEGER,
            event_type  TEXT,
            mob_or_item TEXT,
            value_ped   REAL,
            timestamp   REAL
        )
        """
    )
    return conn


def _insert_notable(
    conn: sqlite3.Connection,
    session_id: str,
    event_type: str,
    mob_or_item: str,
) -> None:
    conn.execute(
        "INSERT INTO notable_events (session_id, kill_id, event_type, "
        "mob_or_item, value_ped, timestamp) VALUES (?, NULL, ?, ?, 0, 0)",
        (session_id, event_type, mob_or_item),
    )
    conn.commit()


def _skill_calibrations_conn() -> sqlite3.Connection:
    """Connection carrying the ``skill_calibrations`` columns the scan view reads."""
    conn = sqlite3.connect(":memory:")
    conn.execute(
        """
        CREATE TABLE skill_calibrations (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            skill_name TEXT NOT NULL,
            level      REAL NOT NULL,
            source     TEXT NOT NULL,
            scanned_at REAL NOT NULL DEFAULT 0
        )
        """
    )
    return conn


def _insert_calibration(
    conn: sqlite3.Connection, skill_name: str, level: float
) -> None:
    conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source) "
        "VALUES (?, ?, 'scan')",
        (skill_name, level),
    )
    conn.commit()


def _codex_conn() -> sqlite3.Connection:
    """Connection carrying the ``codex_progress`` / ``codex_claims`` tables."""
    conn = sqlite3.connect(":memory:")
    conn.executescript(
        """
        CREATE TABLE codex_progress (
            species_name TEXT PRIMARY KEY,
            current_rank INTEGER NOT NULL DEFAULT 0,
            updated_at   REAL NOT NULL DEFAULT 0
        );
        CREATE TABLE codex_claims (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            species_name TEXT NOT NULL,
            rank         INTEGER NOT NULL DEFAULT 0
        );
        """
    )
    return conn


# --------------------------------------------------------------------------
# quests_view_state
# --------------------------------------------------------------------------


def test_quests_view_inactive_session_returns_exact_empty_shape() -> None:
    """A None ``session_id`` short-circuits to the empty projection.

    Pins the ``is None`` branch direction (mutmut_1), the exact keys of
    the inactive-return dict (mutmut_2..5), and that the early return is
    taken rather than the DB read.
    """
    conn = _notable_events_conn()
    # Seed a matching row so an inverted branch (querying despite a None
    # session) would surface a non-empty list and be caught.
    _insert_notable(conn, "sess-X", "quest_started", "ShouldNotAppear")
    try:
        view = quests_view_state(
            QuestsViewContext(
                quest_service=cast(QuestService, None), conn=conn, session_id=None
            )
        )
    finally:
        conn.close()

    assert view == {"session_id": None, "mission_names_received": []}
    assert set(view) == {"session_id", "mission_names_received"}


def test_quests_view_active_session_returns_names_in_rowid_order() -> None:
    """An active session reads ``quest_started`` rows in rowid order.

    Pins the query existing at all (mutmut_6..11, 14, 17 raise without
    it), the active-path result keys (mutmut_20..23), the single-column
    projection (mutmut_24 -> IndexError), and the ordering.
    """
    conn = _notable_events_conn()
    # Insert in an order whose alphabetical sort differs from insert
    # (rowid) order, so an ORDER-BY drop or a column swap is visible.
    _insert_notable(conn, "sess-1", "quest_started", "Zeta")
    _insert_notable(conn, "sess-1", "quest_started", "Alpha")
    _insert_notable(conn, "sess-1", "quest_started", "Mu")
    try:
        view = quests_view_state(
            QuestsViewContext(
                quest_service=cast(QuestService, None), conn=conn, session_id="sess-1"
            )
        )
    finally:
        conn.close()

    assert view == {
        "session_id": "sess-1",
        "mission_names_received": ["Zeta", "Alpha", "Mu"],
    }
    assert set(view) == {"session_id", "mission_names_received"}


def test_quests_view_filters_on_session_and_event_type_literal() -> None:
    """Only ``quest_started`` rows for the active session contribute.

    Pins the ``event_type = 'quest_started'`` literal filter: mutmut_16
    uppercases that literal to ``'QUEST_STARTED'`` (matching nothing) and
    the session-id binding (mutmut_8 drops the params).
    """
    conn = _notable_events_conn()
    _insert_notable(conn, "sess-1", "quest_started", "Wanted")
    # Wrong event type, same session: must be filtered out.
    _insert_notable(conn, "sess-1", "quest_completed", "WrongType")
    # Right event type, different session: must be filtered out.
    _insert_notable(conn, "sess-2", "quest_started", "OtherSession")
    try:
        view = quests_view_state(
            QuestsViewContext(
                quest_service=cast(QuestService, None), conn=conn, session_id="sess-1"
            )
        )
    finally:
        conn.close()

    assert view["mission_names_received"] == ["Wanted"]


def test_quests_view_active_session_with_no_matching_rows_is_empty_list() -> None:
    """An active session with no quest_started rows yields an empty list
    but still echoes the session id (distinguishes the active path from
    the inactive early return)."""
    conn = _notable_events_conn()
    try:
        view = quests_view_state(
            QuestsViewContext(
                quest_service=cast(QuestService, None), conn=conn, session_id="sess-9"
            )
        )
    finally:
        conn.close()

    assert view == {"session_id": "sess-9", "mission_names_received": []}


# --------------------------------------------------------------------------
# scan_view_state
# --------------------------------------------------------------------------


def test_scan_view_counts_distinct_and_total_calibration_rows() -> None:
    """Distinct-skill count and total-row count are read from the right
    COUNT columns.

    Seeds three rows over two distinct skill names so distinct (2) and
    total (3) differ. Pins: the query existing (mutmut_1, 2, 3 ->
    raise/empty), reading column 0 for distinct and column 1 for total
    (mutmut_9 swaps to column 1, mutmut_15 -> out-of-range column 2),
    the ``int(...)`` conversion (mutmut_7, 8, 13, 14), and the result
    keys (mutmut_18..21).
    """
    conn = _skill_calibrations_conn()
    _insert_calibration(conn, "Laser Weaponry Technology", 12.0)
    _insert_calibration(conn, "Laser Weaponry Technology", 13.5)
    _insert_calibration(conn, "Handgun", 7.0)
    try:
        view = scan_view_state(ScanViewContext(conn=conn))
    finally:
        conn.close()

    assert view == {
        "distinct_calibrated_skills": 2,
        "calibration_row_count": 3,
    }
    assert set(view) == {"distinct_calibrated_skills", "calibration_row_count"}
    # Exact types: ints, not None.
    assert isinstance(view["distinct_calibrated_skills"], int)
    assert isinstance(view["calibration_row_count"], int)


def test_scan_view_empty_db_is_exactly_zero() -> None:
    """A fresh DB gives exactly zero on both counts.

    Pins the ``or 0`` defaults against the ``or 1`` / ``else 1`` mutants
    (mutmut_10, 11, 16, 17) which would surface a spurious 1.
    """
    conn = _skill_calibrations_conn()
    try:
        view = scan_view_state(ScanViewContext(conn=conn))
    finally:
        conn.close()

    assert view == {
        "distinct_calibrated_skills": 0,
        "calibration_row_count": 0,
    }


# --------------------------------------------------------------------------
# codex_view_state
# --------------------------------------------------------------------------


def test_codex_view_counts_progress_and_claim_rows() -> None:
    """Progress-row and claim-row counts come from their own tables.

    Seeds two progress rows and three claim rows so the two counts
    differ. Pins: both queries existing (mutmut_1, 2, 6, 7 -> raise),
    the lowercase SQL targeting the right tables, reading column 0
    (mutmut_15, 22 -> out-of-range column 1), the ``int(...)``
    conversion (mutmut_13, 14, 20, 21), and the result keys
    (mutmut_11, 12, 18, 19).
    """
    conn = _codex_conn()
    conn.execute(
        "INSERT INTO codex_progress (species_name, current_rank) VALUES ('Atrox', 3)"
    )
    conn.execute(
        "INSERT INTO codex_progress (species_name, current_rank) VALUES ('Daikiba', 1)"
    )
    conn.execute("INSERT INTO codex_claims (species_name, rank) VALUES ('Atrox', 1)")
    conn.execute("INSERT INTO codex_claims (species_name, rank) VALUES ('Atrox', 2)")
    conn.execute("INSERT INTO codex_claims (species_name, rank) VALUES ('Daikiba', 1)")
    conn.commit()
    try:
        view = codex_view_state(CodexViewContext(conn=conn))
    finally:
        conn.close()

    assert view == {
        "codex_progress_row_count": 2,
        "codex_claim_row_count": 3,
    }
    assert set(view) == {"codex_progress_row_count", "codex_claim_row_count"}
    assert isinstance(view["codex_progress_row_count"], int)
    assert isinstance(view["codex_claim_row_count"], int)


def test_codex_view_empty_db_is_exactly_zero() -> None:
    """A fresh DB gives exactly zero on both counts.

    Pins the ``or 0`` defaults against the ``or 1`` / ``else 1`` mutants
    (mutmut_16, 17, 23, 24) which would surface a spurious 1.
    """
    conn = _codex_conn()
    try:
        view = codex_view_state(CodexViewContext(conn=conn))
    finally:
        conn.close()

    assert view == {
        "codex_progress_row_count": 0,
        "codex_claim_row_count": 0,
    }


def test_codex_progress_and_claims_counts_are_independent() -> None:
    """Each count reads its own table: progress rows do not leak into the
    claim count and vice versa. Strengthens the table-targeting SQL
    (a query reading the wrong table would conflate the two counts)."""
    conn = _codex_conn()
    conn.execute(
        "INSERT INTO codex_progress (species_name, current_rank) VALUES ('Atrox', 3)"
    )
    conn.commit()
    try:
        view = codex_view_state(CodexViewContext(conn=conn))
    finally:
        conn.close()

    assert view["codex_progress_row_count"] == 1
    assert view["codex_claim_row_count"] == 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(pytest.main([__file__, "-v"]))
