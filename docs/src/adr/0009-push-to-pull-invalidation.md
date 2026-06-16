# ADR-0009: Push-to-pull invalidation

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The desktop frontend renders live hunting-session state that the backend mutates continuously from a chat-log tail loop. The frontend reaches the backend two ways: request/response HTTP for state reads and mutations, and a one-way server-sent-events stream (`GET /api/events`) for change notifications. Two designs were available for the streaming half.

The first folds state into the client from the event payloads: each frame carries a delta or a full snapshot, and the client reduces successive frames into its rendered model. That makes frames large, couples the wire schema to every field the UI shows, and forces the producer to assemble a complete readout on the event path. It also leaves the client fragile under loss: a dropped or stale frame corrupts the reduced model permanently unless the transport guarantees ordered, gap-free replay.

The second treats a frame as a minimal invalidation signal: which surface changed and why, nothing more. Three constraints favoured it: a synchronous, lock-disciplined producer (the bus dispatches inline on the publisher's thread, so event-path work must stay cheap); a bounded per-connection SSE queue that must be allowed to drop frames under a stalled reader without corrupting any client; and one source of render shape rather than two (the snapshot and a parallel reducer).

## Decision

Events are invalidation signals, not state snapshots. A frame names the surface that changed and the reason; the window that receives it re-reads full state from a hydration GET, and rendered state always comes from that snapshot read, never folded from the frame.

On the wire each frame is `id: <seq>`, `event: <topic>`, `data: <envelope JSON>`, where the payload carries only an identifier plus a coarse status and reason (for `tracking.session.updated`: `sessionId`, `status` of `active`/`idle`, `reason` of `started`/`updated`/`stopped`). The frontend store (`frontend/src/lib/stores/trackingStore.ts`) enforces the discipline explicitly: a relayed frame is a pure trigger, every render-shaping value comes from `getTrackingSnapshot()`, and the frame's fields are never reduced into state. The single consolidated readout is `GET /api/tracking/snapshot` (`backend/routers/tracking.py`), whose `active` numbers derive from an owned, immutable value the tracker assembles under its lock; the handler unions that with configuration- and runtime-derived fields and returns one response a newly mounted webview reads once, then keeps current from pushed frames.

Because the snapshot is the only source of shape, the store is reconnect-safe by construction: the relay emits a payload-less nudge on every stream open, and since the store re-reads rather than reduces, an absent payload cannot blank an active hunt on an `EventSource` reconnect.

`Last-Event-ID` replay is intentionally not implemented. As `backend/architecture/README.md` records, the `id:` sequence number is advisory only; gap recovery is push-to-pull re-hydration plus the relay's reconnect nudge, not frame replay.

## Consequences

- Frames stay small and the wire contract stays narrow: a UI field can change without touching any event schema, because no UI field travels on the event path.
- The producer stays synchronous and cheap on the event path; assembling full state is deferred to the hydration GET on a server worker thread.
- The bounded SSE queue (256 frames, drop-oldest) is safe: every frame only triggers a fresh full hydration, so dropping old frames loses nothing because the newest frame still reflects every intervening change. A stalled reader recovers on its next hydration.
- The cost is one HTTP round-trip per change rather than self-contained frames; `Cache-Control: no-cache` plus strong ETags on the hydration surface keep that round-trip cheap by returning `304 Not Modified` with an empty body when nothing changed.
- The consumer is constrained to subscribe-then-hydrate and to treat frames as triggers only; the store coalesces overlapping reads and keeps the last good snapshot on a failed read rather than blanking.

This is enforced, not merely documented. `backend/tests/test_network_quiet_seam.py` drives genuine state mutations through the production producer against a live loopback server wrapped in a request recorder and asserts that a hydrate-and-subscribe client stays current on the snapshot plus the push alone: the typed `tracking.session.updated` frame fires, the snapshot re-read it triggers reflects the mutation, and across the exchange the server sees exactly the snapshot hydrations (one on mount, one per pushed frame) and a single event stream, never the retired status / live / recent-events endpoints or the still-routed `/sessions`. The same test pins the parallel scan-overlay flow on `scan.status.changed`.

## Evidence

- `backend/architecture/README.md`
- `backend/routers/tracking.py`
- `frontend/src/lib/stores/trackingStore.ts`
- `backend/tests/test_network_quiet_seam.py`

## Related

- [ADR-0002: Event spine](0002-event-spine.md)
- [ADR index](index.md)
