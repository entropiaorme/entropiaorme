# Database schema reference

This page documents the on-disk persistence layer: the application's SQLite
database, the storage configuration applied to its connections, its tables, the
forward-only migration mechanism, and the bundled game-data snapshot that lives
outside SQLite entirely.

The authoritative schema is the migration set under
`app/src-tauri/eo-services/migrations/`, applied by the embedded runner in
`app/src-tauri/eo-services/src/db/`. The set holds a version-33 baseline
migration, `0001_schema_baseline.sql`, which creates the complete base schema
(tables, indexes, and the timestamp-back-fill triggers) and stamps the
schema-version row, followed by forward-only migrations:
`0002_analytical_indexes.sql` (analytical read-path indexes plus a one-time
`ANALYZE`), `0003_session_summary_read_columns.sql` (extra
`session_summaries` columns for the Activity and session-list reads),
`0004_daily_rollups.sql` (the per-day analytics rollup projection behind the
Overview, plus a ledger date index), `0005_market_observations.sql` (the
market-markup observation feed), `0006_harvest_events.sql` (the
harvesting activity tables plus the harvest columns on the summary and
rollup projections), `0007_map_pins.sql` (the cartography pins),
`0008_map_views.sql` (independent named pin sets over each planet map),
`0009_map_navigation.sql` (persisted routes, stop progress, pin visits, radar
calibration, and cartography spatial indexes),
`0010_navigation_runtime_fields.sql` (the last-position timestamp and
flow-scoped route hotkey), `0011_pin_configs.sql` (the per-preset pin
palette, with each placed pin referencing one configuration),
`0012_harvest_stock_removed.sql` (an interim per-item current-stock overlay,
since retired), `0013_harvest_yield_tier.sql` (durable Tree Cutting yield
activity plus its historical backfill), `0014_auction_sales.sql` (the auction
listing lifecycle, stock conversions, and the signed stock-movement ledger
that replaced the overlay above, whose rows it carries across before dropping
it), `0015_stock_movement_tool.sql` (the tool a movement's stock was produced
with, recorded but not currently reported on), `0016_stock_opening_balance.sql` (a movement kind for stock an outflow
proves was held but that was never recorded, rebuilding the movement table
because SQLite cannot widen a CHECK constraint in place), and
`0017_undone_entries.sql` (the marker that keeps an undone listing or
conversion on file with its effects reversed),
`0018_session_facets.sql` (co-recordable session name and skill-boost facets,
plus mob-stamp provenance), `0019_session_intervals.sql` (the interval and
context spine for duration, cost, and event attribution),
`0020_signal_quests.sql` (loot-signal completion for quests without a mission-log
lifecycle), `0021_quest_families.sql` (variant families with shared,
anchor-aware cooldown), `0022_session_definitions.sql` (authored session
families, activity rosters, and instance references),
`0023_default_session_definition.sql` (the protected fallback definition), and
`0024_hunting_stock_provenance.sql` (the mob-species provenance dimension on
the stock-movement ledger, widening the source vocabulary to hunted loot and
rebuilding the table because SQLite cannot widen a CHECK constraint in place,
plus the activity-family stamp on stock conversions), and
`0025_loot_item_name_indexes.sql` (partial item-name indexes over active loot
rows on both loot tables, serving the per-item position arithmetic and the
DISTINCT item universes without full-table scans), and
`0026_session_activity_rollups.sql` (the per-session activity rollup
projection and its settlement marker; see "Derived caches"), and
`0027_hunting_definition_provenance.sql` (the nullable session-definition
context on stock movements, plus its item/species/definition index, allowing
one confirmed Hunting sale to be projected by both observed species and the
repeatable session that produced it without duplicating the sale), and
`0028_quest_reward_provenance.sql` (immutable completion-time reward and
activity-context provenance), `0029_session_context_loot_rollups.sql`
(the context-grain loot rollup used by Hunting activity composition), and
`0030_quest_reward_items.sql` (actual reward-item evidence captured at quest
completion), and `0031_stock_outcomes.sql` (private trades, stock-only
removals, deliberate Shrapnel conversion gains, and the corresponding
provenance-aware movement kinds), and `0032_inventory_hub.sql` (the equipment
holding lifecycle and the shared loot/equipment market-listing identity), and
`0033_listing_duration.sql` plus `0034_listing_instant.sql` (how long a listing
runs and the instant it started, from which its expiry is derived), and
`0035_quest_reward_kinds.sql` (the canonical economic treatment of each quest
reward, kept separate from its evidence provenance), and
`0036_mixed_quest_reward_kinds.sql` (preservation of a primary liquid or
progression treatment when item evidence is also present),
`0037_typed_quest_rewards.sql` (independent typed completion triggers and
reward policies plus immutable confirmed, none, or unresolved outcomes),
`0038_quest_runs.sql` (durable quest runs and their declared effort intervals),
`0039_market_unit_prices.sql` (informational absolute PED-per-unit quotes), and
`0040_quest_reward_reviews.sql` (append-only adjudication of ambiguous reward
evidence with exact ordinary-loot reclassification), and
`0041_ARIS_unresolved_rewards.sql` (truthful unresolved outcomes for guarded
historical ARIS completions that had no exact voucher attribution), and
`0042_quest_run_ownership.sql` (one-to-one ownership between durable runs and
their completions), and `0043_protection_accounting.sql` (the armour and plate
catalogue, composable protection loadouts, live identity intervals, limited-item
TT observations and reconciliations, and defensive-event evidence),
`0044_deferred_protection_costs.sql` (evidence-window settlement and
session/context allocation for deferred protection costs), and
`0045_manual_quest_hand_in.sql` (manual-hand-in quest completion, stable raw
loot-clump identity, and bounded review evidence), and
`0046_quest_reward_lifecycle.sql` (Universal Ammo ledger recognition, quest
stock provenance and effort attribution, append-only reward corrections, and
the quest-item daily-rollup family), and
`0047_healing_attribution.sql` (paid healing activations, restoration-effect
windows, and classified healing-output evidence),
`0048_expected_hunting_evidence.sql` (model-neutral offensive evidence on each
kill tool phase), `0049_expected_hunting_phase_identity.sql` (distinct
same-name, same-cost phases when their captured loadout evidence differs),
`0052_protection_hit_allocation.sql` (equal-hit protection allocation and the
historical reweighting of settled evidence), and
`0053_session_protection_policy.sql` (definition-authored, session-stamped
segment-protection policy), `0054_session_armour_cost_policy.sql` (the
parent armour-cost policy and whole-session default), and
`0055_session_grain_protection_costs.sql` (each protection recording's stream
position, the retirement of unlimited sets and loadouts, and the session
lookups the recording surface reads), and
`0056_protection_recording_undo.sql` (when a protection recording or reading
was undone), and `0057_healing_corrections.sql` (post-play healing corrections,
their supersession and exact-undo provenance, and the effect-window expiry
read). The
`Db::open` path opens the write connection, configures its session pragmas,
adopts or refuses any pre-existing schema, reconciles baseline-column drift,
runs the embedded chain (`MIGRATIONS` in `eo-services/src/db/migrate.rs`), and
only then starts the reader connections.

The column descriptions below reflect the schema after the full migration set,
save for the stock and auction tables migrations `0014` onward introduce,
which are described by the migrations themselves. The baseline establishes the
version-33 table set; later migrations extend it with further tables, columns,
indexes, and rebuildable read-model fields noted in place.

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

## Application database tables

All tables described here live in `entropia_orme.db`. The version-33 base schema
is created by `0001_schema_baseline.sql`; the forward migration tail extends it
to the current surface. Several `REAL` timestamp columns default to
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

User-tracked capital-equipment positions with TT value and markup paid. A row
survives its market lifecycle so the acquisition basis remains available to
History and correction flows after it leaves current holdings.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key (caller-supplied identifier). |
| `name` | TEXT | Not null; indexed (`idx_inventory_items_name`). |
| `tt_value` | REAL | Not null. |
| `markup_paid` | REAL | Not null. |
| `notes` | TEXT | Optional. |
| `acquired_at` | TEXT | Not null; stored as a text date. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |
| `state` | TEXT | `held`, `listed`, or `sold`; defaults to `held`. |
| `disposed_at` | TEXT | Sale date for a disposed position; null while held or listed. |

