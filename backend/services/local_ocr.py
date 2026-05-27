"""Local OCR runtime.

Captures cropped image regions and runs OpenOCR's SVTRv2-mobile recogniser
(Apache 2.0, ~24 MB ONNX) locally. The model weights ship bundled inside
the installer (``backend/assets/models/svtrv2_rec.onnx``); the recogniser
operates fully offline from cold start with no network access at any
point.

Two consumers share the same singleton engine:

* Skill / profession panel scans — full panel sliced into per-cell crops
  via the calibrated geometry in ``backend/data/panel_geometry.json``,
  each crop OCR'd, then fuzzy-matched against canonical vocab snapshots
  in ``backend/data/snapshot/`` (case + whitespace insensitive). Sub-
  threshold cells emit a backend warning at ``OCR_CONFIDENCE_WARN``;
  the user reviews the diff in the existing accept/reject flow.
* Repair-window cost OCR — single small numeric region read on demand,
  parsed as a PED cost by ``backend/services/repair_ocr.py``.

ONNX session picks an execution provider at construction time: DirectML
on Windows when a DX12 GPU is present, CPU otherwise. The wrapper from
``openocr-python`` handles preprocess / postprocess / dictionary
scaffolding, but we own the ``onnxruntime.InferenceSession`` so the
provider list is ours (the upstream wrapper is CUDA-only and pulls torch
in for its GPU probe). See ``THIRD-PARTY-NOTICES.md`` for license
attribution.
"""

from __future__ import annotations

import json
import logging
import sys
import threading
from pathlib import Path
from typing import Any

import numpy as np
import onnxruntime as ort

from backend.services.skill_panel_parse import (
    fuzzy_resolve,
    parse_bar_fill,
    parse_level,
    slice_panel_cells,
)

log = logging.getLogger(__name__)

GEOMETRY_PATH = Path(__file__).resolve().parents[1] / "data" / "panel_geometry.json"
SNAPSHOT_DIR = Path(__file__).resolve().parents[1] / "data" / "snapshot"

# Sub-threshold cells log a warning but still flow through; user catches
# misreads in the diff-review accept/reject screen.
OCR_CONFIDENCE_WARN = 0.85


def _bundled_model_path() -> Path:
    """Resolve the SVTRv2 ONNX path in dev and frozen modes.

    In a PyInstaller --onefile bundle, ``sys._MEIPASS`` is the temp
    extraction root where ``build_sidecar.spec``'s `datas` entries land.
    In dev, the assets directory sits next to ``backend/services/``.
    """
    if getattr(sys, "frozen", False):
        return Path(sys._MEIPASS) / "backend" / "assets" / "models" / "svtrv2_rec.onnx"  # type: ignore[attr-defined]
    return Path(__file__).resolve().parents[1] / "assets" / "models" / "svtrv2_rec.onnx"


# --- Engine ------------------------------------------------------------------


def _select_onnx_providers() -> list[str]:
    """Pick the best available ONNX Runtime execution provider.

    DirectML on Windows with a DX12 GPU (covers NVIDIA / AMD / Intel),
    CPU otherwise. CUDA via `onnxruntime-gpu` is not in scope: it pulls
    ~250 MB of CUDA libraries and is NVIDIA-only, so DirectML is the
    sole GPU path for the Windows-target audience.
    """
    available = ort.get_available_providers()
    if "DmlExecutionProvider" in available:
        return ["DmlExecutionProvider", "CPUExecutionProvider"]
    return ["CPUExecutionProvider"]


