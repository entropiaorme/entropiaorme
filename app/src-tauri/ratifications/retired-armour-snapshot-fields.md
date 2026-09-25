# Ratification: retired armour fields in the demo tracking snapshot

This is an independent adversarial review of the demo tracking snapshot. Protection (armour) accounting moved to a session-grain model, in which nothing about armour is declared during play (ADR-0031). As part of that change, two snapshot fields were retired along with the surfaces they served.

Range reviewed: `origin/next..HEAD` (`13501d8..a7ef82a`, two commits; `origin/next` is the merge base and the working tree was clean). The only golden edit is in `5f5e9fc`. The later commit `a7ef82a` does not touch any golden.

## Change under review

`demo_goldens/tracking_snapshot.txt` loses exactly two fields:

- `"endOfSessionArmourReminderEnabled":false`
- `"trackProtectionBySegment":false`

The file shrinks from 2743 to 2668 bytes. This was checked mechanically: taking the previous golden and deleting exactly those two `"key":false,` fragments gives a result byte-identical to the new golden. No other value, count, array element, ordering or nested payload changes. The file still has no trailing newline, as before.

In the reviewed range, the only changed path that the ratification guard classes as a golden is this demo golden. That guard covers contract snapshots, corpus `expected/` directories, eo-wire conformance fixtures and demo goldens. The single contract snapshot (`event_schemas`), the eo-wire fixtures and every corpus `expected/` file are untouched.

## Findings

- **OR1**
  - **Path:** `app/src-tauri/eo-api/resources/demo_goldens/tracking_snapshot.txt:1`
  - **Dimension:** 1 (delta accountability) and 2 (minimality)
  - **Severity:** `genuine-spec-move`
  - **Description:** The delta is exactly the removal of the two named keys and nothing else. This is confirmed byte for byte against the previous golden with those two fragments deleted. All the economic figures, the 100-element `multiplierHistory` and `cumulativeNetHistory` series, `recentEvents`, `activities`, `healing` and `trifectaAttribution` are unchanged.
  - **Suggested resolution:** Adapt the golden exactly as proposed.

- **OR2**
  - **Path:** `code:app/src-tauri/eo-api/src/tracking.rs:60`, `code:app/src-tauri/eo-services/src/config_service.rs:469`
  - **Dimension:** 3 (intended, not merely actual) and 4 (fix versus adapt)
  - **Severity:** `genuine-spec-move`
  - **Description:** `endOfSessionArmourReminderEnabled` disappears because the end-of-session armour prompt and its setting were removed, not because the snapshot builder stopped emitting it by accident. The field is removed consistently in every place it lived: the typed `TrackingSnapshot` model and `SNAPSHOT_FIELDS` (51 to 49 entries); both branches of `build_snapshot_value` (idle and active); `AppSettings` and `SettingsPatch`; and `AppConfig` and the demo config stub. The config key has left `KNOWN_KEYS`, so any value already stored now falls into the preserved `extra` map rather than being lost, which is consistent with the additive-data rule. Nothing in the frontend still reads the field, so no consumer silently receives `undefined`. ADR-0031 records the retirement explicitly. The old pin was therefore obsolete, not correct.
  - **Suggested resolution:** Adapt the oracle; there is no code fix to make.

- **OR3**
  - **Path:** `code:app/src-tauri/eo-services/src/tracker/session.rs:830`, `code:app/src-tauri/eo-services/src/tracker/tests.rs:5559`
  - **Dimension:** 3 and 4
  - **Severity:** `genuine-spec-move`
  - **Description:** `trackProtectionBySegment` disappears because per-segment protection attribution was retired, not because the field was dropped by accident. Session start no longer opens a protection interval. The session's stamped facet and view field are gone, as is the definition input and read. The `set_protection` and `declare_whole_session_protection` paths are deleted. The database column itself is kept, with every new session and definition writing a literal `0`, so it remains readable history rather than being dropped. `hits_carry_their_context_and_no_declared_protection` pins that behaviour even under a legacy definition authored with the flag set to 1: the new session stores 0, no protection interval opens, and defensive hits are still recorded with their context. This matches ADR-0031, where costs are spread by hit count at recording time, so no per-segment declaration is needed.
  - **Suggested resolution:** Adapt the oracle.

