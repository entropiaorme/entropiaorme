# Event taxonomy

EntropiaOrme is an analytical desktop application whose Rust core observes a stream of game-state changes (parsed chat-log lines, manual skill scans) and pushes the resulting changes to the frontend windows without polling. Internally this is built as **two distinct event systems**, layered so that each solves a different problem. This page documents both layers end to end: from the low-level, synchronous, in-process topics that core services use to coordinate, up to the coarse typed envelopes that cross to the webview over the in-process Tauri event bridge.

The two-layer shape is implemented in Rust under `app/src-tauri/`: the low-level bus lives in the `eo-services` crate (`app/src-tauri/eo-services/src/event_bus.rs`, with its typed payloads in `app/src-tauri/eo-services/src/bus_events.rs`), and the domain envelopes and the typed broadcast channel live in the `eo-wire` crate (`app/src-tauri/eo-wire/src/domain_events.rs` and `app/src-tauri/eo-wire/src/bus.rs`). This began life as a Python FastAPI sidecar; the Rust implementation is now the only one.

For the wider context of where this fits, see the [architecture overview](overview.md) and the [service map](service-map.md). The two design decisions that shape this layer are recorded as [ADR 0002: the event spine](../adr/0002-event-spine.md) and [ADR 0009: push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Two layers, and why they are separate

The system has two event layers with deliberately different shapes and audiences.

| | Low-level in-process bus | Domain event envelopes |
| --- | --- | --- |
| Defined in | `app/src-tauri/eo-services/src/event_bus.rs` (payloads in `bus_events.rs`) | `app/src-tauri/eo-wire/src/domain_events.rs` |
| Topic form | a `Topic` enum whose `as_str()` yields string constants (`"combat"`, `"loot_group"`, ...) | dotted domain strings (`"tracking.session.updated"`, ...) |
| Payload | a typed `BusEvent` enum, one variant per topic with a per-topic payload struct | a closed, typed envelope struct |
| Granularity | one raw mutation (a single parsed combat line, one loot group) | one coarse change ("the live session changed") |
| Audience | other core services in the same process | the frontend, over the in-process bridge |
| Crosses to the webview? | never | yes, serialised to JSON |
| Dispatch | synchronous, on the publishing thread | synchronous publish on the bus, then republished onto the typed broadcast channel |

The **low-level bus** is intra-core wiring. As the domain-events module puts it, a frontend window does not want "a damage_dealt combat line", it wants "the live session aggregates changed". The raw topics are at the wrong granularity to push to a webview: they are numerous, fine-grained, and carry core-shaped values (snake_case keys, raw float timestamps) that have no business on a public wire.

The **domain event layer** is the coarse, frontend-facing subset. Each domain event is a typed envelope carrying a `type` discriminator, so the wire format is a serde-compatible tagged JSON object. The set of domain events is small and curated; the low-level topics stay inside the process and are never forwarded.

The two layers share one piece of plumbing: the eight domain envelopes ride the *same* `EventBus` instance as the low-level topics, as typed variants of the `BusEvent` enum. They carry the `eo-wire` envelope types (`TrackingSessionUpdated`, `ScanStatusChanged`, `HarvestRecorded`, `NavigationUpdated`, `ProtectionUpdated`, `HealingUpdated`, `WeaponsUpdated`, and `ConsumablesUpdated`) directly. `EventBus::publish` takes a `&BusEvent` and derives the topic from the variant, so "a typed envelope on a domain topic" is enforced by construction at the bus seam: a foreign value on a domain topic is unrepresentable, and no runtime re-validation exists because none is needed. `subscribe_domain_bridge` (in `app/src-tauri/entropia-orme/src/composition.rs`) republishes those eight variants' envelopes, still typed, onto the broadcast channel the shell's bridge consumes.

### The bus mechanics

The bus in `app/src-tauri/eo-services/src/event_bus.rs` is a thread-safe synchronous pub/sub:

- `subscribe(topic, callback)` returns a `Registration` handle; `unsubscribe(topic, registration)` removes it. Subscription is per-topic by design; callbacks take `&BusEvent` and match the variant they expect.
- `publish(&BusEvent)` derives the topic from the variant, snapshots the subscriber list (and the taps) under a `Mutex`, then dispatches outside the lock. Each callback runs synchronously on the publisher's thread.
- A subscriber that panics does not break dispatch: each callback runs inside `catch_unwind`, so a panic is contained and dispatch continues to the next subscriber.
- `add_tap(tap)` installs a **full-stream observer** called with every `BusEvent` that crosses `publish`, regardless of topic, before subscriber dispatch. Because subscription is per-topic, a tap is the only supported way to observe the complete publish stream (new topics included); the replay recorder's fingerprint capture rides one. Taps are likewise panic-contained.

## Low-level topics

The variants of the `Topic` enum in `app/src-tauri/eo-services/src/event_bus.rs` name the intra-core topics; each one's `as_str()` yields the wire string. They are grouped by source.

| `Topic` variant | Topic string | Meaning |
| --- | --- | --- |
| `Combat` | `combat` | A parsed combat line from chat.log (damage dealt or taken). |
| `LootGroup` | `loot_group` | A tick's worth of loot lines, grouped into one event. |
| `HarvestFail` | `harvest_fail` | A failed harvesting swing ("Harvest attempt failed to generate useable resources") parsed from chat.log. |
| `SkillGain` | `skill_gain` | A skill-gain line parsed from chat.log. |
| `EnhancerBreak` | `enhancer_break` | An enhancer-break line parsed from chat.log. |
| `Global` | `global` | A global / hall-of-fame broadcast line. |
| `HotbarIntent` | `hotbar_intent` | A resolved hotbar press with its source session, operating-system occurrence time, equipment identity, kind, cost, reload, healing profile, and optional lifesteal metadata. |
| `ActiveToolChanged` | `active_tool_changed` | The active hotbar tool changed. |
| `ActiveHealToolChanged` | `active_heal_tool_changed` | The active heal tool changed. |
| `ActiveHarvestToolChanged` | `active_harvest_tool_changed` | The active harvesting tool changed (a hotbar "tool" equip), carrying its per-use cost. |
| `SessionStarted` | `session_started` | A tracking session started. |
| `SessionStopped` | `session_stopped` | A tracking session stopped. |
| `MissionReceived` | `mission_received` | A mission was received. |
| `TickFlushed` | `tick_flushed` | The settling boundary: a parse tick has closed and every per-event subscriber write for that tick has completed. |

The same enum also carries the eight frontend-facing domain topics (`TrackingSessionUpdated`, `ScanStatusChanged`, `HarvestRecorded`, `NavigationUpdated`, `ProtectionUpdated`, `HealingUpdated`, `WeaponsUpdated`, and `ConsumablesUpdated`), whose `as_str()` returns the dotted constants from `app/src-tauri/eo-wire/src/domain_events.rs`, because the typed envelopes ride this same bus before the bridge republishes them onto the broadcast channel.

### Intent and evidence

Healing and weapon attribution each combine two independent observations: `HotbarIntent`, the equipment the player pressed and when the input hook saw the key, and the chat-log output that later confirms (or contradicts) it. The chat timestamp has only whole-second precision, so the input occurrence time and local monotonic observation decide ordering. How the tracker reconciles the two, and what it does when they disagree, is recorded in [ADR-0027](../adr/0027-intent-led-healing-attribution.md) (healing), [ADR-0032](../adr/0032-unified-weapon-attribution.md) (weapons), and [ADR-0033](../adr/0033-damage-over-time-effect-windows.md) (damage-over-time effect windows).

### The tick_flushed settling boundary

`Topic::TickFlushed` is special. chat.log timestamps have one-second precision, so all recognised lines sharing a timestamp are treated as one application "tick". The chat-log watcher (`app/src-tauri/eo-services/src/chatlog_watcher.rs`) buffers a tick's events and, when the timestamp advances or the file goes idle, flushes them: loot lines become a single `Topic::LootGroup`, other events are published individually. After **every** per-event publish for that tick has been dispatched (and its subscribers have mutated state synchronously), the watcher publishes `Topic::TickFlushed` last, carrying the tick's timestamp.

This is purely intra-core, like the other low-level topics. Its purpose is to give a stateful subscriber (the tracker) a single, well-defined moment to coalesce a tick's worth of low-level mutations into one coarse domain event, rather than emitting one domain event per raw mutation. The coalescing is described under [Producers and coalescing](#producers-and-coalescing).

## Domain event envelopes

The frontend-facing domain events are defined in `app/src-tauri/eo-wire/src/domain_events.rs`, which is the reference for each envelope's exact fields.

### The shared envelope shape

Every envelope carries the same three top-level fields, then a typed `payload`:

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | a closed topic-tag field | The domain-topic string, verbatim (`"tracking.session.updated"`). This doubles as the discriminator and as the topic the relay re-emits, so the bus-topic to envelope mapping is identity. The tag serialises to exactly one literal and refuses any other input. |
| `event_version` | `i64` (default `1`) | A per-event-type schema version. An additive-only field change bumps it. It is independent of the application version, so the frontend can reason about shape evolution without coupling to the app version. |
| `occurred_at` | `String` (required, **non-nullable**) | An ISO-8601 UTC timestamp for the instant the change occurred. |
| `payload` | a closed struct | The event-specific body (see below). |

#### occurred_at is required and never null

`occurred_at` is a **required** envelope field and is never `null` and never the bus's raw float. In `app/src-tauri/eo-wire/src/domain_events.rs` it is typed as a plain `String` on every envelope, with no `Option`.

An emitter whose domain carries no instant for the change (for example a settled tick that has no timestamp) synthesises one from its injected clock rather than threading a null to the wire. The helper `to_iso_utc(ts)` (in `app/src-tauri/eo-services/src/tracker.rs`) renders a Unix timestamp (a SQLite REAL or a bus float) as ISO-8601 UTC, so the required `occurred_at` always names a real instant.

#### The closed-schema rule

Every envelope struct and every payload struct carries `#[serde(deny_unknown_fields)]`, so the wire contract is closed in both directions: an undeclared key is rejected on the way in, and the core is the *emitter* that constructs these explicitly, so an undeclared key is a bug. Payload field names are spelled camelCase via serde renames (no alias generators), so snake_case keys and float timestamps cannot leak onto the wire. The module's tests assert that an extra payload key, an extra envelope key, a missing `type` tag, and a foreign `type` tag are all rejected.

### The eight events

| Topic | Fires when | Payload |
| --- | --- | --- |
| `tracking.session.updated` | the live session started, advanced a tick that changed its readout, or stopped; also after a decision on a weapon mismatch | `sessionId` (nullable, never omitted), `status` (`active` / `idle`), `reason` (`started` / `updated` / `stopped`) |
| `scan.status.changed` | the manual skill scan changed phase or advanced a capture or OCR step | `phase` (`idle` / `capturing` / `processing` / `awaiting_review`) |
| `harvest.recorded` | a harvesting attempt, successful or failed, has been written durably | `harvestId`, `success` |
| `navigation.updated` | persisted navigation state changed | none |
| `protection.updated` | a protection write committed (a repair or reading, an undo, a limited set changed) | none |
| `healing.updated` | a healing correction or its undo committed, or a session was deleted | none |
| `weapons.updated` | a stored shot of an ended session was corrected, or the correction undone | none |
| `consumables.updated` | the running doses changed: started, expired, removed, or restored ([ADR-0036](../adr/0036-consumable-dose-lifecycle.md)) | none |

Most payloads are empty on purpose (see [push-to-pull](#why-the-payloads-are-minimal-push-to-pull)): each consumer re-reads what it shows. Two domain topics are also consumed inside the core: navigation advances a run on `harvest.recorded`, and the tracker re-reads its live effect windows and running doses on `healing.updated`. A refused write publishes nothing.

### The discriminated union

The eight envelopes form a discriminated union:

```rust
#[serde(untagged)]
pub enum DomainEvent {
    TrackingSessionUpdated(TrackingSessionUpdated),
    ScanStatusChanged(ScanStatusChanged),
    HarvestRecorded(HarvestRecorded),
    NavigationUpdated(NavigationUpdated),
    ProtectionUpdated(ProtectionUpdated),
    HealingUpdated(HealingUpdated),
    WeaponsUpdated(WeaponsUpdated),
    ConsumablesUpdated(ConsumablesUpdated),
}
```

The `#[serde(untagged)]` dispatch is made exact by the closed topic-tag fields: a frame routes to the one variant whose `type` literal it carries, and a missing or unrecognised `type` fails outright, so adding a new member changes neither the existing members nor the wire format. Every call site (the bus publish, the broadcast channel) routes through this union unchanged. `DomainEvent::topic()` returns the variant's wire topic, and `to_wire_json()` yields the compact envelope JSON.

## The typed broadcast channel

Domain events leave the producer spine through a typed broadcast channel and reach the frontend over the in-process Tauri event bridge. The channel (`DomainBus`, in `app/src-tauri/eo-wire/src/bus.rs`) is the broker; the bridge task that consumes it is described under [The bridge and the frontend relay](#the-bridge-and-the-frontend-relay).

### The channel: typed fan-out

`DomainBus` wraps a tokio `broadcast::Sender<DomainEvent>`. It is fed by `subscribe_domain_bridge` (in `app/src-tauri/entropia-orme/src/composition.rs`), which subscribes the eight domain topics on the bus and republishes each typed envelope onto the channel. The low-level topics stay intra-core and are deliberately not forwarded.

The work spans a thread boundary. `EventBus::publish` runs synchronously on whatever thread mutated state (for the tick-coalesced tracking event, that is the chat-log watcher's OS thread). The channel crosses the boundary in one place and one direction: the bus subscriber closure runs on the **publisher** thread and calls `DomainBus::publish` (a non-blocking broadcast send), and the bridge task consumes its `subscribe()` receiver asynchronously on the runtime. The envelope stays typed end to end; nothing is serialised until the bridge hands it to the Tauri emitter.

`DomainBus` also carries taps (`add_tap`): full-stream observers of every published envelope, mirroring the low-level bus's tap affordance for tests and capture harnesses.

#### Bounded delivery with skip-to-live

The broadcast channel is bounded (capacity 256, set at composition). A stalled or slow receiver cannot grow memory without limit: a receiver that falls more than the capacity behind observes a lag error (`RecvError::Lagged`) on its next receive and skips ahead to the oldest retained event. This preserves the self-healing property the retired per-client drop-oldest queues provided, in channel semantics: under push-to-pull the newest event is the one that triggers the freshest hydration, so it is never the one lost, and a skipped event is compensated by the snapshot re-hydration the next received event triggers, which reflects every intervening change.

There is no frame format, no sequence number, and no client registry. The retired HTTP-era fan-out hub rendered each envelope into a server-sent-events text frame that the shell then string-parsed back into JSON; the typed channel deletes that round trip entirely. A receiver's lifetime is its own: dropping the `broadcast::Receiver` ends the subscription, so there is no registration to tear down.

## Producers and coalescing

Core services publish domain events only at settled state boundaries and outside their own lock, so a subscriber never runs while the producer holds its lock. (`EventBus::publish` copies its subscriber list under the bus lock, then releases it before dispatch, so no bus-lock to producer-lock cycle can form.)

### The tracker

The tracker (`app/src-tauri/eo-services/src/tracker.rs`) produces `tracking.session.updated` via its `emit_session_event` helper, which builds a typed `TrackingSessionUpdated` and publishes it as the `BusEvent::TrackingSessionUpdated` variant. It emits in three situations:

- **Session started.** `start_session` builds the new session state under the lock, then (after releasing it) does the DB insert and emits the started event with `status="active"`, `reason="started"`, stamped with the session start time.
- **Session stopped.** `stop_session` finalises the session, then emits the stopped event with `status="idle"`, `reason="stopped"`, stamped from the injected clock's end time.
- **Tick advanced.** This is where `Topic::TickFlushed` does its work. While a session is active, the tracker subscribes to `Topic::TickFlushed`. Its handler `on_tick_flushed` coalesces a settled tick's mutations into one `tracking.session.updated` (`reason="updated"`). Critically, it emits **only when the tick actually changed the live readout**: the P&L handlers set a `session_dirty` flag, and the handler reads and resets that flag under the lock; if the flag is clear (a tick of unrelated chat traffic), nothing is published, so an idle tick does not wake every frontend listener. The event is stamped with the tick's own timestamp; a settled tick that carries no timestamp falls back to the injected clock, so the required `occurred_at` always names a real instant.

In all three paths the published value is a typed `TrackingSessionUpdated` instance, with `occurred_at` produced by `to_iso_utc(...)`. The `session_id` is captured under the lock and passed into the emit helper rather than re-read off the live session, so the published id provably belongs to the session whose mutation the event describes even if a concurrent `stop_session` has since cleared it.

### The manual-scan service

The manual skill-scan service (`app/src-tauri/eo-services/src/skill_scan_manual.rs`) produces `scan.status.changed`. It has no external tick boundary like the tracker's, so it coalesces with an internal **settled-boundary key**. The helper `publish_status` is called after releasing the lock at every settled mutation point (verb completion, each per-page OCR step, worker completion). Under the lock it computes a `status_key()` projection of the owned state and compares it to the last published key (`last_emitted_key`); it advances the key and publishes only when the key actually moved. This coalesces to one frame per discrete status change rather than one per call, and ensures the main and worker threads cannot both emit the same transition. The key is baselined at construction to the resting idle status, so the redundant initial idle frame is suppressed (listeners hydrate the idle status via the GET on mount).

The payload carries only the coarse `phase`. Per-page capture / OCR progress liveness rides the snapshot re-hydration the frame triggers, rather than widening the wire: the emitter fires on every discrete progress change, but the payload stays a minimal invalidation signal.

### The other producers

The remaining producers publish after a committed write, outside their own lock, stamped from the injected clock. The tracker publishes `harvest.recorded` after its harvest transaction commits and `consumables.updated` after each dose change or expiry sweep. The navigation, protection, healing-review, and weapon-review services each take a change sink at composition and call it after every committed write or undo; the sink publishes the service's topic. Navigation debounces repeated harvesting events at one captured location for 30 seconds, so tool swings cannot advance several closely spaced stops.

### Why the payloads are minimal: push-to-pull

The frontend-facing producers follow the **push-to-pull** model (see [ADR 0009](../adr/0009-push-to-pull-invalidation.md)). A payload is a minimal invalidation signal, and the window re-hydrates the full shape through the matching typed snapshot command. This keeps one read model as the source of shape, minimises the serialisation surface, and makes bounded-channel overflow self-healing.

## The bridge and the frontend relay

In the shipped application the producer spine's domain events are forwarded onto the Tauri event bus **in-process** by the shell's domain-event bridge (`spawn_domain_event_bridge` in `app/src-tauri/entropia-orme/src/lib.rs`), the native replacement for the frontend's former `EventSource` relay. The bridge subscribes the typed broadcast channel, receives each envelope already typed, and applies the one transform the Tauri event system needs before re-emitting, so every window (including hidden overlays) receives core state changes by subscription rather than by polling.

- **The dot-to-colon rename.** Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots), so `domain_topic_to_tauri_event` replaces dots with colons, for example `navigation.updated` becomes `navigation:updated`. This is the **only** topic transform; the wire contract keeps the dotted form throughout.

- **Whole-envelope forwarding.** The bridge emits the **whole** typed envelope (`type`, `event_version`, `occurred_at`, `payload`) onto the colon-form Tauri topic, not just the payload, so a topic-aware consumer sees the full contract; the Tauri emitter serialises it, the first and only serialisation on the path. A receiver that falls behind the channel's capacity observes a lag error, which is logged, and skips to live (the snapshot re-hydration the next event triggers absorbs the gap).

There is no frontend relay layer. Each topic-aware consumer (the tracking and scan stores, the overlay) attaches its listener first and then reads its snapshot once, re-reading on every frame rather than reducing it, so a payload-less frame reads as "re-hydrate" rather than as an idle session (`app/src/lib/realtime/snapshotStore.svelte.ts`). Startup needs no special signal: a read made before the backend has composed is held by the typed transport until the startup readiness answer says it is up ([ADR-0030](../adr/0030-startup-readiness-boundary.md)), so the initial read always lands on live state.
