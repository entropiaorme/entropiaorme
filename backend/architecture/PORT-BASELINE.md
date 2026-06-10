# Port baseline: the Python backend's reference numbers

A point-in-time performance and coverage reference for the Python
sidecar, captured before any route of the native (Rust) backend port
went live. Port work is graded against these numbers in flight; the
publishable before/after comparison is produced separately at the end
of the port as a same-day, same-host measurement of both
implementations, so headline claims never rest on two measurements
taken far apart on a drifted machine. This document is that final
comparison's "before" anchor and the in-flight drift reference, not a
benchmark of record.

The tables between the generated markers are written by
`backend/scripts/capture_port_baseline.py`; the prose is maintained by
hand. Regenerate only at deliberate measurement milestones (the
capture commands are below), never as a side effect of unrelated work:
the value of a baseline is that it does not move.

## Host and capture metadata

All figures are host-pinned: they were captured on the machine below
and are only comparable to numbers captured on the same machine. A
native-backend figure measured elsewhere says nothing against this
table.

<!-- BEGIN GENERATED: host -->
| | |
| --- | --- |
| Captured | 2026-06-10 00:40 UTC |
| Commit | `3f48b7c` |
| Platform | Linux x86_64 |
| CPU | 12th Gen Intel(R) Core(TM) i7-12800H |
| Python | 3.12.1 |
<!-- END GENERATED: host -->

## Process figures

Whole-process numbers for the sidecar booted exactly as the external
test harness boots it (`backend/testing/external_process.py`), with a
scenario loaded and test mode on. Cold start is the time from process
spawn to the health route serving 200; idle RSS is sampled after a
settle pause; shutdown is the graceful-signal path the packaged app
uses.

<!-- BEGIN GENERATED: process -->
| Measurement | median | p95 | min | max | samples | unit |
| --- | --- | --- | --- | --- | --- | --- |
| Cold start to healthy | 0.8377 | 0.8645 | 0.8314 | 0.8645 | 5 | s (RSS: MiB) |
| Idle RSS after 2s settle (MiB) | 121.4 | 121.4 | 121.3 | 121.4 | 5 | s (RSS: MiB) |
| Graceful shutdown | 0.3651 | 0.4152 | 0.3647 | 0.4152 | 5 | s (RSS: MiB) |
<!-- END GENERATED: process -->

## HTTP hydration latency

Loopback request latency per hydration endpoint against a freshly
replayed scenario state, measured through the same client the test
harness uses. These are single-client, sequential figures: the app's
real traffic pattern (one desktop frontend) looks the same.

<!-- BEGIN GENERATED: http -->
| Endpoint | p50 ms | p95 ms | min ms |
| --- | --- | --- | --- |
| `GET_codex_meta_attributes` | 1.0912 | 1.1988 | 0.995 |
| `GET_health` | 1.0182 | 1.1539 | 0.9278 |
| `GET_quests` | 1.0907 | 1.2203 | 1.0019 |
| `GET_quests_analytics` | 1.011 | 1.1826 | 0.9993 |
| `GET_quests_mobs` | 1.02 | 1.2871 | 0.9993 |
| `GET_quests_playlists` | 1.0091 | 1.1676 | 0.9972 |
| `GET_scan_skills_status` | 1.125 | 1.777 | 0.8134 |
| `GET_tracking_session_detail` | 1.2282 | 1.5173 | 1.1981 |
| `GET_tracking_session_quest_link_suggestion` | 1.0583 | 1.2082 | 1.0301 |
| `GET_tracking_sessions` | 1.2792 | 2.0239 | 1.1102 |
| `GET_tracking_snapshot` | 1.1188 | 1.3274 | 1.0796 |

30 requests per endpoint after 3 warm-ups, against a freshly replayed `basic_hunt_10_events` state.
<!-- END GENERATED: http -->

## Leaf hot paths

Micro-benchmarks of the pure leaf functions that sit on per-event or
per-shot paths. The native port's benchmark rail measures its ports of
these same paths; band (i) below compares the two.

<!-- BEGIN GENERATED: hot-paths -->
| Hot path | per-call µs |
| --- | --- |
| `chatlog_parser.parse_line (damage line)` | 10.318 |
| `cost_engine.cost_per_shot_from_props` | 3.285 |
| `tt_value_curve.levels_for_tt_value` | 59.276 |
| `tt_value_curve.tt_value_at` | 0.785 |

