# Summary

[Introduction](introduction.md)

# Architecture

- [System overview](architecture/overview.md)
- [Service and crate map](architecture/service-map.md)
- [Event taxonomy](architecture/event-taxonomy.md)
- [OCR pipeline](architecture/ocr-pipeline.md)
- [Database schema reference](architecture/database-schema.md)

# Decision records

- [Architecture decision records](adr/README.md)
  - [ADR-0001: Strangler-fig Python-to-Rust port](adr/0001-strangler-fig-port.md)
  - [ADR-0002: Two-layer event spine](adr/0002-event-spine.md)
  - [ADR-0003: Injected-clock determinism seam](adr/0003-injected-clock-seam.md)
  - [ADR-0004: Test-mode composition root](adr/0004-test-mode-composition-root.md)
  - [ADR-0005: Cross-language equivalence oracle](adr/0005-cross-language-equivalence-oracle.md)
  - [ADR-0006: Tauri 2 and Svelte 5 frontend](adr/0006-tauri-svelte-frontend.md)
  - [ADR-0007: SQLite with write-ahead logging](adr/0007-sqlite-wal.md)
  - [ADR-0008: OCR equivalence frozen to the corpus](adr/0008-ocr-equivalence-frozen.md)
  - [ADR-0009: Push-to-pull invalidation](adr/0009-push-to-pull-invalidation.md)
  - [ADR-0010: Descriptive response models](adr/0010-loose-response-models.md)
  - [ADR-0011: Strong-ETag conditional requests](adr/0011-etag-conditional-requests.md)
  - [ADR-0012: Supervised worker threads](adr/0012-supervised-worker-threads.md)
