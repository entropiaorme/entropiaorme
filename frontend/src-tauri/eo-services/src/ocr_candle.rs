//! The optional candle OCR backend: a from-scratch SVTRv2 forward pass on
//! candle, selected by the default-off `candle` feature. It plugs into the
//! [`InferenceBackend`](crate::ocr_engine::InferenceBackend) seam and reuses
//! the shared preprocess and CTC decode unchanged, so it swaps only the
//! inference step the default ONNX Runtime engine owns.
//!
//! The recogniser is SVTRv2-mobile: an LCNet-style CNN encoder (a two-conv
//! stem then thirteen RepMixer blocks, with squeeze-excite on a subset and
//! two height-halving downsample blocks) feeding a SVTR sequence neck (height
//! pooled to one, two global-attention transformer blocks, then a conv
//! re-fusion) and a CTC linear head. Every stage is replicated from the
//! ONNX graph the default engine runs; the weights are loaded from a
//! safetensors export of that same model (`convert_svtrv2_to_safetensors.py`),
//! so the two backends share one set of weights and differ only in runtime.
//!
//! Shape conventions match the ONNX graph: input `[1, 3, 48, W]` (BGR, the
//! shared `preprocess` already normalised it), output `[1, T, 6625]`
//! post-softmax probabilities the shared `ctc_decode` consumes exactly as it
//! does the ONNX Runtime output.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Result as CandleResult, Tensor, D};
use candle_nn::{LayerNorm, Linear, Module};

use crate::ocr_engine::{InferenceBackend, OcrEngine};

/// Transformer embedding dimension (the SVTR neck token width).
const EMBED: usize = 256;
/// Attention heads and per-head dimension (`8 * 32 == EMBED`).
const HEADS: usize = 8;
const HEAD_DIM: usize = 32;
/// CTC vocabulary size (6624 dictionary symbols + the blank class).
const VOCAB: usize = 6625;
/// LayerNorm epsilons: the transformer block norms use 1e-5, the final neck
/// norm uses 1e-6 (read straight off the ONNX graph; the split matters).
const BLOCK_NORM_EPS: f64 = 1e-5;
const FINAL_NORM_EPS: f64 = 1e-6;

/// The candle recogniser backend: the SVTRv2 weights loaded once, run on the
/// CPU device (candle's CPU matmul path carries no GPU dependency).
pub struct CandleBackend {
    device: Device,
    weights: HashMap<String, Tensor>,
}

impl CandleBackend {
    /// Load the backend from the safetensors export sitting beside the ONNX
    /// model (same stem, `.safetensors` extension).
    pub fn new(model_path: &Path) -> Result<Self, String> {
        let st_path = model_path.with_extension("safetensors");
        let device = Device::Cpu;
        let weights = candle_core::safetensors::load(&st_path, &device).map_err(|error| {
            format!(
                "candle OCR backend: failed to load weights from {}: {error}",
                st_path.display()
            )
        })?;
        Ok(Self { device, weights })
    }

    fn get(&self, name: &str) -> CandleResult<Tensor> {
        self.weights
            .get(name)
            .cloned()
            .ok_or_else(|| candle_core::Error::Msg(format!("missing weight: {name}")))
    }

