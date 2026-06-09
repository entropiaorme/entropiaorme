"""Tests for the canonical screen-capture interface and its skill-scan consumer.

The capturer's ``mss`` session is stubbed out (no display is touched): the
PNG path lets the real ``mss.tools.to_png`` serialise a synthetic grab so the
RGB-in / BGR-out round-trip is asserted end to end, while the skill-scan tests
swap the whole ``ScreenCapturer`` for a recorder to pin the ``(tl, br)`` →
``x/y/w/h`` adaptation and the ``None``-on-failure contract.
"""

import logging
from typing import ClassVar

import cv2
import numpy as np
import pytest

from backend.ocr.capturer import ScreenCapturer
from backend.services.skill_scan_core import SkillScanCore
from backend.testing.capturer import SequencedFixtureCapturer


class _FakeShot:
    """Stand-in for an ``mss`` ScreenShot: raw RGB bytes plus a size."""

    def __init__(self, rgb: bytes, size: tuple[int, int]):
        self.rgb = rgb
        self.size = size


class _FakeSession:
    """Records the monitor dict and returns a preset grab result."""

    def __init__(self, shot):
        self._shot = shot
        self.last_monitor: dict | None = None

    def grab(self, monitor):
        self.last_monitor = monitor
        return self._shot


def _capturer_returning(shot) -> tuple[ScreenCapturer, _FakeSession]:
    """A ScreenCapturer whose thread-local session is the given fake."""
    cap = ScreenCapturer()
    session = _FakeSession(shot)
    cap._sct = lambda: session  # type: ignore[method-assign]
    return cap, session


# ── ScreenCapturer.capture_region_png ────────────────────────────────────────


def test_capture_region_png_round_trips_rgb_to_bgr():
    """to_png serialises RGB; an IMREAD_COLOR decode reads it back as BGR."""
    width, height = 2, 2
    pixel_rgb = (10, 20, 30)
    shot = _FakeShot(bytes(pixel_rgb) * (width * height), (width, height))
    cap, _ = _capturer_returning(shot)

    png = cap.capture_region_png(0, 0, width, height)

    decoded = cv2.imdecode(np.frombuffer(png, dtype=np.uint8), cv2.IMREAD_COLOR)
    assert decoded is not None
    assert decoded.shape == (height, width, 3)
    assert decoded[0, 0].tolist() == [30, 20, 10]  # RGB (10,20,30) -> BGR


def test_capture_region_png_passes_int_monitor():
    shot = _FakeShot(bytes((0, 0, 0)) * 6, (3, 2))
    cap, session = _capturer_returning(shot)

    cap.capture_region_png(5, 7, 3, 2)

    assert session.last_monitor == {"left": 5, "top": 7, "width": 3, "height": 2}


@pytest.mark.parametrize("width,height", [(0, 5), (5, 0), (-1, 5), (5, -1)])
def test_capture_region_png_raises_on_nonpositive_dims(width, height):
    cap, _ = _capturer_returning(_FakeShot(b"", (0, 0)))
    with pytest.raises(ValueError):
        cap.capture_region_png(0, 0, width, height)


# ── SkillScanCore.capture_region delegation ──────────────────────────────────


class _RecordingCapturer:
    """Stub ScreenCapturer counting instantiations and recording PNG args."""

    instances: ClassVar[int] = 0
    calls: ClassVar[list[tuple[int, int, int, int]]] = []

    def __init__(self):
        type(self).instances += 1

    def capture_region_png(self, x, y, w, h) -> bytes:
        type(self).calls.append((x, y, w, h))
        return b"PNGBYTES"


@pytest.fixture
def recording_capturer(monkeypatch):
    _RecordingCapturer.instances = 0
    _RecordingCapturer.calls = []
    monkeypatch.setattr(
        "backend.services.skill_scan_core.ScreenCapturer", _RecordingCapturer
    )
    return _RecordingCapturer


def _core(tmp_path) -> SkillScanCore:
    return SkillScanCore(config_service=None, data_dir=tmp_path)


def test_capture_region_adapts_corners_and_delegates(tmp_path, recording_capturer):
    core = _core(tmp_path)

    out = core.capture_region([100, 50], [40, 80])

    assert out is not None and len(out) > 0
    assert out == b"PNGBYTES"
    # left=min(100,40), top=min(50,80), width=|40-100|, height=|80-50|
    assert recording_capturer.calls == [(40, 50, 60, 30)]


def test_capture_region_reuses_single_capturer(tmp_path, recording_capturer):
    core = _core(tmp_path)
    core.capture_region([0, 0], [10, 10])
    core.capture_region([0, 0], [10, 10])
    assert recording_capturer.instances == 1


@pytest.mark.parametrize("tl,br", [(None, None), ([0, 0], None), (None, [10, 10])])
def test_capture_region_none_on_missing_corners(tmp_path, recording_capturer, tl, br):
    assert _core(tmp_path).capture_region(tl, br) is None
    assert recording_capturer.calls == []


