# Database schema reference

This page documents the on-disk persistence layer: the application's SQLite
database, the storage configuration applied to its connections, its tables, the
forward-only migration mechanism, and the bundled game-data snapshot that lives
outside SQLite entirely.

The authoritative schema definitions live in three Python modules:

- `backend/db/base.py`: the shared database wrapper (pragmas, the metadata
  table, the version-counter migration loop).
- `backend/db/app_database.py`: the application database, its current schema,
  and its forward migrations.
- `backend/tracking/schema.py`: the tracking tables (sessions, kills, loot).

The column descriptions below are taken from those source files; a seeded
database was introspected to confirm the stored schema version. The canonical
table set is the one the source modules define.

## Overview

The application persists user-owned data to a single SQLite database, kept under
the application data directory:

| Database | File | Role |
| --- | --- | --- |
| Application database | `entropia_orme.db` | Long-lived, user-owned data: equipment, calibrations, the ledger, codex and quest tracking, recorded hunting sessions, and the derived analytics caches. |

The database runs in write-ahead-logging (WAL) mode. The rationale for that
choice (concurrent reads alongside a single writer, with the database opened
once and shared across the in-process request handlers and background worker
threads) is covered in [ADR 0007: SQLite in WAL mode](../adr/0007-sqlite-wal.md). The
services that own and query the database are catalogued in the
[service map](service-map.md).

