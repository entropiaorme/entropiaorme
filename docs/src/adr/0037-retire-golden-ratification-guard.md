# ADR-0037: Retire the golden-ratification guard

- Status: Accepted
- Context: amends the enforcement described in [ADR-0017](0017-behavioural-contract-ownership.md) and [ADR-0016](0016-retire-equivalence-oracle.md). EntropiaOrme is a personal application built in public, maintained by one developer, with no second reviewer for a recorded verdict to stand in for.

## Context and problem statement

A committed golden could move only when the same commit range carried a `test: regenerate goldens` commit marker and a recorded adversarial-ratification report whose verdict named every changed golden set. A CI guard (`cargo xtask ratify-check`) enforced both. The protocol was built when the goldens carried a cross-language equivalence proof, and when the application was run as a product for other people.

In practice the guard cost more than it caught. Most golden moves are additive (a new DTO field appearing in a snapshot), and each one needed a separate report. The report could only be presence-checked, never proven, so a skipped report left the integration line red without any regression having occurred.

## Decision

Retire the guard, the commit marker it required, and the ratification report as a precondition for moving a golden.

- The goldens stay: the replay corpus, the contract snapshots, and the wire fixtures still assert by default, so an unintended change still fails a test.
- A deliberate change regenerates the affected goldens, and the diff is reviewed in the commit that moves them. That commit's message says which sets moved and why.
- The reports already under `app/src-tauri/ratifications/` stay as history.

## Consequences

Changing a golden becomes an ordinary reviewed change instead of a gated ceremony, and the integration line no longer goes red over a missing report. The risk the guard addressed, a regression being regenerated into the expected output, is now held by the review of the diff alone. The first generation of a new golden still deserves the closest look, because no prior golden fails to force it.

## Evidence

- `.github/workflows/ci.yml` (the guard job and its CI-gate input removed)
- `app/src-tauri/xtask/src/main.rs` (the `ratify-check` subcommand removed)
- `TESTING.md` ("Goldens regeneration")
