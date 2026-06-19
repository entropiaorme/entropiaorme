# System overview

EntropiaOrme is an analytical desktop tool for Entropia Universe. It runs as a Tauri 2 desktop application: a single native shell process that hosts the application's webviews (a Svelte 5 frontend) and the HTTP backend the frontend reads its state from, in the same process. All gameplay-economy and progression logic lives behind that HTTP surface; the frontend renders snapshots it reads back over it. This page orients a new reader on how the running application is laid out as an operating-system process, and on how it behaves once the application has settled into steady state.

The backend is a native Rust HTTP service that runs inside the Tauri shell. It began as a Python FastAPI process shipped as a separate sidecar binary and was ported to Rust one route at a time behind a strangler-fig proxy seam; once every route was served natively and proven equivalent, the sidecar and the proxy were removed and the backend collapsed into the shell process. That history is recorded in [ADR-0001](../adr/0001-strangler-fig-port.md) (the migration seam, now superseded) and [ADR-0013](../adr/0013-in-process-collapse.md) (the collapse). The Python implementation remains in the repository as the cross-language testing oracle ([ADR-0005](../adr/0005-cross-language-equivalence-oracle.md)); it is no longer shipped.

## Process topology

A running instance is a **single operating-system process**: the Tauri shell. It owns the application windows and webviews, and it hosts the native Rust HTTP service, an axum application in the `eo-http` crate (`frontend/src-tauri/eo-http/src/lib.rs`). There is no second process, no bound network socket, and no loopback hop.

The frontend reaches the backend through one path: the `api_request` Tauri IPC command (`frontend/src-tauri/entropia-orme/src/lib.rs`). The webview calls it with a method, path, headers, and body; the command dispatches the request straight through the in-process router and returns the response. Change notifications travel the other way over the Tauri event system rather than as HTTP. From the frontend's point of view there is one backend it calls and one event bus it listens on.

## The in-process dispatch substrate

The `eo-http` crate is the HTTP substrate, retained from the migration but now dispatched in-process rather than over a socket. `dispatch_in_process` (`frontend/src-tauri/eo-http/src/lib.rs`) builds the request and runs it straight through `build_router` with `tower::ServiceExt::oneshot`, returning the response without binding a listener. This is the server side of the Tauri-IPC transport: the request runs through the identical stack a socket client would have hit (the native route handlers, the Host and Origin guard, CORS, and the observe layer), so behaviour and instrumentation are unchanged; only the transport in front of the router differs.

`build_router` (`frontend/src-tauri/eo-http/src/lib.rs`) assembles the router from the natively-registered routes (`native::register` in `frontend/src-tauri/eo-http/src/native.rs`, with `native_routes` in `lib.rs`), with the framework's own `404` as the fallback for an unmatched path and a `405` for an unported method on a registered path. The guard stack fronts the routes in the backend's order: CORS outermost, then the Host and Origin guard (`api_guard`). A same-process request carries no Origin or Host header, which the guard admits and CORS leaves undecorated, exactly as intended.

## The composition root

Native routes can only serve once their backing services exist. Those services are built in the composition root (`frontend/src-tauri/entropia-orme/src/composition.rs`), which resolves the data directory, opens the application database, loads the bundled game-data snapshot, and constructs the ported services over a single injected clock.

`compose_substrate` (`frontend/src-tauri/entropia-orme/src/lib.rs`) drives this **publish-last**: the shell builds the shared `AppState`, composes the native services, installs them into the state, and only then publishes the state to the `api_request` command. Until that point `api_request` answers a not-ready error, and the frontend re-drives its initial reads on the `substrate:native-installed` event the install emits. Recovery is therefore a frontend re-hydrate on that event, not a transport retry.

Two invariants of the composition are worth noting, both grounded in `frontend/src-tauri/entropia-orme/src/composition.rs`:

- The native read surface and the producer spine **share one database pool and one clock**. The pool is single-owner (a single connection opened with WAL plus a busy timeout), so producer writes and HTTP reads queue through it without deadlock.
- The producer spine and the substrate share live service handles. The same `Arc<HuntTracker>`, settings writer, skill tracker, and hotbar listener are cloned into the substrate's state, so the routes and the producer-side bus subscriptions operate on one instance each.
- OCR is an optional faculty. The ONNX Runtime is pinned to an absolute bundled path before any session is built, and a failed runtime load is logged but never declines composition; the engine simply sits absent and the scan seams report as unavailable.

If composition declines (a missing or empty game-data snapshot, a producer fault, or a database below the adoptable baseline), the shell logs it and the substrate is never published; there is no longer a sidecar to fall back to.

## Steady-state behaviour

Once mounted, the application settles into an idle that issues no work until the backend announces a change. An idle dashboard performs its mount-time hydration reads through `api_request`, listens on the Tauri event bus, and then issues no further reads until a change notification arrives. This property is enforced by the oracle, not assumed: `backend/tests/test_network_quiet_seam.py` records every request the backend serves while driving real state changes through the production producers.

Three characteristics define the steady state:

- **Quiet idle.** Unrelated activity does not generate backend reads. The frontend's only sanctioned recurring timers are visibility-gated, and a covered hydration read returns `304 Not Modified` when nothing has changed, so an idle application does no needless work.
- **One event bus.** Domain events reach the webview over the Tauri event system. The shell's domain-event bridge (`spawn_domain_event_bridge` in `frontend/src-tauri/entropia-orme/src/lib.rs`) subscribes to the event-stream hub and re-emits each frame onto the Tauri bus under its dotted topic; the frontend relay (`frontend/src/lib/realtime/eventRelay.ts`) listens and routes by named listener.
- **Push-to-pull reads.** An event frame is a minimal invalidation signal (which surface changed and why), not the new state. A window that receives one re-reads the full state from a hydration request; rendered state always comes from a snapshot read and is never folded together from event payloads. Dropping an old frame under load is therefore safe, because the next read reflects every intervening change.

The detail of which events exist and what each carries is its own chapter; see [Event taxonomy](event-taxonomy.md). The reasoning behind the push-to-pull invalidation model, and why frames are signals rather than payloads, is recorded in [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Where to next

- [Service and crate map](service-map.md): the services behind the routes and how the workspace crates are organised.
- [Event taxonomy](event-taxonomy.md): the two event layers, the typed domain envelopes, and the bridge contract.
- [OCR pipeline](ocr-pipeline.md): the skill-scan and repair-cost recognition path and its ONNX Runtime obligations.
- [Database schema reference](database-schema.md): the application database the read surface and producer spine share.
- [ADR-0013: Collapse to a single in-process Rust binary](../adr/0013-in-process-collapse.md): the decision behind the single-process topology.
- [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md): the decision behind event-driven hydration.