    /// A 2-D convolution with per-axis padding and stride. candle's conv2d
    /// takes a single symmetric padding and a single stride, so asymmetric
    /// `[1, 3]`-kernel padding (pad width only) and the `[2, 1]` downsample
    /// stride (halve height only) are handled here: pad each axis explicitly,
    /// run a unit-stride conv, then subsample. A strided convolution's output
    /// at index `i` equals the unit-stride output at `i * stride`, so the
    /// subsample is exact.
    #[allow(clippy::too_many_arguments)]
    fn conv(
        &self,
        x: &Tensor,
        name: &str,
        pad_h: usize,
        pad_w: usize,
        stride_h: usize,
        stride_w: usize,
        groups: usize,
    ) -> CandleResult<Tensor> {
        let weight = self.get(&format!("{name}.weight"))?;
        let bias = self.get(&format!("{name}.bias"))?;
        let y = if pad_h == pad_w && stride_h == stride_w {
            // The symmetric common case (every stem, encoder, SE, and 1x1 conv):
            // candle's native conv2d takes the padding and stride directly, so
            // it needs no explicit pad buffer and computes only the strided
            // output (the stem's stride-2 convs would otherwise do 4x the work).
            x.conv2d(&weight, pad_h, stride_h, 1, groups)?
        } else {
            // Asymmetric padding (the [1,3] neck convs) or asymmetric stride
            // (the [2,1] downsample): pad each axis, run a unit-stride conv,
            // then subsample. A strided convolution's output at index i equals
            // the unit-stride output at i*stride, so the subsample is exact.
            let mut y = x.clone();
            if pad_h > 0 {
                y = y.pad_with_zeros(2, pad_h, pad_h)?;
            }
            if pad_w > 0 {
                y = y.pad_with_zeros(3, pad_w, pad_w)?;
            }
            y = y.conv2d(&weight, 0, 1, 1, groups)?;
            if stride_h > 1 {
                y = subsample(&y, 2, stride_h)?;
            }
            if stride_w > 1 {
                y = subsample(&y, 3, stride_w)?;
            }
            y
        };
        let out_c = bias.dim(0)?;
        y.broadcast_add(&bias.reshape((1, out_c, 1, 1))?)
    }

    fn linear(&self, x: &Tensor, name: &str) -> CandleResult<Tensor> {
        let weight = self.get(&format!("{name}.weight"))?;
        let bias = self.get(&format!("{name}.bias"))?;
        Linear::new(weight, Some(bias)).forward(x)
    }

    fn layer_norm(&self, x: &Tensor, name: &str, eps: f64) -> CandleResult<Tensor> {
        let weight = self.get(&format!("{name}.weight"))?;
        let bias = self.get(&format!("{name}.bias"))?;
        LayerNorm::new(weight, bias, eps).forward(x)
    }

    /// One RepMixer encoder block. `se` adds the squeeze-excite gate on the
    /// token mixer; `downsample` swaps the unit-stride depthwise mixer for the
    /// height-halving `[2, 1]` mixer plus a 1x1 channel expansion. The
    /// residual sits on the channel mixer (its input plus its output), matching
    /// the graph; there is no whole-block skip.
    fn enc_block(&self, x: &Tensor, i: usize, se: bool, downsample: bool) -> CandleResult<Tensor> {
        let p = format!("enc.{i}");
        let c_in = x.dim(1)?;
        let t = if downsample {
            // Depthwise 3x3 stride [2,1] (height-halving), then 1x1 expand.
            let t = self.conv(x, &format!("{p}.tm0"), 1, 1, 2, 1, c_in)?;
            self.conv(&t, &format!("{p}.tm2"), 0, 0, 1, 1, 1)?
        } else {
            let mut t = self.conv(x, &format!("{p}.tm0"), 1, 1, 1, 1, c_in)?;
            if se {
                let s = t.mean_keepdim(2)?.mean_keepdim(3)?; // global average pool -> [N,C,1,1]
                let s = self.conv(&s, &format!("{p}.se.fc1"), 0, 0, 1, 1, 1)?.relu()?;
                let s = self.conv(&s, &format!("{p}.se.fc2"), 0, 0, 1, 1, 1)?;
                let s = candle_nn::ops::sigmoid(&s)?;
                t = t.broadcast_mul(&s)?;
            }
            t
        };
        let c = self.conv(&t, &format!("{p}.cm0"), 0, 0, 1, 1, 1)?.gelu_erf()?;
        let c = self.conv(&c, &format!("{p}.cm2"), 0, 0, 1, 1, 1)?;
        t.add(&c)
    }

