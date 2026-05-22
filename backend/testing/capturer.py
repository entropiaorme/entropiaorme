"""Fixture-backed screen capturer for OCR tests.

Production OCR consumes ``ScreenCapturer`` (``backend/ocr/capturer.py``)
which grabs frames via ``mss``. In test mode, ``FixtureCapturer`` serves
pre-loaded PNG fixtures from a scenario's ``scan_captures/`` directory
so the OCR pipeline can be exercised without a live game client.

The first round lands the scaffolding shape; a later round wires the
production seam (factory swap at composition time) and curates the OCR
ground-truth corpus the fixtures pair with.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:  # avoid a hard Pillow dependency at import time
    from PIL.Image import Image


class FixtureSequenceExhausted(RuntimeError):
    """Raised when a scenario asks for more captures than were queued."""


class FixtureCapturer:
    """Serves PNG fixtures in scenario-defined order.

    A scenario queues a sequence of filenames (resolved relative to
    ``fixture_dir``) via :meth:`set_sequence`; each :meth:`capture`
    call returns the next fixture in the queue. Out-of-bounds calls
    raise :class:`FixtureSequenceExhausted` rather than silently
    looping: a scenario asking for an unexpected number of captures is
    a test-authoring bug worth surfacing.

    The first round ships the scaffold only; the production seam
    (swap in lieu of ``ScreenCapturer`` under test mode) lands in a
    later round alongside the OCR ground-truth corpus.
    """

    def __init__(self, fixture_dir: Path):
        """Bind the capturer to a fixture directory; the queue starts empty."""
        self._fixture_dir = fixture_dir
        self._sequence: list[str] = []
        self._next_index = 0

    def set_sequence(self, filenames: list[str]) -> None:
        """Reset the queue to ``filenames`` in order."""
        self._sequence = list(filenames)
        self._next_index = 0

    def capture(self, region: dict | None = None) -> "Image":
        """Return the next queued fixture as a detached Pillow ``Image``.

        ``region`` mirrors the production ``ScreenCapturer.capture``
        signature and is ignored in test mode: the fixture itself is
        the recorded region.

        The returned image is detached (via ``Image.copy()``) so no
        file handle is held open across the call; this matches the
        production ``ScreenCapturer`` shape, which returns a
        fully-realised in-memory frame rather than a lazy file
        reference.
        """
        del region  # accepted for signature parity, unused in test mode

        if self._next_index >= len(self._sequence):
            raise FixtureSequenceExhausted(
                f"FixtureCapturer queue exhausted at index {self._next_index}; "
                f"queued {len(self._sequence)} fixtures."
            )

        # Pillow is in backend deps but the import lives inside the
        # call so importing this module does not pull Pillow at backend
        # startup when test mode is off.
        from PIL import Image

        path = self._fixture_dir / self._sequence[self._next_index]
        self._next_index += 1
        with Image.open(path) as image:
            return image.copy()