A `listed` position returns to `held` when its auction expires. It moves to
`sold` only when an auction sale or immediate player trade is confirmed.

### Ledger

#### `ledger_entries`

The cost/sale ledger: dated, tagged, signed amounts that feed profit-and-loss
accounting. This table is shared between the user-data services and the tracking
layer: the baseline migration declares it once, and the tracker writes
shrapnel-conversion entries into it.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key (caller-supplied identifier). |
| `date` | TEXT | Not null; stored as a text date. Indexed (`idx_ledger_entries_date`, migration `0004`) for the windowed analytics reads and the rollup heal's stray-key sweep. |
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
| `context_id` | INTEGER | Optional reference to `session_contexts(id)` (migration `0019`; indexed `idx_skill_gains_context`). Null means the event predates context capture or was written outside one, not that no interval was active. |
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
| `claimed_at` | REAL | Not null; defaults to `unixepoch('now')`. Indexed (`idx_codex_claims_claimed_at`). |
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
| `reward_ped` | REAL | Optional PES amount when `reward_is_skill = 1`. Legacy non-skill values remain readable but the active service neither authors nor accounts them. |
| `reward_is_skill` | INTEGER | Not null; defaults to 0 (boolean flag). Selects whether `reward_ped` is PES progression. |
| `expected_reward_markup_percent` | REAL | Dormant legacy field. Observed reward evidence and current market projections replace authored reward estimates. |
| `notes` | TEXT | Optional. |
| `chain_name` | TEXT | Optional. |
| `chain_position` | INTEGER | Optional. |
| `chain_total` | INTEGER | Optional. |
| `started_at` | REAL | Optional. |
| `signal_loot_item` | TEXT | Optional (migration `0020`). Null uses the mission-log lifecycle; a value names the loot item whose arrival completes a signal-driven quest. |
| `completion_trigger` | TEXT | Legacy two-value trigger retained for migration compatibility. Current service reads and writes use `completion_mode`. |
| `completion_mode` | TEXT | Canonical completion trigger: `mission_log`, `signal_item`, or `manual_hand_in`; independent of the reward policy. |
| `reward_policy` | TEXT | Active policies are `none`, `fixed_pes`, `named_items`, and `completion_clump`. The stored `fixed_ped` value is legacy and normalises to `none`; Universal Ammo is recognised from observed item evidence instead. |
| `family_id` | INTEGER | Optional reference to `quest_families(id)` (migration `0021`; indexed `idx_quests_family`). Null leaves the quest standalone. |
| `cooldown_anchor` | TEXT | Not null; defaults to `'completion'` (migration `0021`). Selects whether this quest's cooldown runs from pickup or completion. |
| `last_started_at` | REAL | Optional durable pickup timestamp (migration `0021`), retained after `started_at` clears so pickup-anchored cooldown remains measurable. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `category` | TEXT | Optional. |
| `reward_description` | TEXT | Optional. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### `quest_families`

Variant quests that occupy one shared repeatable slot (migration `0021`). A
family groups the variants for availability while each member remains a
separate quest for recording and analysis. Starting or completing one member,
according to the family's cooldown anchor, cools the family as a unit.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `name` | TEXT | Not null. |
| `planet` | TEXT | Not null; defaults to `'Calypso'`. |
| `cooldown_hours` | REAL | Optional. Null groups variants without gating availability. |
| `cooldown_anchor` | TEXT | Not null; defaults to `'pickup'`. The service accepts `pickup` or `completion`. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `updated_at` | REAL | Optional update timestamp. |

#### `quest_mobs`

The mobs associated with a quest. Composite-keyed join table.

| Column | Type | Notes |
| --- | --- | --- |
| `quest_id` | INTEGER | Not null; references `quests(id)`. Part of the composite primary key. |
| `mob_name` | TEXT | Not null. Part of the composite primary key. |

#### `quest_playlists`

Dormant legacy storage for the retired playlist feature. The active product,
command facade, and quest service never read or write these rows. Session
definition rosters are the sole authored quest collection. The table remains
to preserve old databases without a lossy migration.

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

Dormant legacy playlist membership, retained only for non-lossy historical
storage. It has no active service or UI consumer.

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
| `activity_context_id` | INTEGER | Optional reference to `session_contexts(id)`. The exact declared activity signature in force immediately before completion; indexed when present. |
| `activity_interval_id` | INTEGER | Optional reference to `session_intervals(id)`. The declared quest stretch that earned the completion. |
| `reward_source` | TEXT | Nullable for legacy completions; active writes use `none`, `tracked_loot`, or `skill`. The stored `ledger` value is legacy. |
| `reward_kind` | TEXT | Nullable for unclassified legacy completions; active writes use `none`, `included_in_loot`, `item`, or `skill`. Stored `fixed_liquid` rows are legacy. Economic treatment for observed items lives on each reward-item row. |
| `reward_ped` | REAL | Optional immutable PES value when `reward_kind = 'skill'`. Active quest completion never writes a direct liquid-PED reward. |
| `expected_reward_markup_percent` | REAL | Optional legacy completion-time snapshot. Retained for compatibility; Hunting projections do not consume it. |
| `ledger_entry_id` | TEXT | Optional reference to the exact liquid reward row in `ledger_entries`. |
| `quest_claim_id` | INTEGER | Optional reference to the exact progression reward row in `quest_claims`. |
| `reward_outcome` | TEXT | Immutable capture result: `confirmed`, `none`, or `unresolved`. |
| `reward_policy_snapshot` | TEXT | The authored policy at completion time. |
| `reward_unresolved_reason` | TEXT | Optional explanation for ambiguous or missing evidence. |
| `reward_evidence_json` | TEXT | Optional exact completion-tick evidence used by the review surface. |
| `quest_run_id` | INTEGER | Optional reference to the durable run that completion closed. |

A `UNIQUE(session_id, quest_id)` constraint prevents duplicate completion rows.

#### `session_quest_completion_reward_items`

Immutable item evidence observed as part of a quest reward. Hunting projects
these items through current market data outside the accounting aggregate; an
item without usable market data remains at its recorded TT value.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `completion_id` | INTEGER | Not null; references `session_quest_completions(id)` with cascade deletion and is indexed (`idx_sqc_reward_items_completion`). |
| `item_name` | TEXT | Not null; the observed item name. |
| `quantity` | INTEGER | Not null and positive; the observed quantity. |
| `value_ped` | REAL | Not null and non-negative; the completion-time TT value. |
| `accounting_kind` | TEXT | `liquid` for Universal Ammo, otherwise `stock`. Liquid rows enter the ledger and never Inventory; stock rows are Inventory acquisitions. |
| `ledger_entry_id` | TEXT | Optional reference to the exact Universal Ammo gain in `ledger_entries`. Null for stock rewards. |

#### `quest_reward_attributions`

Immutable ownership of a confirmed reward across the exact activity contexts
traversed by its durable run. Rows store the context and session-definition
identity, a conserved weight, and whether cycle PED or elapsed duration supplied
the weight. A run with neither measurable basis remains quest-attributable but
does not invent activity ownership.

#### `quest_reward_reversals`

Append-only economic corrections. A reversal retains the completion and reward
evidence, removes stock acquisitions from current holdings, and references any
compensating Universal Ammo ledger expense or negative PES claim. A reversal is
refused while a dependent stock listing, sale, conversion, or removal exists.

#### `quest_cooldown_resets`

Append-only cooldown corrections, deliberately separate from reward reversal.
Availability ignores a reset completion while its historical run, evidence,
Inventory acquisition, and accounting remain intact.

#### `quest_reward_item_rules`

Quest-local item names that identify a `named_items` reward. The composite
primary key is `(quest_id, item_name)`; `sort_order` preserves authoring order.

