"""User-driven skill scan.

The user docks the in-game skills UI bottom-right, opens the dedicated scan
overlay, and clicks "capture" once per page after manually flipping pages
in-game. After the final page, ``process`` runs the captures through the
local OCR engine on a background thread and holds the result on
``_pending_result`` for the in-app diff-review screen, which then
``accept``s (persists via the completion callback) or ``reject``s
(discards).

Region coords come from :mod:`backend.services.scan_presets`.
"""

from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any, Literal

from backend.core.domain_events import (
    TOPIC_SCAN_STATUS_CHANGED,
    ScanStatusChanged,
    ScanStatusChangedPayload,
    to_iso_utc,
)
from backend.core.event_bus import EventBus
from backend.services.scan_presets import skill_region
from backend.services.skill_scan_core import PAGE_COUNT, SkillScanCore
from backend.testing.clock import Clock, RealClock

log = logging.getLogger(__name__)

MAX_PAGE_COUNT = 30


class SkillScanManual:
    """Manual orchestration over :class:`SkillScanCore`."""

    def __init__(
        self,
        config_service: Any,
        data_dir: Path,
        *,
        event_bus: EventBus | None = None,
        initial_scan_time: float | None = None,
        initial_skills_count: int = 0,
        clock: Clock | None = None,
        capturer_factory: Callable[[], Any] | None = None,
    ):
        self._core = SkillScanCore(
            config_service, data_dir, capturer_factory=capturer_factory
        )
        self._lock = threading.RLock()
        # Time source for the scan status event's publish-time stamp and the
        # last-scan instant; injected so replay scenarios stamp deterministic
        # instants. Defaults to the real clock.
        self._clock = clock or RealClock()
        # Typed outbox: a ``scan.status.changed`` envelope is published on the
        # in-process bus (and forwarded over the SSE bridge) whenever the status
        # settles to a new value. None in pure-OCR unit tests, where status
        # changes simply go unannounced. Mirrors the tracker's typed-instance
        # publish discipline.
        self._event_bus = event_bus

        self._active = False
        self._region: tuple[list[int], list[int]] | None = None
        self._captures: list[bytes | None] = []
        self._processing = False

        # Dynamic — the user picks the page count per scan in the overlay; this
        # default applies until they choose otherwise.
        self._expected_pages: int = PAGE_COUNT

        self._pending_result: dict[str, float] | None = None
        self._processing_progress: tuple[int, int] = (0, 0)
        self._processing_thread: threading.Thread | None = None
        self._error: str | None = None

        self._last_scan_time: float | None = initial_scan_time
        self._last_skills_count: int = initial_skills_count
        self._on_complete: Callable[[dict[str, float]], None] | None = None

        # Optional capture observer. None in normal operation; set by the
        # recording controller to copy each captured page into a bundle.
        # Called as tap(panel: str, region: dict, image_png_bytes: bytes).
        self._capture_tap: Callable[..., None] | None = None

        # Baseline the settled-boundary coalescer at the construction-time (idle)
        # status, so the first genuine change emits but a no-op publish on the
        # resting state does not: listeners hydrate the idle status via the GET
        # on mount, so an initial idle frame would be redundant. (See
        # ``_publish_status``.)
        self._last_emitted_key: tuple = self._status_key()

    def set_capture_tap(self, tap: Callable[..., None]) -> None:
        """Install a capture observer (called after each successful page grab)."""
        self._capture_tap = tap

    def clear_capture_tap(self) -> None:
        """Remove the capture observer."""
        self._capture_tap = None

    def set_completion_callback(
        self, callback: Callable[[dict[str, float]], None]
    ) -> None:
        self._on_complete = callback

    def shutdown(self) -> None:
        with self._lock:
            self._reset()

    # ── Status ──

    def get_status(self) -> dict:
        with self._lock:
            done, total = self._processing_progress
            phase = self._derive_phase()
            return {
                "active": self._active,
                "processing": self._processing,
                "captured_pages": sum(1 for c in self._captures if c is not None),
                "expected_pages": self._expected_pages,
                "last_scan_time": self._last_scan_time,
                "skills_count": self._last_skills_count,
                "configured": self._core.has_engine,
                "game_window_present": skill_region() is not None,
                "phase": phase,
                "processing_progress": {"done": done, "total": total},
                "has_pending_result": self._pending_result is not None,
                "error": self._error,
            }

    def _derive_phase(
        self,
    ) -> Literal["idle", "capturing", "processing", "awaiting_review"]:
        if self._pending_result is not None:
            return "awaiting_review"
        if self._processing:
            return "processing"
        if self._active:
            return "capturing"
        return "idle"

    # ── Status push (typed outbox) ──

    def _status_key(self) -> tuple:
        """The owned-state projection used to detect a settled status change.

        Captured under ``_lock``; an emit fires only when this differs from the
        last published frame, so the producer coalesces to one frame per
        discrete status change rather than one per call (the settled boundary
        ``SkillScanManual`` otherwise lacks, unlike the tracker's tick flush).
        Excludes the environmental fields (engine availability, game-window
        presence) that no verb mutates. Used only for equality, so the element
        types are immaterial.
        """
        return (
            self._derive_phase(),
            sum(1 for c in self._captures if c is not None),
            self._expected_pages,
            self._processing_progress,
            self._pending_result is not None,
            self._error,
            self._last_scan_time,
            self._last_skills_count,
        )

    def _publish_status(self) -> None:
        """Publish a ``scan.status.changed`` envelope iff the status moved.

        Call AFTER releasing ``_lock``, at every settled mutation point (verb
        completion, each per-page OCR step, worker completion). The key compare
        and advance happen under the lock so the main and worker threads cannot
        both emit the same transition; the typed publish happens after release,
        so no subscriber runs while this service holds its lock. Push-to-pull:
        the payload is the coarse phase only; a listener re-hydrates the full
        status via the scan-status GET, so per-page progress liveness rides the
        hydration without widening the wire.
        """
        if self._event_bus is None:
            return
        with self._lock:
            key = self._status_key()
            if key == self._last_emitted_key:
                return
            self._last_emitted_key = key
            phase = self._derive_phase()
        self._event_bus.publish(
            TOPIC_SCAN_STATUS_CHANGED,
            ScanStatusChanged(
                occurred_at=to_iso_utc(self._clock.now().timestamp()),
                payload=ScanStatusChangedPayload(phase=phase),
            ),
        )

    # ── Flow ──

    def start(self, page_count: int | None = None) -> dict:
        if not self._core.has_engine:
            return {"error": "Local OCR engine is unavailable: check the backend log"}
        region = skill_region()
        if region is None:
            return {"error": "Entropia Universe window not found: start the game first"}
        if page_count is not None and (
            not isinstance(page_count, int)
            or page_count < 1
            or page_count > MAX_PAGE_COUNT
        ):
            return {"error": f"page_count must be between 1 and {MAX_PAGE_COUNT}"}
        with self._lock:
            if self._processing:
                return {"error": "Scan currently processing: wait for it to finish"}
            if self._pending_result is not None:
                return {
                    "error": "Pending scan result awaiting review: accept or reject first"
                }
            if page_count is not None:
                self._expected_pages = page_count
            self._active = True
            self._region = region
            self._captures = []
            self._error = None
            self._processing_progress = (0, 0)
            log.info(
                "Manual skill scan started — region=%s, expecting %d pages",
                region,
                self._expected_pages,
            )
            status = self.get_status()
        self._publish_status()
        return status

    def capture_current_page(self) -> dict:
        with self._lock:
            if not self._active:
                return {"error": "No active scan: call start first"}
            if self._region is None:
                return {"error": "Region not configured"}
            tl, br = self._region
            expected = self._expected_pages
        png = self._core.capture_region(tl, br)
        with self._lock:
            self._captures.append(png)
            page_num = len(self._captures)
            ok = png is not None
        if ok:
            tap = self._capture_tap
            if tap is not None:
                try:
                    tap("skill", {"tl": tl, "br": br}, png)
                except Exception:
                    log.exception("Scan capture tap failed")
            log.info("Manual skill scan: captured page %d/%d", page_num, expected)
        else:
            log.warning("Manual skill scan: page %d capture failed", page_num)
        self._publish_status()
        return {"page": page_num, "captured": ok, **self.get_status()}

    def cancel(self) -> dict:
        with self._lock:
            if self._processing:
                return {"error": "Cannot cancel while processing: wait for completion"}
            self._reset()
        log.info("Manual skill scan cancelled")
        self._publish_status()
        return self.get_status()

    def undo_last_capture(self) -> dict:
        """Pop the most recent capture, returning the user one step back.

        Refused while processing or once all captures are taken and review
        is pending. Idempotent on an empty stack: returns an error rather
        than no-op so the frontend can surface the "nothing to undo" state.
        """
        with self._lock:
            if not self._active:
                return {"error": "No active scan: call start first"}
            if self._processing:
                return {"error": "Cannot undo while processing: wait for completion"}
            if self._pending_result is not None:
                return {
                    "error": "Pending result awaiting review: accept or reject first"
                }
            if not self._captures:
                return {"error": "No captures to undo"}
            popped_idx = len(self._captures)
            self._captures.pop()
        log.info("Manual skill scan: undid capture %d", popped_idx)
        self._publish_status()
        return {"undone_page": popped_idx, **self.get_status()}

    def get_capture_png(self, page: int) -> bytes | None:
        """Return PNG bytes for a 1-indexed page, or None if missing."""
        with self._lock:
            if page < 1 or page > len(self._captures):
                return None
            return self._captures[page - 1]

    def get_pending_result(self) -> dict[str, float] | None:
        with self._lock:
            return (
                dict(self._pending_result) if self._pending_result is not None else None
            )

    # ── Process / accept / reject ──

    def process(self) -> dict:
        """Kick off local-OCR extraction on a background thread; result held on ``_pending_result``."""
        with self._lock:
            if self._processing:
                return {"error": "Scan currently processing: wait for it to finish"}
            if self._pending_result is not None:
                return {
                    "error": "Pending result awaiting review: accept or reject first"
                }
            if not self._active:
                return {"error": "No active scan to process"}
            captures = list(self._captures)
            valid_count = sum(1 for c in captures if c is not None)
            expected = self._expected_pages
            if valid_count < expected:
                return {
                    "error": f"Need {expected} pages captured before processing (have {valid_count})"
                }
            self._processing = True
            self._active = False
            self._error = None
            self._processing_progress = (0, valid_count)

        def _worker() -> None:
            try:
                result = self._extract_levels(captures)
                with self._lock:
                    if "error" in result:
                        self._error = result["error"]
                    else:
                        self._pending_result = result["skills"]
                    self._processing = False
                self._publish_status()
            except Exception as exc:
                log.exception("Manual skill scan: process thread crashed")
                with self._lock:
                    self._error = str(exc)
                    self._processing = False
                self._publish_status()

        t = threading.Thread(target=_worker, name="skill-scan-process", daemon=True)
        with self._lock:
            self._processing_thread = t
        t.start()
        self._publish_status()
        return self.get_status()

    def accept(self) -> dict:
        """Persist the held scan result via the completion callback."""
        with self._lock:
            if self._pending_result is None:
                return {"error": "No pending result to accept"}
            skills = dict(self._pending_result)

        if self._on_complete:
            try:
                self._on_complete(skills)
            except Exception as exc:
                log.error("Manual skill scan completion callback error: %s", exc)
                with self._lock:
                    self._error = str(exc)
                self._publish_status()
                return {"error": f"Persist failed: {exc}"}

        with self._lock:
            self._last_scan_time = self._clock.now().timestamp()
            self._last_skills_count = len(skills)
            self._reset()
        log.info("Manual skill scan accepted — %d skills persisted", len(skills))
        self._publish_status()
        return {"ok": True, "skills_persisted": len(skills)}

    def reject(self) -> dict:
        """Discard the held scan result."""
        with self._lock:
            if self._pending_result is None:
                return {"error": "No pending result to reject"}
            self._reset()
        log.info("Manual skill scan rejected — pending result discarded")
        self._publish_status()
        return {"ok": True}

    # ── Internals ──

    def _reset(self) -> None:
        self._active = False
        self._region = None
        self._captures = []
        self._pending_result = None
        self._processing_progress = (0, 0)

    def _extract_levels(self, captures: list[bytes | None]) -> dict:
        """Run local OCR per-page serially; return ``{"skills": dict, "pages_processed": int}`` or ``{"error": str}``.

        Updates ``_processing_progress`` under the lock as each page
        resolves. The OpenOCR ONNX session is single-threaded — running
        pages serially avoids contention on the shared engine.
        """
        valid = [(i, png) for i, png in enumerate(captures) if png]
        if not valid:
            return {"error": "No successful captures to process"}

        log.info("Manual skill scan: extracting %d pages via local OCR", len(valid))
        with self._lock:
            self._processing_progress = (0, len(valid))
        all_skills: dict[str, float] = {}
        for page_idx, png in valid:
            page_num = page_idx + 1
            try:
                levels = self._core.extract_page_levels(png)
            except Exception as exc:
                log.error("Manual skill scan: page %d error: %s", page_num, exc)
                levels = {}
            # Order-independent merge: a duplicate skill name keeps the
            # MAXIMUM level seen across pages (a lower reading is an OCR
            # underread), while first-seen key position is preserved so the
            # output order stays stable. Mirrors the Rust extract_levels.
            for name, level in levels.items():
                prev = all_skills.get(name)
                all_skills[name] = level if prev is None else max(prev, level)
            with self._lock:
                done, total = self._processing_progress
                self._processing_progress = (done + 1, total)
            log.info(
                "Manual skill scan: page %d → %d skills extracted",
                page_num,
                len(levels),
            )
            # Per-page settled boundary: announce the advanced done/total so the
            # overlay's progress stays live without re-introducing a poll. The
            # coalescer makes this exactly one frame per page, not per tick.
            self._publish_status()

        if not all_skills:
            return {"error": "No skills extracted from any page"}

        return {"skills": all_skills, "pages_processed": len(valid)}
