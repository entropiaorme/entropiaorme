# ADR-0030: An explicit startup readiness boundary

- Status: Accepted (supersedes the startup-recovery paragraph of [ADR-0013](0013-in-process-collapse.md))
- Context: reflects the landed implementation

## Context and problem statement

The shell composes the native services off the setup path ([ADR-0013](0013-in-process-collapse.md)), so the webview is already running when composition starts. Until the facade is published, every typed command answers the `unavailable` error. The frontend's only defence was to re-read on a one-shot `substrate:native-installed` event the install emitted.

That left the interval between "webview running" and "facade published" without an owner. On slower hardware the dashboard mounted, issued its first reads, received `unavailable`, and rendered an error before composition finished. The later event re-read the data, but the transient error had already been shown, and a user often had to navigate away and back to clear it. The design also had two structural faults beyond the visible one. A one-shot event is edge-triggered: a listener attached after it fired never learns that the backend is up. And a composition that declined was only logged, so the frontend could not tell "still starting" from "will never start".

## Decision

Startup is an explicit state with three values: **starting**, **ready**, **failed**.

**The shell owns one level-triggered record.** `SubstrateReadiness` (`app/src-tauri/entropia-orme/src/substrate.rs`) is managed on the Tauri builder, so it exists before any window can ask for it. `compose_substrate` settles it exactly once. It settles as `ready` after the complete facade and every lifecycle-owned service are installed, or as `failed` with a closed `DeclineReason` and the logged detail. Composition and installation run in their own task, so a panic inside either still settles the record (as `unexpected`) rather than leaving every window waiting.

**One command answers it, whenever it is asked.** `substrate_ready` returns the settled outcome, waiting for it if composition is still running. A caller that asks after startup finished gets the answer at once, so there is no event to miss. Every window's capability grants this command. It reveals whether the backend is up and, if not, the closed reason; the logged detail, which can carry local paths, is returned to the main window alone, since only the main window hosts the failure surface. The one-shot `substrate:native-installed` event is retired.

**The frontend gates in one place: the typed transport.** `invokeCommand` (`app/src/lib/api/invoke.ts`) awaits `whenSubstrateReady` (`app/src/lib/api/readiness.svelte.ts`) before its first dispatch. That is one readiness question per window, shared by every caller. A command issued while the backend starts is held, not sent, and dispatches once it is ready. If startup failed, it rejects as `unavailable`. No feature retries, sleeps, or suppresses a startup error. Each feature's ordinary loading state simply lasts through startup, and each consumer's own mount-time read is its initial hydration. The frontend re-hydrate relay that answered the old event is removed.

**The frame paints first; data regions show that they are waiting.** The root layout does not wait for the backend. The title bar, the navigation, and every page mount immediately. The page regions that depend on data render a loading placeholder (the shared `Skeleton`) until their first read answers. They never render an empty state that would read as the answer. A single title-bar indicator (`app/src/lib/features/startup/StartupIndicator.svelte`) marks the app as still starting. It appears only if startup takes more than about 150 ms, and adds a "Starting up" label only if the wait passes two seconds. A failed start replaces the page area with a failure surface (`StartupFailure.svelte`): one plain sentence per decline reason, a restart, and the logged detail offered for a report. The navigation is disabled on that surface, because nothing behind it can load.

**The `unavailable` contract stays.** The facade still answers `unavailable` before it is published, as a defence against any caller that bypasses the transport.

## Alternatives considered

- **A blocking splash until ready.** Rejected: it turns a variable composition delay into a fully blocked window on every launch, including fast ones. It also discards the orientation a painted frame gives. Rendering the frame and marking the waiting regions answers faster, and states more truthfully what is and is not known.
- **Transport retries on `unavailable`, or a fixed start-up delay.** Rejected: a retry loop is a polling storm with a guessed interval, and a delay is a guess about hardware. Neither distinguishes a slow start from a failed one.
- **Keeping the one-shot event and re-emitting it on demand.** Rejected: any edge-triggered signal leaves a window that attaches late to reconstruct state it cannot observe. A queryable, level-triggered answer removes that race by construction.

## Consequences

- No window can dispatch a facade command before the facade exists, so the transient backend-not-ready error cannot appear, whatever the hardware speed.
- A failed start is visible, specific, and actionable, instead of being silent in a log while the pages show errors.
- A surface that renders an empty state before its first read resolves is now visibly wrong for as long as startup takes. Loading placeholders are therefore part of each data surface's contract. The startup suites (`app/src/routes/startup.test.ts`, `app/src/routes/page.test.ts`) mount every main page with its reads held, and assert that no read reaches the backend, that no error is shown, and that no listed empty state appears.
- Recurring polls never overlap (`useVisiblePoll` skips a tick while the previous one is in flight), so a poll that falls due during startup costs one held call, not a burst.
- Debug builds honour `ENTROPIAORME_STARTUP_DELAY_MS`, which holds composition back so the slow-start path can be walked by hand on a fast machine.

See also the [architecture overview](../architecture/overview.md) and the [ADR index](index.md).

## Evidence

- `app/src-tauri/entropia-orme/src/substrate.rs`
- `app/src-tauri/entropia-orme/src/lib.rs` (`compose_substrate`)
- `app/src-tauri/entropia-orme/src/composition.rs` (`Composition::Declined`)
- `app/src/lib/api/readiness.svelte.ts`
- `app/src/lib/api/invoke.ts`
- `app/src/routes/+layout.svelte`
- `app/src/routes/startup.test.ts`
