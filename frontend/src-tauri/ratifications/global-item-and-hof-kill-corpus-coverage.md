# Ratification: global_item and hof_kill corpus coverage

Adversarial review of the first-pinned testing-oracle goldens that accompany
closing the last two uncovered `EventType` branches in the cross-language
chatlog differential. The review re-derives the verdict against the current
tree rather than accepting the change author's rationale, because a
self-approved golden move (here, a first-pin with no prior golden to diff
against, so no red gate fires) carries a structural conflict of interest.

## Change under review

The cross-language chatlog differential
(`frontend/src-tauri/eo-services/tests/chatlog_differential.rs`) drove only 19
of the 21 `EventType` classes. Two Globals-channel branches were never driven
by any corpus scenario: the plain (non-Hall-of-Fame) rare-item global
(`global_item`) and the Hall-of-Fame kill global (`hof_kill`). Two new scripted
corpus scenarios close the gap, and a coverage assertion derived from
`EventType::ALL` (`frontend/src-tauri/eo-services/src/chatlog_parser.rs`) makes
21/21 coverage a transcript-observable invariant that forces the corpus to grow
with any future variant.

**No production code changed** (no parser rule, no tracker logic). Each scenario
is DSL-authored apparatus only:

- `global_item_drop`: a kill (28.0 + 33.0 damage, 2 shots), Shrapnel loot
  x1200 (12.00 PED), then a plain rare-item global naming `TestPlayer` within
  the 5-second correlation window (`s.globals.item(..., hof=False)`).
- `hof_kill_correlated`: a kill (40.0 + 35.0 damage, 2 shots), Shrapnel loot
  x1500 (15.00 PED), then a Hall-of-Fame kill global naming `TestPlayer`
  (`s.globals.kill(..., hof=True)`).

Each adds `build.py`, `metadata.yaml`, a generated `chat_replay.log`, an
acceptance test (`backend/tests/e2e/test_global_item_drop.py`,
`backend/tests/e2e/test_hof_kill_correlated.py`), and a Rust replay-oracle
registration (`frontend/src-tauri/eo-services/tests/corpus_replay_oracle.rs`).

## Oracle delta reviewed

The expected-output set first-pinned in this range is exactly four files (the
two scenarios' `expected/{fingerprint.jsonl, db_state.json}`). The parser rules
for both branches already existed and are byte-equivalent across the Rust and
Python arms (the differential stays green); these scenarios merely *drive* the
previously-undriven branches and pin the full-pipeline output.

The four cells of the kill/item-global x plain/HoF matrix are now all pinned;
the two new goldens are byte-identical in structure to their proven-green
siblings (`global_kill_correlated`, `hof_item_drop`), differing only where the
`build.py` DSL intends.

## Adversarial review findings

The review re-derived every load-bearing value from `backend/tracking/tracker.py`
`_on_global` (lines 1544-1591) rather than accepting that the pinned output is
correct because it is what the pipeline emitted:

- **Kill HoF-tagging.** `global_item_drop` pins the lone kill `is_global=1,
  is_hof=0`; `hof_kill_correlated` pins `is_global=1, is_hof=1`. Confirmed
  against `_on_global` (`is_hof = event_type in ("hof_kill", "hof_item")`;
  `target.is_global = True` unconditionally on a within-5s kill). The matrix is
  consistent with `global_kill_correlated` (is_hof=0) and `hof_item_drop`
  (is_hof=1).
- **notable_events.** Correct `event_type` (`global_item` / `hof_kill`),
  `mob_or_item` (the item / creature field), and `value_ped` (850.0 / 2200.0,
  the global's announced value, not the loot TT).
- **Hunt arithmetic.** Damage totals 61.0 / 75.0, shots 2, loot 12.0 / 15.0,
  Shrapnel-conversion ledger 0.12 / 0.15 (1% of loot), matching the siblings.
- **No over-emission.** Line-by-line diff of both fingerprints against their
  structural siblings shows identical topic, ordering and frequency; one global
  event, one kill / loot row / ledger row / notable_event. No phantom rows, no
  double-counting.
- **Determinism.** Every timestamp and id is normalised; the committed clock
  plan supplies the plan-stamped instants. No ambient (wall-clock / random /
  environment) value survived into the pinned output.

## Verdict

```
ORACLE-RATIFICATION
range: origin/main..HEAD
goldens: global_item_drop, hof_kill_correlated
VERDICT: ratification-sound
```