Method: timeit, best of 5 runs of 10000 calls.
<!-- END GENERATED: hot-paths -->

## OCR

Recogniser figures over the locally recorded skill-panel corpus (the
corpus is local-by-default and never committed, so this leg records an
honest skip on hosts without it). Engine load is the one-off model
initialisation; the first page includes session warm-up; warm pages are
the steady state.

<!-- BEGIN GENERATED: ocr -->
| Measurement | value |
| --- | --- |
| Pages read | 12 |
| Engine load (s) | 0.2 |
| First page (s) | 2.105 |
| Warm page median (s) | 2.1739 (p95 2.1846) |
| Process RSS delta (MiB) | 100.4 |
<!-- END GENERATED: ocr -->

## Artefact sizes

The sidecar frozen with PyInstaller on this host, as a same-host size
comparand for a future native artefact. The shipped Windows installer
and its installed footprint are platform artefacts and are captured on
the application's Windows target.

<!-- BEGIN GENERATED: artefacts -->
| Artefact | size MiB |
| --- | --- |
| `entropiaorme-backend` (Linux freeze) | 219.8 |
| Windows installer / installed footprint | pending capture on the application's Windows target |

Freeze duration on this host: 106.3s.
<!-- END GENERATED: artefacts -->

## Performance bands

How port work consumes the numbers above. Any change to a band is a
deliberate, recorded decision made outside a measurement run, never an
adjustment applied at measurement time to make a result fit.

1. **Per-unit hot paths.** Where a ported unit owns a named hot path
   (the leaf table above), the native benchmark rail's figure must not
   be slower than the Python figure for that path on the same host. A
   native port slower than the interpreted original signals a
   structural mistake and stops the work for investigation.
2. **Process-level figures.** Cold start and idle RSS are checked at
   phase boundaries (not per unit; a single unit cannot meaningfully
   move them) against the process table above, with a +10% band.
   Breaching the band stops the work for investigation.
3. **The final before/after.** At the end of the port, both
   implementations are measured on the same host on the same day and
   the aggregate comparison is published; this document supplies the
   methodology, not those numbers.

## Per-module branch coverage

The equivalence claim behind every port is only as strong as the code
paths the test corpus exercises, so each module's measured Python
branch coverage is recorded here and consulted at port time: a ported
module whose measured branch coverage falls materially below its
Python figure is not done, and a thin figure here flags a module that
needs coverage work before its behaviour can be locked by tests. The
generated coverage matrix (`backend/testing/COVERAGE.md`) is a
presence map; this table carries the figures.

One known measurement wrinkle, found and then sharpened by review:
the `backend/routers/analytics.py` row is measurement-unstable by
roughly one branch (about one percentage point) and is excluded from
the exactness claim below. Independent re-runs showed two
contributors: property-based test data reaching data-dependent guards
run to run (a zero-shot session guard in the activity loader), and
the overview trend block's real-clock 30/60-day windows shifting
which trend branches execute across UTC dates. Treat that row at
whole-percent granularity; making the analytics measurement
deterministic (seeded generation plus an injected clock in those
tests) is tracked as its own follow-up and un-caveats the row.

