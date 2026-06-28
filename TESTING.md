# Testing

EntropiaOrme ships as a single pure-Rust binary: a Tauri desktop shell hosting the application backend in-process, with a Svelte frontend. Testing follows that shape in three tracks:

- **The cargo workspace** (`frontend/src-tauri/`): the native backend members and the Tauri shell, run under `cargo nextest`. This is the primary gate on the shipped binary.
- **The frontend track** (`frontend/`): the pure-TypeScript logic layer and the high-logic Svelte components under Vitest and Testing-Library, with Biome owning lint and format.
- **The native-shell end-to-end suite** (`frontend/e2e/`): the real Tauri WebView2 window driven through `tauri-driver`, exercising the desktop IPC boundary and pinning per-surface visual baselines.

This document is the command-level reference; specific counts, coverage percentages, and mutation scores live in the badges and a live run, not in this prose, because they move with every change.

## The equivalence evidence (the rigour story)

The application was originally a Python (FastAPI) backend. It was ported to Rust, and the port was proven by a behavioural-equivalence oracle: for a given scenario's declared inputs, the Python implementation produced a fixed, normalised set of observable outputs (domain events, database state, HTTP responses), pinned as golden files, and the Rust implementation was graded byte-for-byte against them. With the port complete, the Python tree has been retired entirely; the shipped app is the single Rust binary.

The proof survives the retirement. The frozen goldens the port was graded against are committed Rust-side, and a family of hermetic tests re-asserts them on every run **with no second implementation present**: a byte-identical native result is the equivalence evidence, banked permanently. The goldens fall into three groups, all under `frontend/src-tauri/`:

- **The replay corpus** (`fixtures/corpus/`): per-scenario event-stream fingerprints (`expected/fingerprint.jsonl`), database-state snapshots (`expected/db_state.json`), and per-endpoint HTTP-response goldens (`expected/http_responses/`).
- **The contract snapshots** (`contracts/`): the OpenAPI schema snapshot (`openapi.snapshot.json`) and the frontend-facing domain-event schema snapshot (`event_schemas.snapshot.json`).
- **The wire fixtures** (`eo-wire/tests/fixtures/`): the normaliser conformance table and the listener / quest-automation projection mirrors.

These goldens are frozen evidence: the tests below only read and assert them. Changing one is a deliberate re-ratification, governed by the discipline under "Goldens regeneration" below.

### The safety net and the proof it holds

Two ideas sit behind the suite and answer different questions.

- **The replay corpus is the safety net.** It feeds canned versions of the real input surfaces (the chat.log tail) through the production pipeline and pins the externally observable output. That makes "did behaviour change?" a mechanical question. It is cheap; it runs constantly.
- **Mutation testing is the proof the net has no holes.** It deliberately sabotages the code in many small ways and asks whether any test notices. A high mutation score is the evidence that the net is tight enough to catch a silent divergence (an off-by-one, a dropped reset, a sign flip) rather than wave it through. "All my replay tests pass" is a weaker statement without it.

A test that runs code but asserts nothing about the result undermines both: it shows as covered, leaves every behaviour-changing mutant alive, and pins nothing. Coverage proves a line ran; it cannot prove a test would notice if that line were wrong.

## Running the Rust suite

The workspace lives at `frontend/src-tauri/`: the Tauri shell (`entropia-orme`, window orchestration and hosting the backend in-process) plus the native-backend members (`eo-wire`, `eo-http`, `eo-services`) that implement the application logic. The backend members carry the equivalence tests and the bulk of the unit coverage; they build and test without the Tauri system toolchain.

Run the backend members alone (no Tauri toolchain required):

```sh
cd frontend/src-tauri
cargo nextest run -p eo-wire -p eo-http -p eo-services
```

or, from the repository root, the `just` recipe that wraps the same command:

```sh
just test-rust
```

To exercise the whole workspace, the Tauri shell included (needs the platform toolchain, so this is the Windows path):

