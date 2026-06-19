# Service and crate map

The desktop backend is a Rust workspace that runs inside the Tauri shell process and is dispatched in-process: the frontend calls the `api_request` Tauri IPC command, which runs the request straight through an axum router with no socket and no second process. This page enumerates the workspace crates and the services they own, then sets out the HTTP routes the router serves natively.

The backend was ported from a Python FastAPI sidecar one route at a time behind a strangler-fig proxy seam; once the port was complete the sidecar and the proxy were removed and the backend collapsed into the shell. That history is recorded in [ADR-0001](../adr/0001-strangler-fig-port.md) (superseded) and [ADR-0013](../adr/0013-in-process-collapse.md). The Python implementation remains as the cross-language testing oracle ([ADR-0005](../adr/0005-cross-language-equivalence-oracle.md)) and is not shipped. For the surrounding runtime topology see the [System overview](overview.md).

## The Rust workspace

The workspace manifest is `frontend/src-tauri/Cargo.toml`. It declares four members with `resolver = "2"`. Three of them (the `eo-*` crates) are deliberately free of any dependency on the Tauri toolchain, so continuous integration can build and test them on a runner without the GUI system stack; that structurally prevents a window-system dependency from creeping into backend code. Only `entropia-orme` is coupled to Tauri.

| Crate | Responsibility | Depends on |
|---|---|---|
| `entropia-orme` | The Tauri shell and composition root: window chrome, the `api_request` IPC dispatch, the domain-event bridge, and the wiring that constructs the native services and publishes them to the running substrate. | `eo-http`, `eo-services`, `eo-wire` (and Tauri) |
| `eo-http` | The axum HTTP substrate: the in-process router, the native route adapters, the request middleware, and the in-process dispatch (`dispatch_in_process`). | `eo-services`, `eo-wire` |
| `eo-services` | The domain service layer: the tracker, quests, codex, character and cost calculators, the scan and OCR services, the chat-log and input listeners, the persistence handle, and the supporting stores. | `eo-wire` |
| `eo-wire` | The wire-format contracts: the typed domain-event union, the event-stream fan-out hub, the domain-event channel, and the cross-language equivalence emitters. | (leaf) |

The dependency direction is strictly one way: `eo-wire` is a leaf, `eo-services` builds on it, `eo-http` builds on both, and `entropia-orme` sits at the top as the composition root. No `eo-*` crate depends on `entropia-orme`, which is what keeps the backend layers Tauri-free.

## Per-crate detail

### `entropia-orme`: the Tauri shell and composition root

Defined by `frontend/src-tauri/entropia-orme/src/lib.rs` (with the composition logic in `frontend/src-tauri/entropia-orme/src/composition.rs`). The library target builds as `lib`, `cdylib`, and `staticlib`. Its public entry point is `run()`, the Tauri application bootstrap. This crate is the only member coupled to the Tauri toolchain, and it owns three concerns:

- **Window and overlay chrome.** The Tauri command handlers `toggle_overlay`, `show_scan_overlay`, and `hide_scan_overlay` manage the hidden overlay windows; on Windows a runtime-icon installer sets DPI-appropriate window icons.
- **The IPC dispatch and the event bridge.** The `api_request` command is the single seam the frontend reaches the backend through: it forwards the request to `eo_http::dispatch_in_process` against the published substrate state and returns the response (`frontend/src-tauri/entropia-orme/src/lib.rs`). The companion `spawn_domain_event_bridge` subscribes to the event-stream hub and re-emits each frame onto the Tauri event system, the in-process replacement for the frontend's former HTTP event stream.
- **Native-service composition.** `composition.rs` resolves the data directory, opens the application database, loads the game-data snapshot, constructs the ported services over the real clock, and discharges the ONNX Runtime obligations for the OCR recogniser (pinning the bundled dynamic library to an absolute path and configuring the execution-provider ladder). `compose_substrate` (`lib.rs`) runs this publish-last: it builds the state, installs the composed services, and only then publishes the state to `api_request`, emitting `substrate:native-installed` so the frontend re-drives its initial reads. Until that point `api_request` answers a not-ready error; if composition declines, the state is never published.

### `eo-http`: the axum in-process substrate

Defined by `frontend/src-tauri/eo-http/src/lib.rs`. The crate's own description calls it "the in-process router and middleware serving every backend route natively". Its public surface centres on `AppState` (the shared router state), `NativeServices` (the composed service bundle handed to `AppState::install_native`), `build_router`, and `dispatch_in_process`.

`AppState` holds the public-boundary Host allowlist and the composed native-service handles. Each native-service handle sits behind an `RwLock<Option<...>>` so composition can install it after the state is constructed; until a handle is present, a route that needs it answers the `503` service-unavailable floor rather than serving. The key modules are:

