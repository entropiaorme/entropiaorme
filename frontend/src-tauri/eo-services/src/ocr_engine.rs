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

use ort::session::builder::GraphOptimizationLevel;
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

    let mut dst = vec![0u8; dh * dw * 3];
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
    dst
}

/// `RecDynamicResize([48, 320])`: resize + normalise + zero-pad, CHW
/// BGR f32; returns the tensor and its padded width.
pub fn preprocess(img: &[u8], h: usize, w: usize) -> (Vec<f32>, usize) {
    let ratio = w as f64 / h as f64;
    let max_wh_ratio = ratio.max(MAX_RATIO);
    let img_w = (TARGET_H as f64 * max_wh_ratio) as usize;
    let ceil_w = (TARGET_H as f64 * ratio).ceil() as usize;
    let resized_w = if ceil_w > img_w { img_w } else { ceil_w };
    let resized = resize_bilinear(img, h, w, TARGET_H, resized_w);
    let mut tensor = vec![0f32; 3 * TARGET_H * img_w];
    for c in 0..3 {
        for y in 0..TARGET_H {
            for x in 0..resized_w {
                let v = resized[(y * resized_w + x) * 3 + c] as f32;
                tensor[c * TARGET_H * img_w + y * img_w + x] = (v / 255.0 - 0.5) / 0.5;
            }
        }
    }
    (tensor, img_w)
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
}

impl OcrEngine {
    /// Load the model and the decode alphabet. Fails (rather than
    /// panicking) when the ONNX Runtime library, the model, or the
    /// dict is absent: engine availability is a queryable condition
    /// on the scan surface, not an invariant.
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
        let input_name = session
            .inputs()
            .first()
            .map(|input| input.name().to_string())
            .ok_or_else(|| "model declares no inputs".to_string())?;
        Ok(Self {
            session: Mutex::new(session),
            input_name,
            chars,
        })
    }

    /// Recognise one BGR HWC cell; `(text, score)`.
    pub fn recognize_bgr(&self, img: &[u8], h: usize, w: usize) -> Result<(String, f64), String> {
        if h == 0 || w == 0 {
            // The original's resize raises catchably on a degenerate
            // crop; refuse before the arithmetic does anything wild.
            return Err(format!("degenerate cell: {h}x{w}"));
        }
        let (tensor_data, img_w) = preprocess(img, h, w);
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
        if dims.len() != 3 {
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
        Ok(ctc_decode(data, t_len, n_classes, &self.chars))
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
        // Red pixel in BGR order: B=0, G=0, R=255.
        assert_eq!(&data[0..3], &[0, 0, 255]);
        // Blue pixel in BGR order: B=255, G=0, R=0.
        assert_eq!(&data[3..6], &[255, 0, 0]);
        assert!(load_bgr_png(b"not a png").is_err());
    }
}
