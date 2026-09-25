# Testing

EntropiaOrme ships as a single pure-Rust binary: a Tauri desktop shell hosting the application backend in-process, with a Svelte frontend. Testing follows that shape in three tracks:

- **The cargo workspace** (`app/src-tauri/`): the native backend members and the Tauri shell, run under `cargo nextest`. This is the primary gate on the shipped binary.
- **The frontend track** (`app/`): the pure-TypeScript logic layer and the high-logic Svelte components under Vitest and Testing-Library, with Biome owning lint and format.
- **The native-shell end-to-end suite** (`app/e2e/`): the real Tauri WebView2 window driven through `tauri-driver`, exercising the desktop IPC boundary and pinning per-surface visual baselines.

This document is the command-level reference; specific counts, coverage percentages, and mutation scores live in the badges and a live run, not in this prose, because they move with every change.

## The equivalence evidence (the rigour story)

The application was originally a Python (FastAPI) backend. It was ported to Rust, and the port was proven by a behavioural-equivalence oracle: for a given scenario's declared inputs, the Python implementation produced a fixed, normalised set of observable outputs (domain events, database state, HTTP responses), pinned as golden files, and the Rust implementation was graded byte-for-byte against them. With the port complete, the Python tree has been retired entirely; the shipped app is the single Rust binary.

The proof survives the retirement. The frozen goldens the port was graded against are committed Rust-side, and a family of hermetic tests re-asserts them on every run **with no second implementation present**: a byte-identical native result is the equivalence evidence, banked permanently. The goldens fall into three groups, all under `app/src-tauri/`:

- **The replay corpus** (`fixtures/corpus/`): per-scenario event-stream fingerprints (`expected/fingerprint.jsonl`), database-state snapshots (`expected/db_state.json`), and per-endpoint HTTP-response goldens (`expected/http_responses/`).
- **The contract snapshot** (`contracts/`): the frontend-facing domain-event schema snapshot (`event_schemas.snapshot.json`).
- **The wire fixtures** (`eo-wire/tests/fixtures/`): the normaliser conformance table and the listener / quest-automation projection mirrors.

These goldens are frozen evidence: the tests below only read and assert them. Changing one is a deliberate re-ratification, governed by the discipline under "Goldens regeneration" below.

### The safety net and the proof it holds

Two ideas sit behind the suite and answer different questions.

- **The replay corpus is the safety net.** It feeds canned versions of the real input surfaces (the chat.log tail) through the production pipeline and pins the externally observable output. That makes "did behaviour change?" a mechanical question. It is cheap; it runs constantly.
- **Mutation testing is the proof the net has no holes.** It deliberately sabotages the code in many small ways and asks whether any test notices. A high mutation score is the evidence that the net is tight enough to catch a silent divergence (an off-by-one, a dropped reset, a sign flip) rather than wave it through. "All my replay tests pass" is a weaker statement without it.

A test that runs code but asserts nothing about the result undermines both: it shows as covered, leaves every behaviour-changing mutant alive, and pins nothing. Coverage proves a line ran; it cannot prove a test would notice if that line were wrong.

## Running the Rust suite

The workspace lives at `app/src-tauri/`: the Tauri shell (`entropia-orme`, window orchestration and hosting the backend in-process) plus the native-backend members (`eo-wire`, `eo-services`, `eo-api`) that implement the application logic. The backend members carry the equivalence tests and the bulk of the unit coverage; they build and test without the Tauri system toolchain.

Run the backend members alone (no Tauri toolchain required):

```sh
cd app/src-tauri
cargo nextest run -p eo-wire -p eo-services -p eo-api
```

or, from the repository root, the `just` recipe that wraps the same command:

```sh
just test-rust
```

To exercise the whole workspace, the Tauri shell included (needs the platform toolchain, so this is the Windows path):

```sh
cd app/src-tauri
cargo nextest run --workspace
cargo test --workspace --doc      # nextest does not run doctests; this leg keeps them covered
```

`cargo nextest` applies a committed per-test terminating timeout (`app/src-tauri/.config/nextest.toml`: a 60-second slow period, terminating after two, so a hard 120-second ceiling per test). A subset of the heavy substrate-composition tests stand up the full native spine and install the shared OS keyboard hook, whose attach/detach lifecycle can intermittently block in some headless contexts; the timeout kills and reports such a test rather than stalling the run. The ceiling sits well above any legitimate test (the heaviest, an OCR warm-engine composition, completes well under a minute) and far below the stalls a hung hook produces. CI invokes the workspace through nextest for the same reason.

