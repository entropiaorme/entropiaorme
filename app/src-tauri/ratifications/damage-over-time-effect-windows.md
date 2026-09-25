# Ratification: damage-over-time effect windows

An adversarial review of the golden changes that come with damage-over-time weapons: a declared effect profile, persisted effect windows, cost-free `effect_tick` shots, the nearest-floor and concurrency rules, and the `effect_candidates` snapshot column. It also covers the player's own `You missed` now parsing as a countered shot. The review covers the full range `ce22af3..92c0266`. I read the production code behind every pin, not just the diffs:
- `attribution.rs`, `combat.rs`, `weapon_effects.rs`, `weapon_evidence.rs`, `weapon_effect.rs`
- `db_snapshot.rs`, `weapon_review/{read,correct,mod}.rs`
- `chatlog_parser.rs`, `chatlog_watcher.rs`, `bus_events.rs`

I also read the harness and ADR-0033. An earlier round found that the first pin froze a dropped `You missed` (an underbilled primary shot) as its reference result. That was fixed in the code rather than adapted into the oracle, and this round re-audits the rebuilt range.

## Findings

The golden changes are ratified as genuine, intended behaviour changes.

- **OR1**
  - **Path:** `app/src-tauri/fixtures/corpus/{scripted/*,recorded/placeholder_recorded_hunt}/expected/db_state.json` (17 existing scenarios)
  - **Dimension:** 1, 2
  - **Severity:** `genuine-spec-move`
  - **Description:** The additions are exact:
    - The only removed lines are the five `correction_id` lines that gained a trailing comma.
    - No existing `fingerprint.jsonl` moves.
    - Each file gains one `"weapon_effect_windows": []`. Keys serialise alphabetically, and the catalogue appends the table last, so no symbol is renumbered.
    - Each stored shot gains `"effect_candidates": 0`. Per scenario, the count equals the `weapon_shot_evidence` rows exactly: 3+5+7+0+0+3+2+2+0+2+2+2+6+4+4+2+6 = 50.
    - `json_array_length('[]')` is 0, and migration 0059 defaults the column to `'[]'`.
    - Without an open window, the refactored `classify` is equivalent to the old one. The ticking check, the `is_none_or` fold, and the confirm guard `record.windows.is_empty()` all collapse to the previous paths.
    - `You missed` appears in no other fixture, so the new parser rule cannot move these scenarios.
  - **Suggested resolution:** None.

- **OR2**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/basic_hunt_10_events/raw_captures/db_rows.json`
  - **Dimension:** 1, 2
  - **Severity:** `inconsequential`
  - **Description:** The file adds `effect_candidates: 0` to its five hand-authored evidence rows and changes nothing else. The key is the catalogue's output alias, which is correct for raw query rows. The file carries no `weapon_effect_windows` key, and `capture` treats an absent table as empty. This file is an input, not a golden path (the ratify guard only matches paths containing `/expected/`). It is covered under `basic_hunt_10_events`.
  - **Suggested resolution:** None.

- **OR3**
  - **Path:** `app/src-tauri/eo-api/resources/demo_goldens/tracking_session_detail.txt`
  - **Dimension:** 1, 2
  - **Severity:** `genuine-spec-move`
  - **Description:** The only change is four added keys: `markedTicks`, `pricedTicks`, `unclaimedTicks` and `effects: []`. They sit in the struct's declaration order around the existing `effectTicks`. The values come from `weapon_review/read.rs` counts over a demo session with no evidence rows, not from hardcoded defaults.
    - The read-time `standing` flag, the `STANDING_TICKS` join against existing windows, and the refusal to mark a tick on a withdrawn window touch only paths that read or correct rows this session does not have.
    - No other value moves.
  - **Suggested resolution:** None.

- **OR4**
  - **Path:** `code:app/src-tauri/eo-services/src/chatlog_parser.rs:248`, `chatlog_watcher.rs:43`, `bus_events.rs` (`CombatPayload::TargetMiss`), `tracker/combat.rs:297`; `dot_weapon_rotation/expected/fingerprint.jsonl` (the `target_miss` line)
  - **Dimension:** 3, 4
  - **Severity:** `genuine-spec-move`
  - **Description:** The player's own `You missed` is now a paid countered shot.
    - **The rule:** It is anchored `^You missed$`. The message is trimmed first, so CRLF logs match. It cannot collide with `The attack missed you` (`MobMiss`).
    - **The routing:** The miss joins jam, dodge and evade on the `Observation::Countered` path. That path books a shot to the weapon in hand and opens no effect window, because `EffectActivation::for_shot` requires a hit. This is the right classification: a miss spends a use.
    - **The fingerprint:** The `target_miss` line sits in chat order in its tick group. It adds no `tracking.session.updated` frame, since it shares that tick.
    - **Other checks:**
      - `event_schemas.snapshot.json` carries no combat payloads, and schema conformance still passes.
      - The `EventType::ALL` count tripwire was raised from 22 to 23.
  - **Suggested resolution:** None.

- **OR5**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/dot_weapon_rotation/expected/{db_state.json,fingerprint.jsonl}` (first pin)
  - **Dimension:** 1, 3, 5
  - **Severity:** `genuine-spec-move`
  - **Description:** I checked the absolute output line by line against ADR-0033 and `attribution.rs`. The bands are:
    - Mayhem: 95.7-191.4, 0.366602 per shot.
    - Electrocution: declared cast band 100-160, then ticks 35-75 for 25 s, 4.8732 per cast.

    I recomputed every figure from the chat log (55 damage lines, 4 jams, 1 miss, 1 damage-taken, 17 heals):
    - **The cast:** Electrocution is declared, so 129.2 at clock 0 agrees. It also fits Mayhem, but the declared weapon wins. It opens exactly one window, and that window is not withdrawn.
    - **The 0.8:** It is short of every band. `nearest_below` finds tick floor 35 below the declared floor 100, so it becomes a tick rather than a second billed cast.
    - **53.0 at clock 1:** It is still under the Electrocution press, so it is a tick.
    - **57.5 onward:** These come under Mayhem, after the press at 1.5. Each value in 35-75 up to clock 24 is a tick; the last is 37.8.
    - **The 21 ticks:** They sum to 1162.6. Every tick row has cost 0, `tool_name` null, `effect_window_id` pointing to the one window, `effect_candidates` 1, and the kill id.
    - **Hits below the Mayhem floor:** The 90.0, 93.5, 89.8, 86.9 and 95.3 hits exceed the tick maximum, so they agree `BelowBand` with Mayhem.
    - **No false switches:** No tick raises a switch (no reviews, evidenced 0).
    - **The kill:**
      - Electrocution 1 shot, 129.2 damage.
      - Mayhem 37 shots (32 hits worth 4170.9, 4 jams, 1 miss).
      - 38 shots in total.
      - Kill cost: 18.4375.
      - Kill damage: 5462.7 = 4300.1 + 1162.6.
      - Dangling cost: 0.3666, so the session costs 18.8041.
      - Agreed tally: 39.
    - **Fingerprint:** 51 `tracking.session.updated` frames: start, 47 chat ticks, 2 presses, and the stop.
    - **Healing:** The 17 lifesteal outputs are the pre-existing passive classification.
  - **Suggested resolution:** None.

