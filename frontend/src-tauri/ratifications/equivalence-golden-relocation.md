# Equivalence-golden relocation

## Context

Retiring the cross-language equivalence oracle moved every committed golden out of
the retired Python tree into the Rust workspace, with no change to its content:

- the contract snapshots (`openapi.snapshot.json`, `event_schemas.snapshot.json`)
  to `frontend/src-tauri/contracts/`;
- the replay-corpus goldens (each scenario's `fingerprint.jsonl`, `db_state.json`,
  and `http_responses/`) to `frontend/src-tauri/fixtures/corpus/`;
- the normaliser conformance table and the yml-family consistency mirrors to
  `frontend/src-tauri/eo-wire/tests/fixtures/`.

Because the relocation re-paths committed golden files, the golden-ratification
guard treats it as a golden change and requires a recorded verdict, even though
no expected output was regenerated. This report records that verdict.

## Adversarial review

The relocation was checked, file by file, as a pure rename rather than a content
change: every renamed golden's blob object identifier is identical between its
former path and its new path (a full object-identifier sweep over the renamed set
returned zero divergences), so the bytes the hermetic tests assert are unchanged.
The surviving tests were confirmed to still assert those goldens non-vacuously
from the new locations (the corpus replay, the emitter proof, the conformance and
contract checks, and the HTTP-consistency replay all run with no second
implementation present and pass byte-for-byte). No expected output was added or
modified in content; this is a relocation, not a regeneration.

```
ORACLE-RATIFICATION
range: refactor/retire-python-oracle (the equivalence-oracle retirement)
goldens: openapi, event_schemas, normalizer_conformance, hotbar_slot_use, spacebar_scan_capture, quest_automation_with_playlist_match, basic_hunt_10_events, single_mob_hunt, multi_mob_hunt_loot_grouping, mission_completion_with_reward_suppression, crit_dodge_evade_jam, defensive_combat_round, empty_session, enhancer_break_during_hunt, global_item_drop, global_kill_correlated, hof_item_drop, hof_kill_correlated, skill_gain_across_tick, consistency_codex_isolation_midpoint, consistency_quests_mission_lifecycle_midpoint, consistency_scan_isolation_midpoint, consistency_tracking_hunt_midpoint, placeholder_recorded_hunt
VERDICT: ratification-sound
```

The verdict is sound because the relocation is byte-identical: equal bytes mean
the equivalence behaviour the goldens pin is unchanged, so no regression can have
been ratified by the move.
