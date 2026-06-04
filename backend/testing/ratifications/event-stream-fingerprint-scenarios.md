# Oracle ratification: event-stream fingerprint scenarios

Independent ratification of eight scenario golden sets whose
`expected/fingerprint.jsonl` and `expected/db_state.json` changed inside the
audited range. Audited adversarially against the current tree, the production
code under test, and each scenario's source event stream; the author's
rationale was treated as a claim to be disproved, not accepted.

## Provenance (verified per set)

For every one of the 16 golden files (8 sets x 2 files), `git log` over the
audited range returns exactly one in-range commit: 4e6811e (PR #50, the
event-stream bridge). No other in-range commit (the OpenAPI work in #68, the
hydration-only read-surface refactor in #67, the codegen facade in #69, the
docs in #70) touched any of these files. The claimed provenance holds for all
eight.

## Behaviour under test (read from source, not inferred from the diff)

PR #50 publishes a typed `tracking.session.updated` domain-event envelope on
the in-process event bus at three moments, recorded by the fingerprint:

- one `started` envelope at session start (`HuntTracker.start_session`,
  stamped from the session start time);
- one `updated` envelope per settled tick that actually changed the live
  readout, emitted by `HuntTracker._on_tick_flushed`, which is dirty-gated: it
  publishes only when a P&L handler set `_session_dirty` during the tick and
  returns silently otherwise;
- one `stopped` envelope at session stop (`HuntTracker.stop_session`, stamped
  from the session end time).

The pre-existing `tick_flushed` bus frame is now also captured (the recorder
fingerprints every bus topic, and #50 made the tracker subscribe to it), so
each settled tick contributes a `tick_flushed` line whether or not it was
mutating. The dirty flag is set by the combat handler (offensive shots, target
dodge/evade/jam, damage received, a qualifying new self-heal activation), the
loot handler (past its dedup guard), the global handler (past the player-name
and staleness guards), and the enhancer-break handler (only when the break
matches a configured active weapon). Skill gains, and any tick the tracker
does not subscribe to as a mutation, do not set it.

This is the exact failure mode the ratification exists to catch: a new event
whose *frequency* is wrong (firing on no-op ticks). The `updated` count was
therefore verified against each scenario's real mutating ticks rather than
against tick occurrence.

## Delta accountability and minimality (all eight sets)

Mechanical confirmation across the whole set:

- Fingerprints: with the new `tracking.session.updated` and `tick_flushed`
  lines removed and `<TS_N>` symbols normalised, every pre-existing frame is
  byte-identical and in the same relative order in all eight sets. No
  pre-existing event line moved, changed value, or was dropped. The entire
  fingerprint delta is additive envelope/tick frames plus timestamp-symbol
  renumbering caused by the interleaved new timestamps.
- DB state: with `<TS_N>` symbols normalised, every db_state.json is
  byte-identical old-to-new in all eight sets (no concrete value: cost,
  amount, quantity, mob/item name, value_ped, dangling_cost, heal_cost,
  shots_fired, or id structure moved). The raw delta is a single constant
  timestamp-symbol offset per set, with no reordering (the offset set has
  exactly one element per file).

Per-set `updated` counts reconciled against mutating ticks, and the resulting
db_state offset (which equals 1 started + N updated + 1 stopped):

- defensive_combat_round (8 ticks): 3 updated. The five defensive no-op ticks
  (dodge, evade, jam, mob-miss, deflect) each produce a `tick_flushed` frame
  but NO `updated` envelope; only the two damage-received ticks and the
  qualifying self-heal tick mutate. Offset +5 = 1+3+1. This is the headline
  positive: the no-op suppression is real, not coincidental.
- empty_session (0 ticks): no `updated`/`tick_flushed` frames at all, only
  `started` and `stopped`. Offset +2 = 1+0+1. The named boundary case behaves
  correctly.
- enhancer_break_during_hunt (5 ticks): 4 updated. The enhancer-break tick
  produces a `tick_flushed` but NO `updated`, because the harness tracker has
  no configured active weapon, so `_on_enhancer_break` returns before setting
  the dirty flag; the break does not change the readout and the kill's cost
  stays 0.0. Offset +6 = 1+4+1.
- global_kill_correlated (4 ticks): 4 updated. The global tick DOES mutate
  here because the test wires `player_name="TestPlayer"` (matching the
  announcement), so the global handler correlates and tags the kill.
  Offset +6 = 1+4+1.
- hof_item_drop (4 ticks): 4 updated. Same correlated-global path
  (player_name configured), HoF tag applied. Offset +6 = 1+4+1.
- single_mob_hunt (4 ticks): 4 updated, all mutating. Offset +6 = 1+4+1.
- skill_gain_across_tick (7 ticks): 6 updated. The skill-only tick (Anatomy,
  no combat or loot) produces a `tick_flushed` but NO `updated`; the tracker
  does not subscribe to skill gains, so a skill-only tick cannot set the dirty
  flag. Offset +8 = 1+6+1. Second strong positive for the no-op suppression.
- placeholder_recorded_hunt (4 ticks, synthetic stand-in replayed through the
  same watcher path): 4 updated, all mutating. Offset +6 = 1+4+1.

Every element of every diff is accounted for by the intended behaviour change.
No collateral movement was found.

## Intended, not merely actual

The envelope shape is independently pinned by the schema-drift golden added in
the same commit (`backend/tests/expected/event_schemas.snapshot.json`) and the
closed `extra="forbid"` Pydantic contract in `backend/core/domain_events.py`,
so the recorded envelope structure is the declared contract rather than an
incidental dump. The frequency contract (one `updated` per mutating tick, none
on no-op ticks) is what the code is *meant* to do per the dirty-gating design,
and the goldens match that intent on the two scenarios that most stress it
(defensive no-op ticks; a skill-only tick). This is a genuine, desirable spec
move: a new additive event surface, correctly gated.

## Fix-versus-adapt

This is a first-generation pin of newly emitted output, not a regenerated pin
of changed values: the old goldens carried none of these frames. There is no
prior correct pin that the code could be made to hold instead; the events did
not exist before #50. Adapting the oracle is the only correct response. The
pre-existing frames were not disturbed, which is itself evidence that nothing
was regressed under cover of the addition.

## Determinism

One honest wrinkle, recorded as a non-blocking finding. The `started`/`stopped`
envelope `occurred_at` values (and the pre-existing db_state
`started_at`/`ended_at` columns) derive from the tracker's wall-clock reads
(`_now_fn`, defaulting to the system clock) at session start and stop, not from
replayed timestamps. In seven of the eight scenarios the harness does not
inject a clock. This does not leak ambient input into the goldens, because the
fingerprint normaliser assigns `<TS_N>` symbols by encounter position, not by
raw value: `started` is always the first timestamp encountered and `stopped`
the last, so the symbols are position-stable regardless of the underlying
instant, and the replayed tick timestamps never collide with the run-time wall
clock. The one place where two wall-clock reads sit back-to-back with nothing
between them is the zero-event empty_session, and that test correctly injects
a `MockClock` and advances it before stop
(`backend/tests/e2e/test_empty_session.py`, with the reasoning spelled out in
its docstring) precisely so the two boundaries are always distinct and the
golden is order-independent. The other seven rely on the real replay/drain gap
to keep the start and stop reads distinct; that gap is non-zero in practice
for any session with at least one tick, so the goldens are deterministic, but
the robustness rests on a timing margin rather than an injected seam.
Recommendation (non-blocking): for consistency and to remove the residual
margin-of-timing dependence, the seven wall-clock scenarios could adopt the
same `MockClock` injection empty_session already models. This is a latent,
pre-existing property of the tracker's clock use surfaced by #50, not a
regression introduced by it.

## Findings

- **Wall-clock session boundaries (non-blocking note).** The session
  start/stop envelope timestamps (`backend/tracking/tracker.py`, the stamp
  sites) ride the real wall clock in the seven non-mocked scenarios; the
  goldens stay deterministic because encounter-position symbolisation
  neutralises the raw value and the replay gap keeps the two reads distinct.
  empty_session already injects a MockClock for exactly this reason. Optional
  follow-up: extend the MockClock injection to the other seven scenarios to
  remove the timing-margin dependence; not required for soundness.
- **No-op-tick suppression verified (positive evidence).** The defensive
  no-op ticks in defensive_combat_round (dodge/evade/jam/miss/deflect) and the
  skill-only tick in skill_gain_across_tick correctly gain a `tick_flushed`
  frame but no `updated` envelope, confirming the dirty-gating frequency
  contract holds.
- **Unmatched enhancer break correctly silent (positive evidence).** The
  enhancer-break tick in enhancer_break_during_hunt produces no `updated`
  because no active weapon is configured in the harness, so the readout
  genuinely did not change. Consistent with the db_state (cost stays 0.0).

Nothing suggested a swept regression or a nondeterministic pin.

## Summary judgement

Provenance is clean (one in-range commit per file). Every fingerprint delta is
either a new, contract-pinned `tracking.session.updated` envelope, a newly
captured `tick_flushed` frame, or `<TS_N>` renumbering; every pre-existing
frame is preserved in value and order. Every db_state delta is a single
constant timestamp-symbol offset with not one concrete value moved, and each
offset reconciles exactly to one started plus one stopped plus one updated per
genuinely mutating tick. The frequency, the property a self-approving author most
easily skips, was checked against each scenario's real mutating ticks: the no-op
ticks (defensive reactions, a skill-only tick, an unmatched enhancer break)
correctly receive no `updated` envelope. The determinism wrinkle around
wall-clock start/stop timestamps is real but non-blocking, neutralised by
position-based symbolisation and already modelled correctly by empty_session.
This is a genuine, intended, desirable behaviour change across all eight sets,
not a laundered regression.

```text
ORACLE-RATIFICATION
range: a27cefe8ddc3e2a4bdc90ba4f0a83c81bfecfb3e..5f14023be1b4283be98461002d7d6d284aebd07b
goldens: defensive_combat_round, empty_session, enhancer_break_during_hunt, global_kill_correlated, hof_item_drop, placeholder_recorded_hunt, single_mob_hunt, skill_gain_across_tick
VERDICT: ratification-sound
```
