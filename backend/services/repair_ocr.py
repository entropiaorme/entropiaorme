"""Repair cost OCR — one-shot screen read for post-session armour repair cost.

Reads the total repair cost from the in-game repair terminal using the shared
``local_ocr`` engine (SVTRv2-mobile, bundled, DirectML-accelerated when
available). The capture region is derived at scan time from the live EU
client window via :mod:`backend.services.scan_presets`; the user docks the
repair terminal at the bottom-right of the game window at default UI scale
(1.0), and the bundled anchor encodes the cost-number rect relative to that
corner. No per-machine calibration.
"""

from __future__ import annotations

import logging
import re
from collections.abc import Callable
from typing import Any

from backend.services.scan_presets import repair_region

log = logging.getLogger(__name__)


class RepairOcrService:
    """One-shot OCR for the repair terminal cost number."""

    def __init__(
        self,
        config_service,
        *,
        capturer_factory: Callable[[], Any] | None = None,
    ):
        self._config = config_service
        # Capture seam: the composition root injects a factory yielding a
        # fixture-backed capturer under test mode; None means the production
        # screen capturer, resolved per scan.
        self._capturer_factory = capturer_factory
        # Optional capture observer. None in normal operation; set by the
        # recording controller to copy the captured frame into a bundle.
        # Called as tap(panel: str, region: dict, frame: np.ndarray).
        self._capture_tap: Callable[..., None] | None = None

    def set_capture_tap(self, tap: Callable[..., None]) -> None:
        """Install a capture observer (called after a successful frame grab)."""
        self._capture_tap = tap

    def clear_capture_tap(self) -> None:
        """Remove the capture observer."""
        self._capture_tap = None

    def scan_repair_cost(self) -> dict:
        """Capture and OCR the repair cost region. Returns {cost_ped, raw_text, confidence}."""
        region = repair_region()
        if region is None:
            return {
                "error": "Entropia Universe window not found: start the game first",
                "cost_ped": 0.0,
                "raw_text": "",
                "confidence": 0.0,
            }
        tl, br = region

        try:
            from backend.services import local_ocr

            x, y, w, h = tl[0], tl[1], br[0] - tl[0], br[1] - tl[1]
            if w <= 0 or h <= 0:
                return {
                    "error": "Invalid region",
                    "cost_ped": 0.0,
                    "raw_text": "",
                    "confidence": 0.0,
                }

            if self._capturer_factory is not None:
                capturer = self._capturer_factory()
            else:
                # Import at call time so the established test seam (patching
                # ``backend.ocr.capturer.ScreenCapturer``) keeps working.
                from backend.ocr.capturer import ScreenCapturer

                capturer = ScreenCapturer()
            frame = capturer.capture_region(x, y, w, h)
            if frame is None:
                return {
                    "error": "Capture failed",
                    "cost_ped": 0.0,
                    "raw_text": "",
                    "confidence": 0.0,
                }

            tap = self._capture_tap
            if tap is not None:
                try:
                    tap("repair", {"x": x, "y": y, "w": w, "h": h}, frame)
                except Exception:
                    log.exception("Scan capture tap failed")

            engine = local_ocr.get_engine()
            if engine is None:
                return {
                    "error": "Local OCR engine unavailable",
                    "cost_ped": 0.0,
                    "raw_text": "",
                    "confidence": 0.0,
                }

            text, confidence = engine.read_text(frame)
            cost = self._parse_cost(text)

            log.info(
                "Repair OCR: raw='%s' confidence=%.2f parsed=%.2f",
                text,
                confidence,
                cost,
            )
            return {
                "cost_ped": cost,
                "raw_text": text,
                "confidence": confidence,
            }

        except Exception as e:
            log.error("Repair OCR scan failed: %s", e)
            return {"error": str(e), "cost_ped": 0.0, "raw_text": "", "confidence": 0.0}

    @staticmethod
    def _parse_cost(text: str) -> float:
        """Extract a PED cost number from OCR text."""
        match = re.search(r"(\d+\.?\d*)", text.replace(",", ".").replace(" ", ""))
        if match:
            try:
                return float(match.group(1))
            except ValueError:
                pass
        return 0.0
