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
fixture itself is the recorded region. There are two injection routes: the
OCR consumers take a ``capturer_factory`` constructor parameter (the
composition root wires one under test mode), and a test may still swap the
``ScreenCapturer`` symbol the consumer holds, e.g.
``monkeypatch.setattr("backend.services.skill_scan_core.ScreenCapturer",
lambda: FixtureCapturer(panel_png))``.

:class:`SequencedFixtureCapturer` layers multi-capture scans on top: a
recorded scan is a numbered series (``NNNN-skill.png`` pages,
``NNNN-repair.png`` one-shots), and each capture call serves the next
fixture of its panel type, so a whole-process replay walks the recorded
series exactly as the live scan walked the screen.
"""

from __future__ import annotations

import threading
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
        frame is the original grab. The ``x`` / ``y`` position is ignored (the
        fixture is the recorded region); ``width`` / ``height`` are validated
        the same way :class:`ScreenCapturer` validates them, so a non-positive
        region fails here as it would in production rather than silently
        serving the fixture.
        """
        del x, y  # screen position is ignored; the fixture is the region
        if width <= 0 or height <= 0:
            raise ValueError("capture dimensions must be positive")

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
        bytes it saw live. The ``x`` / ``y`` position is ignored (the fixture
        is the recorded region); ``width`` / ``height`` are validated the same
        way :class:`ScreenCapturer` validates them, so a non-positive region
        fails here as it would in production.
        """
        del x, y  # screen position is ignored; the fixture is the region
        if width <= 0 or height <= 0:
            raise ValueError("capture dimensions must be positive")
        return self._png_bytes


class SequencedFixtureCapturer:
    """Serves one panel type's recorded fixtures in sequence.

    Binds to a fixture directory and a panel type (``skill`` or ``repair``)
    and serves that panel's ``NNNN-<panel>.png`` files in name order, one
    per capture call, each through a fresh :class:`FixtureCapturer`. The
    composition root constructs one instance per panel type and hands the
    consumers a factory returning it, so a consumer that re-resolves its
    factory per scan still walks a single shared sequence.

    Construction tolerates a missing or empty directory (a chat-only
    scenario records no captures); a capture call past the end of the
    sequence raises, which the consumers' existing failure contracts turn
    into a logged, non-fatal scan failure. Test mode must never fall back
    to the real screen.
    """

    def __init__(self, fixture_dir: str | Path | None, panel: str):
        self._panel = panel
        self._lock = threading.Lock()
        self._index = 0
        if fixture_dir is None:
            self._fixtures: list[Path] = []
        else:
            self._fixtures = sorted(Path(fixture_dir).glob(f"*-{panel}.png"))

    def _next(self) -> FixtureCapturer:
        with self._lock:
            if self._index >= len(self._fixtures):
                raise ValueError(
                    f"no {self._panel} fixture remaining at capture "
                    f"{self._index + 1} (recorded: {len(self._fixtures)})"
                )
            fixture = self._fixtures[self._index]
            self._index += 1
        return FixtureCapturer(fixture)

    def capture_region(self, x: int, y: int, width: int, height: int) -> np.ndarray:
        """Serve the next fixture as a BGR frame (the repair-cost path)."""
        return self._next().capture_region(x, y, width, height)

    def capture_region_png(self, x: int, y: int, width: int, height: int) -> bytes:
        """Serve the next fixture's PNG bytes verbatim (the skill-scan path)."""
        return self._next().capture_region_png(x, y, width, height)