```sh
cd frontend/src-tauri
cargo nextest run --workspace
cargo test --workspace --doc      # nextest does not run doctests; this leg keeps them covered
```

`cargo nextest` applies a committed per-test terminating timeout (`frontend/src-tauri/.config/nextest.toml`: a 60-second slow period, terminating after two, so a hard 120-second ceiling per test). A subset of the heavy substrate-composition tests stand up the full native spine and install the shared OS keyboard hook, whose attach/detach lifecycle can intermittently block in some headless contexts; the timeout kills and reports such a test rather than stalling the run. The ceiling sits well above any legitimate test (the heaviest, an OCR warm-engine composition, completes well under a minute) and far below the stalls a hung hook produces. CI invokes the workspace through nextest for the same reason.

A `.cargo/config.toml` under the workspace redirects test temporary directories into `target/`, so an interrupted run does not accumulate scratch directories in the OS temp area; reclaim any leftovers with `cargo clean`.

### The equivalence tests

These are the hermetic tests that re-assert the frozen goldens. Each runs with no second implementation; a byte-identical native result is the proof.

- **`eo-services/tests/corpus_replay_oracle.rs`**: replays every scripted scenario through the complete native pipeline (chat-log tail to event bus to tracker to database), then asserts both the event-stream fingerprint and the database-state snapshot byte-for-byte. The two serialisations share one normaliser in fingerprint-then-snapshot order, exactly as the golden harness assigned its encounter-order symbols.
- **`eo-http/tests/http_consistency_replay.rs`**: replays the consistency scenarios through the same pipeline, then drives the read and producer HTTP surface in-memory and re-asserts the per-endpoint response goldens through the native fingerprint emitter.
- **`eo-wire/tests/emitters_proof.rs`**: feeds the committed raw captures (pre-normalisation bus events, database rows, and HTTP responses) through the Rust emitters and asserts byte-equality against the goldens. The raw captures and goldens are committed together, so a stale fixture cannot pass.
- **`eo-wire/tests/conformance.rs`**: replays the normaliser conformance table and checks the native normaliser reproduces every expected output byte-for-byte (and refuses a vacuous pass on an empty table).
- **`eo-wire/tests/openapi_conformance.rs`** and **`eo-wire/tests/event_schema_conformance.rs`**: assert every registered native response model and the native domain-event union against their component in the committed contract snapshots (property sets, required lists, field shapes, nullability, closed-world posture), and round-trip the snapshot's enum values through the real serde implementations.
- **`eo-wire/tests/yml_family.rs`**: asserts the native normaliser and serialiser reproduce the listener and quest-automation projection mirrors byte-for-byte.
- **`eo-http/tests/native_router.rs`**: hermetic router-level coverage driving `build_router(state).oneshot` in-memory (the same router core the production binary serves through the IPC command), pinning route registration, the validation envelopes, and the conditional-GET / CORS contracts without any second toolchain.

### Deterministic scenario clocks

Each corpus scenario commits a clock plan in its `metadata.yaml`:

```yaml
clock:
  start: 2026-01-01T00:00:00
  step_seconds: 1.0
```

The plan defines a frozen, driver-advanced clock for the replay: the scenario clock starts frozen at `start` and only the replay driver advances it, by `step_seconds`, canonically once after the replay has fully drained and before the session stops, so the session boundaries are distinct deterministic instants. Production code under test only ever reads the clock; reads never advance it, so the instants a scenario produces are independent of how many times the implementation reads time. That is what keeps timestamp-bearing output comparable across runs. The replay injects a `MockClock` (`eo_services::clock`) built from the plan; production composes a `RealClock` and is unaffected.

This is the durable discipline behind every golden: the system under test must be a pure function of its declared inputs. Wall-clock time, randomness, environment, and machine timing are not declared inputs; where production needs such a value it is injected as an explicit dependency that defaults to the real source (the clock seam above; the capture and keystroke-source seams for OCR and input). A value that leaks into a golden makes the suite non-deterministic by construction and only accidentally passing.

## Mutation testing

