# Event taxonomy

EntropiaOrme is an analytical desktop application whose Rust core observes a stream of game-state changes (parsed chat-log lines, manual skill scans) and pushes the resulting changes to the frontend windows without polling. Internally this is built as **two distinct event systems**, layered so that each solves a different problem. This page documents both layers end to end: from the low-level, synchronous, in-process topics that core services use to coordinate, up to the coarse typed envelopes that cross to the webview over the in-process Tauri event bridge.

The two-layer shape is implemented in Rust under `frontend/src-tauri/`: the low-level bus lives in the `eo-services` crate (`frontend/src-tauri/eo-services/src/event_bus.rs`), and the domain envelopes and the fan-out hub live in the `eo-wire` crate (`frontend/src-tauri/eo-wire/src/domain_events.rs` and `frontend/src-tauri/eo-wire/src/sse.rs`). This began life as a Python FastAPI sidecar that was kept on only as a cross-language equivalence test oracle; that oracle has since been retired and its tree removed, so the Rust implementation is now the only one. The wire contract the two implementations shared is preserved as the committed schema snapshot the Rust types are asserted against.

For the wider context of where this fits, see the [architecture overview](overview.md) and the [service map](service-map.md). The two design decisions that shape this layer are recorded as [ADR 0002: the event spine](../adr/0002-event-spine.md) and [ADR 0009: push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Two layers, and why they are separate

The system has two event layers with deliberately different shapes and audiences.

| | Low-level in-process bus | Domain event envelopes |
| --- | --- | --- |
| Defined in | `frontend/src-tauri/eo-services/src/event_bus.rs` | `frontend/src-tauri/eo-wire/src/domain_events.rs` |
| Topic form | a `Topic` enum whose `as_str()` yields string constants (`"combat"`, `"loot_group"`, ...) | dotted domain strings (`"tracking.session.updated"`, ...) |
| Payload | a loose `serde_json::Value` | a closed, typed envelope struct |
| Granularity | one raw mutation (a single parsed combat line, one loot group) | one coarse change ("the live session changed") |
| Audience | other core services in the same process | the frontend, over the in-process bridge |
| Crosses to the webview? | never | yes, serialised to JSON |
| Dispatch | synchronous, on the publishing thread | synchronous publish on the bus, then re-validated and fanned out by the hub |

The **low-level bus** is intra-core wiring. As the domain-events module puts it, a frontend window does not want "a damage_dealt combat line", it wants "the live session aggregates changed". The raw topics are at the wrong granularity to push to a webview: they are numerous, fine-grained, and carry core-shaped values (snake_case keys, raw float timestamps) that have no business on a public wire.

The **domain event layer** is the coarse, frontend-facing subset. Each domain event is a typed envelope carrying a `type` discriminator, so the wire format is a serde-compatible tagged JSON object. The set of domain events is small and curated; the low-level topics stay inside the process and are never forwarded.

The two layers share one piece of plumbing: typed domain envelopes are serialised to JSON and published on the *same* `EventBus` instance as the loose low-level topics. `EventBus::publish` takes a `&serde_json::Value`, so "a typed envelope on a domain topic" is a producer-side convention the type system does not enforce at the bus seam. The fan-out hub re-validates at dispatch for exactly this reason: `subscribe_sse_bridge` (in `frontend/src-tauri/entropia-orme/src/composition.rs`) deserialises each value back into a `DomainEvent` and drops anything that fails, so a non-envelope value on a domain topic can never be forwarded as an untyped frame. The typed-ness is therefore guaranteed at both ends: the producer constructs a typed envelope before serialising, and the bridge re-validates before framing.

### The bus mechanics

The bus in `frontend/src-tauri/eo-services/src/event_bus.rs` is a thread-safe synchronous pub/sub:

- `subscribe(topic, callback)` returns a `Registration` handle; `unsubscribe(topic, registration)` removes it. Subscription is per-topic by design.
- `publish(topic, data)` snapshots the subscriber list (and the taps) under a `Mutex`, then dispatches outside the lock. Each callback runs synchronously on the publisher's thread.
- A subscriber that panics does not break dispatch: each callback runs inside `catch_unwind`, so a panic is contained and dispatch continues to the next subscriber.
- `add_tap(tap)` installs a **full-stream observer** called with every `(topic, data)` pair that crosses `publish`, regardless of topic, before subscriber dispatch. Because subscription is per-topic, a tap is the only supported way to observe the complete publish stream (new topics included). Taps are likewise panic-contained.

## Low-level topics

The variants of the `Topic` enum in `frontend/src-tauri/eo-services/src/event_bus.rs` name the intra-core topics; each one's `as_str()` yields the wire string. They are grouped by source.

| `Topic` variant | Topic string | Meaning |
| --- | --- | --- |
| `Combat` | `combat` | A parsed combat line from chat.log (damage dealt or taken). |
| `LootGroup` | `loot_group` | A tick's worth of loot lines, grouped into one event. |
| `SkillGain` | `skill_gain` | A skill-gain line parsed from chat.log. |
| `EnhancerBreak` | `enhancer_break` | An enhancer-break line parsed from chat.log. |
| `Global` | `global` | A global / hall-of-fame broadcast line. |
| `ActiveToolChanged` | `active_tool_changed` | The active hotbar tool changed. |
| `ActiveHealToolChanged` | `active_heal_tool_changed` | The active heal tool changed. |
| `SessionStarted` | `session_started` | A tracking session started. |
| `SessionStopped` | `session_stopped` | A tracking session stopped. |
| `MissionReceived` | `mission_received` | A mission was received. |
| `TickFlushed` | `tick_flushed` | The settling boundary: a parse tick has closed and every per-event subscriber write for that tick has completed. |

The same enum also carries the two frontend-facing domain topics (`TrackingSessionUpdated`, `ScanStatusChanged`), whose `as_str()` returns the dotted constants from `frontend/src-tauri/eo-wire/src/domain_events.rs`, because the typed envelopes ride this same bus before the hub forwards them.

### The tick_flushed settling boundary

`Topic::TickFlushed` is special. chat.log timestamps have one-second precision, so all recognised lines sharing a timestamp are treated as one application "tick". The chat-log watcher (`frontend/src-tauri/eo-services/src/chatlog_watcher.rs`) buffers a tick's events and, when the timestamp advances or the file goes idle, flushes them: loot lines become a single `Topic::LootGroup`, other events are published individually. After **every** per-event publish for that tick has been dispatched (and its subscribers have mutated state synchronously), the watcher publishes `Topic::TickFlushed` last, carrying the tick's timestamp.

This is purely intra-core, like the other low-level topics. Its purpose is to give a stateful subscriber (the tracker) a single, well-defined moment to coalesce a tick's worth of low-level mutations into one coarse domain event, rather than emitting one domain event per raw mutation. The coalescing is described under [Producers and coalescing](#producers-and-coalescing).

## Domain event envelopes

The frontend-facing domain events are defined in `frontend/src-tauri/eo-wire/src/domain_events.rs`. Two concrete envelope types exist today.

### The shared envelope shape

Both envelopes carry the same three top-level fields, then a typed `payload`:

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | a closed topic-tag field | The domain-topic string, verbatim (`"tracking.session.updated"`). This doubles as the discriminator and as the topic the relay re-emits, so the bus-topic to envelope mapping is identity. The tag serialises to exactly one literal and refuses any other input. |
| `event_version` | `i64` (default `1`) | A per-event-type schema version. An additive-only field change bumps it. It is independent of the application version, so the frontend can reason about shape evolution without coupling to the app version. |
| `occurred_at` | `String` (required, **non-nullable**) | An ISO-8601 UTC timestamp for the instant the change occurred. |
| `payload` | a closed struct | The event-specific body (see below). |

#### occurred_at is required and never null

`occurred_at` is a **required** envelope field and is never `null` and never the bus's raw float. In `frontend/src-tauri/eo-wire/src/domain_events.rs` it is typed as a plain `String` on both envelopes, with no `Option`. The schema snapshot in `frontend/src-tauri/contracts/event_schemas.snapshot.json` confirms this: for both envelopes, `occurred_at` is `"type": "string"` and appears in the `required` array, with no null branch.

An emitter whose domain carries no instant for the change (for example a settled tick that has no timestamp) synthesises one from its injected clock rather than threading a null to the wire. The helper `to_iso_utc(ts)` (in `frontend/src-tauri/eo-services/src/tracker.rs`) renders a Unix timestamp (a SQLite REAL or a bus float) as ISO-8601 UTC, so the required `occurred_at` always names a real instant.

#### The closed-schema rule

Every envelope struct and every payload struct carries `#[serde(deny_unknown_fields)]`, so the wire contract is closed in both directions: an undeclared key is rejected on the way in, and the core is the *emitter* that constructs these explicitly, so an undeclared key is a bug the schema-drift snapshot must catch. Payload field names are spelled camelCase via serde renames (no alias generators), so snake_case keys and float timestamps cannot leak onto the wire. The schema snapshot records this as `"additionalProperties": false` on every `$def`. The module's tests assert that an extra payload key, an extra envelope key, a missing `type` tag, and a foreign `type` tag are all rejected.

### `tracking.session.updated`

`TrackingSessionUpdated` fires when the session aggregates changed: the session started, advanced a tick, or stopped. Its payload (`TrackingSessionUpdatedPayload`):

| Payload field | Type | Notes |
| --- | --- | --- |
| `sessionId` | `Option<String>` (default `None`) | Which session changed. Serialised as `null` when absent, never omitted. |
| `status` | `TrackingStatus` (`active` / `idle`) | The coarse session state, so a subscriber can route on it without parsing the body. |
| `reason` | `TrackingReason` (`started` / `updated` / `stopped`) | Why the event fired. |

`session_id: Option<String>` carries `#[serde(rename = "sessionId", default)]`, and a dedicated test asserts a `None` value serialises as `"sessionId":null` rather than being dropped. In the schema snapshot, `sessionId` has an `anyOf` of string and null with a `null` default, and is absent from the payload's `required` list (only `status` and `reason` are required).

### `scan.status.changed`

`ScanStatusChanged` fires when the manual skill-scan status changed: a phase transition, or a capture / OCR progress step. Its payload (`ScanStatusChangedPayload`):

| Payload field | Type | Notes |
| --- | --- | --- |
| `phase` | `ScanPhase` (`idle` / `capturing` / `processing` / `awaiting_review`) | The coarse scan phase. The only payload field. |

`phase` is the sole field and is required (confirmed by the `required: ["phase"]` entry and the four-value enum in the schema snapshot). The `ScanPhase` enum uses `#[serde(rename_all = "snake_case")]`, so `awaiting_review` round-trips byte-for-byte.

### The discriminated union

The two envelopes form a discriminated union:

```rust
#[serde(untagged)]
pub enum DomainEvent {
    TrackingSessionUpdated(TrackingSessionUpdated),
    ScanStatusChanged(ScanStatusChanged),
}
```

The `#[serde(untagged)]` dispatch is made exact by the closed topic-tag fields: a frame routes to the one variant whose `type` literal it carries, and a missing or unrecognised `type` fails outright, so adding a new member changes neither the existing members nor the wire format. Every call site (the bus publish, the hub serialiser, the schema snapshot) routes through this union unchanged. The schema snapshot records the union as a `oneOf` over the two `$def`s with a `discriminator` mapping keyed on `type`. `DomainEvent::topic()` returns the variant's wire topic, and `to_wire_json()` yields the compact envelope JSON.

## The fan-out hub

Domain events leave the producer spine through a fan-out hub and reach the frontend over the in-process Tauri event bridge. The hub (`SseHub`, in `frontend/src-tauri/eo-wire/src/sse.rs`) is the broker; the bridge that drains it is described under [The bridge and the frontend relay](#the-bridge-and-the-frontend-relay).

### The hub: fan-out broker

`SseHub` is fed by `subscribe_sse_bridge`, which subscribes the two domain topics on the bus and dispatches each validated envelope into the hub. The forwarded set is exactly `[Topic::TrackingSessionUpdated, Topic::ScanStatusChanged]`; the low-level topics stay intra-core and are deliberately not forwarded.

The work spans a thread boundary. `EventBus::publish` runs synchronously on whatever thread mutated state (for the tick-coalesced tracking event, that is the chat-log watcher's OS thread). The hub crosses the boundary in one place and one direction:

- The bus subscriber closure runs on the **publisher** thread. It deserialises the value back into a `DomainEvent` (a value that is not a `DomainEvent`, which would be an upstream programming error, is dropped rather than forwarded as an untyped frame: this is the runtime re-validation that compensates for the bus's loosely-typed `Value` payload) and calls `SseHub::dispatch`.
- `dispatch` assigns the frame's sequence number, builds the frame, and offers it to every connected client's queue, signalling each client's `Notify`. A client's asynchronous `next_frame` (driven by the in-process bridge task on the runtime) then wakes and pops.

The hub state (the connection registry, the sequence counter) is guarded by a single `Mutex`, and each client's queue carries its own lock and `Notify`, so dispatch and drain do not contend beyond the brief registry critical section.

#### The frame format and sequence number

`dispatch` builds each frame as:

```
id: {seq}\nevent: {topic}\ndata: {data_json}\n\n
```

- `id:` is a monotonic sequence number (`state.seq += 1`), shared across every client's copy of a frame so a consumer can reason about gaps.
- `event:` carries the domain topic, so the bridge can route a frame without parsing its body.
- `data:` is the compact envelope JSON.

This is a server-sent-events frame format, a lineage of the retired HTTP transport; in the shipped application the frames are consumed in-process by the bridge rather than written to an HTTP response.

#### Bounded queues with drop-oldest

Each connected client gets its own bounded queue, sized `DEFAULT_MAX_QUEUE = 256` frames. A stalled or slow reader cannot grow memory without limit: when its queue is full, `ClientQueue::offer` drops the **oldest** frame before enqueuing the new one. Drop-oldest (not drop-newest) is correct under push-to-pull: the newest frame is the one that triggers the freshest hydration, so it must never be the one discarded. A dropped frame is therefore self-healing: the next frame the reader does receive triggers a snapshot re-hydration that reflects every intervening change.

Client registration is RAII. `SseHub::register` returns an `SseClient` whose `Drop` unregisters it from the hub, so there is no app-lifetime background task and no explicit teardown call: when the in-process bridge that holds the one long-lived client is dropped, the registration goes with it. The 15-second keep-alive and the `: ready` opening comment that belonged to the retired HTTP handler are transport-loop concerns; in the shipped application, where the hub is drained by an in-process bridge rather than served over HTTP, they do not apply.

## Producers and coalescing

Two core services produce domain events. Both share a discipline: **they publish only after releasing their own lock**, so a subscriber never runs while the producer holds its lock. (`EventBus::publish` copies its subscriber list under the bus lock, then releases it before dispatch, so no bus-lock to producer-lock cycle can form.)

### The tracker

The tracker (`frontend/src-tauri/eo-services/src/tracker.rs`) produces `tracking.session.updated` via its `emit_session_event` helper, which builds a typed `TrackingSessionUpdated`, serialises it to a `Value`, and publishes it on `Topic::TrackingSessionUpdated`. It emits in three situations:

- **Session started.** `start_session` builds the new session state under the lock, then (after releasing it) does the DB insert and emits the started event with `status="active"`, `reason="started"`, stamped with the session start time.
- **Session stopped.** `stop_session` finalises the session, then emits the stopped event with `status="idle"`, `reason="stopped"`, stamped from the injected clock's end time.
- **Tick advanced.** This is where `Topic::TickFlushed` does its work. While a session is active, the tracker subscribes to `Topic::TickFlushed`. Its handler `on_tick_flushed` coalesces a settled tick's mutations into one `tracking.session.updated` (`reason="updated"`). Critically, it emits **only when the tick actually changed the live readout**: the P&L handlers set a `session_dirty` flag, and the handler reads and resets that flag under the lock; if the flag is clear (a tick of unrelated chat traffic), nothing is published, so an idle tick does not wake every frontend listener. The event is stamped with the tick's own timestamp; a settled tick that carries no timestamp falls back to the injected clock, so the required `occurred_at` always names a real instant.

In all three paths the published value is a typed `TrackingSessionUpdated` instance serialised to a `Value`, with `occurred_at` produced by `to_iso_utc(...)`. The `session_id` is captured under the lock and passed into the emit helper rather than re-read off the live session, so the published id provably belongs to the session whose mutation the event describes even if a concurrent `stop_session` has since cleared it.

### The manual-scan service

The manual skill-scan service (`frontend/src-tauri/eo-services/src/skill_scan_manual.rs`) produces `scan.status.changed`. It has no external tick boundary like the tracker's, so it coalesces with an internal **settled-boundary key**. The helper `publish_status` is called after releasing the lock at every settled mutation point (verb completion, each per-page OCR step, worker completion). Under the lock it computes a `status_key()` projection of the owned state and compares it to the last published key (`last_emitted_key`); it advances the key and publishes only when the key actually moved. This coalesces to one frame per discrete status change rather than one per call, and ensures the main and worker threads cannot both emit the same transition. The key is baselined at construction to the resting idle status, so the redundant initial idle frame is suppressed (listeners hydrate the idle status via the GET on mount).

The payload carries only the coarse `phase`. Per-page capture / OCR progress liveness rides the snapshot re-hydration the frame triggers, rather than widening the wire: the emitter fires on every discrete progress change, but the payload stays a minimal invalidation signal.

### Why the payloads are minimal: push-to-pull

Both producers follow the **push-to-pull** model (see [ADR 0009](../adr/0009-push-to-pull-invalidation.md)). A payload is a minimal invalidation signal (which session, active versus idle, why; or which scan phase), and the window re-hydrates the full shape via the matching snapshot GET. This keeps the ETag / 304 snapshot as the single source of shape, minimises the serialisation surface, and is exactly what makes drop-oldest queue overflow safe.

## The bridge and the frontend relay

In the shipped application the producer spine's frames are forwarded onto the Tauri event bus **in-process** by the shell's domain-event bridge (`spawn_domain_event_bridge` in `frontend/src-tauri/entropia-orme/src/lib.rs`), the native replacement for the frontend's former `EventSource` relay. The bridge registers a client on the producer spine's hub, drains its frames, and applies the same two transforms the old relay did before re-emitting onto the bus, so every window (including hidden overlays) receives core state changes by subscription rather than by polling.

- **The dot-to-colon rename.** Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots), so `domain_topic_to_tauri_event` namespaces the dotted wire topic with colons: `tracking.session.updated` is re-emitted as `tracking:session:updated`, and `scan.status.changed` as `scan:status:changed`. This is the **only** topic transform; the wire contract keeps the dotted form throughout.

- **Whole-envelope forwarding.** For each frame the bridge parses the `data` JSON (via `parse_domain_frame`) and re-emits the **whole** envelope (`type`, `event_version`, `occurred_at`, `payload`) onto the colon-form Tauri topic, not just the payload, so a topic-aware consumer sees the full contract. A frame whose JSON fails to parse is logged and dropped.

The frontend half (`frontend/src/lib/realtime/eventRelay.ts`) now owns only the **re-hydrate nudge**. It listens for the `substrate:native-installed` event the shell emits once the native services compose, and fires a payload-less re-hydrate on each consumer's topic. Each topic-aware consumer (the tracking and scan stores, the overlay) subscribes through its typed topic and re-reads rather than reduces, so a payload-less frame reads as "re-hydrate" rather than as an idle session; this keeps a freshly live (or re-composed) core from leaving a window showing stale data. The nudge stays frontend-owned because it must fire after the webview is listening, which an emit at install time cannot guarantee on a cold load. The relay returns a stop function the layout hands back to Svelte for teardown on window close.