<!-- BEGIN GENERATED: coverage -->
| Module | statements | branches | branch % | overall % |
| --- | --- | --- | --- | --- |
| `backend/core/domain_events.py` | 28 | 0 | no branches | 100.0 |
| `backend/core/event_bus.py` | 54 | 12 | 100.0 | 100.0 |
| `backend/core/events.py` | 11 | 0 | no branches | 100.0 |
| `backend/data/codex_categories.py` | 34 | 10 | 100.0 | 100.0 |
| `backend/data/tt_value_curve.py` | 39 | 14 | 100.0 | 100.0 |
| `backend/db/app_database.py` | 74 | 30 | 80.0 | 87.5 |
| `backend/db/base.py` | 36 | 2 | 100.0 | 100.0 |
| `backend/dependencies.py` | 31 | 0 | no branches | 100.0 |
| `backend/main.py` | 287 | 82 | 52.4 | 69.9 |
| `backend/middleware/etag.py` | 70 | 30 | 93.3 | 96.0 |
| `backend/routers/analytics.py` | 401 | 114 | 84.2 | 92.8 |
| `backend/routers/character.py` | 328 | 110 | 82.7 | 90.0 |
| `backend/routers/codex.py` | 57 | 6 | 66.7 | 88.9 |
| `backend/routers/demo.py` | 90 | 14 | 57.1 | 84.6 |
| `backend/routers/equipment.py` | 231 | 72 | 87.5 | 91.7 |
| `backend/routers/events.py` | 29 | 4 | 25.0 | 75.8 |
| `backend/routers/health.py` | 6 | 0 | no branches | 100.0 |
| `backend/routers/quests.py` | 152 | 16 | 56.2 | 91.1 |
| `backend/routers/recording.py` | 46 | 2 | 100.0 | 95.8 |
| `backend/routers/response_models.py` | 640 | 0 | no branches | 100.0 |
| `backend/routers/scan_manual.py` | 44 | 4 | 50.0 | 89.6 |
| `backend/routers/settings.py` | 95 | 20 | 85.0 | 94.8 |
| `backend/routers/testing.py` | 62 | 16 | 81.2 | 89.7 |
| `backend/routers/tracking.py` | 488 | 154 | 64.3 | 78.2 |
| `backend/scripts/check_ambient_time.py` | 119 | 58 | 86.2 | 84.7 |
| `backend/scripts/check_authoring_lint.py` | 96 | 30 | 93.3 | 98.4 |
| `backend/scripts/check_golden_ratification.py` | 197 | 70 | 75.7 | 88.0 |
| `backend/scripts/check_no_bare_setinterval.py` | 60 | 14 | 100.0 | 100.0 |
| `backend/scripts/check_version_stamps.py` | 46 | 6 | 100.0 | 100.0 |
| `backend/scripts/classify_change_scope.py` | 50 | 12 | 100.0 | 100.0 |
| `backend/scripts/coverage_matrix.py` | 75 | 16 | 93.8 | 97.8 |
| `backend/services/character_calc.py` | 236 | 94 | 98.9 | 99.4 |
| `backend/services/chatlog_parser.py` | 104 | 26 | 96.2 | 98.5 |
| `backend/services/chatlog_watcher.py` | 245 | 84 | 92.9 | 95.1 |
| `backend/services/codex_service.py` | 181 | 74 | 91.9 | 96.9 |
| `backend/services/config_service.py` | 142 | 40 | 95.0 | 98.4 |
| `backend/services/cost_engine.py` | 89 | 28 | 100.0 | 100.0 |
| `backend/services/event_stream.py` | 64 | 18 | 83.3 | 92.7 |
| `backend/services/game_data_store.py` | 58 | 22 | 100.0 | 100.0 |
| `backend/services/mob_lookup_service.py` | 54 | 26 | 100.0 | 100.0 |
| `backend/services/quest_service.py` | 548 | 202 | 91.1 | 96.3 |
| `backend/services/scan_completion.py` | 49 | 8 | 75.0 | 93.0 |
| `backend/services/scan_drift.py` | 33 | 6 | 100.0 | 100.0 |
| `backend/services/session_summary.py` | 77 | 26 | 92.3 | 97.1 |
| `backend/services/skill_panel_parse.py` | 75 | 28 | 92.9 | 96.1 |
| `backend/services/skill_scan_core.py` | 50 | 10 | 100.0 | 100.0 |
| `backend/services/skill_tracker.py` | 75 | 14 | 92.9 | 98.9 |
| `backend/services/trifecta_service.py` | 40 | 14 | 100.0 | 100.0 |
| `backend/testing/capturer.py` | 43 | 10 | 100.0 | 100.0 |
| `backend/testing/clock.py` | 31 | 2 | 100.0 | 100.0 |
| `backend/testing/clock_plan.py` | 40 | 16 | 68.8 | 82.1 |
| `backend/testing/config.py` | 22 | 0 | no branches | 100.0 |
| `backend/testing/consistency.py` | 66 | 12 | 100.0 | 100.0 |
| `backend/testing/cost_engine_cli.py` | 17 | 6 | 66.7 | 82.6 |
| `backend/testing/db_snapshot.py` | 26 | 2 | 50.0 | 89.3 |
| `backend/testing/diff.py` | 89 | 52 | 96.2 | 97.2 |
| `backend/testing/dsl.py` | 133 | 16 | 93.8 | 99.3 |
| `backend/testing/equivalence/conformance_cases.py` | 3 | 0 | no branches | 100.0 |
| `backend/testing/equivalence/table.py` | 13 | 0 | no branches | 100.0 |
| `backend/testing/equivalence/yml_family.py` | 18 | 2 | 100.0 | 100.0 |
| `backend/testing/events_sink.py` | 34 | 6 | 83.3 | 95.0 |
| `backend/testing/external_process.py` | 152 | 36 | 75.0 | 87.8 |
| `backend/testing/fingerprint.py` | 91 | 36 | 91.7 | 94.5 |
| `backend/testing/golden.py` | 69 | 16 | 56.2 | 83.5 |
| `backend/testing/http_fingerprint.py` | 108 | 28 | 85.7 | 91.2 |
| `backend/testing/keystroke_source.py` | 91 | 20 | 100.0 | 100.0 |
| `backend/testing/normalize_cli.py` | 21 | 8 | 87.5 | 93.1 |
| `backend/testing/recorder.py` | 91 | 4 | 75.0 | 98.9 |
| `backend/testing/recording_controller.py` | 157 | 16 | 81.2 | 94.8 |
| `backend/testing/replay.py` | 34 | 10 | 100.0 | 100.0 |
| `backend/testing/store_reducers.py` | 177 | 60 | 88.3 | 96.6 |
| `backend/testing/wire.py` | 14 | 8 | 100.0 | 100.0 |
| `backend/tracking/loot_filter.py` | 11 | 2 | 100.0 | 100.0 |
| `backend/tracking/models.py` | 72 | 0 | no branches | 100.0 |
| `backend/tracking/schema.py` | 7 | 0 | no branches | 100.0 |
| `backend/tracking/tool_inference.py` | 63 | 16 | 100.0 | 100.0 |
| `backend/tracking/tracker.py` | 777 | 224 | 91.1 | 96.3 |

