"""Unit tests for the skill-panel OCR post-processing in ``local_ocr``.

These pin the pure post-processing layer that sits between the recogniser and
the persisted skill levels: name normalisation and fuzzy resolution, the
integer level parse, the bar-fill estimate, the cell-slicing geometry, and the
per-row assembly in ``read_skill_panel``. The recogniser engine is faked so the
logic is exercised without the bundled ONNX model, which keeps these runnable
on every platform and gives the mutation campaign over ``local_ocr.py`` concrete
assertions to fail against.
"""

import logging

import cv2
import numpy as np
import pytest

from backend.services import local_ocr

# ── _norm_name ───────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "raw,expected",
    [
        ("Food Technology", "foodtechnology"),  # whitespace dropped
        ("  Whip  ", "whip"),  # outer whitespace dropped, lowercased
        ("ABC", "abc"),  # case folded
        ("", ""),
    ],
)
def test_norm_name(raw, expected):
    assert local_ocr._norm_name(raw) == expected


def test_norm_name_none_is_empty():
    """A None reading normalises to the empty string rather than crashing."""
    assert local_ocr._norm_name(None) == ""  # type: ignore[arg-type]


# ── parse_level ──────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "text,expected",
    [
        ("42", 42),
        ("Level 7", 7),
        ("12abc34", 12),  # first integer run wins
        ("0", 0),
    ],
)
def test_parse_level_reads_first_integer(text, expected):
    assert local_ocr.parse_level(text) == expected


@pytest.mark.parametrize("text", ["", None, "no digits"])
def test_parse_level_none_without_an_integer(text):
    assert local_ocr.parse_level(text) is None


# ── parse_bar_fill ───────────────────────────────────────────────────────────


def _gray_bar(width: int, bright_cols: int, height: int = 4) -> np.ndarray:
    """A BGR bar crop: the first ``bright_cols`` columns white, the rest black."""
    crop = np.zeros((height, width, 3), dtype=np.uint8)
    crop[:, :bright_cols, :] = 255
    return crop


def test_parse_bar_fill_half_filled():
    """Five bright columns of ten: rightmost bright is index 4, fill = 5/10."""
    assert local_ocr.parse_bar_fill(_gray_bar(10, 5)) == pytest.approx(0.5)


def test_parse_bar_fill_quarter_filled():
    assert local_ocr.parse_bar_fill(_gray_bar(20, 5)) == pytest.approx(0.25)


def test_parse_bar_fill_full_bar_reads_zero():
    """A bar bright to the last column would be a just-levelled bar: flip to 0.0."""
    crop = _gray_bar(10, 10)
    crop[:, 0, :] = 0  # one dark column gives the contrast the estimator needs
    assert local_ocr.parse_bar_fill(crop) == 0.0


def test_parse_bar_fill_low_contrast_is_zero():
    """A uniform crop has no detectable fill edge: 0.0, not a spurious reading."""
    uniform = np.full((4, 10, 3), 100, dtype=np.uint8)
    assert local_ocr.parse_bar_fill(uniform) == 0.0


@pytest.mark.parametrize(
    "crop",
    [None, np.zeros((4, 0, 3), dtype=np.uint8)],
)
def test_parse_bar_fill_empty_is_zero(crop):
    assert local_ocr.parse_bar_fill(crop) == 0.0


# ── fuzzy_resolve ────────────────────────────────────────────────────────────

VOCAB = ["Whip", "Aim", "Food Technology"]


def test_fuzzy_resolve_exact_match():
    canonical, score, cands = local_ocr.fuzzy_resolve("Whip", VOCAB)
    assert canonical == "Whip"
    assert score == 100.0
    assert cands == [("Whip", 100.0)]


def test_fuzzy_resolve_case_and_whitespace_insensitive():
    """`food technology` resolves to the canonical `Food Technology` at 100."""
    canonical, score, _ = local_ocr.fuzzy_resolve("food technology", VOCAB)
    assert canonical == "Food Technology"
    assert score == 100.0


def test_fuzzy_resolve_near_miss_uses_fuzzy_match():
    canonical, score, cands = local_ocr.fuzzy_resolve("Whp", VOCAB)
    assert canonical == "Whip"
    assert 0.0 < score < 100.0
    assert cands  # a ranked candidate list is returned for review


@pytest.mark.parametrize("text", ["", "   ", None])
def test_fuzzy_resolve_blank_is_unresolved(text):
    assert local_ocr.fuzzy_resolve(text, VOCAB) == (None, 0.0, [])


def test_fuzzy_resolve_empty_vocab_is_unresolved():
    assert local_ocr.fuzzy_resolve("anything", []) == (None, 0.0, [])


# ── slice_panel_cells ────────────────────────────────────────────────────────

_SYNTH_GEOM = {
    "n_rows": 2,
    "cells": {
        "name": {
            "x_left": 0,
            "x_right": 4,
            "first_y_top": 0,
            "last_y_top": 10,
            "height": 5,
        }
    },
}


def test_slice_panel_cells_interpolates_row_offsets(monkeypatch):
    monkeypatch.setattr(local_ocr, "_load_geometry", lambda key: _SYNTH_GEOM)
    panel = np.zeros((20, 4, 3), dtype=np.uint8)

    crops = local_ocr.slice_panel_cells(panel, "skill")

    assert [(c.row, c.cell) for c in crops] == [(0, "name"), (1, "name")]
    # Row 0 top = first_y_top (0); row 1 top interpolates to last_y_top (10).
    assert crops[0].image.shape == (5, 4, 3)  # rows 0:5
    assert crops[1].image.shape == (5, 4, 3)  # rows 10:15


