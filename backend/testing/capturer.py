"""Fixture-backed screen capturer for OCR tests.

Production OCR consumes :class:`~backend.ocr.capturer.ScreenCapturer`
(``backend/ocr/capturer.py``), which grabs frames via ``mss``. In a test,
``FixtureCapturer`` stands in for it and serves a pre-recorded panel PNG so
the OCR pipeline runs end to end without a live game client.

It mirrors the two methods the OCR consumers call, returning the same types
off the bound fixture:

* :meth:`capture_region_png` returns the fixture PNG bytes verbatim, so they
  are bit-identical to what ``ScreenCapturer.capture_region_png`` produced
  when the panel was recorded (the skill-scan path).
* :meth:`capture_region` decodes those bytes back to a BGR ``uint8`` ndarray
  via ``cv2.imdecode(IMREAD_COLOR)``, the inverse of the RGB-to-PNG encode, so
  the frame matches the original grab (the repair-cost path).

The region arguments are accepted for signature parity and ignored: the
fixture itself is the recorded region. A test injects the capturer by
swapping the ``ScreenCapturer`` symbol the consumer holds, e.g.
``monkeypatch.setattr("backend.services.skill_scan_core.ScreenCapturer",
lambda: FixtureCapturer(panel_png))``.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:  # numpy is a runtime dep; the import stays out of the hot path
    import numpy as np


class FixtureCapturer:
    """Serves one recorded panel PNG through the ``ScreenCapturer`` interface."""

    def __init__(self, fixture_path: str | Path):
        """Bind to a fixture PNG, reading its bytes once so a missing file fails fast."""
        self._fixture_path = Path(fixture_path)
        self._png_bytes = self._fixture_path.read_bytes()

    def capture_region(self, x: int, y: int, width: int, height: int) -> np.ndarray:
        """Return the fixture as a BGR uint8 ndarray, as ``mss`` would have.

        Decodes the bound PNG via ``cv2.imdecode(IMREAD_COLOR)``: the inverse
        of the RGB-to-PNG encode the capturer used at record time, so the
        frame is the original grab. The region is ignored (the fixture is the
        recorded region).
        """
        del x, y, width, height  # accepted for parity; the fixture is the region

        import cv2
        import numpy as np

        arr = np.frombuffer(self._png_bytes, dtype=np.uint8)
        frame = cv2.imdecode(arr, cv2.IMREAD_COLOR)
        if frame is None:
            raise ValueError(f"Fixture PNG failed to decode: {self._fixture_path}")
        return frame

    def capture_region_png(self, x: int, y: int, width: int, height: int) -> bytes:
        """Return the fixture PNG bytes verbatim.

        These are bit-identical to ``ScreenCapturer.capture_region_png``'s
        output at record time, so the skill-scan decode path sees exactly the
        bytes it saw live. The region is ignored (the fixture is the recorded
        region).
        """
        del x, y, width, height  # accepted for parity; the fixture is the region
        return self._png_bytes
