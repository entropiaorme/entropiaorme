# System overview

EntropiaOrme is an analytical desktop tool for Entropia Universe. It runs as a Tauri 2 desktop application: a single native shell process that hosts the application's webviews (a Svelte 5 frontend) and the backend the frontend reads its state from, in the same process. All gameplay-economy and progression logic lives behind that backend surface; the frontend renders snapshots it reads back over it. This page orients a new reader on how the running application is laid out as an operating-system process, and on how it behaves once the application has settled into steady state.

The backend is a pure-Rust in-process service that runs inside the Tauri shell. It began as a Python FastAPI process shipped as a separate sidecar binary and was ported to Rust one route at a time behind a strangler-fig proxy seam; once every route was served natively and proven equivalent, the sidecar and the proxy were removed and the backend collapsed into the shell process. The in-process HTTP router that collapse left in place was then itself retired in favour of typed Tauri commands over a service facade, and its `eo-http` crate was deleted. That history is recorded in [ADR-0001](../adr/0001-strangler-fig-port.md) (the migration seam, now superseded), [ADR-0013](../adr/0013-in-process-collapse.md) (the collapse), and [ADR-0019](../adr/0019-typed-command-facade.md) (the typed-command facade). Throughout the port the Rust engines were graded against the Python originals, retained in-repo as a cross-language equivalence oracle ([ADR-0005](../adr/0005-cross-language-equivalence-oracle.md)). That oracle has since been retired: the running application is a single pure-Rust binary with no Python and no second implementation, and the equivalence evidence the oracle produced is preserved as frozen, committed Rust-side goldens ([ADR-0016](../adr/0016-retire-equivalence-oracle.md)).

## Process topology

A running instance is a **single operating-system process**: the Tauri shell. It owns the application windows and webviews, and it hosts the backend as an in-process service composed from the workspace's Rust crates. There is no second process, no bound network socket, and no loopback hop.

The frontend reaches the backend through one seam: typed Tauri IPC commands (`app/src-tauri/entropia-orme/src/commands.rs`), one per operation, each delegating to the composed service facade (`eo-api::Api`). The webview invokes a named typed command with typed arguments and receives a typed value back; a field-name typo is a compile error rather than a runtime `undefined`. Raw image reads (`capture_png` and `planet_map_image`) and shell-owned window lifecycle commands are the narrow exceptions to the generated facade. Change notifications travel the other way over the Tauri event system. From the frontend's point of view there is one backend it calls and one event bus it listens on.

The shell pre-spawns six application webviews: the main application, the tracking strip, the manual-scan overlay, the cartography pin overlay, the navigation strip, and the radar-guidance overlay. The five overlays start hidden, undecorated, transparent, always on top, and absent from the taskbar. The cartography overlay is a deliberately small action surface: it holds only the selected planet and map view synced from the Maps page and reads its pin palette from the database-backed per-preset pin configurations, while each click uses the same coordinate-capture command and typed pin-create command as the Maps page. The navigation strip hydrates one persisted navigation run and offers its operational controls; the click-through radar overlay renders only the bearing line over the user's calibrated game radar. Dedicated Tauri capabilities and generated command groups constrain each webview to its own window lifecycle, event, and application-command surface.

## The in-process dispatch facade

`eo-api::Api` (`app/src-tauri/eo-api/src/lib.rs`) is the application boundary the typed commands dispatch into: each command is a thin `#[tauri::command]` that calls one async facade method, which orchestrates the domain services (`eo-services`) and returns a typed DTO or the typed `ApiError` contract. There is no socket, no router, and no HTTP envelope: a request is a direct in-process call, and serialisation happens exactly once, at the command boundary. The DTOs are plain `serde` structs whose TypeScript bindings are generated from the Rust source by `cargo xtask gen-ts`, held in lock-step with the Rust manifest by a CI check.

## The composition root

The commands can only serve once their backing services exist. Those services are built in the composition root (`app/src-tauri/entropia-orme/src/composition.rs`), which resolves the data directory, opens the application database, loads the bundled game-data snapshot, and constructs the services over a single injected clock.

