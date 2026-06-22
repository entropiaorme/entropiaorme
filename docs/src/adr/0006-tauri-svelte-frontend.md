# ADR-0006: Tauri 2 and Svelte 5 frontend

- Status: Accepted
- Context: reflects the landed frontend and shell implementation; the backend migration to Rust has since completed (see [ADR-0013](0013-in-process-collapse.md))

> **Transport update ([ADR-0013](0013-in-process-collapse.md)).** The frontend decision below is unchanged, and its central bet was vindicated: the contract was stable enough that the backend changed underneath it without the frontend noticing. The context's "the backend is being migrated ... proxying the remainder" is now history; the migration is complete, the sidecar and proxy are gone, and the frontend reaches the now-native backend through the in-process `api_request` Tauri command rather than over HTTP. Read the present-tense migration prose below as the situation at the time of the decision.

## Context and problem statement

EntropiaOrme is a desktop tool that overlays and analyses a running game session. It must render rich analytics panels, draw transparent always-on-top overlays above another application, and stay responsive to a stream of backend domain events, all while shipping as a single installable Windows binary that bundles its own backend process.

That set of constraints rules out a plain browser application and argues for a native shell hosting a web frontend. The shell must own real windows (sizing, transparency, always-on-top, taskbar visibility), expose a window-to-window event bus, and embed the backend executable as a sidecar. The frontend, in turn, must talk to that backend over a contract stable enough that the backend implementation can change underneath it without the frontend noticing.

The latter point is concrete rather than hypothetical: the backend is being migrated from a Python sidecar to a native Rust HTTP service that progressively takes over routes, proxying the remainder to the Python backend. The frontend had to be insulated from that change by construction, not by convention.

## Decision

The desktop shell is Tauri 2; the frontend is SvelteKit on Svelte 5, built to static assets and served inside Tauri webviews.

The frontend stack is pinned in `frontend/package.json`: `svelte ^5.51.0` (runes reactivity), `@sveltejs/kit ^2` with `@sveltejs/adapter-static`, Vite, and `@tauri-apps/api ^2` plus the official shell and store plugins. TypeScript runs in `strict` mode with `moduleResolution: "bundler"` and `checkJs` over the Svelte-generated base config (`frontend/tsconfig.json`).

Window topology is declared in `frontend/src-tauri/entropia-orme/tauri.conf.json`. Three windows are configured: an undecorated main window, plus an `overlay` and a `scan-overlay`, both transparent, `alwaysOnTop`, `skipTaskbar`, and crucially `visible: false`. The overlays are spawned at launch and kept hidden until needed, so showing one is a visibility toggle rather than a window creation. A content-security policy restricts `connect-src` to `self`, the loopback backend origin, and `entropiaorme.com`.

Backend events reach every window through a single relay. `frontend/src/lib/realtime/eventRelay.ts` runs only in the main window (the one window guaranteed alive for the application lifetime, since closing it exits the app), opens the backend's server-sent-events stream once, and re-emits each frame onto the Tauri event bus. Other windows, including the hidden overlays, receive state changes by subscription rather than by polling or by opening duplicate streams.

The HTTP client is generated, not hand-written. `frontend/package.json` runs `openapi-typescript` over the committed OpenAPI snapshot (`backend/tests/expected/openapi.snapshot.json`) to produce the typed `schema.d.ts` that the `openapi-fetch` client is parameterised on. Every call site therefore has its path, method, parameters, and request body checked against the backend contract at compile time, and operation paths and casing match the contract exactly.

## Consequences

The frontend is language-agnostic about its backend. Because both the SSE wire contract and the HTTP/OpenAPI contract are fixed, routes can move from Python to Rust underneath the frontend without touching the relay or the client: the webview never learns which language served a given response.

Pre-spawning the overlays trades a small constant startup cost for instant show/hide and a stable window topology. Routing all event fan-out through the single main-window relay avoids duplicate streams at the cost of a hard dependency on the main window staying alive.

The generated client constrains the frontend to the committed contract. Drift is caught mechanically: `gen:api:check` regenerates the client and fails on any diff against the committed `schema.d.ts`, and it runs in CI (`.github/workflows/ci.yml`) and via the project `justfile`. The contract itself, and the equivalence guarantee that lets the backend be swapped underneath it, are covered by [ADR-0005](0005-cross-language-equivalence-oracle.md).

## Evidence

- `frontend/package.json`
- `frontend/src/lib/realtime/eventRelay.ts`
- `frontend/src/lib/api/client.ts`
- `frontend/src-tauri/entropia-orme/tauri.conf.json`
- `frontend/tsconfig.json`
- `.github/workflows/ci.yml`
