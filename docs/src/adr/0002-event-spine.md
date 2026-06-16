# ADR-0002: Two-layer event spine

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

Backend services share process memory and need to coordinate at a fine grain: a parsed combat line, a loot group, a skill gain, a settled parse tick. These are intra-backend wiring signals, string-typed and dict-shaped, at the wrong granularity to push to a webview. A frontend window does not want "a damage_dealt combat line"; it wants "the live session aggregates changed". The two concerns pull in opposite directions: in-process coordination wants an open, untyped, low-ceremony bus, whereas the push channel to the webview wants a closed, typed, versioned contract that survives the boundary and a future native re-emitter.

A second constraint is the thread boundary. State mutations run on whatever thread did the work (the chatlog watcher's OS thread for a coalesced tick), while the server-sent-events generators await on the uvicorn event loop. A push channel has to cross that boundary safely, and a slow or stalled webview reader must not be able to grow backend memory without bound.

## Decision

Two deliberately separate layers were adopted.

The low-level layer is a synchronous in-process bus. `EventBus.publish(event_type, data)` is `Any`-typed over string topics (the `EVENT_*` constants), runs subscriber callbacks inline on the publisher's thread (the `RLock` guards only the snapshot of the subscriber registry, not the dispatch itself), and contains any subscriber or tap exception so one bad listener cannot break dispatch. A full-stream tap is the only supported way to observe every topic.

The frontend-facing layer is a small, closed set of typed domain envelopes: `TrackingSessionUpdated` and `ScanStatusChanged`, joined by `Field(discriminator="type")` into the `DomainEvent` discriminated union. Each is a Pydantic v2 model with `extra="forbid"`, a `type` literal that doubles as both the discriminator and the bus topic (the mapping is identity), an additive-only `event_version`, a required ISO-8601 UTC `occurred_at`, and camelCase payload keys written literally. Payloads are minimal push-to-pull invalidation signals (which surface changed, and why), not full state; the window re-hydrates via the matching snapshot GET (see ADR-0009).

The seam between the layers is `EventStreamHub`. It subscribes to the domain topics, and on each publish `_on_domain_event` runs on the publisher thread: it serialises the envelope to wire JSON there (re-validating that the payload is a model, since the bus is untyped), then hops the finished frame to the loop via `loop.call_soon_threadsafe`. On the loop thread `_dispatch` assigns a monotonic sequence id, formats the SSE frame (`id`, `event`, `data`), and fans it to every connection's bounded `asyncio.Queue` (default 256, drop-oldest). `GET /api/events` (`backend/routers/events.py`) streams those frames; it sits outside the ETag middleware and the OpenAPI schema because an unbounded stream cannot be buffered or modelled as a request/response. The frontend relay (`frontend/src/lib/realtime/eventRelay.ts`), running only in the always-alive main window, opens one `EventSource` and re-emits each envelope onto the Tauri bus under its colon-form topic.

## Consequences

The closed envelope layer ports mechanically: `frontend/src-tauri/eo-wire/src/domain_events.rs` mirrors the Python models as a `serde` tagged union, and the relay contract is unchanged when the backend language changes. Cross-language wire equality is pinned: the Rust test asserts byte-identical output to `model_dump_json()`, and `backend/tests/test_event_schema_drift.py` asserts `DomainEventAdapter.json_schema()` against the tracked golden `event_schemas.snapshot.json`, fails on any casing or shape drift, and round-trips a value-level vector. Regeneration is deliberate (`pytest --update-fingerprints`).

Costs and constraints follow. A typed instance on a domain topic is a producer-side convention the bus cannot enforce, so the hub re-validates at runtime. Because frames are invalidation signals, drop-oldest is self-healing: a dropped frame is covered by the next hydration. There is no app-lifetime background task; each connection's drain is owned by its request lifecycle.

## Evidence

- `backend/core/event_bus.py`
- `backend/core/events.py`
- `backend/core/domain_events.py`
- `backend/services/event_stream.py`
- `backend/routers/events.py`
- `frontend/src/lib/realtime/eventRelay.ts`
- `frontend/src-tauri/eo-wire/src/domain_events.rs`
- `backend/tests/expected/event_schemas.snapshot.json`
- `backend/tests/test_event_schema_drift.py`
