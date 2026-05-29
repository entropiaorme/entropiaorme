"""Mutation-hardening tests for backend.services.scan_completion.

These tests pin the surviving/no-test mutants in the scan_completion
cluster: the calibration-read helpers, the drift-detection guard, the
drift-log formatting (both summary lines), the completion callback's
insert + stored-points log line, and the hydrate state reader.

The drift logger and the completion callback are pure side-effect on the
module logger, so the formatting mutants are observable only by capturing
the emitted records and asserting the fully formatted message (calling
``record.getMessage()`` so a broken format string or arg list raises).
"""

import logging

import pytest

from backend.db.app_database import AppDatabase
from backend.services.scan_completion import (
    _get_last_skill_scan_time,
    _get_latest_skill_levels,
    _has_post_scan_skill_updates,
    _log_skill_scan_drift,
    hydrate_skill_scan_state,
    make_skill_scan_completion,
    log as scan_completion_log,
)


@pytest.fixture
def app_db(tmp_path):
    db = AppDatabase(tmp_path / "test_app.db")
    yield db
    db.close()


def _seed(conn, skill_name, level, scanned_at, source="scan"):
    cur = conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
        "VALUES (?, ?, ?, ?)",
        (skill_name, level, source, scanned_at),
    )
    return cur.lastrowid


class _RecordCatcher(logging.Handler):
    """Capture LogRecords emitted on the scan_completion module logger.

    Records are stored raw so a test can call ``getMessage()`` itself: a
    mutated format string / argument list raises there, which fails the
    asserting test and so kills the mutant.
    """

    def __init__(self):
        super().__init__(level=logging.INFO)
        self.records: list[logging.LogRecord] = []

    def emit(self, record):
        self.records.append(record)


@pytest.fixture
def caught():
    handler = _RecordCatcher()
    prev_level = scan_completion_log.level
    scan_completion_log.addHandler(handler)
    scan_completion_log.setLevel(logging.INFO)
    try:
        yield handler
    finally:
        scan_completion_log.removeHandler(handler)
        scan_completion_log.setLevel(prev_level)


def _messages(handler):
    """Fully formatted messages; raises if a record's format/args mismatch."""
    return [r.getMessage() for r in handler.records]


# ── _get_latest_skill_levels ────────────────────────────────────────────────


def test_get_latest_skill_levels_keys_are_skill_names(app_db):
    """The dict is keyed by skill_name (row[0]), value is the level (row[1]).

    Kills mutmut_3 which keys the dict by the level instead of the name.
    """
    _seed(app_db.conn, "Anatomy", 42.0, 1000.0)
    app_db.conn.commit()

    result = _get_latest_skill_levels(app_db.conn)

    assert result == {"Anatomy": 42.0}
    assert "Anatomy" in result
    assert 42.0 not in result  # level must not become the key


# ── _get_last_skill_scan_time ───────────────────────────────────────────────


def test_get_last_skill_scan_time_returns_max_scan_timestamp(app_db):
    """Returns MAX(scanned_at) over source='scan' rows.

    Kills mutmut_1 (row=None -> None) and mutmut_6 (value=None -> None).
    """
    _seed(app_db.conn, "Aim", 5.0, 1000.0, source="scan")
    _seed(app_db.conn, "Aim", 6.0, 3000.0, source="scan")
    _seed(app_db.conn, "Aim", 7.0, 9999.0, source="codex")  # non-scan ignored
    app_db.conn.commit()

    assert _get_last_skill_scan_time(app_db.conn) == 3000.0


def test_get_last_skill_scan_time_filters_on_scan_source_literal(app_db):
    """The WHERE clause matches the lowercase 'scan' literal.

    Kills mutmut_5 which uppercases the literal to 'SCAN' and so matches
    no rows (returning None instead of the timestamp).
    """
    _seed(app_db.conn, "Aim", 5.0, 2500.0, source="scan")
    app_db.conn.commit()

    assert _get_last_skill_scan_time(app_db.conn) == 2500.0


def test_get_last_skill_scan_time_none_when_no_scan_rows(app_db):
    _seed(app_db.conn, "Aim", 5.0, 2500.0, source="codex")
    app_db.conn.commit()

    assert _get_last_skill_scan_time(app_db.conn) is None


