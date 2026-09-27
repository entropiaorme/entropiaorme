# EntropiaOrme architecture handbook

EntropiaOrme is an analytical desktop tool for Entropia Universe. It runs as a Tauri 2 desktop shell hosting a Svelte 5 frontend, backed by a pure-Rust in-process backend the frontend reaches over typed IPC commands. The backend began as a Python sidecar, was ported to Rust and collapsed into the single binary, and the in-process HTTP transport that collapse left behind was itself retired in favour of the typed commands; the cross-language equivalence oracle that graded the port has been retired too, its evidence preserved as frozen Rust-side goldens ([ADR-0001](adr/0001-strangler-fig-port.md), [ADR-0013](adr/0013-in-process-collapse.md), [ADR-0016](adr/0016-retire-equivalence-oracle.md), [ADR-0019](adr/0019-typed-command-facade.md)).

This handbook documents the system as it is built today. It is written for contributors and reviewers who want to understand how the pieces fit together: the process topology, the crate and service boundaries, the event spine that keeps the windows in sync, the optical-character-recognition pipeline that reads skill panels, and the database schema that backs it all. The reasoning behind these shapes is recorded separately as [architecture decision records](adr/index.md).

A companion API reference, generated from the Rust source by `cargo doc`, is published alongside this handbook.

## How this handbook is organised

- [System overview](architecture/overview.md): the process topology, the in-process dispatch substrate, and the steady-state runtime behaviour.
- [Service and crate map](architecture/service-map.md): the Rust workspace crates, the services they own, and the operations the typed-command facade exposes.
- [Event taxonomy](architecture/event-taxonomy.md): the two-layer event system, the domain-event envelopes, and the in-process event delivery path.
- [OCR pipeline](architecture/ocr-pipeline.md): how a captured skill panel becomes structured skill levels.
- [Database schema](architecture/database-schema.md): storage, the data model by domain, and the migration mechanism.

## Conventions

This handbook documents landed behaviour only. Where it describes a contract enforced by a test, it names the surface that enforces it. Source paths are given relative to the repository root and are shown as inline code rather than links, so they stay stable as the tree moves.
