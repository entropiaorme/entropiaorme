# ADR-0001: Strangler-fig Python-to-Rust backend port

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The backend began as a Python FastAPI process shipped as a sidecar binary alongside the Tauri shell, with the SvelteKit frontend wired to a fixed public loopback address for both its hydration reads and its server-sent-event stream. Replacing that sidecar with a native Rust implementation in one cut would have meant a flag day: the entire HTTP and event contract reimplemented at once, with no incremental way to prove each route against the live application and no quick way back if a reimplemented route misbehaved in a shipped build.

Two constraints shaped the alternative. The frontend address must not move: the relay, the generated client, and every consumer are a fixed point that a backend swap cannot be allowed to disturb. And the contract is large and exact: documented status codes, content types, ETags, the deliberately mixed key casings of the snapshot, the validation envelopes, and the streaming handshake of `/api/events` must all be reproduced byte for byte, or an existing oracle fails.

## Decision

Introduce the migration as an in-process strangler-fig seam (`frontend/src-tauri/eo-http/src/lib.rs`) rather than a rewrite. The Tauri shell runs an axum application (`eo-http`) that owns the public loopback address the frontend already targets; the Python sidecar relocates to a private port and is dialled as the proxy upstream.

The substrate is a router whose natively-registered routes take precedence and whose fallback (`any(proxy_fallback)`) carries every other method and path to the sidecar (`build_router` in `frontend/src-tauri/eo-http/src/lib.rs`). Each native route is registered through `arm_routed` / `ArmRoutes`, which consults a per-route override map at request time: `Arm::Native` runs the in-process handler, `Arm::Proxy` forwards to the sidecar (`frontend/src-tauri/eo-http/src/native.rs` registers the full route set; the dispatch lives in `ArmRoutes::on`). The override map (`frontend/src-tauri/eo-http/src/arms.rs`) layers a persisted JSON file under the `ENTROPIAORME_ROUTE_ARMS` environment variable; a registered route absent from the map defaults to `Arm::Native`, so a flipped route can be steered back to the sidecar in an already-shipped build without a rebuild, and an operator typo is skipped rather than fatal.

The proxy arm (`frontend/src-tauri/eo-http/src/proxy.rs`) forwards byte-stably: status, content type, cache-control, ETag, and body bytes pass through unmodified, hop-by-hop headers are stripped per RFC 9110, and the `Host` header is rewritten to the private authority actually dialled (which makes the sidecar's own Host guard a no-op for proxied traffic). Response frames stream as they arrive, with no response timeout, so the event stream's `: ready` flush and keep-alive comments reach the webview unbuffered. The event stream is itself an arm-routed route (`events_stream`): it is served natively from the composed SSE hub when present and proxied otherwise.

Native handlers depend on composed services held behind `RwLock`s in `AppState`. The substrate begins proxy-only and hot-upgrades to native the moment composition succeeds; if composition is absent or declined, every native adapter finds no service (for example `state.hydration()` is `None`) and forwards to the proxy arm, so the substrate degrades to the legacy direct topology without failing.

## Consequences

- A route flip is "register a native handler"; a source revert is "delete the line". Each registration is individually revertable, and the runtime override covers every one as a kill-switch.
- The frontend is untouched: it still addresses one loopback port and cannot observe whether a given route was served natively or proxied.
- Both implementations stay live for the duration of the hybrid, so a misbehaving native route is one override entry (or one boot) away from the known-good sidecar.
- The exact byte contract becomes load-bearing on the proxy arm: any header rewrite beyond the hop-by-hop set, or any buffering of the stream, would be observable, which is why the fidelity axes are pinned rather than assumed.
- The network-quiet steady state survives the seam: an idle client hydrates once, opens one event stream, and issues no further requests until a change is pushed. `backend/tests/test_network_quiet_seam.py` enforces this by recording every request the server handles while driving real state mutations through the production producers, asserting the exact request signature (the snapshot hydrations and the single stream, never a retired poll).
- Equivalence between the two implementations is verified against shared oracles rather than trusted; see [ADR-0005](0005-cross-language-equivalence-oracle.md).

See also the [architecture overview](../architecture/overview.md) and the [service map](../architecture/service-map.md), and the full [ADR index](README.md).

## Evidence

- `frontend/src-tauri/eo-http/src/lib.rs`
- `frontend/src-tauri/eo-http/src/native.rs`
- `frontend/src-tauri/eo-http/src/arms.rs`
- `frontend/src-tauri/eo-http/src/proxy.rs`
- `backend/architecture/README.md`
- `backend/architecture/PORT-READINESS.md`
- `backend/tests/test_network_quiet_seam.py`
