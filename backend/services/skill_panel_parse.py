"""Pure parsing for skill-panel OCR.

The device and recogniser side lives in :mod:`backend.services.local_ocr` (the
ONNX engine, the screen grab, the geometry / vocab file loaders). This module
holds the device-free post-processing that turns recognised text and cropped
pixels into structured values: name normalisation and fuzzy resolution against
the canonical vocab, the integer level parse, the bar fill-ratio estimate, the
panel cell-slicing geometry, and the PNG decode.

Keeping it free of the engine and file IO lets it be unit-tested directly,
measured by coverage, and carried as the mutation campaign's OCR target without
dragging in device glue. ``read_skill_panel`` (the orchestration that loads the
engine and vocab and drives these helpers) stays in ``local_ocr``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

import cv2
import numpy as np
from rapidfuzz import fuzz, process

_LEVEL_RE = re.compile(r"\d+")

# Fuzzy lookup tunables: the top-N candidates to return and the WRatio scorer.
_FUZZ_TOP_N = 3
_FUZZ_SCORER = fuzz.WRatio
# The minimum WRatio a fuzzy candidate must score to be accepted. 60 is the
# empirically-observed lower bound of a legitimate match (a single-transposition
# typo of a PRESENT skill scores ~60); below it the read resembles no known
# skill and is far likelier OCR garbage, so it is dropped rather than
# force-matched to a confident wrong label. MUST stay byte-identical with the
# Rust parser's FUZZY_SCORE_FLOOR.
_FUZZY_SCORE_FLOOR = 60.0


# eq=False: the image field is an ndarray, whose element-wise ``==`` and
# unhashability would make a generated __eq__/__hash__ raise; identity
# equality is all this crop holder needs.
@dataclass(frozen=True, eq=False)
class CellCrop:
    row: int
    cell: str
    image: np.ndarray


def _norm_name(s: str) -> str:
    """Whitespace + case insensitive name key for tolerant matching."""
    return re.sub(r"\s+", "", s or "").lower()


def parse_level(text: str | None) -> int | None:
    """Read the first integer run from a level cell's OCR text, or None."""
    if not text:
        return None
    m = _LEVEL_RE.search(text)
    return int(m.group()) if m else None


def parse_bar_fill(crop_bgr: np.ndarray) -> float:
    """Estimate fractional fill in [0, 1) of a skill bar crop.

    Per-column mean luminance, threshold at midpoint of column-mean
    range, rightmost bright column / width. Resolution ~1% on a 95-px
    bar. A reading of 1.0 is impossible mid-bar (the in-game bar would
    have just levelled up); it always means the contrast-low fallback
    misread an empty bar, so it flips to 0.0.
    """
    if crop_bgr is None or crop_bgr.size == 0:
        return 0.0
    gray = cv2.cvtColor(crop_bgr, cv2.COLOR_BGR2GRAY)
    col_mean = gray.mean(axis=0)
    lo, hi = float(col_mean.min()), float(col_mean.max())
    width = gray.shape[1]
    if width <= 0:  # pragma: no mutate - unreachable past the size guard above
        return 0.0  # pragma: no mutate
    if (hi - lo) < 15:
        # Low contrast: no detectable fill edge. Empty bars land here
        # and would otherwise hit the 1.0 -> 0.0 flip below; just bail
        # to 0.0 directly.
        return 0.0
    threshold = (lo + hi) / 2.0
    bright_indices = np.where(col_mean >= threshold)[0]
    if bright_indices.size == 0:
        return 0.0  # pragma: no mutate - the max column always clears the midpoint
    rightmost = int(bright_indices.max())
    fill = (rightmost + 1) / width
    if fill >= 1.0:
        return 0.0
    return fill


def fuzzy_resolve(
    ocr_text: str, vocab: list[str]
) -> tuple[str | None, float, list[tuple[str, float]]]:
    """Resolve an OCR name to its canonical vocab entry.

    Tries (in order):
    1. Exact match.
    2. Case + whitespace insensitive match (covers ``whip`` vs ``Whip``,
       ``FoodTechnology`` vs ``Food Technology``).
    3. rapidfuzz WRatio top-1 against the vocab, if it scores at or above
       ``_FUZZY_SCORE_FLOOR``; a below-floor read is left unresolved
       rather than force-matched to its nearest entry.

    Returns ``(canonical_or_None, top1_score, top_n_candidates)``. The
    canonical is what gets persisted; the OCR text is a lookup key, not
    display text.
    """
    cleaned = (ocr_text or "").strip()
    if not cleaned:
        return None, 0.0, []
    if cleaned in vocab:
        return cleaned, 100.0, [(cleaned, 100.0)]
    norm_query = _norm_name(cleaned)
    for entry in vocab:
        if _norm_name(entry) == norm_query:
            return entry, 100.0, [(entry, 100.0)]
    # WRatio is also rapidfuzz's default scorer, so passing it explicitly is
    # behaviour-equivalent today; it is kept explicit so the choice is robust to
    # a future change in the library default.
    results = process.extract(cleaned, vocab, scorer=_FUZZ_SCORER, limit=_FUZZ_TOP_N)
    cands = [(canon, float(score)) for canon, score, _ in results]
    if not cands:
        return None, 0.0, []
    top_name, top_score = cands[0]
    if top_score < _FUZZY_SCORE_FLOOR:
        # Below the floor the read resembles no known skill, so leave it
        # unresolved (the caller drops a None canonical) rather than
        # force-match its nearest entry to a confident wrong label.
        return None, top_score, cands
    return top_name, top_score, cands


def slice_panel_cells(panel_bgr: np.ndarray, geom: dict) -> list[CellCrop]:
    """Slice a captured panel into per-cell BGR crops via the calibrated grid.

    ``geom`` is a panel geometry entry (``{"n_rows": int, "cells": {...}}``),
    loaded by the caller. Iterates rows top-to-bottom, then cells in
    geometry-defined order so callers can group per-row downstream. ``bar``
    cells flow through as crops; the caller chooses whether to OCR them or
    read fill ratio.
    """
    n_rows: int = geom["n_rows"]
    cells: dict[str, dict] = geom["cells"]
    out: list[CellCrop] = []
    for r in range(n_rows):
        for cell_name, cell in cells.items():
            first = cell["first_y_top"]
            last = cell["last_y_top"]
            y_top = (
                round(first + r * (last - first) / (n_rows - 1))
                if n_rows > 1
                else first
            )
            y_bot = y_top + cell["height"]
            crop = panel_bgr[y_top:y_bot, cell["x_left"] : cell["x_right"]]
            out.append(CellCrop(row=r, cell=cell_name, image=crop))
    return out


def decode_panel_png(png_bytes: bytes) -> np.ndarray:
    """Decode a PNG byte-string captured by mss into a BGR ndarray.

    The manual scan flows store captures as PNG bytes for preview /
    persistence; OCR decodes lazily when ``process`` is invoked.
    """
    arr = np.frombuffer(png_bytes, dtype=np.uint8)
    img = cv2.imdecode(arr, cv2.IMREAD_COLOR)
    if img is None:
        raise ValueError("Failed to decode panel PNG bytes")  # pragma: no mutate
    return img
