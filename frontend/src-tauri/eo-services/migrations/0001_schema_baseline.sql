-- Schema baseline: the application database at version 33, statement text
-- copied verbatim from the schema the Python backend creates on a fresh
-- install, so a freshly-migrated native database and a freshly-created
-- backend database carry identical sqlite_master definitions (the
-- conformance test asserts this against the live backend).
-- sqlite_sequence is absent deliberately: SQLite creates it on the first
-- AUTOINCREMENT table.

-- table: db_metadata
CREATE TABLE db_metadata (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

-- table: equipment_library
CREATE TABLE equipment_library (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                item_type       TEXT NOT NULL,
                catalog_id      TEXT,
                properties_json TEXT NOT NULL,
                created_at      REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at      REAL
            );

-- table: skill_calibrations
CREATE TABLE skill_calibrations (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_name    TEXT NOT NULL,
                level         REAL NOT NULL,
                source        TEXT NOT NULL,
                scanned_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- index: idx_skill_cal_name
CREATE INDEX idx_skill_cal_name
                ON skill_calibrations(skill_name);

-- index: idx_skill_cal_name_scanned
CREATE INDEX idx_skill_cal_name_scanned
                ON skill_calibrations(skill_name, scanned_at DESC);

-- table: ledger_entries
CREATE TABLE ledger_entries (
                id            TEXT PRIMARY KEY,
                date          TEXT NOT NULL,
                type          TEXT NOT NULL,
                description   TEXT NOT NULL,
                amount        REAL NOT NULL,
                tag           TEXT NOT NULL
            );

-- table: skill_gains
CREATE TABLE skill_gains (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id    TEXT NOT NULL,
                timestamp     REAL NOT NULL,
                skill_name    TEXT NOT NULL,
                amount        REAL NOT NULL,
                ped_value     REAL,
                created_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- index: idx_skill_gains_session
CREATE INDEX idx_skill_gains_session
                ON skill_gains(session_id);

-- index: idx_skill_gains_skill
CREATE INDEX idx_skill_gains_skill
                ON skill_gains(skill_name);

-- index: idx_skill_gains_timestamp
CREATE INDEX idx_skill_gains_timestamp
                ON skill_gains(timestamp);

-- table: codex_progress
CREATE TABLE codex_progress (
                species_name TEXT PRIMARY KEY,
                current_rank INTEGER NOT NULL DEFAULT 0,
                updated_at   REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- table: codex_claims
CREATE TABLE codex_claims (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                species_name   TEXT NOT NULL,
                rank           INTEGER NOT NULL,
                skill_name     TEXT NOT NULL,
                ped_value      REAL NOT NULL,
                claimed_at     REAL NOT NULL DEFAULT (unixepoch('now')),
                kind           TEXT NOT NULL DEFAULT 'rank',
                attribute_name TEXT
            );

-- index: idx_codex_claims_species
CREATE INDEX idx_codex_claims_species
                ON codex_claims(species_name);

-- table: quests
CREATE TABLE quests (
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

-- table: quest_mobs
CREATE TABLE quest_mobs (
                quest_id      INTEGER NOT NULL REFERENCES quests(id),
                mob_name      TEXT NOT NULL,
                PRIMARY KEY (quest_id, mob_name)
            );

-- table: quest_playlists
CREATE TABLE quest_playlists (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                name              TEXT NOT NULL,
                planet            TEXT NOT NULL DEFAULT 'Calypso',
                estimated_minutes INTEGER NOT NULL DEFAULT 30,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at        REAL
            );

-- table: quest_playlist_items
CREATE TABLE quest_playlist_items (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                playlist_id   INTEGER NOT NULL REFERENCES quest_playlists(id),
                quest_id      INTEGER NOT NULL REFERENCES quests(id),
                sort_order    INTEGER NOT NULL DEFAULT 0,
                description   TEXT,
                group_type    TEXT NOT NULL DEFAULT 'immediate',
                updated_at    REAL
            );

-- index: idx_qpi_playlist
CREATE INDEX idx_qpi_playlist
                ON quest_playlist_items(playlist_id);

-- table: session_quest_completions
CREATE TABLE session_quest_completions (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id    TEXT NOT NULL,
                quest_id      INTEGER NOT NULL,
                completed_at  REAL NOT NULL DEFAULT (unixepoch('now')),
                UNIQUE(session_id, quest_id)
            );

-- index: idx_sqc_session
CREATE INDEX idx_sqc_session
                ON session_quest_completions(session_id);

-- index: idx_sqc_quest
CREATE INDEX idx_sqc_quest
                ON session_quest_completions(quest_id);

-- table: session_quest_analytics_links
CREATE TABLE session_quest_analytics_links (
                session_id    TEXT PRIMARY KEY,
                link_type     TEXT NOT NULL,
                quest_id      INTEGER,
                playlist_id   INTEGER,
                linked_at     REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- index: idx_sqal_quest
CREATE INDEX idx_sqal_quest
                ON session_quest_analytics_links(quest_id);

-- index: idx_sqal_playlist
CREATE INDEX idx_sqal_playlist
                ON session_quest_analytics_links(playlist_id);

-- table: inventory_items
CREATE TABLE inventory_items (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                tt_value      REAL NOT NULL,
                markup_paid   REAL NOT NULL,
                notes         TEXT,
                acquired_at   TEXT NOT NULL,
                updated_at    REAL
            );

-- index: idx_inventory_items_name
CREATE INDEX idx_inventory_items_name
                ON inventory_items(name);

-- table: ledger_presets
CREATE TABLE ledger_presets (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                type          TEXT NOT NULL,
                description   TEXT NOT NULL,
                amount        REAL NOT NULL,
                tag           TEXT NOT NULL,
                created_at    REAL NOT NULL DEFAULT (unixepoch('now')),
                updated_at    REAL
            );

-- table: quest_claims
CREATE TABLE quest_claims (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                quest_id      INTEGER,
                quest_name    TEXT NOT NULL,
                ped_value     REAL NOT NULL,
                claimed_at    REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- index: idx_quest_claims_quest
CREATE INDEX idx_quest_claims_quest
                ON quest_claims(quest_id);

-- index: idx_quest_claims_claimed_at
CREATE INDEX idx_quest_claims_claimed_at
                ON quest_claims(claimed_at);

-- table: skill_calibrations_archive
CREATE TABLE skill_calibrations_archive (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                original_id   INTEGER NOT NULL,
                skill_name    TEXT NOT NULL,
                level         REAL NOT NULL,
                source        TEXT NOT NULL,
                scanned_at    REAL NOT NULL,
                archived_at   REAL NOT NULL DEFAULT (unixepoch('now'))
            );

-- index: idx_skill_cal_arch_name
CREATE INDEX idx_skill_cal_arch_name
                ON skill_calibrations_archive(skill_name);

-- trigger: trg_fill_updated_at_equipment_library
CREATE TRIGGER trg_fill_updated_at_equipment_library
                AFTER INSERT ON equipment_library
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE equipment_library
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_updated_at_quests
CREATE TRIGGER trg_fill_updated_at_quests
                AFTER INSERT ON quests
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE quests
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_updated_at_quest_playlists
CREATE TRIGGER trg_fill_updated_at_quest_playlists
                AFTER INSERT ON quest_playlists
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE quest_playlists
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_updated_at_quest_playlist_items
CREATE TRIGGER trg_fill_updated_at_quest_playlist_items
                AFTER INSERT ON quest_playlist_items
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE quest_playlist_items
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_linked_at_session_quest_analytics_links
CREATE TRIGGER trg_fill_linked_at_session_quest_analytics_links
                AFTER INSERT ON session_quest_analytics_links
                FOR EACH ROW
                WHEN NEW.linked_at IS NULL
                BEGIN
                    UPDATE session_quest_analytics_links
                    SET linked_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_updated_at_inventory_items
CREATE TRIGGER trg_fill_updated_at_inventory_items
                AFTER INSERT ON inventory_items
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE inventory_items
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- trigger: trg_fill_updated_at_ledger_presets
CREATE TRIGGER trg_fill_updated_at_ledger_presets
                AFTER INSERT ON ledger_presets
                FOR EACH ROW
                WHEN NEW.updated_at IS NULL
                BEGIN
                    UPDATE ledger_presets
                    SET updated_at = unixepoch('now')
                    WHERE rowid = NEW.rowid;
                END;

-- table: tracking_sessions
CREATE TABLE tracking_sessions (
            id                 TEXT PRIMARY KEY,
            started_at         REAL NOT NULL,
            ended_at           REAL,
            is_active          INTEGER NOT NULL DEFAULT 1,
            armour_cost        REAL DEFAULT 0,
            heal_cost          REAL DEFAULT 0,
            dangling_cost      REAL DEFAULT 0,
            mob_tracking_mode  TEXT NOT NULL DEFAULT 'mob',
            updated_at         REAL
        );

-- trigger: trg_fill_updated_at_tracking_sessions
CREATE TRIGGER trg_fill_updated_at_tracking_sessions
        AFTER INSERT ON tracking_sessions
        FOR EACH ROW
        WHEN NEW.updated_at IS NULL
        BEGIN
            UPDATE tracking_sessions
            SET updated_at = unixepoch('now')
            WHERE rowid = NEW.rowid;
        END;

-- table: kills
CREATE TABLE kills (
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

-- index: idx_kill_session
CREATE INDEX idx_kill_session ON kills(session_id);

-- table: kill_tool_stats
CREATE TABLE kill_tool_stats (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            kill_id         TEXT NOT NULL REFERENCES kills(id),
            tool_name       TEXT NOT NULL,
            shots_fired     INTEGER DEFAULT 0,
            damage_dealt    REAL DEFAULT 0,
            critical_hits   INTEGER DEFAULT 0,
            cost_per_shot   REAL DEFAULT 0,
            UNIQUE(kill_id, tool_name, cost_per_shot)
        );

-- table: kill_loot_items
CREATE TABLE kill_loot_items (
            id                   INTEGER PRIMARY KEY AUTOINCREMENT,
            kill_id              TEXT NOT NULL REFERENCES kills(id),
            item_name            TEXT NOT NULL,
            quantity             INTEGER DEFAULT 1,
            value_ped            REAL NOT NULL,
            is_enhancer_shrapnel INTEGER NOT NULL DEFAULT 0,
            deactivated_at       REAL
        );

-- index: idx_kill_loot_items_kill_id
CREATE INDEX idx_kill_loot_items_kill_id
            ON kill_loot_items(kill_id);

-- table: notable_events
CREATE TABLE notable_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL REFERENCES tracking_sessions(id),
            kill_id     TEXT,
            event_type  TEXT NOT NULL,
            mob_or_item TEXT NOT NULL,
            value_ped   REAL NOT NULL,
            timestamp   REAL NOT NULL
        );

-- index: idx_notable_session
CREATE INDEX idx_notable_session ON notable_events(session_id);

-- table: session_summaries
CREATE TABLE session_summaries (
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

-- The schema-version row the backend writes on a fresh install; kept so
-- both processes read the same metadata during the hybrid.
INSERT OR REPLACE INTO db_metadata (key, value) VALUES ('version', '33');
