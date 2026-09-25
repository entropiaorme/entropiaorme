# Event taxonomy

EntropiaOrme is an analytical desktop application whose Rust core observes a stream of game-state changes (parsed chat-log lines, manual skill scans) and pushes the resulting changes to the frontend windows without polling. Internally this is built as **two distinct event systems**, layered so that each solves a different problem. This page documents both layers end to end: from the low-level, synchronous, in-process topics that core services use to coordinate, up to the coarse typed envelopes that cross to the webview over the in-process Tauri event bridge.

The two-layer shape is implemented in Rust under `app/src-tauri/`: the low-level bus lives in the `eo-services` crate (`app/src-tauri/eo-services/src/event_bus.rs`, with its typed payloads in `app/src-tauri/eo-services/src/bus_events.rs`), and the domain envelopes and the typed broadcast channel live in the `eo-wire` crate (`app/src-tauri/eo-wire/src/domain_events.rs` and `app/src-tauri/eo-wire/src/bus.rs`). This began life as a Python FastAPI sidecar that was kept on only as a cross-language equivalence test oracle; that oracle has since been retired and its tree removed, so the Rust implementation is now the only one. The wire contract the two implementations shared is preserved as the committed schema snapshot the Rust types are asserted against.

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

The two layers share one piece of plumbing: the seven domain envelopes ride the *same* `EventBus` instance as the low-level topics, as typed variants of the `BusEvent` enum. They carry the `eo-wire` envelope types (`TrackingSessionUpdated`, `ScanStatusChanged`, `HarvestRecorded`, `NavigationUpdated`, `ProtectionUpdated`, `HealingUpdated`, and `WeaponsUpdated`) directly. `EventBus::publish` takes a `&BusEvent` and derives the topic from the variant, so "a typed envelope on a domain topic" is enforced by construction at the bus seam: a foreign value on a domain topic is unrepresentable, and no runtime re-validation exists because none is needed. `subscribe_domain_bridge` (in `app/src-tauri/entropia-orme/src/composition.rs`) republishes those seven variants' envelopes, still typed, onto the broadcast channel the shell's bridge consumes.

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

The same enum also carries the seven frontend-facing domain topics (`TrackingSessionUpdated`, `ScanStatusChanged`, `HarvestRecorded`, `NavigationUpdated`, `ProtectionUpdated`, `HealingUpdated`, and `WeaponsUpdated`), whose `as_str()` returns the dotted constants from `app/src-tauri/eo-wire/src/domain_events.rs`, because the typed envelopes ride this same bus before the bridge republishes them onto the broadcast channel.

### Healing intent and chat evidence

Healing attribution consumes two independent observations. `HotbarIntent`
carries the user's resolved equipment intent and the time at which the input
hook observed the key. A later compatible activation output confirms one paid
activation: a direct output for a direct or compound profile, or the first
matching tick for a pure over-time profile. Subsequent effect outputs, passive
outputs, and unattributed outputs remain zero-cost evidence. The game chat
timestamp is retained as provenance, but it has only
whole-second precision, so local monotonic observation and input occurrence
times decide ordering and bounded reconciliation.

The hotbar resolver runs off the hook callback on its worker thread. A rapid
healer activation followed by a weapon switch therefore keeps a short closed
intent tail, allowing the heal output to confirm the healer that was actually
used. The inverse delivery order is also safe: an unexplained output is first
persisted at zero cost and can be reconciled only when a subsequently delivered
intent proves it occurred first and matches the healing profile.

The tracker derives its active weapon, healing tool, or harvesting tool from
that same session-scoped `HotbarIntent`, so a session transition cannot split
intent attribution from the corresponding equipment change.
`ActiveToolChanged`, `ActiveHealToolChanged`, and
`ActiveHarvestToolChanged` remain supported compatibility topics for direct
producers. Healing cost no longer depends on chat-only tool inference: a
self-heal without compatible hotbar intent is passive or unattributed evidence
and cannot add PED cost.

### Weapon intent and damage evidence

A weapon press in `HotbarIntent` declares the weapon in hand from its
occurrence time and starts a new attribution regime (a harvesting-tool press
starts one too). Each offensive `Combat` line is then checked against the
carried weapons' damage bands: a hit the declared weapon explains agrees with
it, a hit only one other carried weapon explains is recorded to that weapon
and raises the mismatch the snapshot's `weaponGuardrail` carries, and a hit
several weapons explain, or none, is recorded without a price. The previous
weapon's shots may still land for a delivery tail after a switch, and count as
its own. Without hotbar intent the bands alone attribute each hit. A decision
on the mismatch is a command, not an event: it reprices the regime's shots and
announces itself through `tracking.session.updated`. See ADR-0032.