    /// One SVTR transformer block (pre-norm): global multi-head self-attention
    /// then a SiLU MLP, each with a residual.
    fn transformer_block(&self, x: &Tensor, b: usize) -> CandleResult<Tensor> {
        let p = format!("tb.{b}");
        let (n, seq, _) = x.dims3()?;

        let h = self.layer_norm(x, &format!("{p}.norm1"), BLOCK_NORM_EPS)?;
        let qkv = self.linear(&h, &format!("{p}.qkv"))?; // [N, T, 3*HEADS*HEAD_DIM]
        let qkv = qkv
            .reshape((n, seq, 3, HEADS, HEAD_DIM))?
            .permute((2, 0, 3, 1, 4))? // [3, N, HEADS, T, HEAD_DIM]
            .contiguous()?;
        let q = qkv.narrow(0, 0, 1)?.squeeze(0)?.contiguous()?; // [N, HEADS, T, HEAD_DIM]
        let k = qkv.narrow(0, 1, 1)?.squeeze(0)?.contiguous()?;
        let v = qkv.narrow(0, 2, 1)?.squeeze(0)?.contiguous()?;

        let scale = 1.0 / (HEAD_DIM as f64).sqrt();
        let q = q.affine(scale, 0.0)?;
        let scores = q.matmul(&k.transpose(D::Minus1, D::Minus2)?.contiguous()?)?; // [N,HEADS,T,T]
        let probs = candle_nn::ops::softmax(&scores, D::Minus1)?;
        let ctx = probs.matmul(&v)?; // [N, HEADS, T, HEAD_DIM]
        let ctx = ctx
            .permute((0, 2, 1, 3))? // [N, T, HEADS, HEAD_DIM]
            .contiguous()?
            .reshape((n, seq, EMBED))?;
        let attn_out = self.linear(&ctx, &format!("{p}.proj"))?;
        let x = x.add(&attn_out)?;

        let h = self.layer_norm(&x, &format!("{p}.norm2"), BLOCK_NORM_EPS)?;
        let h = self.linear(&h, &format!("{p}.fc1"))?.silu()?;
        let h = self.linear(&h, &format!("{p}.fc2"))?;
        x.add(&h)
    }