#### `quest_reward_clumps` and `quest_reward_clump_items`

A bounded raw-evidence journal for loot clumps observed while a manual-hand-in
quest stretch is active. Each clump carries the stable source identity shared
with its ordinary kill or harvest record, session and context ownership,
observation time, and an optional unique completion claim. Its ordered item rows
preserve every raw chat-log item not already assigned to an overlapping signal
quest, including names excluded from ordinary loot tracking. Unclaimed evidence
is retention-bounded; a claimed clump remains as durable provenance for its
immutable completion reward.

#### `quest_runs` and `quest_run_intervals`

`quest_runs` records one administrative lifecycle as `in_progress`, `completed`,
or `cancelled`, with start/completion times and its completion id. Partial
unique indexes permit only one in-progress run per quest and ensure a run and
completion own at most one another; reciprocal-pair triggers reject crossed
links once either side has been bound. `quest_run_intervals` links every declared
effort interval that earned the run, including intervals from several sessions.
For manual hand-in, `hand_in_waiting` and `hand_in_after_clump_id` persist the
next-clump capture boundary. A partial unique index permits only one waiting run
at a time, matching the single overlay flow.

#### `quest_reward_reviews` and `quest_reward_review_items`

Append-only decisions over originally unresolved completion evidence. A review
records `confirmed` or `none`. Confirmed stock selections reference and
deactivate the exact ordinary loot acquisitions they reclassified, and each
acquisition can be claimed only once. Filtered Universal Ammo is the narrow
exception: its immutable completion evidence is the acquisition source, so it
creates liquid ledger value without a review-item or Inventory row. The
original completion evidence remains unchanged.

#### `session_quest_analytics_links`

Dormant legacy association storage. Recorded quest intervals and session
definition rosters replaced this link ontology; no active command or analytics
reader consumes it. The table remains for non-lossy historical storage.

| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | TEXT | Primary key. |
| `link_type` | TEXT | Not null. |
| `quest_id` | INTEGER | Optional; indexed (`idx_sqal_quest`). |
| `playlist_id` | INTEGER | Optional; indexed (`idx_sqal_playlist`). |
| `linked_at` | REAL | Not null; defaults to `unixepoch('now')`. Back-filled by an `AFTER INSERT` trigger when left null. |

### Tracking

The baseline creates the original session and event tables. Migrations `0018`
through `0023` add co-recordable facets, stamped activity contexts, authored
session definitions, and the definition roster.

#### `session_definitions`