A priced hit of a weapon with a declared damage-over-time effect opens an
effect window in the tracker, persisted as it lands; no event announces it.
While it is open, an offensive `Combat` line its tick range holds is a tick of
that paid hit (no shot, no cost), and it never raises the mismatch. A line
both the declared weapon and another weapon's open effect could have printed
stays unpriced. See ADR-0033.

### The tick_flushed settling boundary

`Topic::TickFlushed` is special. chat.log timestamps have one-second precision, so all recognised lines sharing a timestamp are treated as one application "tick". The chat-log watcher (`app/src-tauri/eo-services/src/chatlog_watcher.rs`) buffers a tick's events and, when the timestamp advances or the file goes idle, flushes them: loot lines become a single `Topic::LootGroup`, other events are published individually. After **every** per-event publish for that tick has been dispatched (and its subscribers have mutated state synchronously), the watcher publishes `Topic::TickFlushed` last, carrying the tick's timestamp.

This is purely intra-core, like the other low-level topics. Its purpose is to give a stateful subscriber (the tracker) a single, well-defined moment to coalesce a tick's worth of low-level mutations into one coarse domain event, rather than emitting one domain event per raw mutation. The coalescing is described under [Producers and coalescing](#producers-and-coalescing).

## Domain event envelopes

The frontend-facing domain events are defined in `app/src-tauri/eo-wire/src/domain_events.rs`. Four concrete envelope types exist today.

### The shared envelope shape

Every envelope carries the same three top-level fields, then a typed `payload`:

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | a closed topic-tag field | The domain-topic string, verbatim (`"tracking.session.updated"`). This doubles as the discriminator and as the topic the relay re-emits, so the bus-topic to envelope mapping is identity. The tag serialises to exactly one literal and refuses any other input. |
| `event_version` | `i64` (default `1`) | A per-event-type schema version. An additive-only field change bumps it. It is independent of the application version, so the frontend can reason about shape evolution without coupling to the app version. |
| `occurred_at` | `String` (required, **non-nullable**) | An ISO-8601 UTC timestamp for the instant the change occurred. |
| `payload` | a closed struct | The event-specific body (see below). |

#### occurred_at is required and never null

`occurred_at` is a **required** envelope field and is never `null` and never the bus's raw float. In `app/src-tauri/eo-wire/src/domain_events.rs` it is typed as a plain `String` on every envelope, with no `Option`. The schema snapshot in `app/src-tauri/contracts/event_schemas.snapshot.json` confirms this: `occurred_at` is `"type": "string"` and appears in every envelope's `required` array, with no null branch.

An emitter whose domain carries no instant for the change (for example a settled tick that has no timestamp) synthesises one from its injected clock rather than threading a null to the wire. The helper `to_iso_utc(ts)` (in `app/src-tauri/eo-services/src/tracker.rs`) renders a Unix timestamp (a SQLite REAL or a bus float) as ISO-8601 UTC, so the required `occurred_at` always names a real instant.

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

### `harvest.recorded`

`HarvestRecorded` fires only after a successful or failed harvesting attempt has
been written durably. Its payload carries the stable `harvestId` and a required
`success` boolean. Navigation consumes this semantic boundary rather than
guessing from raw loot lines, then captures the current coordinates and applies
the same arrival policy as a manual refresh.

### `navigation.updated`

`NavigationUpdated` is a content-free push-to-pull invalidation for persisted
navigation state. The Maps page and navigation overlay re-read the complete run
from the typed snapshot command on every event. Radar calibration uses its
separate status read and does not emit this route-state signal.

### `protection.updated`

`ProtectionUpdated` is a content-free push-to-pull invalidation that fires after
any protection write commits: a repair or reading recorded from the overlay's
Cost popup, an undo, or a limited set created, edited, removed, or restored.
Recording moves session armour costs in a different window from the ones that
show them, so the Equipment armour tab, the session review list, and an open
session detail each re-read what they show on every event. A refused write
publishes nothing.

### `healing.updated`

`HealingUpdated` is a content-free push-to-pull invalidation that fires after
a healing correction or its undo commits, and after a session deletion (which
takes its healing evidence with it). A correction moves an ended
session's heal cost, and may take back (or give back) an effect window a
running session is still matching ticks against. The session review list and
an open session detail re-read what they show, and the tracker re-reads its
live effect windows from their persisted expiry, so a taken-back effect stops
explaining ticks at once. Like the navigation service's use of
`harvest.recorded`, this makes the tracker a consumer of a domain topic as
well as a producer. A refused correction publishes nothing.

### `weapons.updated`

