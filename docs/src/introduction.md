# EntropiaOrme architecture handbook

EntropiaOrme is an analytical desktop tool for Entropia Universe. It runs as a Tauri 2 desktop shell hosting a Svelte 5 frontend, backed by a native Rust HTTP service spine that has taken over from the original Python sidecar through a strangler-fig migration.

This handbook documents the system as it is built today. It is written for contributors and reviewers who want to understand how the pieces fit together: the process topology, the crate and service boundaries, the event spine that keeps the windows in sync, the optical-character-recognition pipeline that reads skill panels, and the database schema that backs it all. The reasoning behind these shapes is recorded separately as [architecture decision records](adr/README.md).

A companion API reference, generated from the Rust source by `cargo doc`, is published alongside this handbook.

## How this handbook is organised

- [System overview](architecture/overview.md): the process topology, the strangler proxy substrate, and the steady-state runtime behaviour.
- [Service and crate map](architecture/service-map.md): the Rust workspace crates, the services they own, and the routes served natively versus proxied.
- [Event taxonomy](architecture/event-taxonomy.md): the two-layer event system, the domain-event envelopes, and the server-sent-events delivery path.
- [OCR pipeline](architecture/ocr-pipeline.md): how a captured skill panel becomes structured skill levels.
- [Database schema reference](architecture/database-schema.md): every table, its columns, and the migration mechanism.

## Conventions

This handbook documents landed behaviour only. Where it describes a contract enforced by a test, it names the surface that enforces it. Source paths are given relative to the repository root and are shown as inline code rather than links, so they stay stable as the tree moves.
