# Ratification: unified weapon attribution

An adversarial review of the golden changes that come with unified weapon attribution:
- One engine: hotbar intent, checked against each carried weapon's damage band.
- Stored per-shot weapon evidence, live mismatch decisions and per-session tallies.
- The new `weapons.updated` domain event.
- Retirement of the trifecta preset and of the merge that moved unpriced shots onto the first press.

## Findings

The golden changes are ratified as genuine, intended behaviour changes. I read the production code behind every pin, not just the diffs.

- **OR1**
  - **Path:** `app/src-tauri/contracts/event_schemas.snapshot.json`
  - **Dimension:** 1, 2
  - **Severity:** `genuine-spec-move`
  - **Description:** The snapshot gains a closed `WeaponsUpdated` envelope and an empty, closed `WeaponsUpdatedPayload`, plus one discriminator entry and one `oneOf` member. Apart from the description text, the envelope is identical to `HealingUpdated`. It matches the native type in `eo-wire/src/domain_events.rs`, and the conformance test pins it. No existing definition changes. The event is published only from the post-play assignment path (`entropia-orme/src/composition.rs`). Live decisions correctly use `tracking.session.updated` instead.
  - **Suggested resolution:** None.

- **OR2**
  - **Path:** `app/src-tauri/fixtures/corpus/{scripted/*,recorded/placeholder_recorded_hunt}/expected/db_state.json` (16 existing scenarios)
  - **Dimension:** 1, 2, 3
  - **Severity:** `genuine-spec-move`
  - **Description:** The three new catalogue tables are added at the end, so no existing symbol can be renumbered.
    - Across all 16 diffs the only removed lines are closing brackets.
    - No existing `fingerprint.jsonl` moves.
    - `kills` and `kill_tool_stats` are byte-identical.
    - Frequency check: in every scenario, the number of `weapon_shot_evidence` rows for a kill equals exactly the `shots_fired` of that kill's `Unknown` phase. That gives one row per unpriced shot, 1:1: 1+2, 3+1+1, 7, 3, 2, 2, 2, 2, 2+3+1, 4, 2+2 and 2 shots.
    - Scenarios without shots have no rows.
    - There is no `hotbar_tool`, the candidates list is empty, `cost_per_shot` is 0, and there are no reviews.

    This matches the code: `load_carried` returns no weapons when a scenario has no `carried.json`. `classify` then returns `Unresolved` "fits no carried weapon", and `keeps_row` stores every unresolved shot. Pricing is unchanged: the shot books to the `Unknown` phase at zero cost, as before.
  - **Suggested resolution:** None.

- **OR3**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/healing_effect_rotation/expected/db_state.json` (`weapon_attribution_tallies`, first row)
  - **Dimension:** 1, 3
  - **Severity:** `genuine-spec-move`
  - **Description:** The first session's tallies are NULL/NULL, not 0/0. This is correct:
    - The first session is closed by crash recovery. `recover_orphaned_sessions` (`eo-services/src/tracker/persistence.rs`) does not write tallies, because the live counters died with the process. Writing 0/0 would invent a measurement.
    - The second session stops normally and records 0/0.
    - The detail read and the UI already treat NULL as "not tallied" and leave it out.
  - **Suggested resolution:** No change to the golden. The schema reference describes a NULL tally only as "a session recorded before the tally was kept". It should also cover a session closed by crash recovery.

- **OR4**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/crit_dodge_evade_jam/expected/db_state.json`
  - **Dimension:** 1
  - **Severity:** `genuine-spec-move`
  - **Description:** Seven rows for the seven `Unknown` shots, in chat order:
    - Four hits give "fits no carried weapon". The critical 30 correctly has `critical` 1.
    - The dodge, the evade and the jam give "a countered shot with no weapon known", with a null `amount`.

    This follows `classify_countered`: no mismatch, nothing declared and nothing recorded mean the shot is unresolved. The target's evade is an offensive miss, and it was already counted as a shot before this change.
  - **Suggested resolution:** None.

