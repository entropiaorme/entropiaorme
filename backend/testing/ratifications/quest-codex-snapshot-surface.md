# Ratification: quest/codex snapshot-surface extension

Independent review of the DB-state snapshot catalogue's extension across the
quest/codex write surface and the resulting first-pin growth of every corpus
`db_state.json` golden. The reviewer is not the change's author; this audit is
from the evidence in the tree.

## Range and goldens under review

- Range: the catalogue-extension branch against its base (the branch tip
  carrying this report).
- Code: `backend/testing/db_snapshot.py` `CATALOGUE` extended from six to
  sixteen `TableSpec` entries (quests, quest_mobs, quest_playlists,
  quest_playlist_items, session_quest_completions, codex_progress,
  codex_claims, quest_claims, session_quest_analytics_links,
  skill_calibrations), mirrored entry-for-entry in
  `frontend/src-tauri/eo-wire/src/db_snapshot.rs`.
- Goldens: all twelve corpus `expected/db_state.json` files regenerated; each
  gains exactly the same ten keys, all empty arrays (120 insertions, no
  deletions, no existing key or value touched).

## Findings

- **Catalogue parity.** The Python and Rust catalogues gain the same ten
  entries, in the same order, with byte-identical column lists and identical
  `order_by` tuples, appended at the same point so shared-symbol assignment
  order is preserved.
- **Determinism of every captured timestamp.** Five captured columns carry a
  schema `DEFAULT (unixepoch('now'))` (`session_quest_completions.completed_at`,
  `codex_claims.claimed_at`, `quest_claims.claimed_at`,
  `session_quest_analytics_links.linked_at`, `skill_calibrations.scanned_at`).
  Every production insert site passes an explicit timestamp from the service's
  injected clock (or, for the skill tracker's calibration writes, from the
  replayed event's own timestamp), so neither the column default nor the
  auto-fill trigger ever fires and no ambient wall-clock value can enter a
  pinned row. `linked_at` is both trigger-backed and the sort key; the explicit
  stamp at its single insert site is what keeps that trigger dormant.
- **Pure wall-clock columns excluded.** The `created_at` / `updated_at`
  defaults on quests, quest_playlists, quest_playlist_items, and
  codex_progress stay out of the explicit column lists.
- **Empty arrays are the correct pre-port baseline.** The corpus replay
  pipeline boots the tracker only; the quest and codex services are not
  subscribed, and the replay database starts cold, so zero quest/codex rows is
  the genuinely intended contract rather than a faithfully pinned defect. The
  skill tracker's calibration insert is gated on a previously known level, which
  a cold database never supplies. These empties are load-bearing: once the
  ported services come online, any unintended write surfaces as a golden diff.
- **Sort keys are replay-stable.** Each new entry orders by a service-stamped
  instant with `rowid` as tiebreaker (or by natural identity columns); no raw
  UUID column is ever a sort key, so normalisation order cannot flip row order
  across runs.
- **Scope boundary.** `skill_calibrations_archive` stays out of the catalogue
  deliberately: it is written only by the scan-completion flow, which is outside
  the quest/codex surface this extension exists to oracle and carries its own
  unit-level verification.

## Verdict

The delta is a genuine, minimal, deterministic oracle-surface extension, not a
regression pinned as the new truth. Every captured wall-clock-defaulted column
is provably stamped from an injected or event-derived instant; the added keys
are uniform empty arrays whose emptiness is the correct consequence of the
replay pipeline's wiring; and the Python and Rust catalogues move in lockstep.
The full e2e suite, the Rust corpus replay oracle, and the snapshot emitters
proof all pass against the regenerated goldens.

```text
ORACLE-RATIFICATION
range: branch base..HEAD (the catalogue-extension branch)
goldens: backend/tests/e2e/corpus/recorded/placeholder_recorded_hunt/expected/db_state.json, backend/tests/e2e/corpus/scripted/basic_hunt_10_events/expected/db_state.json, backend/tests/e2e/corpus/scripted/crit_dodge_evade_jam/expected/db_state.json, backend/tests/e2e/corpus/scripted/defensive_combat_round/expected/db_state.json, backend/tests/e2e/corpus/scripted/empty_session/expected/db_state.json, backend/tests/e2e/corpus/scripted/enhancer_break_during_hunt/expected/db_state.json, backend/tests/e2e/corpus/scripted/global_kill_correlated/expected/db_state.json, backend/tests/e2e/corpus/scripted/hof_item_drop/expected/db_state.json, backend/tests/e2e/corpus/scripted/mission_completion_with_reward_suppression/expected/db_state.json, backend/tests/e2e/corpus/scripted/multi_mob_hunt_loot_grouping/expected/db_state.json, backend/tests/e2e/corpus/scripted/single_mob_hunt/expected/db_state.json, backend/tests/e2e/corpus/scripted/skill_gain_across_tick/expected/db_state.json
VERDICT: ratification-sound
```
