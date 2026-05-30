"""Property-based tests for skill-scan anchor archival in scan_completion.

Encodes the structural guarantees of ``make_skill_scan_completion``'s
``_on_complete`` callback over generated valid scan inputs and seeded prior
calibration trails:

- per-skill scan-anchor uniqueness after completion,
- the live scan anchor's level equals the scanned value verbatim,
- pre-existing non-scan rows (the believed-current codex/chatlog trail)
  survive a scan unchanged,
- archival/deletion is scoped to exactly the scanned skills,
- displaced scan anchors are conserved into the archive one-for-one.

Each property drives the real callback against a real ``AppDatabase`` so the
SQL, schema, and transaction behaviour are exercised end to end. The shared
``app_db`` fixture is reset at the start of every example because Hypothesis
reuses a function-scoped fixture across the examples it generates.
"""

import math

import pytest
from hypothesis import HealthCheck, assume, given, settings
from hypothesis import strategies as st

from backend.db.app_database import AppDatabase
from backend.services.scan_completion import make_skill_scan_completion

# Skill names: non-empty, free of the comma/quote characters that never occur
# in real OCR-extracted skill names and would only muddy equality reasoning.
_SKILL_NAMES = st.text(
    alphabet=st.characters(
        min_codepoint=33, max_codepoint=122, blacklist_characters=",'\""
    ),
    min_size=1,
    max_size=24,
)
_LEVELS = st.floats(
    min_value=0.0, max_value=1.0e6, allow_nan=False, allow_infinity=False
)
# A scan input: a non-empty mapping of distinct skill -> level.
_SCAN_LEVELS = st.dictionaries(_SKILL_NAMES, _LEVELS, min_size=1, max_size=8)
_NON_SCAN_SOURCES = st.sampled_from(["codex", "chatlog"])

# Each property opens with ``_reset(app_db)``, so the shared function-scoped DB
# fixture is explicitly cleared per generated example; the health check that
# guards against unreset fixtures is therefore satisfied by construction.
_isolated = settings(suppress_health_check=[HealthCheck.function_scoped_fixture])


@pytest.fixture
def app_db(tmp_path):
    db = AppDatabase(tmp_path / "test_app.db")
    yield db
    db.close()


def _reset(db) -> None:
    """Clear both calibration tables so each generated example starts clean."""
    db.conn.execute("DELETE FROM skill_calibrations")
    db.conn.execute("DELETE FROM skill_calibrations_archive")
    db.conn.commit()


def _seed(conn, skill_name, level, scanned_at, source="scan") -> None:
    conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, ?, ?)",
        (skill_name, level, source, scanned_at),
    )


def _live_scan_rows(conn, skill_name):
    return conn.execute(
        "SELECT level FROM skill_calibrations WHERE skill_name = ? AND source = 'scan'",
        (skill_name,),
    ).fetchall()


# --- per-skill scan-anchor uniqueness ---------------------------------------


@_isolated
@given(levels=_SCAN_LEVELS)
def test_each_scanned_skill_has_exactly_one_live_scan_anchor(app_db, levels):
    _reset(app_db)
    # Seed an assortment of prior anchors, including duplicate scan rows for
    # some skills, to confirm the archive+reinsert self-heals to one live row.
    for i, name in enumerate(levels):
        _seed(app_db.conn, name, float(i), 1000.0 + i, source="scan")
        _seed(app_db.conn, name, float(i) + 0.5, 1100.0 + i, source="scan")
    app_db.conn.commit()

    make_skill_scan_completion(app_db)(levels)

    for name in levels:
        assert len(_live_scan_rows(app_db.conn, name)) == 1


# --- scan anchor value equals the scanned level -----------------------------


@_isolated
@given(levels=_SCAN_LEVELS)
def test_live_scan_anchor_value_is_the_scanned_level(app_db, levels):
    _reset(app_db)
    make_skill_scan_completion(app_db)(levels)

    for name, level in levels.items():
        rows = _live_scan_rows(app_db.conn, name)
        assert len(rows) == 1
        stored = rows[0][0]
        assert not math.isnan(stored)
        assert stored == level


# --- non-scan rows survive a scan verbatim ----------------------------------