Coverage proves a line ran; it cannot prove a test would notice if that line were wrong. Mutation testing closes that gap: [cargo-mutants](https://mutants.rs) makes small changes to the code (a `<` becomes `<=`, a `+` a `-`, a constant shifts) and re-runs the tests against each one. A mutant the tests catch is *killed*; one that slips through *survives* and marks a weak spot. The **mutation score** (the share of mutants killed) is the suite's effectiveness metric and the headline quality signal for the native logic core.

The campaign targets the backend members; the Tauri shell stays out (its logic is OS-window plumbing behind `cfg(windows)`, and building it needs the Tauri system toolchain). Because a campaign re-runs the tests once per mutant, it is heavy and runs nightly rather than per-change (`.github/workflows/nightly.yml`), on a Linux runner. Run one locally on a POSIX environment with:

```sh
cd frontend/src-tauri
cargo mutants --package eo-wire --package eo-http --package eo-services --in-place
```

The campaign runs `--in-place` because the member tests read committed fixtures from the repository outside the cargo workspace (the relocated corpus under `fixtures/`), which cargo-mutants' default copied build tree would not contain.

The acceptance bar is a **per-file floor map** enforced by the in-tree guard:

```sh
cargo run -p xtask -- mutation-floors --outcomes mutants.out/outcomes.json
```

A file with an adopted floor must hold its score; a file without one is held to the strictest bar (any missed mutant fails); floors only ever ratchet up. A mutant counts as caught when a test failed on it or the mutated build timed out; unviable mutants (the mutation does not compile) leave the denominator. The aggregate score is published as the README's mutation badge. Triaging a survivor is a two-way choice: strengthen a test until it is killed, or, if the mutation is provably equivalent, leave it with a recorded reason. Never lower a floor to make a regressed run pass.

## Goldens regeneration

The equivalence goldens (the corpus fingerprints, DB-state snapshots, and HTTP-response goldens under `fixtures/corpus/`; the contract snapshots under `contracts/`; the wire fixtures under `eo-wire/tests/fixtures/`) assert by default. A deliberate behaviour change is re-ratified by regenerating the affected goldens and reviewing the resulting diff, then recording that the diff is a genuine intended change rather than a regression.

Regenerating a golden re-ratifies whatever the pipeline currently produces, so an unmarked, unscrutinised change can silently lock in a regression: the expected output simply moves to match the regressed code, and every assertion passes again. The first generation of a new golden is the most dangerous case, because no prior golden means no assertion fails, so an over-emission can be pinned as "expected" and pass silently. Treat any golden diff as a behaviour-change review, never a mechanical step.

### Adversarial ratification (the review step)

Regenerating a golden makes you at once the author of the change, the regenerator of the expected output, and its would-be approver: a structural conflict of interest. The honest tell that you owe a second opinion is that you are reaching to change what *correct* means rather than to make the code meet it.

This project is solo, so what stands in for a second pair of eyes is a structural self-review discipline, not a claim that a different person reviewed the change. Before committing any expected-output change, subject it to a recorded adversarial review pass that judges the one question the marker cannot: is this delta a genuine intended behaviour change, or a regression being laundered into the goldens as the new "correct"? The review records a fenced verdict block, committed verbatim:

```text
ORACLE-RATIFICATION
range: <commit-range>
goldens: <comma-separated sets reviewed>
VERDICT: ratification-sound | regression-suspected | needs-user-judgement
```

Commit that report to `frontend/src-tauri/ratifications/<slug>.md` alongside the golden change, naming the changed sets in its `goldens:` field. It lives outside any `expected/` directory so the guard never treats the report itself as a golden. Proceed only on `ratification-sound`; a `regression-suspected` or `needs-user-judgement` verdict means fix the code (or settle the product question) rather than pin the diff. A committed report is required rather than a bare commit trailer on purpose: fabricating a plausible adversarial report that cites real diff elements is a high bar and is reviewer-visible, where a trailer is not.

### Commit-message convention

A regeneration commit takes the subject prefix `test: regenerate goldens` and lists the regenerated sets in the body, so a reviewer sees at a glance which goldens moved and why:

```
test: regenerate goldens for the basic-hunt loot rounding change

Regenerated alongside the loot-value rounding fix:
- basic_hunt_10_events: fingerprint.jsonl, db_state.json
- basic_hunt_10_events: http_responses/ (tracking + quests endpoints)

The contract snapshots are unchanged.
```

### Ratification guard

Both the marker and the recorded verdict are enforced, not merely a courtesy to reviewers. `cargo run -p xtask -- ratify-check --range <BASE>..<HEAD>` runs as a pull-request and merge-queue job (`Golden ratification guard` in `.github/workflows/ci.yml`). It inspects the diff against the base; a golden file is anything matching the committed-golden paths above. If any commit modifies a golden, the guard requires **both**:

- the `test: regenerate goldens` subject prefix on the relevant commit(s); and
- a ratification report added or modified in the same range, carrying a fenced `ORACLE-RATIFICATION` block whose `VERDICT` is `ratification-sound` and whose `goldens:` field names every changed set, committed no earlier than the last golden change in the range.

Three properties make the verdict hard to satisfy by accident. Tying the report to the range stops a sound verdict from a prior regeneration blessing a fresh golden change. The ordering requirement stops a verdict that reviewed an earlier state from blessing a golden edited in a later commit. And the per-set `goldens:` check stops a verdict recorded for one set blessing another. A change missing any of these fails and surfaces the golden diff for review; a change that touches no golden file is ignored, so the guard is inert for ordinary work. The guard fails closed on any range it cannot resolve. What it cannot do is prove the review actually happened or that it was rigorous; that residual is closed by the report being reviewer-visible and by the human merge to `main`.

## Frontend tests

The Svelte frontend has its own unit track, run with Vitest from the `frontend/` directory and gated by the `frontend` CI job alongside the production build, the type-check, and the Biome lint:

```sh
cd frontend
npm run test            # run the suites once (CI mode)
npm run test:watch      # re-run on change during development
npm run test:coverage   # run with a v8 coverage report
```

Scope is two layers, with end-to-end flows out of scope:

- **Unit**: the cleanly-separated pure-TypeScript logic layer (the rendering, formatting, preference, API-facade, realtime, and store-coordination modules under `src/lib/`). Suites are colocated as `<module>.test.ts` next to their source.
- **Component**: the high-logic Svelte surfaces, rendered under Testing Library with `happy-dom` and the Tauri/backend seams mocked.

Tests assert the code's actual behaviour; where a module diverges from what a reader might expect, the divergence is asserted and flagged in-file as a candidate defect rather than papered over.

### Frontend lint and format (Biome)

Biome owns linting and formatting for the frontend's TypeScript, JavaScript, and JSON (Svelte components stay under `svelte-check`). The configuration is `frontend/biome.json`; the generated `src/lib/api/schema.d.ts` and the lockfile are excluded.

```sh
cd frontend
npm run lint     # biome check: lint + format verification (the CI gate)
npm run format   # biome format --write: apply formatting
```

The `frontend` CI job runs `npm run lint` on every change, and a pre-commit hook mirrors it locally through the lockfile-pinned Biome (run `npm ci` in `frontend/` once so the hook can resolve it).

### Generated API client

The typed frontend API client is generated from the committed OpenAPI snapshot (`frontend/src-tauri/contracts/openapi.snapshot.json`). The `frontend` CI job runs `npm run gen:api:check`, which regenerates the client and fails if the committed output drifts from the snapshot. Regenerate it with `npm run gen:api` (or `just gen-api`) after a change that moves the snapshot.

### Runes-native frontend

The Svelte frontend is runes-native: `svelte.config.js` forces runes mode for every non-`node_modules` file, so the legacy Svelte-4 reactivity primitives (`$:` reactive statements, `export let` props) are compile errors rather than a style preference. The guard is the production build itself: `npm run build` (run by the `frontend` CI job) fails on any legacy-reactivity reintroduction, which is why the convention cannot silently rot. New component state uses `$state` / `$derived` / `$effect` and `$props`; `onMount` stays for genuine run-once mount work, and ordinary `svelte/store` subscriptions are orthogonal to runes and are kept as they are.

### Frontend end-to-end tests (native shell)

The end-to-end layer drives the **real desktop shell** (the Tauri WebView2 window), not a browser tab, through [WebdriverIO](https://webdriver.io) and [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/). Driving the real shell is the point: only there does the desktop IPC bridge exist, so the suite can assert the panels render and the IPC surface is live across the boundary a browser-served harness is structurally blind to. The suite lives under `frontend/e2e/`.

```sh
cd frontend
npm run test:e2e          # functional panel flows against the native shell
npm run test:visual       # diff every visual spec against its committed baseline
npm run test:visual:update # regenerate the baselines after an intended UI change
```

The e2e build embeds the frontend and serves it at the application's own `tauri://` origin (native IPC), built with an in-process request-fixture stub (the `e2e-stub` feature) so responses are deterministic, and with chart tweens frozen (`E2E_FREEZE_TWEENS`) so the visual baselines are stable. A `tauri.e2e.conf.json` overlay (capture window size plus a broadened CSP for the stub) is e2e-only and never ships. The visual layer commits per-surface screenshot baselines under `e2e/baselines/` and diffs against them with a small fuzzy tolerance. Baselines are captured in one rendering environment (WebView2 on Windows), so a different renderer will diff: regenerate after an intended change rather than editing the image.

This layer runs on Windows in CI (the `Frontend e2e + visual (native shell, Windows)` job), the application's platform; it provisions `tauri-driver` and the matching Microsoft Edge WebDriver per run.

## Continuous integration

Every pull request, push to `main`, and merge-queue run executes the workflow in `.github/workflows/ci.yml`. On a documentation-only change (every changed file is Markdown) the compiling jobs are skipped, as described under "Documentation-only changes" below.

- **Change scope and CI gate**: a quick detection job classifies whether the change touches code or only documentation, and the compiling jobs run only for a code change. A small always-running `CI gate` sentinel is the single required check in their place: it passes when the change is documentation-only (those jobs were legitimately skipped) or when every gated job succeeded, and fails closed otherwise, so a skip can never let an untested change merge. Branch protection requires only this one context, so the required-check list never drifts as individual jobs are added or renamed.
- **Golden ratification** (pull requests and the merge queue): the `ratify-check` guard, failing when a commit moves a golden without both the marker and a recorded `ratification-sound` verdict for the changed sets (see "Ratification guard" above).
- **Authoring lint** (pull requests and the merge queue): the `authoring-lint` guard flags em dashes and US spellings on the lines a change adds, and a `version-stamps` step asserts the three application version stamps stay in lock-step (see "Authoring lint" below).
- **Frontend**: the generated-client freshness check, the production build, the type-check, the Biome lint, and the Vitest suites.
- **Frontend e2e + visual** (Windows): the native-shell IPC and visual-regression suites (see above).
- **Rust workspace policy** (`fmt` + `audit` + `deny`): formatting, RustSec advisory audit, and supply-chain policy (licence allowlist, bans, registry sources). Source- and lockfile-level only, so it runs unconditionally on a cheap Linux runner (see "Rust workspace checks" below).
- **Rust workspace** (`clippy` + `build` + `test` + doctests, Windows): lints, compiles, and tests every workspace member on the application's real target (most of the shell sits behind `cfg(windows)`).
- **Rust backend members** (`nextest`, Linux): builds and tests the backend members on a runner without the Tauri toolchain, structurally proving they stay free of GUI dependencies, and compile-checks the criterion benches.
- **Rust backend members** (branch coverage): measures per-member branch coverage over the same members with `cargo llvm-cov` on the nightly toolchain (branch instrumentation is nightly-only). The figure is review evidence (whether a member's tests exercise the paths its behaviour rests on) and is published as the README's coverage badge from `main`.

On the merge queue these same jobs run against the integrated commit, so the queue is the pre-merge gate that vets the exact state being merged.

### The merge queue

A change can pass on its own branch yet break once integrated onto the current `main`; a required merge queue closes that gap. When a pull request is ready, merging it adds it to the queue rather than landing it directly; the queue integrates the change onto the current `main` and runs the CI workflow against that integrated commit on a `merge_group` event, then merges automatically once the run is green. Because the queue tests the integrated result, a regression that only appears once a change is combined with the current `main` is caught before it reaches the branch: a change that fails in the queue is dropped from it rather than landing.

### Documentation-only changes

A change that touches only documentation needs none of the compiling jobs: there is no code to test and no frontend to build. The change-scope detection classifies it on both a pull request and the merge queue, so the heavy jobs are skipped in both places. The classification logic runs from the base commit's copy of the classifier, not the head's, so a fork pull request cannot rewrite it to skip the gates; the head's changed-file list is data the fork cannot forge.

Skipping a required check is the hazard: branch protection treats a never-reported required check as pending (deadlocking the merge) and a skipped one as passing (fail-open). The `CI gate` avoids this with an always-running, fail-closed sentinel that stands in for the gated contexts: a documentation-only change goes green in seconds on the sentinel's verdict alone, while a code change still runs and must pass everything. The classification is deliberately conservative: anything other than Markdown counts as code, so the safe direction (run the suite) is the default whenever there is any doubt.

### Nightly

A separate scheduled workflow (`.github/workflows/nightly.yml`) runs the slower checks once a day: the `cargo-mutants` campaign over the backend members, the per-file mutation-floor enforcement, and the mutation-score badge publish from `main` (see "Mutation testing" above).

## Local checks (pre-commit)

A [pre-commit](https://pre-commit.com/) configuration (`.pre-commit-config.yaml`) mirrors the CI gates into the local development loop, so the same failures surface before a push rather than after. The shipped application is a single Rust binary, so the configuration carries no Python tooling: the lint and test gates are the cargo workspace and the in-tree `cargo xtask` guards. Install the git hook once:

```sh
pre-commit install
```

The hooks then run on each commit. To run them across the whole tree on demand:

```sh
pre-commit run --all-files
```

The configured hooks are:

- **Biome** (lint + format) over the frontend, mirroring the CI `npm run lint` step through the lockfile-pinned binary (run `npm ci` in `frontend/` once so the hook can resolve it).
- **`no-bare-setinterval`**: the frontend polling-discipline guard (a `cargo xtask` subcommand), forbidding a bare `setInterval` outside the visibility-gated helper and any reference to the retired tracking event.
- **authoring lint** (em dash + UK spelling), diff-scoped against the staged change, and **version-stamp parity**, both `cargo xtask` subcommands (see "Authoring lint" below).
- general hygiene: end-of-file and trailing-whitespace fixers, YAML and TOML validity, merge-conflict markers, and a mixed-line-ending check (line-ending policy itself is set per file type in `.gitattributes`).

The xtask guards compile the in-tree `xtask` crate once (cached thereafter) and run the same logic CI runs. The CI `pre-commit` job exercises the hygiene hooks in pre-commit's own managed environments; Biome is skipped there (no `node_modules`), the dedicated frontend job being its enforcing gate.

## Authoring lint

Two mechanical authoring rules are enforced as deterministic lint rather than eyeballed in review: no em dashes (U+2014) in authored content, and UK spelling in authored prose. Both are `cargo xtask` subcommands, run in CI over the pull request's `base..head` range and locally over the staged change.

They are **diff-scoped**: they inspect only the lines a change adds, never the whole tree. This is deliberate. The tree carries pre-existing US spellings and em dashes that predate the discipline, and normalising them drive-by is out of scope; checking added lines only binds new content without disturbing the old. The scope differs by rule:

- The **em-dash ban** applies to every added line in a non-exempt file (licence texts, third-party notices, vendored trees and lockfiles, binaries, and generated artefacts are exempt). U+2014 is never code syntax, so an added em dash is always authored content.
- The **UK-spelling check** applies only to added lines in prose contexts (Markdown / plain-text docs, and comment-only lines in code), because tokens like `color` (CSS), `behavior` (DOM API), `center` (a CSS value), and `serialize` (an identifier) are legitimate US-spelled code, not authoring slips. The US-to-UK map is a curated floor, extended as real slips appear.

A companion check, `cargo xtask version-stamps`, asserts the three application version stamps (`frontend/package.json`, the `[workspace.package]` version in `frontend/src-tauri/Cargo.toml`, and `frontend/src-tauri/entropia-orme/tauri.conf.json`) carry an identical version, so a release bump cannot update some and miss others. It is whole-tree rather than diff-scoped (the invariant holds over the current tree at all times). `cargo xtask bump-version <VERSION>` rewrites all three in lock-step.

## Rust workspace checks

The Rust side is the cargo workspace at `frontend/src-tauri/`. All of the commands below run from the workspace root:

```sh
cd frontend/src-tauri
cargo fmt --check                                       # formatting, all members (apply with `cargo fmt`)
cargo clippy --workspace --all-targets -- -D warnings   # lints, warnings promoted to errors
cargo build                                             # compile check, all members (debug profile)
cargo nextest run -p eo-wire -p eo-http -p eo-services  # backend members alone, no Tauri toolchain needed
cargo test --workspace --doc                            # doctests (nextest does not run them)
cargo llvm-cov nextest --branch -p eo-wire -p eo-http -p eo-services  # branch coverage (nightly toolchain)
cargo mutants -p eo-wire -p eo-http -p eo-services --in-place         # mutation testing (nightly CI cadence)
cargo run -p xtask -- mutation-floors --outcomes mutants.out/outcomes.json  # the floor gate over the campaign
cargo bench -p eo-services                              # criterion micro-benchmarks (hot-path figures)
cargo audit -D warnings                                 # RustSec advisories against Cargo.lock
cargo deny check                                        # licences, bans, sources, advisories
```

The CI jobs split by what they need to compile:

- The **policy job** (`fmt` + `audit` + `deny`) is source- and lockfile-level, so it runs unconditionally on a Linux runner.
- The **workspace job** (`clippy` + `build` + `test` + doctests, whole workspace) runs on Windows: most of the shell sits behind `cfg(windows)`, and linting only the Linux configuration would gate the wrong code. The build step is a debug-profile compile check; the release bundle (`tauri build`) stays a release-time step.
- The **members job** runs `cargo nextest` on the backend members (plus a compile check of the criterion benches) on a plain Linux runner with no Tauri system stack installed. That environment is load-bearing: the backend members must stay buildable and testable without the Tauri toolchain, so a GUI dependency creeping into backend code fails this job structurally rather than landing silently.
- The **coverage job** runs `cargo llvm-cov nextest --branch` over the same members (branch coverage needs the nightly toolchain) and publishes the figure as the README's coverage badge from `main`.

Benchmarks run on demand rather than in CI: shared runners produce noisy timings, so CI only compile-checks the benches and real figures are taken locally when a hot path matters.

Two policy files make the audit and licence gates deliberate rather than advisory:

- `.cargo/audit.toml`: the RustSec ignore list, every entry a transitive crate inside the Tauri toolchain with a per-advisory rationale comment. `-D warnings` makes the list load-bearing: an advisory not explicitly ignored there fails CI.
- `deny.toml`: the supply-chain policy: an explicit licence allowlist (a new dependency carrying any other licence fails until the list is deliberately edited), crates-io-only sources, wildcard-version denial, and the advisory ignores.

Review both files together on every Tauri bump; the Tauri version itself is pinned to a named minor in the workspace manifest. The shell tests live in `entropia-orme/src/lib.rs` under `#[cfg(test)]` and cover its pure logic; each backend member carries its own `#[cfg(test)]` suite alongside the equivalence tests above.
