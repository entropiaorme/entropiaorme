//! The native text recogniser, ported from the recognition chain
//! `backend/services/local_ocr.py` drives through its bundled ONNX
//! engine: the SVTRv2-mobile recogniser run under ONNX Runtime with
//! the production preprocess and decode replicated exactly.
//!
//! * `RecDynamicResize([48, 320])`: bilinear resize to height 48 with
//!   cv2's half-pixel coordinate mapping, `(v/255 - 0.5)/0.5`
//!   normalisation, BGR channel order, CHW layout, zero-padded width
//!   to `int(48 * max(w/h, 320/48))`.
//! * CTC decode over the PaddleOCR v1 key set + space, blank
//!   prepended: per-timestep argmax (first maximum wins),
//!   consecutive-duplicate removal against the previous timestep,
//!   blank dropped, score = mean of the kept probabilities (0.0 when
//!   none survive).
//!
//! The resize is a byte-for-byte port of cv2's INTER_LINEAR
//! fixed-point path (verified tensor-identical against the original
//! pipeline during the runtime feasibility comparison). The runtime
//! binds dynamically at engine construction, so hosts without the
//! ONNX Runtime library refuse loudly there and nothing else in this
//! crate depends on it.

use std::path::Path;
use std::sync::Mutex;

use ort::ep::{DirectML, ExecutionProviderDispatch, CPU};
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::Tensor;

/// The recogniser's input height.
pub const TARGET_H: usize = 48;

/// The minimum width/height ratio the padded input carries.
pub const MAX_RATIO: f64 = 320.0 / 48.0;

/// Load a PNG as BGR u8 HWC (the cv2.imread convention); `(data, h, w)`.
pub fn load_bgr_png(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("unreadable PNG: {error}"))?
        .to_rgb8();
    let (w, h) = img.dimensions();
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 3];
    for (i, px) in img.pixels().enumerate() {
        out[i * 3] = px[2];
        out[i * 3 + 1] = px[1];
        out[i * 3 + 2] = px[0];
    }
    Ok((out, h, w))
}

/// cvRound: round half to even, as cv2's float-to-short cast does.
fn cv_round(v: f64) -> i32 {
    v.round_ties_even() as i32
}

/// Bilinear resize on u8 HWC replicating cv2's INTER_LINEAR
/// fixed-point path byte-for-byte: half-pixel mapping, 2^11-scaled
/// short coefficients (round-half-even), the horizontal i32
/// accumulation, and the vertical
/// `(((b0*(r0>>4))>>16) + ((b1*(r1>>4))>>16) + 2) >> 2` blend with
/// its deliberate 4-bit precision drop.
pub fn resize_bilinear(src: &[u8], sh: usize, sw: usize, dh: usize, dw: usize) -> Vec<u8> {
    let mut dst = Vec::new();
    resize_bilinear_into(src, sh, sw, dh, dw, &mut dst);
    dst
}

/// [`resize_bilinear`] writing into a caller-provided buffer, so the hot
/// recognise path can reuse one resize allocation across cells instead of
/// allocating a fresh buffer per recognition.
pub fn resize_bilinear_into(
    src: &[u8],
    sh: usize,
    sw: usize,
    dh: usize,
    dw: usize,
    dst: &mut Vec<u8>,
) {
    const COEF_SCALE: f64 = 2048.0;
    let scale_x = sw as f64 / dw as f64;
    let scale_y = sh as f64 / dh as f64;

    // Border rule (verified against cv2 empirically): the fractional
    // phase comes from the unclamped floor of the f32-cast mapping
    // and is never adjusted at the edges; only the two sampling
    // indices clamp into range. A border row or column therefore
    // reads the same line twice through both coefficient taps, and
    // the per-tap truncation in the vertical blend makes that visibly
    // different from a single full-weight tap.
    let tap_coords = |d: usize, scale: f64, ssize: usize| -> (usize, usize, i32, i32) {
        let f = ((d as f64 + 0.5) * scale - 0.5) as f32 as f64;
        let fl = f.floor();
        let frac = f - fl;
        let s = fl as isize;
        let i0 = s.clamp(0, ssize as isize - 1) as usize;
        let i1 = (s + 1).clamp(0, ssize as isize - 1) as usize;
        (
            i0,
            i1,
            cv_round((1.0 - frac) * COEF_SCALE),
            cv_round(frac * COEF_SCALE),
        )
    };

    let mut xtaps = vec![(0usize, 0usize, 0i32, 0i32); dw];
    for (dx, tap) in xtaps.iter_mut().enumerate() {
        *tap = tap_coords(dx, scale_x, sw);
    }

    dst.clear();
    dst.resize(dh * dw * 3, 0);
    for dy in 0..dh {
        let (y0, y1, ib0, ib1) = tap_coords(dy, scale_y, sh);

        for dx in 0..dw {
            let (sx, sx1, a0, a1) = xtaps[dx];
            for c in 0..3 {
                let r0 = src[(y0 * sw + sx) * 3 + c] as i32 * a0
                    + src[(y0 * sw + sx1) * 3 + c] as i32 * a1;
                let r1 = src[(y1 * sw + sx) * 3 + c] as i32 * a0
                    + src[(y1 * sw + sx1) * 3 + c] as i32 * a1;
                let val = (((ib0 * (r0 >> 4)) >> 16) + ((ib1 * (r1 >> 4)) >> 16) + 2) >> 2;
                dst[(dy * dw + dx) * 3 + c] = val.clamp(0, 255) as u8;
            }
        }
    }
}