The game-fact data the application reasons over (weapons, mobs, skills,
professions, and so on) is **not** stored in SQLite. It ships as a bundled,
read-only snapshot loaded from per-endpoint JSON files; see
[Bundled game-data snapshot](#bundled-game-data-snapshot) below.

## Storage configuration

Every connection is configured identically by `BaseDatabase._configure_pragmas`
in `backend/db/base.py`. The pragmas are applied once, immediately after the
connection is opened and before any migration runs:

| Pragma | Value | Effect |
| --- | --- | --- |
| `journal_mode` | `WAL` | Write-ahead logging: readers do not block the single writer and the writer does not block readers. |
| `synchronous` | `NORMAL` | Reduced fsync frequency, the standard companion to WAL: durable across application crashes, with a small exposure to a power-loss truncation of the most recent WAL frames. |
| `busy_timeout` | `5000` | Wait up to 5000 ms for a contended lock before raising `SQLITE_BUSY`. |
| `cache_size` | `-8000` | Negative value: an 8 MB page cache (SQLite reads a negative `cache_size` as a kibibyte budget rather than a page count). |

### Shared-connection model

The connection is opened in `BaseDatabase.__init__` with
`check_same_thread=False` and a `sqlite3.Row` row factory, and is shared across
the request handlers plus background worker threads. SQLite serialises
individual `execute` calls internally, but Python-level multi-step patterns (an
`execute` followed by a `fetchall`, or a batch of writes inside one
transaction) need external serialisation to keep cursor state coherent across
threads. The wrapper therefore exposes a re-entrant lock (`self.lock`, a
`threading.RLock`) that callers take with `with db.lock:` around any compound
operation that may cross threads.

The data directory is created on demand: `__init__` calls
`mkdir(parents=True, exist_ok=True)` on the database file's parent before
connecting.

## Application database tables

All tables described here live in `entropia_orme.db`. The current schema is
created in one shot by `AppDatabase._current_schema` (for fresh installs) in
`backend/db/app_database.py`, except the tracking tables, which are created by
`init_tracking_tables` in `backend/tracking/schema.py` when the tracker first
initialises. Several `REAL` timestamp columns default to `unixepoch('now')`;
where a column is instead back-filled by an `AFTER INSERT` trigger when the
caller leaves it `NULL`, that is noted.

### Metadata

#### `db_metadata`

Key/value store for the schema version counter. Created by
`BaseDatabase._ensure_meta_table`; shared with the migration mechanism described
later.

| Column | Type | Notes |
| --- | --- | --- |
| `key` | TEXT | Primary key. The schema version is stored under the key `version`. |
| `value` | TEXT | Stored as text; the version value is parsed back to an integer on read. |

### User data

#### `equipment_library`

The user's saved equipment definitions (weapons, amplifiers, and other gear),
with type-specific attributes held as a JSON blob.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `name` | TEXT | Not null. |
| `item_type` | TEXT | Not null. |
| `catalog_id` | TEXT | Optional link to a game-data catalogue entry. |
| `properties_json` | TEXT | Not null; type-specific attributes serialised as JSON. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `skill_calibrations`

Calibration points for the skill curve: an observed skill level at a point in
time, attributed to a source.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `skill_name` | TEXT | Not null. Indexed (`idx_skill_cal_name`), and indexed with `scanned_at DESC` (`idx_skill_cal_name_scanned`) for latest-per-skill lookups. |
| `level` | REAL | Not null. |
| `source` | TEXT | Not null. |
| `scanned_at` | REAL | Not null; defaults to `unixepoch('now')`. |

#### `skill_calibrations_archive`

Superseded calibration rows, retained for history when a newer calibration
replaces an earlier one.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `original_id` | INTEGER | Not null; the `id` of the archived `skill_calibrations` row. |
| `skill_name` | TEXT | Not null; indexed (`idx_skill_cal_arch_name`). |
| `level` | REAL | Not null. |
| `source` | TEXT | Not null. |
| `scanned_at` | REAL | Not null; the original scan time, carried over. |
| `archived_at` | REAL | Not null; defaults to `unixepoch('now')`. |

#### `inventory_items`

User-tracked inventory entries with TT value and markup paid.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key (caller-supplied identifier). |
| `name` | TEXT | Not null; indexed (`idx_inventory_items_name`). |
| `tt_value` | REAL | Not null. |
| `markup_paid` | REAL | Not null. |
| `notes` | TEXT | Optional. |
| `acquired_at` | TEXT | Not null; stored as a text date. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

### Ledger

#### `ledger_entries`

The cost/sale ledger: dated, tagged, signed amounts that feed profit-and-loss
accounting. This table is shared between the application database and the
tracking layer; both `backend/db/app_database.py` and
`backend/tracking/schema.py` declare it with the identical shape, and the
tracker writes shrapnel-conversion entries into it.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key (caller-supplied identifier). |
| `date` | TEXT | Not null; stored as a text date. |
| `type` | TEXT | Not null. |
| `description` | TEXT | Not null. |
| `amount` | REAL | Not null; signed. |
| `tag` | TEXT | Not null. |

#### `ledger_presets`

Reusable templates for common ledger entries.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key (caller-supplied identifier). |
| `name` | TEXT | Not null. |
| `type` | TEXT | Not null. |
| `description` | TEXT | Not null. |
| `amount` | REAL | Not null. |
| `tag` | TEXT | Not null. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

### Skill gains

#### `skill_gains`

Individual skill-gain events recorded during a session, optionally valued in
PED.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `session_id` | TEXT | Not null; indexed (`idx_skill_gains_session`). |
| `timestamp` | REAL | Not null; indexed (`idx_skill_gains_timestamp`). |
| `skill_name` | TEXT | Not null; indexed (`idx_skill_gains_skill`). |
| `amount` | REAL | Not null. |
| `ped_value` | REAL | Optional PED valuation of the gain. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |

### Codex

#### `codex_progress`

Current codex rank reached per species.

| Column | Type | Notes |
| --- | --- | --- |
| `species_name` | TEXT | Primary key. |
| `current_rank` | INTEGER | Not null; defaults to 0. |
| `updated_at` | REAL | Not null; defaults to `unixepoch('now')`. |

#### `codex_claims`

A log of codex reward claims (rank rewards and, where applicable, attribute
rewards).

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `species_name` | TEXT | Not null; indexed (`idx_codex_claims_species`). |
| `rank` | INTEGER | Not null. |
| `skill_name` | TEXT | Not null. |
| `ped_value` | REAL | Not null. |
| `claimed_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `kind` | TEXT | Not null; defaults to `'rank'`. |
| `attribute_name` | TEXT | Optional; set for attribute claims. |

### Quests

#### `quests`

Quest definitions, including rewards, chain position, and activation state.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `name` | TEXT | Not null. |
| `planet` | TEXT | Not null; defaults to `'Calypso'`. |
| `waypoint` | TEXT | Optional. |
| `cooldown_hours` | REAL | Optional. |
| `reward_ped` | REAL | Optional. |
| `reward_is_skill` | INTEGER | Not null; defaults to 0 (boolean flag). |
| `expected_reward_markup_percent` | REAL | Optional. |
| `notes` | TEXT | Optional. |
| `chain_name` | TEXT | Optional. |
| `chain_position` | INTEGER | Optional. |
| `chain_total` | INTEGER | Optional. |
| `started_at` | REAL | Optional. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `category` | TEXT | Optional. |
| `reward_description` | TEXT | Optional. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `quest_mobs`

The mobs associated with a quest. Composite-keyed join table.

| Column | Type | Notes |
| --- | --- | --- |
| `quest_id` | INTEGER | Not null; references `quests(id)`. Part of the composite primary key. |
| `mob_name` | TEXT | Not null. Part of the composite primary key. |

#### `quest_playlists`

User-defined ordered collections of quests, with an estimated duration.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `name` | TEXT | Not null. |
| `planet` | TEXT | Not null; defaults to `'Calypso'`. |
| `estimated_minutes` | INTEGER | Not null; defaults to 30. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `quest_playlist_items`

The ordered members of a playlist.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `playlist_id` | INTEGER | Not null; references `quest_playlists(id)`. Indexed (`idx_qpi_playlist`). |
| `quest_id` | INTEGER | Not null; references `quests(id)`. |
| `sort_order` | INTEGER | Not null; defaults to 0. |
| `description` | TEXT | Optional. |
| `group_type` | TEXT | Not null; defaults to `'immediate'`. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `quest_claims`

A log of quest reward claims.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `quest_id` | INTEGER | Optional; indexed (`idx_quest_claims_quest`). Nullable so a claim can survive deletion of its quest definition. |
| `quest_name` | TEXT | Not null. |
| `ped_value` | REAL | Not null. |
| `claimed_at` | REAL | Not null; defaults to `unixepoch('now')`. Indexed (`idx_quest_claims_claimed_at`). |

#### `session_quest_completions`

Records that a given quest was completed during a given session. The
`(session_id, quest_id)` pair is unique.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `session_id` | TEXT | Not null; indexed (`idx_sqc_session`). |
| `quest_id` | INTEGER | Not null; indexed (`idx_sqc_quest`). |
| `completed_at` | REAL | Not null; defaults to `unixepoch('now')`. |

A `UNIQUE(session_id, quest_id)` constraint prevents duplicate completion rows.

#### `session_quest_analytics_links`

Associates a session with a single quest or playlist for analytics attribution.
Keyed by session, so each session has at most one link.

| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | TEXT | Primary key. |
| `link_type` | TEXT | Not null. |
| `quest_id` | INTEGER | Optional; indexed (`idx_sqal_quest`). |
| `playlist_id` | INTEGER | Optional; indexed (`idx_sqal_playlist`). |
| `linked_at` | REAL | Not null; defaults to `unixepoch('now')`. Back-filled by an `AFTER INSERT` trigger when left null. |

### Tracking

These tables are defined in `backend/tracking/schema.py` and created by
`init_tracking_tables` when the tracker first initialises. They have no
migration system of their own: fresh installs land on the canonical schema via
`CREATE TABLE IF NOT EXISTS`, and in-place column additions are applied by the
application database's versioned forward migrations (see below).

#### `tracking_sessions`

One row per recorded hunting session, with session-level cost buckets.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key. |
| `started_at` | REAL | Not null. |
| `ended_at` | REAL | Null while the session is open. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `armour_cost` | REAL | Defaults to 0. |
| `heal_cost` | REAL | Defaults to 0. |
| `dangling_cost` | REAL | Defaults to 0. |
| `mob_tracking_mode` | TEXT | Not null; defaults to `'mob'`. Records the attribution input mode (`'mob'` or `'tag'`); a presentation hint only, since the data semantics are identical. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `kills`

One row per kill, which is also one loot group with accumulated combat stats. A
denormalised `loot_total_ped` is maintained alongside the per-item loot rows so
analytics queries can read the total directly.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed (`idx_kill_session`). |
| `mob_name` | TEXT | Optional. In tag-mode sessions the tag string is persisted here. |
| `mob_species` | TEXT | Defaults to `''`. |
| `mob_maturity` | TEXT | Defaults to `''`. |
| `timestamp` | REAL | Not null. |
| `shots_fired` | INTEGER | Defaults to 0. |
| `damage_dealt` | REAL | Defaults to 0. |
| `damage_taken` | REAL | Defaults to 0. |
| `critical_hits` | INTEGER | Defaults to 0. |
| `cost_ped` | REAL | Defaults to 0. |
| `enhancer_cost` | REAL | Defaults to 0. |
| `loot_total_ped` | REAL | Defaults to 0; denormalised per-kill loot total, mutated atomically with loot-item changes. |
| `is_global` | INTEGER | Defaults to 0 (boolean flag). |
| `is_hof` | INTEGER | Defaults to 0 (boolean flag). |
| `original_mob_name` | TEXT | Null until the session's attributed mob is renamed; preserves the first pre-rename value so a rename can be reverted. |

#### `kill_tool_stats`

Per-tool combat statistics within a single kill. The
`(kill_id, tool_name, cost_per_shot)` triple is unique.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `kill_id` | TEXT | Not null; references `kills(id)`. |
| `tool_name` | TEXT | Not null. |
| `shots_fired` | INTEGER | Defaults to 0. |
| `damage_dealt` | REAL | Defaults to 0. |
| `critical_hits` | INTEGER | Defaults to 0. |
| `cost_per_shot` | REAL | Defaults to 0. |

A `UNIQUE(kill_id, tool_name, cost_per_shot)` constraint keeps one row per
tool-and-cost combination per kill.

#### `kill_loot_items`

The individual loot items dropped by a kill.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `kill_id` | TEXT | Not null; references `kills(id)`. Indexed (`idx_kill_loot_items_kill_id`). |
| `item_name` | TEXT | Not null. |
| `quantity` | INTEGER | Defaults to 1. |
| `value_ped` | REAL | Not null. |
| `is_enhancer_shrapnel` | INTEGER | Not null; defaults to 0 (boolean flag). |
| `deactivated_at` | REAL | Null when active (included in aggregates); a Unix-epoch timestamp when the entry has been deactivated by a post-hoc edit. Recoverable: clearing the timestamp reactivates the entry. |

#### `notable_events`

Notable in-session events (for example globals and Hall-of-Fame drops).

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed (`idx_notable_session`). |
| `kill_id` | TEXT | Optional; the kill the event is associated with. |
| `event_type` | TEXT | Not null. |
| `mob_or_item` | TEXT | Not null. |
| `value_ped` | REAL | Not null. |
| `timestamp` | REAL | Not null. |

### Derived caches

#### `session_summaries`

Per-session aggregates used by the character/prospect analytics path. This is a
derived cache rather than a source of truth: a row is filled when a session ends
and is lazily rebuilt on read if missing.

| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | TEXT | Primary key. |
| `summary_version` | INTEGER | Not null; defaults to 1. Cache-format version for invalidation. |
| `started_at` | REAL | Not null. |
| `ended_at` | REAL | Not null. |
| `duration_hours` | REAL | Not null. |
| `kills` | INTEGER | Not null. |
| `loot_tt` | REAL | Not null. |
| `weapon_cost` | REAL | Not null. |
| `enhancer_cost` | REAL | Not null. |
| `armour_cost` | REAL | Not null. |
| `heal_cost` | REAL | Not null. |
| `dangling_cost` | REAL | Not null. |
| `cycled_ped` | REAL | Not null. |
| `regular_skill_ped_json` | TEXT | Not null; per-skill PED breakdown serialised as JSON. |
| `attribute_levels_json` | TEXT | Not null; per-attribute level breakdown serialised as JSON. |
| `regular_skill_tt` | REAL | Not null. |
| `attribute_levels_total` | REAL | Not null. |
| `dominant_mob` | TEXT | Optional. |
| `dominant_tag` | TEXT | Optional. |
| `dominant_weapon` | TEXT | Optional. |
| `computed_at` | REAL | Not null; defaults to `unixepoch('now')`. |

## Migration mechanism

Schema evolution is handled by a forward-only version counter in `BaseDatabase`
(`backend/db/base.py`).

### How the counter works

1. On construction, the wrapper ensures the `db_metadata` table exists, then
   reads the integer stored under the `version` key (treating a missing row as
   version 0).
2. If the stored version is below the subclass's `DB_VERSION`, the wrapper calls
   `_migrate(from_version)` and then stamps `DB_VERSION` into `db_metadata`.
3. If the stored version already equals (or exceeds) `DB_VERSION`, nothing runs.

Migrations only ever move forward; there is no down-migration path.

### Application database migrations

`AppDatabase` (`backend/db/app_database.py`) sets `DB_VERSION = 33`, the current
schema version. Its `_migrate` method branches on the starting version:

- **From version 0 (fresh install):** the entire current schema is created in
  one step by `_current_schema`, which also defines the timestamp-back-fill
  triggers. No incremental steps run.
- **Below the supported floor (less than 28):** migration is refused with a
  `RuntimeError`. Rather than stamp the current version over a schema that was
  never actually migrated, the wrapper instructs the user to rebuild the
  database from scratch or restore a backup matching the current version.
  **Version 28 is therefore the oldest in-place-upgradable schema.**
- **From version 28 up to 33:** the relevant incremental steps run in order. As
  landed, these steps are:

  | Step | Change |
  | --- | --- |
  | v29 | Drops the unused `profession_calibrations` and `profession_calibrations_archive` tables. |
  | v30 | Adds the nullable `deactivated_at` column to `kill_loot_items`. |
  | v31 | Adds the nullable `original_mob_name` column to `kills`. |
  | v32 | Adds the `mob_tracking_mode` column (`NOT NULL DEFAULT 'mob'`) to `tracking_sessions`. |
  | v33 | Drops the unused `tt_curve_observations` table. |

The three column-adding steps (v30, v31, v32) target tables owned by the
tracking schema. They are defensive in two directions: if the target table does
not yet exist (a prior install that never started tracking), the step logs and
skips, because `init_tracking_tables` will later create the table with the
column already baked in; and if the column already exists (a partial earlier
run), the duplicate-column error is caught so the step is idempotent.

### Tracking-schema versioning

The tracking tables have no version counter of their own. Fresh installs land
on the canonical schema through `CREATE TABLE IF NOT EXISTS` in
`init_tracking_tables`, and in-place column additions to existing tracking
tables are carried by the application database's versioned forward migrations
described above. This is why migrations that add columns to `kills`,
`kill_loot_items`, and `tracking_sessions` live in `app_database.py` rather than
in `tracking/schema.py`.

### Shipped adoption model (the native runtime)

The ladder above is the testing oracle's. The shipped application is the single
Rust binary, whose persistence layer (`eo-services/src/db.rs`) does not re-run
the historical ladder. It instead pins a single **baseline schema at version
33** (the migration under `eo-services/migrations/`), reproduced statement for
statement so a freshly created database is identical to the schema the ladder
produces at version 33. On open it takes one of three paths:

- **Fresh:** an empty database is created directly at the version-33 baseline.
- **Adoption:** a database already at version 33 is adopted in place, marking the
  baseline applied without re-running any DDL.
- **Native first-launch upgrade:** a database at **version 32**, the version
  every installed v0.1.0-lineage database occupies, is upgraded in place to the
  version-33 baseline (dropping the retired write-only `tt_curve_observations`
  table) and then adopted, exactly as a fresh version-33 database is.

A database older than version 32 is declined rather than upgraded: no installed
database occupies those versions, so the earlier ladder steps are deliberately
not reproduced natively. The user's file is left untouched on a decline.

## Bundled game-data snapshot

The game-fact data the application reasons over (weapons, mobs, skills,
professions, and the rest) ships as a snapshot that is **not** stored in SQLite.
`GameDataStore` (`backend/services/game_data_store.py`) loads it once at startup
from per-endpoint JSON files under `backend/data/snapshot/` and serves all
queries from memory. Each file is named for its endpoint (the file stem becomes
the endpoint key); most files hold a JSON list, while `skill_ranks` holds a
single object that the store wraps in a one-element list.

The bundled snapshot files are:

| File | Endpoint |
| --- | --- |
| `absorbers.json` | `absorbers` |
| `enhancers.json` | `enhancers` |
| `medical_tools.json` | `medical_tools` |
| `mobs.json` | `mobs` |
| `professions.json` | `professions` |
| `skill_ranks.json` | `skill_ranks` (single object) |
| `skills.json` | `skills` |
| `stimulants.json` | `stimulants` |
| `weapon_amplifiers.json` | `weapon_amplifiers` |
| `weapon_vision_attachments.json` | `weapon_vision_attachments` |
| `weapons.json` | `weapons` |

This JSON snapshot is the read-only, in-memory source of truth for game facts.
It is a maintained static asset that ships with the build and holds no
user-authored data.
