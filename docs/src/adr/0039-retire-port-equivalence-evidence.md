# ADR-0039: Retire the port-equivalence evidence and scale testing to a personal project

- Status: Accepted
- Context: amends [ADR-0016](0016-retire-equivalence-oracle.md) and [ADR-0017](0017-behavioural-contract-ownership.md), and changes the mutation-testing and coverage cadence. EntropiaOrme is a personal application built in public, maintained by one developer; [ADR-0038](0038-promotion-carries-public-polish.md) moves the public surface's obligations to promotion.

## Context and problem statement

When the Python reference implementation was retired, the evidence that the Rust port matched it byte-for-byte was kept as frozen goldens, re-asserted by hermetic tests on every run. Much of that evidence pinned surfaces that no longer exist: the per-endpoint HTTP responses of the retired HTTP API, four midpoint scenarios whose only output was those responses, the raw captures they were rendered from, a round-trip of the listener projections, a schema snapshot of the domain events that the generated TypeScript bindings already hold in step, and forty-four review reports from the ratification protocol [ADR-0037](0037-retire-golden-ratification-guard.md) retired. Each golden move had to regenerate some of it, and none of it could fail for a reason that mattered to the application as it now is.

Three other test surfaces cost more than they returned:

- The native-shell WebDriver suite, with per-surface screenshot baselines, has not run in continuous integration since a WebView2 update broke session creation on the hosted runners. Its baselines can only be regenerated on Windows and had already drifted from the interface.
- The per-file mutation-score floors turned the mutation campaign into a gate, so new code needed a separate coverage pass before a promotion could stay green. The campaign ran daily against a `main` that changes only at promotion.
- Branch coverage re-ran the whole backend suite under instrumentation on every push to `next`, to produce a badge published only from `main`.

## Decision

- **The port-equivalence evidence is removed**: the HTTP-response goldens and their emitter, the four midpoint scenarios, the raw captures and emitter proof, the listener projection mirrors, the domain-event schema snapshot and its conformance test, the synthetic recorded-scenario placeholder, and the ratification reports. Git history keeps them.
- **The replay corpus stays** as the end-to-end regression net for the tracking pipeline: each scripted scenario's event-stream fingerprint and database snapshot. A deliberate change regenerates them and the diff is reviewed in the commit that moves them (ADR-0037).
- **The native-shell WebDriver suite and its screenshot baselines are removed**, with the fixture backend compiled into the e2e build and the e2e-only build flags.
- **Mutation testing becomes a measurement.** The floors and their gate are retired; `cargo xtask mutation-score` reports per-file and aggregate scores and writes the badge. The campaign runs when `main` moves and the campaigned crates changed, and on demand.
- **Coverage is measured on the vetted line only**: pull requests and pushes to `main`, not pushes to `next`.

## Consequences

- The tests that remain assert this codebase's behaviour, not a retired implementation's output; a golden move touches only the replay scenarios.
- Nothing drives the real desktop window in an automated test. The interface is covered by the Vitest component and route suites, and by daily use of the `next` build. A Linux WebDriver job is the path back if that proves insufficient.
- The mutation badge reports the score honestly and can fall; raising it is an optional, deliberate pass.
- A push to `next` runs the same tests as before, minus the instrumented coverage re-run.

## Evidence

- `app/src-tauri/eo-services/tests/corpus_replay_oracle.rs` (the replay corpus)
- `app/src-tauri/xtask/src/mutation_score.rs` (the score report)
- `.github/workflows/mutation.yml`, `.github/workflows/ci.yml`
- `TESTING.md`