- **OR5**
  - **Path:** `weapon_shot_evidence.observed_at`, all scenarios; `code:app/src-tauri/eo-services/src/tracker/combat.rs:240`
  - **Dimension:** 6
  - **Severity:** `inconsequential`
  - **Description:** `observed_at` is taken from the injected clock (`self.clock.now()`), not from the ambient wall clock. In stepless scenarios the replay clock never advances, so every row shares the session-start symbol. `kills.timestamp` stays a chat-log domain timestamp, as before. Healing evidence and hotbar presses are stamped the same way. Nothing ambient leaks in: evidence and review ids are random UUIDs, but they normalise to symbols in a deterministic `observed_at, rowid` / `decided_at, rowid` order.
  - **Suggested resolution:** None.

- **OR6**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/basic_hunt_10_events/raw_captures/db_rows.json`
  - **Dimension:** 1, 2
  - **Severity:** `inconsequential`
  - **Description:** The file gains five hand-authored evidence rows and one tally row. They reuse the real session and kill ids, use synthetic evidence UUIDs, and set `observed_at` to the raw `started_at` (1767225600.0). The hermetic `emitters_proof` therefore reproduces the regenerated `db_state`, including the shared symbol with `started_at`. That test proves the catalogue and the normaliser; the live corpus oracle proves the pipeline. This file is an input, not a golden path.
  - **Suggested resolution:** None.

- **OR7**
  - **Path:** `app/src-tauri/eo-api/resources/demo_goldens/tracking_snapshot.txt`
  - **Dimension:** 1, 2, 4
  - **Severity:** `genuine-spec-move`
  - **Description:** Parsed key by key, the only changes are:
    - `weaponAttribution` and `trifectaAttribution` are removed, because the mode and the preset are retired.
    - `hotbarKeysEnabled` and `unpricedShots` (0) are added.
    - No shared value changes, and the shared key order is preserved.

    `hotbarKeysEnabled` is true because the demo config now enables hotbar hooks and binds the curated weapons and healer to slots 1 to 3. The old snapshot reported the listener active while running band mode, so this is more coherent. The demo config only shapes this read. `weaponGuardrail` is correctly absent, since nothing is mismatched.
  - **Suggested resolution:** None.

- **OR8**
  - **Path:** `app/src-tauri/eo-api/resources/demo_goldens/tracking_session_detail.txt`
  - **Dimension:** 1, 2
  - **Severity:** `genuine-spec-move`
  - **Description:** The only change is the added `weaponAttribution` block, placed before `healing` as the struct declares.
    - `correctable` is true, from the same ended-session test healing uses.
    - The tallies are null, because the bundled demo session predates them.
    - All stored counts are zero and there are no reviews.
  - **Suggested resolution:** None.

- **OR9**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/weapon_attribution_mismatch/expected/{db_state.json,fingerprint.jsonl}` (first pin)
  - **Dimension:** 3, 5
  - **Severity:** `genuine-spec-move`
  - **Description:** I checked this pin line by line against the attribution design and `attribution.rs`, with no baseline. The bands are pistol 5-10, cannon 20-40 and rifle 8-16. Criticals reach three times the maximum. Per-shot costs come from the decay: 0.05, 0.20 and 0.10.
    - **Before the confirm:**
      - The 7 agrees with the pistol and proves it.
      - The jam agrees as a countered shot.
      - The 30 fits only the cannon. It becomes evidence, priced 0.20, and raises the mismatch.
      - Kill 1 settles on the loot.
      - The 33 is evidence for the cannon, and the dodge is evidence "while Cannon is recorded".
      - The non-critical 90 exceeds every band and stays unresolved.
    - **The confirm:** It walks back from the newest shot:
      - The 90 does not fit the cannon, so it stays.
      - The jam moves.
      - The walk stops at the 7.

      So it reprices exactly the jam, inside the already-settled kill 1, from 0.05 to 0.20. The result is 1 repriced shot, a delta of +0.15, and kill 1 at 0.45 with Pistol 1 shot and Cannon 2 shots.
    - **After the confirm (the cannon is declared):**
      - After the 2-second advance, the 25 agrees.
      - The critical 13 fits the pistol and the rifle but not the cannon (its critical floor is 20). It stays unresolved as "several other".
      - Kill 2 = Cannon 3 (58 damage, 0.60) plus Unknown 2 (103 damage, 1 crit), cost 0.6.
    - **Switch back to the pistol:**
      - The 35 lands at 0 seconds, inside the 1.25-second tail. It agrees in flight and is priced to the cannon.
      - The 9 agrees with the pistol.
      - The 14 fits only the rifle and becomes evidence.
    - **The keep:** It reprices the 14 back to the pistol (1 shot, -0.05) and silences the rifle for the rest of the regime. The 15 is therefore a kept agreement.
    - **Dangling cost:** 0.20 + 3 × 0.05 = 0.35.
    - **Tallies:**
      - Agreed 6: 7, the jam as observed, 25, 35, 9 and 15, then +14 and -jam after the decisions.
      - Evidenced 4: 30, 33, the dodge and the jam after the confirm.
      - Unresolved 2.

      These match the documented definitions ("as they stood after any live decision", "including those a confirmed decision repriced"). The 35 counts as agreed but is priced to the previous weapon. That follows the design, which places in-flight shots under agreement, even though the column text says "the weapon the hotbar declared".
    - **Stored rows:** exactly the six the design keeps: four evidence against a declared weapon, two unresolved. The kept 14 carries the keep review's id, is priced to the pistol at 0.05, and has no kill.
    - **Fingerprint:** 20 `tracking.session.updated` frames: 1 start, 14 chat ticks, 2 press nudges, 1 per decision, and the stop. No frame fires on a no-op tick. No `weapons.updated` is emitted during live play, which is correct.
  - **Suggested resolution:** None.

