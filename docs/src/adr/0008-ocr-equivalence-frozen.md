# ADR-0008: OCR equivalence frozen to the corpus

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The skill-panel and repair-cost scans run a local recogniser (the full pipeline is described in the [OCR pipeline](../architecture/ocr-pipeline.md) page): the SVTRv2-mobile ONNX model bundled in `backend/assets/models/svtrv2_rec.onnx`, driven from `backend/services/local_ocr.py` through a third-party recogniser wrapper that supplies OpenCV-shaped preprocessing, CTC decode, and the character dictionary, with skill names then fuzzy-matched against canonical vocabulary snapshots. This is the port's highest-risk surface: unlike the byte-pinned wire surfaces, recognition is a numerically delicate image pipeline, and a faithful port has to reproduce not just the model inference but every byte of the preprocess and decode around it.

The hard constraint is that there is no abstract specification of "correct" OCR. The recogniser has quirks (case and spacing drift in the raw text, the exact phase rule of OpenCV's fixed-point bilinear resize, ties-to-even rounding, the first-maximum CTC argmax) and downstream code is calibrated against those quirks. The only honest specification of correct behaviour is therefore the output the existing engine produces over a recorded ground-truth corpus of real gameplay crops. The port must reproduce that output, or it has changed behaviour, whatever else it does.

## Decision

The recorded corpus is frozen as the specification. The native engine (`frontend/src-tauri/eo-services/src/ocr_engine.rs`) reimplements the recognition chain to reproduce the original's outputs exactly rather than to be independently "better": the bilinear resize is a byte-for-byte port of OpenCV's `INTER_LINEAR` fixed-point path (half-pixel mapping, 2048-scaled round-half-even coefficients, the per-tap precision drop in the vertical blend), the `(v/255 - 0.5)/0.5` BGR normalisation and zero-padding match, and the CTC decode replicates per-timestep argmax with first-maximum tie-breaking, consecutive-duplicate removal, blank dropping, and the mean-of-kept-probabilities score.

The corpus quirks are inherited deliberately, not cleaned up: the raw model text is graded strictly against screen-verbatim ground truth, and the spacing and case drift it carries is left for the downstream name resolution to recover, exactly as the original engine relies on. Reproducing the corpus output means reproducing those imperfections too.

What is explicitly excluded from the equivalence requirement is the packaging and provider glue. Provider selection (`backend/services/local_ocr.py` runs a DirectML-then-CPU ladder; `ocr_engine.rs` rebuilds the same ladder in `new_with_providers`, with an EP-agnostic `new` for the hermetic tests) and the build-time handling of the two ONNX Runtime distributions are runtime-packaging concerns. `backend/architecture/PORT-READINESS.md` records these as redesigned, not ported: the model is runtime-agnostic, native bindings (`ort`) exist, and the equivalence obligation rests on the corpus output, not on how the runtime is selected or shipped.

## Consequences

The benefit is a single, mechanical definition of OCR correctness that survives a language change. Equivalence is gradeable on bytes, and the recogniser can be ported without re-litigating what each quirky read "should" have been.

The enforcing guard is the differential bench `frontend/src-tauri/eo-services/tests/ocr_bench_differential.rs`. Its test `the_native_recogniser_holds_the_original_engines_exact_rate` reads every graded cell through the native engine and asserts the raw exact-match count does not fall below 262 of 594 cells, the figure the original engine recorded over the same corpus. Falling below that is a regression and fails the gate. The bench is host-gated: real gameplay captures stay out of the public tree, so the test runs only where `EO_OCR_BENCH_DIR` points at the locally held corpus and the ONNX Runtime is loadable, and skips with a stated reason otherwise rather than passing vacuously.

The costs and constraints this now imposes:

- The native preprocess and decode are pinned in-crate as well as on the corpus: `ocr_engine.rs` carries unit tests that pin the resize against OpenCV-computed bytes, the round-half-even rule, the dictionary's blank-and-space bracketing, and the CTC dedupe/score behaviour, so a drift surfaces close to its cause rather than only as a corpus-count drop.
- The 262/594 figure is a floor, not a target: it is the original's recorded raw rate, and the corpus, not a human, decides what each cell should read.
- Provider and packaging changes are permitted and expected; only the recognised text and score are held to the corpus.

## Evidence

- `backend/services/local_ocr.py`
- `frontend/src-tauri/eo-services/src/ocr_engine.rs`
- `frontend/src-tauri/eo-services/tests/ocr_bench_differential.rs`
- `backend/architecture/PORT-READINESS.md`
