# System overview

EntropiaOrme is an analytical desktop tool for Entropia Universe. It runs as a Tauri 2 desktop application: a native shell process that hosts the application's webviews (a Svelte 5 frontend) and an HTTP backend the frontend reads its state from. All gameplay-economy and progression logic lives behind that HTTP surface; the frontend renders snapshots it reads back over it. This page orients a new reader on how the running application is laid out as operating-system processes, and on how those processes behave once the application has settled into steady state.

The backend exists in two forms that run side by side. It began as a Python FastAPI process shipped as a sidecar binary, and it is being ported to a native Rust HTTP service that runs inside the Tauri shell itself. The two are not mutually exclusive at runtime: the landed design is a hybrid in which a native substrate owns the public address and serves the routes that have been ported natively, while reverse-proxying everything else to the relocated Python sidecar. The shape of that hybrid, a strangler-fig migration, is the subject of the bulk of this page.

## Process topology

A running instance is two cooperating operating-system processes:

- **The Tauri shell process.** This owns the application windows and webviews. It also hosts the native Rust HTTP service, an axum application in the `eo-http` crate (`frontend/src-tauri/eo-http/src/lib.rs`). That service binds the **public loopback port** the frontend is wired to and is the single HTTP endpoint the webview talks to.
- **The Python sidecar process.** This is the FastAPI backend (`backend/main.py`), launched by the shell as a child process. In the hybrid topology it no longer owns the public port; it is relocated onto a **private loopback port** chosen at launch.

The frontend reaches the backend two ways, both over the public port: request/response HTTP for state reads and mutations, and a one-way server-sent-events stream (`GET /api/events`) for change notifications. The frontend is unaware of the split between the two backend forms; from its point of view there is one HTTP origin.

### The public/private port split

The split of one public port across two processes is what lets the substrate stand in front of the sidecar transparently.

- The sidecar reads its bind port from the `ENTROPIAORME_BACKEND_PORT` environment variable, defaulting to `8421` when unset (`backend/main.py`). The launcher passes the chosen private port through this variable, so relocating the sidecar requires no code change: the backend already treats this value as where it binds.
- The sidecar's own Host-header guard follows that bind automatically. Its allowlist of acceptable `Host` authorities is derived from the same `ENTROPIAORME_BACKEND_PORT` value (`ALLOWED_API_HOSTS` in `backend/main.py`), so the guard names the private authority once the port moves.
- The substrate (`AppState::new` in `frontend/src-tauri/eo-http/src/lib.rs`) is constructed with both the sidecar's `host:port` authority (its upstream) and the public port it serves itself. From the public port it derives its own inbound Host allowlist (`127.0.0.1:<public_port>` and `localhost:<public_port>`), mirroring the backend's guard for the public boundary.

When the substrate forwards a request to the sidecar, the proxy rewrites the `Host` header to the private authority it dials. The sidecar's own Host check therefore sees only the rewritten private authority and passes, while the substrate's public-boundary guard (`api_guard` in `frontend/src-tauri/eo-http/src/lib.rs`) is the one that actually screens inbound `Host` and `Origin` values at the public edge.

## The strangler-fig proxy substrate

The substrate is the seam that makes the port incremental. The `eo-http` axum router owns the public address and decides, per request, whether to serve it from a native in-process handler or to reverse-proxy it to the relocated sidecar.

### Routing: native handlers in front, proxy fallback behind

`build_router` (`frontend/src-tauri/eo-http/src/lib.rs`) assembles the router so that natively-registered routes take precedence and a catch-all proxy fallback carries every other method and path to the sidecar. The guard stack fronts both arms in the backend's own order: CORS outermost, then the Host and Origin guard.

Native routes are registered one line per route (`native::register` in `frontend/src-tauri/eo-http/src/native.rs`, with `native_routes` in `lib.rs`). The registrations cover a broad set of the API surface (quests, codex, tracking reads and writes, analytics, scan, settings, character, equipment, and the `/api/events` stream), each pinned to its exact path and method. Anything not registered, including unported methods on an otherwise-native path, falls through to the proxy fallback rather than to an empty `405` (`ArmRoutes::at` installs a proxy fallback and an explicit `HEAD` proxy leg for exactly this reason).

### Byte-stable reverse proxy

For a proxied route the substrate forwards the request to the sidecar and streams the response back as it arrives (`AppState::proxy`, delegating to the `proxy` module). The proxy is byte-stable on the axes the contract is pinned against (status, content-type, cache-control, etag, body), and it streams response frames unbuffered, so the event stream's opening `: ready` flush and its periodic keep-alive comments pass straight through (`backend/architecture/README.md`). The natively-served `/api/events` arm, when composed, serves the same stream contract from an in-process hub; without a composed hub the route simply proxies to the sidecar, which streams the identical contract (`events_stream` in `frontend/src-tauri/eo-http/src/native.rs`).

### Per-route arm override

Every taken-over route keeps **both** implementations alive for the lifetime of the hybrid: the native handler and the live sidecar behind the proxy. Which one serves is decided at request time by consulting an override map (`arms.rs`).

`ArmRoutes::on` (`frontend/src-tauri/eo-http/src/lib.rs`) wraps each native method so that, on every request, it reads the current arm for the route and either runs the native handler (`Arm::Native`) or proxies to the sidecar (`Arm::Proxy`). A route absent from the override map runs its default arm, which is `Native` once it has been flipped (`ArmOverrides::arm_for` in `frontend/src-tauri/eo-http/src/arms.rs`).

The override map is populated from two sources, with later entries winning (`frontend/src-tauri/eo-http/src/arms.rs`):

