"""Skill scan core — screenshot capture + local OCR extraction primitives.

Captures via the shared :class:`~backend.ocr.capturer.ScreenCapturer` and
runs OpenOCR-backed local extraction. Skill
panel cells are sliced via the calibrated geometry in
``backend/data/panel_geometry.json`` and read per-cell by
:mod:`backend.services.local_ocr`. Names resolve through fuzzy match
against ``backend/data/snapshot/skills.json`` so what we persist is the
canonical vocab entry, not raw OCR text.
"""

import logging
from pathlib import Path
from typing import Any

from backend.ocr.capturer import ScreenCapturer
from backend.services import local_ocr

log = logging.getLogger(__name__)

PAGE_COUNT = 12


class SkillScanCore:
    """Screenshot + local OCR scan primitives for skill pages."""

    def __init__(self, config_service: Any, data_dir: Path):
        from backend.services.config_service import ConfigService

        self._config: ConfigService = config_service
        self._data_dir = data_dir
        self._capturer: ScreenCapturer | None = None

    @property
    def has_engine(self) -> bool:
        """Whether the local OCR engine can be loaded right now."""
        return local_ocr.is_engine_available()

    def capture_region(
        self, tl: list[int] | None, br: list[int] | None
    ) -> bytes | None:
        """Capture the skill panel region as PNG bytes via the shared capturer.

        Adapts the ``(tl, br)`` corner pair to the capturer's ``x/y/w/h``
        primitive and delegates to :meth:`ScreenCapturer.capture_region_png`.
        Returns ``None`` on bad input, an empty region, or any capture
        failure (mss unavailable, grab error) so the caller's failure
        contract is preserved.
        """
        if not tl or not br:
            return None
        x1, y1 = tl
        x2, y2 = br
        left, top = min(x1, x2), min(y1, y2)
        width, height = abs(x2 - x1), abs(y2 - y1)
        if width <= 0 or height <= 0:
            return None
        try:
            if self._capturer is None:
                self._capturer = ScreenCapturer()
            return self._capturer.capture_region_png(left, top, width, height)
        except Exception:
            log.exception(
                "Skill scan: capture failed for region (%d, %d, %d, %d)",
                left,
                top,
                width,
                height,
            )
            return None

    def extract_page_levels(self, png_bytes: bytes) -> dict[str, float]:
        """Run local OCR on a single page PNG; return {canonical_name: level}."""
        try:
            panel_bgr = local_ocr.decode_panel_png(png_bytes)
        except ValueError as exc:
            log.warning("skill scan: PNG decode failed: %s", exc)
            return {}
        rows = local_ocr.read_skill_panel(panel_bgr)
        out: dict[str, float] = {}
        for row in rows:
            name = row.get("name")
            level = row.get("level")
            if name and level is not None:
                out[name] = float(level)
        return out