| Module | Role |
|---|---|
| `native` | The native route adapters and the route-registration table (`register`). Each adapter extracts its route's parameters and calls the corresponding handler, or returns the `503` floor when its service is not composed. |
| `hydration` | The natively-served read handlers, byte-faithful to the backend's response formatting and its strong-ETag conditional-GET semantics. |
| `pyjson` | A reference-faithful JSON reader that reproduces Python's `json` module behaviour (the `Infinity`/`NaN` literals, arbitrary-precision integers, and the specific error messages and positions the validation envelope echoes). |
| `cors` | The backend's CORS contract reproduced for the native arm. |
| `body`, `extract` | The request-body and path/query extraction layers that reproduce the backend's validation envelopes. |
| `analytics_routes`, `character_routes`, `equipment_routes`, `producer_routes`, `scan_routes`, `settings_routes`, `tracking_routes` | Per-area route logic supporting the adapters in `native`. |
| `demo` | The guide-mode read dataset: a parallel hydration + tracker over a writable clone of the bundled demo database, behind the `/api/demo/*` routes. |
| `dev_routes` | The hidden developer-mode-gated routes (the metrics snapshot and the crash-reporting toggle), native-only and off the equivalence surface; they answer `404` when developer mode is disabled. |

The substrate's request path applies the same middleware nesting as the original Python backend: CORS is the outermost layer (a preflight short-circuits routing and the guards when the contract is configured), then the Host and origin guard (`api_guard`), then routing. Strong ETags are computed over the response body with SHA-256 in the hydration handlers. An unmatched path is the framework's own `404`, and an unported method on a registered path is a `405`.

### `eo-services`: the domain services

Defined by `frontend/src-tauri/eo-services/src/lib.rs`. This crate carries the service layer behind the HTTP surface, ported service by service from the Python implementation. Its modules group as follows:

- **Live tracking.** `tracker` (the `HuntTracker` producer spine), `chatlog_watcher` and `chatlog_parser` (the tailing watcher and line grammar), `session_summary`, `tracking_models`, `loot_filter`, and `mob_lookup_service`.
- **Quests and codex.** `quests`, `codex`, and `codex_categories`.
- **Character and cost analytics.** `character_calc`, `cost_engine` (the pure-arithmetic leaf service), `trifecta_service`, `tt_value_curve`, and `tool_inference`.
- **Configuration.** `config_service` (the settings reader and writer).
- **Scanning and OCR.** `ocr_engine` (the recogniser, EP-agnostic; the runtime wiring is a composition-root concern), `screen_capture`, `skill_scan_manual`, `skill_panel`, `scan_completion`, `scan_drift`, `scan_presets`, and `repair_ocr`. The fuzzy text matching used by these services lives in `fuzzy_match` and `difflib`.
- **Input listeners.** `hotbar_listener` and `spacebar_capture_listener` (the two OS keyboard hooks), behind the `keystroke_source` seam that filters keys at the hook boundary and provides an injectable mock for tests.
- **Skill tracking.** `skill_tracker`.
- **Infrastructure.** `game_data_store` (the bundled game-data snapshot), `db` (the persistence handle), `clock` (the injected-clock seam), `event_bus`, and `eu_window`/`paths` (Windows window-enumeration and path resolution). The `fingerprint_recorder` supports the equivalence infrastructure.

The crate is Windows-aware: the input listeners and window enumeration compile platform bindings under `cfg(windows)`. The OCR recogniser binds the ONNX Runtime dynamically, so a host without the runtime skips the engine-running tests honestly rather than failing to build.

### `eo-wire`: the wire contracts

Defined by `frontend/src-tauri/eo-wire/src/lib.rs`. This leaf crate carries the byte-level contracts. It splits into two groups:

