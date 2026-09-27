# Database schema

This page describes the on-disk persistence layer: the application's SQLite
database, how its connections are configured, the shape of its data model, the
forward-only migration mechanism, and the bundled game-data snapshot that lives
outside SQLite entirely.

The authoritative schema is the migration set under
`app/src-tauri/eo-services/migrations/`, applied by the embedded runner in
`app/src-tauri/eo-services/src/db/`. This page stays at the level of tables and
their roles; for a column, read the migration that introduced it.

## Overview

The application persists user-owned data to a single SQLite database, kept under
the application data directory:

| Database | File | Role |
| --- | --- | --- |
| Application database | `entropia_orme.db` | Long-lived, user-owned data: equipment, calibrations, the ledger, codex and quest tracking, recorded hunting sessions, and the derived analytics caches. |

The database runs in write-ahead-logging (WAL) mode. One shared `Db` core owns
one write connection and four reader connections, allowing concurrent reads
alongside the serial writer while the database is still opened exactly once by
the composition root. The rationale is covered in
[ADR 0007: SQLite in WAL mode](../adr/0007-sqlite-wal.md); the services that own
and query the database are catalogued in the [service map](service-map.md).

The game-fact data the application reasons over (weapons, mobs, skills,
professions, and so on) is **not** stored in SQLite. It ships as a bundled,
read-only snapshot loaded from per-endpoint JSON files; see
[Bundled game-data snapshot](#bundled-game-data-snapshot) below.

## Storage configuration

Every core connection is configured identically by `open_configured` in
`eo-services/src/db/pool.rs`. The pragmas are applied as each connection opens;
the write connection is configured before adoption and migration, and readers
open only after the schema is current:

| Pragma | Value | Effect |
| --- | --- | --- |
| `journal_mode` | `WAL` | Write-ahead logging: readers do not block the single writer and the writer does not block readers. |
| `synchronous` | `NORMAL` | Reduced fsync frequency, the standard companion to WAL: durable across application crashes, with a small exposure to a power-loss truncation of the most recent WAL frames. |
| `busy_timeout` | `5000` | Wait up to 5000 ms for a contended lock before raising `SQLITE_BUSY`. |
| `cache_size` | `-64000` | Negative value: a 64 MB page-cache ceiling per connection (SQLite reads a negative `cache_size` as a kibibyte budget rather than a page count). Pages are demand-allocated, so this is a limit rather than an upfront resident cost. |
| `foreign_keys` | `OFF` | Referential enforcement is disabled on the writer and all four readers, so the schema's `REFERENCES` clauses are declarative and services own integrity explicitly. This is the pragma surface the schema was authored against; one consequence is that an overlay write for a session id with no surviving session row must be accepted. |

### Synchronous writer and reader core

`Db` is a narrow closure-based seam over one dedicated writer thread and four
reader threads. Each thread owns its own `rusqlite::Connection`: every mutation
submitted through `Db::with_writer` runs serially on the writer, while
`Db::with_reader` sends a read to whichever reader is free. WAL lets those
readers proceed concurrently with the writer. No raw connection, pool checkout,
or lock ordering escapes the module.

Cloning `Db` shares this one running core rather than opening another owner. The
write connection is opened, adopted, reconciled, and migrated before the reader
threads start, so no reader can observe a pre-migration schema. The composition
root creates the data directory before opening the database.

## The data model

All tables live in `entropia_orme.db`. They fall into three kinds: **user-owned
records** (what the player declared or confirmed), **observed facts** (what the
chat log, the hotbar, and screen captures recorded during play), and **derived
read models** (projections the analytical reads are served from, rebuildable
from the first two).

| Domain | Main tables | What they hold |
| --- | --- | --- |
| Metadata | `db_metadata` | The schema-version row and other key/value markers. |
| Equipment and calibration | `equipment_library`, `skill_calibrations`, `skill_calibrations_archive` | The player's equipment with its economics, and skill-level calibrations from scans. |
| Inventory and stock | `inventory_items`, `stock_movements`, `auction_listings`, `private_sales`, `stock_conversions`, `stock_removals` | Held equipment and the lifecycle of loot after it is recorded: listings, trades, conversions, and removals as signed stock movements over immutable loot. |
| Ledger | `ledger_entries`, `ledger_presets` | Confirmed gains and losses outside tracked activity, and reusable entry presets. |
| Skills and codex | `skill_gains`, `codex_progress`, `codex_claims` | Skill gains from the chat log, and codex rank progress and claims. |
| Quests | `quests`, `quest_families`, `quest_mobs`, `quest_playlists`, `quest_claims`, `quest_runs`, `session_quest_completions`, the reward and review tables | The quest catalogue, runs and completions, and canonical reward accounting ([ADR-0026](../adr/0026-canonical-quest-reward-accounting.md)). |
| Tracking | `session_definitions`, `tracking_sessions`, `session_intervals`, `session_contexts`, `kills`, `kill_tool_stats`, `kill_loot_items`, `harvest_events`, `harvest_loot_items`, `notable_events` | Recorded sessions and their activities: kills, loot, harvesting, and globals. |
| Attribution evidence | the healing, weapon, effect-window, and consumable-dose tables | The observations cost attribution rests on, kept so a correction can re-price them ([ADR-0027](../adr/0027-intent-led-healing-attribution.md), [ADR-0032](../adr/0032-unified-weapon-attribution.md), [ADR-0033](../adr/0033-damage-over-time-effect-windows.md), [ADR-0036](../adr/0036-consumable-dose-lifecycle.md)). |
| Protection | the protection catalogue and limited-item tables | Armour and plates, and costs recorded at session grain when the player repairs ([ADR-0031](../adr/0031-session-grain-protection-costs.md)). |
| Market data | `market_submissions`, `market_observations`, `market_unit_price_observations` | Estimated market data, quarantined from the accounting surfaces ([ADR-0024](../adr/0024-market-informational-layer.md)). |
| Cartography and navigation | `map_views`, `map_pins`, `pin_configs`, `navigation_runs`, `navigation_stops`, `map_pin_visits`, `radar_calibration` | Named pin sets over each planet map, persisted routes, and radar calibration. |
| Derived read models | `session_summaries`, `daily_rollups`, `daily_ledger_rollups`, the `session_*_rollups` tables, and their `*_meta` watermarks | Per-session and per-day projections behind the analytics reads. |

**Derived read models are projections, not sources of truth.** They are
maintained eagerly at the write points, healed lazily on read (a dirty flag, a
version bump, or a watermark walk), and regenerate identically from scratch, so
analytical reads do work proportional to days or sessions rather than to the
whole event history ([ADR-0018](../adr/0018-daily-rollup-read-model.md)).

Referential integrity is owned by the services, not by SQLite (see
`foreign_keys` above), and several `REAL` timestamp columns default to
`unixepoch('now')` or are back-filled by an `AFTER INSERT` trigger.

## Migration mechanism

Schema application is handled by the embedded migration runner (`MIGRATIONS`
in `eo-services/src/db/migrate.rs`) over the migration set in
`eo-services/migrations/`, whose files are compiled into the binary (a unit
test pins the embedded chain to the directory's contents). The set begins with
a version-33 baseline (`0001_schema_baseline.sql`) followed by forward-only
migrations. The runner records applied migrations in the `_sqlx_migrations`
ledger with SHA-384 checksums and never runs a down-migration. Applied rows must
form a contiguous, checksum-identical prefix of the embedded chain; any drift
refuses loudly before anything applies.

Migration files are immutable once they can have been applied. A later schema
refinement always receives the next version, including during development
against a persistent database, so checksum validation stays a corruption and
provenance guard rather than mutable development state
(`migration_checksums_are_immutable` enforces it).

### Open paths: fresh, adoption, and first-launch upgrade

The baseline is the schema as it stood at version 33, written out statement for
statement; the earlier incremental history is folded into it rather than
replayed. On open, `Db::open` configures the connection, then `adopt_or_refuse`
reconciles the on-disk schema with the baseline:

- **Fresh:** an empty (or absent) database gets the baseline applied directly.
- **Adoption:** a database already at version 33 without a migration ledger is
  adopted in place: the baseline is marked applied without re-running any DDL.
- **First-launch upgrade:** a database at version 32 (an installed
  v0.1.0-lineage database) is upgraded to 33 in one transaction and then
  adopted.

A database older than version 32 is declined rather than upgraded, and the
user's file is left untouched.

## Bundled game-data snapshot

The game-fact data the application reasons over (weapons, mobs, skills,
professions, and the rest) ships as a snapshot that is **not** stored in SQLite.
`GameDataStore` (`eo-services/src/game_data_store.rs`) loads it once at startup
from per-endpoint JSON files under
`app/src-tauri/entropia-orme/resources/snapshot/` and serves all queries
from memory. Each file is named for its endpoint (the file stem becomes the
endpoint key); most files hold a JSON list, while `skill_ranks` holds a single
object that the store wraps in a one-element list.

The bundled snapshot files are:

| File | Endpoint |
| --- | --- |
| `absorbers.json` | `absorbers` |
| `enhancers.json` | `enhancers` |
| `harvesting_tools.json` | `harvesting_tools` |
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
user-authored data. In particular, `weapons.json` and
`weapon_amplifiers.json` carry each catalogue entity's `economy.efficiency`,
`economy.max_tt`, and `economy.min_tt`. Equipment and expected-hunting reads
resolve game facts from the bundled catalogue without a runtime Nexus request;
maximum TT, minimum TT, and per-use decay also provide the source basis for a
derived limited-item lifetime when an economic explanation needs it.
