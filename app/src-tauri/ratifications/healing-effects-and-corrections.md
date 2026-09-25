# Ratification: healing effects and corrections

Adversarial review of the golden movement that comes with carrying healing
effect windows across restarts, correcting healing evidence after play, the
new `healing.updated` domain event, and the healing tables in the database
snapshot catalogue.

## Findings

The golden movement is ratified as a genuine, intended behaviour change.

The event schema snapshot gains a closed `healing.updated` envelope and an
empty payload. They exactly mirror the native type and the existing
protection event, and no existing definition changes. The demo session
detail gains only the healing `correctable` flag, true for its closed
session.

Every existing database snapshot gains the three healing evidence tables.
They are appended at the end of the snapshot catalogue, so no existing symbol
can be renumbered, and no line is removed from any golden. They are empty
everywhere except the defensive combat scenario. There, a self-heal the
tracker has always recorded, with no paid healer, now shows as one
unattributed output at zero cost. Unattributed is correct because passive
correlation applies only to damage dealt. Scenarios without a step script
replay in exactly the previous order, and their fingerprints are
byte-identical.

The new healing rotation scenario was checked line by line against the
intended contract, since it has no prior baseline:

- The restoration chip and the first-aid pack each bill exactly once.
- Every later tick is a zero-cost output of the restoration's single effect
  window.
- A tick after the segment change is stamped with the new context, while the
  cost stays where it was paid.
- After a crash, the orphaned session closes at its latest heal, and a tick
  in the new session is explained by the persisted window, which carries the
  original activation.
- The effect's expiry at twenty seconds ends attribution.
- A first-aid pack retry inside its reload bills nothing.
- Session heal costs are 0.07 and 0.03.

The fingerprint carries five hotbar intents, two session starts and one stop.
Session updates fire once per heal and once per actual tool change, not on
the repeated press. Every time comes from the injected clock or a domain
timestamp. The replay was reproduced under three host timezones and in five
repeat runs, and the corpus replay oracle passed 17 of 17.

Remaining coverage limits, none of which affects correctness:

- Corrections are not captured by the snapshot catalogue; the service's own
  tests pin them.
- Reload enforcement across a restart is pinned by the tracker's unit tests
  rather than by this scenario.

```text
ORACLE-RATIFICATION
range: 67f403d..HEAD
goldens: event_schemas, tracking_session_detail, placeholder_recorded_hunt, basic_hunt_10_events, crit_dodge_evade_jam, defensive_combat_round, empty_session, enhancer_break_during_hunt, global_item_drop, global_kill_correlated, healing_effect_rotation, hof_item_drop, hof_kill_correlated, mission_completion_with_reward_suppression, multi_mob_hunt_loot_grouping, single_mob_hunt, skill_gain_across_tick, tree_harvesting_session
VERDICT: ratification-sound
```
