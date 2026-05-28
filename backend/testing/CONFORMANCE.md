# HTTP / OpenAPI conformance surface

The conformance surface pins the backend's read API at the HTTP layer:
every hydration GET endpoint carries a strong ETag, mandates
revalidation via `Cache-Control: no-cache`, and answers a matching
`If-None-Match` with `304 Not Modified`. The same endpoints have
per-scenario response goldens so the body shape pinned by the OpenAPI
spec is also pinned by an end-to-end snapshot of what the route
actually returns under replayed gameplay state. The OpenAPI spec
itself is golden-tracked so a silent drift between the documented
contract and the runtime shape surfaces in CI.

## Components

### ETag + Cache-Control substrate (`backend/middleware/etag.py`)

An HTTP middleware applied to GET responses under the hydration
prefixes:

- `/api/tracking`
- `/api/scan`
- `/api/quests`
- `/api/codex`

A 2xx response is wrapped with:

- `ETag: "<sha256-hex>"`: strong, RFC 7232 quoted form, computed over
  the serialised response body. Equal bodies yield equal ETags
  regardless of route or process.
- `Cache-Control: no-cache`: mandate revalidation. The substrate's
  goal is body-skip, not network-skip; the frontend still polls, and
  the backend still computes, but `304 Not Modified` carries no body.

A request whose `If-None-Match` header matches the freshly-computed
ETag is answered `304 Not Modified` with an empty body, carrying the
same `ETag` + `Cache-Control` headers plus any cross-cutting headers
inner middleware added (CORS, Vary, Date) per RFC 7232 §4.1.

Routes outside the four prefixes pass through unchanged. The narrow
remit is asserted in `test_etag.py::test_routes_outside_the_hydration_prefixes_carry_no_etag`
so a future change that widens `ETAG_PREFIXES` surfaces against the
contract.

### HTTP fingerprint apparatus (`backend/testing/http_fingerprint.py`)

For each scripted scenario, the apparatus captures (request,
normalised-response) pairs against per-endpoint goldens under
`<scenario>/expected/http_responses/<endpoint_id>.json`. The same
`Normalizer` the per-scenario `fingerprint.jsonl` and `db_state.json`
already use is shared with the HTTP-fingerprint run, so UUIDs and
timestamps land on identical symbols across the three surfaces. That
makes a diverged kill UUID readable as "session `<UUID_1>` lost a kill"
in either the event-stream or the HTTP response view.

The response projection:

- **Status code**: literal.
- **Headers**: a curated set: `Content-Type`, `Cache-Control`, `ETag`.
  ETag is projected as the sentinel `"<STRONG_ETAG>"` when it matches
  the strong-format pattern; any other shape (weak, malformed, empty)
  is kept verbatim so an unexpected shape surfaces as a divergence.
- **Body**: JSON bodies are parsed and walked through the `Normalizer`.
  Empty bodies become `null`. Binary bodies are projected as
  `{"_binary": true, "byte_length": N}` so the byte-shape is pinned
  without embedding raw bytes (which would defeat the normalisation
  rationale for JSON).

The path string itself is also normalised, so the same UUID embedded
in `/api/tracking/session/<id>` and in the response body
`"sessionId"` field resolves to the same `<UUID_N>` symbol.

### OpenAPI drift detection (`backend/tests/test_openapi_drift.py`)

`app.openapi()` is snapshotted against
`backend/tests/expected/openapi.snapshot.json`. The default mode
asserts equality; `--update-fingerprints` flips into write mode after
surfacing the unified diff so a ratification is deliberate rather
than mechanical.

Companion sanity check
`test_openapi_get_surface_carries_expected_prefixes` asserts the four
hydration prefixes are still present in the spec so a refactor that
silently drops a router (which would shrink the golden) surfaces
loudly.

### Contract suite over replayed state (`backend/tests/test_api_contract_with_state.py`)

Sibling to the foundation `test_api_contract.py`. The foundation
suite exercises every GET against a freshly-booted app, which catches
the regressions that depend only on the spec and the empty-state
shape. The remaining failure mode is a regression that only manifests
once the backend holds non-empty state: a session-scoped field the
spec declares optional but the runtime always emits, an analytics row
that fails because a join surfaces an unexpected null, and so on.

The state variant boots the same lifespan, then drives the
`basic_hunt_10_events` scripted scenario through the production
tracker before schemathesis collection runs. The check set is
identical to the foundation suite (`not_a_server_error` on every
response; `response_schema_conformance` on 2xx) so a spec-conformance
regression that only appears with state surfaces here.

## Golden workflow

- `pytest backend/tests/e2e/ -k http_fingerprint` asserts every
  per-scenario HTTP-response golden under default mode.
- `pytest backend/tests/test_openapi_drift.py` asserts the OpenAPI
  golden.
- `pytest backend/tests/e2e/ -k http_fingerprint --update-fingerprints`
  rewrites the HTTP-response goldens; the same flag rewrites the
  fingerprint.jsonl + db_state.json goldens for the same scenarios in
  one pass.
- `pytest backend/tests/test_openapi_drift.py --update-fingerprints`
  rewrites the OpenAPI golden, surfacing the diff inline.

Three guardrails push back against reflex ratification: the default
mode fails on diff, update mode is gated behind an explicit CLI flag,
and update mode prints the diff before writing.

A regeneration commit follows the goldens-regeneration commit-message
convention in [TESTING.md](../../TESTING.md#commit-message-convention).

## Future event-driven hydration hook

When the frontend store layer adopts an event-driven hydration model,
the closing-signal assertion is one line on top of this substrate: a
scenario hits the snapshot endpoint, captures its ETag, then issues
`If-None-Match: "<that hex>"` and asserts the body is empty + the
status is 304. The current goldens already pin the substrate is
engaged on every snapshot endpoint, so the only new contract at that
point is the frontend's discipline at sending `If-None-Match`. The
deeper closing signal ("FastAPI access log stays empty after the
dashboard mounts") needs no new harness piece either: a router
access-log probe in the e2e fixture, hit-counted at the test's
boundary, will close that property.

## Conformance scope (intentionally narrow)

- POST/PUT/PATCH/DELETE routes are not under the substrate. Mutating
  endpoints have no idempotent body, the validation flow is
  request-shape-driven, and the OpenAPI contract suite already pins
  their response models.
- Routes outside the four hydration prefixes (`/api/health`,
  `/api/character`, `/api/equipment`, `/api/analytics`,
  `/api/settings`, `/api/demo`, `/api/recording`) are not under the
  substrate. They poll at user-driven cadence rather than the
  always-on dashboard cadence, and the ETag substrate's body-skip
  value drops to negligible against routes that change rarely.
- ETag values are not pinned in goldens; the body normalisation
  already pins identity. The headers track only that the substrate
  is engaged, in the right shape, on the right routes.
