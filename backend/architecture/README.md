# Backend architecture

The prose map of how the EntropiaOrme backend is put together: the event spine that pushes change notifications to the UI, the hydration HTTP surface the UI reads state from, the actor-shaped services that own mutable state, and the enforcement that keeps each of those properties true. The companion [`PORT-READINESS.md`](PORT-READINESS.md) inventories how these shapes map onto a contemplated native port of the sidecar.

Code is the source of truth; every section names the modules it describes so claims can be checked against the tree.

## Topology

The desktop app is three cooperating pieces:

- the SvelteKit frontend, rendered in Tauri webviews: one main window plus pre-spawned, initially hidden overlay windows;
- the Tauri shell (the `entropia-orme` member of the `frontend/src-tauri/` cargo workspace): window chrome and sidecar launch, no business logic;
- this Python backend: a FastAPI process on the loopback interface, shipped as a sidecar binary.

All domain logic lives in the backend. The frontend reaches it two ways: request/response HTTP for state reads and mutations, and a one-way server-sent-events stream for change notifications. The combination is deliberately **push-to-pull**: an event frame is a minimal invalidation signal (which surface changed and why), and the window that receives one re-reads the full state from a hydration GET. Rendered state always comes from a snapshot read; it is never folded together from event payloads.

### The native takeover substrate

During the native backend port, the shell additionally runs the strangler-fig seam (`eo-http`): an axum router that owns the public loopback address the frontend is wired to, while the Python backend relocates to a private port chosen at launch (the launcher passes it via `ENTROPIAORME_BACKEND_PORT`, which the backend already treats as its bind port; its Host-header guard follows automatically, and the proxy rewrites `Host` to the private authority it dials). Natively-ported routes are served in-process; every other method and path, including the event stream, is reverse-proxied to the relocated backend byte-stably on the golden-projected axes (status, content-type, cache-control, etag, body) with response frames streamed as they arrive, so the `: ready` prompt flush and keep-alive comments pass through unbuffered. Each flipped route keeps both arms live: a per-route override map (`ENTROPIAORME_ROUTE_ARMS`, plus a persisted JSON file named by `ENTROPIAORME_ROUTE_ARMS_FILE`) is consulted at request time, so a misbehaving native route can be steered back to the Python implementation in a shipped build without a rebuild. Every substrate failure path degrades to the legacy direct topology (backend on the public port, no proxy). The frontend is unaware of any of this; see `PORT-READINESS.md` for the route-by-route plan.

The payoff is a network-quiet steady state: an idle dashboard performs its mount-time hydration reads, opens one event stream, and then issues no further requests until the backend announces a change. `backend/tests/test_network_quiet_seam.py` pins this by recording every request the app serves while driving real state changes through the production producers.

## The two event layers

Events exist at two deliberately separate levels, both carried by the same in-process bus.

### The in-process bus

`backend/core/event_bus.py` is a small synchronous pub/sub: a dict of topic string to subscriber list, guarded by an `RLock`. Three properties are load-bearing:

- `publish` snapshots the subscriber list under the lock, releases it, then invokes each callback **outside** the lock. No subscriber ever runs under the bus lock, so no lock cycle can form between the bus and a service lock.
- Dispatch is synchronous on the publisher's thread. There is no queue and no executor; a subscriber's work happens inline before `publish` returns.
- Subscribers are error-isolated: a raising callback is logged and skipped, and dispatch continues with the rest.

### Low-level backend events

`backend/core/events.py` defines eleven string topics with dict payloads, used for intra-backend coordination. The chat-log watcher (`backend/services/chatlog_watcher.py`) produces most of them (`combat`, `loot_group`, `skill_gain`, `enhancer_break`, `global`, `mission_received`, and the tick boundary `tick_flushed`); the tracker produces `session_started` and `session_stopped`; the hotbar listener produces `active_tool_changed` and `active_heal_tool_changed`. Five modules subscribe: the tracker, the hotbar listener, the quest service, the skill tracker, and the SSE hub's domain-topic half (below). These events never leave the process.

`tick_flushed` is published last in each watcher flush, after every per-event publish for that tick has been dispatched, which makes it the settled boundary the tracker coalesces its outbound notification onto.

### Typed domain events

`backend/core/domain_events.py` defines the coarse, frontend-facing layer: Pydantic models with a `Literal` `type` discriminator, assembled into a discriminated union (`DomainEvent`). The wire form is two-level: the discriminator and bookkeeping fields sit on the envelope, and the event's own fields nest under `payload`:

