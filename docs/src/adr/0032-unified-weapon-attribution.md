# ADR-0032: Unified weapon attribution: hotbar intent validated by damage bands

- Status: Accepted
- Context: weapon cost followed one of two mutually exclusive modes. With the
  hotbar key listener on, every shot was priced to the last weapon pressed;
  with it off, a preset of a small weapon, a big weapon, and a healer priced
  each hit to whichever of the two weapons' non-overlapping damage bands
  contained it.

## Context and problem statement

Each mode failed where the other was strong. Hotbar attribution trusted a press
absolutely: a weapon switched from the inventory, or a missed key, priced every
later shot to the wrong weapon until the next press. Band attribution needed
exactly two weapons with disjoint bands, guessed between overlapping bands, and
could not tell which weapon was in hand when two could explain a hit. The
player had to choose a mode, and the choice decided which failure they lived
with.

Weapon cost is liquid PED accounting. A shot priced to the wrong weapon is a
wrong cost, and a shot priced by guesswork is an invented one. The two signals
are also complementary: a press is the player's declaration of intent, and a
hit's printed damage is observed evidence of what fired it.

## Decision

One engine, no modes. The hotbar declares the weapon in hand; each carried
weapon's damage band validates the declaration.

- The carried weapons are every weapon bound to a hotbar slot plus those the
  player carries without a hotkey. Each band is the weapon's catalogue damage
  with its amplifier and damage enhancers, half to all of the total at full
  skill; a critical may reach three times the maximum. Printed damage is
  matched with half a printed step of tolerance.
- Every offensive observation resolves to one of four recorded states. It
  **agrees** when the declared weapon explains it: its band fits, it has no
  band to contradict, the hit falls short of its band and no other weapon
  explains it (mob armour and sub-maximal skill only ever lower a hit), or the
  previous weapon's shot lands within the delivery tail after a switch. It is
  an **effect tick** when an open window of an earlier paid activation explains
  it: no shot and no cost. It is **evidence** when exactly one other carried
  weapon explains it, or when nothing is declared at all: it is priced to that
  weapon. Otherwise it is **unresolved**: recorded and counted, but never
  priced.
- Evidence may only override belief inside the current intent regime, which a
  weapon or harvesting-tool press, a decision, or the session boundary starts.
  Evidence against a declared weapon raises a mismatch the overlay shows as the
  weapon being recorded, with one decision: **confirm** makes the evidence
  weapon the declared one and reprices the regime's shots it plausibly fired
  (back to the last hit only the declared weapon explains); **keep** reprices
  the evidence shots back to the declared weapon and stops that weapon's
  evidence overriding it for the rest of the regime. A decision moves memory
  first against a backup, then writes the decision, the repriced kills, and the
  moved evidence in one transaction; a failed write restores the backup.
- Shots worth keeping individually (evidence against a declared weapon,
  unresolved shots, effect ticks) are stored with the kill they settle into, so
  a row exists exactly when its shot's cost does. An ended session's unresolved
  shots can be assigned to a weapon carried at the time, priced from the weapon
  as configured then, exactly undoable.
- Every surface built on a session cost discloses unpriced shots rather than
  presenting an incomplete figure as a total.

The trifecta preset, its selector, its validation, and the duplicate healer
resolver are retired. Stored presets remain on disk untouched; a configuration
written before carried weapons existed seeds them once from the preset that was
in force, so a player who used band attribution keeps those weapons as
candidates. Healing billing is unchanged (ADR-0027): the healing profile is the
healer's guardrail, and healing still bills only from hotbar intent.

## Consequences

There is no attribution mode to choose. With the listener on, a missed switch
is caught and costed to the weapon that fired, and the player confirms or
corrects it without leaving the game; with it off, the bands attribute alone,
now across any number of carried weapons, and overlap becomes an honest
unresolved state instead of a narrowest-band guess.

Conservative ambiguity can leave a real shot unpriced, but cannot price it to a
guessed weapon. Every such shot stays visible, counted in the session's shots,
disclosed on the cost it is missing from, and assignable after play. Timing
feasibility beyond the delivery tail is not yet available because the bundled
weapon data carries no reload or attack-speed figure; the seam exists, and
damage-over-time weapons fill the effect-window seam with their own profiles.

See [ADR-0027](0027-intent-led-healing-attribution.md) for the intent model this
extends, the [event taxonomy](../architecture/event-taxonomy.md) for the intent
path, and the [database schema reference](../architecture/database-schema.md)
for the evidence model.
