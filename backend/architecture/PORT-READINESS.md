# Port readiness

> **Historical.** The native port is complete: the backend now runs as a single in-process Rust spine inside the Tauri shell (PR #150), and the Python implementation is retained only as the cross-language equivalence test oracle, not as a shipped runtime. This document is preserved as the port-readiness record (what mapped mechanically, what needed a deliberate decision, what was kept on purpose); it describes the analysis done ahead of the port, not pending work.

The backend's architecture was deliberately shaped so that a native port of the sidecar is a translation exercise rather than a redesign: the patterns in [`README.md`](README.md) were chosen to have direct, idiomatic equivalents in a systems language. This file inventories that mapping honestly: what ports mechanically, what needs a deliberate decision, and which Python-specific choices were kept on purpose.

"Port" here means a contemplated native rewrite of this Python sidecar (the working target sketch is Rust inside the Tauri shell, collapsing the two-runtime topology into one process). It does not mean operating-system portability. Nothing in this file commits to the rewrite; it records the readiness so the option stays cheap. The companion [`PORTING-RULEBOOK.md`](PORTING-RULEBOOK.md) turns this inventory into application-ready rules for an implementer.

## Why the shapes were chosen ahead of time

A rewrite that has to design its concurrency model while also fighting a borrow checker pays for both at once. The architecture work that preceded this file landed the hard design decisions in Python first, where they could be proved against the live app and pinned by tests:

- state ownership is explicit (one service, one lock, one owner per mutable field);
- events are typed, versioned, closed-schema envelopes whose JSON form is golden-pinned;
- the HTTP surface is hydration-only, contract-pinned by an OpenAPI golden and a generated client;
- workers are named, owned, and enforced by tests rather than convention.

A port translates those settled shapes and inherits their test oracles: the replay corpus, the HTTP and event-schema goldens, and the network-quiet seam test do not care which language produced the bytes. A ported service that reproduces the same fingerprints against the same recorded scenarios is equivalent by construction.

## What ports mechanically

| Today (Python) | Native equivalent | Notes |
|---|---|---|
| `DomainEvent` discriminated union (`type` literal tag, camelCase payloads, `extra="forbid"`) | `#[serde(tag = "type")] enum DomainEvent` with `#[serde(rename_all = "camelCase")]` | Field-for-field; `occurred_at` ISO-8601 UTC maps to `chrono::DateTime<Utc>` in RFC 3339 form. The wire bytes are already golden-pinned by `test_event_schema_drift.py`. |
| `EventStreamHub` fan-out (bounded per-connection queues, drop-oldest) | `tokio::sync::broadcast`, or a fan-out task feeding one bounded `mpsc` per connection | Behaviourally equivalent under push-to-pull (a slow reader loses the oldest frames and self-heals on the next hydration), but `broadcast` is a single shared ring, not the per-connection isolation the Python hub gives; the `mpsc`-per-connection shape is the structural twin. Make the capacity and isolation choice consciously. |
| Actor-shaped services (own lock, own state, publish after release) | One struct per service owning its state; commands in, events out | The single-owner property already holds; the borrow checker verifies a design that exists rather than forcing one into existence. |
| Named, owned worker threads (`chatlog-watcher` and friends) | Named spawned tasks owned by their service, joined on shutdown | `test_supervised_workers.py` already enforces the ownership discipline the task model wants. |
| Hydration HTTP surface + OpenAPI golden | Any spec-generating HTTP layer (`axum` + `utoipa` is the working sketch) | The committed `openapi.snapshot.json` is the contract. Preserve the exact path strings and the deliberately mixed key casing, and the same `gen:api` run regenerates a drop-in TypeScript client; the frontend client keys on path strings, so operation naming differences are a non-issue. |
| ETag middleware (strong SHA-256 ETags, `no-cache`, 304 on match) | Equivalent response-layer middleware | Pure function of the response body; `test_etag.py` pins the semantics either side of the port. |
| SSE endpoint contract (`: ready` open, `id:`/`event:`/`data:` frames, 15 s keep-alive) | Any streaming response implementation | The contract is documented prose plus seam tests, deliberately outside OpenAPI; the frontend relay and stores do not change at all. |

The frontend is the fixed point: the relay, the stores, the generated client, and every consumer are untouched by a backend swap. The seam tests (`test_network_quiet_seam.py`, `test_event_stream_seam.py`) describe the behaviour any implementation must reproduce.

## What needs a deliberate decision

These are the places where the Python shape should **not** be translated literally; each is an improvement the port makes by construction, or a feasibility question to resolve before committing.

### The shared SQLite connection

Today: one `check_same_thread=False` connection in WAL mode behind a wrapper whose lock is honoured unevenly. The skill tracker writes under it; most GET handlers and the provider closures wired in the application lifespan read the bare connection from the server's worker threads without it; the tracker holds its own lock and touches the connection only outside it, with one documented provider-callback exception ordered tracker-lock-first. The single-producer reality (chat-driven writes all run on the watcher thread) rules out writer/writer races in practice, but it does not serialise a worker-thread read against a concurrent watcher-thread write, which is the multi-step cursor-coherency case the wrapper's own warning describes. It is a convention with a hole.

Port decision: do not translate the lock layout. Give the database one owner (a single writer task or a connection pool with explicit transaction scope) and route every service through it. That removes the uneven discipline, the lock-ordering invariant, and the provider-callback exception in one move.

### The event bus typing gap

Today: one `Any`-typed bus carries both the low-level dict events and the typed domain envelopes; "a typed instance on a domain topic" is producer-side convention, and the SSE hub type-guards at runtime.

Port decision: split the layers structurally. A monomorphic channel for domain events (`broadcast::Sender<DomainEvent>`) makes the convention a compile-time guarantee and turns the hub's defensive branches into dead code. The low-level layer can stay loosely typed internally or gain its own enum; that choice is free once the wire layer is monomorphic.

### The per-keypress worker

Today: the hotbar listener spawns a short-lived `hotbar-resolve` thread per keypress to keep DB reads off the OS hook thread.

Port decision: replace spawn-per-event with one owned worker draining a channel. The pattern exists in Python only because spawning a thread was the cheapest correct move; a task-and-channel runtime makes the owned-worker shape the cheapest correct move instead.

### OCR

Today: recognition runs through a bundled SVTRv2 ONNX model (`onnxruntime`, with the DirectML execution provider on Windows) behind a third-party recogniser wrapper, with OpenCV preprocessing and fuzzy text matching. This is the port's highest-risk surface and its feasibility gate:

- the ONNX model itself is runtime-agnostic, and native ONNX bindings exist (`ort`);
- the preprocessing and postprocessing around it, and the recogniser wrapper's behaviour, must be reproduced and verified against the OCR ground-truth corpus the test infrastructure already carries;
- the provider-selection and packaging glue (DirectML vs CPU fallback, and the build-time handling around the two `onnxruntime` distributions) is Python-packaging-specific and does not translate; it gets redesigned, not ported.

A port should treat OCR as a gated spike: prove text-equivalence on the corpus before committing, and keep "everything except OCR" as an acceptable fallback shape.

### Input listeners

Today: the two OS keyboard hooks run through `pynput` behind a `KeystrokeSource` seam (`backend/testing/keystroke_source.py`) with a constructor `key_allowlist` that filters at the hook boundary, plus a mock twin for tests. The seam is the prepared edge: production code consumes the abstraction, not the library.

Port decision: the hook itself becomes a platform API binding (on Windows, a low-level keyboard hook via the platform crate). The two properties that must survive are the allowlist filtering at the hook boundary (the input-minimisation posture) and the mock seam (the replay tests depend on injectable keystrokes).

### Configuration persistence

Today: a plain dataclass serialised by hand to `settings.json` with an atomic write-and-swap plus a `.bak` recovery file. Deliberately not a framework. Port decision: a serde struct with the same atomic-swap semantics. Three contracts must survive: the on-disk JSON shape, the recovery behaviour, and the unknown-key merge-forward (a save reads the existing file and carries forward keys it does not recognise, so values written by third-party tooling survive a save by a process that does not know about them; a naive struct round-trip drops them, so the port needs an explicit catch-all field for unrecognised keys).

### Windows-specific glue

Today: raw `ctypes` calls against `user32` (window enumeration for the game client) and `kernel32` (process priority), plus packaging-detection via the frozen-build markers. All of it is FFI by hand; a native port replaces it with the platform crate equivalents and its own packaging story. None of it is load-bearing architecture.

## What stays Python-shaped on purpose

Kept deliberately, with eyes open, because each earns its place at this boundary:

- **Pydantic at the wire.** Response models are descriptive (`_Loose`, `extra="allow"`) so they can never silently truncate a handler's output; domain events are closed (`extra="forbid"`) because the wire contract is the product. The split is the design; the library is incidental and maps to serde cleanly.
- **Middleware nesting.** ETag is the outermost layer, wrapping the origin guard, with CORS innermost (Starlette runs the last-added middleware first; `backend/main.py` adds CORS, then the origin guard, then ETag). The nesting matters for the 304 path: the ETag layer must wrap the others so a 304 still carries the headers the inner layers add (the note in `backend/middleware/etag.py` spells this out). Any replacement layer must preserve the nesting.
- **The uniform-float numeric convention.** Numeric value fields serialise as JSON numbers in float form regardless of integral value; counts, ranks, and identifiers stay integers. Chosen because it is also the natural serialisation in an `f64`-typed implementation, removing a whole class of wire-form differences at port time.
- **The chat-log tail loop.** A 100 ms polling tail on one named thread, rather than filesystem notifications: simple, deterministic under replay, and the cadence is part of the recorded-scenario timing model. A port keeps the loop shape (one owned task) even if it swaps the discovery mechanism.

## Known hazards a port resolves by construction

Carried here so they are owned rather than rediscovered:

- the uneven database-lock discipline (closed by single-ownership; see above);
- the tracker's provider-callback exception to "no DB under the lock" and its one-directional lock-order invariant (dissolved by the same move);
- the bus typing gap and the hub's runtime type-guard (closed by a monomorphic event channel);
- the absent `Last-Event-ID` replay on the event stream: the sequence number is advisory today, and gap recovery is re-hydration by design. A port should make the same choice consciously (keep the advisory id and the push-to-pull recovery) rather than inherit it silently.

## Contract continuity checklist

What must hold on both sides for the existing oracles to carry the port:

- OpenAPI: identical path strings; identical key casing (including the snapshot's deliberate snake-and-camel mix); the committed snapshot regenerates an identical client.
- Events: identical topics, discriminators, and payload schemas; the event-schema golden passes unchanged.
- Stream: identical frame format, keep-alive cadence, and ready-comment handshake; the seam tests pass against the new server.
- Conformance: the schemathesis contract suite passes against the new server, proving live 2xx bodies conform to the declared schemas at runtime (spec byte-equality alone does not prove what a fresh implementation actually emits).
- Behaviour: identical fingerprints over the recorded replay corpus, which is the deepest equivalence check the infrastructure offers.