A `.cargo/config.toml` under the workspace redirects test temporary directories into `target/`, so an interrupted run does not accumulate scratch directories in the OS temp area; reclaim any leftovers with `cargo clean`.

### The equivalence tests

These are the hermetic tests that re-assert the frozen goldens. Each runs with no second implementation; a byte-identical native result is the proof.

- **`eo-services/tests/corpus_replay_oracle.rs`**: replays every scripted scenario through the complete native pipeline (chat-log tail to event bus to tracker to database), then asserts both the event-stream fingerprint and the database-state snapshot byte-for-byte. The two serialisations share one normaliser in fingerprint-then-snapshot order, exactly as the golden harness assigned its encounter-order symbols.
- **`eo-wire/tests/emitters_proof.rs`**: feeds the committed raw captures (pre-normalisation bus events, database rows, and HTTP responses) through the Rust emitters and asserts byte-equality against the goldens. The raw captures and goldens are committed together, so a stale fixture cannot pass.
- **`eo-wire/tests/conformance.rs`**: replays the normaliser conformance table and checks the native normaliser reproduces every expected output byte-for-byte (and refuses a vacuous pass on an empty table).
- **`eo-wire/tests/event_schema_conformance.rs`**: asserts the native domain-event union against its counterpart in the committed event-schema snapshot (property sets, required lists, field shapes, nullability, closed-world posture), and round-trips the snapshot's enum values through the real serde implementations.
- **`eo-wire/tests/yml_family.rs`**: asserts the native normaliser and serialiser reproduce the listener and quest-automation projection mirrors byte-for-byte.

### Deterministic scenario clocks

Each corpus scenario commits a clock plan in its `metadata.yaml`:

```yaml
clock:
  start: 2026-01-01T00:00:00
  step_seconds: 1.0
```

The plan defines a frozen, driver-advanced clock for the replay: the scenario clock starts frozen at `start` and only the replay driver advances it, by `step_seconds`, canonically once after the replay has fully drained and before the session stops, so the session boundaries are distinct deterministic instants. Production code under test only ever reads the clock; reads never advance it, so the instants a scenario produces are independent of how many times the implementation reads time. That is what keeps timestamp-bearing output comparable across runs. The replay injects a `MockClock` (`eo_services::clock`) built from the plan; production composes a `RealClock` and is unaffected.

### Scripted scenario steps

A scenario can also commit a `steps.jsonl` script, for behaviour the chat log alone cannot drive (hotbar-led healing, context changes, restarts). Each line is one step, run in order, and the script must consume every tick group of `chat_replay.log`:

```jsonl
{"chat": 1}
{"advance": 1.0}
{"hotbar": {"slot": "3", "equipment_id": 9, "item_name": "FAP", "item_kind": "healing", "cost_per_use_ped": 0.03, "reload_seconds": 3.0, "healing_profile": {"mode": "direct", "direct_min": 60.0, "direct_max": 100.0}}}
{"segment": "Boss"}
{"decide": "confirm"}
"restart"
```

