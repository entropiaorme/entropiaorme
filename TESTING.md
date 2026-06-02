# Testing

EntropiaOrme ships with an integrated Python test suite under `backend/tests/`. The suite exercises the tracker pipeline, chat.log parsing, cost / character / codex math, skill tracking, and quest automation end-to-end.

## Running the suite

The test tooling is installed from `backend/requirements-dev.txt`. From the repo root:

```bash
.venv/Scripts/python.exe -m pytest -q
```

The Linux / macOS invocation is `.venv/bin/python -m pytest -q`.

Expected: the suite passes deterministically, with no flaky tests, no network access, and no on-disk state outside `tmp_path` fixtures. Test order is randomised (via `pytest-randomly`) to surface any accidental coupling between tests. For the exact current test count, collect without running: `.venv/Scripts/python.exe -m pytest --collect-only -q`.

### Runtime tiers

Tests are tagged by runtime tier so the right subset runs in the right place:

| Tier | Covers | Command |
| ---- | ------ | ------- |
| `fast` | Pure-logic, in-memory, sub-second | `pytest -m fast` |
| `standard` | Database / filesystem / in-process state | `pytest -m standard` |
| `full` | The slowest suites: the schemathesis contract suites and OCR equivalence (device / OCR / slow) | `pytest -m full` |

Each module's tier is set in `backend/tests/conftest.py`; a module with no entry defaults to `standard`. The tier is selected positively (`pytest -m "fast or standard"`), not by negative exclusion, so a test that lands without a marker stays out of the per-PR run until it is deliberately classified rather than silently joining the gate.

### Parallelism

