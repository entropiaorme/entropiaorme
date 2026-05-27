# Testing

EntropiaOrme ships with an integrated Python test suite under `backend/tests/`. The suite exercises the tracker pipeline, chat.log parsing, cost / character / codex math, skill tracking, and quest automation end-to-end.

## Running the suite

The test tooling is installed from `backend/requirements-dev.txt`. From the repo root:

```bash
.venv/Scripts/python.exe -m pytest -q
```

The Linux / macOS invocation is `.venv/bin/python -m pytest -q`.

Expected: 381 tests pass in roughly 3 seconds. The suite is deterministic; no flaky tests, no network access, no on-disk state outside `tmp_path` fixtures. Test order is randomised (via `pytest-randomly`) to surface any accidental coupling between tests.

### Runtime tiers

Tests are tagged by runtime tier so the right subset runs in the right place:

| Tier | Covers | Command |
| ---- | ------ | ------- |
| `fast` | Pure-logic, in-memory, sub-second (162 tests) | `pytest -m fast` |
| `standard` | Database / filesystem / in-process state (219 tests) | `pytest -m standard` |
| `full` | Device / screen-capture / slow checks (none yet) | `pytest -m full` |

Each module's tier is set in `backend/tests/conftest.py`; a module with no entry defaults to `standard`.

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

The run must hold a total branch-coverage floor (currently 44%). The floor sits a few points below the measured figure and ratchets upward as coverage improves; it is never lowered to make a red gate pass. Device, input-listener, screen-capture, and one-off script modules are run but excluded from measurement: they cannot be unit-covered without real hardware or a display, so the floor reflects testable logic rather than platform glue. The exclusion list lives under `[tool.coverage.run]`.

The API contract suite contributes to coverage as well, since it exercises the HTTP router surface. It runs in deterministic mode against a pinned generator, so its contribution is reproducible; in CI it runs as a separate step whose coverage is accumulated into the same report (`coverage` with `--cov-append`), and both coverage passes pin the test order so the measured figure does not vary run to run. The single local command above already covers it (the contract tier is part of `not full`).

On a pull request, `diff-cover` additionally holds new and changed lines to a higher bar (85%), so coverage rises with every change even while the older surface is brought up over time.

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

A few deliberate scoping choices keep the run honest and reproducible:

- **Conformance is asserted on `2xx` responses.** Request-validation failures return FastAPI's standard `HTTPValidationError` (a `detail` array), while a few handlers raise a `422` with a string `detail` for business-rule violations: a pre-existing dual shape under one status code. Strictly conforming those error bodies is out of scope for describing current behaviour, so they are held to the no-server-error bar instead.
- **The demo surface is seeded into a temporary directory for the run**, so the bundled demo routes are exercised identically here and in CI without depending on a checked-in database.
- **Generated integer ids are clamped to the storable 64-bit range.** A value beyond it cannot match a stored row and trips a driver-level overflow rather than a clean `404`; that robustness gap on out-of-range ids is tracked separately and is outside a change that only describes current behaviour.

The suite covers the read surface; mutating endpoints need request fixtures and stateful setup and stay on the existing integration tests.

### Mutation testing

Coverage proves a line ran; it cannot prove a test would notice if that line were wrong. [Mutation testing](https://mutmut.readthedocs.io) closes that gap: [mutmut](https://github.com/boxed/mutmut) makes small changes to the code (a `<` becomes `<=`, a `+` a `-`, a constant shifts) and re-runs the tests against each one. A mutant the tests catch is *killed*; one that slips through *survives* and marks a weak spot. The **mutation score** (the share of mutants killed) is the suite's effectiveness metric, and the headline quality signal for the pure-logic core.

The campaign targets that core (`cost_engine`, `character_calc`, `codex_categories`, `tt_value_curve`, `scan_drift`, `loot_filter`, `tool_inference`); device, IO, and router glue carry little mutation value. The configuration lives in `[tool.mutmut]` in `pyproject.toml`. Because a campaign is slow (it re-runs tests once per mutant), it runs nightly and on demand rather than on every pull request (`.github/workflows/nightly.yml`); the engine has no native Windows support, so it runs on a Linux runner, which the pure-logic targets are indifferent to.

The current baseline is **75.1%** across the seven modules, measured under the derandomised profile so it is reproducible (floor **72%**, with headroom to ratchet upward). `codex_categories` is strongest at 95%; `character_calc` carries most of the survivors (its profession-path heuristics admit many behaviour-preserving mutants). Run a campaign on a POSIX environment with:

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

Every pull request and push to `main` runs six jobs (`.github/workflows/ci.yml`):

- **Backend**, on Windows across Python 3.11 and 3.14: the suite excluding the `full` tier. The 3.14 leg additionally reports branch coverage and, on pull requests, enforces diff coverage on the changed lines, and runs the API contract tests (once, rather than on both legs).
- **Lint**: `ruff check` and `ruff format --check`.
- **Typing**: `mypy backend`.
- **Dependency audit**: `pip-audit` against the pinned requirements.
- **Pre-commit hooks**: `pre-commit run --all-files`, validating the hook configuration the local development loop uses (see "Local checks" below).
- **Frontend**: the type-check and production build.

The backend runs on Windows because that is the application's platform: the screen-capture and input-listener code paths target it directly.

A separate scheduled workflow (`.github/workflows/nightly.yml`) runs the slower checks once a day: it re-runs the dependency audit (so an advisory published after a change has landed is surfaced without waiting for the next pull request) and runs the mutation campaign, publishing the mutation score and refreshing the coverage figure as the badges at the top of the README.

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
- general hygiene: end-of-file and trailing-whitespace fixers, YAML and TOML validity, merge-conflict markers, and a mixed-line-ending check (line-ending policy itself is set per file type in `.gitattributes`).

The dependency audit is slower, so it is reserved for the manual stage rather than every commit; run it explicitly when checking dependencies:

```bash
pre-commit run --hook-stage manual pip-audit
```

The mypy and test hooks run against the active virtual environment; the lint and hygiene hooks run in environments pre-commit manages itself, so the CI `pre-commit` job exercises those without reinstalling the dependency tree the typing and backend jobs already cover.

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