- **OR6**
  - **Path:** `dot_weapon_rotation/metadata.yaml`, `docs/src/adr/0033-damage-over-time-effect-windows.md`, the feature commit message
  - **Dimension:** 1
  - **Severity:** `genuine-spec-move`
  - **Description:** The rationale matches the pin: one cast, 38 primary shots (the miss included), 21 ticks, 18.80 PED. The installed capture's kill recorded Electrocution 3 shots and Mayhem 56: 3 × 4.8732 + 56 × 0.366602 = 35.1493. The ratio 35.1493 / 18.8041 = 1.87, so "nearly doubling" is accurate. The ADR discloses that the original recording also dropped the miss.
  - **Suggested resolution:** None.

- **OR7**
  - **Path:** `dot_weapon_rotation/expected/db_state.json` (`tracking_sessions.dangling_cost` 0.3666; kill damage without the 103.2)
  - **Dimension:** 3
  - **Severity:** `inconsequential`
  - **Description:** The final 103.2 is dangling because the whole 17:26:45 tick group arrives in one flush, and `flush_tick` publishes loot before other events. The live build settled it into the kill. The session total is unaffected. The metadata discloses this. It rests on the harness's one-flush-per-tick contract, which every scenario shares.
  - **Suggested resolution:** None.

- **OR8**
  - **Path:** `dot_weapon_rotation/expected/db_state.json` (`kill_tool_stats` Electrocution `damage_dealt` 129.2); `code:app/src-tauri/eo-services/src/tracker/combat.rs` (`record_offensive_shot`)
  - **Dimension:** 3
  - **Severity:** `genuine-spec-move`
  - **Description:** Tick damage counts in the kill's damage but not in the casting weapon's tool stats, as ADR-0033 states. The figures reconcile through `effects[].tickDamage`.
  - **Suggested resolution:** None.

- **OR9**
  - **Path:** all new instants (`weapon_effect_windows.started_at` and `expires_at`, tick `observed_at`, withdrawal stamps)
  - **Dimension:** 6
  - **Severity:** `inconsequential`
  - **Description:** No ambient input reaches the pin:
    - Windows open at `observed_at`, which comes from the injected clock.
    - `expires_at` is start plus duration. Restore compares against the clock-derived session start. Withdrawal stamps the review's clock-derived `decided_at`.
    - Window ids are random UUIDs. They normalise in `started_at, rowid` order, and the snapshot reduces the in-JSON candidate ids to a count by design.
  - **Suggested resolution:** None.

- **OR10**
  - **Path:** corpus coverage (no golden)
  - **Dimension:** 5
  - **Severity:** `inconsequential`
  - **Description:** No corpus scenario reaches five paths; only unit and tracker tests cover them:
    - withdrawal on keep,
    - overlapping windows,
    - the concurrency `Unresolved` rule,
    - cross-session or restart restore,
    - the post-play corrections.
  - **Suggested resolution:** Optionally, add a follow-up scenario with a restart mid-window and a second cast that overlaps the first.

Verification (all through the bounded runner with serial flags, at `92c0266`):
- Corpus replay oracle: 19 of 19 on the host, and 19 of 19 under each of America/Los_Angeles, Asia/Kolkata and Pacific/Chatham.
- `dot_weapon_rotation` alone: passed in five repeat runs.
- Hermetic emitter proof and event schema conformance: 13 of 13.
- Demo golden reproduction: passed.
- Parser, watcher, bus, weapon-effect and weapon-review unit tests: 76 of 76.
- Scripted arithmetic: recomputing from the chat log matches every pinned figure, including the miss.

```text
ORACLE-RATIFICATION
range: ce22af3..HEAD
goldens: tracking_session_detail, placeholder_recorded_hunt, basic_hunt_10_events, crit_dodge_evade_jam, defensive_combat_round, dot_weapon_rotation, empty_session, enhancer_break_during_hunt, global_item_drop, global_kill_correlated, healing_effect_rotation, hof_item_drop, hof_kill_correlated, mission_completion_with_reward_suppression, multi_mob_hunt_loot_grouping, single_mob_hunt, skill_gain_across_tick, tree_harvesting_session, weapon_attribution_mismatch
VERDICT: ratification-sound
```
