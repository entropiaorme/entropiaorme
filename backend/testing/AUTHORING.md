# Authoring DSL scenarios for the E2E replay harness

This is the convention reference for hand-authoring scripted
scenarios via the gameplay DSL at `backend/testing/dsl.py`.
Recorded scenarios captured from live gameplay are covered by a
separate workflow documented in `RECORDING.md`; this document
covers scripted scenarios only.

## What a scripted scenario is

A self-describing directory under
`backend/tests/e2e/corpus/scripted/<name>/` carrying:

- `metadata.yaml` (handwritten): describes the scenario's
  surfaces, expected kill count, character context, and any
  notes useful for a reader.
- `build.py` (handwritten): a small script that imports the
  DSL, builds a `Scenario`, and calls `s.write(...)` to emit
  `chat_replay.log`. Re-run to regenerate the log when the
  authoring source changes.
- `chat_replay.log` (DSL-generated): the canonical chat.log
  the harness streams through the production `ChatlogWatcher`
  tail loop. Do not hand-edit; regenerate via `build.py`.
- `expected/fingerprint.jsonl` and `expected/db_state.json`
  (golden, generated): captured by
  `pytest --update-fingerprints` after the scenario is wired
  into a test file. Asserted against on every subsequent run.

Plus a paired test file at
`backend/tests/e2e/test_<scenario_name>.py` that boots the
pipeline, replays the scenario, and asserts both explicit
kill-shape expectations and golden equality.

## DSL surface

```python
from backend.testing.dsl import Scenario

s = Scenario(name="my_scenario")
s.at("2026-05-19 10:00:00")
s.combat.damage_dealt(15.0)
s.tick()
s.loot.received("Shrapnel", value_ped=3.50, quantity=350)
s.write(scenario_dir)
```

### Time management

- `s.at(timestamp)` sets the current "now" for subsequent
  builders. Accepts `str` parsed as `"%Y-%m-%d %H:%M:%S"` or
  `datetime`. Use it to anchor the scenario and to jump
  explicitly between widely-separated time windows.
- `s.tick(seconds=1)` advances the current timestamp by
  `seconds` (default 1). Use it as a visual flush marker
  between event clusters so consecutive emissions get
  monotonically-rising timestamps without re-typing absolute
  values.
- A builder call before any `s.at(...)` raises `RuntimeError`;
  the same applies to `s.tick()` before any `s.at(...)`.

The DSL does not model the runtime tick buffer or the
combat-accumulator state-machine. It only emits chat.log lines
in source order; the production `ChatlogWatcher` reads them and
the production tracker handles the accumulator semantics.

### Sub-namespaces

| Namespace | Builders | Notes |
| --- | --- | --- |
| `s.combat` | `damage_dealt(amount)`, `critical_hit(amount)`, `target_dodge()`, `target_evade()`, `target_jam()`, `damage_received(amount)`, `player_dodge()`, `player_evade()`, `player_jam()`, `mob_miss()`, `deflect()`, `self_heal(amount)` | Offensive + defensive combat lines. |
| `s.loot` | `received(item_name, value_ped, quantity=1)` | Single-item drop when `quantity == 1`, quantity-bearing form otherwise. |
| `s.skill` | `gained(amount, skill)` | Modern format `"You have gained X.XXXX Skill"`. Other format variants can be added if a scenario needs to pin them. |
| `s.enhancer` | `broken(enhancer_name, item_name, shrapnel_ped, remaining=0)` | Default `remaining=0` matches the most-observed "last enhancer just broke" shape. |
| `s.globals` | `kill(player, creature, value_ped, hof=False)`, `item(player, item, value_ped, hof=False)` | Set `hof=True` to append the Hall-of-Fame suffix and promote the parse to `HOF_KILL` / `HOF_ITEM`. |
| `s.mission` | `received(mission_name)`, `completed(mission_name)` | Mission lifecycle lines. |

The DSL is silent on event surfaces that are not chatlog-sourced
in the current parser: repair tooling and profession panel
events surface via the screen-capture and keystroke harness layers,
and scenarios needing those surfaces use the corresponding layers. Player-state lines (death, revive, item
tier-up) are also out of scope: the parser recognises no such
lines today, so the DSL has no sub-namespace for them.

