"""Tests for the recording-mode taps and bundle writer.

The chatlog tap is exercised through a real ``ChatlogWatcher`` (the chatlog is
the surface the controller's determinism step replay-verifies). The scan and
keystroke taps are exercised directly: their service-side wiring is a guarded
one-line call, and their replay consumers do not exist yet, so direct coverage
of the bundle-writing mechanism is the meaningful unit here.
"""

from __future__ import annotations

import json
import time
from pathlib import Path

import numpy as np
from PIL import Image

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.recorder import KeystrokeTap, Recorder, ScanCaptureTap
from backend.testing.replay import wait_for_drain

COMBAT_LINES = [
    "2026-05-19 10:00:00 [System] [] You inflicted 12.0 points of damage\n",
    "2026-05-19 10:00:01 [System] [] You inflicted 8.0 points of damage\n",
]


def _stream(path: Path, lines: list[str]) -> None:
    """Append + flush each line so the watcher tails complete lines."""
    with path.open("a", encoding="utf-8") as sink:
        for line in lines:
            sink.write(line)
            sink.flush()


def test_chatlog_tap_records_verbatim_and_keeps_publishing(tmp_path):
    """The line tap copies every tailed line verbatim while the watcher's
    normal publish path still fires (the tap is additive, not a replacement)."""
    chatlog = tmp_path / "chat.log"
    chatlog.touch()
    recorder = Recorder(tmp_path / "bundle")

    bus = EventBus()
    combat: list = []
    bus.subscribe(EVENT_COMBAT, combat.append)

    watcher = ChatlogWatcher(bus, chatlog)
    watcher.set_line_tap(recorder.chatlog.record_line)
    watcher.start()
    try:
        _stream(chatlog, COMBAT_LINES)
        wait_for_drain(watcher, chatlog)
    finally:
        watcher.stop()
        recorder.close()

    recorded = (recorder.bundle_dir / "chat_replay.log").read_text(encoding="utf-8")
    assert recorded == "".join(COMBAT_LINES)
    assert recorder.chatlog.count == len(COMBAT_LINES)
    # Two damage lines on distinct timestamps → two combat events still published.
    assert len(combat) == 2


def test_chatlog_tap_clear_reverts_to_no_capture(tmp_path):
    """Clearing the tap stops capture without disturbing the watcher."""
    chatlog = tmp_path / "chat.log"
    chatlog.touch()
    recorder = Recorder(tmp_path / "bundle")

    watcher = ChatlogWatcher(EventBus(), chatlog)
    watcher.set_line_tap(recorder.chatlog.record_line)
    watcher.clear_line_tap()
    watcher.start()
    try:
        _stream(chatlog, COMBAT_LINES)
        wait_for_drain(watcher, chatlog)
    finally:
        watcher.stop()
        recorder.close()

    assert recorder.chatlog.count == 0
    assert (recorder.bundle_dir / "chat_replay.log").read_text(encoding="utf-8") == ""


def test_scan_capture_tap_writes_png_bytes_and_sidecar(tmp_path):
    """Skill-path PNG bytes are written through unchanged with a JSON sidecar."""
    tap = ScanCaptureTap(tmp_path / "scan_captures")
    png = b"\x89PNG\r\n\x1a\nfake-but-opaque-bytes"
    tap.record_capture("skill", {"tl": [0, 0], "br": [10, 10]}, png)

    out = tmp_path / "scan_captures"
    assert (out / "0001-skill.png").read_bytes() == png
    sidecar = json.loads((out / "0001-skill.json").read_text(encoding="utf-8"))
    assert sidecar["panel"] == "skill"
    assert sidecar["region"] == {"tl": [0, 0], "br": [10, 10]}
    assert sidecar["seq"] == 1
    assert "captured_at" in sidecar
    assert tap.count == 1


def test_scan_capture_tap_encodes_bgr_ndarray_to_rgb_png(tmp_path):
    """Repair-path BGR ndarrays are losslessly PNG-encoded, BGR→RGB corrected."""
    tap = ScanCaptureTap(tmp_path / "caps")
    frame = np.zeros((4, 6, 3), dtype=np.uint8)
    frame[:, :, 0] = 255  # blue in BGR

    tap.record_capture("repair", {"x": 0, "y": 0, "w": 6, "h": 4}, frame)

    img = Image.open(tap_dir := tmp_path / "caps" / "0001-repair.png")
    assert img.size == (6, 4)  # PIL size is (width, height)
    assert img.convert("RGB").getpixel((0, 0)) == (0, 0, 255)  # BGR blue → RGB blue
    assert tap_dir.with_suffix(".json").exists()


def test_scan_capture_tap_sequences_multiple_captures(tmp_path):
    tap = ScanCaptureTap(tmp_path / "caps")
    tap.record_capture("skill", {}, b"a")
    tap.record_capture("skill", {}, b"b")
    out = tmp_path / "caps"
    assert (out / "0001-skill.png").read_bytes() == b"a"
    assert (out / "0002-skill.png").read_bytes() == b"b"
    assert tap.count == 2


def test_keystroke_tap_appends_jsonl(tmp_path):
    path = tmp_path / "keystrokes.jsonl"
    tap = KeystrokeTap(path, time.monotonic())
    tap.record_key("1", "press")
    tap.record_key("space", "press")
    tap.record_key("space", "release")
    tap.close()

    records = [
        json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()
    ]
    assert [(r["key"], r["kind"]) for r in records] == [
        ("1", "press"),
        ("space", "press"),
        ("space", "release"),
    ]
    assert all(r["offset_s"] >= 0 for r in records)
    assert tap.count == 3


def test_recorder_status_counts(tmp_path):
    recorder = Recorder(tmp_path / "bundle")
    recorder.chatlog.record_line("a\n")
    recorder.keystrokes.record_key("1", "press")
    recorder.scan.record_capture("skill", {}, b"png")
    assert recorder.status_counts() == {"lines": 1, "captures": 1, "keystrokes": 1}
    recorder.close()
