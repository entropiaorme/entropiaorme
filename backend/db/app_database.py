"""Application database — user data, equipment, tracking, settings.

Long-lived database with user-owned data.
"""

from __future__ import annotations

import logging
import sqlite3

from backend.db.base import BaseDatabase

log = logging.getLogger(__name__)

DB_VERSION = 32


# Tables that auto-fill a timestamp column on INSERT when callers leave it NULL.
_TIMESTAMPED_TABLES: tuple[tuple[str, str], ...] = (
    ("equipment_library", "updated_at"),
    ("quests", "updated_at"),
    ("quest_playlists", "updated_at"),
    ("quest_playlist_items", "updated_at"),
    ("session_quest_analytics_links", "linked_at"),
)


class AppDatabase(BaseDatabase):
    DB_VERSION = DB_VERSION

    def _migrate(self, from_version: int) -> None:
        if from_version == 0:
            self._current_schema()
            return

        # Legacy schemas before the supported window are not migrated.
        # Refuse rather than no-op into a current-version stamp over a
        # non-migrated schema.
        if from_version < 28:
            raise RuntimeError(
                f"Cannot migrate from v{from_version} to v{self.DB_VERSION}: "
                "legacy schema versions are not supported. Either rebuild the "
                "database from scratch or restore from a backup matching the "
                "current schema version."
            )

        # Forward migrations from v28.
        if from_version < 29:
            self._migrate_to_v29()
        if from_version < 30:
            self._migrate_to_v30()
        if from_version < 31:
            self._migrate_to_v31()
        if from_version < 32:
            self._migrate_to_v32()

    def _migrate_to_v29(self) -> None:
        """Drop the unused profession_calibrations + archive tables.

        Both were write-only in the v28 surface (no read path consumed
        them) and the user-facing profession scan flow has been removed
        in favour of the formula-derived view in `character_calc`.
        """
        self.conn.executescript("""
            DROP TABLE IF EXISTS profession_calibrations;
            DROP TABLE IF EXISTS profession_calibrations_archive;
        """)
        log.info("Migrated app DB to v29: dropped profession_calibrations tables")

    def _migrate_to_v30(self) -> None:
        """Add the nullable `deactivated_at` column to `kill_loot_items`.

        Enables the recoverable post-hoc loot-entry deactivation affordance on
        the analytics → sessions tab. The column is owned by the tracking
        schema (`backend/tracking/schema.py`), but the migration lives here
        because tracking tables have no version-counter migration system of
        their own: they rely on `CREATE TABLE IF NOT EXISTS` for fresh
        installs and on the app DB's versioned forward-migrations for
        in-place schema evolution.

        Defensive in two directions:
          - "no such table": the user installed a prior version but never
            started tracking, so kill_loot_items doesn't exist yet. The
            future `init_tracking_tables` call on Tracker init will create
            it with the column baked in (per the canonical schema).
          - "duplicate column name": the column was already added by a
            partial run; idempotent re-run skips cleanly.
        """
        try:
            self.conn.execute(
                "ALTER TABLE kill_loot_items ADD COLUMN deactivated_at REAL"
            )
            self.conn.commit()
            log.info("Migrated app DB to v30: added kill_loot_items.deactivated_at")
        except sqlite3.OperationalError as exc:
            msg = str(exc).lower()
            if "no such table" in msg:
                log.info(
                    "v30: kill_loot_items not present yet; column will land "
                    "via tracking_schema when Tracker first initialises"
                )
            elif "duplicate column" in msg:
                log.info("v30: kill_loot_items.deactivated_at already present, skipping")
            else:
                raise

    def _migrate_to_v31(self) -> None:
        """Add the nullable `original_mob_name` column to `kills`.

        Enables the recoverable post-hoc session-metadata edit affordance
        on the analytics sessions tab: the mass-rename endpoint preserves
        the pre-edit `kills.mob_name` value into `original_mob_name` on
        the first rename via COALESCE, so the inverse restore endpoint
        can revert the rename even after multiple consecutive renames.

        Defensive in the same two directions as v30 (and follows the
        same migration shape for parity):
          - "no such table": the user installed a prior version but never
            started tracking, so `kills` doesn't exist yet. The future
            `init_tracking_tables` call on Tracker init will create it
            with the column baked in.
          - "duplicate column name": idempotent re-run.
        """
        try:
            self.conn.execute(
                "ALTER TABLE kills ADD COLUMN original_mob_name TEXT"
            )
            self.conn.commit()
            log.info("Migrated app DB to v31: added kills.original_mob_name")
        except sqlite3.OperationalError as exc:
            msg = str(exc).lower()
            if "no such table" in msg:
                log.info(
                    "v31: kills not present yet; column will land via "
                    "tracking_schema when Tracker first initialises"
                )
            elif "duplicate column" in msg:
                log.info("v31: kills.original_mob_name already present, skipping")
            else:
                raise

    def _migrate_to_v32(self) -> None:
        """Add the `mob_tracking_mode` column to `tracking_sessions`.

        Records which input mode the session was captured under ('mob'
        vs 'tag') so post-hoc UI surfaces can choose label vocabulary
        ('Mob Attribution' vs 'Tag Attribution'). The column is
        NOT NULL with DEFAULT 'mob': pre-migration rows surface as
        mob-mode, which matches the historical implicit default. Any
        undocumented tag-mode usage before this migration loses its
        mode flavour cosmetically; the underlying data is unaffected
        (tag-mode sessions persist the tag string into kills.mob_name
        the same way as mob-mode sessions).

        Defensive in the same two directions as v30 / v31:
          - "no such table": the user installed a prior version but
            never started tracking. The future `init_tracking_tables`
            call on Tracker init will create tracking_sessions with the
            column baked in.
          - "duplicate column name": idempotent re-run.
        """
        try:
            self.conn.execute(
                "ALTER TABLE tracking_sessions ADD COLUMN "
                "mob_tracking_mode TEXT NOT NULL DEFAULT 'mob'"
            )
            self.conn.commit()
            log.info(
                "Migrated app DB to v32: added tracking_sessions.mob_tracking_mode"
            )
        except sqlite3.OperationalError as exc:
            msg = str(exc).lower()
            if "no such table" in msg:
                log.info(
                    "v32: tracking_sessions not present yet; column will land "
                    "via tracking_schema when Tracker first initialises"
                )
            elif "duplicate column" in msg:
                log.info(
                    "v32: tracking_sessions.mob_tracking_mode already present, "
                    "skipping"
                )
            else:
                raise

    def _current_schema(self) -> None:
        """Create the current schema directly for a fresh install."""
        conn = self.conn
        conn.executescript("""
            CREATE TABLE IF NOT EXISTS equipment_library (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                item_type       TEXT NOT NULL,
                catalog_id      TEXT,
                properties_json TEXT NOT NULL,
                created_at      REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at      REAL
            );

            CREATE TABLE IF NOT EXISTS skill_calibrations (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_name    TEXT NOT NULL,
                level         REAL NOT NULL,
                source        TEXT NOT NULL,
                scanned_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_skill_cal_name
                ON skill_calibrations(skill_name);
            CREATE INDEX IF NOT EXISTS idx_skill_cal_name_scanned
                ON skill_calibrations(skill_name, scanned_at DESC);

            CREATE TABLE IF NOT EXISTS ledger_entries (
                id            TEXT PRIMARY KEY,
                date          TEXT NOT NULL,
                type          TEXT NOT NULL,
                description   TEXT NOT NULL,
                amount        REAL NOT NULL,
                tag           TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS skill_gains (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id    TEXT NOT NULL,
                timestamp     REAL NOT NULL,
                skill_name    TEXT NOT NULL,
                amount        REAL NOT NULL,
                ped_value     REAL,
                created_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_skill_gains_session
                ON skill_gains(session_id);
            CREATE INDEX IF NOT EXISTS idx_skill_gains_skill
                ON skill_gains(skill_name);
            CREATE INDEX IF NOT EXISTS idx_skill_gains_timestamp
                ON skill_gains(timestamp);

            CREATE TABLE IF NOT EXISTS codex_progress (
                species_name TEXT PRIMARY KEY,
                current_rank INTEGER NOT NULL DEFAULT 0,
                updated_at   REAL NOT NULL DEFAULT (unixepoch('now'))
            );

            CREATE TABLE IF NOT EXISTS codex_claims (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                species_name   TEXT NOT NULL,
                rank           INTEGER NOT NULL,
                skill_name     TEXT NOT NULL,
                ped_value      REAL NOT NULL,
                claimed_at     REAL NOT NULL DEFAULT (unixepoch('now')),
                kind           TEXT NOT NULL DEFAULT 'rank',
                attribute_name TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_codex_claims_species
                ON codex_claims(species_name);

            CREATE TABLE IF NOT EXISTS tt_curve_observations (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_name    TEXT NOT NULL,
                from_level    REAL NOT NULL,
                level_gain    REAL NOT NULL,
                known_ped     REAL NOT NULL,
                curve_ped     REAL NOT NULL,
                deviation     REAL NOT NULL,
                source        TEXT NOT NULL DEFAULT 'codex',
                observed_at   REAL NOT NULL DEFAULT (unixepoch('now'))
            );

            CREATE TABLE IF NOT EXISTS quests (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                name              TEXT NOT NULL,
                planet            TEXT NOT NULL DEFAULT 'Calypso',
                waypoint          TEXT,
                cooldown_hours    REAL,
                reward_ped        REAL,
                reward_is_skill   INTEGER NOT NULL DEFAULT 0,
                expected_reward_markup_percent REAL,
                notes             TEXT,
                chain_name        TEXT,
                chain_position    INTEGER,
                chain_total       INTEGER,
                started_at        REAL,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        REAL NOT NULL DEFAULT (unixepoch('now')),
                category          TEXT,
                reward_description TEXT,
                updated_at        REAL
            );

            CREATE TABLE IF NOT EXISTS quest_mobs (
                quest_id      INTEGER NOT NULL REFERENCES quests(id),
                mob_name      TEXT NOT NULL,
                PRIMARY KEY (quest_id, mob_name)
            );

            CREATE TABLE IF NOT EXISTS quest_playlists (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                name              TEXT NOT NULL,
                planet            TEXT NOT NULL DEFAULT 'Calypso',
                estimated_minutes INTEGER NOT NULL DEFAULT 30,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at        REAL
            );

            CREATE TABLE IF NOT EXISTS quest_playlist_items (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                playlist_id   INTEGER NOT NULL REFERENCES quest_playlists(id),
                quest_id      INTEGER NOT NULL REFERENCES quests(id),
                sort_order    INTEGER NOT NULL DEFAULT 0,
                description   TEXT,
                group_type    TEXT NOT NULL DEFAULT 'immediate',
                updated_at    REAL
            );
            CREATE INDEX IF NOT EXISTS idx_qpi_playlist
                ON quest_playlist_items(playlist_id);

            CREATE TABLE IF NOT EXISTS session_quest_completions (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id    TEXT NOT NULL,
                quest_id      INTEGER NOT NULL,
                completed_at  REAL NOT NULL DEFAULT (unixepoch('now')),
                UNIQUE(session_id, quest_id)
            );
            CREATE INDEX IF NOT EXISTS idx_sqc_session
                ON session_quest_completions(session_id);
            CREATE INDEX IF NOT EXISTS idx_sqc_quest
                ON session_quest_completions(quest_id);

            CREATE TABLE IF NOT EXISTS session_quest_analytics_links (
                session_id    TEXT PRIMARY KEY,
                link_type     TEXT NOT NULL,
                quest_id      INTEGER,
                playlist_id   INTEGER,
                linked_at     REAL NOT NULL DEFAULT (unixepoch('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_sqal_quest
                ON session_quest_analytics_links(quest_id);
            CREATE INDEX IF NOT EXISTS idx_sqal_playlist
                ON session_quest_analytics_links(playlist_id);

            CREATE TABLE IF NOT EXISTS inventory_items (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                tt_value      REAL NOT NULL,
                markup_paid   REAL NOT NULL,
                notes         TEXT,
                acquired_at   TEXT NOT NULL,
                updated_at    REAL
            );
            CREATE INDEX IF NOT EXISTS idx_inventory_items_name
                ON inventory_items(name);

            CREATE TABLE IF NOT EXISTS ledger_presets (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                type          TEXT NOT NULL,
                description   TEXT NOT NULL,
                amount        REAL NOT NULL,
                tag           TEXT NOT NULL,
                created_at    REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at    REAL
            );

            CREATE TABLE IF NOT EXISTS quest_claims (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                quest_id      INTEGER,
                quest_name    TEXT NOT NULL,
                ped_value     REAL NOT NULL,
                claimed_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_quest_claims_quest
                ON quest_claims(quest_id);
            CREATE INDEX IF NOT EXISTS idx_quest_claims_claimed_at
                ON quest_claims(claimed_at);

            CREATE TABLE IF NOT EXISTS skill_calibrations_archive (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                original_id   INTEGER NOT NULL,
                skill_name    TEXT NOT NULL,
                level         REAL NOT NULL,
                source        TEXT NOT NULL,
                scanned_at    REAL NOT NULL,
                archived_at   REAL NOT NULL DEFAULT (unixepoch('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_skill_cal_arch_name
                ON skill_calibrations_archive(skill_name);
        """)

        # Auto-fill timestamp columns when callers leave them NULL. Created
        # per-trigger via `execute` (not `executescript`) — Python's sqlite3
        # does not reliably handle BEGIN...END blocks with embedded semicolons.
        for table, ts_col in _TIMESTAMPED_TABLES:
            conn.execute(f"""
                CREATE TRIGGER IF NOT EXISTS trg_fill_{ts_col}_{table}
                AFTER INSERT ON {table}
                FOR EACH ROW
                WHEN NEW.{ts_col} IS NULL
                BEGIN
                    UPDATE {table}
                    SET {ts_col} = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END
            """)

        for table in ("inventory_items", "ledger_presets"):
            conn.execute(f"""
                CREATE TRIGGER IF NOT EXISTS trg_fill_updated_at_{table}
                AFTER INSERT ON {table}
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE {table}
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END
            """)

        conn.commit()