@_isolated
@given(
    levels=_SCAN_LEVELS,
    non_scan=st.lists(
        st.tuples(_SKILL_NAMES, _LEVELS, _NON_SCAN_SOURCES),
        max_size=6,
    ),
)
def test_non_scan_rows_are_immutable_across_a_scan(app_db, levels, non_scan):
    _reset(app_db)
    # A prior scan anchor per scanned skill so drift logging and archival both
    # have something to act on, alongside the non-scan trail under test.
    for i, name in enumerate(levels):
        _seed(app_db.conn, name, float(i), 1000.0 + i, source="scan")
    for j, (name, level, source) in enumerate(non_scan):
        _seed(app_db.conn, name, level, 1200.0 + j, source=source)
    app_db.conn.commit()

    before = app_db.conn.execute(
        "SELECT skill_name, level, source, scanned_at FROM skill_calibrations "
        "WHERE source != 'scan' ORDER BY id"
    ).fetchall()

    make_skill_scan_completion(app_db)(levels)

    after = app_db.conn.execute(
        "SELECT skill_name, level, source, scanned_at FROM skill_calibrations "
        "WHERE source != 'scan' ORDER BY id"
    ).fetchall()

    assert [tuple(r) for r in after] == [tuple(r) for r in before]
    # And no non-scan row ever leaks into the archive.
    archived_non_scan = app_db.conn.execute(
        "SELECT COUNT(*) FROM skill_calibrations_archive WHERE source != 'scan'"
    ).fetchone()[0]
    assert archived_non_scan == 0


# --- archival is scoped to the scanned skills -------------------------------


@_isolated
@given(
    levels=_SCAN_LEVELS,
    untouched=st.lists(
        st.tuples(_SKILL_NAMES, _LEVELS),
        max_size=6,
    ),
)
def test_archival_leaves_untouched_skill_anchors_intact(app_db, levels, untouched):
    _reset(app_db)
    # Restrict the untouched set to skills that are genuinely not scanned.
    untouched = [(n, v) for (n, v) in untouched if n not in levels]
    assume(len({n for n, _ in untouched}) == len(untouched))

    for i, name in enumerate(levels):
        _seed(app_db.conn, name, float(i), 1000.0 + i, source="scan")
    for k, (name, level) in enumerate(untouched):
        _seed(app_db.conn, name, level, 1300.0 + k, source="scan")
    app_db.conn.commit()

    make_skill_scan_completion(app_db)(levels)

    for name, level in untouched:
        rows = _live_scan_rows(app_db.conn, name)
        assert len(rows) == 1
        assert rows[0][0] == level
        archived = app_db.conn.execute(
            "SELECT COUNT(*) FROM skill_calibrations_archive WHERE skill_name = ?",
            (name,),
        ).fetchone()[0]
        assert archived == 0


# --- displaced scan anchors are conserved into the archive ------------------


@_isolated
@given(levels=_SCAN_LEVELS)
def test_displaced_scan_anchors_are_conserved_one_for_one(app_db, levels):
    _reset(app_db)
    # One prior live scan anchor per scanned skill; capture its identity so we
    # can assert the archive holds exactly that row, matched by original_id.
    prior = {}
    for i, name in enumerate(levels):
        _seed(app_db.conn, name, float(i) + 0.25, 1000.0 + i, source="scan")
    app_db.conn.commit()
    for name in levels:
        row = app_db.conn.execute(
            "SELECT id, level, source, scanned_at FROM skill_calibrations "
            "WHERE skill_name = ? AND source = 'scan'",
            (name,),
        ).fetchone()
        prior[name] = tuple(row)

    make_skill_scan_completion(app_db)(levels)

    for name, (pid, plevel, psource, pscanned_at) in prior.items():
        archived = app_db.conn.execute(
            "SELECT original_id, level, source, scanned_at "
            "FROM skill_calibrations_archive WHERE skill_name = ?",
            (name,),
        ).fetchall()
        # Exactly one archive row, conserving the displaced anchor verbatim.
        assert len(archived) == 1
        assert tuple(archived[0]) == (pid, plevel, psource, pscanned_at)
        # The displaced anchor's id is gone from the live table.
        still_live = app_db.conn.execute(
            "SELECT COUNT(*) FROM skill_calibrations WHERE id = ?", (pid,)
        ).fetchone()[0]
        assert still_live == 0
