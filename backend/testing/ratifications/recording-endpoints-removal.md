# Ratification: recording-endpoints removal from the OpenAPI snapshot

Independent review of the OpenAPI spec snapshot regeneration that follows the
deliberate removal of the `/api/recording/*` surface. The reviewer is not the
change's author; this audit is from the evidence in the tree.

## Range and goldens under review

- Range: `origin/main..origin/refactor/native-ipc-collapse`.
- Code: the recording feature was dropped (commit `2f0e4c0`): the
  recording router include, the DI `Services` field, the lifespan construction
  and abort, the controller, the three response/request models, the three
  production recording tests, and the frontend recording UI / wrappers / types /
  test were all excised. The underlying replay infrastructure (`recorder.py`,
  `replay.py`, the clock, config, events sink, capturer, keystroke source) is
  retained and untouched, since it backs the test oracle, not the deleted HTTP
  endpoints.
- Goldens: `backend/tests/expected/openapi.snapshot.json` regenerated (269
  deletions, 0 additions), and `frontend/src/lib/api/schema.d.ts` regenerated to
  match.

## Findings

- **Delta accountability.** A structural comparison of the two snapshots
  confirms the delta is exactly four paths (`/api/recording/{abort,start,status,
  stop}`) and four schemas (`RecordingAbortResult`, `RecordingStatus`,
  `RecordingStopResult`, `StopRecordingBody`) removed. Every other path and
  schema is byte-identical; the `info` and `openapi` version blocks are
  byte-identical; the shared `HTTPValidationError` schema (referenced by the
  deleted stop endpoint's 422) is retained, not collaterally swept.
- **Minimality.** Zero additions in both the snapshot and the regenerated
  TypeScript client. The only operation keys removed from `schema.d.ts` are the
  four recording ones; every other removed line is an interior field of a
  deleted recording type. No collateral drift on any unrelated field.
- **Intended, not merely actual.** The endpoints were removed deliberately and
  coherently across the router include, the DI `Services` field, the lifespan,
  and the imports. The branch's own `PORTING-RULEBOOK.md` pre-declared this
  regeneration as a golden re-ratification by design, so the pin was moved
  consciously rather than snapshotted after the fact.
- **Runtime safety.** `backend.main` imports cleanly from the working tree;
  `Services` no longer carries `recording_controller`; the deleted files are
  gone. The retained service-layer line-tap hooks are optional attributes
  defaulting to `None`, not dangling references, so there is no 500 risk.
- **Replay infra separation.** The retained recorder / replay / golden / corpus
  machinery is genuinely distinct from the deleted HTTP surface: no live backend
  code reaches the deleted controller, and the remaining `backend.testing`
  imports are all oracle infrastructure.
- **Determinism.** The snapshot is a static contract document with no ambient
  input (no wall clock, randomness, environment, or timing), so no
  nondeterministic pin is possible.
- **Housekeeping (out of scope).** Stale documentation references to the deleted
  modules survive in `PORT-BASELINE.md`, `port_baseline.json`,
  `backend/testing/README.md`, and the `recorder.py` docstring. These carry no
  runtime or oracle impact and sit outside this gate; they are folded into the
  branch's standing stale-reference follow-ups.

## Verdict

The golden delta is a genuine, minimal, pre-declared spec move, not a regression
pinned as the new truth. The diff is provably nothing but the recording surface;
the code deletes that surface coherently with no dangling reference; the retained
replay oracle is genuinely separate; and the change was named in advance as a
deliberate re-ratification.

```text
ORACLE-RATIFICATION
range: origin/main..origin/refactor/native-ipc-collapse
goldens: openapi (backend/tests/expected/openapi.snapshot.json)
VERDICT: ratification-sound
```
