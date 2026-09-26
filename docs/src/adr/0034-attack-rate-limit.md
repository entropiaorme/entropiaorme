# ADR-0034: The server's attack-rate limit as a per-attack cost and damage factor

- Status: Accepted
- Context: every weapon was priced and attributed per attack at its catalogue
  figures, whatever reload speed the player had declared. The game does not
  work that way past a certain rate, so a fast weapon under reload-speed
  effects was both under-priced and checked against a damage band its real
  hits fall outside.

## Context and problem statement

The game server processes at most 100 attacks a minute. Since release 15.7.1
(15 December 2015), an attack whose reload-speed-buffed rate would exceed that
limit is still held to 100 a minute, but deals proportionally more damage, and
consumes proportionally more ammunition and decay, than the same attack at the
weapon's own rate. The release notes state both halves: the damage increase
matches what the faster rate would have achieved, and the ammunition and
deterioration grow in proportion to the added damage. Damage per PEC is
therefore unchanged; the cost of one attack and the damage it prints both grow
by one factor.

Reload speed itself is limited by where it comes from. The game caps each
effect per source category; for reload speed, equipped items can add at most
15%, consumed actions 20%, and the total is capped at 30%. The Entropia Nexus
effect catalogue publishes these limits, and its figures for the same system's
critical-hit limits match the ones MindArk's own release 15.16.0 set. The
catalogue also publishes each weapon's base rate (`UsesPerMinute`), which the
bundled snapshot now carries.

Without this model the app summed every declared reload-speed source without a
limit, and ignored rate entirely when pricing a weapon.

## Decision

- **One factor, both consumers.** A weapon's attack-rate factor is its base
  rate times the reload multiplier in effect, over 100, or 1 at and below the
  limit. The cost engine scales every line of the per-attack cost by it, and
  the damage profile a weapon's attribution band derives from scales by the
  same factor, so cost and band can never disagree about the rate.
- **Read-time derivation, never stored.** A weapon's props are enriched on
  read with the catalogue base rate and the factor under the reload speed in
  effect at that moment. Stored equipment rows never carry the factor; a
  change of declared effects reprices every weapon from then on, and historical
  shot costs, already booked, do not move.
- **Every pricing path goes through the same preparation.** The live tracker's
  weapon profile and carried weapons, the hotbar's weapon cost, post-play review
  assignments, and the Equipment library and detail all price a weapon through
  it. A weapon the catalogue gives no rate for keeps a factor of 1.
- **The game's stacking limit applies to declarations.** Declared persistent
  effects are equipped items, so their summed reload speed is held at the
  item limit before it reaches anything: the attack rate and the healing
  reload alike. Declared slowing applies after the limit. The declared sum
  and the figure in effect are both shown, so the limit is disclosed where the
  sources are edited.
- **Declared damage-over-time bands are not scaled.** A weapon with a declared
  effect profile is checked against the ranges the player read off the game,
  which already include whatever the rate did to them.

## Consequences

A weapon held at the limit shows its rate, the rate it would reach, and the
factor on its Equipment detail, and every per-attack figure (the breakdown, the
total, the damage range, the chart the carried weapons are compared on)
includes the factor. A buffed weapon's full-strength hits fit its band instead
of reading as another weapon's evidence or as out of profile.

The factor applies to catalogue base rates above 100 without any buff too,
which only a handful of novelty weapons publish; their catalogue damage is read
as the per-attack figure at their own rate, as for every other weapon. Consumed
reload-speed doses do not yet feed the evaluator, so only the item limit can
bind today; the consumed-action and total limits apply once a dose lifecycle
records them. The healing tools' own rate is left to their reload and cooldown
model: the release notes describe attacks.

See [ADR-0032](0032-unified-weapon-attribution.md) for the damage bands this
scales and [ADR-0033](0033-damage-over-time-effect-windows.md) for the declared
effect bands it leaves alone.