# ── _has_post_scan_skill_updates ────────────────────────────────────────────


def test_has_post_scan_skill_updates_true_for_later_non_scan_row(app_db):
    """True iff a non-scan row exists with scanned_at strictly after the time.

    Kills mutmut_1 (row=None -> always False) and mutmut_6 (inverts the
    None check so the truth value is flipped).
    """
    _seed(app_db.conn, "Aim", 6.0, 2000.0, source="codex")
    app_db.conn.commit()

    assert _has_post_scan_skill_updates(app_db.conn, 1000.0) is True


def test_has_post_scan_skill_updates_false_without_later_non_scan_row(app_db):
    _seed(app_db.conn, "Aim", 6.0, 2000.0, source="scan")  # scan source excluded
    _seed(app_db.conn, "Aim", 6.5, 500.0, source="codex")  # before the cutoff
    app_db.conn.commit()

    assert _has_post_scan_skill_updates(app_db.conn, 1000.0) is False


# ── _log_skill_scan_drift: drift IS logged when there is drift ───────────────


def _seed_drift_scenario(conn):
    """A tracked anchor + a later non-scan update, so drift logging fires."""
    _seed(conn, "Anatomy", 100.0, 1000.0, source="scan")
    _seed(conn, "Anatomy", 130.0, 2000.0, source="codex")
    conn.commit()


def test_drift_is_logged_when_drift_exists(app_db, caught):
    """A prior scan anchor + a later non-scan update + a differing scan input
    must emit both drift summary lines.

    Kills the guard/short-circuit mutants that suppress logging:
    mutmut_1 (last_scan_time=None), mutmut_4 (is None -> is not None),
    mutmut_5 (drops `not` on _has_post...), mutmut_7 (passes None as the
    scan time), mutmut_12 (drift=None), mutmut_17 (`not drift` -> `drift`).
    """
    _seed_drift_scenario(app_db.conn)

    _log_skill_scan_drift(app_db.conn, {"Anatomy": 100.0})

    messages = _messages(caught)
    assert len(messages) == 2
    assert messages[0].startswith(
        "Skill scan drift before recalibration: compared=1"
    )
    assert messages[1].startswith("Skill scan drift worst skill: Anatomy")


def test_no_drift_log_without_post_scan_updates(app_db, caught):
    """With a prior scan anchor but NO later non-scan update, nothing logs.

    Kills mutmut_3 (`or` -> `and`): the mutant stops short-circuiting on a
    present-but-stale scan time and logs anyway.
    """
    _seed(app_db.conn, "Anatomy", 100.0, 1000.0, source="scan")
    app_db.conn.commit()

    _log_skill_scan_drift(app_db.conn, {"Anatomy": 130.0})

    assert _messages(caught) == []


# ── _log_skill_scan_drift: exact formatting of the summary line ─────────────


def test_drift_summary_line_is_formatted_exactly(app_db, caught):
    """Pin the first summary line verbatim.

    Kills the first-log mutants: format string set to None / dropped
    (18, 26), any positional arg set to None (19-25), any arg dropped
    (27-33), and the string-content edits (34 marker, 35 lower, 36 upper,
    37 marker, 38 upper). Calling getMessage() makes the None/arg-count
    mutants raise.
    """
    # tracked Anatomy=100, scanned=130 -> signed +30, abs 30, pct 30/130*100.
    _seed(app_db.conn, "Anatomy", 100.0, 1000.0, source="scan")
    _seed(app_db.conn, "Anatomy", 100.0, 2000.0, source="codex")
    app_db.conn.commit()

    _log_skill_scan_drift(app_db.conn, {"Anatomy": 130.0})

    messages = _messages(caught)
    assert len(messages) == 2
    expected_pct = 30.0 / 130.0 * 100.0
    assert messages[0] == (
        "Skill scan drift before recalibration: compared=1 total_abs=30.0000 "
        f"avg_abs=30.0000 avg_abs_pct={expected_pct:.4f}% "
        "signed_total=30.0000 tracked_only=0 scan_only=0"
    )


