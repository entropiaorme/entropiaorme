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
- `http_fingerprint.py`: per-endpoint HTTP response fingerprints.
- `consistency.py`, `store_reducers.py`: the consistency harness and reducers.
- `recorder.py`, `recording_controller.py`: the live-session recorder.
- `capturer.py`: the fixture-backed screen-capture seam for OCR replay.
