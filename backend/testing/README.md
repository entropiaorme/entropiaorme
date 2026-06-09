# Backend test harness

`backend/testing/` holds the end-to-end replay harness: it redirects the
backend's real input surfaces (the chat.log tail, screen capture, and the
hotbar / spacebar keystroke hooks) to in-workspace fixtures at test time, so a
scripted or recorded scenario drives the production pipeline and its output is
diffed against golden files. This page is the index; each concern has its own
document.

## Start here

- **Run the tests** (tiers, parallelism, golden regeneration): the repo-root
  [`TESTING.md`](../../TESTING.md).
- **Write a new scripted scenario**: [`AUTHORING.md`](AUTHORING.md), which
  covers the gameplay DSL, the scenario directory layout, and the golden
  workflow.
- **Record a scenario from live play**: [`RECORDING.md`](RECORDING.md), which
  covers the developer recording mode and the local-by-default corpus policy.

## The documents

| Document | Covers |
| --- | --- |
| [`../../TESTING.md`](../../TESTING.md) | Running the suite: the `fast` / `standard` / `full` / `contract` tiers, local parallelism, the serial CI rationale, and the goldens-regeneration convention. |
| [`AUTHORING.md`](AUTHORING.md) | The gameplay DSL, scenario directory layout, and how scripted scenarios are built and kept in sync with the parser. |
| [`RECORDING.md`](RECORDING.md) | The developer recording workflow that captures a live session into a replayable bundle, and the local-by-default privacy policy. |
| [`CONSISTENCY.md`](CONSISTENCY.md) | The snapshot to event-stream consistency apparatus and the store-reducer reference it pins. |
| [`CONFORMANCE.md`](CONFORMANCE.md) | The HTTP / OpenAPI conformance surface: per-endpoint response fingerprints, the ETag substrate, and OpenAPI drift detection. |
| [`COVERAGE.md`](COVERAGE.md) | The generated service-to-scenario coverage matrix and the mutation-campaign scope. |

## Module map

- `config.py`, `clock.py`: the test-mode config overlay and the injectable clock.
- `replay.py`: streams a scenario's chat lines into the watcher and drains it.
- `dsl.py`: the gameplay DSL scripted scenarios are authored in.
- `fingerprint.py`, `db_snapshot.py`, `golden.py`, `diff.py`: the event-stream
  and database-state fingerprint format, golden storage, and diff rendering.
- `http_fingerprint.py`: per-endpoint HTTP response fingerprints and the
  canonical hydration-endpoint capture order.
- `wire.py`: reduces live bus payloads to their JSON wire form.
- `events_sink.py`: the test-mode `events.jsonl` full-stream bus sink.
- `consistency.py`, `store_reducers.py`: the consistency harness and reducers.
- `recorder.py`, `recording_controller.py`: the live-session recorder.
- `capturer.py`: the fixture-backed screen-capture seam for OCR replay,
  single-shot and sequenced.
- `external_process.py`: boots the backend as a whole subprocess, drives a
  scenario over HTTP, and captures the three equivalence surfaces in golden
  form; the launch command is a parameter so a second implementation of the
  same surface can take one leg.

## External whole-process runs

Most of the suite drives the pipeline in-process. The same harness also
boots as a whole process: `backend/main.py`'s composition root reads
`TestModeConfig.from_env()` once at startup and, when test mode is on,
selects the redirected chatlog, the mock keystroke sources, the fixture
capturers, and an `events.jsonl` sink recording every bus publish in
order (a strict superset of the SSE surface). Frozen (packaged) builds
refuse test mode regardless of environment, and the test-only routes are
only registered when the overlay is active, so the production API shape
is untouched.

An external harness boots `python -m backend.main` with:

- `ENTROPIA_TEST_MODE=1`, plus `ENTROPIA_TEST_SCENARIO_DIR` for the
  scenario to drive.
- `ENTROPIA_TEST_CHATLOG` pointing at a fresh file for the watcher to
  tail (created at startup if missing). Do not point it at the
  scenario's committed `chat_replay.log`: the tail starts at end-of-file,
  and the replay route refuses to stream a file into itself.
- `ENTROPIA_TEST_CLOCK_START` set to the scenario clock plan's start
  instant (see `clock_plan.py`), so timestamps are deterministic.
- `ENTROPIAORME_BACKEND_PORT` / `ENTROPIAORME_DATA_DIR` for an isolated
  port and data directory. `events.jsonl` lands in the data directory.

It then drives the run over HTTP: `POST /api/testing/replay` performs
the canonical driver sequence (session start, tick-atomic streaming,
drain, the plan's clock advance, session stop) and returns only when the
process is drained and fingerprint-comparable; `GET /api/testing/drain`
exposes the watcher's raw drain state for feeders that stream the
chatlog themselves (it covers watcher-driven events only). Settings
changes that restart the watcher onto the user-configured path (such as
`PATCH /api/settings` with a new `chatlog_path`) are not test-mode-aware
and would undo the redirection; scenarios must not exercise them.

`backend/testing/external_process.py` packages this whole lifecycle
(spawn with a parametrised launch command, health wait, replay drive,
surface capture and normalisation, graceful shutdown) for reuse.
`backend/tests/e2e/test_external_process_equivalence.py` is the
single-process reference: it boots one subprocess this way and proves
the captured `events.jsonl`, database file, and hydration responses
byte-identical to the committed scenario goldens.
`backend/tests/e2e/test_dual_process_equivalence.py` boots two
processes per scenario and proves the same surfaces byte-identical
between the legs as well as to the goldens: the comparison shape a
second implementation of the surface plugs into by overriding one leg's
launch command.
