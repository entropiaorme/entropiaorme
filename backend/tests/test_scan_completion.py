"""Tests for scan-time anchor archival in scan_completion."""

import pytest

from backend.db.app_database import AppDatabase
from backend.services.scan_completion import make_skill_scan_completion


@pytest.fixture
def app_db(tmp_path):
    return AppDatabase(tmp_path / "test_app.db")


def _seed_skill_scan(
    conn, skill_name: str, level: float, scanned_at: float, source: str = "scan"
) -> int:
    cur = conn.execute(
        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, ?, ?)",
        (skill_name, level, source, scanned_at),
    )
    return cur.lastrowid


# ── scan-time archival ──────────────────────────────────────────────────────


def test_skill_scan_archives_prior_anchor(app_db):
    _seed_skill_scan(app_db.conn, "Anatomy", 100.0, 1000.0)
    app_db.conn.commit()

    on_complete = make_skill_scan_completion(app_db)
    on_complete({"Anatomy": 105.0})

    active = app_db.conn.execute(
        "SELECT level FROM skill_calibrations WHERE skill_name = 'Anatomy' AND source = 'scan'"
    ).fetchall()
    assert len(active) == 1
    assert active[0][0] == 105.0

    archive = app_db.conn.execute(
        "SELECT level, source FROM skill_calibrations_archive WHERE skill_name = 'Anatomy'"
    ).fetchall()
    assert len(archive) == 1
    assert archive[0][0] == 100.0
    assert archive[0][1] == "scan"


def test_skill_scan_preserves_non_scan_rows_in_active(app_db):
    """Codex/chatlog rows form the believed-current trail and stay live."""
    _seed_skill_scan(app_db.conn, "Aim", 500.0, 1000.0, source="scan")
    _seed_skill_scan(app_db.conn, "Aim", 510.0, 1100.0, source="codex")
    _seed_skill_scan(app_db.conn, "Aim", 515.0, 1150.0, source="chatlog")
    app_db.conn.commit()

    on_complete = make_skill_scan_completion(app_db)
    on_complete({"Aim": 520.0})

    active = app_db.conn.execute(
        "SELECT source, level FROM skill_calibrations WHERE skill_name = 'Aim' ORDER BY scanned_at"
    ).fetchall()
    assert [(r[0], r[1]) for r in active] == [
        ("codex", 510.0),
        ("chatlog", 515.0),
        ("scan", 520.0),
    ]
    archive = app_db.conn.execute(
        "SELECT source, level FROM skill_calibrations_archive WHERE skill_name = 'Aim'"
    ).fetchall()
    assert [(r[0], r[1]) for r in archive] == [("scan", 500.0)]


def test_skill_scan_with_no_prior_anchor_is_a_noop_for_archive(app_db):
    on_complete = make_skill_scan_completion(app_db)
    on_complete({"Athletics": 50.0})

    active = app_db.conn.execute(
        "SELECT level FROM skill_calibrations WHERE skill_name = 'Athletics'"
    ).fetchall()
    assert len(active) == 1
    archive_count = app_db.conn.execute(
        "SELECT COUNT(*) FROM skill_calibrations_archive"
    ).fetchone()[0]
    assert archive_count == 0


def test_skill_scan_archives_only_targeted_skills(app_db):
    """A scan that touches Anatomy must not archive Aim's anchor."""
    _seed_skill_scan(app_db.conn, "Anatomy", 100.0, 1000.0)
    _seed_skill_scan(app_db.conn, "Aim", 500.0, 1000.0)
    app_db.conn.commit()

    on_complete = make_skill_scan_completion(app_db)
    on_complete({"Anatomy": 105.0})

    aim_active = app_db.conn.execute(
        "SELECT level FROM skill_calibrations WHERE skill_name = 'Aim' AND source = 'scan'"
    ).fetchall()
    assert len(aim_active) == 1
    assert aim_active[0][0] == 500.0
    aim_archive = app_db.conn.execute(
        "SELECT COUNT(*) FROM skill_calibrations_archive WHERE skill_name = 'Aim'"
    ).fetchone()[0]
    assert aim_archive == 0
