# Ratification: typed API response models

Adversarial review of the testing-oracle output that accompanies declaring
OpenAPI response schemas across the previously-untyped API read surface
(FastAPI `response_model=` added to ~94 routes; the response models authored in
`backend/routers/response_models.py`).

## Change under review

`response_model=` was attached to 94 routes that previously emitted FastAPI's
untyped `{}` placeholder, across the quests, codex, equipment, settings,
recording, scan-manual, analytics, health, character, tracking, and demo
routers. No handler logic changed: only route decorators and the new Pydantic
models. Every model subclasses the existing `_Loose` base (`extra="allow"`), so
un-enumerated handler keys pass through untouched; the polymorphic / divergent
200s use `response_model_exclude_unset=True`; `response_model_exclude_none` is
used nowhere. `GET /api/scan/skills/capture/{page}` (which returns `image/png`,
not JSON, and is reached by a hand-built URL rather than the generated client)
was removed from the documented surface via `include_in_schema=False`.

## Oracle delta reviewed

- `backend/tests/expected/openapi.snapshot.json` (regenerated): an object-level
  comparison confirms the delta is strictly additive save one justified removal.
  87 component schemas added, 0 removed, 0 existing schema's content changed, 0
  paths added, 1 path removed (the misdescribed PNG route). Typed 200 responses
  go from 5 to 99. No non-200 response, parameter, requestBody, or operationId on
  any surviving operation changed.
- No other golden moved in the range: the per-scenario HTTP-response
  fingerprints, the event-stream `fingerprint.jsonl` / `db_state.json` goldens,
  the consistency `.yml` goldens, and `COVERAGE.md` are all unchanged.

## Verdict

The delta is a faithful, response-preserving documentation of the existing read
surface, not a regression pinned as the new truth. Response-preservation is
proven by the 10 GET x 7 scenario HTTP-response fingerprint goldens staying
byte-identical (including the byte-pinned tracking sessions / session-detail /
quest-link-suggestion, scan/skills/status, codex/meta/attributes, and quests
readouts) and by the full schemathesis contract tier (`test_api_contract` +
`test_api_contract_with_state`) passing against the typed code. Field names,
casing (the deliberate per-surface snake/camel mix), and nullability are
reproduced per-handler; the `_Loose` base prevents silent field-drop; and the
one subtle type choice (`SessionDetail.skillGains.level` / `ttValueGained` typed
`int | float`) is the correct preservation of `round()`'s integer-zero wire form,
proven by the byte-identical session-detail golden. The PNG path removal corrects
a route the old snapshot actively misdescribed as JSON. Determinism is intact: no
ambient / time / random tokens appear in any added schema, and the snapshot is a
pure function of the decorators and models.

## Amendment: numeric-form fixes

A follow-up commit regenerated the snapshot once more with exactly three schema
changes, independently re-reviewed: (1) `HpOptimizerResult.currentHp` moved from
`integer` to `number`; the handler emits `round(current_hp, 2)`, fractional for
any calibrated HP-contributing skill, so the old integer pin was itself the
regression (a response-validation 500), now locked by an HTTP-layer regression
test that exercises the route through the real serialisation stack. (2)
`SessionSkillGain.level` / `ttValueGained` narrowed from `anyOf[integer, number]`
to `number`; the producer genuinely emits an integer zero on the wire and the
session-detail expectations pin it, but the comparison is value-based, so the
narrowing was adversarially verified to break no pin (fingerprint scenarios pass
with those expectations byte-identical) and to reject no real response (JSON
Schema `number` admits integers; the contract suite passes). (3) The `Quest`
schema description dropped an internal helper reference, text only. The
amendment's golden delta is exactly these three changes and nothing else; the
cumulative range remains response-preserving and the verdict carries forward.

```text
ORACLE-RATIFICATION
range: c84cac6..HEAD
goldens: backend/tests/expected/openapi.snapshot.json
VERDICT: ratification-sound
```