/// `RecDynamicResize([48, 320])`: resize + normalise + zero-pad, CHW
/// BGR f32; returns the tensor and its padded width.
pub fn preprocess(img: &[u8], h: usize, w: usize) -> (Vec<f32>, usize) {
    let mut resize_buf = Vec::new();
    preprocess_into(img, h, w, &mut resize_buf)
}

/// [`preprocess`] reusing a caller-provided resize buffer, so the hot
/// recognise path does not allocate a fresh resize buffer per cell. The
/// output tensor is returned owned (the caller moves it into the inference
/// engine), so only the intermediate resize buffer is pooled.
pub fn preprocess_into(
    img: &[u8],
    h: usize,
    w: usize,
    resize_buf: &mut Vec<u8>,
) -> (Vec<f32>, usize) {
    let ratio = w as f64 / h as f64;
    let max_wh_ratio = ratio.max(MAX_RATIO);
    let img_w = (TARGET_H as f64 * max_wh_ratio) as usize;
    let ceil_w = (TARGET_H as f64 * ratio).ceil() as usize;
    let resized_w = if ceil_w > img_w { img_w } else { ceil_w };
    resize_bilinear_into(img, h, w, TARGET_H, resized_w, resize_buf);
    let mut tensor = vec![0f32; 3 * TARGET_H * img_w];
    for c in 0..3 {
        for y in 0..TARGET_H {
            for x in 0..resized_w {
                let v = resize_buf[(y * resized_w + x) * 3 + c] as f32;
                tensor[c * TARGET_H * img_w + y * img_w + x] = (v / 255.0 - 0.5) / 0.5;
            }
        }
    }
    (tensor, img_w)
}

// Reusable per-thread resize buffer for the recognise hot path. Recognitions
// run serially on the chat-log / scan worker thread, so one buffer per thread
// carries no contention and turns the per-cell resize allocation into reuse.
thread_local! {
    static OCR_RESIZE_SCRATCH: std::cell::RefCell<Vec<u8>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The decode alphabet: the character dictionary with the CTC blank
/// prepended and the space appended (the engine's
/// `use_space_char` merge).
pub fn load_dict(path: &Path) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("unreadable character dict {}: {error}", path.display()))?;
    let mut chars = vec!["blank".to_string()];
    for line in raw.lines() {
        chars.push(line.trim_matches(['\r', '\n']).to_string());
    }
    chars.push(" ".to_string());
    Ok(chars)
}

/// CTCLabelDecode: argmax, dedupe against the previous timestep, drop
/// blank, score = mean of the kept probabilities.
pub fn ctc_decode(
    logits: &[f32],
    t_len: usize,
    n_classes: usize,
    chars: &[String],
) -> (String, f64) {
    let mut text = String::new();
    let mut kept_probs: Vec<f64> = Vec::new();
    let mut prev_idx = usize::MAX;
    for t in 0..t_len {
        let row = &logits[t * n_classes..(t + 1) * n_classes];
        let mut best_idx = 0usize;
        let mut best_val = f32::NEG_INFINITY;
        for (i, &v) in row.iter().enumerate() {
            if v > best_val {
                best_val = v;
                best_idx = i;
            }
        }
        let duplicate = t > 0 && best_idx == prev_idx;
        prev_idx = best_idx;
        if duplicate || best_idx == 0 {
            continue;
        }
        text.push_str(&chars[best_idx]);
        kept_probs.push(best_val as f64);
    }
    let score = if kept_probs.is_empty() {
        0.0
    } else {
        kept_probs.iter().sum::<f64>() / kept_probs.len() as f64
    };
    (text, score)
}

