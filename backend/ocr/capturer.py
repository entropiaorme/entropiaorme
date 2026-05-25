"""Screen-region capture helper."""

from __future__ import annotations

import threading

import numpy as np

try:
    import mss
    import mss.tools
except ImportError:
    mss = None


__all__ = ["ScreenCapturer"]


class ScreenCapturer:
    """Canonical screen-region capture interface for OCR.

    A region goes in (``x``, ``y``, ``width``, ``height``); a BGR uint8
    ndarray (:meth:`capture_region`) or PNG bytes (:meth:`capture_region_png`)
    comes out. ``mss`` is owned internally via a lazy per-thread session, so
    callers never touch it directly: this is the one capture path OCR
    features should reach for.
    """

    def __init__(self):
        if mss is None:
            raise ImportError("mss is required for screen capture")
        self._local = threading.local()

    def stop(self) -> None:
        sct = getattr(self._local, "sct", None)
        if sct is not None:
            close = getattr(sct, "close", None)
            if close is not None:
                close()
            del self._local.sct

    def _sct(self):
        sct = getattr(self._local, "sct", None)
        if sct is None:
            sct = mss.mss()
            self._local.sct = sct
        return sct

    def capture_region(self, x: int, y: int, width: int, height: int) -> np.ndarray:
        """Capture a screen rectangle and return a BGR uint8 image."""
        if width <= 0 or height <= 0:
            raise ValueError("capture dimensions must be positive")
        shot = self._sct().grab(
            {"left": int(x), "top": int(y), "width": int(width), "height": int(height)}
        )
        return np.asarray(shot, dtype=np.uint8)[:, :, :3]

    def capture_region_png(self, x: int, y: int, width: int, height: int) -> bytes:
        """Capture a screen rectangle and return PNG-encoded bytes.

        Serialises the grab via ``mss.tools.to_png`` (RGB), matching what an
        ``IMREAD_COLOR`` decode reads back as BGR, so the bytes are
        interchangeable with the manual-scan preview / persistence path.
        """
        if width <= 0 or height <= 0:
            raise ValueError("capture dimensions must be positive")
        shot = self._sct().grab(
            {"left": int(x), "top": int(y), "width": int(width), "height": int(height)}
        )
        return mss.tools.to_png(shot.rgb, shot.size)
