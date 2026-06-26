# ADR-0013: Collapse to a single in-process Rust binary

- Status: Accepted (supersedes [ADR-0001](0001-strangler-fig-port.md))
- Context: reflects the landed implementation

## Context and problem statement

The strangler-fig seam ([ADR-0001](0001-strangler-fig-port.md)) did its job. Every route is now served by a native Rust handler, proven byte-equivalent to the original against the shared oracle ([ADR-0005](0005-cross-language-equivalence-oracle.md)). With the native arm serving the whole surface, the proxy fallback forwards nothing, and the machinery that made the migration incremental has become pure cost rather than insurance.

That machinery is substantial. The hybrid ran a second operating-system process (the Python sidecar) that the shell had to launch, relocate onto a private port, idle the producers of, and terminate on exit. It bound a public loopback socket and reverse-proxied across it. It kept a per-route arm-override map so any route could be steered back to the sidecar at runtime. And it shipped a frozen PyInstaller bundle of the Python runtime, roughly 170 MiB, inside the installer. Each of these was justified only while two implementations had to run side by side; none serves a single, proven native implementation. A loopback socket is needless attack surface, a second producer spine is a class of split-brain bug to keep idled, and the bundled runtime is the bulk of the download.

## Decision

Remove the sidecar from the shipped application and collapse the backend into the Tauri shell process. The frontend reaches the backend through one path: the `api_request` Tauri IPC command (`frontend/src-tauri/entropia-orme/src/lib.rs`), which dispatches the request in-process via `dispatch_in_process` (`frontend/src-tauri/eo-http/src/lib.rs`): it builds the request, runs it straight through `build_router` with `tower::ServiceExt::oneshot`, and returns the response. No socket is bound and no loopback hop is made.

`eo-http` is **retained** as the in-process dispatch substrate. It is the same router, middleware stack (CORS, the Host and Origin guard, the observe layer), and native handlers as before; only the transport in front of it changed, from a bound socket fronting a reverse proxy to a Tauri IPC command. Deleted with the hybrid are the reverse-proxy arm, the per-route arm-override map, the upstream proxy client, the sidecar lifecycle (private-port allocation, spawn, idle-signalling, exit-time teardown), and the recovery-to-direct-topology fallback. An unmatched path is now the framework's own `404`, not a forward.

Domain events reach the frontend through an in-process Tauri event bridge rather than an HTTP stream. The old `GET /api/events` server-sent-event route is gone; `spawn_domain_event_bridge` (`frontend/src-tauri/entropia-orme/src/lib.rs`) subscribes to the event-stream hub and re-emits each frame onto the Tauri event system, and the frontend listens on that bus (`frontend/src/lib/realtime/eventRelay.ts`). The typed envelope contract and the two-layer spine are untouched (see the update to [ADR-0002](0002-event-spine.md)).

Composition is publish-last. The shell builds `AppState`, composes the native services, and only then publishes the state to the `api_request` command; until that point `api_request` answers a not-ready error, and the frontend re-drives its initial reads on the `substrate:native-installed` event the install emits (`compose_substrate` in `frontend/src-tauri/entropia-orme/src/lib.rs`). Recovery is therefore a frontend re-hydrate on that event, not a transport retry.

The Python implementation **stays in the repository as the cross-language testing oracle** ([ADR-0005](0005-cross-language-equivalence-oracle.md)). Only the shipped sidecar is removed; the oracle the native code is graded against is unaffected.

## Consequences

- A running instance is one process. No second process to launch, relocate, idle, or reap; no loopback socket bound, so the network attack surface the proxy edge once presented is gone (the IPC command carries no listener).
- The installer drops by roughly three-quarters (the bundled Python runtime is gone): the shipped bundle is the single Rust binary plus its data, model, and ONNX Runtime assets, with no `entropiaorme-backend.exe`, no `python` runtime, and no PyInstaller payload.
- In-process dispatch is sub-millisecond per request at p50 and p95 across the hydration surface, with no socket or serialisation hop. The `eo-http` router micro-benchmark (`frontend/src-tauri/eo-http/tests/router_microbench.rs`) measures it.
- The byte contract that the proxy arm once had to preserve is now wholly the native handlers' own. This is not a new risk: the equivalence oracle that proved each route stays in place and continues to grade the native output against the Python reference.
- The runtime arm-override kill-switch is gone. The revert for a misbehaving route is now a source change rather than a runtime flip, which is acceptable because the migration is complete and every route is oracle-proven; there is no second implementation to fall back to.
- The first-launch database upgrade the sidecar used to perform (migrating a pre-baseline schema forward) now runs natively in-process for the one schema version existing installations occupy: a version-32 database is upgraded to the version-33 baseline on open (dropping the retired write-only `tt_curve_observations` table, which no read path consumed) and then adopted, exactly as a fresh version-33 database is. Schemas older than version 32 remain a deliberate decline, since no installed database occupies them.
- The event contract ([ADR-0002](0002-event-spine.md)) and the push-to-pull invalidation model ([ADR-0009](0009-push-to-pull-invalidation.md)) survive unchanged; only the transport beneath them moved from an HTTP SSE stream to the in-process Tauri event bridge.

See also the [architecture overview](../architecture/overview.md) and the [service map](../architecture/service-map.md), and the full [ADR index](index.md).

## Evidence

- `frontend/src-tauri/entropia-orme/src/lib.rs`
- `frontend/src-tauri/eo-http/src/lib.rs`
- `frontend/src-tauri/entropia-orme/src/composition.rs`
- `frontend/src/lib/realtime/eventRelay.ts`
- `frontend/src-tauri/eo-http/tests/router_microbench.rs`
- `THIRD-PARTY-NOTICES.md`
