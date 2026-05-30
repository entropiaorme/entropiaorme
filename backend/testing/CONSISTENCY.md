# Snapshot / event-stream consistency

A hydrating client that fetches a snapshot once and then follows the
event bus must converge on the same state a fresh re-fetch would
return. This module's apparatus mechanises that property end-to-end
against the scripted scenario corpus.

## The property

Given a scenario split into a pre-midpoint segment (`chat_replay.log`)
and a post-midpoint segment (`chat_replay_after.log`):

1. Replay the pre-midpoint segment through the production pipeline.
2. Capture `snapshot_t0` via the surface's view function (a composer
   over the existing `*_impl` router helpers).
3. Install a fresh reducer on the bus, hydrate it with `snapshot_t0`,
   replay the post-midpoint segment so each event folds into the
   hydrated state.
4. Capture `snapshot_t1` via the same view function on the live
   backend.
5. Assert: `reducer.state` (hydrated + folded) equals `snapshot_t1`
   (freshly composed) under the projected fields.

If the assertion holds, the hydrating client and the polling client
have converged. If it diverges, either the reducer is missing an
event, the view is reading a field outside the projected set, or the
production pipeline is treating the same event differently in the two
paths.

## Why this matters

The downstream work that moves the backend off the HTTP poll-poll-poll
backbone onto an event-driven push model collapses every dashboard
poll into a one-shot snapshot fetch followed by event subscription on
the same surface. Whatever frontend stores follow that change will
fold bus events into a snapshot-shaped projection. The Python reducer
in `store_reducers.py` is the contract those stores will be authored
against: the harness pins it here against the live backend, so the
frontend side can be a faithful port rather than a discovery exercise.

When the consolidated `/api/snapshot` endpoint lands, the surface's
`view_fn` becomes a one-line call to that endpoint (or its ASGI
equivalent) and the harness is otherwise unchanged: the property is
invariant to the transport.

## Scenario format

A consistency scenario directory under
`backend/tests/e2e/corpus/scripted/consistency_*/` carries:

```
consistency_<surface>_<beat>_midpoint/
  metadata.yaml          # description, segments, expected event counts
  build.py               # DSL build script
  chat_replay.log        # pre-midpoint events
  chat_replay_after.log  # post-midpoint events
```

The two-file split is the midpoint marker. There is no sentinel line
embedded in `chat_replay.log` and no event-index parameter coupling
the test to the scenario's length: the filesystem layout itself is
the convention, and any author can read the directory and see the
beats. Scenarios that do not exercise the consistency property keep
their single `chat_replay.log` and stay unchanged.

If a surface needs events that the chatlog does not carry (synthetic
bus events for a future scan / codex hydration path, for example),
add `bus_events.jsonl` / `bus_events_after.jsonl` sidecars beside the
chat segments. None of the present scenarios need them; the
convention is reserved for the moment those event surfaces land.

## Adding a new surface

Each surface contributes:

- A **view function** `Callable[[ViewContext], dict]` composing the
  surface's hydration state from live backend handles (typically by
  calling the existing `*_impl` router helpers and selecting the
  fields the projection tracks).
- A **`Reducer` subclass** declaring the bus topics it subscribes to,
  the initial-state dict shape, and an `on_event(topic, payload)`
  handler that folds events into `self._state`.
- A **`SurfaceAdapter`** wiring the two together with a stable surface
  name.
- One **scripted scenario** with the two-segment split, plus a test
  using the `ConsistencyHarness` and `pytest-regressions`'
  `data_regression` fixture for golden review.

Keep the projection a strict subset of the snapshot view: include a
field only when the bus payload alone gives the reducer enough to
update it without re-implementing production-side derivation
(equipment-library lookups, kill correlation, TT-value formulas).
Fields the reducer cannot derive get dropped from the projection;
they remain in the snapshot view's HTTP response but are out of scope
for the consistency property until the matching event surface lands.

## Applicability today

The tracking surface is the only one with a meaningful event-stream
contract in the current backend:

- **Tracking**: events `session_started`, `session_stopped`, `combat`,
  `loot_group` carry the data the reducer's projection needs. The
  property runs cleanly.
- **Quests**: the chatlog-driven auto-start path (`mission_received`)
  is event-stream-driven, but quest completion (the input to the
  session-link suggestion) flows via `quest_reward_filter` calling
  `QuestService.complete_quest` directly without a bus event. A
  meaningful quests property test waits on the event surface the
  future event-driven hydration model introduces.
- **Scan**: skill scans complete via a `SkillScanManual` callback
  rather than a bus event; the snapshot view reads the
  `skill_calibrations` table the callback wrote into. No bus
  consumption to assert against, so the property is vacuous today.
- **Codex**: claims and rank progression are HTTP-mutated via
  `/codex/claim` and `/codex/calibrate`; no bus event flows. Same
  situation as scan.

The apparatus ships with the tracking property test now; scan,
codex, and quests join the suite when the bus contracts for them
land. This document is the contract that future work authors
against.

## Running the suite

```
pytest backend/tests/e2e/test_consistency_*.py -v
```

The suite contains the surface property tests plus a negative-control
test that wires a deliberately-broken reducer onto the tracking
scenario and asserts the harness reports a divergence. The control
is a regression test for the apparatus itself: a future refactor that
weakened the comparison so the property no longer caught reducer
regressions would fire the control before reaching production.

To regenerate the `pytest-regressions` goldens after a deliberate
projection change:

```
pytest backend/tests/e2e/test_consistency_*.py --force-regen
```

Review the diff against the prior golden before committing, the same
guardrail the surrounding harness's `--update-fingerprints` workflow
applies, and follow the goldens-regeneration commit-message convention
in [TESTING.md](../../TESTING.md#commit-message-convention).