```json
{
  "type": "tracking.session.updated",
  "event_version": 1,
  "occurred_at": "2026-01-01T00:00:00+00:00",
  "payload": { "sessionId": "...", "status": "active", "reason": "updated" }
}
```

Two events exist today:

| Event | Topic and discriminator | Fields nested under `payload` |
|---|---|---|
| `TrackingSessionUpdated` | `tracking.session.updated` | `sessionId`, `status` (`active`/`idle`), `reason` (`started`/`updated`/`stopped`) |
| `ScanStatusChanged` | `scan.status.changed` | `phase` (`idle`/`capturing`/`processing`/`awaiting_review`) |

The envelope discipline:

- The `type` literal doubles as the bus topic and the frontend topic; the mapping is identity by construction.
- Every envelope carries `event_version` (per-event schema version, currently 1) and `occurred_at` (a required, never-null ISO-8601 UTC string stamped from the domain timestamp, with the injected clock supplying the instant when the domain carries none, so events are deterministic under replay).
- Envelope and payload models set `extra="forbid"`: the wire contract is closed, and an undeclared key is a bug. Payload field names are spelled camelCase literally, with no alias generator. This is the deliberate opposite of the read surface's `_Loose` response models (below).
- The JSON schema of every envelope is pinned by a golden (`backend/tests/test_event_schema_drift.py` against `backend/tests/expected/event_schemas.snapshot.json`), so a payload change is a reviewed, ratified event rather than an accident.

One honest caveat, documented in the module itself: the bus's `publish(topic, data)` is `Any`-typed and carries both layers, so "a typed instance on a domain topic" is a producer-side convention the type checker cannot enforce. The SSE hub type-guards at runtime for exactly this reason. A producer-side narrowing (`publish_domain`) is a noted follow-up; the contemplated native port makes the guarantee structural (see `PORT-READINESS.md`).

## Push-to-pull delivery

A domain event reaches a window through one fixed hop chain. For a tracking update:

1. The producer thread (the chat-log watcher) mutates tracker state and, after the tracker lock is released, publishes a `TrackingSessionUpdated` instance on the bus.
2. The SSE hub's `_on_domain_event` runs synchronously on that producer thread: it type-guards the envelope, serialises it to wire JSON there (keeping CPU work off the event loop), and hops to the asyncio loop via `call_soon_threadsafe`.
3. `_dispatch`, on the loop thread, assigns a monotonic sequence number and fans the frame out to every connection queue.
4. Each `GET /api/events` connection's generator yields the frame to its client.
5. The main window's relay re-emits the envelope onto the Tauri event bus, where every window can hear it.
6. A listening window re-reads its hydration GET. The frame payload itself is never rendered.

### The SSE hub

`backend/services/event_stream.py` (`EventStreamHub`) subscribes to the two domain topics and fans frames out to per-connection bounded queues:

- The thread boundary is crossed in exactly one place and one direction (`_on_domain_event` to `_dispatch`); the connection registry, sequence counter, and queues are touched only on the loop thread and need no lock.
- Each connection queue is bounded (256 frames) with **drop-oldest** overflow: a stalled reader cannot grow memory, and dropping old frames is safe because every frame only triggers a fresh full hydration; the newest frame always reflects every intervening change.
- The hub holds no background task. Each connection's drain is owned by its request lifecycle, which the server supervises and tears down on disconnect.
- On shutdown the hub closes first, before producers stop, so a final shutdown event cannot race a frame onto a closing loop.

The runtime type-guard: a payload on a domain topic that is not a Pydantic model, or whose `type` is not a string, is logged at error level and dropped rather than forwarded.

### The stream endpoint contract

`GET /api/events` (`backend/routers/events.py`) is the one HTTP surface deliberately outside both the OpenAPI schema and the ETag middleware, so its contract lives here:

- Media type `text/event-stream`; response headers set `Cache-Control: no-cache`, `Connection: keep-alive`, and `X-Accel-Buffering: no`.
- On connect the stream yields an opening `: ready` comment (flushing headers; tests synchronise on it), then frames of the form `id: <seq>`, `event: <topic>`, `data: <envelope JSON>`.
- `event:` carries the dotted domain topic so a client routes by named listener without parsing the body. `id:` is a process-monotonic sequence number; it is advisory only. The server does **not** implement `Last-Event-ID` replay: gap recovery is push-to-pull re-hydration plus the relay's reconnect nudge, not frame replay.
- A quiet stream emits a `: keep-alive` comment every 15 seconds; disconnection is checked at each keep-alive and after each frame.
- The connection queue is registered before streaming begins, so an event published immediately after open is delivered rather than raced away.

