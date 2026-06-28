# ADR-0016: Retire the cross-language equivalence oracle

- Status: Accepted
- Context: the Python-to-Rust port is complete and shipped ([ADR-0013](0013-in-process-collapse.md): a single in-process Rust binary), and the cross-language equivalence oracle of [ADR-0005](0005-cross-language-equivalence-oracle.md), retained after the port as a test-only reference implementation, has served its purpose and is now removed. This record supersedes ADR-0005.

## Context and problem statement

The backend was ported from Python to Rust one service at a time, graded against a cross-language equivalence oracle ([ADR-0005](0005-cross-language-equivalence-oracle.md)): the original Python implementation, retained in-repo as a test-only reference, with a shared normaliser reimplemented byte-for-byte on each side and a live differential that ran the Rust engines against the Python originals over a generated input domain. The port landed and the runtime collapsed to a single in-process Rust binary ([ADR-0013](0013-in-process-collapse.md)); the Python tree shipped with nothing, retained only as that oracle.

Once the port was complete the oracle became a depreciating asset. Every Rust-only feature added afterwards needed no Python counterpart, so the reference implementation no longer grew with the product, while the cost of carrying it persisted: a second language in the tree, a second dependency set to audit, the `setup-python` continuous-integration steps, and the dual maintenance of any logic that did still straddle both sides. An external reader cloning the repository saw a large Python tree and Python continuous-integration jobs beside a project whose shipped artefact is a single Rust binary. The question this record answers is how to stand the codebase down to one language without discarding the equivalence evidence the oracle was built to produce.

## Decision

The Python oracle is retired. The reference implementation, the live differential, the cross-language conformance suites, the Python-generated goldens' generators, and the Python continuous-integration toolchain are removed; the equivalence **evidence** is preserved as frozen, committed, Rust-side assertions.

The distinction is the whole of the decision. The oracle produced two things: a *live* differential (the Rust engines compared against a running Python reference over a generated input domain) and a set of *frozen goldens* (byte-pinned observable output: the event-stream fingerprints, the database-state snapshots, the OpenAPI and event-schema contract snapshots, the normaliser conformance table, and the recorded-corpus replay goldens). Only the live differential needed the interpreter. The frozen goldens are bytes, and the Rust side already reimplements the normaliser and every emitter that produces them, so a hermetic Rust test reproduces each golden with no second implementation present. Retirement keeps every frozen golden and the hermetic tests that assert them, and drops only the live differential and the Python that backed it.

Concretely:

- The committed goldens move out of the retired tree into the Rust workspace: the replay corpus under `frontend/src-tauri/fixtures/corpus/`, the ratified contract snapshots under `frontend/src-tauri/contracts/`, and the normaliser and listener-family fixtures under `frontend/src-tauri/eo-wire/tests/fixtures/`. The hermetic tests that assert them (`corpus_replay_oracle.rs`, `emitters_proof.rs`, `conformance.rs`, `yml_family.rs`, `openapi_conformance.rs`, `event_schema_conformance.rs`) are repointed and continue to pass byte-for-byte with no Python.
- The handful of behaviours that the live differential alone exercised, with no frozen golden, were captured as committed Rust-side assertions before the differential was removed, so the proof of each survives its retirement.
- The `cross-language` Cargo feature and the differential test files are removed.
- The dormant HTTP-transport scaffolding the oracle's battery was pinned against goes with it: the never-called socket-serving entry point, and the environment-reading allowlist construction that only an explicit-Host or explicit-Origin request could ever reach. Production dispatches in-process through the same router and guard stack, which is unchanged (the origin and Host guard, the CORS contract, and the live event hub all remain and are still asserted).
- The continuous-integration guards that were Python (the golden-ratification guard, the authoring lint, the version-stamp parity check, the change-scope classifier, and the mutation-floor enforcement) are reimplemented as a Rust `cargo xtask` and a shell classifier, so no workflow installs a Python interpreter. Mutation testing moves from the Python engine to `cargo-mutants` over the Rust workspace members, with the aggregate score published as the repository's mutation badge.

## Consequences

The repository is single-language: the shipped artefact, the tests, and the continuous-integration tooling are all Rust (plus the Svelte frontend and a little shell). The headline migration claim is now sayable without qualification.

The equivalence rigour story is unchanged in substance and stronger in independence from a live interpreter. The frozen goldens still pin observable behaviour byte-for-byte, the golden-ratification discipline still forces any change to a golden through a recorded, reviewable verdict (now enforced by the `cargo xtask` guard against the relocated paths and the reports under `frontend/src-tauri/ratifications/`), and the mutation campaign still measures whether the assertions have holes. What is gone is the ability to re-derive a golden by running a second implementation: a future change to a pinned behaviour is now graded against the committed golden, which is exactly the regression-oracle contract the goldens always carried.

The cost accepted is that the live, generated-input differential no longer runs. That coverage was random-input exploration over the normaliser and a few numeric engines; it is inherently not freezable (a fixed assertion cannot stand in for an unbounded input domain), and the fixed hazards it targeted are pinned by the conformance tables and unit tests that remain. The reference implementation itself is preserved in version history, so a future revisit has it to read.

See [ADR-0005](0005-cross-language-equivalence-oracle.md) for the oracle this concludes, [ADR-0001](0001-strangler-fig-port.md) and [ADR-0013](0013-in-process-collapse.md) for the port and the collapse that preceded it, [ADR-0008](0008-ocr-equivalence-frozen.md) for the OCR corpus equivalence (which was always asserted on the Rust path and is untouched), and the [ADR index](index.md).

## Evidence

- `frontend/src-tauri/eo-services/tests/corpus_replay_oracle.rs`
- `frontend/src-tauri/eo-wire/tests/conformance.rs`
- `frontend/src-tauri/eo-wire/tests/emitters_proof.rs`
- `frontend/src-tauri/contracts/openapi.snapshot.json`
- `frontend/src-tauri/xtask/`
- `frontend/src-tauri/ratifications/`