The suite parallelises with [pytest-xdist](https://pytest-xdist.readthedocs.io) under the `loadfile` scheduler, both locally and (per-PR) in CI:

```bash
.venv/Scripts/python.exe -m pytest -m "fast or standard" -n auto --dist=loadfile
```

`loadfile` sends each whole test file to a single worker. That keeps the module-scoped lifespan-boot fixtures (the contract, ETag, and HTTP-fingerprint scenario suites each boot the FastAPI lifespan against a temporary data directory) booting once per file rather than once per worker, and never splits a file's tests across workers. A parallel run reproduces the serial pass count exactly; no test is dropped by worker collection.

**Continuous integration runs the two per-PR backend legs under xdist** (`--dist=loadfile -n auto`), the same form shown above. This was once the opposite: an earlier measurement on the hosted Windows runner had the supported-floor leg at roughly three minutes serially against about fourteen and a half minutes under `-n auto`, because at that suite size each worker re-paid the heavy import cost (onnxruntime, OpenCV, FastAPI) and the per-process spawn dwarfed the parallelism gain. The suite has since grown several-fold; re-benchmarked at the current size that import overhead is a small fraction of the run, and the result reverses: `--dist=loadfile -n auto` roughly halves each per-PR leg, so CI now parallelises them. Two legs stay serial by design: the `full` tier runs single-worker (the stateful schemathesis pass carries per-process generation state, and its appended coverage must stay reproducible), and the nightly run executes the complete suite serially in randomised order, which is where cross-file ordering coupling is surfaced thoroughly. The markers and the scheduler wiring below stay in place so the `loadgroup` opt-out keeps working.

The suite parallelises safely because every test isolates its own external state: xdist workers are separate processes, so no in-memory singleton is shared between them; no test binds a real OS socket (the HTTP suites drive the app in-process through Starlette's `TestClient`, and `BACKEND_PORT` only builds the origin-checked `base_url` string); per-test state runs through `tmp_path` / `mkdtemp` and fresh in-memory SQLite; and the one direct `os.environ` mutation (the data-directory override in the e2e HTTP fixture) is save-and-restore guarded and per-process. `pytest-randomly` already shuffles order within each worker, so intra-process ordering coupling would surface regardless of how files are distributed.

A test that genuinely cannot run concurrently with tests in *other* files (one that binds a fixed OS port, or mutates a process-wide global another file reads) marks itself `@pytest.mark.no_xdist`. Under `--dist=loadgroup` the collection hook keeps unmarked tests grouped by file and collapses every `no_xdist` test onto one shared worker, so activating the opt-out is a one-flag change on the affected leg rather than a code change. No test needs it today.

The e2e scenario and recorder tests drive the real `ChatlogWatcher` tail loop, then wait for it to catch up before asserting. That wait (`wait_for_drain` in `backend/testing/replay.py`) is a condition wait, not a fixed-duration sleep: it blocks until the watcher reports it has read every line the scenario appended and flushed its final tick, and raises `TimeoutError` if the watcher never reaches that state. Two seams make it correct under contention. First, `ChatlogWatcher.start()` blocks until the tail loop has opened the file and seeked to its end, so a scenario that writes immediately after `start()` cannot have its lines missed by a not-yet-seeked watcher; without that barrier, heavy parallel load could delay the seek past the writes and the watcher would read nothing. Second, the watcher signals an idle condition on each end-of-file cycle, which the drain wait blocks on rather than polling a clock. Together they make the drain converge the instant the watcher is genuinely idle regardless of scheduling latency, which is why the suite holds under the aggressive `loadgroup` scheduler as well as `loadfile`. This honours the "flakes are bugs, no reruns" stance: the timing race is removed at the source rather than absorbed by a sleep or a rerun plugin.

### Linting and formatting

```bash
.venv/Scripts/python.exe -m ruff check .
.venv/Scripts/python.exe -m ruff format --check .
```

`ruff format` owns code style and line length; `ruff check` enforces the lint rules configured in `pyproject.toml`.

### Coverage

Branch coverage is measured with `pytest-cov` (configuration under `[tool.coverage]` in `pyproject.toml`):

```bash
.venv/Scripts/python.exe -m pytest -m "not full" --cov=backend --cov-branch --cov-report=term-missing
```

The run must hold a total branch-coverage floor (the `fail_under` value in `pyproject.toml`; the live measured figure is the coverage badge at the top of the README). The floor sits a few points below the measured figure and ratchets upward as coverage improves; it is never lowered to make a red gate pass. Device, input-listener, screen-capture, and one-off script modules are run but excluded from measurement: they cannot be unit-covered without real hardware or a display, so the floor reflects testable logic rather than platform glue. The exclusion list lives under `[tool.coverage.run]`.

The API contract suite exercises the HTTP router surface, but as the `full` tier it runs on the merge queue, post-merge, and nightly rather than the per-PR coverage leg (see "Runtime tiers" and "Continuous integration"). The per-PR coverage leg measures the `fast or standard` set and clears the floor on its own because the seeded API-surface walk and mutation tests already cover the router branches the contract suite touches. The post-merge leg appends the full tier's coverage (`pytest -m full`: the contract suites and OCR equivalence, with `--cov-append`) so the published badge reflects the full surface; order is pinned on both coverage passes, so the measured figure does not vary run to run. The local command above (`-m "not full"`) reproduces the per-PR coverage; add a `-m full --cov-append` pass to fold in the full tier.

On a pull request, `diff-cover` additionally holds new and changed lines to a higher bar (85%), so coverage rises with every change even while the older surface is brought up over time. A new read endpoint therefore needs a `fast` or `standard` tier test exercising it (for example an entry in the e2e API-surface walk): the contract suite that fuzzes the GET surface is the `full` tier and does not run on the per-PR coverage leg (the merge queue runs it without measuring coverage), so leaning on it alone for a new route would surface as a diff-coverage miss on the PR.

### Typing

The backend is type-checked with [mypy](https://mypy.readthedocs.io) (configuration under `[tool.mypy]` in `pyproject.toml`):

```bash
.venv/Scripts/python.exe -m mypy backend
```

The gate is clean at a defined, honest level and tightens over time rather than all at once. Three things set that level:

- **A lenient, fully-clean base.** Every module is checked with `check_untyped_defs` on, so the bodies of as-yet-unannotated functions are still verified for real type errors (bad indexing, mismatched operands, wrong return types). The base is held at zero errors.
- **A strict allow-list.** A small set of pure-logic modules is additionally held to `disallow_untyped_defs` (every function fully annotated): `cost_engine`, `scan_drift`, `loot_filter`, `codex_categories`, and `tt_value_curve`. This list only grows.
- **Scoped third-party ignores.** The C-extension dependencies that ship no type information (`cv2`, `mss`, `pynput`, `onnxruntime`, `openocr`, `rapidfuzz`) have missing-import errors suppressed per module, so a genuinely missing first-party import still surfaces. The suppression is never global.

To promote a module into the strict set: annotate it fully, confirm `mypy backend` stays green, then add it to the `disallow_untyped_defs` override in `pyproject.toml`. The gate only ever ratchets towards stricter checking; a landed strictness level is never relaxed to make a red check pass.

### Dependency advisories

Dependencies are scanned for known advisories with [pip-audit](https://pypi.org/project/pip-audit/) against the PyPA advisory database:

```bash
.venv/Scripts/python.exe -m pip_audit -r backend/requirements.txt -r backend/requirements-dev.txt --strict
```

The audit reads the pinned requirements rather than the ambient environment, so only the project's own runtime and development dependencies are in scope (the installer and other incidental tooling in the virtual environment are not). `--strict` fails the run if any dependency cannot be fully audited. The gate is clean today and acts as a forward regression guard: the advisory database is updated continuously, so a run that is green now can later turn red with no change on our side, which is the gate working as intended rather than a flake.

If an advisory has no available fix and does not affect how the application uses the dependency, it can be suppressed with `pip-audit --ignore-vuln <ID>`, accompanied by an inline note recording the advisory identifier and the reason, and tracked until a fix is published. A suppression is always scoped to a single advisory; the gate is never disabled wholesale.

### Property-based tests

The pure-logic core is also checked with [Hypothesis](https://hypothesis.readthedocs.io) (`backend/tests/test_*_properties.py`, the net-new `test_scan_drift.py` / `test_loot_filter.py` units, and the `test_tracker_stateful.py` state machine). Rather than asserting fixed examples, these assert invariants (conservation, monotonicity, round-trips, bounds) over generated inputs; a `RuleBasedStateMachine` additionally drives the hunt tracker through the event bus, asserting its accumulator and kill-model invariants after every step.

Three settings profiles are registered in `conftest.py` and selected with the `HYPOTHESIS_PROFILE` environment variable (default `dev`):

| Profile | Examples | Used by |
| ------- | -------- | ------- |
| `dev` | 100 | local runs (default) |
| `ci` | 300 | the CI backend job |
| `nightly` | 1000 | reserved for the scheduled workflow |
| `mutation` | 200 | the mutation campaign (derandomised, so the score is reproducible) |

Deadlines are disabled on every profile: example timing varies on shared runners, and a deterministic property must never fail merely because one example ran slowly. A failing property prints a minimal, shrunk counterexample (with a reproduction blob under the `ci` profile) so it can be replayed exactly.

```bash
HYPOTHESIS_PROFILE=ci .venv/Scripts/python.exe -m pytest -m "not full"
```

### API contract tests

The API's read surface is checked against its own OpenAPI schema with [schemathesis](https://schemathesis.readthedocs.io) (`backend/tests/test_api_contract.py`). It loads the running app's schema in-process and, for every `GET` operation, generates requests and asserts two properties:

- **No server errors** (`not_a_server_error`): no generated input drives an endpoint into an unhandled `5xx`.
- **Schema conformance** (`response_schema_conformance`): successful (`2xx`) responses match the Pydantic `response_model` the endpoint declares.

The prioritised endpoints carry response models so this has teeth: the polymorphic `tracking/status` and `tracking/live` shapes (`unavailable` / `idle` / `active`), the analytics overview, the character prospect forecast, and the notable-event feed, plus their demo equivalents. Those routes serialise with `response_model_exclude_unset=True`, so the lean shapes keep only the keys the handler actually set rather than gaining a wall of nulls; the models allow extra keys, so adding one can only ever describe a response, never truncate it.

```bash
.venv/Scripts/python.exe -m pytest -m contract
```

The suite is the `full` tier, so on CI it runs on the merge queue, post-merge, and nightly rather than on every pull request (see "Continuous integration"); the command above runs it locally on demand.

A few deliberate scoping choices keep the run honest and reproducible:

- **Conformance is asserted on `2xx` responses.** Request-validation failures return FastAPI's standard `HTTPValidationError` (a `detail` array), while a few handlers raise a `422` with a string `detail` for business-rule violations: a pre-existing dual shape under one status code. Strictly conforming those error bodies is out of scope for describing current behaviour, so they are held to the no-server-error bar instead.
- **The demo surface is seeded into a temporary directory for the run**, so the bundled demo routes are exercised identically here and in CI without depending on a checked-in database.
- **Generated integer ids are clamped to the storable 64-bit range.** A value beyond it cannot match a stored row and trips a driver-level overflow rather than a clean `404`; that robustness gap on out-of-range ids is tracked separately and is outside a change that only describes current behaviour.

The suite covers the read surface; mutating endpoints need request fixtures and stateful setup and stay on the existing integration tests.

### Mutation testing

Coverage proves a line ran; it cannot prove a test would notice if that line were wrong. [Mutation testing](https://mutmut.readthedocs.io) closes that gap: [mutmut](https://github.com/boxed/mutmut) makes small changes to the code (a `<` becomes `<=`, a `+` a `-`, a constant shifts) and re-runs the tests against each one. A mutant the tests catch is *killed*; one that slips through *survives* and marks a weak spot. The **mutation score** (the share of mutants killed) is the suite's effectiveness metric, and the headline quality signal for the pure-logic core.

The campaign targets that pure-logic core; the exact target list is `[tool.mutmut] paths_to_mutate` in `pyproject.toml`, and the service coverage matrix (`backend/testing/COVERAGE.md`) marks every module the campaign covers. Device, IO, and router glue carry little mutation value, so they stay out. Because a campaign is slow (it re-runs tests once per mutant), it runs nightly and on demand rather than on every pull request (`.github/workflows/nightly.yml`); the engine has no native Windows support, so it runs on a Linux runner, which the pure-logic targets are indifferent to.

The campaign runs under the derandomised profile so the score is reproducible. The live aggregate score is the mutation badge at the top of the README; the enforced floors (a ratcheting aggregate floor plus a per-module floor map, each held a few points below the measured score) live in `.github/workflows/nightly.yml` and only ever rise. Branchy heuristic modules admit more behaviour-preserving mutants, so they carry most of the survivors at any given time. Run a campaign on a POSIX environment with:

```bash
HYPOTHESIS_PROFILE=mutation mutmut run   # run the campaign (derandomised, so the score is reproducible)
mutmut results                           # list surviving mutants
mutmut show <mutant>                     # inspect one survivor's diff
mutmut export-cicd-stats                 # write mutants/mutmut-cicd-stats.json
python -m backend.scripts.mutation_score # reduce that to a score (and, with --badge-out, a badge)
```

**Triaging a survivor** is a two-way choice: either add or strengthen a test until it is killed, or, if the mutation is provably equivalent (it cannot change observable behaviour), mark the line with `# pragma: no mutate` and a one-line reason so it leaves the denominator honestly. The floor only ever rises; never lower it to make a regressed run pass.

The metric has teeth precisely because it is not coverage: a test that executes a function but asserts nothing about the result leaves every behaviour-changing mutant of that function alive, dropping the score even though the line stayed "covered".

## Continuous integration

Every pull request, push to `main`, and merge-queue run executes these jobs (`.github/workflows/ci.yml`); on a documentation-only change the backend matrix and the frontend build are skipped, as described under "Documentation-only changes" below:

- **Change scope and CI gate**: a quick detection job classifies whether the pull request touches code or only documentation (every changed file is Markdown), and the backend matrix and frontend build run only for a code change. A small always-running `CI gate` sentinel is the required check in their place: it passes when the change is documentation-only (those jobs were legitimately skipped) or when both gated jobs succeeded, and fails closed otherwise, so a skip can never let an untested change merge.
- **Backend**, on Windows across Python 3.11 and 3.14: the `fast or standard` tiers, run under xdist (`--dist=loadfile -n auto`, see "Parallelism"). The 3.14 leg additionally reports branch coverage and, on pull requests, enforces diff coverage on the changed lines. The `full` tier (the schemathesis contract suites and OCR equivalence) does not run on the per-PR gate; it runs on the merge queue's integrated commit before a change lands (see "The merge queue" below), post-merge on a push to `main` (on the 3.14 leg, its coverage appended so the published badge reflects the full surface), and in the nightly workflow, so the per-PR gate stays fast while nothing reaches `main` un-vetted by the full suite.
- **Lint**: `ruff check` and `ruff format --check`.
- **Typing**: `mypy backend`.
- **Dependency audit**: `pip-audit` against the pinned requirements.
- **Pre-commit hooks**: `pre-commit run --all-files`, validating the hook configuration the local development loop uses (see "Local checks" below).
- **Golden ratification** (pull requests and the merge queue): fails when a commit moves a golden file without both the `test: regenerate goldens` marker and a recorded independent `ratification-sound` verdict for the changed sets, so a regression can be ratified neither unconsciously nor by its own author (see "Goldens regeneration" above).
- **Authoring lint** (pull requests and the merge queue): flags em dashes and US spellings on the lines a change adds (see "Authoring lint" below). Diff-scoped, so it binds new content without forcing a drive-by sweep of the old.
- **Frontend**: the type-check and production build.

On the merge queue these same jobs run against the integrated commit, and the backend `full` tier runs there too, so the queue is the pre-merge gate (see "The merge queue" below).

The backend runs on Windows because that is the application's platform: the screen-capture and input-listener code paths target it directly.

### The merge queue

The `full` tier is kept off the per-pull-request gate so iteration stays fast, but nothing should reach `main` without it. A required GitHub merge queue closes that gap. When a pull request is ready, merging it adds it to the queue rather than landing it directly; the queue integrates the change onto the current `main` and runs the CI workflow against that integrated commit on a `merge_group` event. The backend `full` tier runs there (see the backend job above), so the exact state that will land is vetted by the complete suite, and the queue merges the change automatically once the run is green.

Because the queue tests the integrated result, it keeps `main` green without the per-PR gate ever paying the `full` tier: day to day you iterate on the fast per-PR gate, then merge, and the queue runs the heavy suite once at the point of landing. A change that fails in the queue is dropped from it rather than landing, so a regression that only appears once a change is integrated onto the current `main` is caught before it reaches the branch.

### Documentation-only changes

A change that touches only documentation (every changed file is Markdown) needs neither the backend test matrix, the frontend build, nor the full tier: there is no code to test and no frontend to build. The change-scope detection job classifies it on both a pull request and the merge queue (it reads the queue's integrated range as well as the pull request's), so the heavy jobs are skipped in both places.

Skipping a *required* check is the hazard: branch protection treats a never-reported required check as pending (deadlocking the merge) and a skipped one as passing (fail-open). The `CI gate` avoids this with an always-running sentinel: it stands in for the backend and frontend contexts, always reports, and is fail-closed. A documentation-only change passes on that verdict alone, but a detection that did not run cleanly, or a code change whose jobs did not pass, fails the gate. So a documentation-only change goes green in seconds, while a code change still runs and must pass everything. The classification is deliberately conservative: anything other than Markdown (source, configuration, the workflow files themselves, an asset) counts as code, so the safe direction (run the suite) is the default whenever there is any doubt.

A separate scheduled workflow (`.github/workflows/nightly.yml`) runs the slower checks once a day: it re-runs the dependency audit (so an advisory published after a change has landed is surfaced without waiting for the next pull request), runs the complete backend suite across all tiers under randomised order (the `full` tier, plus a cross-file isolation check the pinned per-PR legs cannot give), and runs the mutation campaign, publishing the mutation score as a badge at the top of the README. (The coverage badge is refreshed separately, by the main-branch run of the CI workflow above.)

## Local checks (pre-commit)

A [pre-commit](https://pre-commit.com/) configuration (`.pre-commit-config.yaml`) mirrors the CI gates into the local development loop, so the same failures surface before a push rather than after. Install the git hook once, after creating the virtual environment:

```bash
pre-commit install
```

The hooks then run on each commit. To run them across the whole tree on demand:

```bash
pre-commit run --all-files
```

The configured hooks are:

- **ruff** (lint with autofix) and **ruff-format**, pinned to the same ruff version as the lint job, reading the same configuration.
- **mypy** over `backend`, run against the project virtual environment so it resolves dependencies exactly as the typing job does.
- the **fast test tier** (`pytest -m fast`), the quick pure-logic subset.
- **authoring lint** (em dash + UK spelling), diff-scoped against the staged change (see "Authoring lint" below).
- **version-stamp parity**, asserting the three app version stamps stay in lock-step (see "Authoring lint" below).
- general hygiene: end-of-file and trailing-whitespace fixers, YAML and TOML validity, merge-conflict markers, and a mixed-line-ending check (line-ending policy itself is set per file type in `.gitattributes`).

The authoring-lint and version-stamp hooks are pure stdlib plus git, so unlike mypy and the fast test tier (which need the installed dependency tree and are skipped in the CI pre-commit job) they run in CI as well as locally.

The dependency audit is slower, so it is reserved for the manual stage rather than every commit; run it explicitly when checking dependencies:

```bash
pre-commit run --hook-stage manual pip-audit
```

The mypy and test hooks run against the active virtual environment; the lint and hygiene hooks run in environments pre-commit manages itself, so the CI `pre-commit` job exercises those without reinstalling the dependency tree the typing and backend jobs already cover.

## Authoring lint

Two mechanical authoring rules are enforced as deterministic lint rather than eyeballed in review: no em dashes (U+2014) in authored content, and UK spelling in authored prose. Both live in `backend/scripts/check_authoring_lint.py`.

They are **diff-scoped**: they inspect only the lines a change *adds*, never the whole tree. This is deliberate. The tree carries pre-existing US spellings and em dashes that predate the discipline, and normalising them drive-by is out of scope; a whole-tree gate could not run green without that churn, and a lint floor only ratchets up. Checking added lines only binds new content without disturbing the old.

The scope differs by rule for a reason:

- The **em-dash ban** applies to every added line in a non-exempt file (LICENSE, THIRD-PARTY-NOTICES, vendored trees and lockfiles, binaries, and generated artefacts are exempt). U+2014 is never code syntax, so an added em dash is always authored content.
- The **UK-spelling check** applies only to added lines in prose contexts: Markdown / plain-text docs, and comment-only lines in code. A blanket US-to-UK check over code would be unworkable, because `color` (CSS), `behavior` (DOM API), `center` (CSS value), `license` (the `package.json` field), and `serialize` / `initialize` (identifiers) are legitimate US-spelled code tokens, not authoring slips. In-app copy in string literals stays under the em-dash net but is left to review for UK spelling. The US-to-UK map is a curated floor, extended as real slips appear.

Run it against the staged change (the pre-commit invocation) or an explicit range:

```bash
.venv/Scripts/python.exe -m backend.scripts.check_authoring_lint              # staged vs HEAD
.venv/Scripts/python.exe -m backend.scripts.check_authoring_lint --range origin/main..HEAD
```

The pull-request gate runs it over the PR's `base..head` range. Pass `--warn-only` to print findings without failing.

A companion check, `backend/scripts/check_version_stamps.py`, asserts the three application version stamps (`frontend/package.json`, `frontend/src-tauri/Cargo.toml`, `frontend/src-tauri/tauri.conf.json`) carry an identical version, so a release bump cannot update some and miss others. It is whole-tree rather than diff-scoped (the invariant holds over the current tree at all times), so it runs in the pre-commit job rather than the diff-scoped authoring-lint job. `CURRENT_TOS_VERSION` in `frontend/src/lib/tos.ts` is deliberately excluded: it versions the terms-of-service document, a separate namespace that moves independently of the application release.

## Goldens regeneration

Several e2e suites assert against committed golden files: the per-scenario event-stream fingerprint (`fingerprint.jsonl`) and DB-state snapshot (`db_state.json`), the per-endpoint HTTP-response goldens, the OpenAPI spec snapshot (`backend/tests/expected/openapi.snapshot.json`), and the `pytest-regressions` goldens (OCR equivalence, snapshot / event-stream consistency). The default mode asserts; a deliberate behaviour change is re-ratified by regenerating the affected goldens and reviewing the resulting diff.

Regenerate with the harness flag, which surfaces the diff before writing so a ratification is deliberate rather than mechanical:

```bash
.venv/Scripts/python.exe -m pytest --update-fingerprints <selector>
```

Narrow `<selector>` to the affected scenarios or modules. The `pytest-regressions` goldens regenerate with that library's `--force-regen` instead; each suite documents its own flow (`backend/testing/CONFORMANCE.md`, `backend/testing/CONSISTENCY.md`, `backend/testing/AUTHORING.md`).

### Independent ratification (the review step)

Regenerating a golden makes you, at once, the author of the change, the regenerator of the expected output, and its would-be approver. That is a structural conflict of interest: the honest tell that you owe a second opinion is that you are reaching to change what *correct* means rather than to make the code meet it. So a golden move is not yours to approve alone.

Before committing any expected-output change (a regenerated golden, or the first pin of a newly emitted one), have an independent reviewer (someone who did not author the change) examine it against the code change and your rationale, and judge the one question the marker cannot: is this delta a genuine intended behaviour change, or a regression being laundered into the goldens as the new "correct"? The first-pin case is the most dangerous, because no prior golden means no assertion fails, so an over-emission can be pinned as "expected" and pass silently; the review scrutinises the absolute output, not just a diff.

The review records a fenced verdict block you commit verbatim (the block must be fenced; the guard reads only the fenced block, never any `VERDICT:` line that happens to appear in the report's surrounding prose):

```text
ORACLE-RATIFICATION
range: <commit-range>
goldens: <comma-separated sets reviewed>
VERDICT: ratification-sound | regression-suspected | needs-user-judgement
```

Commit that report to `backend/testing/ratifications/<slug>.md` alongside the golden change, naming the changed sets in its `goldens:` field. It lives deliberately *outside* any `expected/` directory, so the guard never treats the report as a golden and demands a ratification of the ratification. Proceed only on `ratification-sound`; a `regression-suspected` or `needs-user-judgement` verdict means fix the code (or settle the product question) rather than pin the diff.

A committed report is required rather than a bare commit trailer on purpose: forging a one-line trailer is free, whereas fabricating a plausible adversarial report that cites real diff elements is a high bar and is reviewer-visible (CodeRabbit, a human) where a trailer is not.

### Commit-message convention

A regeneration commit takes the subject prefix `test: regenerate goldens` and lists the regenerated sets in the body, so a reviewer sees at a glance which goldens moved and why:

```
test: regenerate goldens for the basic-hunt loot rounding change

Regenerated alongside the loot-value rounding fix:
- basic_hunt_10_events: fingerprint.jsonl, db_state.json
- basic_hunt_10_events: http_responses/ (tracking + quests endpoints)

The OpenAPI snapshot and the consistency goldens are unchanged.
```

The regeneration may sit in its own commit alongside the behaviour-change commit, or be the sole content of a goldens-only maintenance pull request; the convention is the same either way.

### Ratification guard

Neither the marker nor the independent verdict is merely a courtesy to reviewers: both are enforced. Regenerating a golden re-ratifies whatever the pipeline currently produces, so an unmarked, unscrutinised golden change can silently lock in a regression (the expected output simply moves to match the regressed code, and every assertion passes again). To make that ratification deliberate *and* independently signed off, `backend/scripts/check_golden_ratification.py` runs as a lightweight pull-request job (`golden-ratification` in `.github/workflows/ci.yml`).

The guard inspects the pull request's diff against its base. A golden file is anything under a `backend/tests` `expected/` directory (the per-scenario `fingerprint.jsonl` and `db_state.json`, the HTTP-response goldens), the OpenAPI snapshot at `backend/tests/expected/openapi.snapshot.json`, the `pytest-regressions` consistency goldens beside the `test_consistency_*` modules, or the generated `backend/testing/COVERAGE.md` matrix. If any commit modifies a golden, the job requires **both**:

- the `test: regenerate goldens` subject prefix on the relevant commit(s); and
- a ratification artefact (`backend/testing/ratifications/<slug>.md`) added or modified **in the same PR range**, carrying a fenced `ORACLE-RATIFICATION` block whose `VERDICT` is `ratification-sound` and whose `goldens:` field names every changed set, and committed **no earlier than the last golden change in the range**.

Three properties make the verdict hard to satisfy by accident. Tying the artefact to the range stops a sound verdict from a *prior* regeneration blessing a fresh golden change. The ordering requirement stops a verdict from an *earlier commit in the same range* blessing a golden edit made in a later commit (the verdict reviewed the earlier state, not the final one), so a same-range golden change after the report forces the report to be re-reviewed and re-committed. And the per-set `goldens:` check stops a verdict recorded for one set blessing another. A change missing any of these fails and surfaces the golden diff for review; a change that touches no golden file is ignored, so the guard is inert for ordinary work.

Run it locally before pushing a goldens change, against the staged / working-tree diff or an explicit range:

```bash
.venv/Scripts/python.exe -m backend.scripts.check_golden_ratification              # staged vs HEAD
.venv/Scripts/python.exe -m backend.scripts.check_golden_ratification --range origin/main..HEAD
```

The staged / working-tree invocation is **marker-only and advisory**: before the commit exists there is no committed verdict for it to inspect, so it checks the marker and surfaces the diff, leaving the verdict requirement to the range-mode pull-request gate. Pass `--warn-only` to surface the diff without failing.

What the guard *cannot* do is prove the independent review actually happened: it can only confirm a sound verdict artefact is present, parse it, and tie it to the range. That residual is closed not by CI but by the committed report being reviewer-visible (CodeRabbit, a human) and by the human merge to `main`. The guard is the mechanical backstop; the report and the review are the substance.

## Test layout

| File | Coverage |
| ---- | -------- |
| `test_codex_formulas.py` | Codex rank multipliers, category cycling, cat4 bonus shape, rank cost and reward PED, inverse TT lookup. |
| `test_chatlog_parser.py` | Every EventType has at least one parametrised case. Critical-hit vs damage-dealt, HoF vs Global, quantity extraction, verbose vs direct skill formats. |
| `test_cost_engine.py` | Per-shot cost breakdown: weapon, amp, scope, absorber, damage enhancers, markups. Heal cost, heal range, damage range, weapon total damage. |
| `test_character_calc.py` | TT value curve anchors, profession level math, skill rank lookup, codex category resolution, HP formula, HP optimiser, profession path optimiser (target and budget modes). |
| `test_tool_inference.py` | Damage-based weapon attribution: band containment, narrowest-band selection with name tie-break, critical-hit band widening and the big-weapon preference rule, profile bookkeeping. |
| `test_codex_service.py` | Species listing with dedup and progress cross-ref, rank breakdowns, claim recording, calibrate, skill-option ranking by profession or HP target, meta claims. |
| `test_skill_tracker.py` | Session-scoped recording, TT-value computation, codex claim suppression (one-shot, with-observation, expiry handling). |
| `test_scan_completion.py` | Scan-time anchor archival: prior `scan` rows move to the archive table, non-`scan` rows (codex / chatlog) stay live. |
| `test_chatlog_watcher.py` | Tick buffering, loot grouping by timestamp, quest-reward suppression (PED / zero-PED / skill), enhancer-break shrapnel matching. |
| `test_quests.py` | Quest CRUD, cooldown, completion routing (ledger vs `quest_claims`), playlist grouping and reorder, mission-name fuzzy matching, session-link suggestions, curated analytics. |
| `test_tracker_integration.py` | Full pipeline via the event bus: kills model, dangling cost, tool-stats merge, manual mob and tag modes, global / HoF correlation, crash recovery on orphaned sessions. |

## Adding tests

The suite uses pytest. No fixtures live outside the test files themselves; each test sets up an in-memory `sqlite3` database via `AppDatabase(tmp_path / "test.db")` or `sqlite3.connect(":memory:")`. The event bus is constructed per test.

Conventions:

- One module per system under test, located at `backend/tests/test_<module>.py`.
- Add the new module to the tier map in `backend/tests/conftest.py` (it defaults to `standard` otherwise).
- Helpers are file-local (`_make_*`, `_seed_*`) rather than shared fixtures, to keep each file self-contained.
- Use `tmp_path` for any test that touches `AppDatabase`; do not use a persistent location.
- Avoid wall-clock dependencies; use explicit timestamps passed to `bus.publish` or seeded directly into rows.

## Posture

This suite is the core integrated coverage: the pipelines, formulas, and database contracts the rest of the app composes against, gated on every change by continuous integration. Router-level tests, OCR-pipeline tests, property-based tests, and broader unit coverage are being expanded on top of this foundation.