- **OR10**
  - **Path:** `weapon_attribution_mismatch/expected/db_state.json` (`weapon_attribution_reviews.mismatch_since`)
  - **Dimension:** 5
  - **Severity:** `inconsequential`
  - **Description:** In both reviews `mismatch_since` equals `decided_at`. The script never advances the clock between the first disagreeing shot and the decision, so this pin could not catch a regression that stamped `since` at decision time. The code is correct: `since` is the first evidence instant (`attribution.rs` `apply`, persisted by `weapon_evidence.rs`), and the classifier's unit tests pin it.
  - **Suggested resolution:** Optionally add a short `advance` before each `decide` step, so the snapshot shows `since` and `decided_at` as different instants.

- **OR11**
  - **Path:** `weapon_attribution_mismatch/expected/db_state.json` (the 14's row); `code:app/src-tauri/eo-services/src/weapon_review/read.rs:203`
  - **Dimension:** 3
  - **Severity:** `inconsequential`
  - **Description:** The kept 14 keeps `attribution` `evidence` (its live classification) while it is priced to the hotbar weapon, with the reversal linked through `review_id`. This is consistent with the schema reference and keeps the provenance. The side effect is on the read side: the detail's `evidenceShots` count and the review group "Overrode the hotbar" include a shot that, after the keep, did not override the hotbar. The review UI does describe the shot as kept.
  - **Suggested resolution:** Optional read-side refinement. No change to the golden.

The frozen HTTP-response goldens under `expected/http_responses` correctly stay the same. They are frozen evidence from the retired transport, which the live oracle never compares, as the retired-armour ratification (OR5) already established.

Verification:
- Corpus replay oracle: 18 of 18 on the host, and 18 of 18 under each of three host timezones (America/Los_Angeles, Asia/Kolkata, Pacific/Chatham).
- New scenario alone: passed in five repeat runs.
- Hermetic emitter proof: 3 of 3.
- Event schema conformance: 10 of 10.
- Demo golden reproduction: passed.

```text
ORACLE-RATIFICATION
range: 5a10eb4..HEAD
goldens: event_schemas, tracking_snapshot, tracking_session_detail, placeholder_recorded_hunt, basic_hunt_10_events, crit_dodge_evade_jam, defensive_combat_round, empty_session, enhancer_break_during_hunt, global_item_drop, global_kill_correlated, healing_effect_rotation, hof_item_drop, hof_kill_correlated, mission_completion_with_reward_suppression, multi_mob_hunt_loot_grouping, single_mob_hunt, skill_gain_across_tick, tree_harvesting_session, weapon_attribution_mismatch
VERDICT: ratification-sound
```
