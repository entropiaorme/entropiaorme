"""Recording-mode taps and bundle writer.

Pure mechanism: three observer taps that copy a live input surface into a
recording bundle directory, plus a :class:`Recorder` container bundling them
against one target directory. No lifecycle and no global state live here; the
recording controller (:mod:`backend.testing.recording_controller`) owns the
start/stop state machine and attaches each tap to its live service.

Thread-safety: the chatlog tap fires on the watcher's tail thread, the
keystroke tap on the pynput listener thread, and the scan tap on a capture
worker thread. Every tap guards its mutable state with a lock and flushes
each write so a crashed recording still leaves a readable partial bundle.

Surface coverage note: at the time this lands, only the chatlog surface has a
replay-side consumer, so only ``chat_replay.log`` is golden-verified by the
controller's determinism step. ``scan_captures/`` and ``keystrokes.jsonl`` are
captured faithfully and preserved permanently, but their replay verification
activates when the screen-capture and keystroke-source replay seams land.
"""

from __future__ import annotations

import io
import json
import threading
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


def _utc_now_iso() -> str:
    return datetime.now(UTC).isoformat()


def _to_png_bytes(image: Any) -> bytes:
    """Normalise a captured image to PNG bytes.

    The skill-scan path already hands us PNG-encoded ``bytes`` (passed through
    unchanged, bit-identical to what the live engine produced). The repair
    path hands us a BGR ``uint8`` ndarray from :class:`ScreenCapturer`, which
    we convert to RGB and PNG-encode losslessly via Pillow.
    """
    if isinstance(image, (bytes, bytearray)):
        return bytes(image)

    import numpy as np
    from PIL import Image

    arr = np.asarray(image)
    if arr.ndim == 3 and arr.shape[2] == 3:
        arr = arr[:, :, ::-1]  # BGR -> RGB
    buf = io.BytesIO()
    Image.fromarray(arr).save(buf, format="PNG")
    return buf.getvalue()


class ChatlogTap:
    """Copies every tailed chat.log line verbatim into ``chat_replay.log``.

    Lines arrive exactly as the watcher's ``readline()`` produced them
    (trailing newline intact), so the written log replays byte-identically
    through the watcher's tail loop.
    """

    def __init__(self, path: Path) -> None:
        self._path = path
        self._lock = threading.Lock()
        self._handle = path.open("a", encoding="utf-8", newline="")
        self._count = 0

    def record_line(self, line: str) -> None:
        """Append one verbatim line and flush."""
        with self._lock:
            self._handle.write(line)
            self._handle.flush()
            self._count += 1

    @property
    def count(self) -> int:
        with self._lock:
            return self._count

    def close(self) -> None:
        with self._lock:
            self._handle.close()


class ScanCaptureTap:
    """Saves each captured scan image + a JSON sidecar into ``scan_captures/``.

    Files are sequence-numbered (``0001-skill.png`` + ``0001-skill.json``) so
    the on-disk order matches capture order. The sidecar carries the trigger
    panel, the source region, and a capture timestamp.
    """

    def __init__(self, directory: Path) -> None:
        self._dir = directory
        self._dir.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._seq = 0

    def record_capture(self, panel: str, region: Any, image: Any) -> None:
        """Persist one capture as ``<NNNN>-<panel>.png`` + JSON sidecar."""
        with self._lock:
            self._seq += 1
            seq = self._seq
        stem = f"{seq:04d}-{panel}"
        png_bytes = _to_png_bytes(image)
        (self._dir / f"{stem}.png").write_bytes(png_bytes)
        sidecar = {
            "seq": seq,
            "panel": panel,
            "region": region,
            "captured_at": _utc_now_iso(),
        }
        (self._dir / f"{stem}.json").write_text(
            json.dumps(sidecar, indent=2, sort_keys=True), encoding="utf-8"
        )

    @property
    def count(self) -> int:
        with self._lock:
            return self._seq


class KeystrokeTap:
    """Appends keystroke edges into ``keystrokes.jsonl``.

    Each record carries the key, the edge kind (``press`` / ``release``), a
    monotonic offset from the recording's start epoch (replay timing
    reference), and a wall-clock ISO timestamp.
    """

    def __init__(self, path: Path, monotonic_epoch: float) -> None:
        self._path = path
        self._epoch = monotonic_epoch
        self._lock = threading.Lock()
        self._handle = path.open("a", encoding="utf-8", newline="")
        self._count = 0

    def record_key(self, key: str, kind: str) -> None:
        """Append one keystroke edge and flush."""
        record = {
            "key": key,
            "kind": kind,
            "offset_s": round(time.monotonic() - self._epoch, 6),
            "wall": _utc_now_iso(),
        }
        with self._lock:
            self._handle.write(json.dumps(record, sort_keys=True) + "\n")
            self._handle.flush()
            self._count += 1

    @property
    def count(self) -> int:
        with self._lock:
            return self._count

    def close(self) -> None:
        with self._lock:
            self._handle.close()


class Recorder:
    """Bundles the three taps against one ``recording_in_progress`` directory.

    Construct one per active recording. The controller installs each tap's
    ``record_*`` method onto the matching live service, then calls
    :meth:`close` at stop to release the open file handles before the bundle
    is finalised.
    """

    def __init__(self, bundle_dir: Path, monotonic_epoch: float | None = None) -> None:
        self.bundle_dir = bundle_dir
        bundle_dir.mkdir(parents=True, exist_ok=True)
        epoch = time.monotonic() if monotonic_epoch is None else monotonic_epoch
        self.chatlog = ChatlogTap(bundle_dir / "chat_replay.log")
        self.scan = ScanCaptureTap(bundle_dir / "scan_captures")
        self.keystrokes = KeystrokeTap(bundle_dir / "keystrokes.jsonl", epoch)

    def status_counts(self) -> dict[str, int]:
        """Live counters for the recording-status surface."""
        return {
            "lines": self.chatlog.count,
            "captures": self.scan.count,
            "keystrokes": self.keystrokes.count,
        }

    def close(self) -> None:
        """Release the open file handles. Idempotent enough for finally-blocks."""
        self.chatlog.close()
        self.keystrokes.close()