class _BundledOnnxEngine:
    """Duck-typed replacement for ``openocr.tools.infer.onnx_engine.ONNXEngine``.

    Same `run(image_numpy)` shape so it can drop into
    ``OpenRecognizer.onnx_rec_engine`` post-construct, but with our
    provider auto-pick and anti-stutter session options. On provider
    init failure (driver mismatch, GPU OOM, etc.) falls back to CPU
    once; the chosen provider is logged at construction.
    """

    def __init__(self, onnx_path: Path) -> None:
        so = ort.SessionOptions()
        # Anti-stutter: don't peg idle cores between reads.
        so.add_session_config_entry("session.intra_op.allow_spinning", "0")
        so.add_session_config_entry("session.inter_op.allow_spinning", "0")
        so.add_session_config_entry("session.force_spinning_stop", "1")
        so.intra_op_num_threads = 1
        so.inter_op_num_threads = 1
        so.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL

        preferred = _select_onnx_providers()
        try:
            session = ort.InferenceSession(str(onnx_path), so, providers=preferred)
        except Exception as exc:
            if preferred != ["CPUExecutionProvider"]:
                log.warning(
                    "Local OCR: preferred provider init failed (%s); falling back to CPU.",
                    exc,
                )
                session = ort.InferenceSession(
                    str(onnx_path), so, providers=["CPUExecutionProvider"]
                )
            else:
                raise

        self._session = session
        self._input_name = session.get_inputs()[0].name
        self._output_names = [n.name for n in session.get_outputs()]
        self.provider = session.get_providers()[0]
        log.info(
            "Local OCR session: provider=%s onnx=%s",
            self.provider,
            onnx_path.name,
        )

    def run(self, image_numpy: np.ndarray) -> list[np.ndarray]:
        return self._session.run(self._output_names, {self._input_name: image_numpy})


class OpenOcrEngine:
    """Wraps OpenOCR's SVTRv2-mobile ONNX recogniser (recognition-only).

    The model is loaded from the bundled path (no ModelScope fetch); the
    onnxruntime session uses an auto-picked provider (DirectML > CPU).
    The upstream ``OpenRecognizer`` is told ``use_gpu='false'`` to avoid
    its torch-dependent CUDA-only GPU probe; the session it would have
    built is then replaced with ``_BundledOnnxEngine``.
    """

    name = "openocr_svtrv2"

    def __init__(self) -> None:
        try:
            from openocr.tools.infer_rec import OpenRecognizer
        except ModuleNotFoundError as exc:
            raise ModuleNotFoundError(
                "openocr-python not installed. pip install openocr-python"
            ) from exc

        model_path = _bundled_model_path()
        if not model_path.is_file():
            raise FileNotFoundError(
                f"Bundled SVTRv2 ONNX missing at {model_path}; "
                "expected `backend/assets/models/svtrv2_rec.onnx` to be "
                "vendored into the repo and bundled into the sidecar by "
                "`backend/build_sidecar.spec`."
            )

        # `use_gpu='false'` skips OpenRecognizer's torch.cuda probe and its
        # buggy provider list; we replace `onnx_rec_engine` with ours below.
        rec = OpenRecognizer(
            mode="mobile",
            backend="onnx",
            onnx_model_path=str(model_path),
            use_gpu="false",
        )
        rec.onnx_rec_engine = _BundledOnnxEngine(model_path)
        self._rec = rec

    @property
    def provider(self) -> str:
        engine: _BundledOnnxEngine = self._rec.onnx_rec_engine
        return engine.provider

    def warm_up(self) -> None:
        dummy = np.full((48, 200, 3), 255, dtype=np.uint8)
        self.read_text(dummy)

    def read_text(self, crop_bgr: np.ndarray) -> tuple[str, float]:
        results = self._rec(img_numpy=crop_bgr)
        if not results:
            return "", 0.0
        r = results[0]
        text = str(r.get("text", "") or "")
        score = float(r.get("score", 0.0) or 0.0)
        return text, score


_engine: OpenOcrEngine | None = None
_engine_lock = threading.Lock()
_engine_load_error: str | None = None


