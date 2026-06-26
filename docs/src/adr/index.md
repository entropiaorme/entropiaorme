# Architecture decision records

This section collects the significant architectural decisions behind EntropiaOrme, recorded as short [Markdown Architecture Decision Records](https://adr.github.io/madr/). Each record captures the context that forced a decision, the decision itself, and the consequences that followed.

Every record here describes a decision that has landed in the codebase, so each carries the status **Accepted**. New decisions are added as new numbered records rather than by rewriting old ones; a decision that is later reversed is superseded by a new record that references it.

| ADR | Decision |
| --- | --- |
| [ADR-0001](0001-strangler-fig-port.md) | Strangler-fig Python-to-Rust backend port (superseded by ADR-0013) |
| [ADR-0002](0002-event-spine.md) | Two-layer event spine: an in-process bus plus typed domain envelopes |
| [ADR-0003](0003-injected-clock-seam.md) | An injected clock as the determinism seam for replay |
| [ADR-0004](0004-test-mode-composition-root.md) | A separate test-mode composition root |
| [ADR-0005](0005-cross-language-equivalence-oracle.md) | A hybrid cross-language equivalence oracle for the port |
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
