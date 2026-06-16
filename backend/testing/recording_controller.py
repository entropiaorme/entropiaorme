"""Recording lifecycle controller.

A process-singleton that drives recording mode: install the three taps onto
the live services, manage the ``recording_in_progress/`` staging directory,
capture stop-time metadata, finalise the bundle into ``corpus/recorded/`` with
an atomic rename, and run the chatlog determinism verification.

State machine: ``idle`` -> ``recording`` -> ``finalising`` -> ``idle``. The
controller refuses to start while already recording and refuses to stop while
idle. ``scenario_name`` validity and target-collision are checked before any
tap is uninstalled, so a rejected stop leaves recording cleanly in progress
for a corrected retry.

Determinism boundary: only ``chat_replay.log`` has a replay-side consumer, so
the verification step replays the recorded chatlog through a throwaway
pipeline twice (generate goldens, then re-assert). ``scan_captures/`` and
``keystrokes.jsonl`` are preserved in the bundle but not golden-verified here.
"""

from __future__ import annotations

import logging
import os
import re
import shutil
import sqlite3
import tempfile
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

import yaml

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.golden import GoldenAssertionFailure, GoldenSet
from backend.testing.recorder import Recorder, _utc_now_iso
from backend.testing.replay import replay_scenario, wait_for_drain
from backend.tracking.tracker import HuntTracker

log = logging.getLogger(__name__)

_SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9_]*$")


class RecordingStateError(RuntimeError):
    """Raised when an operation is invalid for the current recording state
    (e.g. start while already recording, or a scenario-name collision)."""


class RecordingValidationError(ValueError):
    """Raised when stop-time metadata fails validation (e.g. a bad slug)."""


def _validate_slug(name: str) -> str:
    """Return ``name`` if it is a safe scenario directory slug, else raise."""
    name = (name or "").strip()
    if not _SLUG_RE.match(name):
        raise RecordingValidationError(
            "scenario_name must be a lowercase slug "
            "(letters, digits, underscores; no path separators)"
        )
    return name