`compose_substrate` (`app/src-tauri/entropia-orme/src/lib.rs`) drives this **publish-last**: the shell composes the services, builds the `eo-api::Api` facade whole from them, and only then publishes it to the command layer. It then settles a level-triggered readiness record (`app/src-tauri/entropia-orme/src/substrate.rs`) as ready, or as failed with its reason, and the `substrate_ready` command answers that outcome whenever it is asked.

The webview does not wait for any of this to paint. The title bar, the navigation, and the pages mount at once, and the typed transport (`app/src/lib/api/invoke.ts`) holds each command until `substrate_ready` says the backend is up. A read made during startup therefore lands on live state instead of failing, and the regions still waiting on data show loading placeholders rather than empty states. A failed start replaces the pages with a failure surface. The facade's not-ready error stays as a defence for any caller outside the transport. The reasoning is recorded in [ADR-0030: An explicit startup readiness boundary](../adr/0030-startup-readiness-boundary.md).

Three invariants of the composition are worth noting, all grounded in `app/src-tauri/entropia-orme/src/composition.rs`:

- The read surface and the producer spine **share one database pool and one clock**. The pool is single-owner (a single connection opened with WAL plus a busy timeout), so producer writes and reads queue through it without deadlock.
- The producer spine and the facade share live service handles. The same `Arc<HuntTracker>`, navigation service, settings writer, skill tracker, and hotbar listener are cloned into the facade, so the commands and the producer-side bus subscriptions operate on one instance each.
- OCR is an optional faculty. The ONNX Runtime is pinned to an absolute bundled path before any session is built, and a failed runtime load is logged but never declines composition; the engine simply sits absent and the scan seams report as unavailable.

If composition declines (a missing or empty game-data snapshot, a producer fault, or a database below the adoptable baseline), the shell logs it and the substrate is never published; there is no longer a sidecar to fall back to.

## Steady-state behaviour

Once mounted, the application settles into an idle that issues no work until the backend announces a change. An idle dashboard performs its mount-time hydration reads through the typed commands, listens on the Tauri event bus, and then issues no further reads until a change notification arrives. This falls out of the in-process design rather than being assumed: there is no network socket to poll, and the frontend's only sanctioned recurring timers are visibility-gated (`app/src/lib/realtime/useVisiblePoll.ts`), so a read is issued only in response to an event that says a surface changed.

Three characteristics define the steady state:

- **Quiet idle.** Unrelated activity does not generate backend reads. The frontend's only sanctioned recurring timers are visibility-gated, and a window re-reads only when an event signals its surface changed, so an idle application does no needless work.
- **One event bus.** Domain events reach the webview over the Tauri event system. The shell's domain-event bridge (`spawn_domain_event_bridge` in `app/src-tauri/entropia-orme/src/lib.rs`) subscribes to the event-stream hub and re-emits each frame onto the Tauri bus under its colon-form topic; the topic-aware consumers (the tracking and scan stores, the overlay) subscribe by named listener and read their own snapshot once on mount.
- **Push-to-pull reads.** An event frame is a minimal invalidation signal (which surface changed and why), not the new state. A window that receives one re-reads the full state from a hydration request; rendered state always comes from a snapshot read and is never folded together from event payloads. Dropping an old frame under load is therefore safe, because the next read reflects every intervening change.

The detail of which events exist and what each carries is its own chapter; see [Event taxonomy](event-taxonomy.md). The reasoning behind the push-to-pull invalidation model, and why frames are signals rather than payloads, is recorded in [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md).

## Where to next

- [Service and crate map](service-map.md): the services behind the typed commands and how the workspace crates are organised.
- [Event taxonomy](event-taxonomy.md): the two event layers, the typed domain envelopes, and the bridge contract.
- [OCR pipeline](ocr-pipeline.md): the skill-scan and repair-cost recognition path and its ONNX Runtime obligations.
- [Database schema reference](database-schema.md): the application database the read surface and producer spine share.
- [ADR-0013: Collapse to a single in-process Rust binary](../adr/0013-in-process-collapse.md): the decision behind the single-process topology.
- [ADR-0009: Push-to-pull invalidation](../adr/0009-push-to-pull-invalidation.md): the decision behind event-driven hydration.
