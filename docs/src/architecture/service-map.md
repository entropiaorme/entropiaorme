# Service and crate map

The desktop backend exists in two implementations that run side by side. The
original is a Python FastAPI sidecar; the native implementation is a Rust
workspace that runs inside the Tauri shell process. The two are joined by a
strangler-fig HTTP substrate: an axum application that owns the public loopback
address the frontend dials, serves the routes that have been ported in-process,
and reverse-proxies everything else to the relocated Python sidecar. This page
enumerates the Rust workspace crates and the services they own, then sets out
which HTTP routes are served natively against which still proxy to the sidecar.

The strangler-fig shape and its rationale are recorded in
[ADR-0001](../adr/0001-strangler-fig-port.md); this page is the structural
inventory rather than the decision record. For the surrounding runtime topology
see the [System overview](overview.md).

## The Rust workspace

The workspace manifest is `frontend/src-tauri/Cargo.toml`. It declares four
members with `resolver = "2"`. Three of them (the `eo-*` crates) are
deliberately free of any dependency on the Tauri toolchain, so continuous
integration can build and test them on a runner without the GUI system stack;
that structurally prevents a window-system dependency from creeping into backend
code. Only `entropia-orme` is coupled to Tauri.

| Crate | Responsibility | Depends on |
|---|---|---|
| `entropia-orme` | The Tauri shell and composition root: window chrome, sidecar lifecycle, and the wiring that constructs the native services and installs them into the running substrate. | `eo-http`, `eo-services`, `eo-wire` (and Tauri) |
| `eo-http` | The axum HTTP substrate: the in-process router, the native route adapters, the reverse-proxy arm, the request middleware, and the per-route arm overrides. | `eo-services`, `eo-wire` |
| `eo-services` | The domain service layer: the tracker, quests, codex, character and cost calculators, the scan and OCR services, the chat-log and input listeners, the persistence handle, and the supporting stores. | `eo-wire` |
| `eo-wire` | The wire-format contracts: the typed domain-event union, the event-stream fan-out hub, the domain-event channel, and the cross-language equivalence emitters. | (leaf) |

The dependency direction is strictly one way: `eo-wire` is a leaf, `eo-services`
builds on it, `eo-http` builds on both, and `entropia-orme` sits at the top as
the composition root. No `eo-*` crate depends on `entropia-orme`, which is what
keeps the backend layers Tauri-free.

## Per-crate detail

### `entropia-orme`: the Tauri shell and composition root

Defined by `frontend/src-tauri/entropia-orme/src/lib.rs` (with the composition
logic in `frontend/src-tauri/entropia-orme/src/composition.rs`). The library
target builds as `lib`, `cdylib`, and `staticlib`. Its public entry point is
`run()`, the Tauri application bootstrap. This crate is the only member coupled
to the Tauri toolchain, and it owns three concerns:

- **Window and overlay chrome.** The Tauri command handlers `toggle_overlay`,
  `show_scan_overlay`, and `hide_scan_overlay` manage the hidden overlay
  windows; on Windows a runtime-icon installer sets DPI-appropriate window
  icons.
- **Sidecar lifecycle and the substrate topology.** On a bundled release build
  the shell binds the public loopback port for the native substrate, allocates a
  private port for the sidecar, spawns the PyInstaller sidecar relocated onto
  that private port, and serves the strangler router on the public port. Every
  failure path degrades to the legacy direct topology (the sidecar on the public
  port, no proxy) so a substrate fault never takes the app down. A dedicated
  exit seam terminates the sidecar process tree and stops the native producers
  on application exit.
- **Native-service composition.** `composition.rs` mirrors the backend's own
  startup: it resolves the data directory, opens the application database, loads
  the game-data snapshot, constructs the ported services over the real clock,
  and discharges the ONNX Runtime obligations for the OCR recogniser (pinning
  the bundled dynamic library to an absolute path and configuring the
  execution-provider ladder). Composition runs off the serve path, so the
  substrate answers proxy-only the instant it binds and hot-installs the native
  services the moment they are ready; a first launch after an upgrade, where the
  database is briefly below the adoptable baseline while the sidecar migrates it
  forward, upgrades to native without a restart. If composition declines
  permanently, the shell recovers by respawning the relocated sidecar in a
  producing role so live tracking continues.

### `eo-http`: the axum strangler substrate

Defined by `frontend/src-tauri/eo-http/src/lib.rs`. The crate's own description
calls it "the in-process router and middleware that progressively take over
routes from the Python sidecar". Its public surface centres on `AppState` (the
shared router state), `NativeServices` (the composed service bundle handed to
`AppState::install_native`), `build_router`, and `serve`.

`AppState` holds the pooled upstream proxy client, the sidecar authority, the
public-boundary Host allowlist, the hot-swappable arm-override map, and the
composed native-service handles. Each native-service handle sits behind an
`RwLock<Option<...>>` so composition can install it after `serve` has already
started; until a handle is present, the routes that need it fall back to the
proxy arm. The key modules are:

| Module | Role |
|---|---|
| `native` | The native route adapters and the route-registration table (`register`). Each adapter extracts its route's parameters and either calls the corresponding handler or proxies when its service is not composed. |
| `hydration` | The natively-served read handlers, byte-faithful to the backend's response formatting and its strong-ETag conditional-GET semantics. |
| `proxy` | The reverse-proxy arm: forwards a request to the relocated sidecar and streams the response back unmodified, consuming hop-by-hop headers per RFC 9110. |
| `arms` | The runtime per-route arm-override map (`Arm`, `ArmOverrides`). |
| `pyjson` | A reference-faithful JSON reader that reproduces Python's `json` module behaviour (the `Infinity`/`NaN` literals, arbitrary-precision integers, and the specific error messages and positions the validation envelope echoes). |
| `cors` | The backend's CORS contract reproduced for the native arm. |
| `body`, `extract` | The request-body and path/query extraction layers that reproduce the backend's validation envelopes. |
| `analytics_routes`, `character_routes`, `equipment_routes`, `producer_routes`, `scan_routes`, `settings_routes`, `tracking_routes` | Per-area route logic supporting the adapters in `native`. |
| `sse` | The HTTP handler that drains the event-stream hub (the `: ready` opening comment and 15-second keep-alive live here). |