`WeaponsUpdated` is a content-free push-to-pull invalidation that fires after
a stored shot of an ended session is corrected (an unpriced shot or an effect
tick assigned to a weapon, or an unresolved hit marked as an effect's tick),
or the correction is undone. The correction moves that session's weapon cost
or shot count (the kill's, or the dangling cost for a shot after the last
kill), so the session review list re-reads its rows and their marks, and an
open session detail re-reads its cost and its weapon attribution evidence.
Corrections touch only ended sessions and never an effect window, so the
tracker does not consume it. A refused correction publishes nothing. (A decision on a live mismatch is part of the running
session and announces itself as `tracking.session.updated` instead.)

### The discriminated union

The seven envelopes form a discriminated union:

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
}
```

The `#[serde(untagged)]` dispatch is made exact by the closed topic-tag fields: a frame routes to the one variant whose `type` literal it carries, and a missing or unrecognised `type` fails outright, so adding a new member changes neither the existing members nor the wire format. Every call site (the bus publish, the broadcast channel, the schema snapshot) routes through this union unchanged. The schema snapshot records the union as a `oneOf` over the seven `$def`s with a `discriminator` mapping keyed on `type`. `DomainEvent::topic()` returns the variant's wire topic, and `to_wire_json()` yields the compact envelope JSON.

## The typed broadcast channel

Domain events leave the producer spine through a typed broadcast channel and reach the frontend over the in-process Tauri event bridge. The channel (`DomainBus`, in `app/src-tauri/eo-wire/src/bus.rs`) is the broker; the bridge task that consumes it is described under [The bridge and the frontend relay](#the-bridge-and-the-frontend-relay).

### The channel: typed fan-out

`DomainBus` wraps a tokio `broadcast::Sender<DomainEvent>`. It is fed by `subscribe_domain_bridge` (in `app/src-tauri/entropia-orme/src/composition.rs`), which subscribes the seven domain topics on the bus and republishes each typed envelope onto the channel. The low-level topics stay intra-core and are deliberately not forwarded.

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

### Harvesting and navigation

The tracker publishes `harvest.recorded` after its harvest transaction commits,
for both resource-yielding and failed attempts. The navigation service subscribes
to that topic only while a run is live. After serialising its route mutations it
publishes `navigation.updated`; navigation consumers then hydrate the persisted
snapshot. Repeated harvesting events at the same captured location are debounced
for 30 seconds so tool swings cannot advance several closely spaced stops.

### Protection

The protection service takes a change sink at composition, the same shape as
navigation's, and calls it after each committed write. The sink publishes
`protection.updated` stamped from the injected clock.

### Healing corrections

The healing review service takes the same kind of change sink and calls it
after each committed correction or undo. The sink publishes `healing.updated`
stamped from the injected clock.

### Weapon assignments

The weapon review service takes the same kind of change sink and calls it
after each committed assignment or undo. The sink publishes `weapons.updated`
stamped from the injected clock.

### Why the payloads are minimal: push-to-pull

The frontend-facing producers follow the **push-to-pull** model (see [ADR 0009](../adr/0009-push-to-pull-invalidation.md)). A payload is a minimal invalidation signal, and the window re-hydrates the full shape through the matching typed snapshot command. This keeps one read model as the source of shape, minimises the serialisation surface, and makes bounded-channel overflow self-healing.

## The bridge and the frontend relay

In the shipped application the producer spine's domain events are forwarded onto the Tauri event bus **in-process** by the shell's domain-event bridge (`spawn_domain_event_bridge` in `app/src-tauri/entropia-orme/src/lib.rs`), the native replacement for the frontend's former `EventSource` relay. The bridge subscribes the typed broadcast channel, receives each envelope already typed, and applies the one transform the Tauri event system needs before re-emitting, so every window (including hidden overlays) receives core state changes by subscription rather than by polling.

- **The dot-to-colon rename.** Tauri event names admit only alphanumerics and `-`, `/`, `:`, `_` (no dots), so `domain_topic_to_tauri_event` replaces dots with colons, for example `navigation.updated` becomes `navigation:updated`. This is the **only** topic transform; the wire contract keeps the dotted form throughout.

- **Whole-envelope forwarding.** The bridge emits the **whole** typed envelope (`type`, `event_version`, `occurred_at`, `payload`) onto the colon-form Tauri topic, not just the payload, so a topic-aware consumer sees the full contract; the Tauri emitter serialises it, the first and only serialisation on the path. A receiver that falls behind the channel's capacity observes a lag error, which is logged, and skips to live (the snapshot re-hydration the next event triggers absorbs the gap).

There is no frontend relay layer. Each topic-aware consumer (the tracking and scan stores, the overlay) attaches its listener first and then reads its snapshot once, re-reading on every frame rather than reducing it, so a payload-less frame reads as "re-hydrate" rather than as an idle session (`app/src/lib/realtime/snapshotStore.svelte.ts`). Startup needs no special signal: a read made before the backend has composed is held by the typed transport until the startup readiness answer says it is up ([ADR-0030](../adr/0030-startup-readiness-boundary.md)), so the initial read always lands on live state.
