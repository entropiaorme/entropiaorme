# ADR-0010: Descriptive response models

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The HTTP read surface returns JSON shapes that the frontend already consumes. Pinning each endpoint to a Pydantic `response_model` buys two things: a generated OpenAPI schema that the contract suite can validate real responses against, and a documented type for every field. The risk is that a model added after the fact silently changes the wire contract. Pydantic's default serialisation drops any key a model does not enumerate, so a model that fails to list a field would quietly truncate the response and break a consumer with no error at the boundary.

A second tension is shape. Several handlers are polymorphic under one route: `/tracking/snapshot` returns `unavailable`, `idle`, or `active`; the scan verbs return a full status or a bare `error`; the prospect and optimiser endpoints return a result branch or an error branch. A single model that declares every field as required would force the lean branches to emit a wall of explicit nulls they never carried before.

A third is typing. JSON has one number type, but the handlers compute a mix of integer-valued and fractional quantities, and the snapshot reuses readout dictionaries that were never uniformly cased.

These constraints pull the read surface in the opposite direction from the outbound domain event envelopes, which are constructed explicitly by the application as the emitter and must be closed.

## Decision

Read-surface models (`backend/routers/response_models.py`) are **descriptive, not prescriptive**: they describe shapes the handlers already return without adding, dropping, or changing any field. Three conventions make that hold:

- **Extra keys pass through.** Every model derives from `_Loose`, whose `model_config` sets `extra="allow"`, so a key a model does not enumerate is serialised untouched rather than dropped. Adding a model can therefore never truncate a response; the enumerated fields are simply the ones the contract pins a type to. `RepairScanResult` exploits this deliberately: its failure branch adds an undeclared `error` key that passes through, so the success branch never gains a null `error`.
- **Lean branches stay lean.** Polymorphic models mark every non-discriminator field optional, and their routes serialise with `response_model_exclude_unset=True` (see the `/tracking/snapshot` and quest-link routes in `backend/routers/tracking.py`, and the recording-stop route in `backend/routers/recording.py`). Only keys the handler actually set appear, so the `unavailable` and `idle` shapes do not gain explicit nulls for the active-session fields.
- **Numbers are typed by kind.** Value fields are typed `float` (the contract's number form); an integer-valued amount serialises in float form, which is value-identical to a JSON consumer. Genuinely integral fields (counts, ranks, identifiers) stay `int`.

The `TrackingSnapshot` model preserves casing exactly as the underlying readouts produce it: `session_id`, `started_at`, and `kill_count` stay snake_case while the headline numbers (`returnRate`, `damageDealtTotal`) stay camelCase. This is not an inconsistency to fix; it reproduces the wire contract the dashboard already reads, and the model documents it rather than rewriting it.

By contrast, the domain event envelopes in `backend/core/domain_events.py` set `extra="forbid"` on `_EventModel`. The application constructs these explicitly as the emitter, so the envelope is the product and any undeclared key is a bug the schema-drift golden must catch. The two bases are the deliberate opposites of each other: passthrough for what is read back, closed for what is sent out.

## Consequences

- A new read model is safe to add incrementally: it can never silently shrink a response, lowering the cost of typing the surface.
- The trade is that `extra="allow"` will not flag a key the handler emits but the model forgot. Type coverage of a field is opt-in per enumerated field, not enforced by exhaustiveness.
- Casing and number-kind conventions are documented at the model, so reviewers can read the intended wire shape directly rather than inferring it from handlers.
- Enforcement comes from the contract suite (`backend/tests/test_api_contract.py`). It drives every GET operation from the app's generated `/openapi.json` with schemathesis and asserts `not_a_server_error` on all responses plus `response_schema_conformance` on 2xx bodies, so a response that diverges from its declared model fails. Because conformance never pins a computed value, value-level oracles run alongside (for example `/api/health` returns exactly `{"status": "ok"}`, and the demo overview totals are pinned strictly positive) to catch a structurally valid but semantically wrong body.

The closed envelope discipline is what lets the snapshot remain the single source of shape under the push-to-pull invalidation model; see [ADR-0011: ETag conditional requests](0011-etag-conditional-requests.md) for the conditional-GET path that hydration rides on. The contract suite is one arm of the broader [cross-language equivalence oracle](0005-cross-language-equivalence-oracle.md) that keeps behaviour stable across the Python-to-Rust port. See the [ADR index](index.md) for the full set.

## Evidence

- `backend/routers/response_models.py`
- `backend/routers/tracking.py`
- `backend/routers/recording.py`
- `backend/core/domain_events.py`
- `backend/tests/test_api_contract.py`