def get_engine() -> OpenOcrEngine | None:
    """Return the process-wide singleton, loading + warming on first call.

    Returns ``None`` if the engine fails to load (e.g. dep missing); the
    error is cached so subsequent calls don't hammer a broken setup.
    """
    global _engine, _engine_load_error
    if _engine is not None:
        return _engine
    if _engine_load_error is not None:
        return None
    with _engine_lock:
        if _engine is not None:
            return _engine
        if _engine_load_error is not None:
            return None
        try:
            log.info("Loading local OCR engine (openocr_svtrv2)...")
            engine = OpenOcrEngine()
            engine.warm_up()
            _engine = engine
            log.info(
                "Local OCR engine ready (openocr_svtrv2 mobile/onnx, provider=%s).",
                engine.provider,
            )
            return _engine
        except Exception as exc:
            _engine_load_error = str(exc)
            log.error("Local OCR engine failed to load: %s", exc)
            return None


def is_engine_available() -> bool:
    """Whether the local OCR engine is currently usable.

    Triggers a load on first call; cheap on subsequent calls. The scan
    overlays use this for the UI gate that decides whether the OCR-driven
    UI is offerable.
    """
    return get_engine() is not None


# --- Geometry / vocab --------------------------------------------------------


def _load_geometry(panel_key: str) -> dict:
    if not GEOMETRY_PATH.exists():
        raise FileNotFoundError(
            f"Panel geometry not found at {GEOMETRY_PATH}. Run the calibration tool."
        )
    data = json.loads(GEOMETRY_PATH.read_text(encoding="utf-8"))
    if panel_key not in data:
        raise KeyError(f"No '{panel_key}' entry in {GEOMETRY_PATH}.")
    return data[panel_key]


def _load_skill_vocab() -> list[str]:
    path = SNAPSHOT_DIR / "skills.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    return [entry["name"] for entry in data]


# --- Per-panel readers -------------------------------------------------------


def _log_low_conf(panel_key: str, row: int, cell: str, text: str, conf: float) -> None:
    log.warning(
        "local_ocr low-confidence: panel=%s row=%d cell=%s conf=%.3f text=%r threshold=%.2f",
        panel_key,
        row,
        cell,
        conf,
        text,
        OCR_CONFIDENCE_WARN,
    )


def read_skill_panel(panel_bgr: np.ndarray) -> list[dict[str, Any]]:
    """Read a skill panel BGR ndarray; return one dict per data row.

    Each row dict carries ``{"name": canonical_or_None, "level": float_or_None}``.
    The integer comes from OCR'ing the level cell, the fractional part
    from the bar cell's fill ratio (~1% precision). Rows where the name
    doesn't resolve are still emitted with ``name=None``; the caller
    decides how to handle them (currently: filtered out before
    persistence by ``_extract_levels``).
    """
    engine = get_engine()
    if engine is None:
        raise RuntimeError("Local OCR engine unavailable; cannot read skill panel.")
    vocab = _load_skill_vocab()
    crops = slice_panel_cells(panel_bgr, _load_geometry("skill"))

    rows: dict[int, dict[str, Any]] = {}
    for crop in crops:
        row = rows.setdefault(
            crop.row,
            {"name": None, "level": None, "_int_level": None, "_bar_fill": 0.0},
        )
        if crop.cell == "bar":
            row["_bar_fill"] = parse_bar_fill(crop.image)
            continue
        text, conf = engine.read_text(crop.image)
        if conf < OCR_CONFIDENCE_WARN:
            _log_low_conf("skill", crop.row, crop.cell, text, conf)
        if crop.cell == "name":
            canonical, _score, _cands = fuzzy_resolve(text, vocab)
            row["name"] = canonical
        elif crop.cell == "level":
            row["_int_level"] = parse_level(text)

    out: list[dict[str, Any]] = []
    for r in sorted(rows):
        row = rows[r]
        int_level = row["_int_level"]
        level = float(int_level) + row["_bar_fill"] if int_level is not None else None
        out.append({"name": row["name"], "level": level})
    return out
