"""Tests for the recording lifecycle controller.

The five tapped services are injected, so these tests use lightweight fakes
that capture the installed tap callable. The chatlog fake's captured tap is
driven directly to feed valid chat lines into the real bundle, so stop() has
a genuine chat_replay.log to finalise and determinism-verify. The determinism
step runs the real replay pipeline on an in-memory DB.
"""

from __future__ import annotations

import yaml

from backend.testing.golden import GoldenAssertionFailure
from backend.testing.recording_controller import (
    RecordingController,
    RecordingStateError,
    RecordingValidationError,
)

CHAT_LINES = [
    "2026-05-19 10:00:00 [System] [] You inflicted 12.0 points of damage\n",
    "2026-05-19 10:00:01 [System] [] You received Shrapnel x (800) Value: 8.00 PED\n",
]


class _FakeChatlog:
    def __init__(self):
        self.tap = None

    def set_line_tap(self, tap):
        self.tap = tap

    def clear_line_tap(self):
        self.tap = None


class _FakeScan:
    def __init__(self):
        self.tap = None

    def set_capture_tap(self, tap):
        self.tap = tap

    def clear_capture_tap(self):
        self.tap = None


class _FakeKeys:
    def __init__(self):
        self.tap = None

    def set_key_tap(self, tap):
        self.tap = tap

    def clear_key_tap(self):
        self.tap = None


def _make_controller(tmp_path):
    chatlog, skill, repair, hotbar, spacebar = (
        _FakeChatlog(),
        _FakeScan(),
        _FakeScan(),
        _FakeKeys(),
        _FakeKeys(),
    )
    ctrl = RecordingController(
        chatlog_watcher=chatlog,  # type: ignore[arg-type]
        skill_scan_manual=skill,
        repair_ocr=repair,
        hotbar_listener=hotbar,
        spacebar_capture_listener=spacebar,
        corpus_root=tmp_path / "corpus",
    )
    return ctrl, chatlog, skill, repair, hotbar, spacebar


def _feed(chatlog, lines=CHAT_LINES):
    for line in lines:
        chatlog.tap(line)


def test_start_installs_taps_and_reports_recording(tmp_path):
    ctrl, chatlog, skill, repair, hotbar, spacebar = _make_controller(tmp_path)
    status = ctrl.start()
    assert status["state"] == "recording"
    assert status["started_at"]
    for svc in (chatlog, skill, repair, hotbar, spacebar):
        assert svc.tap is not None


def test_double_start_raises(tmp_path):
    ctrl, *_ = _make_controller(tmp_path)
    ctrl.start()
    try:
        ctrl.start()
        raise AssertionError("expected RecordingStateError")
    except RecordingStateError:
        pass


def test_stop_finalises_bundle_and_verifies_determinism(tmp_path):
    ctrl, chatlog, skill, *_ = _make_controller(tmp_path)
    ctrl.start()
    _feed(chatlog)

    result = ctrl.stop(
        {
            "scenario_name": "my_recording",
            "description": "a worked recording",
            "surfaces": ["tracking-kill-creation"],
        }
    )

    assert result["determinism"] == "ok"
    target = tmp_path / "corpus" / "recorded" / "my_recording"
    assert (target / "chat_replay.log").read_text(encoding="utf-8") == "".join(
        CHAT_LINES
    )
    assert (target / "expected" / "fingerprint.jsonl").exists()
    assert (target / "expected" / "db_state.json").exists()

    meta = yaml.safe_load((target / "metadata.yaml").read_text(encoding="utf-8"))
    assert meta["name"] == "my_recording"
    assert meta["flavour"] == "recorded"
    assert meta["counts"]["chat_lines"] == len(CHAT_LINES)

    assert ctrl.status()["state"] == "idle"
    # Taps cleared on finalise.
    assert chatlog.tap is None and skill.tap is None


def test_stop_refuses_existing_scenario_name(tmp_path):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    (tmp_path / "corpus" / "recorded" / "dup").mkdir(parents=True)
    ctrl.start()
    _feed(chatlog)
    try:
        ctrl.stop({"scenario_name": "dup"})
        raise AssertionError("expected RecordingStateError")
    except RecordingStateError:
        pass
    # Rejected before taps were touched: still recording, retryable.
    assert ctrl.status()["state"] == "recording"
    assert chatlog.tap is not None
    ctrl.abort()


def test_stop_rejects_bad_slug(tmp_path):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    ctrl.start()
    try:
        ctrl.stop({"scenario_name": "Bad Name"})
        raise AssertionError("expected RecordingValidationError")
    except RecordingValidationError:
        pass
    assert ctrl.status()["state"] == "recording"
    ctrl.abort()


def test_abort_discards_bundle(tmp_path):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    ctrl.start()
    _feed(chatlog)
    assert (tmp_path / "corpus" / "recording_in_progress").exists()

    ctrl.abort()

    assert ctrl.status()["state"] == "idle"
    assert chatlog.tap is None
    assert not (tmp_path / "corpus" / "recording_in_progress").exists()
    assert not (tmp_path / "corpus" / "recorded").exists()


def test_start_clears_stale_staging(tmp_path):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    stale = tmp_path / "corpus" / "recording_in_progress"
    stale.mkdir(parents=True)
    (stale / "junk.txt").write_text("leftover")

    ctrl.start()

    assert not (stale / "junk.txt").exists()
    ctrl.abort()


def test_finalisation_failure_leaves_bundle_and_clears_taps(tmp_path, monkeypatch):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    ctrl.start()
    _feed(chatlog)

    import backend.testing.recording_controller as rc

    def _boom(*_a, **_k):
        raise OSError("rename failed")

    monkeypatch.setattr(rc.os, "replace", _boom)

    result = ctrl.stop({"scenario_name": "doomed"})

    assert "error" in result
    assert result["recovery_path"].endswith("recording_in_progress")
    assert ctrl.status()["state"] == "idle"
    assert chatlog.tap is None  # taps cleared despite the failure
    assert (tmp_path / "corpus" / "recording_in_progress").exists()


def test_determinism_leak_surfaced(tmp_path):
    ctrl, chatlog, *_ = _make_controller(tmp_path)
    ctrl.start()
    _feed(chatlog)

    def _fake_pipeline(scenario_dir, *, update, player_name):
        if update:
            (scenario_dir / "expected").mkdir(parents=True, exist_ok=True)
            (scenario_dir / "expected" / "fingerprint.jsonl").write_text("{}\n")
            (scenario_dir / "expected" / "db_state.json").write_text("{}")
        else:
            raise GoldenAssertionFailure("leaky", "fingerprint drift", None)

    ctrl._run_pipeline = _fake_pipeline  # shadow the staticmethod for this test

    result = ctrl.stop({"scenario_name": "leaky"})

    assert result["determinism"] == "leak"
    assert "diff" in result
    # Bundle preserved for inspection.
    assert (tmp_path / "corpus" / "recorded" / "leaky").exists()
