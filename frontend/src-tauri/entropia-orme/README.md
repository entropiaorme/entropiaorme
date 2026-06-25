# Tauri config

## Files

- **`tauri.conf.json`**: base config. Authoritative for release builds.
- **`tauri.dev.conf.json`**: dev-only CSP overlay. Applied by the `tauri:dev` npm script (which `just dev` invokes); release builds via `npm run tauri:build` ignore it and use the base config's strict CSP.
- **`build-dev-config.mjs`**: generates `tauri.dev.local.json` ahead of each dev launch. Writes the env-driven `build.devUrl`: the HTTPS hostname when `ENTROPIAORME_HOSTNAME` is set (served through the local Caddy reverse proxy), otherwise an `http://localhost:<port>` fallback driven by `ENTROPIAORME_FRONTEND_PORT`. Tauri 2 cannot interpolate env vars inside config field values, so this generated overlay is the shim that keeps the dev URL env-driven.
- **`tauri.dev.local.json`**: generated per-checkout overlay (output of `build-dev-config.mjs`), gitignored because its `devUrl` differs per checkout and environment. Regenerated on every `tauri:dev` run, so do not hand-edit it.

## Why a separate dev CSP

The base `tauri.conf.json` ships a strict Content Security Policy appropriate for the production webview. Dev and release both talk to the in-process native backend over Tauri IPC (never to a localhost port), so the dev overlay broadens `connect-src` and `img-src` only for what serving the frontend from a live Vite dev server actually needs:

- `ws://127.0.0.1:*` and `ws://localhost:*` for Vite's HMR websocket (plain-localhost dev URL).
- `https://*.localhost` and `wss://*.localhost` for the dev URL and HMR websocket when served through the local Caddy reverse proxy over HTTPS.

The release build retains the strict CSP because the production webview loads its own bundled assets and reaches the backend only over IPC: there is no dev server and no HMR websocket.

## When to edit

- **Adding a new external host the release app needs to reach**: edit the base `tauri.conf.json` CSP. The dev overlay inherits via the merge; you don't need to also widen it there.
- **Adding a dev-only host (e.g. a temporary mock server)**: edit `tauri.dev.conf.json` so the relaxation never reaches release.
- **Changing the dev URL** (hostname vs plain localhost port): set `ENTROPIAORME_HOSTNAME` / `ENTROPIAORME_FRONTEND_PORT` in the environment. Do not edit `tauri.dev.local.json` directly; it is regenerated on every dev launch.

Any CSP change should be reviewed against the standing security posture before merging; the dev overlay's permissiveness is a deliberate dev-affordance, not a template for production.