Excluded from measurement (device / IO glue that cannot run headless; see the `omit` list in `pyproject.toml`): `backend/ocr/capturer.py`, `backend/services/eu_window.py`, `backend/services/hotbar_listener.py`, `backend/services/local_ocr.py`, `backend/services/repair_ocr.py`, `backend/services/scan_presets.py`, `backend/services/skill_scan_manual.py`, `backend/services/spacebar_capture_listener.py`. Their equivalence rests on the recorded OCR / scan corpus and the input-seam tests rather than a coverage figure.
<!-- END GENERATED: coverage -->

## Reproduction and tolerances

From the repo root, with the development virtualenv:

```
python -m pytest -m "fast or standard" -p no:randomly --cov=backend \
    --cov-branch --cov-report= -n auto --dist=loadfile
python -m coverage json
python -m backend.scripts.capture_port_baseline --coverage-json coverage.json
```

Skip flags (`--skip-process`, `--skip-http`, `--skip-hot-paths`,
`--skip-ocr`, `--skip-freeze`) allow partial reruns; a skipped leg
keeps its previous value. `--render-only` re-renders this document
from the committed JSON (`port_baseline.json`).

Expected run-to-run variance on an otherwise idle host: timing medians
(process, HTTP, hot paths, OCR) within roughly ±25%; RSS within ±10%;
artefact sizes exact at the table's rounding; the coverage table exact
for the same commit and dependency set, except the
`backend/routers/analytics.py` row, which is measurement-unstable by
about a branch (see the caveat above). Two sharpenings from review: the
sub-millisecond legs (HTTP medians, hot-path microseconds) are
sensitive to execution context inside a long full-pipeline run on a
hybrid-core CPU, so verify them with standalone leg re-runs (the skip
flags isolate a leg) where the ±25% envelope applies like-for-like;
and the document is a pure function of `port_baseline.json` (the
renderer always re-reads the serialised JSON), so `--render-only` is
byte-stable. A re-run outside those envelopes on the same host
warrants investigation, not a quiet refresh.
