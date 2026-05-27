"""Unit tests for ``SkillScanCore.extract_page_levels``.

``extract_page_levels`` is the post-processing that turns a page's recognised
rows into the ``{canonical_name: level}`` map the scan persists: it drops rows
with no resolved name or no level and coerces the level to ``float``. The
recogniser is faked here (via ``read_skill_panel``) so the filter and the
PNG-decode failure path are pinned without the ONNX model, giving the mutation
campaign over ``skill_scan_core.py`` assertions to fail against.
"""

import cv2
import numpy as np

from backend.services import local_ocr
from backend.services.skill_scan_core import SkillScanCore


def _core(tmp_path) -> SkillScanCore:
    return SkillScanCore(config_service=None, data_dir=tmp_path)


def _png_bytes() -> bytes:
    img = np.zeros((2, 2, 3), dtype=np.uint8)
    ok, buf = cv2.imencode(".png", img)
    assert ok
    return buf.tobytes()


def test_extract_page_levels_returns_empty_on_decode_failure(tmp_path):
    """Undecodable bytes are a clean empty page, not a crash."""
    assert _core(tmp_path).extract_page_levels(b"not a png") == {}


def test_extract_page_levels_keeps_named_levelled_rows(tmp_path, monkeypatch):
    rows = [
        {"name": "Whip", "level": 12.5},  # kept
        {"name": None, "level": 3.0},  # dropped: no resolved name
        {"name": "Aim", "level": None},  # dropped: no level
        {"name": "Bioregenesis", "level": 0.0},  # kept: level 0.0 is a real level
    ]
    monkeypatch.setattr(local_ocr, "read_skill_panel", lambda panel: rows)

    out = _core(tmp_path).extract_page_levels(_png_bytes())

    assert out == {"Whip": 12.5, "Bioregenesis": 0.0}


def test_extract_page_levels_coerces_level_to_float(tmp_path, monkeypatch):
    monkeypatch.setattr(
        local_ocr, "read_skill_panel", lambda panel: [{"name": "Whip", "level": 7}]
    )

    out = _core(tmp_path).extract_page_levels(_png_bytes())

    assert out == {"Whip": 7.0}
    assert isinstance(out["Whip"], float)


def test_extract_page_levels_empty_rows_is_empty_map(tmp_path, monkeypatch):
    monkeypatch.setattr(local_ocr, "read_skill_panel", lambda panel: [])
    assert _core(tmp_path).extract_page_levels(_png_bytes()) == {}
