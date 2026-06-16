"""Convert the bundled SVTRv2 recogniser ONNX into a safetensors file the
candle backend loads.

The candle OCR backend (`eo-services`, the default-off `candle` feature) runs
a from-scratch SVTRv2 forward pass and reads its weights from safetensors via
candle's `VarBuilder`. This script extracts the ONNX graph's weight/bias
initializers, renames them to the candle module tree's flat scheme, transposes
the linear (`MatMul`) weights into candle's `Linear` `[out, in]` layout, and
writes `svtrv2_rec.safetensors` beside the ONNX.

It is reproducible: keys are sorted and arrays are made C-contiguous, so the
same ONNX yields a byte-identical safetensors. A SHA-256 of the output is
printed so the vendored artefact can be checksum-pinned.

Only weight-class initializers are emitted. The scalar constants the graph
carries for activations (the exact-GELU 1/sqrt(2), +1, *0.5), the attention
scale, and the dynamic-shape reshape targets are NOT weights; the candle
forward pass reproduces them in code, so they are deliberately skipped.

Usage (from the repo root, in the project virtualenv):
    .venv/Scripts/python.exe backend/scripts/convert_svtrv2_to_safetensors.py

Deps: onnx, numpy, safetensors (dev-only; not runtime sidecar deps).
"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

import numpy as np
import onnx
from onnx import numpy_helper
from safetensors.numpy import save_file

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ONNX = REPO_ROOT / "backend/assets/models/svtrv2_rec.onnx"
DEFAULT_OUT = REPO_ROOT / "backend/assets/models/svtrv2_rec.safetensors"

# The model is checksum-pinned, so the node names below are frozen; the map is
# keyed on the stable dotted NODE names (not the exporter-generated
# `onnx::MatMul_*` initializer names, which are not stable across re-exports).


def clean_name_from_node(node_name: str) -> str | None:
    """Map an ONNX node name to the candle module tree's flat weight prefix.

    Returns None for nodes that are not weight-bearing layers.
    """
    path = "/".join(node_name.strip("/").split("/")[:-1])  # drop the op segment
    patterns = [
        (r"^encoder/features\.0/features\.0\.(\d+)$", lambda m: f"stem.{m[1]}"),
        (r"^encoder/features\.(\d+)/token_mixer/token_mixer\.0$", lambda m: f"enc.{m[1]}.tm0"),
        (r"^encoder/features\.(\d+)/token_mixer/token_mixer\.1/(fc1|fc2)$", lambda m: f"enc.{m[1]}.se.{m[2]}"),
        (r"^encoder/features\.(\d+)/token_mixer/token_mixer\.2$", lambda m: f"enc.{m[1]}.tm2"),
        (r"^encoder/features\.(\d+)/channel_mixer/m/m\.(\d+)$", lambda m: f"enc.{m[1]}.cm{m[2]}"),
        (r"^decoder/svtr_encoder/(conv1|conv2|conv3|conv4|conv1x1)/conv$", lambda m: f"neck.{m[1]}"),
        (r"^decoder/svtr_encoder/svtr_block\.(\d+)/(norm1|norm2)$", lambda m: f"tb.{m[1]}.{m[2]}"),
        (r"^decoder/svtr_encoder/svtr_block\.(\d+)/mixer/(qkv|proj)$", lambda m: f"tb.{m[1]}.{m[2]}"),
        (r"^decoder/svtr_encoder/svtr_block\.(\d+)/mlp/(fc1|fc2)$", lambda m: f"tb.{m[1]}.{m[2]}"),
        (r"^decoder/svtr_encoder/norm$", lambda m: "neck.norm"),
        (r"^decoder/fc$", lambda m: "head"),
    ]
    for pattern, render in patterns:
        match = re.match(pattern, path)
        if match:
            return render(match)
    return None


def clean_name_from_bias(init_name: str) -> str | None:
    """Map a NAMED linear-bias initializer (consumed by an Add after a MatMul,
    so it is not reachable from the MatMul node) to its candle prefix + .bias."""
    patterns = [
        (r"^decoder\.svtr_encoder\.svtr_block\.(\d+)\.mixer\.(qkv|proj)\.bias$", lambda m: f"tb.{m[1]}.{m[2]}.bias"),
        (r"^decoder\.svtr_encoder\.svtr_block\.(\d+)\.mlp\.(fc1|fc2)\.bias$", lambda m: f"tb.{m[1]}.{m[2]}.bias"),
        (r"^decoder\.fc\.bias$", lambda m: "head.bias"),
    ]
    for pattern, render in patterns:
        match = re.match(pattern, init_name)
        if match:
            return render(match)
    return None


def main() -> int:
    onnx_path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_ONNX
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else DEFAULT_OUT

    model = onnx.load(str(onnx_path))
    graph = model.graph
    inits = {init.name: init for init in graph.initializer}
    arrays = {name: numpy_helper.to_array(init) for name, init in inits.items()}

    tensors: dict[str, np.ndarray] = {}
    consumed: set[str] = set()

    def emit(candle_name: str, init_name: str, *, transpose: bool) -> None:
        if candle_name in tensors:
            raise SystemExit(f"name collision: {candle_name} (from {init_name})")
        arr = arrays[init_name]
        if arr.dtype == np.float16:
            arr = arr.astype(np.float32)
        if transpose:
            if arr.ndim != 2:
                raise SystemExit(f"expected 2-D MatMul weight for {init_name}, got {arr.shape}")
            arr = arr.T
        tensors[candle_name] = np.ascontiguousarray(arr.astype(np.float32))
        consumed.add(init_name)

    # Pass 1: weight-bearing layer nodes (Conv/MatMul/LayerNormalization).
    for node in graph.node:
        prefix = None
        if node.op_type in ("Conv", "MatMul", "Gemm", "LayerNormalization"):
            prefix = clean_name_from_node(node.name)
        if prefix is None:
            continue
        weight_init = node.input[1] if len(node.input) > 1 else None
        if weight_init not in inits:
            # A MatMul with two dynamic inputs (attention q.k^T / attn.v): no weight.
            continue
        emit(f"{prefix}.weight", weight_init, transpose=(node.op_type == "MatMul"))
        # Conv / LayerNorm carry their bias as input[2]; MatMul biases ride a
        # separate Add (handled in pass 2).
        if len(node.input) > 2 and node.input[2] in inits:
            emit(f"{prefix}.bias", node.input[2], transpose=False)

    # Pass 2: the linear biases (named initializers added after each MatMul).
    for init_name in inits:
        if init_name in consumed:
            continue
        bias_name = clean_name_from_bias(init_name)
        if bias_name is not None:
            emit(bias_name, init_name, transpose=False)

    # Sanity: every emitted weight needs its bias and vice versa (the model
    # has no bias-free linears or convs), and the layer count is the expected
    # 2 stem + 13 encoder-block + 5 neck-conv + (2 blocks x 6) transformer +
    # 1 neck-norm + 1 head.
    weights = {k[: -len(".weight")] for k in tensors if k.endswith(".weight")}
    biases = {k[: -len(".bias")] for k in tensors if k.endswith(".bias")}
    missing_bias = sorted(weights - biases)
    missing_weight = sorted(biases - weights)
    if missing_bias or missing_weight:
        raise SystemExit(
            f"weight/bias mismatch: no bias for {missing_bias}; no weight for {missing_weight}"
        )

    out_path.parent.mkdir(parents=True, exist_ok=True)
    ordered = {k: tensors[k] for k in sorted(tensors)}
    save_file(ordered, str(out_path))

    digest = hashlib.sha256(out_path.read_bytes()).hexdigest()
    total_params = sum(int(a.size) for a in tensors.values())
    print(f"wrote {len(tensors)} tensors ({len(weights)} layers, {total_params:,} params) -> {out_path}")
    print(f"sha256: {digest}")
    print(f"layers: {', '.join(sorted(weights))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