### The frontend relay

`frontend/src/lib/realtime/eventRelay.ts` runs in the main window only (it is guaranteed alive for the app's lifetime) and is started from the root layout. It opens one `EventSource` on the stream and, per forwarded topic, re-emits each frame's whole envelope onto the Tauri event bus as a global broadcast.

Its only transform is topic renaming: Tauri event names forbid dots, so `tracking.session.updated` becomes `tracking:session:updated` and `scan.status.changed` becomes `scan:status:changed`. On every stream open, including `EventSource` auto-reconnects, the relay emits a payload-less nudge frame on each forwarded topic so every window re-hydrates; a reconnect can therefore never leave a window stale.

### Consumers

The consumer discipline is subscribe-then-hydrate: attach the Tauri listener first, then run the initial read, so a change landing between the first read and the listener attach is re-announced rather than lost. The ordering is the consumer's responsibility, not a store guarantee: `frontend/src/lib/stores/trackingStore.ts` and `scanStore.ts` expose `subscribe` and `hydrate` as independent calls and contribute the shared read mechanics (a relayed frame is a pure trigger; `hydrate` coalesces overlapping reads, with a frame arriving mid-read queueing exactly one follow-up, and a failed read keeps the last good snapshot rather than blanking). The character view is the canonical implementation: it attaches the listener inside the subscribe promise, then hydrates. The dashboard and the quests route also consume the tracking store (the quests route currently runs its first read before its listener attaches, a known ordering gap); the overlay and scan-overlay windows attach their own local listeners to the same topics with their own local refresh functions.

Two window-event surfaces are deliberately **not** part of this spine: the overlay popup protocols (`overlay-menu:*`, `overlay-armour-cost:*`), which are directed window-to-window IPC for transient UI, and the shell-emitted `overlay-shown` event, which prompts the overlay to re-read configuration fields that no tracking frame announces (the overlay is a pre-spawned hidden window, so no visibility event fires on show).

## The hydration surface

### The tracking snapshot

`GET /api/tracking/snapshot` (`backend/routers/tracking.py`) is the single consolidated tracking readout; it replaced three separately polled routes. The response is a three-state union discriminated by `status`:

- `unavailable`: no tracker is wired (the response is just the status);
- `idle`: no active session; configuration fields are present and `recentEvents` is `[]` (the feed clears on idle);
- `active`: the full readout, derived from an owned, immutable value the tracker assembles under its lock.

The polymorphism is carried by `response_model_exclude_unset=True` rather than separate models: only `status` is required, and unset keys are dropped at serialisation so the lean states do not gain a wall of nulls. The snapshot deliberately mixes key casings (`session_id`, `started_at`, `kill_count` beside camelCase headline numbers): it unions the shapes of the three routes it replaced without renaming anything, so existing consumers carried over unchanged. The demo namespace (`/api/demo/tracking/snapshot`) reuses the same implementation and model.

### Conditional requests

`backend/middleware/etag.py` adds strong ETags (a quoted SHA-256 of the response body) to every 2xx GET under four hydration prefixes: `/api/tracking`, `/api/scan`, `/api/quests`, `/api/codex`. A matching `If-None-Match` returns `304 Not Modified` with an empty body; comparison follows RFC 7232 weak semantics. `Cache-Control` is `no-cache`, which mandates revalidation: the goal is skipping the body and the client-side re-render when nothing changed, not skipping the network round-trip. `covered_get_routes` enumerates the covered surface so the contract test (`backend/tests/test_etag.py`) fails by name when a covered GET silently leaves the contract.

Coverage is deliberately scoped to these four event-driven surfaces. The remaining read surfaces (analytics, character, equipment, settings) hydrate on mount and navigation rather than on a domain event, sit outside the conditional-request contract on purpose, and are pinned as such by the same test's out-of-scope assertions.

### Response models

`backend/routers/response_models.py` types every JSON response. The models are descriptive, not prescriptive: each subclasses `_Loose` (`extra="allow"`), so a handler key the model does not enumerate passes through serialisation untouched rather than being silently dropped. Numeric value fields are typed `float` uniformly (integer-valued numbers serialise in their float form); genuinely integral fields (counts, ranks, identifiers) stay `int`. `response_model_exclude_unset` is applied per route, only where a 200 has genuinely divergent shapes.

### The contract chain

Four interlocking gates pin the HTTP contract end to end:

| Link | Pinned by |
|---|---|
| Handler output matches its declared model | Pydantic `response_model` serialisation on every route |
| The OpenAPI spec matches the committed snapshot | `backend/tests/test_openapi_drift.py` against `backend/tests/expected/openapi.snapshot.json` (byte-equality of canonical JSON) |
| The generated TypeScript client matches the snapshot | `npm run gen:api:check` in the frontend CI job (`openapi-typescript` over the committed snapshot, then a clean-diff check) |
| Live 2xx bodies conform to their declared schemas | the schemathesis contract suite (`backend/tests/test_api_contract.py`), full tier |

The frontend client is generated from the committed snapshot, never from a running server, so the same file is simultaneously the drift-test fixture and the codegen input: a backend contract change must regenerate the golden (a reviewed, ratified event; CI enforces a commit marker plus an independent ratification record) and regenerate the client, or one of the gates fails.

## Actor-shaped services

"Actor-shaped" here is a convention, not a framework: a service is a plain class that owns its mutable state behind one private lock, mutates that state only in its own methods, and publishes typed events only after releasing the lock. There is no mailbox, no message queue, and no registry; handlers are invoked synchronously by the bus on the publisher's thread. The value of the convention is that each service has exactly one synchronisation boundary and a single owner for every piece of mutable state, which is also what makes the shape mechanical to port (see `PORT-READINESS.md`).

### The tracker

`backend/tracking/tracker.py` (`HuntTracker`) is the central session coordinator and the largest actor-shaped service:

- One `RLock` serialises all owned in-memory session state. Re-entrancy is a defensive default, not a requirement; no locked region re-acquires the lock today.
- Bus publishes and the tracker's own DB writes happen only after the lock is released; the lock is never held across SQLite, with one documented exception: the equipment cost and profile provider callbacks may read the shared connection under the lock on a cache miss. That exception is deadlock-free only because the global lock order is tracker lock first, database lock second, and the providers never take the database lock; wrapping a provider in the database lock is the documented landmine.
- `snapshot()` returns an owned, immutable readout: aggregation runs under the lock with every value captured into locals, then two session-scoped DB reads run outside the lock, keyed on a session id captured under it so a concurrent stop cannot tear the readout.
- Outbound notification is coalesced. Mutating handlers set a dirty flag; the `tick_flushed` boundary handler reads and resets it and, only when dirty, publishes one `tracking.session.updated` with reason `updated`. One frame stands for a tick's worth of low-level events, and unrelated chat traffic wakes nobody.
- Event subscriptions are session-scoped: `start_session` subscribes the seven low-level handlers and `stop_session` unsubscribes them, so an idle tracker consumes nothing.

`backend/tests/test_tracker_concurrency.py` hammers the read path against a mutating producer thread to pin the lock discipline; `backend/tests/test_tracking_snapshot.py` pins the snapshot's shape.

### The scan service

`backend/services/skill_scan_manual.py` (`SkillScanManual`) mirrors the same shape for the manual skill-scan flow, with its own lock and its own settled boundary: it has no tick, so it synthesises one by projecting its owned state to a status key and publishing `scan.status.changed` only when the key changes (compare-and-advance under the lock, publish after release). The coalescer is baselined to the idle state at construction, so the resting state is silent; listeners hydrate the idle status via the GET on mount, and the first genuine change emits.

## Supervised workers

The backend runs a small, fixed set of named worker threads; the discipline is that every worker is named, daemonised, and owned by a service that stops it.

- Production threads: `chatlog-watcher` (the one app-lifetime worker, a 100 ms tail loop), `hotbar-resolve` (short-lived, spawned per hotbar keypress), `spacebar-capture`, and `skill-scan-process`. The two OS keyboard hooks are `pynput` listener threads named at construction through the same convention.
- The backend has zero free-floating asyncio tasks by design. The one place a long-lived task could exist, the SSE fan-out, deliberately has none: each connection's drain belongs to its request lifecycle.

`backend/tests/test_supervised_workers.py` enforces the convention rather than trusting it: a static scan over production sources forbids unsupervised coroutine spawns and requires every thread literal to carry `daemon=True` and a non-empty `name=`, and runtime checks verify the SSE hub detaches on shutdown and the watcher actually terminates when stopped. The scanners are themselves tested to have teeth (a planted violation must flag; a compliant construction must pass).

The frontend twin is the timer discipline: `frontend/src/lib/realtime/useVisiblePoll.ts` is the single sanctioned home for `setInterval`. It clears the timer entirely while the window reports hidden and fires one catch-up tick on resume; `windowGeometryPoll`, in the same module, is the one sanctioned always-running exception (the overlay must persist its position while hidden) and exposes no pause knob so it cannot smuggle a network poll past the gate. `backend/scripts/check_no_bare_setinterval.py` (with `backend/tests/test_no_bare_setinterval.py`) enforces both rules tree-wide over the tracked frontend sources, and additionally bans the retired `tracking-state-changed` window event that the typed topic replaced.

A naming note worth keeping straight: the wire and bus topic is dotted (`tracking.session.updated`), the Tauri-side topic is colon-separated (`tracking:session:updated`), and the hyphenated `tracking-state-changed` is the retired event the lint bans.

## Concurrency model

The honest inventory, since most of the backend's subtlety lives here:

- **Threads.** The asyncio event loop serves HTTP and the SSE fan-out. The chat-log watcher publishes synchronously from its own thread, which means every bus subscriber's handler (tracker mutation, skill-tracker DB writes, quest-service updates, SSE serialisation) runs on that single watcher thread for chat-driven events. The hotbar listener publishes from short-lived per-press threads. FastAPI handlers run on the server's worker threads.
- **Locks.** The bus lock (subscriber list only, never held across dispatch), the tracker lock, the scan-service lock, and the shared database wrapper's lock. The one cross-lock ordering that matters: tracker lock before database lock, never the reverse.
- **Known gap, deliberately documented rather than papered over:** database-lock discipline across services is uneven. The skill tracker writes under the shared database lock, but much of the read path (most GET handlers, and the provider closures wired in the application lifespan) reads the bare shared connection from the server's worker threads without it. The single-producer reality (chat-driven writes all execute on the watcher thread) rules out writer/writer races, but a multi-step read interleaving with a concurrent watcher-thread write is exactly the cursor-coherency case the database wrapper's own warning describes. It is a convention with a hole rather than a structural guarantee, and the contemplated port closes it by construction (an owned connection behind one writer; see `PORT-READINESS.md`).

## Enforcement map

The architecture's properties are tests, not prose promises:

| Property | Enforced by |
|---|---|
| Idle app issues no polling traffic; events drive hydration | `backend/tests/test_network_quiet_seam.py` |
| Domain event wire schemas are stable | `backend/tests/test_event_schema_drift.py` + `event_schemas.snapshot.json` |
| SSE queue bounds and drop-oldest overflow | `backend/tests/test_event_stream.py` |
| End-to-end stream seam over a live server | `backend/tests/test_event_stream_seam.py` |
| Tracker read path is safe against the producer thread | `backend/tests/test_tracker_concurrency.py` |
| Snapshot shape and idle union | `backend/tests/test_tracking_snapshot.py` |
| ETag coverage and 304 semantics on every covered hydration GET | `backend/tests/test_etag.py` |
| OpenAPI spec matches the committed contract | `backend/tests/test_openapi_drift.py` |
| Generated TS client matches the contract | `gen:api:check` (frontend CI job) |
| 2xx bodies conform to declared schemas | `backend/tests/test_api_contract.py` (schemathesis) |
| Worker threads are named, daemonised, owned | `backend/tests/test_supervised_workers.py` |
| Frontend timers are visibility-gated, in one module | `backend/tests/test_no_bare_setinterval.py` |
| Golden changes are deliberate, reviewed, ratified | the golden-ratification CI guard |

## Related documents

- [`PORT-READINESS.md`](PORT-READINESS.md): how these shapes map onto a contemplated native port, and what does not port mechanically.
- [`PORTING-RULEBOOK.md`](PORTING-RULEBOOK.md): the application-ready rule set for the port; boundary fidelity, interior latitude, the deliberate-divergence register, and the verification obligations.
- [`PORT-BASELINE.md`](PORT-BASELINE.md): the Python backend's captured performance and coverage reference, the bands port work is graded against in flight, and the per-module branch-coverage table.
- [`../../TESTING.md`](../../TESTING.md): the test suite, tiers, and gates that enforce the properties above.
- [`../testing/CONSISTENCY.md`](../testing/CONSISTENCY.md): the snapshot and event-stream consistency apparatus.
- [`../testing/CONFORMANCE.md`](../testing/CONFORMANCE.md): the HTTP conformance substrate (ETag, OpenAPI, contract tests).
