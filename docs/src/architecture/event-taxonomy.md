# Event taxonomy

EntropiaOrme runs an analytical desktop application whose backend observes a stream of game-state changes (parsed chat-log lines, manual skill scans) and pushes the resulting changes to the frontend windows without polling. Internally this is built as **two distinct event systems**, layered so that each solves a different problem. This page documents both layers end to end: from the low-level, synchronous, in-process topics that backend services use to coordinate, up to the coarse typed envelopes that cross the process boundary and reach the webview over server-sent events (SSE).

The backend was ported from a Python FastAPI sidecar to a native Rust HTTP spine. The two-layer shape exists in both languages: the Python implementation lives under `backend/core/` and `backend/services/`, and the native equivalents live under `frontend/src-tauri/eo-wire/src/`. Where the two diverge, the divergence is called out below.

For the wider context of where this fits, see the [architecture overview](overview.md) and the [service map](service-map.md). The two design decisions that shape this layer are recorded as [ADR 0002: the event spine](../adr/0002-event-spine.md) and [ADR 0009: push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Two layers, and why they are separate

The system has two event layers with deliberately different shapes and audiences.

| | Low-level in-process bus | Domain event envelopes |
| --- | --- | --- |
| Defined in | `backend/core/event_bus.py`, `backend/core/events.py` | `backend/core/domain_events.py` |
| Topic form | string constants (`"combat"`, `"loot_group"`, ...) | dotted domain strings (`"tracking.session.updated"`, ...) |
| Payload | loose `dict` (`Any`-typed) | a closed Pydantic v2 model instance |
| Granularity | one raw mutation (a single parsed combat line, one loot group) | one coarse change ("the live session changed") |
| Audience | other backend services in the same process | the frontend, over the SSE bridge |
| Crosses the process boundary? | never | yes, serialised to JSON |
| Dispatch | synchronous, on the publishing thread | synchronous publish on the bus, then hopped to the event loop and fanned out |

The **low-level bus** is intra-backend wiring. As the module docstring in `backend/core/domain_events.py` puts it, a frontend window does not want "a damage_dealt combat line", it wants "the live session aggregates changed". The raw topics are at the wrong granularity to push to a webview: they are numerous, fine-grained, and carry backend-shaped dictionaries (snake_case keys, raw float timestamps) that have no business on a public wire.

The **domain event layer** is the coarse, frontend-facing subset. Each domain event is a typed envelope: a Pydantic model carrying a `type` discriminator, so the wire format is a serde-compatible tagged JSON object. The set of domain events is small and curated; the low-level topics stay inside the process and are never forwarded.

The two layers do share one piece of plumbing: typed domain envelopes are published on the *same* `EventBus` instance as the loose low-level topics. The bus's `publish` is `Any`-typed, so "a typed instance on a domain topic" is a producer-side convention the Python type checker cannot enforce. The SSE hub re-validates at runtime for exactly this reason (see below). The native backend removes this ambiguity: it gives the domain events their own monomorphic channel, described in the [native channel shape](#native-divergence-a-monomorphic-channel) section.

### The bus mechanics

The bus in `backend/core/event_bus.py` is a thread-safe synchronous pub/sub:

- `subscribe(event_type, callback)` / `unsubscribe(...)` register per-topic callbacks. Subscription is per-topic by design.
- `publish(event_type, data)` snapshots the subscriber list (and the taps) under an `RLock`, then dispatches outside the lock. Each callback runs synchronously on the publisher's thread.
- A subscriber that raises does not break dispatch: the bus catches and logs the exception, then continues to the next subscriber.
- `add_tap(tap)` installs a **full-stream observer** called with every `(event_type, data)` pair that crosses `publish`, regardless of topic, before subscriber dispatch. Because subscription is per-topic, a tap is the only supported way to observe the complete publish stream (new topics included). Taps are likewise exception-contained.

## Low-level topics

The string constants in `backend/core/events.py` name the intra-backend topics. They are grouped by source.

| Constant | Topic string | Meaning |
| --- | --- | --- |
| `EVENT_COMBAT` | `combat` | A parsed combat line from chat.log (damage dealt or taken). |
| `EVENT_LOOT_GROUP` | `loot_group` | A tick's worth of loot lines, grouped into one event. |
| `EVENT_SKILL_GAIN` | `skill_gain` | A skill-gain line parsed from chat.log. |
| `EVENT_ENHANCER_BREAK` | `enhancer_break` | An enhancer-break line parsed from chat.log. |
| `EVENT_GLOBAL` | `global` | A global / hall-of-fame broadcast line. |
| `EVENT_ACTIVE_TOOL_CHANGED` | `active_tool_changed` | The active hotbar tool changed. |
| `EVENT_ACTIVE_HEAL_TOOL_CHANGED` | `active_heal_tool_changed` | The active heal tool changed. |
| `EVENT_SESSION_STARTED` | `session_started` | A tracking session started. |
| `EVENT_SESSION_STOPPED` | `session_stopped` | A tracking session stopped. |
| `EVENT_MISSION_RECEIVED` | `mission_received` | A mission was received. |
| `EVENT_TICK_FLUSHED` | `tick_flushed` | The settling boundary: a parse tick has closed and every per-event subscriber write for that tick has completed. |

### The tick_flushed settling boundary

`EVENT_TICK_FLUSHED` is special. chat.log timestamps have one-second precision, so all recognised lines sharing a timestamp are treated as one application "tick". The chat-log watcher (`backend/services/chatlog_watcher.py`) buffers a tick's events and, when the timestamp advances or the file goes idle, flushes them: loot lines become a single `EVENT_LOOT_GROUP`, other events are published individually. After **every** per-event publish for that tick has been dispatched (and its subscribers have mutated state synchronously), the watcher publishes `EVENT_TICK_FLUSHED` last, carrying the tick's timestamp.

This is purely intra-backend, like the other `EVENT_*` constants. Its purpose is to give a stateful subscriber (the tracker) a single, well-defined moment to coalesce a tick's worth of low-level mutations into one coarse domain event, rather than emitting one domain event per raw mutation. The coalescing is described under [Producers and coalescing](#producers-and-coalescing).

## Domain event envelopes

The frontend-facing domain events are defined in `backend/core/domain_events.py`. Two concrete envelope types exist today.

### The shared envelope shape

Both envelopes carry the same three top-level fields, then a typed `payload`:

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `Literal[...]` discriminator | The domain-topic string, verbatim (`"tracking.session.updated"`). This doubles as the discriminator and as the topic the relay re-emits, so the bus-topic to envelope mapping is identity. |
| `event_version` | `int` (default `1`) | A per-event-type schema version. An additive-only field change bumps it. It is independent of the application version, so the frontend and a native emitter can reason about shape evolution without coupling to the app version. |
| `occurred_at` | `str` (required, **non-nullable**) | An ISO-8601 UTC timestamp for the instant the change occurred. |
| `payload` | a closed model | The event-specific body (see below). |

#### occurred_at is required and never null

`occurred_at` is a **required** envelope field and is never `null` and never the bus's raw float. The schema golden in `backend/tests/expected/event_schemas.snapshot.json` confirms this: for both envelopes, `occurred_at` is `"type": "string"` and appears in the `required` array, with no null branch.

An emitter whose domain carries no instant for the change (for example a settled tick that has no timestamp) synthesises one from its injected clock rather than threading `None` to the wire. The helper `to_iso_utc(ts)` in `backend/core/domain_events.py` renders a Unix timestamp (a SQLite REAL or a bus float) as ISO-8601 UTC. It is re-implemented in `core` rather than imported from the routers layer so that `core` carries no upward dependency on `routers`. The native mirror in `frontend/src-tauri/eo-wire/src/domain_events.rs` likewise types `occurred_at` as a required `String`.

#### The closed-schema rule

Both the envelope models and their payload models set `extra="forbid"` (via the shared `_EventModel` base, `model_config = ConfigDict(extra="forbid")`). The backend is the *emitter* and constructs these explicitly, so the wire contract is closed: an undeclared key is a bug that the schema-drift golden must catch. This is the deliberate opposite of the read-surface loose base used for handler passthrough. Payload field names are spelled camelCase literally (no alias generators), so snake_case keys and float timestamps cannot leak onto the wire. The schema golden records this as `"additionalProperties": false` on every `$def`.

The native side enforces the same closure with `#[serde(deny_unknown_fields)]` on every struct in `frontend/src-tauri/eo-wire/src/domain_events.rs`, and its tests assert that an extra payload key, an extra envelope key, a missing `type` tag, and a foreign `type` tag are all rejected.

### `tracking.session.updated`

`TrackingSessionUpdated` fires when the session aggregates changed: the session started, advanced a tick, or stopped. Its payload (`TrackingSessionUpdatedPayload`):

| Payload field | Type | Notes |
| --- | --- | --- |
| `sessionId` | `str \| None` (default `None`) | Which session changed. Serialises as `null` when absent, never omitted. |
| `status` | `Literal["active", "idle"]` | The coarse session state, so a subscriber can route on it without parsing the body. |
| `reason` | `Literal["started", "updated", "stopped"]` | Why the event fired. |

The `sessionId` nullability is mirrored exactly in Rust: `session_id: Option<String>` with `#[serde(rename = "sessionId", default)]`, and a dedicated test asserts a `None` value serialises as `"sessionId":null` rather than being dropped. In the schema golden, `sessionId` has an `anyOf` of string and null with a `null` default, and is absent from the payload's `required` list (only `status` and `reason` are required).

### `scan.status.changed`

`ScanStatusChanged` fires when the manual skill-scan status changed: a phase transition, or a capture / OCR progress step. Its payload (`ScanStatusChangedPayload`):

| Payload field | Type | Notes |
| --- | --- | --- |
| `phase` | `Literal["idle", "capturing", "processing", "awaiting_review"]` | The coarse scan phase. The only payload field. |

`phase` is the sole field and is required (confirmed by the `required: ["phase"]` entry and the four-value enum in the schema golden). The Rust mirror uses a `ScanPhase` enum with `#[serde(rename_all = "snake_case")]`, so `awaiting_review` round-trips byte-for-byte.

### The discriminated union

The two envelopes form a discriminated union:

```python
DomainEvent = Annotated[
    TrackingSessionUpdated | ScanStatusChanged,
    Field(discriminator="type"),
]
```

`Field(discriminator="type")` selects the member by its `type` tag, so adding a new member changes neither the existing members nor the wire format. Every call site (the bus publish, the SSE serialiser, the schema golden) routes through this union unchanged. The schema golden records the union as a `oneOf` over the two `$def`s with a `discriminator` mapping keyed on `type`. The native union in `frontend/src-tauri/eo-wire/src/domain_events.rs` is a `#[serde(untagged)]` enum made exact by closed topic-tag fields, so a frame routes to the one variant whose `type` literal it carries and an unrecognised tag fails outright.

## The SSE hub and endpoint

The bridge that carries domain events out of the backend process has two parts: the hub (`backend/services/event_stream.py`) and the endpoint (`backend/routers/events.py`).

### The hub: fan-out broker

`EventStreamHub` subscribes to the domain topics on the bus and fans their serialised frames out to every connected SSE stream. The forwarded set is the tuple `DOMAIN_TOPICS = (TOPIC_TRACKING_SESSION_UPDATED, TOPIC_SCAN_STATUS_CHANGED)`; the legacy low-level `EVENT_*` topics stay intra-backend and are deliberately not forwarded.

The hard part is a thread boundary. `EventBus.publish` runs synchronously on whatever thread mutated state (for the tick-coalesced tracking event, that is the chat-log watcher's OS thread, not the uvicorn event-loop thread the SSE generators await on). The hub crosses the boundary in one place and one direction:

- `_on_domain_event` runs on the **publisher** thread. It validates the envelope (a non-`BaseModel` payload, or one lacking a string `type`, is logged and dropped rather than forwarded as an untyped frame: this is the runtime re-validation that compensates for the bus's `Any` typing). It then serialises the envelope to wire JSON via `model_dump_json()` (pure CPU, thread-safe) and hops the finished frame onto the loop with `loop.call_soon_threadsafe`. A closed loop during a shutdown race raises `RuntimeError`, which is suppressed and drops the frame.
- `_dispatch` runs on the **loop** thread. It assigns the frame's sequence number and fans the frame out to every connection's queue.

Because the connection registry, the sequence counter, and the per-connection queues are touched only on the loop thread, they need no lock of their own.

#### The frame format and sequence number

`_dispatch` builds each frame as:

```
id: {seq}\nevent: {topic}\ndata: {data_json}\n\n
```

- `id:` is a monotonic sequence number (`self._seq += 1`), shared across the whole process so a client can reason about gaps.
- `event:` carries the domain topic, so the frontend relay can route a frame without parsing its body.
- `data:` is the compact envelope JSON.

#### Bounded queues with drop-oldest

Each connection gets its own bounded queue, sized `DEFAULT_MAX_QUEUE = 256` frames. A stalled or slow webview reader cannot grow memory without limit: when its queue is full, `_offer` drops the **oldest** frame before enqueuing the new one. Drop-oldest (not drop-newest) is correct under push-to-pull: the newest frame is the one that triggers the freshest hydration, so it must never be the one discarded. A dropped frame is therefore self-healing: the next frame the reader does receive triggers a snapshot re-hydration that reflects every intervening change. The full-check then put runs only on the loop thread, so it is race-free.

The hub does **not** run an app-lifetime background task. Each connection's drain is owned by its own request lifecycle (the Starlette / uvicorn response). On shutdown, `close()` unsubscribes from the bus and drops all connections so a late publish cannot hop a frame onto a closing loop.

### The endpoint: `GET /api/events` (the oracle's transport)

In the shipped native application, domain frames reach the frontend over the in-process Tauri event bridge described under [The bridge and the frontend relay](#the-bridge-and-the-frontend-relay), not over HTTP. The `GET /api/events` endpoint described here is the Python oracle's transport, retained as the reference the native bridge reproduces. `backend/routers/events.py` exposes the stream at `GET /api/events` as a long-lived `text/event-stream`. The frame generator:

1. Registers its queue with the hub *before* the response begins streaming, so an event published the instant after the stream opens is delivered rather than raced away.
2. Yields an opening comment `: ready\n\n` on connect. This flushes the response headers and signals that the connection is registered.
3. Loops, awaiting the next frame with a timeout of `KEEPALIVE_SECONDS = 15.0`. On a real frame, it yields it. On timeout, it checks for client disconnect and, if still connected, yields a keep-alive comment `: keep-alive\n\n`. The keep-alive cadence stops the browser `EventSource` and any intermediary from treating a quiet connection as dead, without coupling the cadence to any domain-event rate.
4. In a `finally`, unregisters the queue from the hub on disconnect.

The response carries `Cache-Control: no-cache` (no intermediary caching of the stream), `Connection: keep-alive`, and `X-Accel-Buffering: no` (disables reverse-proxy buffering so frames flush immediately).

The endpoint sits **outside** the ETag hydration prefixes by design: the ETag middleware buffers a whole response body to hash it, which would never return on an unbounded stream. It is also excluded from the OpenAPI schema (`include_in_schema=False`): an infinite event stream is not a request/response operation the spec, the contract walk, or the generated TypeScript client can model.

### Native divergence: a monomorphic channel

The native backend keeps the same observable contract but tightens the internal shape. `frontend/src-tauri/eo-wire/src/bus.rs` defines a `DomainBus` whose typed `DomainEvent` envelopes travel on a dedicated `tokio::sync::broadcast` channel, so "a typed event on a domain topic" is a compiler-checked invariant on the producer side rather than a convention. Like the Python bus, it supports full-stream taps that run synchronously on the publishing thread before subscriber delivery, and a panicking tap is isolated per invocation so it neither takes the bus down nor blocks delivery.

The native hub in `frontend/src-tauri/eo-wire/src/sse.rs` reproduces the Python hub's observable semantics exactly: one bounded queue per client (default 256 frames) with drop-oldest on overflow, a process-monotonic sequence number assigned at dispatch and shared across every client's copy of a frame, and the identical frame format `id: N\nevent: <topic>\ndata: <json>\n\n` with the envelope JSON byte-identical to the Python `model_dump_json()`. The 15-second keep-alive and the `: ready` opening comment are SSE-transport concerns: they live with the Python oracle's HTTP handler, and in the shipped native application, where the hub is drained by an in-process bridge rather than served over HTTP, they do not apply.

## Producers and coalescing

Two backend services produce domain events. Both share a discipline: **they publish only after releasing their own lock**, so a subscriber never runs while the producer holds its lock. (`EventBus.publish` copies its subscriber list under the bus lock, then releases it before dispatch, so no bus-lock to producer-lock cycle can form.)

### The tracker

The tracker (`backend/tracking/tracker.py`) produces `tracking.session.updated`. It publishes one in three situations:

- **Session started.** `start_session` builds the new session state under the lock, then (after releasing it) does the DB insert and emits the started event with `status="active"`, `reason="started"`, stamped with the session start time.
- **Session stopped.** `stop_session` finalises the session, then emits the stopped event with `status="idle"`, `reason="stopped"`, stamped from the injected clock's `end_time`.
- **Tick advanced.** This is where `EVENT_TICK_FLUSHED` does its work. While a session is active, the tracker subscribes to `EVENT_TICK_FLUSHED`. Its handler `_on_tick_flushed` coalesces a settled tick's mutations into one `tracking.session.updated` (`reason="updated"`). Critically, it emits **only when the tick actually changed the live readout**: the P&L handlers set a `_session_dirty` flag, and the handler reads and resets that flag under the lock; if the flag is clear (a tick of unrelated chat traffic), nothing is published, so an idle tick does not wake every frontend listener. The event is stamped with the tick's own timestamp (already present on the tick's loot / combat events); a settled tick that carries no timestamp falls back to the injected clock, so the required `occurred_at` always names a real instant.

In all three paths, the published value is a typed `TrackingSessionUpdated` instance, not a dict, with `occurred_at` produced by `to_iso_utc(...)`. The `session_id` is captured under the lock and passed into the emit helper rather than re-read off `self._session`, so the published id provably belongs to the session whose mutation the event describes even if a concurrent `stop_session` has since cleared it.

### The manual-scan service

The manual skill-scan service (`backend/services/skill_scan_manual.py`) produces `scan.status.changed`. It has no external tick boundary like the tracker's, so it coalesces with an internal **settled-boundary key**. The helper `_publish_status` is called after releasing the lock at every settled mutation point (verb completion, each per-page OCR step, worker completion). Under the lock it computes a `_status_key()` projection of the owned state and compares it to the last published key (`_last_emitted_key`); it advances the key and publishes only when the key actually moved. This coalesces to one frame per discrete status change rather than one per call, and ensures the main and worker threads cannot both emit the same transition. The key is baselined at construction to the resting idle status, so the redundant initial idle frame is suppressed (listeners hydrate the idle status via the GET on mount).

The payload carries only the coarse `phase`. Per-page capture / OCR progress liveness rides the snapshot re-hydration the frame triggers, rather than widening the wire: the emitter fires on every discrete progress change, but the payload stays a minimal invalidation signal.

### Why the payloads are minimal: push-to-pull

Both producers follow the **push-to-pull** model (see [ADR 0009](../adr/0009-push-to-pull-invalidation.md)). A payload is a minimal invalidation signal (which session, active versus idle, why; or which scan phase), and the window re-hydrates the full shape via the matching snapshot GET. This keeps the ETag / 304 snapshot as the single source of shape, minimises the serialisation surface, and is exactly what makes drop-oldest queue overflow safe.

## The bridge and the frontend relay

In the shipped application the producer spine's frames are forwarded onto the Tauri event bus **in-process** by the shell's domain-event bridge (`spawn_domain_event_bridge` in `frontend/src-tauri/entropia-orme/src/lib.rs`), the native replacement for the frontend's former `EventSource` relay. The bridge registers a consumer on the producer spine's hub, drains its frames, and applies the same two transforms the old relay did before re-emitting onto the bus, so every window (including hidden overlays) receives backend state changes by subscription rather than by polling.

- **The dot-to-colon rename.** Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots), so `domain_topic_to_tauri_event` namespaces the dotted wire topic with colons: `tracking.session.updated` is re-emitted as `tracking:session:updated`, and `scan.status.changed` as `scan:status:changed`. This is the **only** topic transform; the wire contract keeps the dotted form throughout.

- **Whole-envelope forwarding.** For each frame the bridge parses the `data` JSON and re-emits the **whole** typed envelope (`type`, `event_version`, `occurred_at`, `payload`) onto the colon-form Tauri topic, not just the payload, so a topic-aware consumer sees the full contract. A frame whose JSON fails to parse is logged and dropped.

The frontend half (`frontend/src/lib/realtime/eventRelay.ts`) now owns only the **re-hydrate nudge**. It listens for the `substrate:native-installed` event the shell emits once the native services compose, and fires a payload-less re-hydrate on each consumer's topic. Each topic-aware consumer (the tracking and scan stores, the overlay) subscribes through its typed topic and re-reads rather than reduces, so a payload-less frame reads as "re-hydrate" rather than as an idle session; this keeps a freshly live (or re-composed) backend from leaving a window showing stale data. The nudge stays frontend-owned because it must fire after the webview is listening, which an emit at install time cannot guarantee on a cold load. The relay returns a stop function the layout hands back to Svelte for teardown on window close.