### Emission

- `s.write(scenario_dir)` writes `chat_replay.log` into
  `scenario_dir` (created if absent) and returns the resolved
  path. One line per recorded event, in source order.
- `s.lines()` returns the in-memory line list without writing;
  used by the round-trip property test and by ad-hoc author
  debugging.

## Workflow for a new scenario

1. Pick a parser-surface gap or behavioural surface to pin.
   Check the existing `corpus/scripted/` directories; the goal
   is one scenario per logically distinct surface, not many
   scenarios per surface.
2. Create the scenario directory:
   `backend/tests/e2e/corpus/scripted/<name>/`.
3. Write `metadata.yaml` per the existing convention: `name`,
   `flavour: scripted`, `description`, `surfaces` list,
   `events` count, `expected_kills` count, `character_context`
   placeholders, and a `notes` line. The `events` field counts
   chat lines the scenario's `chat_replay.log` carries (the same
   convention the original `basic_hunt_10_events` scenario set,
   encoded in its name); it is not the count of bus events the
   tracker emits at replay time, which the goldens already pin
   downstream of the parser's tick-grouping.
4. Write `build.py` importing the DSL. Anchor with `s.at(...)`,
   emit builders interleaved with `s.tick()` calls, finish with
   `s.write(Path(__file__).parent)`.
5. Generate `chat_replay.log` once via
   `python -m backend.tests.e2e.corpus.scripted.<name>.build`.
6. Write the paired test file at
   `backend/tests/e2e/test_<name>.py`. Match the existing
   convention: pull `e2e_pipeline` (or `make_e2e_pipeline` if a
   non-default `player_name` is needed), build a `golden_set`,
   install the recorder on the bus, run `tracker.start_session`,
   `replay_scenario`, `wait_for_drain`, `tracker.stop_session`.
   Add explicit kill-shape assertions so a reader can orient at
   a glance without diffing the goldens, then call
   `goldens.assert_matches(in_memory_db)`.
7. Generate the goldens via
   `pytest backend/tests/e2e/test_<name>.py --update-fingerprints`.
8. Re-run without the flag to confirm the goldens are stable.
9. Commit the directory + test file + goldens together.

## Regeneration cadence

- `build.py` is the source of truth for `chat_replay.log`. If
  you change the authoring script, re-run it and commit both
  files together.
- `expected/{fingerprint.jsonl, db_state.json}` are the source
  of truth for "what the production code did with this
  scenario." If a code change deliberately changes behaviour,
  regenerate the goldens via
  `pytest --update-fingerprints` (narrow with ordinary pytest
  path filters), review the diff in the PR, and commit the
  golden update alongside the code change with a commit message
  in the form `e2e: update golden fingerprints for <scope>:
  <reason>`.

## Determinism notes

- Every `ORDER BY` in the DB-snapshot SQL catalogue anchors on a
  stable source-side key (chatlog-parsed timestamp) with the
  SQLite `rowid` as tiebreaker. Author scenarios so unique
  timestamps fall on each load-bearing event; scenarios with
  identical timestamps on multiple events that need stable
  golden ordering should add a one-second gap between them.
- Floating-point values get rounded to four decimals at
  fingerprint normalisation time; authoring with up to four
  decimal places keeps goldens stable across float
  representation quirks.
- Scenario name will be the stochasticity seed when higher-level
  templates (e.g. realistic-hunt expansions) eventually land.
  Keep scenario names stable; renaming a scenario invalidates
  its template-derived output and forces a regeneration.

## What not to do

- Don't hand-edit `chat_replay.log`. Author through
  `build.py`; the round-trip test will catch parser drift but
  not authoring drift between the script and the emitted log.
- Don't author the same surface across multiple scenarios
  without intent. One scenario per behavioural surface keeps
  the corpus orientation-friendly; the goldens catch
  duplicate-surface drift on either scenario if behaviour
  regresses.
- Don't author scenarios that lean on real-world timing edge
  cases (file-watcher jitter, partial-line reads, encoding
  variants). The harness reads from disk via the real tail
  loop but with a deterministic write order from
  `replay_scenario`; OS-level edge cases are out of scope here
  and belong in live-UAT verification instead.