def test_drift_worst_line_is_formatted_exactly(app_db, caught):
    """Pin the second (worst-skill) summary line verbatim.

    Kills the second-log mutants: format string set to None / dropped
    (53, 59), any positional arg set to None (54-58), any arg dropped
    (60-64), and the string-content edits (65 marker, 66 lower, 67 upper).
    """
    _seed(app_db.conn, "Anatomy", 100.0, 1000.0, source="scan")
    _seed(app_db.conn, "Anatomy", 100.0, 2000.0, source="codex")
    app_db.conn.commit()

    _log_skill_scan_drift(app_db.conn, {"Anatomy": 130.0})

    messages = _messages(caught)
    assert len(messages) == 2
    assert messages[1] == (
        "Skill scan drift worst skill: Anatomy tracked=100.0000 "
        "scanned=130.0000 delta=+30.0000 abs=30.0000"
    )


# ── make_skill_scan_completion: the INSERT actually persists scan rows ──────


def test_completion_inserts_scan_row(app_db):
    """The callback inserts a source='scan' row with the scanned level.

    Guards the INSERT shape against any future regression and exercises the
    happy path the other completion tests rely on.
    """
    make_skill_scan_completion(app_db)({"Aim": 12.5})

    rows = app_db.conn.execute(
        "SELECT level, source FROM skill_calibrations WHERE skill_name = 'Aim'"
    ).fetchall()
    assert [(r[0], r[1]) for r in rows] == [(12.5, "scan")]


def test_completion_logs_stored_points_line_exactly(app_db, caught):
    """Pin the final 'stored N calibration points' log line verbatim.

    Kills mutmut_18 (msg None), 19 (count arg None), 20 (format string
    dropped), 21 (count arg dropped), 22 (marker), 23 (lower), 24 (upper
    with invalid %D). getMessage() raises for the None/%D/arg mutants.
    """
    make_skill_scan_completion(app_db)({"Aim": 1.0, "Anatomy": 2.0, "Athletics": 3.0})

    messages = _messages(caught)
    assert "Skill scan stored 3 calibration points" in messages


# ── hydrate_skill_scan_state ────────────────────────────────────────────────


def test_hydrate_returns_last_time_and_distinct_count(app_db):
    """Returns (MAX(scanned_at), COUNT(DISTINCT skill_name)) for scan rows.

    Kills mutmut_1 (row=None), 2/3 (broken SQL -> error), 5 (uppercased
    'SCAN' literal -> matches nothing), 6 (last_time=None), 7 (float(None)
    -> error), 8 (last_time=float(count)), 11 (count=None), 12 (row[2] ->
    IndexError).
    """
    _seed(app_db.conn, "Aim", 5.0, 1000.0, source="scan")
    _seed(app_db.conn, "Aim", 6.0, 3000.0, source="scan")
    _seed(app_db.conn, "Anatomy", 7.0, 2500.0, source="scan")
    _seed(app_db.conn, "Athletics", 8.0, 9999.0, source="codex")  # non-scan
    app_db.conn.commit()

    last_time, count = hydrate_skill_scan_state(app_db)

    assert last_time == 3000.0
    assert count == 2


def test_hydrate_zero_timestamp_anchor_is_returned_via_count_guard(app_db):
    """A scan anchor at scanned_at=0.0 still yields last_time=0.0.

    The orig guard is `row and row[0]`; with row[0]==0.0 (falsy) it returns
    None. Kills mutmut_10 (`row and row[1]`): row[1]==1 is truthy, so the
    mutant returns float(row[0])==0.0 instead of None - diverging from
    orig exactly here.
    """
    _seed(app_db.conn, "Aim", 5.0, 0.0, source="scan")
    app_db.conn.commit()

    last_time, count = hydrate_skill_scan_state(app_db)

    assert last_time is None
    assert count == 1


def test_hydrate_empty_db_returns_none_and_zero(app_db):
    """On an empty table MAX is NULL and COUNT is 0.

    Kills mutmut_9 (`row or row[0]`): the aggregate row (None, 0) is truthy,
    so the mutant takes float(row[0]) == float(None) and raises instead of
    returning (None, 0).
    """
    last_time, count = hydrate_skill_scan_state(app_db)

    assert last_time is None
    assert count == 0
