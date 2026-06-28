# Ratification: codex unclaim route (OpenAPI contract)

Adversarial review of the OpenAPI snapshot golden change that accompanies the
new codex "unclaim" endpoint. The review re-derives the verdict against the
current tree rather than accepting the change author's rationale, because a
self-approved golden move carries a structural conflict of interest.

## Change under review

A new full-stack feature reverts a species' most recent codex rank claim. The
FastAPI surface (the OpenAPI contract source that types the frontend client)
gains one endpoint and one request model: `POST /api/codex/unclaim` taking
`UnclaimRequest { species_name }`, backed by `unclaim_rank` in
`backend/services/codex_service.py`. The endpoint reuses the existing
`CodexClaimResult` response model, so it adds no new response schema.

## Oracle delta reviewed

The only changed golden in this range is
`backend/tests/expected/openapi.snapshot.json`. The diff is strictly additive
(55 insertions, zero deletions): the new `UnclaimRequest` schema and the new
`/api/codex/unclaim` POST path. The path's 200 response `$ref`s the existing
`CodexClaimResult`; its 422 `$ref`s the existing `HTTPValidationError`. No
pre-existing path, schema, field, ordering, or count was mutated or dropped, and
`CodexClaimResult` is unchanged.

## Adversarial review findings

- **Delta accountability.** Every added element maps to wired code: the
  `UnclaimRequest` schema mirrors the Pydantic model field-for-field; the path is
  produced by the `@router.post("/unclaim")` decorator under the `/codex` router
  registered at `/api` in `main.py`; the operationId, summary, description, and
  tag match FastAPI's auto-derivation.
- **Minimality.** `git diff --numstat` reports `55 0` for the snapshot; the only
  hunks are the two additive blocks. No collateral movement.
- **Reused response type.** The new path's 200/422 block is structurally
  identical to the established sibling `/api/codex/claim`, which also reuses
  `CodexClaimResult`/`HTTPValidationError`; the new endpoint inherits the house
  pattern rather than diverging.
- **Determinism.** The added output is fully static (path string, operationId,
  schema field names and types); no clock, randomness, environment, or
  machine-timing value leaks into the pinned contract.
- **Scope note.** This golden pins the HTTP contract surface only. The unclaim
  service semantics (the rank step-back, the calibration-row deletion, and the
  nothing-to-unclaim and concurrent-revert failure modes) are exercised by the
  unit tests in `backend/tests/test_codex_service.py`, not by this snapshot.

The snapshot delta is exactly the additive contract for one genuinely-wired
endpoint: minimal, additive, deterministic, and consistent with its sibling.

```text
ORACLE-RATIFICATION
range: e36b1bf..HEAD
goldens: backend/tests/expected/openapi.snapshot.json
VERDICT: ratification-sound
```