`chat` streams the next N tick groups and drains them; `advance` moves the frozen clock by that many seconds; `hotbar` publishes a resolved hotbar press stamped at the clock's current instant (`item_kind` is `healing` or `weapon`); `segment` declares a segment, or ends the standing one when `null`; `restart` is a crash, in which the old process's bus and chat-log tail go quiet and a fresh bus, tail, and tracker open the same database, recovering the orphaned session before the next one starts. `decide` is the player's call on the standing weapon mismatch (`confirm` or `keep`), and fails the replay if none stands. A scenario may also commit a `carried.json`, the weapons its tracker carries (`[{"equipment_id": 1, "name": "Pistol", "damage": 10.0, "decay": 5.0}]`: one impact damage figure, whose half-to-full range is the weapon's damage band, and a per-shot decay in PEC); a weapon may add an `effect_profile`, its declared damage-over-time effect exactly as Equipment stores it, whose cast range then replaces the damage band; without the file nothing is carried and every shot records unpriced. Every producer publish completes its tracker dispatch before returning, so each step observes the effects of the ones before it. A scenario without a script streams its whole chat log exactly as before. The `healing_effect_rotation` scenario is the reference script for healing, `weapon_attribution_mismatch` for weapon attribution, and `dot_weapon_rotation` (a real capture's damage-over-time rotation) for weapon effect windows.

This is the durable discipline behind every golden: the system under test must be a pure function of its declared inputs. Wall-clock time, randomness, environment, and machine timing are not declared inputs; where production needs such a value it is injected as an explicit dependency that defaults to the real source (the clock seam above; the capture and keystroke-source seams for OCR and input). A value that leaks into a golden makes the suite non-deterministic by construction and only accidentally passing.

## Mutation testing

Coverage proves a line ran; it cannot prove a test would notice if that line were wrong. Mutation testing closes that gap: [cargo-mutants](https://mutants.rs) makes small changes to the code (a `<` becomes `<=`, a `+` a `-`, a constant shifts) and re-runs the tests against each one. A mutant the tests catch is *killed*; one that slips through *survives* and marks a weak spot. The **mutation score** (the share of mutants killed) is the suite's effectiveness metric and the headline quality signal for the native logic core.

The campaign targets the backend members; the Tauri shell stays out (its logic is OS-window plumbing behind `cfg(windows)`, and building it needs the Tauri system toolchain). Because a campaign re-runs the tests once per mutant, it is heavy and runs nightly rather than per-change (`.github/workflows/nightly.yml`), on a Linux runner. Run one locally on a POSIX environment with:

```sh
cd app/src-tauri
cargo mutants --package eo-wire --package eo-services --in-place
```

The campaign runs `--in-place` because the member tests read committed fixtures from the repository outside the cargo workspace (the relocated corpus under `fixtures/`), which cargo-mutants' default copied build tree would not contain.

The acceptance bar is a **per-file floor map** enforced by the in-tree guard:

```sh
cargo run -p xtask -- mutation-floors --outcomes mutants.out/outcomes.json
```

(The `--outcomes` flag repeats to merge the outcome files of a sharded campaign; per-file counts are summed before scoring.) The floor map is the explicit register of files under mutation coverage. A file with an adopted floor must hold its score, and floors only ever ratchet up. A file without a floor is unadopted: it is still measured and counted in the aggregate badge, and reported as awaiting coverage, but it is non-blocking, so surviving mutants on a newly added file do not fail the gate. Bringing a file into coverage (killing its survivors, recording its floor, excluding any provable equivalents) is done in periodic, deliberate coverage passes rather than in every feature change, so day-to-day work stays off the slow campaign while the aggregate score stays honest. A mutant counts as caught when a test failed on it or the mutated build timed out; unviable mutants (the mutation does not compile) leave the denominator. The aggregate score is published as the README's mutation badge. Triaging a survivor is a two-way choice: strengthen a test until it is killed, or, if the mutation is provably equivalent, leave it with a recorded reason. Never lower a floor to make a regressed run pass.

## Goldens regeneration

The equivalence goldens (the corpus fingerprints, DB-state snapshots, and HTTP-response goldens under `fixtures/corpus/`; the contract snapshots under `contracts/`; the wire fixtures under `eo-wire/tests/fixtures/`) assert by default. A deliberate behaviour change is re-ratified by regenerating the affected goldens and reviewing the resulting diff, then recording that the diff is a genuine intended change rather than a regression.

What the goldens pin going forward is this codebase's own ratified contract, not fidelity to the retired reference implementation: they move only through ratification, and a golden whose bytes encoded an artefact of that reference (a representation detail, a transport-envelope shape, an error-message text) may be changed as a ratifiable behaviour decision. See [ADR-0017](docs/src/adr/0017-behavioural-contract-ownership.md). The equivalence evidence banked at the crossing is untouched by this: it records what was proven equal at the port, and the byte-for-byte re-assertions above still hold it.

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

Commit that report to `app/src-tauri/ratifications/<slug>.md` alongside the golden change, naming the changed sets in its `goldens:` field. It lives outside any `expected/` directory so the guard never treats the report itself as a golden. Proceed only on `ratification-sound`; a `regression-suspected` or `needs-user-judgement` verdict means fix the code (or settle the product question) rather than pin the diff. A committed report is required rather than a bare commit trailer on purpose: fabricating a plausible adversarial report that cites real diff elements is a high bar and is reviewer-visible, where a trailer is not.

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

Both the marker and the recorded verdict are enforced, not merely a courtesy to reviewers. `cargo run -p xtask -- ratify-check --range <BASE>..<HEAD>` runs on every pull request and push (`Golden ratification guard` in `.github/workflows/ci.yml`). It inspects the diff against the base; a golden file is anything matching the committed-golden paths above. If any commit modifies a golden, the guard requires **both**:

- the `test: regenerate goldens` subject prefix on the relevant commit(s); and
- a ratification report added or modified in the same range, carrying a fenced `ORACLE-RATIFICATION` block whose `VERDICT` is `ratification-sound` and whose `goldens:` field names every changed set, committed no earlier than the last golden change in the range.

Three properties make the verdict hard to satisfy by accident. Tying the report to the range stops a sound verdict from a prior regeneration blessing a fresh golden change. The ordering requirement stops a verdict that reviewed an earlier state from blessing a golden edited in a later commit. And the per-set `goldens:` check stops a verdict recorded for one set blessing another. A change missing any of these fails and surfaces the golden diff for review; a change that touches no golden file is ignored, so the guard is inert for ordinary work. The guard fails closed on any range it cannot resolve. What it cannot do is prove the review actually happened or that it was rigorous; that residual is closed by the report being reviewer-visible and by the human merge to `main`.

## Frontend tests

The Svelte frontend has its own unit track, run with Vitest from the `app/` directory and gated by the `frontend` CI job alongside the production build, the type-check, and the Biome lint:

```sh
cd app
npm run test            # run the suites once (CI mode)
npm run test:watch      # re-run on change during development
npm run test:coverage   # run with a v8 coverage report
```

Scope is two layers, with end-to-end flows owned by the native-shell suite (below):

- **Module**: the extracted logic layers under `src/lib/`. Each route's behaviour lives in a per-surface feature module (`lib/features/<surface>/`: view models and pure domain logic, e.g. `questsModel.svelte.ts`, `cooldown.ts`), backed by the shared view-model helpers (`lib/view/`: table, form-modal, typeahead, error-state), the window and overlay helpers (`lib/windows/`), the realtime plumbing (`lib/realtime/`), the typed API seam (`lib/api/`), and the runes state modules (`lib/*.svelte.ts`, `lib/stores/`). Suites are colocated as `<module>.test.ts` next to their source; `.svelte.ts` modules compile through the Svelte plugin, so `$state` / `$derived` behave in the tests exactly as in the app.
- **Component**: the high-logic Svelte surfaces, rendered under Testing Library with `happy-dom` and the Tauri/backend seams mocked.

Routes can be mounted too: `vitest.config.ts` aliases SvelteKit's `$app/navigation` and `$app/state` to inert stubs under `app/test-stubs/`. The startup suites (`src/routes/startup.test.ts`, `src/routes/page.test.ts`) use this to mount every main page while the backend is still starting, with only Tauri's `invoke` mocked. They assert that no read reaches the backend before it is ready, that no page shows an error, and that no region shows an empty state as though it were the answer. To walk that path by hand on a machine that starts quickly, set `ENTROPIAORME_STARTUP_DELAY_MS` (debug builds only) to hold startup back, e.g. `ENTROPIAORME_STARTUP_DELAY_MS=5000 just dev`.

Coverage instrumentation (`coverage.include` in `app/vitest.config.ts`) is directory-based over those tested layers plus the standalone pure-logic modules, so a new module landing in a tested layer is instrumented from the moment it exists rather than once someone remembers to list it. `.svelte` components, generated files, test files, and fixtures are excluded: components are exercised through the component suites and the native-shell e2e, not unit-instrumented.

Tests assert the code's actual behaviour; where a module diverges from what a reader might expect, the divergence is asserted and flagged in-file as a candidate defect rather than papered over.

### Frontend lint and format (Biome)

Biome owns linting and formatting for the frontend's TypeScript, JavaScript, and JSON (Svelte components stay under `svelte-check`). The configuration is `app/biome.json`; the generated `src/lib/api/commands.gen.ts` and the lockfile are excluded.

```sh
cd app
npm run lint     # biome check: lint + format verification (the CI gate)
npm run format   # biome format --write: apply formatting
```

The `frontend` CI job runs `npm run lint` on every change, and a pre-commit hook mirrors it locally through the lockfile-pinned Biome (run `npm ci` in `app/` once so the hook can resolve it).

### Generated API client

The typed frontend API client is generated from the Rust DTOs: the command bindings live in `app/src/lib/api/commands.gen.ts`, emitted by `cargo xtask gen-ts` from the `eo-api` command manifest. The Linux backend-members CI job runs `cargo xtask gen-ts --check`, which fails if the committed bindings drift from the manifest. Regenerate them with `just gen-ts` after a change that moves an `eo-api` DTO or the manifest.

### Runes-native frontend

The Svelte frontend is runes-native: `svelte.config.js` forces runes mode for every non-`node_modules` file, so the legacy Svelte-4 reactivity primitives (`$:` reactive statements, `export let` props) are compile errors rather than a style preference. The guard is the production build itself: `npm run build` (run by the `frontend` CI job) fails on any legacy-reactivity reintroduction, which is why the convention cannot silently rot. New component state uses `$state` / `$derived` / `$effect` and `$props`; `onMount` stays for genuine run-once mount work.

Shared state is runes-native too: cross-surface state is authored as `.svelte.ts` modules, the legacy `svelte/store` writables have all been migrated away, and the `no-new-writable` authoring lint (`cargo run -p xtask -- no-new-writable`, run in CI) forbids any `svelte/store` import from returning. Its frozen-legacy allowlist is empty and can only ever shrink, so the guarantee is whole-tree: no importer anywhere.

### Frontend end-to-end tests (native shell)

The end-to-end layer drives the **real desktop shell** (the Tauri WebView2 window), not a browser tab, through [WebdriverIO](https://webdriver.io) and [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/). Driving the real shell is the point: only there does the desktop IPC bridge exist, so the suite can assert the panels render and the IPC surface is live across the boundary a browser-served harness is structurally blind to. The suite lives under `app/e2e/`.

```sh
cd app
npm run test:e2e          # functional panel flows against the native shell
npm run test:visual       # diff every visual spec against its committed baseline
npm run test:visual:update # regenerate the baselines after an intended UI change
```

The e2e build embeds the frontend and serves it at the application's own `tauri://` origin (native IPC), built with an in-process request-fixture stub (the `e2e-stub` feature) so responses are deterministic, and with chart tweens frozen (`E2E_FREEZE_TWEENS`) so the visual baselines are stable. A `tauri.e2e.conf.json` overlay (capture window size plus a broadened CSP for the stub) is e2e-only and never ships. The suite is hermetic on a developer machine as well as in CI: the harness recreates a fresh backend data directory per run (`e2e/.data/`, injected through the spawn environment) so a run can never open a developer's own database, and the e2e build scopes its preference store to a separate file (`E2E_ISOLATED_PREFS`) so it never shares onboarding or consent state with a real installation. The visual layer commits per-surface screenshot baselines under `e2e/baselines/` and diffs against them with a small fuzzy tolerance. Baselines are captured in one rendering environment (WebView2 on Windows), so a different renderer will diff: regenerate after an intended change rather than editing the image.

The functional suite covers the panel flows and the live IPC surface, plus a keyboard-only smoke (`e2e/specs/keyboard.e2e.mjs`) that drives the shared modal and menu primitives with real key events: the modal's focus trap, Escape dismissal, and focus return, and the menu's roving menuitem focus, arrow keys, and Escape-to-trigger. The visual suite pins the dashboard and analytics surfaces against their populated fixtures, and the quests and character surfaces in their default (empty-database) states; populated-state visual coverage for those two surfaces needs fixture-served data and is a deliberate follow-up. A missing baseline fails the run rather than silently adopting the first capture (`autoSaveBaseline: false`), so new baselines land deliberately: `npm run test:visual:update` locally, or committing the actuals a CI run uploads.

This layer runs on Windows in CI (the `Frontend e2e + visual (native shell, Windows)` job), the application's platform; it provisions `tauri-driver` and the matching Microsoft Edge WebDriver per run. The CI job is temporarily disabled: the runner image's WebView2 150 update broke WebDriver session creation for every run (the DevTools remote-debugging endpoint does not come up under the runner's elevated execution context; MicrosoftEdge/WebView2Feedback#5640), while the identical suite passes on a real Windows machine, so the suite runs locally until the upstream regression is fixed (the job comment in `ci.yml` carries the re-enable path).

## Continuous integration

Every pull request and every push to the two development branches executes the workflow in `.github/workflows/ci.yml`. `next` is the integration branch work lands on directly; `main` is the stable branch releases are cut from, reached by a promotion pull request from `next` (merged as a merge commit, so both lines keep the same commits) or by a squash hotfix pull request. On a documentation-only change (every changed file is Markdown) the compiling jobs are skipped, as described under "Documentation-only changes" below.

- **Change scope and CI gate**: a quick detection job classifies whether the change touches code or only documentation, and the compiling jobs run only for a code change. A small always-running `CI gate` sentinel is the single required check in their place: it passes when the change is documentation-only (those jobs were legitimately skipped) or when every gated job succeeded, and fails closed otherwise, so a skip can never let an untested change merge. Branch protection requires only this one context, so the required-check list never drifts as individual jobs are added or renamed.
- **Golden ratification** (every pull request and push): the `ratify-check` guard, failing when a commit moves a golden without both the marker and a recorded `ratification-sound` verdict for the changed sets (see "Ratification guard" above).
- **Authoring lint** (every pull request and push): the `authoring-lint` guard flags em dashes and US spellings on the lines a change adds, and references to absent files, iteration tokens, and tool-attribution lines in the added prose and the commit messages; a `version-stamps` step asserts the three application version stamps stay in lock-step (see "Authoring lint" below).
- **Frontend**: the generated-client freshness check, the production build, the type-check, the Biome lint, and the Vitest suites.
- **Frontend e2e + visual** (Windows): the native-shell IPC and visual-regression suites (see above).
- **Rust workspace policy** (`fmt` + `audit` + `deny`): formatting, RustSec advisory audit, and supply-chain policy (licence allowlist, bans, registry sources). Source- and lockfile-level only, so it runs unconditionally on a cheap Linux runner (see "Rust workspace checks" below).
- **Rust workspace** (`clippy` + `build` + `test` + doctests, Windows): lints, compiles, and tests every workspace member on the application's real target (most of the shell sits behind `cfg(windows)`).
- **Rust backend members** (`nextest`, Linux): builds and tests the backend members on a runner without the Tauri toolchain, structurally proving they stay free of GUI dependencies, and compile-checks the criterion benches.
- **Rust backend members** (branch coverage): measures per-member branch coverage over the same members with `cargo llvm-cov` on the nightly toolchain (branch instrumentation is nightly-only). The figure is review evidence (whether a member's tests exercise the paths its behaviour rests on) and is published as the README's coverage badge from `main`.

On a push these same jobs run against the state that landed, so a promotion to `main` is verified on the exact merged result as well as on its pull request.

### The two branches

A change lands on `next` by direct push and is run from there before it is promoted. `main` accepts only pull requests, requires the `CI gate` check with the branch up to date, and merges by auto-merge once the check is green; there is no merge queue, because the workflow already runs on every push to both branches and the nightly campaign covers what is too slow for it. A promotion pull request from `next` merges as a merge commit; the integration branch is never squashed, so the two branches keep the same commits and a promotion that falls behind `main` is refreshed by merging `main` into `next`.

### Documentation-only changes

A change that touches only documentation needs none of the compiling jobs: there is no code to test and no frontend to build. The change-scope detection classifies it on both a pull request and a push, so the heavy jobs are skipped in both places. The classification logic runs from the base commit's copy of the classifier, not the head's, so a fork pull request cannot rewrite it to skip the gates; the head's changed-file list is data the fork cannot forge.

Skipping a required check is the hazard: branch protection treats a never-reported required check as pending (deadlocking the merge) and a skipped one as passing (fail-open). The `CI gate` avoids this with an always-running, fail-closed sentinel that stands in for the gated contexts: a documentation-only change goes green in seconds on the sentinel's verdict alone, while a code change still runs and must pass everything. The classification is deliberately conservative: anything other than Markdown counts as code, so the safe direction (run the suite) is the default whenever there is any doubt.

### Nightly

A separate scheduled workflow (`.github/workflows/nightly.yml`) runs the slower checks once a day: the `cargo-mutants` campaign over the backend members (sharded across parallel runners; a single runner cannot finish the full campaign inside the hosted six-hour job ceiling), a verdict job that merges the shard outcomes for the per-file mutation-floor enforcement, and the mutation-score badge publish from `main` (see "Mutation testing" above).

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

- **Biome** (lint + format) over the frontend, mirroring the CI `npm run lint` step through the lockfile-pinned binary (run `npm ci` in `app/` once so the hook can resolve it).
- **`no-bare-setinterval`**: the frontend polling-discipline guard (a `cargo xtask` subcommand), forbidding a bare `setInterval` outside the visibility-gated helper and any reference to the retired tracking event.
- **`in-development`**: the in-development surface guard (a `cargo xtask` subcommand); see "In-development surfaces" below.
- **`id-order`**: the latest-in-time read guard (a `cargo xtask` subcommand); see "Latest-in-time reads" below.
- **authoring lint** (em dash, UK spelling, and the reference and vocabulary rules), diff-scoped against the staged change, and **version-stamp parity**, both `cargo xtask` subcommands (see "Authoring lint" below).
- general hygiene: end-of-file and trailing-whitespace fixers, YAML and TOML validity, merge-conflict markers, and a mixed-line-ending check (line-ending policy itself is set per file type in `.gitattributes`).

The xtask guards compile the in-tree `xtask` crate once (cached thereafter) and run the same logic CI runs. The CI `pre-commit` job exercises the hygiene hooks in pre-commit's own managed environments; Biome is skipped there (no `node_modules`), the dedicated frontend job being its enforcing gate.

## In-development surfaces

Development lands on the main line continuously, so a control or a panel can reach the tree before the capability behind it. Such a surface must not read as finished, and the rule is enforced mechanically rather than remembered.

Three parts, in `app/src/lib/inDevelopment/`:

- **The register** (`registry.ts`) declares each in-development surface once, with the user-facing text explaining what is unavailable and what will make it work. One register rather than a flag per component keeps the set auditable; graduating a surface means deleting its entry.
- **The marker** (`InDevelopmentMark.svelte`) is the single affordance every such surface renders, so the disclosure reaches the person using the app rather than only a reader of the change history.
- **The channel** (`channel.ts`) decides whether these surfaces render at all. It is stamped at build time via `ENTROPIAORME_STABLE_CHANNEL`, set only by the release workflow: a published artefact hides them, while a locally built installer, a source build, and the dev server show them marked. The stamp is a build-time input rather than a build-mode check because an installer built from the latest source is itself a production build, so build mode cannot tell the two apart.

`cargo xtask in-development` (a required CI check and a pre-commit hook) fails on three conditions: a marker referencing an id the register does not declare, a register entry no consumer references (a surface finished without removing its entry), and a release build step that does not stamp the channel. The third is checked because losing that stamp is invisible in CI and visible only to whoever downloads the release.

Only genuinely misleading surfaces belong in the register: a control that does nothing when used, or a figure whose value can diverge from the truth with nothing signalling it. A figure that sits beside unbuilt work but is already correct needs no entry.

## Latest-in-time reads

The database is long-lived and its rows are not guaranteed to arrive in chronological order: a skill scan can be recorded from an older screenshot after a newer one, a backup can be restored beside newer rows, a chat log can be replayed, and an import or backfill writes older data with higher ids. An autoincrement id is therefore an arrival order, never a time order, and the convention is:

- A read that means "the latest in time" orders by the row's timestamp column, with `id` as the tiebreak only (`ORDER BY scanned_at DESC, id DESC LIMIT 1`, or the `MAX(scanned_at)` window with `MAX(id)` inside it). The current level per skill is one such read, shared by every consumer through `latest_skill_levels` in the database module so the definition cannot drift between call sites.
- A read that genuinely orders by id says which allowed case it is, on a one-line comment at the site: `id-order: cursor` (a stream position: the highest id seen so far, or the rows on one side of a recorded position), `id-order: retention` (a window of the most recently appended rows of an append-only journal), `id-order: tiebreak` (rows already narrowed to one timestamp), or `id-order: insertion` (insertion order is itself the meaning: a seeded default, the current entry of a stream only ever appended in process order, a test reading what it just wrote).

`cargo xtask id-order` (a required CI check and a pre-commit hook) scans the tracked Rust sources of the backend crates for the id-ordering shapes (`MAX(id)` and `MIN(id)` over an `id`, `rowid`, or `<name>_id` column, an `ORDER BY` that leads with such a column and takes `DESC` or a `LIMIT`, and a placeholder comparison that bounds one) and fails on any site whose annotation is missing from the twelve lines above it, or names a case outside the four. Comment lines are skipped, so prose about the patterns does not trip it. A timestamp-first order with the id trailing as the tiebreak matches none of the shapes and needs no annotation. Migrations are immutable once applied and are out of scope.

## Authoring lint

Five mechanical authoring rules are enforced as deterministic lint rather than eyeballed in review: no em dashes (U+2014) in authored content, UK spelling in authored prose, and, over the added prose and the commit messages, no references to files the repository does not have, no iteration tokens, and no tool-attribution lines. All run from one `cargo xtask authoring-lint` subcommand, in CI over the pull request's or push's `base..head` range and locally over the staged change.

They are **diff-scoped**: they inspect only the lines and commit messages a change adds, never the whole tree. This is deliberate. The tree carries pre-existing US spellings and em dashes that predate the discipline, and normalising them drive-by is out of scope; checking added lines only binds new content without disturbing the old. The scope differs by rule:

- The **em-dash ban** applies to every added line in a non-exempt file (licence texts, third-party notices, vendored trees and lockfiles, binaries, and generated artefacts are exempt). U+2014 is never code syntax, so an added em dash is always authored content.
- The **UK-spelling check** applies only to added lines in prose contexts (Markdown / plain-text docs, and comment-only lines in code), because tokens like `color` (CSS), `behavior` (DOM API), `center` (a CSS value), and `serialize` (an identifier) are legitimate US-spelled code, not authoring slips. The US-to-UK map is a curated floor, extended as real slips appear.
- The **reference check** applies to the commit messages in the range and to added prose lines. A path-like token (a Markdown file name, a dot-directory path, or, in a commit message, a slash path with a source extension) must name something the repository has: a tracked file or directory at the head of the range, a suffix of one at a path-component boundary, a path relative to the citing file, a path the change itself deletes or renames away, or a path the repository declares ignored. Git trailers (`Co-Authored-By`, `Fixes #N`) and URLs are exempt. Prose in a decision record legitimately cites files a later decision removed, so diff text is checked for the two narrow shapes only.
- The **iteration-token check** flags round-number tokens (an upper-case R and one or two digits; three digits is git's rename score, not a round) in messages and prose, and "round N" in commit messages; a message describes the change, not its attempt number.
- The **attribution-line check** flags a line that opens with an attribution verb ("Generated with", "Authored by") and names a tool by link or product name. Authorship is the commit's author and its `Co-Authored-By` trailers.

A companion check, `cargo xtask version-stamps`, asserts the three application version stamps (`app/package.json`, the `[workspace.package]` version in `app/src-tauri/Cargo.toml`, and `app/src-tauri/entropia-orme/tauri.conf.json`) carry an identical version, so a release bump cannot update some and miss others. It is whole-tree rather than diff-scoped (the invariant holds over the current tree at all times). `cargo xtask bump-version <VERSION>` rewrites all three in lock-step.

## Rust workspace checks

The Rust side is the cargo workspace at `app/src-tauri/`. All of the commands below run from the workspace root:

```sh
cd app/src-tauri
cargo fmt --check                                       # formatting, all members (apply with `cargo fmt`)
cargo clippy --workspace --all-targets -- -D warnings   # lints, warnings promoted to errors
cargo build                                             # compile check, all members (debug profile)
cargo nextest run -p eo-wire -p eo-services -p eo-api   # backend members alone, no Tauri toolchain needed
cargo test --workspace --doc                            # doctests (nextest does not run them)
cargo llvm-cov nextest --branch -p eo-wire -p eo-services  # branch coverage (eo-api is nextest-only here, not measured)
cargo mutants -p eo-wire -p eo-services --in-place         # mutation testing (eo-wire + eo-services only, matching CI)
cargo run -p xtask -- mutation-floors --outcomes mutants.out/outcomes.json  # the floor gate over the campaign
cargo bench -p eo-services                              # criterion micro-benchmarks (hot-path figures)
cargo audit -D warnings                                 # RustSec advisories against Cargo.lock
cargo deny check                                        # licences, bans, sources, advisories
```

The CI jobs split by what they need to compile:

- The **policy job** (`fmt` + `audit` + `deny`) is source- and lockfile-level, so it runs unconditionally on a Linux runner.
- The **workspace job** (`clippy` + `build` + `test` + doctests, whole workspace) runs on Windows: most of the shell sits behind `cfg(windows)`, and linting only the Linux configuration would gate the wrong code. The build step is a debug-profile compile check; the release bundle (`tauri build`) stays a release-time step.
- The **members job** runs `cargo nextest` on the backend members (plus a compile check of the criterion benches) on a plain Linux runner with no Tauri system stack installed. That environment is load-bearing: the backend members must stay buildable and testable without the Tauri toolchain, so a GUI dependency creeping into backend code fails this job structurally rather than landing silently.
- The **Linux bundle job** builds the full Tauri shell on Linux and bundles the `.deb`, the platform counterpart of the Windows workspace/e2e build: a Linux-only regression (a platform seam that stops compiling, a bundler or resource misconfiguration) fails here rather than surfacing only at release.
- The **coverage job** runs `cargo llvm-cov nextest --branch` over the same members (branch coverage needs the nightly toolchain) and publishes the figure as the README's coverage badge from `main`.

Benchmarks run on demand rather than in CI: shared runners produce noisy timings, so CI only compile-checks the benches and real figures are taken locally when a hot path matters.

Two policy files make the audit and licence gates deliberate rather than advisory:

- `.cargo/audit.toml`: the RustSec ignore list, every entry a transitive crate inside the Tauri toolchain with a per-advisory rationale comment. `-D warnings` makes the list load-bearing: an advisory not explicitly ignored there fails CI.
- `deny.toml`: the supply-chain policy: an explicit licence allowlist (a new dependency carrying any other licence fails until the list is deliberately edited), crates-io-only sources, wildcard-version denial, and the advisory ignores.

Review both files together on every Tauri bump; the Tauri version itself is pinned to a named minor in the workspace manifest. The shell tests live in `entropia-orme/src/lib.rs` under `#[cfg(test)]` and cover its pure logic; each backend member carries its own `#[cfg(test)]` suite alongside the equivalence tests above.