/// The recogniser: one ONNX session over the bundled SVTRv2 model,
/// single-threaded and sequential exactly as the original configures
/// its engine (the model's session is not concurrency-safe to share,
/// so calls serialise on the inner guard).
pub struct OcrEngine {
    session: Mutex<Session>,
    input_name: String,
    chars: Vec<String>,
    /// The execution provider the session was actually built on, as the
    /// original records `session.get_providers()[0]`. This ort version
    /// exposes no per-session provider readout, so the value is derived
    /// from the construction control flow (the DirectML-then-CPU attempt
    /// in [`OcrEngine::new_with_providers`]): the requested-and-committed
    /// provider, not a queried one. `new` (no EP selection) records the
    /// runtime's own default.
    provider: &'static str,
}

/// The base session options every engine shares, matching the
/// original's `SessionOptions` exactly: full graph optimisation,
/// single intra/inter-op thread, sequential execution, and the
/// anti-stutter spinning controls so an idle session never pegs a
/// core between reads (`session.{intra,inter}_op.allow_spinning=0`
/// plus `session.force_spinning_stop=1`). The optimisation level and
/// thread counts mirror the EP-agnostic `new`; the sequential and
/// anti-stutter options are added here for provider parity.
fn base_session_options(builder: SessionBuilder) -> Result<SessionBuilder, String> {
    builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| error.to_string())?
        .with_intra_threads(1)
        .map_err(|error| error.to_string())?
        .with_inter_threads(1)
        .map_err(|error| error.to_string())?
        .with_parallel_execution(false)
        .map_err(|error| error.to_string())?
        .with_intra_op_spinning(false)
        .map_err(|error| error.to_string())?
        .with_inter_op_spinning(false)
        .map_err(|error| error.to_string())?
        .with_config_entry("session.force_spinning_stop", "1")
        .map_err(|error| error.to_string())
}

