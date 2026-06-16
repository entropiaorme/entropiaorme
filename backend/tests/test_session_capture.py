"""Tests for the live-DB snapshot primitive and the recorder's DB capture
(starting/post-session snapshots)."""

from __future__ import annotations

import sqlite3
from typing import Any

import yaml

from backend.testing.recording_controller import RecordingController
from backend.testing.session_capture import snapshot_sqlite

CHAT_LINES = [
    "2026-05-19 10:00:00 [System] [] You inflicted 12.0 points of damage\n",
    "2026-05-19 10:00:01 [System] [] You received Shrapnel x (800) Value: 8.00 PED\n",
]


class _Tap:
    """A tap-able fake capturing the installed callable (mirrors the recorder's
    five injected services)."""

    def __init__(self) -> None:
        self.tap: Any = None

    def set_line_tap(self, tap) -> None:
        self.tap = tap

    def clear_line_tap(self) -> None:
        self.tap = None

    set_capture_tap = set_line_tap
    clear_capture_tap = clear_line_tap
    set_key_tap = set_line_tap
    clear_key_tap = clear_line_tap


def test_snapshot_sqlite_clones_a_live_db(tmp_path) -> None:
    src = sqlite3.connect(":memory:")
    src.execute("CREATE TABLE t (id INTEGER, name TEXT)")
    src.execute("INSERT INTO t VALUES (1, 'alpha')")
    src.commit()

    dest = tmp_path / "nested" / "snap.sqlite"
    snapshot_sqlite(src, dest)

    assert dest.is_file()
    clone = sqlite3.connect(str(dest))
    assert clone.execute("SELECT id, name FROM t").fetchall() == [(1, "alpha")]
    clone.close()
    # The online backup left the source usable (no lock-out, no close).
    src.execute("INSERT INTO t VALUES (2, 'beta')")
    src.commit()
    src.close()


def test_recording_controller_captures_starting_and_post_session_db(tmp_path) -> None:
    src = sqlite3.connect(":memory:", check_same_thread=False)
    src.execute("CREATE TABLE codex_claims (rank INTEGER)")
    src.commit()
    taps = [_Tap() for _ in range(5)]
    ctrl = RecordingController(
        chatlog_watcher=taps[0],  # type: ignore[arg-type]
        skill_scan_manual=taps[1],
        repair_ocr=taps[2],
        hotbar_listener=taps[3],
        spacebar_capture_listener=taps[4],
        corpus_root=tmp_path / "corpus",
        db_snapshot_writer=lambda dest: snapshot_sqlite(src, dest),
    )

    ctrl.start()
    in_progress = tmp_path / "corpus" / "recording_in_progress"
    assert (in_progress / "starting_db.sqlite").is_file(), "starting DB not captured"

    for line in CHAT_LINES:
        taps[0].tap(line)

    ctrl.stop({"scenario_name": "cap_test"})
    bundle = tmp_path / "corpus" / "recorded" / "cap_test"
    assert (bundle / "starting_db.sqlite").is_file()
    assert (bundle / "post_session_db.sqlite").is_file()
    assert (bundle / "chat_replay.log").is_file()
    meta = yaml.safe_load((bundle / "metadata.yaml").read_text(encoding="utf-8"))
    assert meta["db_captured"] is True
    assert meta["started_at"] and meta["stopped_at"]
    src.close()
