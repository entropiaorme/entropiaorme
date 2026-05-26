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

Deadlines are disabled on every profile: example timing varies on shared runners, and a deterministic property must never fail merely because one example ran slowly. A failing property prints a minimal, shrunk counterexample (with a reproduction blob under the `ci` profile) so it can be replayed exactly.

```bash
HYPOTHESIS_PROFILE=ci .venv/Scripts/python.exe -m pytest -m "not full"
```

## Continuous integration

Every pull request and push to `main` runs five jobs (`.github/workflows/ci.yml`):

- **Backend**, on Windows across Python 3.11 and 3.14: the suite excluding the `full` tier. The 3.14 leg additionally reports branch coverage and, on pull requests, enforces diff coverage on the changed lines.
- **Lint**: `ruff check` and `ruff format --check`.
- **Typing**: `mypy backend`.
- **Dependency audit**: `pip-audit` against the pinned requirements.
- **Frontend**: the type-check and production build.

The backend runs on Windows because that is the application's platform: the screen-capture and input-listener code paths target it directly.

A separate scheduled workflow (`.github/workflows/nightly.yml`) re-runs the dependency audit once a day, so an advisory published after a change has landed is surfaced without waiting for the next pull request.

## Test layout

| File | Coverage |
| ---- | -------- |
| `test_codex_formulas.py` | Codex rank multipliers, category cycling, cat4 bonus shape, rank cost and reward PED, inverse TT lookup. |
| `test_chatlog_parser.py` | Every EventType has at least one parametrised case. Critical-hit vs damage-dealt, HoF vs Global, quantity extraction, verbose vs direct skill formats. |
| `test_cost_engine.py` | Per-shot cost breakdown: weapon, amp, scope, absorber, damage enhancers, markups. Heal cost, heal range, damage range, weapon total damage. |
| `test_character_calc.py` | TT value curve anchors, profession level math, skill rank lookup, codex category resolution, HP formula, HP optimiser, profession path optimiser (target and budget modes). |
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