impl OcrEngine {
    /// Load the model and the decode alphabet with the ONNX Runtime's
    /// own default execution provider (no DirectML preference, no
    /// fallback ladder). The EP-agnostic path the hermetic recogniser
    /// tests and the offline bench exercise; production composes the
    /// engine through [`OcrEngine::new_with_providers`]. Fails (rather
    /// than panicking) when the ONNX Runtime library, the model, or the
    /// dict is absent: engine availability is a queryable condition on
    /// the scan surface, not an invariant.
    pub fn new(model_path: &Path, dict_path: &Path) -> Result<Self, String> {
        let chars = load_dict(dict_path)?;
        let session = (|| -> Result<Session, String> {
            Session::builder()
                .map_err(|error| error.to_string())?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|error| error.to_string())?
                .with_intra_threads(1)
                .map_err(|error| error.to_string())?
                .with_inter_threads(1)
                .map_err(|error| error.to_string())?
                .commit_from_file(model_path)
                .map_err(|error| error.to_string())
        })()
        .map_err(|error| format!("OCR engine load failed: {error}"))?;
        // The default-provider path does not run the DirectML attempt,
        // so it cannot honestly claim DirectML; record the runtime's own
        // default rather than fabricate a selection.
        Self::from_session(session, chars, "default")
    }

    /// Load the model with the production execution-provider ladder:
    /// DirectML preferred, CPU fallback, faithful to `local_ocr.py`'s
    /// `try InferenceSession(providers=["Dml","CPU"]) except CPU-only`.
    ///
    /// The fallback is deliberately a two-attempt control flow rather
    /// than a single mixed `[DirectML, CPU]` commit. DirectML's
    /// registration succeeds even with no DX12 GPU (it only appends the
    /// EP to the session options); the no-GPU failure surfaces at session
    /// commit, which a mixed list does NOT auto-recover from. So the
    /// first attempt asks for DirectML (with CPU in the list for per-op
    /// fallback once the session is up); on a commit error the second
    /// attempt rebuilds CPU-only. The succeeding attempt also tells us
    /// which provider we actually got, recorded on the engine since this
    /// ort version exposes no `Session::get_providers()`.
    pub fn new_with_providers(model_path: &Path, dict_path: &Path) -> Result<Self, String> {
        let chars = load_dict(dict_path)?;
        let build = |eps: Vec<ExecutionProviderDispatch>| -> Result<Session, String> {
            let builder = Session::builder().map_err(|error| error.to_string())?;
            let builder = builder
                .with_execution_providers(eps)
                .map_err(|error| error.to_string())?;
            base_session_options(builder)?
                .commit_from_file(model_path)
                .map_err(|error| error.to_string())
        };

        let (session, provider) =
            // `error_on_failure` so a DirectML REGISTRATION failure (no DML
            // runtime / non-Windows / no DX12) surfaces as an `Err` and
            // routes to the honest CPU-only retry below, rather than being
            // swallowed (ort's default) into a CPU-only session that would
            // still be mislabelled `DmlExecutionProvider`. The recorded
            // provider then never lies: it is DML only when DML truly ran.
            match build(vec![
                DirectML::default().build().error_on_failure(),
                CPU::default().build(),
            ]) {
                Ok(session) => (session, "DmlExecutionProvider"),
                Err(dml_error) => {
                    // The whole-session DirectML init failed (no DX12 GPU,
                    // driver mismatch, GPU OOM); retry CPU-only exactly as
                    // the original's except-branch does.
                    tracing::warn!(
                        target: "eo::ocr",
                        "DirectML session init failed ({dml_error}); falling back to CPU"
                    );
                    let session = build(vec![CPU::default().build()])
                        .map_err(|error| format!("OCR engine load failed: {error}"))?;
                    (session, "CPUExecutionProvider")
                }
            };
        Self::from_session(session, chars, provider)
    }

    /// Finalise an engine over a committed session: resolve the model's
    /// input name and stash the decode alphabet and the recorded
    /// provider. Shared by both constructors so the session-derived
    /// state is built one way.
    fn from_session(
        session: Session,
        chars: Vec<String>,
        provider: &'static str,
    ) -> Result<Self, String> {
        let input_name = session
            .inputs()
            .first()
            .map(|input| input.name().to_string())
            .ok_or_else(|| "model declares no inputs".to_string())?;
        Ok(Self {
            session: Mutex::new(session),
            input_name,
            chars,
            provider,
        })
    }

    /// The execution provider the session was built on, as the original
    /// logs `session.get_providers()[0]`: `"DmlExecutionProvider"` or
    /// `"CPUExecutionProvider"` from [`OcrEngine::new_with_providers`],
    /// `"default"` from the EP-agnostic [`OcrEngine::new`].
    pub fn provider(&self) -> &'static str {
        self.provider
    }

    /// Force the first (cold) inference off the production hot path,
    /// mirroring `local_ocr.warm_up`: one recognition over a 48x200
    /// all-white BGR cell, the result discarded. Best-effort by design;
    /// a warm-up failure is no different from a real-inference failure
    /// the scan path already reports, so it must not change engine
    /// construction. DirectML in particular compiles shaders / JITs
    /// kernels on first run, which this absorbs at startup.
    pub fn warm_up(&self) {
        let dummy = vec![255u8; TARGET_H * 200 * 3];
        let _ = self.recognize_bgr(&dummy, TARGET_H, 200);
    }

    /// Recognise one BGR HWC cell; `(text, score)`.
    pub fn recognize_bgr(&self, img: &[u8], h: usize, w: usize) -> Result<(String, f64), String> {
        if h == 0 || w == 0 {
            // The original's resize raises catchably on a degenerate
            // crop; refuse before the arithmetic does anything wild.
            return Err(format!("degenerate cell: {h}x{w}"));
        }
        if img.len() != h * w * 3 {
            return Err(format!(
                "cell buffer is {} bytes for {h}x{w} (expected {})",
                img.len(),
                h * w * 3
            ));
        }
        // Observe-only OCR-latency timing around the inference + decode (the
        // user-visible recognise cost). Recorded only on the success path; the
        // degenerate/size/tensor error returns leave no sample.
        let started = std::time::Instant::now();
        let (tensor_data, img_w) = OCR_RESIZE_SCRATCH.with(|buf| {
            let buf = &mut *buf.borrow_mut();
            preprocess_into(img, h, w, buf)
        });
        let tensor = Tensor::from_array(([1usize, 3, TARGET_H, img_w], tensor_data))
            .map_err(|error| format!("input tensor: {error}"))?;
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outputs = session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(|error| format!("session run: {error}"))?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| format!("output tensor: {error}"))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() != 3 || dims[0] != 1 {
            return Err(format!("expected a (1, T, C) output, got {dims:?}"));
        }
        let t_len = dims[1] as usize;
        let n_classes = dims[2] as usize;
        if n_classes != self.chars.len() {
            return Err(format!(
                "model classes vs dict mismatch: {n_classes} vs {}",
                self.chars.len()
            ));
        }
        let decoded = ctc_decode(data, t_len, n_classes, &self.chars);
        let elapsed = started.elapsed();
        eo_wire::metrics::metrics().record_ocr_latency(elapsed);
        tracing::debug!(
            target: "eo::ocr",
            provider = self.provider,
            elapsed_us = elapsed.as_micros() as u64,
            "ocr inference"
        );
        Ok(decoded)
    }

    /// Recognise one PNG cell; `(text, score)`.
    pub fn recognize_png(&self, png: &[u8]) -> Result<(String, f64), String> {
        let (img, h, w) = load_bgr_png(png)?;
        self.recognize_bgr(&img, h, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The pooled preprocess reuses its resize buffer across cells (no per-cell
    /// reallocation) and produces output byte-identical to the fresh-allocating
    /// path.
    #[test]
    fn preprocess_into_reuses_the_resize_buffer_and_matches_fresh() {
        let (h, w) = (48usize, 100usize);
        let img = vec![128u8; h * w * 3];
        let mut resize_buf = Vec::new();

        let (_, w1) = preprocess_into(&img, h, w, &mut resize_buf);
        let cap = resize_buf.capacity();
        assert!(cap > 0);

        // A second same-size cell reuses the resize allocation: capacity unchanged.
        let (pooled, w2) = preprocess_into(&img, h, w, &mut resize_buf);
        assert_eq!(w1, w2);
        assert_eq!(resize_buf.capacity(), cap, "resize buffer reallocated");

        // The pooled output equals the fresh-allocating preprocess exactly.
        let (fresh, wf) = preprocess(&img, h, w);
        assert_eq!(wf, w2);
        assert_eq!(fresh, pooled);
    }

    /// Serialises every test that pins `ORT_DYLIB_PATH` and builds a real
    /// ONNX Runtime session. `set_var` is process-global and NOT
    /// thread-safe against ORT's own `getenv`, and the runtime init is
    /// once-only; two such tests running concurrently (cargo's default)
    /// race, which surfaced as an intermittent baseline failure. Holding
    /// this lock across the env-set + session-build makes them sequential.
    /// Poison-tolerant so a panicking test cannot wedge the rest.
    static ORT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_ort() -> std::sync::MutexGuard<'static, ()> {
        ORT_TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The repo's bundled recogniser model + dict, the same pair the
    /// offline bench resolves.
    fn repo_model_paths() -> (PathBuf, PathBuf) {
        let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("backend/assets/models");
        (
            assets.join("svtrv2_rec.onnx"),
            assets.join("ppocr_keys_v1.txt"),
        )
    }

    /// The committed ONNX Runtime dylib next to the bundle's DirectML.dll
    /// and providers-shared sibling: the dev resource layout.
    fn repo_ort_dylib() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("frontend/src-tauri/entropia-orme/resources/ort/onnxruntime.dll")
    }

    /// PROVIDER SELECTION: `new_with_providers` runs the DirectML-then-CPU
    /// ladder and records a *real* provider (never the EP-agnostic
    /// `"default"`), faithful to `local_ocr.py` recording
    /// `session.get_providers()[0]`. On a DX12 host the first attempt
    /// commits and records `DmlExecutionProvider`; on a host without a
    /// DX12 GPU (or a non-Windows target, where DirectML registration
    /// itself fails) the second attempt fires and records
    /// `CPUExecutionProvider`. We assert the provider is one of those two
    /// (host-dependent which), proving the ladder ran and recorded its
    /// selection; the [DirectML, CPU] order is fixed in
    /// `new_with_providers` and the EP-agnostic `new` is asserted to
    /// record `"default"` so the two paths never blur.
    ///
    /// Host-gated like `ocr_bench_differential`: the committed dylib is
    /// pinned via `ORT_DYLIB_PATH` so the test can load the runtime
    /// without a system install; if the dylib is absent or the runtime
    /// cannot be loaded on this host, the test skips with its reason
    /// rather than passing vacuously.
    #[test]
    fn new_with_providers_runs_the_dml_then_cpu_ladder_and_records_the_selection() {
        let _ort = lock_ort();
        // The bundled ONNX Runtime is the Windows onnxruntime-directml build;
        // loading that PE via the loader on a non-Windows host hangs rather
        // than erroring, so this load-bearing test runs only on Windows
        // (where the real runtime is present and the OCR feature ships).
        if !cfg!(windows) {
            eprintln!("the bundled ONNX Runtime is Windows-only; skipping on this platform");
            return;
        }
        let dylib = repo_ort_dylib();
        if !dylib.is_file() {
            eprintln!(
                "committed ONNX Runtime dylib absent at {} on this host; skipping",
                dylib.display()
            );
            return;
        }
        // Pin the dylib for this process. ORT loads it lazily on first
        // use; setting the absolute path here makes the bundled runtime
        // (and its sibling DirectML.dll / providers-shared) authoritative
        // without a system install. Process-global and once-only: the
        // first load wins, which is exactly the production contract.
        // SAFETY: `lock_ort` serialises every env-setting / session-building
        // test, so no other thread reads or writes the env concurrently.
        unsafe {
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
        }

        let (model, dict) = repo_model_paths();
        if !model.is_file() {
            eprintln!("repo model absent at {}; skipping", model.display());
            return;
        }
        let engine = match OcrEngine::new_with_providers(&model, &dict) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("ONNX Runtime unavailable on this host ({error}); skipping");
                return;
            }
        };
        let provider = engine.provider();
        assert!(
            provider == "DmlExecutionProvider" || provider == "CPUExecutionProvider",
            "the ladder records a real provider (DML on a DX12 host, CPU otherwise), \
             got {provider:?}"
        );
        assert_ne!(
            provider, "default",
            "new_with_providers never records the EP-agnostic default"
        );
        eprintln!("new_with_providers selected provider={provider}");

        // The warmed engine still recognises: a white cell reads to a
        // (possibly empty) string and a finite score without panicking,
        // proving the EP-configured session is live, not just built.
        engine.warm_up();
        let white = vec![255u8; TARGET_H * 200 * 3];
        let (_text, score) = engine
            .recognize_bgr(&white, TARGET_H, 200)
            .expect("the EP-configured session recognises a warm-up-shaped cell");
        assert!(score.is_finite(), "the score is finite, got {score}");
    }

    /// The EP-agnostic constructor records `"default"`, never a DirectML
    /// or CPU claim it did not make. Same host-gating as the ladder test.
    #[test]
    fn new_records_the_default_provider() {
        let _ort = lock_ort();
        // Windows-only: see `new_with_providers_...` (loading the bundled
        // Windows runtime on another platform hangs the loader).
        if !cfg!(windows) {
            eprintln!("the bundled ONNX Runtime is Windows-only; skipping on this platform");
            return;
        }
        let dylib = repo_ort_dylib();
        if !dylib.is_file() {
            eprintln!("committed ONNX Runtime dylib absent; skipping");
            return;
        }
        // SAFETY: `lock_ort` serialises every env-setting / session-building
        // test, so no other thread reads or writes the env concurrently.
        unsafe {
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
        }
        let (model, dict) = repo_model_paths();
        if !model.is_file() {
            eprintln!("repo model absent; skipping");
            return;
        }
        let engine = match OcrEngine::new(&model, &dict) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("ONNX Runtime unavailable ({error}); skipping");
                return;
            }
        };
        assert_eq!(
            engine.provider(),
            "default",
            "the EP-agnostic new records the runtime's own default, not a fabricated selection"
        );
    }

    /// The OCR instrumentation fires: a real recognise records one latency
    /// sample into the metrics registry (the OCR-latency proof for the
    /// observability spine). Host-gated exactly like the ladder test above:
    /// it needs the bundled Windows ONNX Runtime and the repo model, and skips
    /// with a reason otherwise rather than passing vacuously.
    #[test]
    fn recognising_records_an_ocr_latency_sample() {
        let _ort = lock_ort();
        if !cfg!(windows) {
            eprintln!("the bundled ONNX Runtime is Windows-only; skipping on this platform");
            return;
        }
        let dylib = repo_ort_dylib();
        if !dylib.is_file() {
            eprintln!("committed ONNX Runtime dylib absent; skipping");
            return;
        }
        // SAFETY: `lock_ort` serialises every env-setting / session-building
        // test, so no other thread reads or writes the env concurrently.
        unsafe {
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
        }
        let (model, dict) = repo_model_paths();
        if !model.is_file() {
            eprintln!("repo model absent; skipping");
            return;
        }
        let engine = match OcrEngine::new_with_providers(&model, &dict) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("ONNX Runtime unavailable on this host ({error}); skipping");
                return;
            }
        };
        let before = eo_wire::metrics::metrics().snapshot().ocr_latency.count;
        let white = vec![255u8; TARGET_H * 200 * 3];
        let _ = engine
            .recognize_bgr(&white, TARGET_H, 200)
            .expect("the EP-configured session recognises a warm-up-shaped cell");
        let after = eo_wire::metrics::metrics().snapshot().ocr_latency.count;
        assert!(
            after > before,
            "a recognise records one OCR-latency sample (before={before}, after={after})"
        );
    }

    #[test]
    fn cv_round_ties_to_even() {
        assert_eq!(cv_round(0.5), 0);
        assert_eq!(cv_round(1.5), 2);
        assert_eq!(cv_round(2.5), 2);
        assert_eq!(cv_round(-0.5), 0);
        assert_eq!(cv_round(1.4), 1);
        assert_eq!(cv_round(1.6), 2);
    }

    #[test]
    fn the_dict_brackets_blank_and_space() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.txt");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let chars = load_dict(&path).unwrap();
        assert_eq!(chars, vec!["blank", "a", "b", "c", " "]);
        assert!(load_dict(&dir.path().join("missing.txt")).is_err());
    }

    #[test]
    fn ctc_decode_dedupes_skips_blank_and_averages() {
        let chars: Vec<String> = ["blank", "a", "b"].iter().map(|s| s.to_string()).collect();
        // Timesteps: a, a (dup), blank, a, b -> "aab" is wrong; dedupe
        // is against the PREVIOUS timestep only, so a,a collapses but
        // the post-blank a survives: "ab" with the blank dropped...
        // explicitly: kept = [a(t0), a(t3), b(t4)].
        #[rustfmt::skip]
        let logits = vec![
            0.1, 0.8, 0.1, // a
            0.2, 0.7, 0.1, // a (duplicate: dropped)
            0.9, 0.05, 0.05, // blank (dropped, resets nothing)
            0.1, 0.6, 0.3, // a (kept: previous timestep was blank)
            0.1, 0.2, 0.7, // b
        ];
        let (text, score) = ctc_decode(&logits, 5, 3, &chars);
        assert_eq!(text, "aab");
        let expected = (0.8f32 as f64 + 0.6f32 as f64 + 0.7f32 as f64) / 3.0;
        assert!((score - expected).abs() < 1e-12);

        // All blank: empty text, zero score.
        let logits = vec![0.9, 0.05, 0.05, 0.8, 0.1, 0.1];
        let (text, score) = ctc_decode(&logits, 2, 3, &chars);
        assert_eq!(text, "");
        assert_eq!(score, 0.0);

        // First maximum wins a tie.
        let logits = vec![0.4, 0.4, 0.2];
        let (text, _) = ctc_decode(&logits, 1, 3, &chars);
        assert_eq!(text, "", "the tie resolves to the earlier index (blank)");
    }

    #[test]
    fn preprocess_pads_to_the_ratio_floor_and_normalises() {
        // A 24x60 cell: ratio 2.5 < 320/48, so the padded width is
        // int(48 * 320/48) = 320 and the content width is
        // ceil(48 * 2.5) = 120.
        let src = vec![255u8; 24 * 60 * 3];
        let (tensor, img_w) = preprocess(&src, 24, 60);
        assert_eq!(img_w, 320);
        assert_eq!(tensor.len(), 3 * TARGET_H * 320);
        // White maps to +1.0; the padding stays at exactly 0.0.
        assert!((tensor[0] - 1.0).abs() < 1e-6);
        assert_eq!(tensor[TARGET_H * 320 - 1], 0.0, "padding is zero");

        // A wide cell exceeds the floor: padded width tracks the
        // cell's own ratio.
        let src = vec![0u8; 10 * 100 * 3];
        let (tensor, img_w) = preprocess(&src, 10, 100);
        assert_eq!(img_w, (48.0 * 10.0) as usize);
        // Black maps to -1.0 inside the content region.
        assert!((tensor[0] + 1.0).abs() < 1e-6);
    }

    /// The resize pinned byte-for-byte against the original image
    /// library's output for a deterministic non-uniform 5x7 source
    /// at two destination shapes (values computed by cv2).
    #[test]
    fn resize_matches_the_original_library_bytes() {
        const SRC: [u8; 105] = [
            175, 240, 136, 19, 196, 228, 50, 58, 25, 194, 168, 199, 246, 41, 168, 81, 67, 150, 59,
            112, 211, 208, 108, 250, 151, 3, 53, 185, 103, 54, 161, 116, 92, 133, 93, 250, 185,
            236, 217, 78, 142, 221, 218, 137, 23, 10, 141, 67, 72, 110, 73, 128, 89, 209, 52, 22,
            110, 241, 113, 18, 42, 250, 91, 107, 218, 106, 184, 68, 136, 179, 18, 4, 167, 76, 248,
            127, 230, 151, 27, 135, 68, 4, 226, 173, 176, 197, 105, 222, 127, 215, 193, 205, 135,
            225, 177, 84, 172, 91, 133, 97, 0, 222, 151, 100, 75,
        ];
        const OUT_A: [u8; 117] = [
            186, 196, 174, 148, 176, 173, 82, 141, 170, 75, 109, 118, 92, 77, 45, 136, 109, 94,
            183, 150, 163, 196, 101, 181, 201, 63, 194, 151, 98, 181, 108, 123, 179, 81, 122, 201,
            65, 122, 214, 218, 137, 23, 154, 138, 37, 42, 140, 60, 34, 129, 69, 67, 112, 73, 98,
            100, 136, 128, 89, 209, 87, 53, 156, 67, 29, 103, 168, 78, 53, 210, 134, 29, 103, 208,
            69, 42, 250, 91, 153, 204, 105, 170, 174, 131, 200, 122, 176, 201, 121, 151, 190, 140,
            99, 196, 143, 113, 205, 143, 138, 179, 140, 139, 151, 130, 141, 106, 80, 158, 78, 60,
            161, 93, 112, 127, 102, 142, 107,
        ];
        const OUT_B: [u8; 108] = [
            116, 223, 170, 68, 72, 47, 239, 57, 172, 67, 95, 188, 140, 172, 172, 106, 83, 51, 205,
            70, 191, 84, 122, 198, 179, 86, 175, 169, 101, 57, 148, 91, 224, 112, 168, 216, 166,
            100, 115, 136, 106, 72, 103, 67, 182, 117, 187, 150, 140, 139, 40, 79, 107, 90, 62, 30,
            122, 117, 199, 64, 138, 151, 83, 134, 62, 59, 101, 130, 145, 59, 195, 103, 142, 163,
            120, 179, 45, 45, 137, 199, 159, 26, 177, 133, 174, 168, 136, 190, 143, 97, 163, 138,
            139, 91, 105, 131, 193, 171, 146, 197, 201, 129, 178, 102, 127, 131, 62, 130,
        ];
        assert_eq!(resize_bilinear(&SRC, 5, 7, 3, 13), OUT_A.to_vec());
        assert_eq!(resize_bilinear(&SRC, 5, 7, 9, 4), OUT_B.to_vec());

        // The preprocess tensor over the same source: the resized
        // content normalises and the padding floor holds; spot-pin a
        // resized byte through the normalisation.
        let (tensor, img_w) = preprocess(&SRC, 5, 7);
        assert_eq!(img_w, 320);
        let resized = resize_bilinear(&SRC, 5, 7, TARGET_H, 68);
        let expected = (resized[0] as f32 / 255.0 - 0.5) / 0.5;
        assert!((tensor[0] - expected).abs() < 1e-7);
    }

    #[test]
    fn resize_identity_and_border_taps() {
        // Identity resize returns the source bytes.
        let src: Vec<u8> = (0..2 * 2 * 3).map(|v| v as u8 * 10).collect();
        assert_eq!(resize_bilinear(&src, 2, 2, 2, 2), src);

        // A uniform image stays uniform through the fixed-point blend
        // at any scale (the coefficient pairs always sum to 2048).
        let src = vec![200u8; 3 * 5 * 3];
        let out = resize_bilinear(&src, 3, 5, 48, 17);
        assert!(out.iter().all(|&v| v == 200));
    }

    #[test]
    fn png_loading_is_bgr_hwc() {
        let mut png_bytes: Vec<u8> = Vec::new();
        let img = image::RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (data, h, w) = load_bgr_png(&png_bytes).unwrap();
        assert_eq!((h, w), (1, 2));
        assert_eq!(data.len(), 6, "one row, two pixels, three channels");
        // Red pixel in BGR order: B=0, G=0, R=255.
        assert_eq!(&data[0..3], &[0, 0, 255]);
        // Blue pixel in BGR order: B=255, G=0, R=0.
        assert_eq!(&data[3..6], &[255, 0, 0]);
        assert!(load_bgr_png(b"not a png").is_err());
    }
}
