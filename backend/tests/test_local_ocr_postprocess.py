"""Tests for ``read_skill_panel``, the engine-driven skill-panel orchestration.

``read_skill_panel`` ties the device side (the recogniser engine, the geometry
and vocab loaders) to the pure parsing in ``skill_panel_parse``: it slices the
panel, OCRs each cell, resolves names, and combines the integer level with the
bar fill into one row per skill. The engine and loaders are faked here so the
row assembly is exercised without the bundled ONNX model. The pure helpers it
calls are covered directly in ``test_skill_panel_parse``.
"""

import logging

import numpy as np
import pytest

from backend.services import local_ocr

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