- **The wire-contract spine.** `domain_events` is the typed frontend-facing event union (closed in both directions, camelCase payload keys, a required ISO-8601 UTC `occurred_at`, declaration-order serialisation matching the Python `model_dump_json()` output). `bus` is the monomorphic domain-event broadcast channel that makes "a typed event on a domain topic" a compiler-checked invariant. `sse` is the event-stream fan-out hub: one bounded per-client queue (default 256 frames) with drop-oldest delivery, a process-monotonic sequence number shared across each frame's copies, and the `id: N\nevent: <topic>\ndata: <json>\n\n` frame format. The shell's domain-event bridge drains this hub onto the Tauri event system. `models` carries the shared response model types.
- **The cross-language equivalence emitters.** `normalizer` is the shared canonicaliser (UUIDs to sequential symbols, timestamps to symbols, floats rounded to four decimal places, keys sorted, serialised through a faithful reimplementation of Python's `json.dumps` including its float formatting). `fingerprint` emits the event-stream JSONL golden, `db_snapshot` emits the database-state snapshot golden, and `http_fingerprint` emits the HTTP-response golden. Each is a byte-exact port of its `backend/testing/` counterpart and is asserted against the committed Python goldens by the equivalence runner. The oracle these emitters serve is described in [ADR-0005](../adr/0005-cross-language-equivalence-oracle.md).

## The Python oracle

The Python FastAPI implementation remains in the repository under `backend/`, organised into `backend/routers/` and `backend/services/`. It is no longer shipped or run in the application; it is the reference the native ports are graded against. The cross-language equivalence runner replays the recorded scenarios through both implementations and asserts the native output byte-for-byte against the Python goldens, so a native regression fails a test rather than reaching a user ([ADR-0005](../adr/0005-cross-language-equivalence-oracle.md)). The `eo-services` and `eo-wire` modules are ports of the corresponding `backend/services/` and `backend/testing/` code, and the oracle is what keeps the two in step.

## Route serving

Every backend route is served by a native handler in the workspace. A route is registered through `native_routes` (the health route) in `frontend/src-tauri/eo-http/src/lib.rs` and `register` in `frontend/src-tauri/eo-http/src/native.rs`; a registered route whose backing service has not yet composed answers the `503` service-unavailable floor until composition completes, after which it serves. The router registers the following route groups:

| Route group | Registered routes |
|---|---|
| Health | `GET /api/health` |
| Quests | `GET`/`POST /api/quests`; `GET /api/quests/mobs`; `GET /api/quests/analytics`; `GET`/`POST /api/quests/playlists`; `GET /api/quests/playlists/analytics`; `PUT`/`DELETE /api/quests/playlists/{playlist_id}`; `GET`/`PUT`/`DELETE /api/quests/{quest_id}`; `POST` of `{quest_id}/start`, `/complete`, `/cancel` |
| Codex | `GET /api/codex/species`; `GET /api/codex/species/{name}/ranks`; `GET /api/codex/recommend`; `POST /api/codex/calibrate`; `POST /api/codex/claim`; `POST /api/codex/meta/claim`; `GET /api/codex/meta/attributes` |
| Analytics | `GET /api/analytics/overview`; `GET /api/analytics/activity`; the ledger (`GET`/`POST /api/analytics/ledger`, `DELETE /api/analytics/ledger/{entry_id}`), presets (`GET`/`POST /api/analytics/ledger/presets`, `DELETE .../presets/{preset_id}`), and inventory (`GET`/`POST /api/analytics/inventory`, `PATCH`/`DELETE .../{item_id}`, `POST .../{item_id}/sell`) routes |
| Tracking (reads) | `GET /api/tracking/sessions`; `GET /api/tracking/session/{session_id}`; `GET /api/tracking/tag-suggestions`; `GET /api/tracking/snapshot` |
| Tracking (producer) | `POST /api/tracking/start`; `POST /api/tracking/stop`; `GET /api/tracking/manual-mob-suggestions`; `POST` of `/release-mob`, `/manual-mob-lock`, `/tag-lock` |
| Tracking (session edits) | `POST` of session `/rename-mob`, `/restore-mob`, the loot-item flip wildcard, `/armour-cost`, `/quest-link`, `/repair-scan`; `GET .../quest-link-suggestion` |
| Scan | `GET /api/scan/skills/status`; `POST` of `/start`, `/capture`, `/cancel`, `/undo`, `/process`, `/accept`, `/reject`; `GET /api/scan/skills/capture/{page}`; `GET /api/scan/skills/pending`; `POST /api/scan/spacebar-capture` |
| Settings | `GET`/`PATCH /api/settings`; `POST /api/settings/reset`; `GET`/`PUT /api/settings/overlay-position` |
| Character | `GET` of `/calibration`, `/stats`, `/skills`, `/professions`, `/prospect-options`, `/prospect`, `/profession-optimizer`, `/profession-path-optimizer`, `/hp-optimizer`, `/codex` |
| Equipment | `GET /api/equipment/search`; `GET`/`POST /api/equipment/library`; `PUT`/`DELETE /api/equipment/library/{item_id}`; `GET /api/equipment/library/{item_id}/detail`; `POST /api/equipment/cost/calculate` |
| Demo (guide mode) | `GET` of `/api/demo/analytics/overview`, `/api/demo/analytics/activity`, `/api/demo/analytics/ledger`, `/api/demo/analytics/ledger/presets`, `/api/demo/analytics/inventory`, `/api/demo/tracking/sessions`, `/api/demo/tracking/session/{session_id}`, `/api/demo/tracking/snapshot` |

The settings `PATCH /api/settings` and `POST /api/settings/reset` writes are served natively: the handler validates the patch, writes through the configuration service, and signals the live producers (restarting the chat-log watcher on a path change, toggling the hotbar hooks, and reloading the tracker configuration) so a settings change reconciles without a restart. Domain events no longer travel over an HTTP route; they reach the frontend over the Tauri event bridge described in the [System overview](overview.md). Beyond the groups above, the router also registers the hidden developer-mode dev-tools routes (`GET /api/dev/metrics`, `GET`/`POST /api/dev/crash-reporting`); these are native-only, gated on developer mode, and answer `404` when it is disabled, so they sit off the equivalence-covered surface. Every unregistered path is the framework `404`, and an unported method on a registered path is a `405`.

For the event contract carried over the bridge, see the [Event taxonomy](event-taxonomy.md). For the OCR services behind the scan routes, see the [OCR pipeline](ocr-pipeline.md). For the shared SQLite database the read surface and the producer spine both use, see the [Database schema reference](database-schema.md).