def test_capture_region_none_on_empty_region(tmp_path, recording_capturer):
    """tl == br collapses to a zero-area region, refused before any grab."""
    assert _core(tmp_path).capture_region([10, 10], [10, 10]) is None
    assert recording_capturer.calls == []


def test_capture_region_uses_injected_factory_over_module_symbol(
    tmp_path, recording_capturer
):
    """A constructor-injected capturer factory wins over the module symbol.

    The composition root wires a fixture-backed factory under test mode; the
    production ``ScreenCapturer`` symbol (patched here to the recording stub)
    must never be resolved, and the factory's capturer is cached like the
    production one.
    """
    served: list[tuple[int, int, int, int]] = []
    resolutions: list[int] = []

    class _Injected:
        def capture_region_png(self, x, y, w, h) -> bytes:
            served.append((x, y, w, h))
            return b"FIXTURE"

    def _factory() -> _Injected:
        resolutions.append(1)
        return _Injected()

    core = SkillScanCore(
        config_service=None, data_dir=tmp_path, capturer_factory=_factory
    )

    assert core.capture_region([0, 0], [10, 10]) == b"FIXTURE"
    assert core.capture_region([0, 0], [10, 10]) == b"FIXTURE"

    assert resolutions == [1]  # resolved once; the capturer is cached
    assert served == [(0, 0, 10, 10), (0, 0, 10, 10)]
    assert recording_capturer.instances == 0  # module symbol never touched


def test_capture_region_none_on_capture_failure(tmp_path, monkeypatch, caplog):
    """A capturer that raises (e.g. mss unavailable) yields None, not an error.

    Distinct from the bad-input None paths: a valid region is adapted and the
    capturer instantiation is attempted, so the exception branch logs the
    failure with the adapted coordinates before returning None.
    """
    attempts: list[int] = []

    class _Boom:
        def __init__(self):
            attempts.append(1)
            raise ImportError("mss is required for screen capture")

    monkeypatch.setattr("backend.services.skill_scan_core.ScreenCapturer", _Boom)
    with caplog.at_level(logging.ERROR, logger="backend.services.skill_scan_core"):
        assert _core(tmp_path).capture_region([0, 0], [10, 10]) is None
    # The exception path was taken, not silent input rejection: instantiation
    # was attempted and the failure was logged with the adapted region.
    assert attempts == [1]
    assert any(
        "capture failed for region (0, 0, 10, 10)" in record.message
        for record in caplog.records
    )


# ── SequencedFixtureCapturer ──────────────────────────────────────────────────


def test_sequenced_capturer_serves_panel_fixtures_in_name_order(tmp_path):
    """Each capture call serves the next fixture of the bound panel type.

    The PNG bytes come back verbatim (the skill-scan path), in name order,
    and the other panel's fixtures are invisible to this sequence.
    """
    (tmp_path / "0002-skill.png").write_bytes(b"PAGE2")
    (tmp_path / "0010-skill.png").write_bytes(b"PAGE10")
    (tmp_path / "0001-repair.png").write_bytes(b"REPAIR")

    seq = SequencedFixtureCapturer(tmp_path, "skill")

    assert seq.capture_region_png(0, 0, 10, 10) == b"PAGE2"
    assert seq.capture_region_png(0, 0, 10, 10) == b"PAGE10"


def test_sequenced_capturer_raises_past_the_recorded_end(tmp_path):
    """A capture past the recorded series fails loudly with both counts.

    The consumers' failure contracts turn this into a logged scan failure;
    test mode must never silently fall back to the real screen.
    """
    (tmp_path / "0001-skill.png").write_bytes(b"PAGE1")
    seq = SequencedFixtureCapturer(tmp_path, "skill")
    seq.capture_region_png(0, 0, 10, 10)

    with pytest.raises(
        ValueError, match=r"no skill fixture remaining at capture 2 \(recorded: 1\)"
    ):
        seq.capture_region_png(0, 0, 10, 10)


def test_sequenced_capturer_tolerates_absent_fixtures(tmp_path):
    """A missing directory (or none at all) constructs fine and fails on use.

    Chat-only scenarios record no captures, so the composition root must be
    able to wire the capturer unconditionally; only an actual capture call
    surfaces the absence.
    """
    for seq in (
        SequencedFixtureCapturer(tmp_path / "absent", "repair"),
        SequencedFixtureCapturer(None, "repair"),
    ):
        with pytest.raises(ValueError, match="no repair fixture remaining"):
            seq.capture_region(0, 0, 10, 10)


def test_sequenced_capturer_repair_path_decodes_the_recorded_frame(tmp_path):
    """The repair path gets the recorded frame back as a BGR ndarray."""
    frame = np.zeros((4, 4, 3), dtype=np.uint8)
    frame[:, :] = (10, 20, 30)
    ok, buf = cv2.imencode(".png", frame)
    assert ok
    (tmp_path / "0001-repair.png").write_bytes(buf.tobytes())

    seq = SequencedFixtureCapturer(tmp_path, "repair")
    out = seq.capture_region(0, 0, 4, 4)

    assert np.array_equal(out, frame)