def test_slice_panel_cells_single_row_uses_first_offset(monkeypatch):
    geom = {"n_rows": 1, "cells": _SYNTH_GEOM["cells"]}
    monkeypatch.setattr(local_ocr, "_load_geometry", lambda key: geom)
    panel = np.zeros((20, 4, 3), dtype=np.uint8)

    crops = local_ocr.slice_panel_cells(panel, "skill")

    assert len(crops) == 1
    assert crops[0].row == 0
    assert crops[0].image.shape == (5, 4, 3)  # rows 0:5, no interpolation


# ── decode_panel_png ─────────────────────────────────────────────────────────


def test_decode_panel_png_round_trips():
    img = np.array([[[10, 20, 30], [40, 50, 60]]], dtype=np.uint8)
    ok, buf = cv2.imencode(".png", img)
    assert ok

    decoded = local_ocr.decode_panel_png(buf.tobytes())

    assert decoded.shape == img.shape
    assert np.array_equal(decoded, img)


def test_decode_panel_png_rejects_garbage():
    with pytest.raises(ValueError):
        local_ocr.decode_panel_png(b"not a png")


# ── read_skill_panel ─────────────────────────────────────────────────────────

_SKILL_GEOM = {
    "n_rows": 1,
    "cells": {
        "name": {
            "x_left": 0,
            "x_right": 4,
            "first_y_top": 0,
            "last_y_top": 0,
            "height": 8,
        },
        "level": {
            "x_left": 4,
            "x_right": 6,
            "first_y_top": 0,
            "last_y_top": 0,
            "height": 8,
        },
        "bar": {
            "x_left": 6,
            "x_right": 16,
            "first_y_top": 0,
            "last_y_top": 0,
            "height": 8,
        },
    },
}


class _FakeEngine:
    """Returns a reading keyed on crop width, so it is order-independent.

    The geometry above gives the name cell width 4 and the level cell width 2;
    the bar cell (width 10) never reaches the engine (it is read by fill).
    """

    def __init__(self, name_text="Whip", level_text="12", conf=0.99):
        self._name_text = name_text
        self._level_text = level_text
        self._conf = conf

    def read_text(self, crop):
        width = crop.shape[1]
        if width == 4:
            return self._name_text, self._conf
        if width == 2:
            return self._level_text, self._conf
        raise AssertionError(f"unexpected crop width {width}")


def _skill_panel_half_bar() -> np.ndarray:
    """An 8x16 panel whose bar region (x 6:16) is half bright."""
    panel = np.zeros((8, 16, 3), dtype=np.uint8)
    panel[:, 6:11, :] = 255  # 5 bright of the bar's 10 columns -> fill 0.5
    return panel


def _patch_skill_engine(monkeypatch, engine):
    monkeypatch.setattr(local_ocr, "_load_geometry", lambda key: _SKILL_GEOM)
    monkeypatch.setattr(local_ocr, "_load_skill_vocab", lambda: ["Whip", "Aim"])
    monkeypatch.setattr(local_ocr, "get_engine", lambda: engine)


def test_read_skill_panel_combines_level_and_bar_fill(monkeypatch):
    _patch_skill_engine(monkeypatch, _FakeEngine(name_text="Whip", level_text="12"))

    rows = local_ocr.read_skill_panel(_skill_panel_half_bar())

    assert rows == [{"name": "Whip", "level": 12.5}]


def test_read_skill_panel_unresolved_name_kept_as_none(monkeypatch):
    """A blank name reading leaves the row with name=None for the caller to drop."""
    _patch_skill_engine(monkeypatch, _FakeEngine(name_text="", level_text="3"))

    rows = local_ocr.read_skill_panel(_skill_panel_half_bar())

    assert rows == [{"name": None, "level": 3.5}]


def test_read_skill_panel_missing_level_yields_none_level(monkeypatch):
    _patch_skill_engine(monkeypatch, _FakeEngine(name_text="Whip", level_text="--"))

    rows = local_ocr.read_skill_panel(_skill_panel_half_bar())

    assert rows == [{"name": "Whip", "level": None}]


def test_read_skill_panel_low_confidence_warns(monkeypatch, caplog):
    _patch_skill_engine(monkeypatch, _FakeEngine(conf=0.50))

    with caplog.at_level(logging.WARNING, logger="backend.services.local_ocr"):
        local_ocr.read_skill_panel(_skill_panel_half_bar())

    assert any("low-confidence" in r.message for r in caplog.records)


def test_read_skill_panel_high_confidence_does_not_warn(monkeypatch, caplog):
    _patch_skill_engine(monkeypatch, _FakeEngine(conf=0.99))

    with caplog.at_level(logging.WARNING, logger="backend.services.local_ocr"):
        local_ocr.read_skill_panel(_skill_panel_half_bar())

    assert not any("low-confidence" in r.message for r in caplog.records)


def test_read_skill_panel_raises_without_engine(monkeypatch):
    monkeypatch.setattr(local_ocr, "get_engine", lambda: None)
    with pytest.raises(RuntimeError):
        local_ocr.read_skill_panel(_skill_panel_half_bar())
