"""Driver — orchestrates the demo seeder pipeline.

Wires the core seeder first. Per-domain seeders are registered via
``run(data_dir, extra_seeders=[...])``. Topological order is decided
here from each seeder's ``depends_on`` declaration.
"""

from __future__ import annotations

import logging
import sqlite3
from pathlib import Path

from backend.db.app_database import AppDatabase
from backend.scripts.demo_seed.canonical import CoreSeeder
from backend.scripts.demo_seed.contract import Seeder, SeedRunReport
from backend.tracking.schema import init_tracking_tables

log = logging.getLogger(__name__)

# Tables we count for the run report (others may exist; this is the "interesting" set).
_REPORTED_TABLES: tuple[str, ...] = (
    "equipment_library",
    "quests",
    "quest_mobs",
    "quest_playlists",
    "quest_playlist_items",
    "codex_progress",
    "codex_claims",
    "skill_calibrations",
    "tracking_sessions",
    "kills",
    "kill_loot_items",
    "kill_tool_stats",
    "ledger_entries",
    "ledger_presets",
    "inventory_items",
    "session_summaries",
    "skill_gains",
    "notable_events",
    "quest_claims",
    "session_quest_completions",
    "session_quest_analytics_links",
)


def run(data_dir: Path, extra_seeders: list[Seeder] | None = None) -> SeedRunReport:
    """Run the seeder pipeline against ``data_dir``.

    Creates ``data_dir`` if missing; opens / creates ``entropia_orme.db`` and
    initialises both app and tracking schemas. Runs CoreSeeder first (it
    builds + freezes CanonicalRefs and writes foundational rows). Then runs
    any per-domain seeders in topological order based on ``depends_on``.

    Aborts (without further DB writes) if any seeder reports synthetic-data
    violations. The DB is left in whatever state writes have already
    happened — caller is responsible for cleaning up if a partial seed is
    worse than no seed.
    """
    data_dir = Path(data_dir)
    data_dir.mkdir(parents=True, exist_ok=True)

    db_path = data_dir / "entropia_orme.db"
    log.info("Seeding demo data → %s", data_dir)

    # Initialise app + tracking schema on the same SQLite file (mirrors what
    # backend/main.py + HuntTracker.__init__ do at runtime).
    app_db = AppDatabase(db_path)
    init_tracking_tables(app_db.conn)
    app_db.conn.commit()

    report = SeedRunReport(data_dir=data_dir)

    try:
        # 1. Core seeder — build refs, validate, write foundational rows.
        core = CoreSeeder()
        refs = core.build_refs()
        report.demo_now = refs.timeline.demo_now

        violations = core.validate_synthetic_data(refs)
        if violations:
            report.violations.extend(f"core: {v}" for v in violations)
            return report

        refs = core.seed(refs, app_db.conn, data_dir)
        app_db.conn.commit()
        report.seeders_run.append(core.name)
        log.info("Core seeder complete; canonical refs frozen.")

        # 2. Per-domain seeders in topological order.
        pending: dict[str, Seeder] = {s.name: s for s in (extra_seeders or [])}
        if core.name in pending:
            report.violations.append(
                f"per-domain seeder reuses core's name {core.name!r} — rename it"
            )
            return report

        completed: set[str] = {core.name}

        while pending:
            ready = [
                s
                for s in pending.values()
                if all(dep in completed for dep in s.depends_on)
            ]
            if not ready:
                report.violations.append(
                    f"unresolvable dependencies — pending {sorted(pending)} "
                    f"blocked by missing seeders (completed: {sorted(completed)})"
                )
                break

            for seeder in ready:
                v = seeder.validate_synthetic_data(refs)
                if v:
                    report.violations.extend(f"{seeder.name}: {x}" for x in v)
                    return report
                seeder.seed(refs, app_db.conn, data_dir)
                app_db.conn.commit()
                report.seeders_run.append(seeder.name)
                completed.add(seeder.name)
                pending.pop(seeder.name)
                log.info("Seeder %r complete.", seeder.name)

        # 3. Row counts.
        for table in _REPORTED_TABLES:
            try:
                cur = app_db.conn.execute(f"SELECT COUNT(*) FROM {table}")
                report.rows_written[table] = int(cur.fetchone()[0])
            except sqlite3.OperationalError:
                # Table doesn't exist yet; skip silently.
                pass

    finally:
        app_db.close()

    return report


def format_report(report: SeedRunReport) -> str:
    """Human-readable report."""
    lines = [
        "Demo seed run summary",
        f"  data_dir         : {report.data_dir}",
        f"  demo_now (epoch) : {report.demo_now:.0f}",
        f"  seeders          : {', '.join(report.seeders_run) or '(none)'}",
    ]
    if report.violations:
        lines.append("  violations       :")
        for v in report.violations:
            lines.append(f"    - {v}")
    if report.rows_written:
        lines.append("  rows by table    :")
        for table, count in sorted(report.rows_written.items()):
            if count > 0:
                lines.append(f"    {table:40s} {count:>6d}")
    return "\n".join(lines)