    /// The full SVTRv2 forward pass: preprocessed input tensor in, CTC
    /// post-softmax probabilities `[1, T, VOCAB]` out.
    fn forward(&self, tensor_data: &[f32], width: usize) -> CandleResult<Tensor> {
        let x = Tensor::from_slice(tensor_data, (1usize, 3, 48, width), &self.device)?;

        // Stem: conv (3->48, s2) -> exact GELU -> conv (48->96, s2). H 48->12.
        let x = self.conv(&x, "stem.0", 1, 1, 2, 2, 1)?.gelu_erf()?;
        let x = self.conv(&x, "stem.2", 1, 1, 2, 2, 1)?;

        // Encoder: 13 RepMixer blocks. SE on 1,5,7,9,12; downsample on 4,11.
        let x = self.enc_block(&x, 1, true, false)?;
        let x = self.enc_block(&x, 2, false, false)?;
        let x = self.enc_block(&x, 3, false, false)?;
        let x = self.enc_block(&x, 4, false, true)?;
        let x = self.enc_block(&x, 5, true, false)?;
        let x = self.enc_block(&x, 6, false, false)?;
        let x = self.enc_block(&x, 7, true, false)?;
        let x = self.enc_block(&x, 8, false, false)?;
        let x = self.enc_block(&x, 9, true, false)?;
        let x = self.enc_block(&x, 10, false, false)?;
        let x = self.enc_block(&x, 11, false, true)?;
        let x = self.enc_block(&x, 12, true, false)?;
        let x = self.enc_block(&x, 13, false, false)?; // [1, 384, 3, W/16]

        // SVTR neck: collapse height, halve width, project to tokens.
        let pooled = x.mean_keepdim(2)?; // ReduceMean over H -> [1, 384, 1, W/16]
        let pooled = pooled.avg_pool2d((1, 2))?; // AvgPool [1,2] s[1,2] -> [1, 384, 1, T]
        let h = self.conv(&pooled, "neck.conv1", 0, 1, 1, 1, 1)?.silu()?; // 384->48, k[1,3]
        let h = self.conv(&h, "neck.conv2", 0, 0, 1, 1, 1)?.silu()?; // 48->256, 1x1
        let seq = h.dim(3)?;
        let tokens = h.reshape((1, EMBED, seq))?.transpose(1, 2)?.contiguous()?; // [1, T, 256]

        // Two transformer blocks, then the final neck norm.
        let tokens = self.transformer_block(&tokens, 0)?;
        let tokens = self.transformer_block(&tokens, 1)?;
        let tokens = self.layer_norm(&tokens, "neck.norm", FINAL_NORM_EPS)?;

        // Conv re-fusion: transformer branch (conv3) fused with the pooled
        // branch, then conv4 / conv1x1. Concat order is [pooled, conv3].
        let back = tokens.transpose(1, 2)?.reshape((1, EMBED, 1, seq))?.contiguous()?;
        let conv3 = self.conv(&back, "neck.conv3", 0, 0, 1, 1, 1)?.silu()?; // 256->384, 1x1
        let fused = Tensor::cat(&[&pooled, &conv3], 1)?; // [1, 768, 1, T]
        let h = self.conv(&fused, "neck.conv4", 0, 1, 1, 1, 1)?.silu()?; // 768->48, k[1,3]
        let h = self.conv(&h, "neck.conv1x1", 0, 0, 1, 1, 1)?.silu()?; // 48->256, 1x1

        // CTC head: [1,256,1,T] -> [1,T,256] -> linear -> softmax over vocab.
        let seq2 = h.dim(3)?;
        let head_in = h.reshape((1, EMBED, seq2))?.transpose(1, 2)?.contiguous()?;
        let logits = self.linear(&head_in, "head")?; // [1, T, VOCAB]
        candle_nn::ops::softmax(&logits, 2)
    }
}

impl InferenceBackend for CandleBackend {
    fn infer(
        &self,
        tensor_data: &[f32],
        padded_width: usize,
    ) -> Result<(Vec<f32>, (usize, usize)), String> {
        let probs = self
            .forward(tensor_data, padded_width)
            .map_err(|error| format!("candle forward pass: {error}"))?;
        let (_, t_len, n_classes) = probs
            .dims3()
            .map_err(|error| format!("candle output shape: {error}"))?;
        let flat = probs
            .to_dtype(DType::F32)
            .and_then(|t| t.flatten_all())
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|error| format!("candle output extract: {error}"))?;
        Ok((flat, (t_len, n_classes)))
    }
}

/// Select every `step`-th index along `dim` (the exact equivalent of a strided
/// convolution applied to a unit-stride convolution's output).
fn subsample(x: &Tensor, dim: usize, step: usize) -> CandleResult<Tensor> {
    let n = x.dim(dim)?;
    let idx: Vec<u32> = (0..n as u32).step_by(step).collect();
    let len = idx.len();
    let idx = Tensor::from_vec(idx, len, x.device())?;
    x.index_select(&idx, dim)
}

/// Build an [`OcrEngine`] backed by the candle recogniser, loading the same
/// decode alphabet the ONNX Runtime constructors use so `ctc_decode` reads an
/// identical vocabulary across both backends.
pub fn candle_engine(model_path: &Path, dict_path: &Path) -> Result<OcrEngine, String> {
    let backend = Box::new(CandleBackend::new(model_path)?);
    OcrEngine::with_backend(backend, dict_path, "CandleBackend")
}

#[allow(dead_code)]
const _VOCAB_CHECK: usize = VOCAB; // VOCAB documents the head width; silence unused on some builds.
