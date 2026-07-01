# Ratification: optimiser HP per-attribute contribution removal

Adversarial review of the OpenAPI contract golden that accompanies removing the
per-attribute `hpContribution` figure from the HP optimiser. The verdict is
re-derived against the current tree rather than accepting the change author's
rationale, because a self-approved golden move carries a structural conflict of
interest.

## Change under review

The HP optimiser's `currentHp` was reconciled to the Stats panel's source of
truth: the truncated `Health` skill level. `hp_skill_optimizer`
(`eo-services/src/character_calc.rs`) now derives `current_hp` from
`skill_levels.get("Health").as_f64().unwrap_or(0.0).trunc()`, matching
`character_stats`' `Health as i64` (`eo-http/src/character_routes.rs`). This
corrected a pre-existing overshoot where `calculate_hp` counted the attributes
with the x20 profession multiplier. Once `currentHp` reads from `Health`, the
per-attribute `hpContribution` figure (still x20-inflated) no longer reconciled
with the corrected total, and there is no principled per-attribute HP
decomposition under the `Health`-skill model, so it was dropped from the
optimiser output, the OpenAPI contract, the regenerated `schema.d.ts`, the
hand-authored TS type, the Svelte chip, and the inline pins. The `levelsPerHp`
gaining-rate metric is retained.

## Oracle delta reviewed

The only committed golden that moved in the range is
`frontend/src-tauri/contracts/openapi.snapshot.json` (set key `openapi`). Its
diff is a single hunk on `HpOptimizerAttribute`: the `hpContribution` property
and its `required` entry are removed, and the description changes from "An
attribute ranked by HP contribution." to "An attribute ranked by levels per HP."
No other property, no sibling schema, and no other snapshot moved.

A full-tree search for `hpContribution` (and its casings) returns zero matches:
the field is gone from the Rust emitter, the generated and hand-authored TS
types, the Svelte view, and the inline pins, with no surviving reader. The
adjacent inline pins that moved (`native_router.rs` `currentHp 80.0 -> 0.0`;
`character_calc.rs` test pins `190.0 -> 30.0` and `214.69 -> 30.0`) are not
committed goldens under the guard, but each new value provably equals the
declared Stats source (`int(Health)`) for its inputs (Health 30 -> 30; no Health
-> 0), which is positive evidence of a real specification move rather than a
snapshotted number. `calculate_hp` is retained unchanged as the frozen
HP-equivalence reference (its own pin at `214.68828125` is untouched), so the
historical formula's equivalence coverage is preserved rather than deleted.

## Verdict

The delta is a genuine, intended specification move, not a regression pinned as
the new truth. The schema change is the complete and exact consequence of
dropping a field with no principled meaning under the reconciled HP model, with
no collateral movement. The removal is corroborated end-to-end with zero
surviving readers, and the reconciled `currentHp` equals the Stats-panel source
formula by construction rather than being a descriptively-true snapshot of
drift, which is the strongest available evidence against a swept regression.
Determinism is intact: `current_hp` is a pure `.trunc()` of a data-derived skill
level, and the snapshot is schema-derived, so no wall-clock, randomness,
environment, or timing enters the pinned output.

```text
ORACLE-RATIFICATION
range: main..HEAD
goldens: openapi (frontend/src-tauri/contracts/openapi.snapshot.json)
VERDICT: ratification-sound
```