Authored activity families that tracked sessions can be instances of
(migrations `0022` and `0023`). Archive and Restore toggle `is_active`; they do
not delete the definition, its roster, or its recorded instances. The seeded
protected fallback guarantees an active choice without making historical
`tracking_sessions.definition_id` non-null.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `name` | TEXT | Not null. Active names are enforced case-insensitively by the service. |
| `ad_hoc_segments` | INTEGER | Not null; defaults to 0. Opts the definition into naming segments during play. |
| `track_protection_costs` | INTEGER | Not null; defaults to 1 (migration `0054`). Controls whether the session records defensive evidence and offers armour-cost accounting at all. |
| `track_protection_by_segment` | INTEGER | Not null; introduced by migration `0053`. Retired (ADR-0031): new and edited definitions write 0 and nothing reads it. |
| `is_active` | INTEGER | Not null; defaults to 1. Archived definitions retain 0. |
| `is_protected` | INTEGER | Not null; defaults to 0 (migration `0023`). Protected definitions cannot be archived. |
| `created_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `updated_at` | REAL | Optional update timestamp. |

Migration `0023` inserts `Default Tracking` when no active definition already
has that name, then protects the active row that does. The settings file owns
the current selection, so the migration does not backfill it into SQLite.

#### `session_definition_roster`

The ordered activities authored for one session definition (migration `0022`).
The service replaces a definition's roster wholesale on update. `kind` selects
how to interpret the nullable fields: `quest_family` and `quest` use `ref_id`;
`segment` uses `label`.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `definition_id` | INTEGER | Not null; references `session_definitions(id)`. Indexed with `idx_session_definition_roster_definition`. |
| `position` | INTEGER | Not null; stable authored or promotion order. |
| `kind` | TEXT | Not null; `quest_family`, `quest`, or `segment` in the service vocabulary. |
| `ref_id` | INTEGER | Optional domain reference selected by `kind`. |
| `label` | TEXT | Optional segment label. |

#### `tracking_sessions`

One row per recorded session, with session-level cost buckets, its stamped
facets, and an optional session-definition identity.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key. |
| `started_at` | REAL | Not null; indexed (`idx_tracking_sessions_started_at`). |
| `ended_at` | REAL | Null while the session is open. |
| `is_active` | INTEGER | Not null; defaults to 1. |
| `armour_cost` | REAL | Defaults to 0. |
| `heal_cost` | REAL | Defaults to 0. |
| `dangling_cost` | REAL | Defaults to 0. |
| `mob_tracking_mode` | TEXT | Not null; defaults to `'mob'`. Records the attribution input mode (`'mob'` or `'tag'`); a presentation hint only, since the data semantics are identical. |
| `session_name` | TEXT | Optional designated session-name stamp (migration `0018`). It remains the recorded name even if an attached definition is later renamed. |
| `skill_boost_percent` | INTEGER | Optional positive boost declaration (migration `0018`). Null means not captured. |
| `definition_id` | INTEGER | Optional reference to `session_definitions(id)` (migration `0022`; indexed `idx_tracking_sessions_definition`). Null is valid for legacy or deliberately unattached sessions. |
| `track_protection_costs` | INTEGER | Not null; defaults to 1 (migration `0054`). Immutable policy stamped from the selected definition at session start. When 0, the session records no defensive evidence, so no protection recording can reach it. |
| `track_protection_by_segment` | INTEGER | Not null; migration `0053`. Retired (ADR-0031): new sessions record 0 and nothing reads it. |
| `updated_at` | REAL | Back-filled by an `AFTER INSERT` trigger when left null. |

#### Healing attribution evidence

Migration `0047` separates a paid healing activation from the healing outputs
the game reports. This is the durable boundary that prevents lifesteal and
restoration ticks from being charged as repeated tool uses.

`healing_activations` records one cost-bearing use after a healing hotbar intent
is confirmed by a compatible chat output. It retains the equipment identity,
tool name, operating-system intent time, local observation time, raw chat
timestamp, immutable healing-profile JSON, attributed session context, charged
PED, and whether confirmation was direct, health-capped, or reconciled after
delivery-order inversion. It is indexed by session and observation time.

`healing_effect_windows` records the bounded over-time effect created by a
confirmed activation. Each row names its parent activation, session, equipment,
tool, time bounds, optional tick interval and cadence, and attribution context.
Effect outputs within that window remain evidence but never create another
cost. It is indexed by session and start time, and by expiry (migration
`0057`): the persisted absolute expiry is the truth for a running effect, so
every session start reads back each live window whose expiry is still ahead,
whichever session paid for it. A heal-over-time effect therefore keeps
explaining its ticks across a new session or an application restart, while
its cost stays with the activation that paid for it.

`healing_outputs` retains every positive self-heal line observed during a
session. A row can reference the activation and effect window that explain it,
and otherwise remains classified as `passive` or `unattributed`. It records the
local observation time separately from the game's whole-second chat timestamp,
plus the amount and an explanatory reason. A direct or compound profile's
confirming output is `direct`; a pure over-time profile's first matching output
is `effect` and confirms its activation. Subsequent `effect` outputs and every
`passive` or `unattributed` output are zero-cost evidence. It is indexed by
session and observation time.

Migration `0057` makes this evidence correctable after a session ends, without
deleting any of it. `healing_corrections` records each correction: its session,
its kind (`not_paid_use` takes a billed activation back; `paid_use` bills an
uncosted output as one use of a chosen healing item), the activation it
superseded or minted, the output it started from, its signed cost delta, when
it was made, and when it was undone (null while it stands). It is indexed by
session and correction time.

The evidence tables gain the columns corrections need:

| Table | Column | Notes |
| --- | --- | --- |
| `healing_activations` | `superseded_at` | When a correction took the activation back, or when the undo of the correction that minted it did. A superseded activation costs nothing and stays as provenance. |
| `healing_activations` | `confirming_output_id` | The output that confirmed the activation. Back-filled for existing rows from the output written with it, or the earlier output a delayed hotbar occurrence reconciled. |
| `healing_activations` | `correction_id` | The `paid_use` correction that minted the activation, if any. Such an activation keeps the checked provenance `direct` and is reported as corrected. |
| `healing_effect_windows` | `superseded_at` | Set with its activation's; a superseded window is never read back. |
| `healing_outputs` | `correction_id` | The live correction that last moved the output. Indexed. |
| `healing_outputs` | `prior_classification`, `prior_activation_id`, `prior_effect_window_id`, `prior_reason` | What the output was before that correction moved it, so its undo restores it exactly. |

`healing_outputs` is also indexed by activation. Each output and activation is
moved by at most one live correction, and only an ended session is corrected:
every correction and undo moves `tracking_sessions.heal_cost` by its delta and
repairs the session summary, daily rollup, and settled session cells in the
same transaction, so a session's heal cost stays equal to its live
activations' costs (plus any legacy aggregate from before migration `0047`).

Foreign-key enforcement remains disabled for the application database, so
session deletion removes these four tables explicitly in correction, output,
window, activation order before deleting the session.

#### `session_intervals`

The authoritative duration and cost stretches inside a session (migration
`0019`). Quests, segments, modifiers, and future kinds share this open-vocabulary
primitive. Interval timestamps are wall-clock bounds and are never compared to
event timestamps for attribution.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed with `kind`; open intervals also have the partial `idx_session_intervals_open` index. |
| `kind` | TEXT | Not null; intentionally open vocabulary interpreted by the service. |
| `label` | TEXT | Optional display name. |
| `ref_id` | INTEGER | Optional domain reference whose table is implied by `kind`. |
| `magnitude` | REAL | Optional magnitude. Zero is meaningful; null means this interval kind carries none. |
| `started_at` | REAL | Not null; wall-clock start. |
| `ended_at` | REAL | Optional wall-clock end; null while open. |
| `origin_device` | TEXT | Optional origin-device identifier. |

#### Protection catalogue and limited-item accounting

Protection costs are recorded at session grain when the player repairs or
scans (ADR-0031). Nothing about protection is declared during play: the
tracker records defensive hits, and each recording is spread over the sessions
the player ticks when recording it. There are two kinds of cost stream. The
pooled unlimited stream records confirmed Repair Terminal totals at raw TT.
Each limited armour or plate set is its own stream, measured through
successive Trade Terminal TT-value observations at its frozen markup.

`protection_sets` owns the limited-set catalogue:

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `kind` | TEXT | Not null; `armour` or `plates`. |
| `name` | TEXT | Not null. Active names are unique case-insensitively within a kind. |
| `economy_kind` | TEXT | Not null; `limited` or `unlimited`. New sets are always `limited`. Migration `0055` archived every `unlimited` row, releasing its name; those rows are history from before the pooled stream and are no longer read as sets. |
| `markup_percent` | REAL | Required and at least 100 for limited sets; null for unlimited sets. The approximate average acquisition basis across the set, frozen once the set has a reading. |
| `created_at` | REAL | Not null. |
| `archived_at` | REAL | Optional archive stamp. |

`protection_loadouts`, the `protection_state` singleton, and
`session_protection_intervals` (the snapshot beside each
`session_intervals(kind = 'protection')` row) are retired history from the
declared-loadout model of migration `0043`. Nothing writes them any more;
migration `0055` archived every loadout, and their rows stay readable.

`protection_observations` records one confirmed total TT reading of one
limited set. Its client token is unique, making confirmation idempotent.
`source` is `ocr` or `manual`; `raw_text` preserves the recognised text when
present. A non-null `reset_reason` establishes a new baseline instead of
claiming decay, which is required when a reading rises, and books nothing.
Each observation also stores `defence_event_cursor`, the latest defensive-event
identifier visible at confirmation. That durable cursor is the limited
stream's position: the next reading of the set covers only hits after it.
`superseded_at` (migration `0056`) marks a reading that was undone; the set's
baseline is its latest reading without one.

`protection_reconciliations` preserves the initial single-session
implementation and is no longer written; `protection_cost_windows` supersedes
it.

`protection_cost_windows` is the authoritative record of every protection
cost. A `limited_decay` window references the set, its opening and closing
observations, the frozen markup, TT consumed, and the effective PED cost. A
`repair` window records a pooled unlimited repair; `armour_set_id` and
`plate_set_id` are set only on history from before the pool. Each window
carries an idempotency token where the interaction can be repeated, a `booked`
or `pending` status (`pending` with the reason "Not attributed to any session"
when no session was ticked), and `evidence_cursor` (migration `0055`): the
latest defensive-event identifier the recording covered. The latest repair
window's cursor is the unlimited stream's position. Migration `0055`
backfilled the cursor for history from each limited window's closing
observation, and for each repair window from the hits it claimed and every
session that had ended before it.

`superseded_at` (migration `0056`) marks an undone recording. Only the latest
live recording of a stream can be undone: for the unlimited stream its newest
repair window, for a limited set the window its latest reading closed. Undoing
never deletes. The window (and, for a limited set, its closing reading) is
marked, its allocation rows stay as provenance, and each session it reached
gives its share back to `armour_cost`, with its summary and daily rollup
repaired, in the same transaction. Every read of a stream's position, of which
sessions a recording covers, and of unrecorded play skips superseded rows, so
the stream falls back to its previous recording and the next one offers the
same sessions again.

`protection_cost_allocations` stores the conserved per-session split and
`protection_cost_context_allocations` the finer split over each session's
immutable activity contexts. Both carry `hit_count` (migration `0052`): every
defensive event is one equal-weight hit, whether the game reported numeric
damage or a deflection. A recording's cost is split across its ticked
sessions by hit count, then within each session across its contexts by hit
count, and the last context takes the rounding residual so the stored rows sum
to the recorded cost exactly. Each allocated session's `armour_cost`, summary,
and daily rollup are repaired in the same transaction. A session with
defensive hits and no allocation row from a live recording reads as having no
protection cost recorded yet.

`protection_cost_evidence` holds the per-hit claims written by the
declared-loadout model. Recordings no longer write it: which hits a recording
covers follows from its sessions and the stream positions.

`protection_defence_events` retains each numeric damage-taken event and each
deflection, stamped with its session and attribution context. Deflection
deliberately has no invented damage amount. `protection_interval_id` is set
only on history from the declared-loadout model. Migration `0055` indexes the
events by session and identifier for the recording surface's reads.

#### `session_contexts`

Immutable attribution snapshots minted whenever the set of active intervals
changes (migration `0019`). Every economically relevant event stamps the current
context when written, avoiding invalid comparisons between wall-clock interval
bounds and the game's server-time event stamps.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed with `idx_session_contexts_session`. |
| `created_at` | REAL | Not null; wall-clock creation time. |

#### `session_context_intervals`

The many-to-many membership of intervals in an attribution context (migration
`0019`). A context with no rows records declared-none; an event with no
`context_id` instead predates context capture or was written outside it.

| Column | Type | Notes |
| --- | --- | --- |
| `context_id` | INTEGER | Not null; references `session_contexts(id)`. Part of the composite primary key. |
| `interval_id` | INTEGER | Not null; references `session_intervals(id)`. Part of the composite primary key and indexed with `idx_session_context_intervals_interval`. |

#### `kills`

One row per kill, which is also one loot group with accumulated combat stats. A
denormalised `loot_total_ped` is maintained alongside the per-item loot rows so
analytics queries can read the total directly.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key. |
| `loot_source_id` | TEXT | Optional stable raw-clump identity (migration `0045`), unique when present. Live watcher loot uses it for exact manual reward reclassification; legacy and synthetic records may be null. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed (`idx_kill_session`). |
| `mob_name` | TEXT | Optional. In tag-mode sessions the tag string is persisted here. |
| `mob_species` | TEXT | Defaults to `''`. |
| `mob_maturity` | TEXT | Defaults to `''`. |
| `timestamp` | REAL | Not null; indexed (`idx_kills_timestamp`) for the analytics time-window reads. |
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
| `mob_stamp_source` | TEXT | Optional (migration `0018`); `declared` for the player's current mob choice, `detected` reserved for automatic detection. Null means the provenance was not captured. |
| `context_id` | INTEGER | Optional reference to `session_contexts(id)` (migration `0019`; indexed `idx_kills_context`). Null is unknown or outside context capture, not declared-none. |

#### `kill_tool_stats`

Per-tool combat statistics within a single kill. The
`(kill_id, tool_name, cost_per_shot, evidence_fingerprint)` tuple is unique.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `kill_id` | TEXT | Not null; references `kills(id)`. |
| `tool_name` | TEXT | Not null. |
| `shots_fired` | INTEGER | Defaults to 0. |
| `damage_dealt` | REAL | Defaults to 0. |
| `critical_hits` | INTEGER | Defaults to 0. |
| `cost_per_shot` | REAL | Defaults to 0. |
| `expected_economics_json` | TEXT | Optional model-neutral weapon/amplifier evidence captured for this offensive phase (migration `0048`): catalogue identity, raw TT per use, consumed limited-item premium, component Efficiency, the three session-start hunting-looter levels, and their selected source. Null marks legacy or unsupported evidence rather than zero Efficiency. |
| `evidence_fingerprint` | TEXT | Not null; defaults to `''` (migration `0049`). The serialised evidence for current rows, used only as phase identity so economically distinct loadouts cannot collapse when name and cost happen to match. |

A `UNIQUE(kill_id, tool_name, cost_per_shot, evidence_fingerprint)` constraint
keeps one row per distinct tool, cost, and evidence combination per kill. A covering index
`idx_kill_tool_stats_covering(kill_id, cost_per_shot, shots_fired, tool_name)`
(migration `0002`) carries `shots_fired` and `tool_name` alongside the join key,
so the weapon-cost aggregate resolves from the index without a per-row table
fetch.

#### `kill_loot_items`

The individual loot items dropped by a kill.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `kill_id` | TEXT | Not null; references `kills(id)`. Indexed (`idx_kill_loot_items_kill_id`). |
| `item_name` | TEXT | Not null. Partially indexed over active rows (`idx_kill_loot_items_item_active`, migration `0025`) for per-item position reads and the DISTINCT item universe. |
| `quantity` | INTEGER | Defaults to 1. |
| `value_ped` | REAL | Not null. |
| `is_enhancer_shrapnel` | INTEGER | Not null; defaults to 0 (boolean flag). |
| `deactivated_at` | REAL | Null when active (included in aggregates); a Unix-epoch timestamp when the entry has been deactivated by a post-hoc edit. Recoverable: clearing the timestamp reactivates the entry. |

#### `harvest_events`

One row per harvesting (tree cutting) swing, the second tracked activity
beside hunting (migration `0006`). A successful swing arrives as a wood loot
group; a failed swing arrives as the explicit "Harvest attempt failed" chat
line, so every swing is directly counted rather than inferred. The tool
identity and per-swing cost are captured at swing time, so later equipment
edits cannot rewrite history; `tool_name` is null when no harvesting tool was
known and the swing recorded at zero cost. The effective yield tier is stored
independently: board evidence classifies the swing as short, long, or huge,
while a boardless swing may inherit a tier from direct board evidence on the same session and tool within 30 seconds. Live attribution takes the nearest preceding evidence in the same keypress; the historical backfill, and any later restamp when fresh direct evidence arrives, compare both temporal sides and infer only when one side carries evidence or the two agree. Direct evidence is never overwritten. Unsupported
evidence remains explicitly unknown.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | TEXT | Primary key. |
| `loot_source_id` | TEXT | Optional stable raw-clump identity (migration `0045`), unique when present across harvesting rows. Failed swings and legacy records may be null. |
| `session_id` | TEXT | Not null; references `tracking_sessions(id)`. Indexed alone (`idx_harvest_session`) and with tool and timestamp (`idx_harvest_session_tool_time`). |
| `timestamp` | REAL | Not null. Indexed with yield tier and tool (`idx_harvest_time_tier_tool`) for period-scoped analytics. |
| `success` | INTEGER | Not null; defaults to 1 (boolean flag). |
| `tool_name` | TEXT | Optional; the harvesting tool equipped via the hotbar at swing time. |
| `yield_tier` | TEXT | Not null; `short`, `long`, `huge`, or `unknown` (migration `0013`). This is the effective board yield, not a physical-tree observation. |
| `yield_tier_source` | TEXT | Optional; `board` when the swing's own loot named the class, or `inferred` when it was taken from adjacent direct evidence under the bound above. Null when the tier remains unknown. |
| `cost_ped` | REAL | Defaults to 0; the tool's per-use decay (markup-weighted) at swing time. |
| `loot_total_ped` | REAL | Defaults to 0; denormalised per-swing loot total beside the per-item rows. |
| `context_id` | INTEGER | Optional reference to `session_contexts(id)` (migration `0019`; indexed `idx_harvest_events_context`). Null is unknown or outside context capture, not declared-none. |

#### `harvest_loot_items`

The individual wood items a successful swing dropped.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `harvest_id` | TEXT | Not null; references `harvest_events(id)`. Indexed (`idx_harvest_loot_items_harvest`). |
| `item_name` | TEXT | Not null. Partially indexed over active rows (`idx_harvest_loot_items_item_active`, migration `0025`) for per-item position reads and the DISTINCT item universe. |
| `quantity` | INTEGER | Defaults to 1. |
| `value_ped` | REAL | Defaults to 0. |
| `deactivated_at` | REAL | Null when active; mirrors `kill_loot_items` so the loot-edit flow can extend to harvest loot without a schema move. |

#### `harvest_stock_removed` (retired)

An interim per-item overlay of quantity already removed from the recorded
harvest position (migration `0012`). Migration `0014` retired it: its rows were
carried into `stock_movements` as explicitly unattributed adjustments and the
table was dropped, so current position derives from recorded loot plus that
signed movement ledger rather than from two sources of the same quantity. No
current database carries it.

### Activity stock outcomes

Recorded loot remains the immutable acquisition base. Current stock is that
loot plus the signed rows in `stock_movements`; auction listings, conversions,
private trades, and removals own the lifecycle records those movements refer
to. A stock action never edits the loot that originally established an
activity's TT return.

- `auction_listings` records the pending, sold, or expired market lifecycle.
  `subject_kind` distinguishes fungible loot from a whole equipment position;
  equipment rows point to `inventory_item_id` and retain their acquisition
  `cost_basis`. For equipment, `channel` distinguishes auction listings from
  immediate player trades recorded on this same whole-position lifecycle.
  Stock leaves at listing time, the listing fee is spent then, and markup is
  realised only when the final sale is confirmed. `auction_days` (migration
  `0033`) records how long the listing was posted for and `listed_instant`
  (migration `0034`) the moment it started; both are nullable, because
  a listing made before durations were recorded has no deadline and inventing
  one would fabricate a decision the player never made. The expiry date is
  derived from `listed_instant` plus the duration rather than stored, so it
  cannot drift from either, and it carries a time of day because the auction
  clock does: a listing posted at 18:20 for seven days ends at 18:20.
- `private_sales` records a completed loot trade with its quantity, TT,
  tracked and untracked shares, final price, date, and owned ledger entry. It
  has no auction fees and recognises markup atomically with the stock outflow.
- `stock_conversions` records source and target stock. Ordinary Nanocube
  recycling preserves TT; deliberate Shrapnel conversion records its 101%
  conversion and owns only the 1% ledger gain. The consumed Shrapnel leaves
  stock; Universal Ammo never becomes an Inventory position.
- `stock_removals` records that a quantity is no longer held when its outcome
  is unknown. It has no ledger effect, so historical loot TT remains intact.
- `stock_movements` is the signed, provenance-aware inventory ledger. Its
  source kind carries Hunting independently of its optional observed species;
  harvesting tier/tool and Hunting species/session definition remain
  downstream analytical dimensions through transformations and into realised
  outcomes. Hunting loot also retains the exact declared activity context when
  one was captured, so a later sale or conversion can attribute realised
  markup to that activity rather than only to the session or species. Quest
  stock additionally carries reward-item, durable-run, quest,
  activity-context, and session-definition identity. Missing mob or context
  evidence therefore limits target detail without suppressing stock or broader
  activity attribution.

Every outcome can be undone as a correction while retaining its lifecycle row
marked as undone. A conversion undo is refused if later movements have already
consumed what it produced.

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

### Market data

The manually-fed market-markup layer: the user pastes the game's auction-house
market-ledger export (a tab-separated table of per-item markup and sales
volume over five aggregation horizons), and each accepted paste is stored as
one submission with its per-item, per-horizon observations. This layer is
informational only: estimated markup never contributes to the ledger, the
analytics aggregates, or any realised profit-and-loss figure (a boundary the
`market-isolation` CI guard enforces mechanically).

#### `market_submissions`

One accepted paste.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `submitted_at` | REAL | Not null; Unix-epoch seconds. |
| `source` | TEXT | Not null; `paste` (room for future feeds). |
| `item_count` | INTEGER | Not null; the number of items the paste carried. |

#### `market_observations`

The per-item, per-horizon readings of a submission (five rows per item, one
per aggregation horizon).

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `submission_id` | INTEGER | Not null; references `market_submissions(id)`. Indexed (`idx_market_observations_submission`). |
| `item_name` | TEXT | Not null. Indexed with `horizon` and `submission_id` (`idx_market_observations_item`). |
| `tier` | INTEGER | Not null; the item tier the export reported. |
| `horizon` | TEXT | Not null; one of `day`, `week`, `month`, `year`, `decade`. |
| `markup_pct` | REAL | Null where the export reported `N/A` (no sales in that horizon). |
| `sales_ped` | REAL | Not null; TT turnover over the horizon, normalised to PED. |

#### `market_unit_price_observations`

Manual informational PED-per-unit quotes for zero-TT or unit-priced items.
Each row carries the item name, non-negative unit price, observation time, and
source. Reward MU multiplies a captured quantity by the latest unit quote;
these rows never enter ledger or realised accounting.

### Cartography

#### `map_views`

User-named pin sets over a bundled planet map (migration `0008`). The
permanent Default set is represented by a null `map_view_id` on `map_pins`,
so it needs no row and every pin created before this migration remains on
Default. Names are unique per planet without regard to case. Deleting a
named map and its pins is an explicit transaction in `map_pins`, because
foreign-key enforcement is intentionally disabled for this database.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `planet` | TEXT | Not null; the bundled planet map this pin set overlays. Indexed with `created_at` and `id` (`idx_map_views_planet`). |
| `name` | TEXT | Not null, case-insensitive; unique with `planet`. |
| `created_at` | REAL | Not null; Unix-epoch seconds. |

#### `map_pins`

User-authored pins on the bundled planet maps (migration `0007`): a named,
icon-carrying location, either an exact point or an area of a given radius.
Coordinates are game units on the game's global tile grid; the write path
refuses coordinates outside the selected planet's calibrated map bounds, so
an implausible pin is never stored. `kind` and `icon` are user-shaped
presentation vocabulary (the pin palette is user-configured), deliberately
open text rather than a closed set.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `planet` | TEXT | Not null; the bundled map the pin sits on. Indexed (`idx_map_pins_planet`), indexed with `map_view_id` and `created_at` (`idx_map_pins_planet_view`), and indexed with map identity plus coordinates (`idx_map_pins_spatial`) for viewport and proximity reads. |
| `lon` | REAL | Not null; game units (longitude grows eastward). |
| `lat` | REAL | Not null; game units (latitude grows northward). |
| `altitude` | REAL | Optional; carried when the source read included one. |
| `name` | TEXT | Not null. |
| `icon` | TEXT | Not null; the pin's icon identifier. |
| `kind` | TEXT | Not null; the user-shaped pin category. |
| `radius_m` | REAL | Null for an exact point; an area pin's radius in metres. |
| `notes` | TEXT | Optional free text. |
| `session_id` | TEXT | Optional; references `tracking_sessions(id)`, the session the pin was dropped during. |
| `map_view_id` | INTEGER | Optional; references `map_views(id)`. Null means the permanent Default map. |
| `pin_config_id` | INTEGER | Optional; references `pin_configs(id)` (migration `0011`, indexed `idx_map_pins_config`). The palette configuration the pin instantiates: its colour, category, and special behaviour derive from that row, so `name`/`icon`/`radius_m` are a snapshot at drop time. Deleting the configuration deletes its pins (explicit cascade; foreign keys are disabled). |
| `created_at` | REAL | Not null; Unix-epoch seconds. |

#### `pin_configs`

The per-preset pin palette (migration `0011`): each row is a *type* of pin
scoped to one `(planet, map_view_id)` preset, and placed pins are instances of
it. Editing a configuration restyles its placed pins; deleting one removes them.
`category` is `generic` (no behaviour) or `special`; the only special `kind` so
far is `tree`, which carries a distinct on-cooldown colour and is the sole pin
kind the navigation router treats as a route stop.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `planet` | TEXT | Not null; the bundled map this palette entry belongs to. Indexed with `map_view_id` and `ordinal` (`idx_pin_configs_scope`). |
| `map_view_id` | INTEGER | Optional; references `map_views(id)`. Null is the permanent Default map. |
| `label` | TEXT | Not null; the palette label, snapshotted onto placed pins as their name. |
| `category` | TEXT | Not null; `generic` or `special` (checked). |
| `special_kind` | TEXT | Null for generic; `tree` for the special tree kind (checked). |
| `icon` | TEXT | Not null; the palette emoji. |
| `radius_m` | REAL | Null for an exact point; an area radius in metres. |
| `colour` | TEXT | Not null; the generic colour, or a tree's available colour (hex). |
| `cooldown_colour` | TEXT | Null for generic; a tree's on-cooldown colour (hex). |
| `ordinal` | INTEGER | Not null; palette display order within the preset. |
| `created_at` | REAL | Not null; Unix-epoch seconds. |

### Map navigation

#### `navigation_runs`

One persisted traversal over a planet and named map. A partial unique index
(`idx_navigation_one_live_run`) permits at most one `active` or `paused` run.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `planet` | TEXT | Not null; the bundled planet map. |
| `map_view_id` | INTEGER | Optional reference to `map_views(id)`; null selects Default. |
| `status` | TEXT | One of `active`, `paused`, `completed`, or `ended`. |
| `start_lon`, `start_lat` | REAL | The route's fixed starting position. |
| `current_lon`, `current_lat` | REAL | The most recently captured position. |
| `last_position_at` | REAL | Optional Unix-epoch time of the latest manual, hotkey, or harvesting coordinate sample. |
| `hop_count` | INTEGER | Not null; number of stops admitted to the route. |
| `hotkey` | TEXT | Not null; the F6 through F12 position-update key selected for this run. |
| `created_at`, `updated_at` | REAL | Unix-epoch seconds. |

#### `navigation_stops`

The ordered pins selected for one run. The run and pin pair and the run and
ordinal pair are both unique. `idx_navigation_stops_run_status` serves the
active and pending traversal reads.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `run_id`, `pin_id` | INTEGER | Not-null references to the run and map pin. |
| `ordinal` | INTEGER | Stable route order within the run. |
| `status` | TEXT | One of `pending`, `active`, `visited`, or `skipped`. |
| `completed_at` | REAL | Optional Unix-epoch completion time. |
| `completion_source` | TEXT | Optional source such as `manual` or `harvest`. |
| `observed_lon`, `observed_lat` | REAL | Optional captured completion position. |
| `observed_distance` | REAL | Optional Euclidean distance from the pin. |

#### `map_pin_visits`

Append-only visit evidence for manual and harvesting-driven completion.
`idx_map_pin_visits_pin_time` serves newest-first history for a pin.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key, autoincrement. |
| `pin_id` | INTEGER | Not-null reference to `map_pins(id)`. |
| `run_id` | INTEGER | Optional reference to the navigation run. |
| `visited_at` | REAL | Not-null Unix-epoch time. |
| `source`, `outcome` | TEXT | Not-null operational provenance. |
| `observed_lon`, `observed_lat`, `observed_distance` | REAL | Not-null capture and match evidence. |

#### `radar_calibration`

A singleton physical-screen calibration. The centre and north-edge points
define a circular radar and its northbound bearing frame.

| Column | Type | Notes |
| --- | --- | --- |
| `singleton` | INTEGER | Primary key constrained to `1`. |
| `centre_x`, `centre_y` | INTEGER | Physical-screen centre coordinates. |
| `north_x`, `north_y` | INTEGER | Physical-screen north-edge coordinates. |
| `radius_px` | REAL | Not null and at least eight physical pixels. |
| `display_scale` | REAL | Positive display scale, currently `1.0`. |
| `updated_at` | REAL | Not-null Unix-epoch time. |

### Derived caches

#### `session_summaries`

Per-session aggregates that back the character/prospect path and, since the
read-model work, the Activity and session-list reads as well. This is a derived
cache rather than a source of truth: a row is filled when a session ends and is
lazily rebuilt on read when missing or below the current `summary_version`.
The steady-state healer prequalifies a missing row from indexed session-owned
cost evidence before running the full summary computation. Ended sessions with
no positive duration or cycled cost therefore remain correctly absent without
repeating raw-fact aggregation on every analytical read; a stale-version row is
still recomputed unconditionally so version invalidation cannot be skipped.

The base columns are created by the baseline; the read columns below the
`computed_at` row are added by migration `0003`, the harvest columns by
migration `0006`, and the session facets by migration `0018`. Each extension
is healed in by a version bump (the code's `SUMMARY_VERSION` is 4).

| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | TEXT | Primary key. |
| `summary_version` | INTEGER | Not null; column default 1. Cache-format version for invalidation; the code writes the current `SUMMARY_VERSION` and rebuilds any lower-versioned row on read. |
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
| `dominant_mob_kills` | INTEGER | Not null; defaults to 0 (migration `0003`). The dominant mob's kill count, summed by the Activity read. |
| `dominant_tag_kills` | INTEGER | Not null; defaults to 0 (migration `0003`). The dominant tag's kill count. |
| `activity_skill_tt` | REAL | Not null; defaults to 0 (migration `0003`). The raw session `SUM(ped_value)` the Activity read uses (distinct from the per-skill `regular_skill_tt`). |
| `primary_mobs_json` | TEXT | Not null; defaults to `'[]'` (migration `0003`). The session-list top-three mob names as a JSON array. |
| `primary_weapons_json` | TEXT | Not null; defaults to `'[]'` (migration `0003`). The session-list top-three weapon names as a JSON array. |
| `globals` | INTEGER | Not null; defaults to 0 (migration `0003`). The session's global count. |
| `hofs` | INTEGER | Not null; defaults to 0 (migration `0003`). The session's Hall-of-Fame count. |
| `harvest_swings` | INTEGER | Defaults to 0 (migration `0006`). Harvesting swings, successes plus explicit fails. |
| `harvest_successes` | INTEGER | Defaults to 0 (migration `0006`). |
| `harvest_loot_tt` | REAL | Defaults to 0 (migration `0006`). Wood loot TT; also included in `loot_tt`. |
| `harvest_cost` | REAL | Defaults to 0 (migration `0006`). Swing decay; also included in `cycled_ped`. |
| `session_name` | TEXT | Optional designated session-name stamp copied from `tracking_sessions` (migration `0018`). |
| `skill_boost_percent` | INTEGER | Optional positive boost declaration copied from `tracking_sessions` (migration `0018`). |

#### `daily_rollups`

Per-UTC-day sums of every aggregate family the analytics Overview and its
breakdowns read (migration `0004`; see
[ADR-0018](../adr/0018-daily-rollup-read-model.md)). Like `session_summaries`,
this is a rebuildable projection over the raw tracking tables: rows are
maintained eagerly at the write points, healed lazily on read (dirty flag,
version bump, and the watermark walk described under `daily_rollup_meta`), and
regenerate identically from scratch.

Column nullability is load-bearing: each family column stores the raw per-day
`SUM` verbatim, `NULL` when the day had no contributing rows, so aggregates over
rollups reproduce the engine typing the raw queries put on the wire. `has_rows`
carries the one distinction `NULL` sums erase: whether any source rows existed
that day at all, which decides the day's membership in the timeline and monthly
point sets.

| Column | Type | Notes |
| --- | --- | --- |
| `day` | TEXT | Primary key; ISO `YYYY-MM-DD`, UTC. Ledger dates that do not name a canonical calendar day get rows keyed by their literal text. |
| `rollup_version` | INTEGER | Not null. Projection-format version; a below-version row heals on the next read. |
| `dirty` | INTEGER | Not null; defaults to 0. Set inside a writing transaction when a backdated mutation touches the day; the eager recompute clears it, and the next heal repairs a crash between the two. |
| `has_rows` | INTEGER | Not null; defaults to 0. Day-membership bit (see above). |
| `loot_tt` | REAL | Nullable; `SUM(kills.loot_total_ped)` for the day. |
| `weapon_cost` | REAL | Nullable; `SUM(cost_per_shot * shots_fired)` over the day's kill tool stats. |
| `enhancer_cost` | REAL | Nullable; `SUM(kills.enhancer_cost)`. |
| `armour_cost` | REAL | Nullable; `SUM(tracking_sessions.armour_cost)` by session start day. |
| `heal_cost` | REAL | Nullable; `SUM(tracking_sessions.heal_cost)`. |
| `dangling_cost` | REAL | Nullable; `SUM(tracking_sessions.dangling_cost)`. |
| `skill_tt` | REAL | Nullable; `SUM(skill_gains.ped_value)`. |
| `codex_pes` | REAL | Nullable; `SUM(codex_claims.ped_value)`. |
| `quest_pes` | REAL | Nullable; `SUM(quest_claims.ped_value)`. |
| `computed_at` | REAL | Not null; defaults to `unixepoch('now')`. |
| `harvest_loot_tt` | REAL | Nullable (migration `0006`); `SUM(harvest_events.loot_total_ped)`. |
| `harvest_cost` | REAL | Nullable (migration `0006`); `SUM(harvest_events.cost_ped)`. |
| `quest_item_tt` | REAL | Nullable (migration `0046`); confirmed non-Universal-Ammo quest reward TT. Universal Ammo enters through the ledger instead. |

#### `daily_ledger_rollups`

The per-day ledger sums by entry type and tag (migration `0004`), normalised so
the window totals, per-day maps, and monthly merges each stay one SQL pass.
Amounts are stored unrounded; rounding stays at response-build time.

| Column | Type | Notes |
| --- | --- | --- |
| `day` | TEXT | Part of the primary key; the `ledger_entries.date` text verbatim. |
| `entry_type` | TEXT | Part of the primary key. |
| `tag` | TEXT | Part of the primary key. |
| `amount` | REAL | Not null; the unrounded per-day sum. |

#### `daily_rollup_meta`

The rollup heal watermark (migration `0004`): a single row whose
`rolled_through` day is the split boundary the reader uses. Every day from the
earliest data day up to and including it has a `daily_rollups` row; healing
advances it to yesterday, so the in-flight day is always served from the raw
tables and every day is re-verified once after it completes.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER | Primary key; constrained to the single row `1`. |
| `rolled_through` | TEXT | Not null; the inclusive ISO day the rollups are current through. |

#### `session_kill_rollups`, `session_loot_rollups`, `session_context_loot_rollups`, `session_pes_rollups`, `session_offensive_evidence_rollups`

Per-session activity rollups (migrations `0026`, `0029`, `0050`, and `0051`;
`eo-services/src/session_rollup.rs`),
the session-grain sibling of the daily projection: the read model behind the
Hunting analytics aggregate, the stock position arithmetic's hunted arm, the
hunting market item universe, and the Market Mobs composition. An ended session's events settle into cells
at the finest grain any of those consumers folds (kill cells by context,
species, and maturity; active loot cells by species, shrapnel flag, and item,
with the species pre-folded to the empty string for shrapnel rows; item
composition by activity context; skill-gain cells by context; model-neutral
offensive evidence by activity context, species, and evidence fingerprint), so readers do
O(cells) work however long the raw history grows. Expected-return cells retain
the captured evidence JSON rather than a computed model output, which lets a
later Community Model version replay historical inputs without returning to
lifetime kill facts. Like the daily rollups this is a rebuildable projection: cells write
eagerly in the mutating transaction (session stop, orphan recovery, the loot
edit flip, session delete) and heal lazily before an activity read.

`session_rollup.rs` owns the typed effective hunted-loot and offensive-evidence
boundaries shared by these consumers. A fully settled read selects only the
projection tables and does not prepare statements against `kills`,
`kill_loot_items`, or `kill_tool_stats`. When a live, invalidated, or
below-version session exists, the same boundaries add raw cells for that named
session through the session and kill indexes. The context stamp lets the same
offensive-evidence cells serve Overall, definition, species, and exact
activity-signature folds without returning to lifetime kill facts or inventing
a split inside a joint activity.

| Table | Cell key | Sums |
| --- | --- | --- |
| `session_kill_rollups` | `session_id`, `context_id` (nullable), `mob_species`, `mob_maturity` (empty string for unclassified) | `kills`, `cycled_ped` (weapon + enhancer), `loot_tt` |
| `session_loot_rollups` | `session_id`, `mob_species`, `is_enhancer_shrapnel`, `item_name` (active rows only) | `quantity`, `value_ped` |
| `session_context_loot_rollups` | `session_id`, `context_id` (nullable), `item_name` (active non-enhancer-shrapnel rows only) | `quantity`, `value_ped` |
| `session_pes_rollups` | `session_id`, `context_id` (nullable) | `pes` |
| `session_offensive_evidence_rollups` | `session_id`, `context_id` (nullable), `mob_species`, `evidence_fingerprint`; nullable `expected_economics_json` retains model-neutral captured evidence | `shots_fired`; `missing_candidate_raw_tt` and `missing_basis_phases` cover every null-evidence phase, including legacy and unsupported evidence |

#### `session_rollup_meta`

The settlement marker: a session listed at the current `ROLLUP_VERSION` is
served from its cells, and every other session (the live one, a freshly
edited one, a stale version) is served from the raw tables scoped to its own
id, so a reader is correct regardless of heal timing. The raw statement is
prepared only when this unsettled set is non-empty; settled reads cannot fall
back to lifetime fact-table scans.

| Column | Type | Notes |
| --- | --- | --- |
| `session_id` | TEXT | Primary key. |
| `rollup_version` | INTEGER | Not null. Cell-format version; a below-version session is served raw and re-settles on the next heal. |

## Migration mechanism

Schema application is handled by the embedded migration runner (`MIGRATIONS`
in `eo-services/src/db/migrate.rs`) over the migration set in
`eo-services/migrations/`, whose files are compiled into the binary (a unit
test pins the embedded chain to the directory's contents). The set carries the
version-33 baseline (`0001_schema_baseline.sql`) followed by forward-only
migrations (`0002_analytical_indexes.sql`,
`0003_session_summary_read_columns.sql`, `0004_daily_rollups.sql`,
`0005_market_observations.sql`, `0006_harvest_events.sql`,
`0007_map_pins.sql`, `0008_map_views.sql`, `0009_map_navigation.sql`,
`0010_navigation_runtime_fields.sql`, `0011_pin_configs.sql`,
`0012_harvest_stock_removed.sql`, `0013_harvest_yield_tier.sql`,
`0014_auction_sales.sql`, `0015_stock_movement_tool.sql`,
`0016_stock_opening_balance.sql`, `0017_undone_entries.sql`,
`0018_session_facets.sql`, `0019_session_intervals.sql`,
`0020_signal_quests.sql`, `0021_quest_families.sql`,
`0022_session_definitions.sql`, `0023_default_session_definition.sql`,
`0024_hunting_stock_provenance.sql`, `0025_loot_item_name_indexes.sql`,
`0026_session_activity_rollups.sql`,
`0027_hunting_definition_provenance.sql`,
`0028_quest_reward_provenance.sql`,
`0029_session_context_loot_rollups.sql`,
`0030_quest_reward_items.sql`, `0031_stock_outcomes.sql`,
`0032_inventory_hub.sql`, `0033_listing_duration.sql`,
`0034_listing_instant.sql`, `0035_quest_reward_kinds.sql`,
`0036_mixed_quest_reward_kinds.sql`, `0037_typed_quest_rewards.sql`,
`0038_quest_runs.sql`, `0039_market_unit_prices.sql`,
`0040_quest_reward_reviews.sql`, `0041_ARIS_unresolved_rewards.sql`,
`0042_quest_run_ownership.sql`, `0043_protection_accounting.sql`,
`0044_deferred_protection_costs.sql`, `0045_manual_quest_hand_in.sql`,
`0046_quest_reward_lifecycle.sql`, `0047_healing_attribution.sql`,
`0048_expected_hunting_evidence.sql`,
`0049_expected_hunting_phase_identity.sql`,
`0050_session_offensive_evidence_rollups.sql`,
`0051_context_offensive_evidence.sql`, `0052_protection_hit_allocation.sql`,
`0053_session_protection_policy.sql`, `0054_session_armour_cost_policy.sql`,
`0055_session_grain_protection_costs.sql`, `0056_protection_recording_undo.sql`,
`0057_healing_corrections.sql`); the runner
records applied migrations in the `_sqlx_migrations` ledger (the table name,
column shapes, and SHA-384 checksum accounting are inherited unchanged from
the previous runner, so existing databases reconcile byte for byte) and never
runs a down-migration. Applied rows must form a contiguous, checksum-identical
prefix of the embedded chain; any drift refuses loudly before anything
applies. The version the baseline reproduces is pinned by the
`BASELINE_SCHEMA_VERSION` constant (33); the post-baseline migrations extend
the schema in place and do not change that version row.

Migration files are immutable once they can have been applied. A later schema
refinement always receives the next version, including during feature-branch
development against a persistent dogfood database. This preserves checksum
validation as a corruption and provenance guard rather than turning the ledger
into mutable development state.

### The version-33 baseline

The baseline is the schema as it stands at version 33, written out statement for
statement. It creates every table, index, and timestamp-back-fill trigger in one
migration and stamps the `db_metadata` version row to `33`. The version number
is the cumulative result of the schema's earlier evolution; that incremental
history is folded into the single baseline rather than replayed. A fresh
database therefore lands directly on the version-33 surface before the forward
migration tail brings it to the current schema.

### Open paths: fresh, adoption, and first-launch upgrade

On open, `Db::open` configures the connection, then calls `adopt_or_refuse`
before running the migration chain. This reconciles the on-disk schema with the
baseline and takes one of these paths:

- **Fresh:** an empty (or absent) database is created and the runner applies
  the baseline directly, landing it at version 33.
- **Adoption:** a database already at version 33 that carries no migration
  ledger is adopted in place. The baseline is marked applied (the ledger row is
  written with the baseline's own checksum) without re-running any DDL, and the
  post-adoption runner pass then validates that row.
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
