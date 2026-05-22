"""Tracking schema — sessions, kills, loot.

Fresh installs land directly on the canonical schema via ``_current_schema``.
Each kill = one loot group with accumulated combat stats.
"""

from __future__ import annotations

import sqlite3


def init_tracking_tables(conn: sqlite3.Connection) -> None:
    """Create tracking tables if they don't exist."""
    _current_schema(conn)
    conn.commit()


def _current_schema(conn: sqlite3.Connection) -> None:
    conn.executescript("""
        CREATE TABLE IF NOT EXISTS tracking_sessions (
            id             TEXT PRIMARY KEY,
            started_at     REAL NOT NULL,
            ended_at       REAL,
            is_active      INTEGER NOT NULL DEFAULT 1,
            armour_cost    REAL DEFAULT 0,
            heal_cost      REAL DEFAULT 0,
            dangling_cost  REAL DEFAULT 0,
            updated_at     REAL
        );

        CREATE TRIGGER IF NOT EXISTS trg_fill_updated_at_tracking_sessions
        AFTER INSERT ON tracking_sessions
        FOR EACH ROW
        WHEN NEW.updated_at IS NULL
        BEGIN
            UPDATE tracking_sessions
            SET updated_at = unixepoch('now')
            WHERE rowid = NEW.rowid;
        END;

        -- `original_mob_name` preserves the pre-edit `mob_name` value
        -- when the user mass-renames a session's attributed mob via the
        -- sessions-tab metadata-edit affordance. NULL = never renamed;
        -- populated = renamed at least once, and the inverse restore
        -- endpoint can clear `original_mob_name` while reverting
        -- `mob_name` to it. COALESCE on the rename write keeps the
        -- *first* original across N consecutive renames so undo always
        -- lands at the genuinely-original capture.
        CREATE TABLE IF NOT EXISTS kills (
            id                 TEXT PRIMARY KEY,
            session_id         TEXT NOT NULL REFERENCES tracking_sessions(id),
            mob_name           TEXT,
            mob_species        TEXT DEFAULT '',
            mob_maturity       TEXT DEFAULT '',
            timestamp          REAL NOT NULL,
            shots_fired        INTEGER DEFAULT 0,
            damage_dealt       REAL DEFAULT 0,
            damage_taken       REAL DEFAULT 0,
            critical_hits      INTEGER DEFAULT 0,
            cost_ped           REAL DEFAULT 0,
            enhancer_cost      REAL DEFAULT 0,
            loot_total_ped     REAL DEFAULT 0,
            is_global          INTEGER DEFAULT 0,
            is_hof             INTEGER DEFAULT 0,
            original_mob_name  TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_kill_session ON kills(session_id);

        CREATE TABLE IF NOT EXISTS kill_tool_stats (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            kill_id         TEXT NOT NULL REFERENCES kills(id),
            tool_name       TEXT NOT NULL,
            shots_fired     INTEGER DEFAULT 0,
            damage_dealt    REAL DEFAULT 0,
            critical_hits   INTEGER DEFAULT 0,
            cost_per_shot   REAL DEFAULT 0,
            UNIQUE(kill_id, tool_name, cost_per_shot)
        );

        -- `deactivated_at` is a nullable Unix-epoch timestamp; NULL = active
        -- (included in aggregates), populated = deactivated at that moment by
        -- a post-hoc edit on the sessions tab. Deactivation is recoverable:
        -- clearing the timestamp reactivates the entry. The denormalised
        -- per-kill total `kills.loot_total_ped` is mutated atomically
        -- alongside the flag, so analytics queries reading `kills.loot_total_ped`
        -- need no filter clause and stay untouched by this affordance.
        CREATE TABLE IF NOT EXISTS kill_loot_items (
            id                   INTEGER PRIMARY KEY AUTOINCREMENT,
            kill_id              TEXT NOT NULL REFERENCES kills(id),
            item_name            TEXT NOT NULL,
            quantity             INTEGER DEFAULT 1,
            value_ped            REAL NOT NULL,
            is_enhancer_shrapnel INTEGER NOT NULL DEFAULT 0,
            deactivated_at       REAL
        );
        CREATE INDEX IF NOT EXISTS idx_kill_loot_items_kill_id
            ON kill_loot_items(kill_id);

        CREATE TABLE IF NOT EXISTS notable_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL REFERENCES tracking_sessions(id),
            kill_id     TEXT,
            event_type  TEXT NOT NULL,
            mob_or_item TEXT NOT NULL,
            value_ped   REAL NOT NULL,
            timestamp   REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_notable_session ON notable_events(session_id);

        -- Ledger table (shared with app_database; tracker writes shrapnel conversion entries).
        CREATE TABLE IF NOT EXISTS ledger_entries (
            id          TEXT PRIMARY KEY,
            date        TEXT NOT NULL,
            type        TEXT NOT NULL,
            description TEXT NOT NULL,
            amount      REAL NOT NULL,
            tag         TEXT NOT NULL
        );

        -- Per-session aggregates for the Character → Prospect path.
        -- Derived cache (not a source of truth); filled when a session ends or
        -- lazily rebuilt on read when a row is missing.
        CREATE TABLE IF NOT EXISTS session_summaries (
            session_id             TEXT PRIMARY KEY,
            summary_version        INTEGER NOT NULL DEFAULT 1,
            started_at             REAL NOT NULL,
            ended_at               REAL NOT NULL,
            duration_hours         REAL NOT NULL,
            kills                  INTEGER NOT NULL,
            loot_tt                REAL NOT NULL,
            weapon_cost            REAL NOT NULL,
            enhancer_cost          REAL NOT NULL,
            armour_cost            REAL NOT NULL,
            heal_cost              REAL NOT NULL,
            dangling_cost          REAL NOT NULL,
            cycled_ped             REAL NOT NULL,
            regular_skill_ped_json TEXT NOT NULL,
            attribute_levels_json  TEXT NOT NULL,
            regular_skill_tt       REAL NOT NULL,
            attribute_levels_total REAL NOT NULL,
            dominant_mob           TEXT,
            dominant_tag           TEXT,
            dominant_weapon        TEXT,
            computed_at            REAL NOT NULL DEFAULT (unixepoch('now'))
        );
    """)
