# ADR-0015: A native candle OCR backend, evaluated and not adopted

- Status: Accepted
- Context: an exploratory pure-Rust (candle) reimplementation of the SVTRv2 recogniser was built and benchmarked against the shipped ONNX Runtime engine, then deliberately not carried into the product; ONNX Runtime remains the sole recogniser

## Context and problem statement

The skill-panel and repair-cost scans run a local recogniser (the full pipeline is described in the [OCR pipeline](../architecture/ocr-pipeline.md) page): the SVTRv2-mobile model executed through ONNX Runtime, driven natively via the `ort` crate with an OpenCV-exact preprocess and CTC decode reimplemented around it. ADR-0008 pins that engine's output to the recorded ground-truth corpus.

ONNX Runtime is a C++ library bound through `load-dynamic`: the recogniser depends on a native inference runtime shipped beside the binary, and on a DirectX 12 host on the DirectML execution provider. That raised a question worth answering rather than assuming: could the recogniser instead run entirely in Rust, with no native inference runtime, by reimplementing the model's forward pass on candle (a Rust ML framework)? A pure-Rust path would simplify the dependency and packaging story. The ONNX Runtime engine already meets the product requirement, so any second backend would be an optional alternative, never a forced replacement. This record captures the evaluation and the decision not to adopt it.

## Decision

ONNX Runtime is kept as the sole OCR backend. The candle reimplementation was built far enough to measure, found faithful but materially slower, and is not carried in the product.

What was built: a from-scratch candle SVTRv2 forward pass (the LCNet convolutional encoder, the SVTR sequence neck, and the CTC head) behind a shared `InferenceBackend` seam, so only the inference step differs and the decode, preprocess, and dictionary stay shared code both paths run. Its weights come from a reproducible safetensors export of the same bundled ONNX model. It sat behind a default-off Cargo feature, so a normal build never compiled or linked it.

The backend proved **faithful**: it reproduces ONNX Runtime to floating-point precision (post-softmax cosine 1.000000, max-abs around 6e-5, every timestep's argmax identical), and over the full 594-cell ground-truth corpus it matches the default engine cell for cell (identical 262 raw-exact, 100% per-cell text agreement). The difference is not accuracy; it is latency.

The benefit, a recogniser with no native inference runtime, did not outweigh the costs:

- **Latency.** candle's CPU kernels are roughly four times slower than ONNX Runtime's tuned inference (about 158 ms versus 37 ms per cell, release build, CPU).
- **No GPU acceleration on the target.** candle has no DirectML backend (its GPU support is CUDA and Metal), so on the application's Windows DirectX 12 target it runs CPU-only, while the shipped engine is DirectML-accelerated. The measured comparison is therefore CPU-to-CPU, and the real-world gap on a GPU host is wider still.
- **Carrying cost.** Adopting it would duplicate the model weights on disk (a vendored 25 MB safetensors artefact), add a supply-chain entry through candle's transitive dependencies (the unmaintained `paste` proc-macro and a C-toolchain `onig` via `tokenizers`), and impose the perpetual maintenance of a second, default-off backend.

Since ONNX Runtime already meets the requirement at a quarter of the latency and with GPU acceleration, carrying candle would add cost and risk for no product gain. The decision is therefore non-adoption, not deferral; the work is preserved for a possible future revisit rather than kept on the maintenance books.

## Consequences

ONNX Runtime remains the only backend a build compiles, and ADR-0008's corpus equivalence is unchanged: it continues to measure the ONNX path alone. The recogniser keeps its native-runtime dependency, which is the accepted cost of the latency and GPU acceleration it buys.

The experiment is preserved as an annotated tag, `experiment/candle-ocr-backend`, rather than a live branch: the tag carries the candle forward pass, the tensor-to-tensor and corpus comparison harness, and the reproducible weight-export script, and a future revisit starts from there. A revisit is worth reopening if candle gains a DirectML-class backend for the Windows target, or if the native-runtime dependency becomes costly enough to justify trading latency for it.

The measured comparison (release build, CPU, mean over the 594-cell corpus; taken on the maintainer's machine, since the corpus is real gameplay captures held out of the public tree):

| Backend | Raw-exact | Raw accuracy | Recognition latency (CPU) |
| --- | --- | --- | --- |
| ONNX Runtime (shipped) | 262 / 594 | 44.1% | ~37 ms/cell |
| candle (evaluated) | 262 / 594 | 44.1% | ~158 ms/cell |

The faithfulness figures are asserted by the comparison harness (cosine agreement, raw-exact within two percentage points of the corpus, at least 98% per-cell text agreement); the latency figures are measured by that same harness and recorded here, not enforced in CI, because the bench is host-gated on the locally held corpus.

See [ADR-0008](0008-ocr-equivalence-frozen.md) for the corpus equivalence this leaves untouched, the [OCR pipeline](../architecture/ocr-pipeline.md) page for the shipped recogniser, and the [ADR index](index.md).

## Evidence

Shipped recogniser (on `main`):

- `frontend/src-tauri/eo-services/src/ocr_engine.rs`
- `frontend/src-tauri/eo-services/tests/ocr_bench_differential.rs`

The evaluated backend (preserved on the `experiment/candle-ocr-backend` tag, not on `main`):

- `frontend/src-tauri/eo-services/src/ocr_candle.rs`
- `frontend/src-tauri/eo-services/tests/ocr_backend_comparison.rs`
- `backend/scripts/convert_svtrv2_to_safetensors.py`
