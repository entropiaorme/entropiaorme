# Ratification: clock seam and required `occurred_at`

Independent review of the testing-oracle output that accompanies making the
domain event `occurred_at` a required, non-null field and routing every backend
time read through an injected clock. The review re-derives the verdict against
the current tree rather than accepting the change author's rationale, because a
self-approved golden move carries a structural conflict of interest.

## Change under review

`occurred_at` on `TrackingSessionUpdated` and `ScanStatusChanged`
(`backend/core/domain_events.py`) moved from `occurred_at: str | None = None` to
`occurred_at: str`, and the envelope's `to_iso_utc` helper was made total
(`float -> str`, the `None` branch removed). Every emitter now supplies a real
instant: the tracker stamps the domain timestamp that triggered the event (the
tick instant or the session start/stop boundary) and falls back to its injected
clock only when a settled tick carries no timestamp
(`backend/tracking/tracker.py`), and the scan-manual service stamps its publish
instant through the same injected clock (`backend/services/skill_scan_manual.py`).
Both coalesced emitters fire only on a real mutation (the tracker's
`_session_dirty` gate; the scan service's `_last_emitted_key` de-duplication),
so the contract tightening adds no event on a no-op tick.

Separately, every production-package time read was routed through the injected
`Clock` (`backend/testing/clock.py`), each replay scenario was put on a committed
per-scenario clock plan (`backend/testing/clock_plan.py` + each scenario's
`metadata.yaml`), and a whole-tree no-ambient-time guard
(`backend/scripts/check_ambient_time.py`, asserted by
`backend/tests/test_ambient_time_guard.py`) was added. The marker commit for the
oracle regeneration is `5d78b2d`.

## Oracle delta reviewed

The expected-output set that moved in the range is exhaustively four files
(confirmed by name-filtering the range diff over `expected/` and `*.snapshot.json`
paths):

- `backend/tests/expected/event_schemas.snapshot.json` (regenerated): the only
  change is, on each of the two envelope `$defs`, `occurred_at` collapsing from
  `anyOf [{string}, {null}] + default: null` to a plain `"type": "string"`, and
  `occurred_at` being added to that envelope's `required` list. This snapshot is
  compared as canonical text (a byte move, not a value-equal refresh), so the
  delta is the genuine schema consequence. No payload field, no `event_version`,
  no `type` discriminator, no enum, and no other `$defs` entry changed; the
  schema delta is the complete and exact mechanical consequence of the field's
  type change (a non-optional field with no default renders as a required
  `string`) and contains nothing else.
- `basic_hunt_10_events/expected/http_responses/GET_tracking_session_detail.json`
  and `mission_completion_with_reward_suppression/.../GET_tracking_session_detail.json`:
  the only change is `level` and `ttValueGained` rendering as `0.0` rather than
  `0` on the single `skillGains` entry.
- `consistency_quests_mission_lifecycle_midpoint/.../GET_tracking_sessions.json`:
  the only change is `returns` rendering as `0.0` rather than `0`.

The five integral-zero re-renderings sit on fields the response models type as
`float` (`SessionSkillGain.level`, `SessionSkillGain.ttValueGained`, and
`TrackingSession.returns` in `backend/routers/response_models.py`). They are the
stale-parse artefact documented by rule W-3 in
`backend/architecture/PORTING-RULEBOOK.md`: a float-typed field with an integral
value serialises with a trailing `.0` on the live wire, while the
HTTP-fingerprint harness compares parsed bodies (`backend/testing/http_fingerprint.py`
walks `json.loads` output and asserts object equality), where `0 == 0.0`. The
prior on-disk `0` therefore still matched the live `0.0`; the regeneration merely
refreshed the committed form to the live bytes. The change is value-equal and is
not a numeric-behaviour change. No `occurred_at` value appears in any HTTP-body
golden (it is an event-stream envelope field, not a read-surface field), so the
field's de-null has no HTTP-body footprint, consistent with the observed delta.

No other golden moved: the per-scenario event-stream `fingerprint.jsonl` /
`db_state.json` goldens, the consistency `.yml` goldens, the remaining
HTTP-response fingerprints, and `openapi.snapshot.json` are all unchanged in the
range.

## Verdict

The delta is a genuine, intended specification move, not a regression pinned as
the new truth. The schema change is the complete and exact consequence of the
field's type tightening, with no collateral movement in any sibling field. The
tightening is correct rather than a faithful snapshot of a bug: both production
construction sites supply a real instant unconditionally (the tracker from the
domain timestamp, with an injected-clock fallback when a settled tick carries
none; the scan service from the injected clock), and both fire only on a real
mutation, so the required field can never be violated at runtime and no event is
emitted on a no-op tick. The prior nullable pin was therefore legitimately
obsolete, not a contract that should have held while the code regressed. The
five `0 -> 0.0` re-renderings are value-equal refreshes of the W-3 stale-parse
artefact on float-typed fields, confirmed value-preserving by the
HTTP-fingerprint scenarios passing against the regenerated goldens.

Determinism is intact and was reproduced rather than assumed. The synthesised
`occurred_at` instants route through the injected clock and `to_iso_utc` is a
pure transform of its float argument, so no wall-clock, randomness, environment,
or timing input leaks into any pinned value. The no-ambient-time guard passes
(eight cases). Regenerating both the schema snapshot and the HTTP-fingerprint
scenarios under their committed clock plans reproduced every committed golden as
an exact fixpoint (zero further diff), which confirms reproduction under the
plans and that no golden drifted silently outside the reviewed set. The
event-schema-drift contract, the event-stream contract, the HTTP-fingerprint
scenarios, and the affected scenario tests all pass against the regenerated
oracle.

```text
ORACLE-RATIFICATION
range: 4933253..HEAD
goldens: backend/tests/expected/event_schemas.snapshot.json, backend/tests/e2e/corpus/scripted/basic_hunt_10_events/expected/http_responses/GET_tracking_session_detail.json, backend/tests/e2e/corpus/scripted/mission_completion_with_reward_suppression/expected/http_responses/GET_tracking_session_detail.json, backend/tests/e2e/corpus/scripted/consistency_quests_mission_lifecycle_midpoint/expected/http_responses/GET_tracking_sessions.json
VERDICT: ratification-sound
```
