# Database schema reference

This page documents the on-disk persistence layer: the application's SQLite
database, the storage configuration applied to its connections, its tables, the
forward-only migration mechanism, and the bundled game-data snapshot that lives
outside SQLite entirely.

The authoritative schema is the sqlx migration set under
`frontend/src-tauri/eo-services/migrations/`, applied by the migrator in
`frontend/src-tauri/eo-services/src/db.rs`. The set currently holds a single
baseline migration, `0001_schema_baseline.sql`, which creates the complete
schema (tables, indexes, and the timestamp-back-fill triggers) and stamps the
schema-version row. The `Db::open` path opens the database, configures its
session pragmas, adopts or refuses any pre-existing schema, and then runs the
migrator (`MIGRATOR` in `db.rs`).

The column descriptions below are taken from that baseline migration. The
canonical table set is the one it defines.

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

Every connection is configured identically by the connect options assembled in
`Db::open` (`eo-services/src/db.rs`). The pragmas are applied as the connection
is opened, before adoption and the migrator run:

| Pragma | Value | Effect |
| --- | --- | --- |
| `journal_mode` | `WAL` | Write-ahead logging: readers do not block the single writer and the writer does not block readers. |
| `synchronous` | `NORMAL` | Reduced fsync frequency, the standard companion to WAL: durable across application crashes, with a small exposure to a power-loss truncation of the most recent WAL frames. |
| `busy_timeout` | `5000` | Wait up to 5000 ms for a contended lock before raising `SQLITE_BUSY`. |
| `cache_size` | `-8000` | Negative value: an 8 MB page cache (SQLite reads a negative `cache_size` as a kibibyte budget rather than a page count). |
| `foreign_keys` | `OFF` | Referential enforcement is left off, so the schema's `REFERENCES` clauses are declarative; this matches the effective pragma surface the schema was authored against, where an overlay write for a session id with no surviving session row must be accepted. |

### Single-connection model

The handle is a `SqlitePool` capped at a single connection
(`max_connections(1)` in `Db::open`), so every statement serialises on one
underlying connection. Cloning a `Db` shares that one pool rather than opening a
second; the composition root opens the application database exactly once. The
single-writer model is intentional, and relaxing to multiple reader connections
would be a later, benchmark-justified change.

The data directory is created on demand: the connect options set
`create_if_missing(true)`, and the composition root ensures the parent directory
exists before opening.

## Application database tables

All tables described here live in `entropia_orme.db`. The complete schema,
including the tracking tables, is created in one shot by the baseline migration
`0001_schema_baseline.sql`. Several `REAL` timestamp columns default to
`unixepoch('now')`; where a column is instead back-filled by an `AFTER INSERT`
trigger when the caller leaves it `NULL`, that is noted.

### Metadata

#### `db_metadata`

Key/value store for the schema version counter. Created by the baseline
migration; the version row it carries is read by the adoption logic described
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
accounting. This table is shared between the user-data services and the tracking
layer: the baseline migration declares it once, and the tracker writes
shrapnel-conversion entries into it.

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

These tables are created by the baseline migration alongside the rest of the
schema. They carry no separate creation step or version counter of their own:
the single baseline reproduces the complete version-33 surface, the tracking
tables included.

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

Schema application is handled by the sqlx migrator (`MIGRATOR` in
`eo-services/src/db.rs`) over the migration set in
`eo-services/migrations/`. The set carries a single forward migration, the
version-33 baseline (`0001_schema_baseline.sql`); sqlx records applied
migrations in its own `_sqlx_migrations` ledger and never runs a down-migration.
The version the baseline reproduces is pinned in `db.rs` by the
`BASELINE_SCHEMA_VERSION` constant (33).

### The version-33 baseline

The baseline is the schema as it stands at version 33, written out statement for
statement. It creates every table, index, and timestamp-back-fill trigger in one
migration and stamps the `db_metadata` version row to `33`. The version number
is the cumulative result of the schema's earlier evolution; that incremental
history is folded into the single baseline rather than replayed, so a freshly
migrated database lands directly on the current surface.

### Open paths: fresh, adoption, and first-launch upgrade

On open, `Db::open` configures the connection, then calls `adopt_or_refuse`
before running the migrator. This reconciles the on-disk schema with the
baseline and takes one of these paths:

- **Fresh:** an empty (or absent) database is created and the migrator applies
  the baseline directly, landing it at version 33.
- **Adoption:** a database already at version 33 that carries no sqlx ledger is
  adopted in place. The baseline is marked applied (the ledger row is written
  with the baseline's own checksum) without re-running any DDL, and the
  post-adoption migrator run then validates that row.
- **Native first-launch upgrade:** a database at **version 32**, the version an
  installed v0.1.0-lineage database occupies, is upgraded in place by
  `upgrade_to_baseline` (dropping the retired write-only `tt_curve_observations`
  table and bumping the version row to 33) and then adopted, exactly as a fresh
  version-33 database is. The upgrade and the baseline stamp share one
  transaction, so a failure rolls the file back to exactly as it was found.

A database older than version 32 is declined rather than upgraded
(`DbError::UnsupportedSchemaVersion`): no installed database occupies those
versions, so the earlier upgrade steps are deliberately not carried natively.
The user's file is left untouched on a decline, and the composition root
(`Db::open_adopted`) treats a pre-existing-but-unadoptable file as a quarantine
signal rather than a bare error.

## Bundled game-data snapshot

The game-fact data the application reasons over (weapons, mobs, skills,
professions, and the rest) ships as a snapshot that is **not** stored in SQLite.
`GameDataStore` (`eo-services/src/game_data_store.rs`) loads it once at startup
from per-endpoint JSON files under
`frontend/src-tauri/entropia-orme/resources/snapshot/` and serves all queries
from memory. Each file is named for its endpoint (the file stem becomes the
endpoint key); most files hold a JSON list, while `skill_ranks` holds a single
object that the store wraps in a one-element list.

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
