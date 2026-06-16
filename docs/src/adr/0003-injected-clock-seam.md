# ADR-0003: Injected-clock determinism seam

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The application records gameplay event streams and replays them through the
production pipeline to produce golden-observed outputs. A golden is only
reproducible if every value that reaches it is a deterministic function of the
recorded input. Ambient time reads break that property: a service that calls
`datetime.now()` or `time.monotonic()` stamps an output with the wall clock at
replay time, so the same scenario produces a different golden on every run and
the count of reads (how many times the surface happened to sample the clock)
leaks into the result.

The pipeline spans two implementations: the original Python backend and the
native Rust spine it was ported to under the strangler-fig migration. Both must
agree, so both must read time through the same shape, and a frozen, explicitly
advanced clock has to be drivable through every timestamp-producing surface.

## Decision

All time reads flow through an injectable `Clock` abstraction with two methods,
`now()` (a wall-clock instant) and `monotonic()` (seconds since an arbitrary
epoch, where only deltas are meaningful). The two implementations are mirrors:
`backend/testing/clock.py` defines the Python `Clock` ABC, and
`frontend/src-tauri/eo-services/src/clock.rs` ports the same trait to Rust.

Each provides two concrete clocks:

| Implementation | Production | Test |
| --- | --- | --- |
| `RealClock` | delegates to the stdlib (`datetime.now`, `time.monotonic` in Python; `Local::now().naive_local()` and a process `Instant` in Rust) | not used |
| `MockClock` | not used | frozen by default (start defaults to `2026-01-01 00:00:00`), advanced explicitly |

`MockClock.advance(seconds)` moves the wall-clock and monotonic streams in
lockstep and rejects negative (and, in Rust, non-finite) deltas to preserve the
monotonic invariant; `freeze_at(instant)` resets only the wall-clock stream so
the monotonic count survives a scenario that walks through several frozen
instants. The clock is constructed once at the composition root and injected
into services from there, so a recorded scenario can drive deterministic
instants through every output-reaching timestamp (see
[ADR-0004](0004-test-mode-composition-root.md)).

## Consequences

Because every replayed timestamp is plan-derived rather than sampled from the
wall clock, the harness can normalise timestamps to symbols, making goldens
reproducible and independent of how many times a surface reads the clock. The
same seam is what lets the Python and Rust paths be compared against one shared
expectation (see
[ADR-0005](0005-cross-language-equivalence-oracle.md)).

The cost is discipline: production code may not read the ambient clock directly.
This is enforced statically by `backend/scripts/check_ambient_time.py`, a
whole-tree AST lint over the tracked production packages (`backend/services`,
`backend/routers`, `backend/core`, `backend/tracking`, `backend/db`,
`backend/main.py`, `backend/dependencies.py`). It flags `time.time()`,
`time.monotonic()`, `time.perf_counter()`, `datetime.now()`,
`datetime.utcnow()` and `date.today()`, including module- and class-aliased and
bare (uncalled) references and direct `from time import ...` of those callables.
Matching on the AST means strings and comments never false-positive, and
`time.sleep` is deliberately permitted (it produces no value). A genuinely
justified site may carry a same-line `# ambient-time: allowed (<reason>)`
pragma with a non-empty reason; the tree ships with zero pragmas. The guard is
itself covered by `backend/tests/test_ambient_time_guard.py`, which asserts the
live tree is clean and that every forbidden form turns the guard red. The
`testing` package is excluded by construction, since that is where the
`RealClock` stdlib delegation legitimately lives.

A static zero-site assertion is necessary because a golden-stability check
alone passes vacuously on a clock-coupled surface that happens to carry no
golden.

## Evidence

- `frontend/src-tauri/eo-services/src/clock.rs`
- `backend/testing/clock.py`
- `backend/scripts/check_ambient_time.py`
- `backend/tests/test_ambient_time_guard.py`
- `backend/testing/CONSISTENCY.md`