class RecordingController:
    """Owns the recording lifecycle against a set of live services."""

    def __init__(
        self,
        *,
        chatlog_watcher: ChatlogWatcher,
        skill_scan_manual: Any,
        repair_ocr: Any,
        hotbar_listener: Any,
        spacebar_capture_listener: Any,
        corpus_root: Path,
        db_snapshot_writer: Callable[[Path], None] | None = None,
    ) -> None:
        self._chatlog_watcher = chatlog_watcher
        self._skill_scan_manual = skill_scan_manual
        self._repair_ocr = repair_ocr
        self._hotbar_listener = hotbar_listener
        self._spacebar_capture_listener = spacebar_capture_listener
        # Optional: snapshot the live DB into the bundle at session start
        # (the pre-segment image) and stop (the post-session image), so the
        # replay cross-check can replay from the starting snapshot. The
        # writer owns the app DB lock; absent it (e.g. unit tests), DB capture
        # is simply skipped and the bundle carries only the chatlog segment.
        self._db_snapshot_writer = db_snapshot_writer

        self._corpus_root = corpus_root
        self._in_progress_dir = corpus_root / "recording_in_progress"
        self._recorded_dir = corpus_root / "recorded"

        self._lock = threading.Lock()
        self._state = "idle"
        self._recorder: Recorder | None = None
        self._started_at: str | None = None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def start(self) -> dict:
        """Begin a recording: clear stale staging, install taps, start capture."""
        with self._lock:
            if self._state != "idle":
                raise RecordingStateError(
                    f"Cannot start: recording state is {self._state!r}"
                )
            self._clear_in_progress()
            self._recorder = Recorder(self._in_progress_dir)
            self._install_taps(self._recorder)
            # Capture the pre-segment DB image before any tapped line lands, so
            # the replay can start from the true starting state.
            if self._db_snapshot_writer is not None:
                self._db_snapshot_writer(self._in_progress_dir / "starting_db.sqlite")
            self._state = "recording"
            self._started_at = _utc_now_iso()
            log.info("Recording started -> %s", self._in_progress_dir)
            return self._status_locked()

    def status(self) -> dict:
        """Current recording state plus live capture counters."""
        with self._lock:
            return self._status_locked()

    def stop(self, metadata: dict) -> dict:
        """Finalise the recording into ``corpus/recorded/<scenario_name>/``.

        Validates the scenario name and target collision while still
        recording (a rejection leaves capture running for a retry), then
        uninstalls the taps, writes metadata, atomically renames the bundle,
        and runs the chatlog determinism verification.
        """
        scenario_name = _validate_slug(metadata.get("scenario_name", ""))
        with self._lock:
            if self._state != "recording":
                raise RecordingStateError(
                    f"Cannot stop: recording state is {self._state!r}"
                )
            target = self._recorded_dir / scenario_name
            if target.exists():
                raise RecordingStateError(f"Scenario {scenario_name!r} already exists")
            recorder = self._recorder
            assert recorder is not None
            self._state = "finalising"

        # Past this point capture has stopped; commit the bundle.
        try:
            self._uninstall_taps()
            counts = recorder.status_counts()
            recorder.close()
            # Capture the post-session DB image (capture has stopped, so the
            # DB is at the segment's final state): the reference the
            # cross-check diffs the offline replay against.
            if self._db_snapshot_writer is not None:
                self._db_snapshot_writer(
                    self._in_progress_dir / "post_session_db.sqlite"
                )
            self._write_metadata(self._in_progress_dir, scenario_name, metadata, counts)
            self._recorded_dir.mkdir(parents=True, exist_ok=True)
            os.replace(self._in_progress_dir, target)
        except Exception as exc:  # noqa: BLE001 — surfaced as a recovery result
            log.exception("Recording finalisation failed")
            with self._lock:
                self._recorder = None
                self._started_at = None
                self._state = "idle"
            return {
                "error": str(exc),
                "recovery_path": str(self._in_progress_dir),
            }

        determinism = self._verify_determinism(target, metadata)

        with self._lock:
            self._recorder = None
            self._started_at = None
            self._state = "idle"
        log.info("Recording finalised -> %s (%s)", target, determinism["determinism"])
        return {"finalized_path": str(target), **determinism}

    def abort(self) -> dict:
        """Discard the in-flight recording without finalising it."""
        with self._lock:
            if self._state == "idle":
                return {"state": "idle"}
            self._state = "finalising"
            recorder = self._recorder
        self._uninstall_taps()
        if recorder is not None:
            recorder.close()
        self._clear_in_progress()
        with self._lock:
            self._recorder = None
            self._started_at = None
            self._state = "idle"
        log.info("Recording aborted; staging cleared")
        return {"state": "idle"}

    @property
    def is_recording(self) -> bool:
        with self._lock:
            return self._state != "idle"

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _status_locked(self) -> dict:
        counts = (
            self._recorder.status_counts()
            if self._recorder is not None
            else {"lines": 0, "captures": 0, "keystrokes": 0}
        )
        return {
            "state": self._state,
            "started_at": self._started_at,
            **counts,
        }

    def _install_taps(self, recorder: Recorder) -> None:
        self._chatlog_watcher.set_line_tap(recorder.chatlog.record_line)
        self._skill_scan_manual.set_capture_tap(recorder.scan.record_capture)
        self._repair_ocr.set_capture_tap(recorder.scan.record_capture)
        self._hotbar_listener.set_key_tap(recorder.keystrokes.record_key)
        self._spacebar_capture_listener.set_key_tap(recorder.keystrokes.record_key)

    def _uninstall_taps(self) -> None:
        # Best-effort: a fault clearing one tap must not strand the others.
        for clear in (
            self._chatlog_watcher.clear_line_tap,
            self._skill_scan_manual.clear_capture_tap,
            self._repair_ocr.clear_capture_tap,
            self._hotbar_listener.clear_key_tap,
            self._spacebar_capture_listener.clear_key_tap,
        ):
            try:
                clear()
            except Exception:
                log.exception("Failed to clear a recording tap")

    def _clear_in_progress(self) -> None:
        if self._in_progress_dir.exists():
            log.warning("Clearing stale recording staging at %s", self._in_progress_dir)
            shutil.rmtree(self._in_progress_dir, ignore_errors=True)

    def _write_metadata(
        self, bundle_dir: Path, scenario_name: str, metadata: dict, counts: dict
    ) -> None:
        doc = {
            "name": scenario_name,
            "flavour": "recorded",
            "description": metadata.get("description", ""),
            "recorded_at": _utc_now_iso(),
            # The session's real instants (the replay clock schedule) and
            # whether the starting/post-session DB images were captured.
            "started_at": self._started_at,
            "stopped_at": _utc_now_iso(),
            "db_captured": self._db_snapshot_writer is not None,
            "surfaces": metadata.get("surfaces", []),
            "character_context": metadata.get("character_context", {}),
            "rare_event_flags": metadata.get("rare_event_flags", []),
            "counts": {
                "chat_lines": counts.get("lines", 0),
                "scan_captures": counts.get("captures", 0),
                "keystrokes": counts.get("keystrokes", 0),
            },
            "notes": metadata.get("notes", ""),
            "verification": (
                "Only chat_replay.log is golden-verified. scan_captures/ and "
                "keystrokes.jsonl are preserved but their replay verification "
                "is not yet wired."
            ),
        }
        (bundle_dir / "metadata.yaml").write_text(
            yaml.safe_dump(doc, sort_keys=False, allow_unicode=True),
            encoding="utf-8",
        )

    def _verify_determinism(self, scenario_dir: Path, metadata: dict) -> dict:
        """Replay the recorded chatlog twice: generate goldens, then re-assert.

        A divergence between the two replays is a determinism leak (in the
        recording or a production code path), surfaced as a diff for the
        developer rather than ratified.
        """
        player_name = (metadata.get("character_context") or {}).get("player_name", "")
        self._run_pipeline(scenario_dir, update=True, player_name=player_name)
        try:
            self._run_pipeline(scenario_dir, update=False, player_name=player_name)
        except GoldenAssertionFailure as exc:
            return {"determinism": "leak", "diff": str(exc)}
        return {"determinism": "ok"}

    @staticmethod
    def _run_pipeline(scenario_dir: Path, *, update: bool, player_name: str) -> None:
        """Replay ``scenario_dir/chat_replay.log`` through a throwaway pipeline."""
        db = sqlite3.connect(":memory:", check_same_thread=False)
        try:
            bus = EventBus()
            tracker = HuntTracker(bus, db, player_name=player_name)
            goldens = GoldenSet(scenario_dir, update=update)
            goldens.recorder.install(bus)
            with tempfile.TemporaryDirectory() as td:
                chatlog = Path(td) / "chat.log"
                chatlog.touch()
                watcher = ChatlogWatcher(bus, chatlog)
                watcher.start()
                try:
                    tracker.start_session()
                    replay_scenario(scenario_dir, chatlog)
                    wait_for_drain(watcher, chatlog)
                finally:
                    watcher.stop()
                tracker.stop_session()
            goldens.assert_matches(db)
        finally:
            db.close()
