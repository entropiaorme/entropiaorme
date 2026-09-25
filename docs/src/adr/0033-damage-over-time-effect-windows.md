# ADR-0033: Damage-over-time effects as persisted windows of one paid activation

- Status: Accepted
- Context: ADR-0032 left an effect-window seam in weapon attribution that
  nothing opened. A weapon whose cast keeps dealing damage (an Electrocution
  chip, for one) prints a damage line per tick for some seconds after the cast,
  and the tracker priced every one of those lines as a shot of whichever
  weapon was in hand.

## Context and problem statement

A real rotation (one Electrocution cast, then a switch to the primary chip
while the effect ticked) recorded 35.15 PED against a true 18.80 PED (one cast
and 38 primary shots): its 21 ticks were charged as shots, two at the chip's
price and nineteen at the primary's, nearly doubling the cost. The recording
also dropped one missed primary shot; a miss is now a countered shot, like a
jam, dodge, or evade. The ticks are outcomes of the one paid
cast. Charging them is an invented cost; dropping them would lose real damage.

The bundled catalogue carries no duration, cadence, or tick figure for such a
weapon, and its single damage number does not reconcile with what the game
prints. So the effect cannot be derived from game data. And a tick is not an
alternative to the weapon in hand, the way a second weapon is: it is a second
source printing lines beside it.

## Decision

A weapon may declare its damage-over-time effect in Equipment: an initial hit
range (or none, when the first tick is the cast's outcome), the effect's
duration, its tick range, and optionally its cadence. The declared cast range
replaces the catalogue figure as the band attribution checks the weapon's
shots against.

- **One paid activation, one window.** A priced hit of a weapon with a declared
  effect (a jam, dodge, evade, or miss starts nothing) opens a window at the hit's
  observation time, through the injected clock, for the effect's duration. The
  window goes into attribution at once and is written straight after, keeping
  the activation's provenance (the weapon, the hit, the price it was booked at,
  its context) and the profile as declared then.
- **Ticks cost nothing.** A hit an open window's tick range holds is an effect
  tick: no shot, no ammunition, no decay, no cost. It counts in the kill's
  damage, stamps the context it lands in, and is stored with the window it
  belongs to. The activation's cost stays with its shot.
- **Concurrency is ambiguity, not intent.** A hit that both the declared
  weapon's band and another weapon's open effect explain stays unresolved:
  recorded, counted, disclosed, and correctable after play, but not priced and
  not waved through. The same holds for the previous weapon's shot still
  landing after a switch. A tick of the weapon in hand's own effect is a tick.
  A hit that fits one other weapon's band but also an open effect is a tick,
  never a missed switch.
- **Short of every band, the nearest floor explains it.** Mob armour and a
  killing blow only lower a printed figure. A hit below the declared weapon's
  band and below an open effect's tick range belongs to whichever floor sits
  nearest above it: the effect's, when that is lower.
- **Overlapping windows keep every candidate.** A tick several open windows
  explain names none of them; each stays a candidate in its provenance. No
  window is chosen by the order it opened in.
- **Windows outlive their session.** Every session start reads back each window
  whose absolute expiry is still ahead, whichever session paid for it, so a
  restart or a new session mid-effect keeps treating its ticks as ticks. Expiry
  is a comparison with the clock, never a timer.
- **Keeping the hotbar's weapon takes a cast back.** A window opened by a hit
  that damage evidence named (a missed switch to the effect weapon) is
  withdrawn when the player keeps the hotbar's weapon over that evidence: the
  hit is repriced to the kept weapon and the window explains no later tick.
  It stays as provenance. A confirm opens no window retroactively; the
  evidence hit that raised the mismatch already opened one.
- **Corrections stay honest and undoable.** After play, a stored tick can be
  priced as a paid shot of a carried weapon, and an unresolved hit can be
  marked as a tick of an effect that was open when it landed. Either moves the
  shot in or out of its kill's phases and shot count, repairs every derived
  figure in one transaction, and undoes exactly.

## Consequences

A DoT rotation bills its cast once and its primary weapon normally; its ticks
are visible as the cast's outcomes in session detail, with the damage they did.
The replay of the captured rotation is pinned in the corpus
(`dot_weapon_rotation`): one cast, 38 primary shots, 21 ticks, 18.80 PED.

The effect's figures are the player's declaration, fitted to what the game
prints. A tick range drawn too wide can hold a genuine primary hit, and one
drawn too narrow lets a tick fall to the weapon in hand, but only where the
ranges meet; Equipment's damage ranges chart shows where an effect's ticks
overlap another weapon's hits. Window cadence is recorded but never used to
reject a tick, because the chat log can deliver a burst of lines at once.

See [ADR-0032](0032-unified-weapon-attribution.md) for the attribution engine
this extends, [ADR-0027](0027-intent-led-healing-attribution.md) for the
healing effect-window precedent, and the
[database schema reference](../architecture/database-schema.md) for the window
and correction model.
