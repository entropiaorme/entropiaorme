# ADR-0005: Cross-language equivalence oracle

- Status: Superseded by [ADR-0016](0016-retire-equivalence-oracle.md)
- Context: reflects the landed implementation; the port is now complete ([ADR-0013](0013-in-process-collapse.md): a single in-process Rust binary, superseding the strangler-fig topology of [ADR-0001](0001-strangler-fig-port.md)), and this oracle was retained after the port as the cross-language equivalence test harness.

> **Superseded.** This record describes the cross-language equivalence oracle that graded the Python-to-Rust port: the Python reference implementation retained as a test-only oracle, the shared normaliser reimplemented byte-for-byte on each side, and the live differential. The oracle has served its purpose and been retired; the equivalence evidence it produced is preserved as frozen, committed, Rust-side assertions (the byte-identical fingerprint and database-state goldens, the contract snapshots, and the normaliser conformance table), asserted with no second implementation present. See [ADR-0016](0016-retire-equivalence-oracle.md). The text below is preserved unchanged as the record of how equivalence was graded during the port.

## Context and problem statement

The backend was ported from a Python FastAPI sidecar to a native Rust spine ([ADR-0001](0001-strangler-fig-port.md)) one service at a time, with both implementations expected to serve identical behaviour at every externally observable surface. Reviewing a port by eye does not scale to that bar: the observable surfaces (event-stream frames, persisted database rows, HTTP response bodies, the route document, and the domain-event envelopes) interlock, and the failure modes are subtle. The same response mixes snake_case and camelCase keys per field, value fields render as floats with a trailing decimal point while counts render bare, Python's `round` is round-half-to-even on the binary value, and Python's `float.__repr__` and `json.dumps` produce byte forms that a naive Rust serialiser does not reproduce. A claim of equivalence needs a mechanical judge, not a human one.

The constraint that shapes the judge: equivalence must be decidable as **byte equality** of committed golden artefacts, so that a ported unit either reproduces the goldens exactly or fails visibly. That requires a single canonicalisation applied identically by both implementations, and a fixed projection of each surface that masks only genuine nondeterminism (identifiers, wall-clock instants, sub-precision float noise) and observes everything else verbatim.

## Decision

Equivalence is graded by interlocking golden oracles, one per observable surface, all reduced through one shared normaliser. The Rust half of the runner reimplements that normaliser byte-for-byte so a native backend's output can be compared against the Python-generated goldens directly.

The shared normaliser (`frontend/src-tauri/eo-wire/src/normalizer.rs`) canonicalises any JSON value: UUID-shaped strings become sequential `<UUID_N>` symbols, ISO-prefixed timestamps and bare epoch-window floats become `<TS_N>` symbols, other floats round half-to-even to four decimal places, and object keys sort lexically. One `Normalizer` instance is threaded across all surfaces, so an identifier first seen on the event stream and later in a database column resolves to the same symbol everywhere; encounter order, not surface, assigns the number. Crucially the normaliser owns its own serialisers rather than deferring to a JSON library: `python_repr_f64` reproduces Python's shortest-round-trip float rendering (the fixed-vs-scientific threshold, the trailing `.0` on integral floats, the zero-padded signed exponent), and `to_python_json` reproduces `json.dumps(sort_keys=True, ensure_ascii=False)` in both compact and indented forms. A separate `to_wire_json` reproduces the live wire serialisation (insertion-order keys, compact separators) for byte-level checks the goldens deliberately mask.

The per-surface oracles, all built on that normaliser:

