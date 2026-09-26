# Architecture decision records

This section collects the significant architectural decisions behind EntropiaOrme, recorded as short [Markdown Architecture Decision Records](https://adr.github.io/madr/). Each record captures the context that forced a decision, the decision itself, and the consequences that followed.

Every record here describes a decision that has landed in the codebase, so each carries the status **Accepted**. New decisions are added as new numbered records rather than by rewriting old ones; a decision that is later reversed is superseded by a new record that references it.

Several of the earlier records predate two later structural decisions: the collapse to a single in-process Rust binary (ADR-0013) and the retirement of the cross-language equivalence oracle (ADR-0016). They cite the Python implementation in which their decision was first realised. Unless a later record supersedes it, each decision remains in force in the native Rust workspace; the architecture handbook chapters describe the current implementation, while these records preserve the reasoning as it was set down.

| ADR | Decision |
| --- | --- |
| [ADR-0001](0001-strangler-fig-port.md) | Strangler-fig Python-to-Rust backend port (superseded by ADR-0013) |
| [ADR-0002](0002-event-spine.md) | Two-layer event spine: an in-process bus plus typed domain envelopes |
| [ADR-0003](0003-injected-clock-seam.md) | An injected clock as the determinism seam for replay |
| [ADR-0004](0004-test-mode-composition-root.md) | A separate test-mode composition root |
| [ADR-0005](0005-cross-language-equivalence-oracle.md) | A hybrid cross-language equivalence oracle for the port (superseded by ADR-0016) |
| [ADR-0006](0006-tauri-svelte-frontend.md) | Tauri 2 and Svelte 5 runes for the desktop frontend |
| [ADR-0007](0007-sqlite-wal.md) | SQLite with write-ahead logging for local storage |
| [ADR-0008](0008-ocr-equivalence-frozen.md) | OCR behaviour frozen to the recorded corpus |
| [ADR-0009](0009-push-to-pull-invalidation.md) | Push-to-pull invalidation for window synchronisation |
| [ADR-0010](0010-loose-response-models.md) | Descriptive read models, closed event envelopes |
| [ADR-0011](0011-etag-conditional-requests.md) | Strong-ETag conditional requests on hydration reads |
| [ADR-0012](0012-supervised-worker-threads.md) | Named, owned, supervised worker threads |
| [ADR-0013](0013-in-process-collapse.md) | Collapse to a single in-process Rust binary |
| [ADR-0014](0014-release-engineering.md) | Bespoke installer, signed auto-update, and a provenance-bearing release pipeline |
| [ADR-0015](0015-candle-ocr-backend-not-adopted.md) | Native candle OCR backend evaluated and not adopted; ONNX Runtime kept as the sole recogniser |
| [ADR-0016](0016-retire-equivalence-oracle.md) | Retire the cross-language equivalence oracle; preserve the evidence as frozen Rust-side goldens |
| [ADR-0017](0017-behavioural-contract-ownership.md) | Own the behavioural contract in this codebase; the goldens pin its ratified contract, not reference fidelity |
| [ADR-0018](0018-daily-rollup-read-model.md) | Daily-rollup read model: the Overview aggregates a rebuildable per-day projection, O(days) not O(rows) |
| [ADR-0019](0019-typed-command-facade.md) | Typed IPC commands over a service facade; TypeScript generated from the Rust types; the HTTP transport retires family by family |
| [ADR-0020](0020-tracker-actor-refounding.md) | The tracker as a single-owner actor with a typestate session, named seams, and an instant time basis |
| [ADR-0021](0021-synchronous-database-core.md) | A synchronous database core on dedicated threads, adopted against a measured null result |
| [ADR-0022](0022-runes-native-shared-state.md) | Runes-native shared state: the svelte/store surface is frozen behind a whole-tree guard and only shrinks |
| [ADR-0023](0023-linux-platform-layer.md) | The Linux platform layer: XWayland windowing, evdev key observation, ScreenCast-portal capture, and deb/AppImage packaging |
| [ADR-0024](0024-market-informational-layer.md) | Estimated market data as a quarantined informational layer; the accounting surfaces can never read it, CI-enforced |
| [ADR-0025](0025-central-market-data-service.md) | A central market-data service on AWS serverless: token-authenticated ingest, scheduled aggregation, versioned snapshot distribution |
| [ADR-0026](0026-canonical-quest-reward-accounting.md) | Canonical quest reward accounting and session-owned quest rosters |
| [ADR-0027](0027-intent-led-healing-attribution.md) | Intent-led healing attribution with durable output evidence |
| [ADR-0028](0028-versioned-expected-hunting-economics.md) | Versioned expected-hunting economics over immutable offensive evidence |
| [ADR-0029](0029-two-line-development.md) | Two-line development: `next` integrates, `main` is promoted to after soak; the merge queue retired |
| [ADR-0030](0030-startup-readiness-boundary.md) | An explicit startup readiness boundary: a level-triggered shell answer, commands held in the typed transport, the frame painted first |
| [ADR-0031](0031-session-grain-protection-costs.md) | Protection costs recorded at session grain, when the player repairs: nothing declared during play, hit-weighted over the sessions the player ticks |
| [ADR-0032](0032-unified-weapon-attribution.md) | Unified weapon attribution: hotbar intent validated by each carried weapon's damage band, with bounded, player-decided correction and no mode to choose |
| [ADR-0033](0033-damage-over-time-effect-windows.md) | Damage-over-time effects as persisted windows of one paid activation: ticks cost nothing, concurrent explanations stay unresolved, and windows outlive their session |
| [ADR-0034](0034-attack-rate-limit.md) | The server's attack-rate limit as a per-attack cost and damage factor, applied at read time through one pricing preparation; reload speed held at the game's per-source stacking limits |
