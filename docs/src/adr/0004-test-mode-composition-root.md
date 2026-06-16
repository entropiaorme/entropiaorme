# ADR-0004: Test-mode composition root

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

Deterministic scenario replay needs the backend to run against test doubles: a frozen clock, injectable input sources, redirected chat-log and screen-capture inputs, and a complete record of every event the process publishes. Threading per-call `if test_mode` branches through the services would scatter the substitution across the hot path, leave production code carrying test concerns, and make it impossible to assert that production wiring is unaffected.

Two further constraints shape the design. A packaged install must never expose the replay surface, whatever its environment holds. And the harness that drives replay is an external process: it cannot reach into the backend to poke objects, so every test seam must be selectable from the environment at startup and drivable through the public HTTP and event-stream surface.

## Decision

Test mode is resolved once, in the composition root, and selects concrete dependencies for the lifetime of the process; no service carries a runtime test-mode branch. `TestModeConfig.from_env` (`backend/testing/config.py`) reads `ENTROPIA_TEST_MODE` and the scenario, chat-log, and fixture path variables into a process-wide overlay. The app's startup wiring (the `lifespan` context in `backend/main.py`) consults that overlay and, when it is enabled, swaps in:

- a `MockClock` frozen at `ENTROPIA_TEST_CLOCK_START` instead of the real clock (see [ADR-0003](0003-injected-clock-seam.md));
- `MockKeystrokeSource` instances for the hotbar and spacebar listeners in place of the live OS-hook sources, behind a shared `KeystrokeSource` abstract base class (`backend/testing/keystroke_source.py`);
- a chat-log watcher tailing the redirected replay sink;
- `SequencedFixtureCapturer` capturers serving the scenario's recorded panel images;
- an `EventsJsonlSink` writing `data/events.jsonl`, installed before any producer starts so it observes every publish.

The test-only router (`backend/routers/testing.py`) is registered only when the overlay is enabled, so in production the surface 404s at the routing layer and stays absent from the OpenAPI schema. `_build_test_mode` returns an inert config whenever `sys.frozen` is set, so a frozen build refuses test mode outright. An external harness drives replay through this surface: `ExternalBackendLeg` (`backend/testing/external_process.py`) boots the backend as a subprocess with the test-mode environment, posts to the synchronous `/api/testing/replay` route, and captures the event stream, database, and hydration responses as the recorded ground truth (see [ADR-0005](0005-cross-language-equivalence-oracle.md)). Scenarios are authored with the gameplay DSL (`backend/testing/dsl.py`), whose output is indistinguishable from a recorded session at the harness layer.

## Consequences

Production wiring is byte-for-byte unchanged: services never see test concerns, and seam selection is a one-shot decision rather than a hot-path cost. The replay surface cannot leak into a shipped build, because freezing disables it at the composition root and the routes are never registered.

Each test-only handler also re-checks the gate server-side (`backend/routers/testing.py`, `backend/routers/recording.py`), so an accidental registration stays inert (403) as defence in depth. `backend/tests/test_test_mode_wiring.py` enforces the contract from both sides: production wiring keeps the live input sources, installs no sink, and 404s the test routes; test mode selects the mock sources, redirected chat-log, fixture capturers, and sink; and a build with `sys.frozen` set never registers `/api/testing`.

## Evidence

- `backend/testing/external_process.py`
- `backend/testing/dsl.py`
- `backend/routers/testing.py`
- `backend/routers/recording.py`
- `backend/testing/keystroke_source.py`
- `backend/main.py`
- `backend/testing/config.py`
- `backend/dependencies.py`
- `backend/tests/test_test_mode_wiring.py`