| Surface | Emitter | Golden form |
| --- | --- | --- |
| Event-stream frames in publish order | `frontend/src-tauri/eo-wire/src/fingerprint.rs` | one compact `{"payload", "topic"}` line per event, sorted keys, trailing newline |
| Persisted tracking, quest, codex, and skill rows | `frontend/src-tauri/eo-wire/src/db_snapshot.rs` | per-table named-column selects in a fixed deterministic order, indented JSON |
| Per-endpoint HTTP responses | `frontend/src-tauri/eo-wire/src/http_fingerprint.rs` | status, three projected headers, normalised body, indented JSON |
| The route document | `backend/tests/expected/openapi.snapshot.json` | byte-pinned OpenAPI snapshot, also the typed-client codegen input |
| Domain-event envelopes | event-schema snapshot | schema and value pins |

`fingerprint.rs` walks the recorded `(topic, payload)` stream in publish order, normalising each payload while leaving the topic verbatim. `db_snapshot.rs` carries the catalogue verbatim: explicit column lists (never `*`), `COALESCE(heal_cost, 0.0)` and `COALESCE(dangling_cost, 0.0)` so a NULL renders as float `0.0`, and a heterogeneous per-table ordering taken from the catalogue rather than generalised (`tracking_sessions` and `ledger_entries` by `rowid`; `kills` and `notable_events` by `timestamp` then `rowid`; the joined child tables by the parent kill's `timestamp`). `http_fingerprint.rs` projects a narrow fixed axis set: the status code, the three lower-cased headers `content-type`/`cache-control`/`etag` (a strong ETag reduces to a `<STRONG_ETAG>` sentinel, anything else stays verbatim and surfaces as a diff), the body walked through the shared normaliser with any numeric `duration` reduced to a `<SESSION_DURATION>` sentinel, UUID path segments symbolised, an empty body rendered as `null`, and a non-JSON body projected to `{"_binary": true, "byte_length": N}`. The OpenAPI snapshot doubles as the codegen input for the typed client, so a route change that drifts the document also breaks the generated client.

A ported unit that reproduces the same fingerprints from the same scenarios is equivalent by construction over the paths those scenarios exercise: the goldens are bytes, and equal bytes mean equal observable behaviour.

## Consequences

The benefit is that equivalence becomes mechanical and reviewable. A divergence at any pinned surface shows up as a byte diff rather than as an unnoticed behaviour change, and the same goldens that grade the Python reference grade the port, so neither side gets a private definition of correct.

The costs and the constraints it now enforces:

- The Rust normaliser is held to byte equality with the Python original by differential testing: a differential fuzz ranges over the JSON wire domain (including both 64-bit integer extremes), and the in-crate unit tests in `normalizer.rs` pin the load-bearing cases directly (the trailing `.0` on `15.0`, ties-to-even at the fourth place such as `0.03125` to `0.0312`, the epoch-window symbolisation, the scientific-notation threshold). `round_half_even` is exposed so ported services round intermediate figures the Python way, keeping their values bit-identical when they later fold into a golden.
- The oracle certifies only the code paths the scenarios exercise. A Python branch no scenario drives is a place the port can diverge with every golden still green, so branch coverage on the ported side is measured and compared, not assumed, and goldens are never regenerated to make a port pass.
- The surfaces the projection deliberately drops carry no end-to-end byte pin and so carry explicit port-side verification duties instead: CORS headers and the origin-guard 403 bodies (outside the HTTP projection), business-rule status codes and detail strings (the scenarios pin 200 paths), and the live wire's raw byte form, which the goldens mask but the strong ETag (a hash of the exact body bytes) still observes in production. The full enforcement map, per surface and per rule, lives in `backend/architecture/PORTING-RULEBOOK.md`.

The open read models that let the goldens capture undeclared keys are covered by [ADR-0010](0010-loose-response-models.md).

## Evidence

- `backend/architecture/PORTING-RULEBOOK.md`
- `frontend/src-tauri/eo-wire/src/normalizer.rs`
- `frontend/src-tauri/eo-wire/src/fingerprint.rs`
- `frontend/src-tauri/eo-wire/src/db_snapshot.rs`
- `frontend/src-tauri/eo-wire/src/http_fingerprint.rs`
- `backend/tests/expected/openapi.snapshot.json`
