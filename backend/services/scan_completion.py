"""Skill scan completion callback.

Persists scanned skill levels into ``skill_calibrations`` and emits a
drift summary line comparing tracked vs. scanned values before
recalibration. Profession levels are derived from skill calibrations on
read (see ``character_calc.all_profession_levels``); the formula is the
canonical source, so there is no separate profession-scan persistence.
"""

import logging
import time
from collections.abc import Callable

from backend.services.scan_drift import summarize_level_drift

log = logging.getLogger(__name__)


def _get_latest_skill_levels(conn) -> dict[str, float]:
    """Latest calibrated level per skill.

    Ordered by `MAX(scanned_at)` with `MAX(id)` as a tiebreaker; the
    tiebreaker only matters when two rows share a timestamp.
    """
    rows = conn.execute(
        """
        WITH latest_ts AS (
            SELECT skill_name, MAX(scanned_at) AS ts
            FROM skill_calibrations
            GROUP BY skill_name
        )
        SELECT skill_name, level FROM skill_calibrations
        WHERE id IN (
            SELECT MAX(s2.id) FROM skill_calibrations s2
            JOIN latest_ts m ON s2.skill_name = m.skill_name AND s2.scanned_at = m.ts
            GROUP BY s2.skill_name
        )
        """
    ).fetchall()
    return {row[0]: float(row[1]) for row in rows}


def _get_last_skill_scan_time(conn) -> float | None:
    row = conn.execute(
        "SELECT MAX(scanned_at) FROM skill_calibrations WHERE source = 'scan'"
    ).fetchone()
    value = row[0] if row else None
    return float(value) if value is not None else None


def _has_post_scan_skill_updates(conn, scan_time: float) -> bool:
    row = conn.execute(
        """
        SELECT 1
        FROM skill_calibrations
        WHERE scanned_at > ?
          AND source != 'scan'
        LIMIT 1
        """,
        (scan_time,),
    ).fetchone()
    return row is not None


def _log_skill_scan_drift(conn, scanned_levels: dict[str, float]) -> None:
    last_scan_time = _get_last_skill_scan_time(conn)
    if last_scan_time is None or not _has_post_scan_skill_updates(conn, last_scan_time):
        return

    tracked_levels = _get_latest_skill_levels(conn)
    drift = summarize_level_drift(tracked_levels, scanned_levels)
    if not drift:
        return

    log.info(
        "Skill scan drift before recalibration: compared=%d total_abs=%.4f avg_abs=%.4f "
        "avg_abs_pct=%.4f%% signed_total=%.4f tracked_only=%d scan_only=%d",
        drift["compared_count"],
        drift["total_abs_diff"],
        drift["avg_abs_diff"],
        drift["avg_abs_pct"],
        drift["total_signed_diff"],
        drift["tracked_only_count"],
        drift["scan_only_count"],
    )
    log.info(
        "Skill scan drift worst skill: %s tracked=%.4f scanned=%.4f delta=%+.4f abs=%.4f",
        drift["worst_name"],
        drift["worst_tracked"],
        drift["worst_scanned"],
        drift["worst_signed_diff"],
        drift["worst_abs_diff"],
    )


def _archive_prior_skill_anchors(conn, skill_names: list[str]) -> None:
    """Move existing source='scan' rows for the given skills into the archive.

    Called inside the scan transaction, before the new anchor rows are
    inserted. Non-scan rows (codex/chatlog updates between scans) are
    untouched: they form the believed-current trail and stay live.
    """
    if not skill_names:
        return
    placeholders = ",".join("?" * len(skill_names))
    conn.execute(
        f"""
        INSERT INTO skill_calibrations_archive
            (original_id, skill_name, level, source, scanned_at)
        SELECT id, skill_name, level, source, scanned_at
        FROM skill_calibrations
        WHERE source = 'scan' AND skill_name IN ({placeholders})
        """,
        skill_names,
    )
    conn.execute(
        f"""
        DELETE FROM skill_calibrations
        WHERE source = 'scan' AND skill_name IN ({placeholders})
        """,
        skill_names,
    )


def make_skill_scan_completion(app_db) -> Callable[[dict[str, float]], None]:
    """Build the skill-scan completion callback."""

    def _on_complete(levels: dict[str, float]) -> None:
        with app_db.lock:
            _log_skill_scan_drift(app_db.conn, levels)
            scan_time = time.time()
            _archive_prior_skill_anchors(app_db.conn, list(levels.keys()))
            for skill_name, level in levels.items():
                app_db.conn.execute(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'scan', ?)",
                    (skill_name, level, scan_time),
                )
            app_db.conn.commit()
        log.info("Skill scan stored %d calibration points", len(levels))

    return _on_complete


def hydrate_skill_scan_state(app_db) -> tuple[float | None, int]:
    """Read last skill scan time + unique skills count from the DB."""
    with app_db.lock:
        row = app_db.conn.execute(
            "SELECT MAX(scanned_at), COUNT(DISTINCT skill_name) FROM skill_calibrations WHERE source = 'scan'"
        ).fetchone()
    last_time = float(row[0]) if row and row[0] else None
    count = row[1] if row else 0
    return last_time, count
