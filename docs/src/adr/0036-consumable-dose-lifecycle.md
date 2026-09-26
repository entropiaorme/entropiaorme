# ADR-0036: Consumable doses as one persisted lifecycle

- Status: Accepted
- Context: consumables were names in the equipment library. A stimulant
  bound to a hotbar slot resolved at no cost and with no effect, so taking a
  dose booked nothing, its reload-speed effect never reached the reload speed
  in effect, and nothing on screen said how long it had left.

## Context and problem statement

A consumable dose is a paid input with a timed effect. Its cost belongs in
cycled PED once, where it was taken; its effect changes how weapons and
healing tools are priced for as long as it lasts; and the player needs to see
it running and to take back a key pressed by mistake. The game prints no
reliable per-item line when a dose starts, and usually none when it ends, so
the dose cannot be read from the chat log.

The bundled catalogue now carries each stimulant's printed effects (name,
strength, unit, duration) and its TT value, and the medical tools that grant
a buff on every use (Eir Mk 1: reload speed +10% for eight seconds).

## Decision

- **A dose is a persisted record with an absolute expiry.** It is written as
  it starts (its item, its effects as they stood, what it booked, the session
  and context it was taken in) and read back when the tracker starts, so it
  outlives a restart and a session boundary. A dose starts from the item's
  hotbar key (the press is the dose), from a manual start on the overlay or
  the dashboard, or, for a tool with an on-use buff, from each paid heal.
- **The tracker owns the lifecycle.** Starts, re-doses, removals, restores,
  and expiries are serialised with the shots and heals they reprice. Expiry is
  a comparison with the injected clock made before every message the tracker
  handles, and a dose that ended since the last one is closed at its own
  expiry. A wake-up at the next expiry only nudges the same comparison, so
  readouts and context close on time with nothing else happening; the clock,
  never the wake-up, decides.
- **One reload evaluator.** A dose's reload-speed effect is the consumed input
  to the evaluator [ADR-0034](0034-attack-rate-limit.md) established, under
  the consumed limit and the total limit. The tracker publishes the running
  doses to one shared read model that every other pricing path (the hotbar
  resolver, Equipment, post-play review) reads under its own reading of the
  clock. When a dose starts or ends, the carried weapons and the held healer
  are re-priced at once, keeping enhancer stacks and the attribution regime.
- **The rate a charge was made at is kept.** Each stored shot and paid heal
  records the reload speed it was charged under, and review prices an
  assignment at that rate rather than today's.
- **Cost is booked once, by the player's choice.** Whether a dose books its
  cost is chosen per item in Equipment. A booked dose costs its TT value at
  the recorded acquisition markup, once, to the session running when it was
  taken, into a consumed-dose bucket beside the other consumed inputs; a
  dose outside a session, or of an item left unbooked, books nothing and
  still counts its effect. A heal's buff books nothing: the heal carries the
  cost.
- **Re-doses never stack.** A second dose of an item ends the running one
  where it starts, and starts one new expiry. Different items overlap freely.
- **Corrections supersede, never delete.** Removing a dose (a misclicked key)
  takes back its effect and anything it booked, and the dose it had ended
  runs on in its place; restoring gives everything back exactly. A heal's
  buff is removed with its heal by a healing correction and returns when the
  correction is undone.
- **Context follows timed doses.** A timed dose stands as a stacking
  consumable interval in the running session, opened for doses already
  running when a session starts, so the play it covers carries its context.
  A heal's eight-second buff opens none; it belongs to the heal's own
  provenance.
- **Every countdown is a projection.** Readouts measure remaining time from
  the stored end with a display tick, and re-read on `consumables.updated`.

## Consequences

Cycled PED gains a consumed-dose bucket on the session, its summary, the
daily rollups, the Overview, Activity, and quest analytics. Earlier sessions
booked none, and existing consumables keep booking nothing until their cost
tracking is turned on. A dose's effects are the ones the catalogue prints;
only reload speed is evaluated, and the rest are shown under their printed
names.

The re-dose rule is the conservative one: the game may refresh, extend, or
refuse a second dose of a running item, and until that is observed a re-dose
is recorded as ending the first. The anonymous effect lines the chat log
prints are not used to start or validate a dose. A post-play correction that
makes a heal a paid use does not open a buff for it retroactively; the
effect is long over by then and nothing it would reprice is still open.

See [ADR-0034](0034-attack-rate-limit.md) for the evaluator and the
attack-rate factor the doses feed, and
[ADR-0033](0033-damage-over-time-effect-windows.md) for the effect-window
lifecycle this follows.