| Source | Form |
|---|---|
| A persisted JSON file (path supplied by the shell) | a JSON object of `{ "<route>": "native" \| "proxy" }` |
| The `ENTROPIAORME_ROUTE_ARMS` environment variable | a comma-separated `route=arm` list, for example `/api/health=proxy` |

Both parsers are deliberately fault-tolerant: malformed entries and unreadable or malformed files are skipped rather than fatal, so an operator typo can never take the router down. The map can also be replaced at runtime through `AppState::set_overrides`, so a settings surface can swap arms without restarting the router. The practical effect is a runtime kill-switch: a misbehaving native route is one override entry away from the known-good sidecar implementation, in an already-shipped build, without a rebuild.

For the engineering rationale behind this seam, see [ADR-0001: Strangler-fig Python-to-Rust port](../adr/0001-strangler-fig-port.md).

## The composition root and hot-upgrade

Native routes can only serve once their backing services exist. Those services are built in the composition root (`frontend/src-tauri/entropia-orme/src/composition.rs`), which mirrors the sidecar's own startup composition: resolve the data directory, open the application database, load the bundled game-data snapshot, and construct the ported services over a single injected clock.

### Constructing and installing native services

A successful composition yields a bundle of service handles (`Composed`, and the `NativeServices` bundle handed to the substrate). The substrate holds each native service handle behind a read/write lock, initially empty (`AppState` in `frontend/src-tauri/eo-http/src/lib.rs`). `AppState::install_native` writes the composed handles into those slots; from that point the next request for a natively-registered route reads the now-present service and runs its native arm instead of the proxy fallback.

Several invariants of the composition are worth noting, all grounded in `frontend/src-tauri/entropia-orme/src/composition.rs`:

- The native read surface and the producer spine **share one database pool and one clock**. The pool is single-owner (a single connection opened with WAL plus a busy timeout), so producer writes and HTTP reads queue through it without deadlock.
- The producer spine and the substrate share live service handles. The same `Arc<HuntTracker>`, `Arc<SseHub>`, settings writer, skill tracker, and hotbar listener are cloned into the substrate's state, so the routes and the producer-side bus subscriptions operate on one instance each.
- OCR is an optional faculty. The ONNX Runtime is pinned to an absolute bundled path before any session is built, and a failed runtime load is logged but never declines composition; the engine simply sits absent and the scan seams report as unavailable.

### Proxy-only by default, hot-upgrade on success

The substrate is designed to be useful before composition completes, and to remain correct if composition never completes at all.

- Composition runs in a background task **off the startup path**, so the substrate answers proxy-only the instant it binds the public port. Until `install_native` is called, every native handler finds its backing service slot empty and proxies the request to the sidecar (the adapters in `frontend/src-tauri/eo-http/src/native.rs` each fall back to `state.proxy(req)` when their service is `None`).
- If composition **declines permanently** (a missing or empty game-data snapshot, a producer fault, or an unrecoverable database fault), the substrate stays proxy-only for the rest of the session and the sidecar serves everything, exactly as before any route was flipped (`Composition::Declined` in `frontend/src-tauri/entropia-orme/src/composition.rs`).
- If composition finds the database **below the adoptable baseline** (the first launch after an upgrade, where the sidecar has not yet migrated the database forward), composition reports `AwaitingMigration` rather than declining. The substrate keeps serving proxy-only and a later retry adopts the database once the sidecar has migrated it, and `install_native` then hot-upgrades the live router to native without a restart.

Because the substrate and sidecar would otherwise both run event producers (two chat-log tailers writing the same database, two OS keyboard hooks), the sidecar stands its own producers down when the shell sets `ENTROPIAORME_PRODUCERS_IDLE`. In that mode the sidecar still constructs its services so every proxied route serves, but starts no chat-log tail thread, no OS key hooks, and no background scan or OCR (`_producers_idle` in `backend/main.py`). The substrate's producer spine owns production instead.

## Steady-state behaviour

Once mounted, the application settles into a network-quiet idle. An idle dashboard performs its mount-time hydration reads, opens one event stream, and then issues no further requests until the backend announces a change (`backend/architecture/README.md`). This property is enforced, not assumed: `backend/tests/test_network_quiet_seam.py` records every request the app serves while driving real state changes through the production producers.

Three characteristics define the steady state:

- **Network-quiet idle.** Unrelated activity does not generate HTTP traffic. The frontend's only sanctioned recurring timers are visibility-gated, and a covered hydration read returns `304 Not Modified` when nothing has changed, so an idle application is genuinely quiet on the wire.
- **One server-sent-events stream.** The frontend opens a single `EventSource` on `GET /api/events` from the main window for the application's lifetime. Change notifications arrive on it as a one-way push; the stream carries a dotted topic per frame so a client routes by named listener.
- **Push-to-pull reads.** An event frame is a minimal invalidation signal (which surface changed and why), not the new state. A window that receives one re-reads the full state from a hydration GET; rendered state always comes from a snapshot read and is never folded together from event payloads. Dropping an old frame under load is therefore safe, because the next read reflects every intervening change.

The detail of which events exist and what each carries is its own chapter; see [Event taxonomy](event-taxonomy.md). The reasoning behind the push-to-pull invalidation model, and why frames are signals rather than payloads, is recorded in [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Where to next

- [Service and crate map](service-map.md): the services behind the routes and how the workspace crates are organised.
- [Event taxonomy](event-taxonomy.md): the two event layers, the typed domain envelopes, and the stream contract.
- [OCR pipeline](ocr-pipeline.md): the skill-scan and repair-cost recognition path and its ONNX Runtime obligations.
- [Database schema reference](database-schema.md): the application database the read surface and producer spine share.
- [ADR-0001: Strangler-fig Python-to-Rust port](../adr/0001-strangler-fig-port.md): the decision behind the substrate and the incremental takeover.
- [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md): the decision behind event-driven hydration.
