"""Tests for ``FixtureCapturer``, the ``ScreenCapturer`` stand-in for OCR tests.

The capturer must be a transparent substitute for the production capturer at
the two methods the OCR consumers call: ``capture_region_png`` returns the
recorded PNG bytes verbatim (the skill-scan path decodes them itself), and
``capture_region`` returns the same frame as a BGR ndarray (the repair path
reads the ndarray). These pin that fidelity, plus the fail-fast on a missing or
undecodable fixture.
"""

import cv2
import numpy as np
import pytest

from backend.testing.capturer import FixtureCapturer


def _write_png(path, img: np.ndarray) -> bytes:
    ok, buf = cv2.imencode(".png", img)
    assert ok
    data = buf.tobytes()
    path.write_bytes(data)
    return data


def test_capture_region_png_returns_bytes_verbatim(tmp_path):
    img = np.array([[[10, 20, 30], [40, 50, 60]]], dtype=np.uint8)
    png_path = tmp_path / "panel.png"
    written = _write_png(png_path, img)

    cap = FixtureCapturer(png_path)

    assert cap.capture_region_png(1895, 939, 635, 331) == written


def test_capture_region_decodes_to_matching_bgr(tmp_path):
    img = np.array([[[10, 20, 30], [40, 50, 60]]], dtype=np.uint8)
    png_path = tmp_path / "panel.png"
    _write_png(png_path, img)

    frame = FixtureCapturer(png_path).capture_region(0, 0, 2, 1)

    assert frame.shape == img.shape
    assert frame.dtype == np.uint8
    assert np.array_equal(frame, img)  # PNG is lossless: exact round-trip


def test_region_arguments_are_ignored(tmp_path):
    """Different regions serve the same fixture: the fixture is the region."""
    img = np.full((3, 3, 3), 128, dtype=np.uint8)
    png_path = tmp_path / "panel.png"
    _write_png(png_path, img)
    cap = FixtureCapturer(png_path)

    assert cap.capture_region_png(0, 0, 1, 1) == cap.capture_region_png(99, 99, 5, 5)


@pytest.mark.parametrize("width,height", [(0, 5), (5, 0), (-1, 5), (5, -1)])
def test_capture_rejects_nonpositive_dims(tmp_path, width, height):
    """The double rejects a non-positive region as the real capturer does, so a
    broken region calculation fails in tests instead of silently passing."""
    img = np.full((2, 2, 3), 128, dtype=np.uint8)
    png_path = tmp_path / "panel.png"
    _write_png(png_path, img)
    cap = FixtureCapturer(png_path)

    with pytest.raises(ValueError):
        cap.capture_region(0, 0, width, height)
    with pytest.raises(ValueError):
        cap.capture_region_png(0, 0, width, height)


def test_missing_fixture_fails_fast(tmp_path):
    with pytest.raises(FileNotFoundError):
        FixtureCapturer(tmp_path / "does-not-exist.png")


def test_capture_region_raises_on_undecodable_fixture(tmp_path):
    bad = tmp_path / "bad.png"
    bad.write_bytes(b"not a real png")
    with pytest.raises(ValueError):
        FixtureCapturer(bad).capture_region(0, 0, 1, 1)
