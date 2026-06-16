# ADR-0014: an optional candle OCR backend

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The recogniser (the full pipeline is described in the [OCR pipeline](../architecture/ocr-pipeline.md) page) runs the SVTRv2-mobile model through ONNX Runtime: the Python sidecar drove it via `onnxruntime`, and the native port drives the same model file through the `ort` crate, with the OpenCV-exact preprocess and CTC decode reimplemented around it. That engine is the default and is correct; [ADR-0008](0008-ocr-equivalence-frozen.md) pins it to the recorded ground-truth corpus.

ONNX Runtime is a C++ library bound through `load-dynamic`: the recogniser depends on a native runtime shipped beside the binary (and on a DirectX 12 host, on the DirectML execution provider). The question this ADR settles is whether to also run the model in Rust through candle, with no native inference runtime. The ONNX Runtime engine already meets the product requirement, so any second backend is an optional alternative, kept off the default path, not a replacement.

## Decision

Carry a second OCR backend that runs the SVTRv2 forward pass from scratch on candle (a Rust ML framework), behind a Cargo feature (`candle`) that is **off by default**. A normal build neither compiles nor links it; the soaked default recognition path is the ONNX Runtime engine, unchanged.

The two backends share one seam. The inference step is factored behind an `InferenceBackend` trait in `ocr_engine.rs` (a preprocessed CHW tensor in, CTC logits out); the ONNX Runtime path moved behind it byte-for-byte, and the candle path implements the same contract. Everything around inference (the PNG decode, the OpenCV fixed-point preprocess, the CTC decode and dictionary) is shared code both backends run, so the second backend cannot drift on those stages by construction.

The candle backend (`ocr_candle.rs`) reimplements the network: the LCNet convolutional encoder (a two-conv stem, thirteen RepMixer blocks with squeeze-excite on a subset and two height-halving downsample blocks), the SVTR sequence neck (height pooled to one row, two global multi-head-attention transformer blocks, a convolutional re-fusion), and the CTC linear head. Its weights come from a safetensors export of the same bundled ONNX model, produced by a committed, reproducible script (`backend/scripts/convert_svtrv2_to_safetensors.py`) that lifts the ONNX initialisers, transposes the linear weights into candle's layout, and writes `backend/assets/models/svtrv2_rec.safetensors` beside the ONNX.

The backend is held to **reproduce** ONNX Runtime, exactly as the port itself is. A comparison harness (`ocr_backend_comparison.rs`) checks it tensor-to-tensor on a single cell (cosine 1.000000, max-abs ~6e-5, every timestep's argmax identical) and over the full 594-cell corpus (identical 262/594 raw-exact, 100% per-cell text agreement).

## Consequences

The benefit is a contained, faithful reimplementation of the recogniser in Rust: reproduced to floating-point precision against the C++ runtime over real ground-truth data, with the reproduction proven rather than asserted. Because it is faithful, it does not fork the recogniser's behaviour, and because it is default-off, it carries no risk to the shipped product.

The cost is latency. candle's CPU kernels have no equivalent to ONNX Runtime's tuned (and, on a DX12 host, DirectML-accelerated) inference, so candle is roughly four times slower on CPU (around 158 ms versus 37 ms per cell, release build, mean over the 594-cell corpus). On a GPU host the gap is wider still, since the default engine uses DirectML and candle has no matching backend (its GPU support is CUDA/Metal). This is why candle stays an optional backend and ONNX Runtime remains the default; the gap is a runtime-performance fact, not an accuracy one.

The constraints this imposes:

- **Default-off, and it must stay that way.** The `candle` feature gates the backend, its dependency subtree, and the comparison harness. The default build is unchanged and the equivalence suite measures only the ONNX Runtime path; the feature is what lets the backend exist beside the shipped recogniser without touching the soak-tested default path.
- **A supply-chain entry rides with it.** candle pulls the unmaintained `paste` proc-macro transitively (a build-time macro, no runtime surface); its advisory is recorded in the `cargo-audit` and `cargo-deny` ignore lists, scoped to the candle feature. candle also pulls a C-toolchain transitive dependency (`onig` via `tokenizers`), which only matters where the feature is actually built.
- **The safetensors export is vendored.** The weights are a derived artefact of the committed ONNX model; the export script is the reproducible source, and the `.safetensors` is committed so a `--features candle` build needs no Python step. This duplicates the model weights on disk; if repository size becomes a concern, the artefact can be regenerated from the script instead of vendored.
- **The accuracy gates are a local maintainer gate.** The comparison sweep and the OCR microbenchmarks depend on the locally-held ground-truth bench (real gameplay screens stay out of the public tree), so they run on the maintainer's machine, not in CI, and skip with a stated reason elsewhere.

This work also added allocation-aware pooling of the preprocess scratch buffers (shared by both backends) and criterion microbenchmarks over the preprocess and recognition hot paths (`benches/ocr.rs`).

## Evidence

- `frontend/src-tauri/eo-services/src/ocr_engine.rs` (the `InferenceBackend` seam, the pooled preprocess)
- `frontend/src-tauri/eo-services/src/ocr_candle.rs` (the candle SVTRv2 forward pass)
- `backend/scripts/convert_svtrv2_to_safetensors.py` (the reproducible export)
- `frontend/src-tauri/eo-services/tests/ocr_backend_comparison.rs` (the faithfulness sweep)
- `frontend/src-tauri/eo-services/benches/ocr.rs` (the microbenchmarks)