The substrate's request path applies the same middleware nesting as the Python
backend: CORS is the outermost layer (a preflight short-circuits routing and the
guards when the contract is configured), then the Host and origin guard
(`api_guard`), then routing. Strong ETags are computed over the response body
with SHA-256 in the hydration handlers. The route-flip mechanism is deliberately
minimal: registering a native handler is a route flip, deleting that
registration is the source-level revert, and a runtime arm override steers any
flipped route back to the live sidecar in an already-shipped build (see
[Route serving](#route-serving)).

### `eo-services`: the domain services

Defined by `frontend/src-tauri/eo-services/src/lib.rs`. This crate carries the
service layer behind the HTTP surface, ported service by service from the Python
implementation. Its modules group as follows:

- **Live tracking.** `tracker` (the `HuntTracker` producer spine),
  `chatlog_watcher` and `chatlog_parser` (the tailing watcher and line grammar),
  `session_summary`, `tracking_models`, `loot_filter`, and `mob_lookup_service`.
- **Quests and codex.** `quests`, `codex`, and `codex_categories`.
- **Character and cost analytics.** `character_calc`, `cost_engine` (the
  pure-arithmetic leaf service), `trifecta_service`, `tt_value_curve`, and
  `tool_inference`.
- **Configuration.** `config_service` (the settings reader and writer).
- **Scanning and OCR.** `ocr_engine` (the recogniser, EP-agnostic; the runtime
  wiring is a composition-root concern), `screen_capture`, `skill_scan_manual`,
  `skill_panel`, `scan_completion`, `scan_drift`, `scan_presets`, and
  `repair_ocr`. The fuzzy text matching used by these services lives in
  `fuzzy_match` and `difflib`.
- **Input listeners.** `hotbar_listener` and `spacebar_capture_listener` (the
  two OS keyboard hooks), behind the `keystroke_source` seam that filters keys
  at the hook boundary and provides an injectable mock for tests.
- **Skill tracking.** `skill_tracker`.
- **Infrastructure.** `game_data_store` (the bundled game-data snapshot), `db`
  (the persistence handle), `clock` (the injected-clock seam), `event_bus`, and
  `eu_window`/`paths` (Windows window-enumeration and path resolution). The
  `fingerprint_recorder` supports the equivalence infrastructure.

The crate is Windows-aware: the input listeners and window enumeration compile
platform bindings under `cfg(windows)`. The OCR recogniser binds the ONNX
Runtime dynamically, so a host without the runtime skips the engine-running
tests honestly rather than failing to build.

### `eo-wire`: the wire contracts

Defined by `frontend/src-tauri/eo-wire/src/lib.rs`. This leaf crate carries the
byte-level contracts. It splits into two groups:

- **The wire-contract spine.** `domain_events` is the typed frontend-facing
  event union (closed in both directions, camelCase payload keys, a required
  ISO-8601 UTC `occurred_at`, declaration-order serialisation matching the
  Python `model_dump_json()` output). `bus` is the monomorphic domain-event
  broadcast channel that makes "a typed event on a domain topic" a
  compiler-checked invariant. `sse` is the event-stream fan-out hub: one bounded
  per-client queue (default 256 frames) with drop-oldest delivery, a
  process-monotonic sequence number shared across each frame's copies, and the
  `id: N\nevent: <topic>\ndata: <json>\n\n` frame format. `models` carries the
  shared response model types.
- **The cross-language equivalence emitters.** `normalizer` is the shared
  canonicaliser (UUIDs to sequential symbols, timestamps to symbols, floats
  rounded to four decimal places, keys sorted, serialised through a faithful
  reimplementation of Python's `json.dumps` including its float formatting).
  `fingerprint` emits the event-stream JSONL golden, `db_snapshot` emits the
  database-state snapshot golden, and `http_fingerprint` emits the HTTP-response
  golden. Each is a byte-exact port of its `backend/testing/` counterpart and is
  asserted against the committed Python goldens by the equivalence runner. The
  oracle these emitters serve is described in
  [ADR-0005](../adr/0005-cross-language-equivalence-oracle.md).

## The Python sidecar

The Python FastAPI sidecar remains the running implementation for every route
that has not been ported. It still lives under `backend/`, organised into
`backend/routers/` (the HTTP route definitions) and `backend/services/` (the
service layer). The native substrate does not replace the sidecar wholesale; it
adopts routes one at a time and proxies the rest, so both implementations stay
live for the duration of the hybrid.

Each router area continues to be served by Python except for the specific
methods and paths the native substrate has registered. The router files present
are `analytics.py`, `character.py`, `codex.py`, `demo.py`, `equipment.py`,
`events.py`, `health.py`, `quests.py`, `recording.py`, `scan_manual.py`,
`settings.py`, `tracking.py`, plus the shared `response_models.py` and a
`testing.py` router. Routers with no natively-registered counterpart at all
(for example `demo.py`, `recording.py`, and `testing.py`) are served entirely by
the sidecar.

On the service side, the Python modules under `backend/services/` (for example
`chatlog_watcher.py`, `cost_engine.py`, `codex_service.py`, `event_stream.py`,
`local_ocr.py`, `quest_service.py`, `skill_tracker.py`, and the rest) remain the
implementation the sidecar runs against. The Rust `eo-services` modules are
ports of these, but in the relocated topology only one side acts as the producer
at a time: when the native spine composes and owns production, the relocated
sidecar serves proxied reads with its own producers idled, so two implementations
never write the shared database concurrently. If the native spine never composes,
the recovery respawns the sidecar in its producing role instead.

The substrate also proxies the `/api/events` server-sent-event stream to the
sidecar whenever the native event-stream hub is not composed; the sidecar streams
the same contract, so the frontend relay is unaffected by which side serves it.

## Route serving

A route is served natively only if the native substrate has registered a handler
for it (the health route in `native_routes` in
`frontend/src-tauri/eo-http/src/lib.rs`, every other native route in `register`
in `frontend/src-tauri/eo-http/src/native.rs`) and its backing service has
composed. Every method and path the substrate has not
registered, and every registered route whose service is absent, falls through to
the proxy fallback and is served by the Python sidecar. The substrate registers
the following route groups natively:

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
| Settings | `GET /api/settings`; `GET`/`PUT /api/settings/overlay-position` |
| Character | `GET` of `/calibration`, `/stats`, `/skills`, `/professions`, `/prospect-options`, `/prospect`, `/profession-optimizer`, `/profession-path-optimizer`, `/hp-optimizer`, `/codex` |
| Equipment | `GET /api/equipment/search`; `GET`/`POST /api/equipment/library`; `PUT`/`DELETE /api/equipment/library/{item_id}`; `GET /api/equipment/library/{item_id}/detail`; `POST /api/equipment/cost/calculate` |
| Event stream | `GET /api/events` |

Several routes registered natively still proxy in practice when their backing
service is not present. The settings `PATCH /api/settings` and
`POST /api/settings/reset` writes are not registered natively at all and stay
proxied; the overlay-position write is registered because it has no producer
side effects. The `/api/events` stream serves natively only when the event-stream
hub is composed, and otherwise proxies. Every other path and method, including
unregistered methods on a natively-served path and the bare `OPTIONS` the backend
answers itself, falls to the proxy fallback.

### The runtime arm-override map

Each natively-registered route consults a per-route arm at request time. The arm
is one of two values (`arms` module): `Native` runs the in-process handler, and
`Proxy` forwards to the sidecar. A route absent from the override map runs its
default arm, which is `Native` once it has been flipped.

The override map is assembled at startup from two sources, with later entries
winning: a persisted JSON file of `{"<route>": "native"|"proxy"}` (its path
supplied through `ENTROPIAORME_ROUTE_ARMS_FILE`), overlaid by the comma-separated
`route=arm` list in the `ENTROPIAORME_ROUTE_ARMS` environment variable.
Malformed entries are skipped rather than treated as fatal, so an operator typo
cannot take the router down, and an unreadable or malformed file yields the empty
map. Because the router reads this map on every request, and `AppState` exposes a
`set_overrides` method to replace it at runtime, a misbehaving native route is one
override entry away from the known-good sidecar implementation in an
already-shipped build, with no rebuild and no restart.

For the event contract carried over the natively-served and proxied `/api/events`
stream, see the [Event taxonomy](event-taxonomy.md). For the OCR services behind
the scan routes, see the [OCR pipeline](ocr-pipeline.md). For the shared SQLite
database the ported services and the sidecar both read, see the
[Database schema reference](database-schema.md).
