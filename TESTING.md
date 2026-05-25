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

## Continuous integration

Every pull request and push to `main` runs three jobs (`.github/workflows/ci.yml`):

- **Backend**, on Windows across Python 3.11 and 3.14: the suite excluding the `full` tier.
- **Lint**: `ruff check` and `ruff format --check`.
- **Frontend**: the type-check and production build.

The backend runs on Windows because that is the application's platform: the screen-capture and input-listener code paths target it directly.

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
