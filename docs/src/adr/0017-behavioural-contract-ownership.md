# ADR-0017: Own the behavioural contract in this codebase

- Status: Accepted; its enforcement is amended by [ADR-0037](0037-retire-golden-ratification-guard.md), which retires the ratification guard, and its contract snapshots and HTTP-response goldens are retired by [ADR-0039](0039-retire-port-equivalence-evidence.md)
- Context: the cross-language equivalence oracle is retired ([ADR-0016](0016-retire-equivalence-oracle.md), which superseded [ADR-0005](0005-cross-language-equivalence-oracle.md)), and the equivalence evidence survives as frozen, hermetically asserted Rust-side goldens. Those goldens have until now doubled as a byte-fidelity pin to the retired reference implementation. This record redefines what they are pinned to, without changing how the pin is enforced.

## Context and problem statement

The backend was ported from a reference implementation one service at a time, graded against a behavioural-equivalence oracle ([ADR-0005](0005-cross-language-equivalence-oracle.md)): the reference produced a fixed, normalised set of observable outputs for each scenario, pinned as golden files, and the native engines were graded against them byte-for-byte. That grading pinned everything the reference emitted, indiscriminately: the intended behaviour, and also the incidental ways the reference happened to represent a value. Floating-point formatting, the shape of the validation-error envelope it returned, and specific exception-message texts were all captured as golden bytes with equal authority, because at the crossing the only question that mattered was whether the native output matched.

The oracle is now retired ([ADR-0016](0016-retire-equivalence-oracle.md)). The frozen goldens and the hermetic tests that re-assert them remain, and they still pin those exact bytes. This leaves an ambiguity about what the goldens mean going forward. Some of their bytes record a behaviour this project chose; others record only how the reference implementation happened to represent something. When later work deliberately changes one of the second kind (returning a typed error in place of a stringly one, dropping a transport envelope the in-process collapse made vestigial, or normalising a timestamp representation), the golden diff is indistinguishable, on its face, from a regression being ratified as the new truth. Without a recorded contract shift, every such change has to argue its own legitimacy from scratch, and the safe reading of any golden move is "fidelity to the reference has been broken".

## Decision

The behavioural contract is owned by this codebase. The goldens pin **this project's own ratified contract**, not fidelity to the retired reference implementation. Byte-fidelity to that reference is no longer, in itself, a constraint on the goldens.

The enforcement mechanism is unchanged. Every change to a golden still passes through the golden-ratification guard: a required continuous-integration check that refuses a golden move unless the same commit range carries a recorded adversarial-ratification verdict, in a fenced block whose verdict is sound and whose declared golden sets cover the change. A golden still cannot move silently, and an accidental, unratified diff still fails the build exactly as before.

What changes is the standard the ratification verdict is judged against. Where a golden's bytes encode an artefact of the reference implementation (a representation detail, a transport-envelope shape, an error-message text) rather than an intended behaviour of this application, changing them is a legitimate, ratifiable decision, not a regression to be resisted. The reviewer's question is no longer "does this still match the reference?" but "is this delta a genuine intended behaviour of the product, or a regression laundered into the goldens as the new correct?". The equivalence evidence banked at the crossing is unaffected: it stated what was proven equal at the moment of the port, and it still does.

## Consequences

A deliberate behaviour or representation change now proceeds by regenerating the affected goldens and carrying them through the ratification guard, with the recorded verdict naming the intent. This is the path the guard was always built to police; this record removes the unstated presumption that a golden move is illegitimate because it departs from the reference.

The guard does not weaken. It remains a required check, and accidental drift is caught precisely as before: an unratified golden move fails, and a first-generated golden with no prior to diff against still demands the same scrutiny (the most dangerous case, since nothing fails to force the review). What shifted is what the goldens are pinned to, not how firmly they are held.

The frozen equivalence evidence remains in version history as the record of what was proven at the crossing, and the reference implementation itself remains in history for a future revisit to read. This record reframes the goldens' forward role, from a byte-fidelity pin to the retired reference into a regression oracle for the contract this codebase now owns; it does not disturb the proof that was banked.

See [ADR-0016](0016-retire-equivalence-oracle.md) for the retirement this builds on, [ADR-0005](0005-cross-language-equivalence-oracle.md) for the oracle whose goldens these were, [ADR-0013](0013-in-process-collapse.md) for the in-process collapse that made some transport bytes vestigial, and the [ADR index](index.md).

## Evidence

- `frontend/src-tauri/xtask/`
- `frontend/src-tauri/ratifications/`
- `frontend/src-tauri/contracts/`
- `frontend/src-tauri/fixtures/corpus/`