- **OR4**
  - **Path:** `code:app/src-tauri/eo-api/src/demo.rs:590`, `code:app/src-tauri/eo-api/src/tracking.rs:1652`
  - **Dimension:** 2 (minimality) and 3
  - **Severity:** `genuine-spec-move`
  - **Description:** The neighbouring field `trackProtectionCosts` is deliberately kept and still reads `false` in the demo. The curated demo still starts its session with `SessionFacets::default()`, and its fixture contains no armour cost and no defensive evidence. The reasoning in the earlier ratification of these fields therefore still holds. The field's meaning is now "defensive hits are recorded for later costing", and a false value remains a truthful description of the demo session. No value moved on a field the change was not about.
  - **Suggested resolution:** No remediation required.

- **OR5**
  - **Path:** `app/src-tauri/fixtures/corpus/scripted/*/expected/http_responses/GET_tracking_snapshot.json`
  - **Dimension:** 1 and 2 (goldens that did not move)
  - **Severity:** `inconsequential`
  - **Description:** The seven corpus `GET_tracking_snapshot.json` goldens still contain `endOfSessionArmourReminderEnabled`, and it is correct that they do not move. They are frozen evidence from the retired HTTP transport, not projections of the live output: they record the old `/api/tracking/snapshot` route, carry `mobEntryMode` and `mobSource`, lack fields the live snapshot has carried for some time (`trackProtectionCosts`, `activities`, `healing`), and were last edited by a directory rename. Only `basic_hunt_10_events` has raw captures, and `eo-wire/tests/emitters_proof.rs:79` compares its HTTP goldens against the committed `raw_captures/http_responses.json`, never against live output. The live corpus oracle, `eo-services/tests/corpus_replay_oracle.rs`, compares only `fingerprint.jsonl` and `db_state.json`. The `db_state` catalogue (`eo-wire/src/db_snapshot.rs`) captures none of the protection-flag columns and none of the protection tables, which is why `defensive_combat_round` still matches even though defensive hits no longer write `protection_interval_id`.
  - **Suggested resolution:** No action required.

- **OR6**
  - **Path:** `app/src-tauri/eo-api/resources/demo_goldens/tracking_snapshot.txt:1`
  - **Dimension:** 6 (determinism)
  - **Severity:** `genuine-spec-move`
  - **Description:** The delta only removes content, so it adds no ambient input. The two removed values came from config and immutable session facets. The golden's existing time-relative values (`started_at`, `recentEvents[].timestamp`, `elapsed`) are unchanged and are already normalised by the demo comparator before the comparison.
  - **Suggested resolution:** No remediation required.

- **OR7**
  - **Path:** commit `5f5e9fc` message
  - **Dimension:** process (ratification guard)
  - **Severity:** `inconsequential`
  - **Description:** The `test: regenerate goldens` marker appears in the body of `5f5e9fc`. The guard matches the marker in any commit message in the range, so it is satisfied. The guard also requires this report to be committed no earlier than the last golden change, so it lands in a commit after `5f5e9fc`.
  - **Suggested resolution:** Commit this report on top of `a7ef82a`.

## Test evidence

Narrow, resource-bounded runs at `a7ef82a` (`--build-jobs 1 --test-threads 1`):

| Scope | Result |
|---|---|
| eo-api demo tests (including `demo_reads_reproduce_the_curated_goldens`), plus `tracking_facade` and `settings_facade` | passed |
| eo-wire `emitters_proof` | 3 of 3 passed |
| eo-services `corpus_replay_oracle` | 16 of 16 scenarios passed, including `defensive_combat_round` |
| Full eo-wire suite | 64 of 64 passed |
| Full eo-api suite, plus the eo-services `tracker`, `session_definitions`, `config_service` and `protection` modules | 298 of 298 passed |

## Judgement

The old golden is out of date because the two surfaces it described were deliberately retired: the end-of-session armour reminder and per-segment protection attribution. ADR-0031 records both decisions, and the code removes them end to end. Neither pin describes a regression that the code should instead be fixed to preserve. The delta is exact and minimal: removing the two fragments from the old golden reproduces the new golden byte for byte. The neighbouring `trackProtectionCosts` value stays truthful for the demo session. No other golden moved, and none should have. The unchanged corpus snapshot goldens are frozen evidence from the old transport, and the live corpus oracle does not compare against them. Nothing non-deterministic is introduced. This is a genuine change of specification, not a swept regression.

```text
ORACLE-RATIFICATION
range: origin/next..HEAD
goldens: tracking_snapshot
VERDICT: ratification-sound
```
