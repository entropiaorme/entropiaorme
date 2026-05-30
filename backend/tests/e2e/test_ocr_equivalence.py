"""OCR equivalence against the recorded ground-truth panels.

Runs the full production OCR pipeline (the real bundled SVTRv2 recogniser) over
the panels captured in the ``hunt_with_skill_scan`` recorded bundle and pins the
structured output against golden files within a numeric tolerance. Two real OCR
surfaces exist (skill panel and repair window), so the corpus covers exactly
those: one repair fixture (single cost-field parse), and skill fixtures for the
structurally distinct cases (a full page, the partial last page, and the
multi-page aggregation across the whole scan).

Local-by-default, like the recorded bundle it draws from. The panels are a real
account's data, so they are not committed: they are supplied locally and the
golden files this test writes are gitignored alongside them. With the panels
absent (a fresh clone, CI), the whole module skips, so the public surface and CI
stay green without the real captures. The ``full`` marker keeps it off the
per-PR leg regardless; it runs locally and on demand.

A model or device change that legitimately shifts a reading is a deliberate
re-ratification: regenerate the goldens with ``--force-regen`` and review the
diff. Inference drift across GPUs is out of scope: the goldens are pinned to
this host's recogniser output.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import pytest

from backend.services import local_ocr
from backend.services.repair_ocr import RepairOcrService
from backend.services.skill_scan_core import SkillScanCore
from backend.services.skill_scan_manual import SkillScanManual
from backend.testing.capturer import FixtureCapturer

E2E_DIR = Path(__file__).parent
SCAN_CAPTURES = (
    E2E_DIR / "corpus" / "recorded" / "hunt_with_skill_scan" / "scan_captures"
)
REPAIR_PNG = SCAN_CAPTURES / "0001-repair.png"
SKILL_PAGES = sorted(SCAN_CAPTURES.glob("*-skill.png"))

# The real panels are local-by-default; with them absent the surface this test
# exercises does not exist to assert against, so skip the module wholesale. The
# `full` marker additionally keeps it off the per-PR CI leg.
_HAVE_CORPUS = REPAIR_PNG.exists() and len(SKILL_PAGES) >= 1
pytestmark = [
    pytest.mark.full,
    pytest.mark.skipif(
        not _HAVE_CORPUS,
        reason="recorded OCR ground-truth corpus not present (local-by-default)",
    ),
]


@pytest.fixture(scope="module")
def ocr_engine():
    """The real bundled recogniser, or a skip when it cannot load on this host."""
    engine = local_ocr.get_engine()
    if engine is None:
        pytest.skip("local OCR engine unavailable on this host")
    return engine


def _sidecar(png_path: Path) -> dict:
    return json.loads(png_path.with_suffix(".json").read_text(encoding="utf-8"))


def _repair_region() -> tuple[list[int], list[int]]:
    """The recorded repair region as the ``(tl, br)`` pair ``repair_region`` returns."""
    r = _sidecar(REPAIR_PNG)["region"]
    tl = [r["x"], r["y"]]
    br = [r["x"] + r["w"], r["y"] + r["h"]]
    return tl, br


def _skill_region(png_path: Path) -> tuple[list[int], list[int]]:
    r = _sidecar(png_path)["region"]
    return r["tl"], r["br"]


def _stable_levels(levels: dict[str, float]) -> dict[str, float]:
    """Sort by skill name and round, so the golden is order- and jitter-stable.

    Rounding to two decimals is the numeric tolerance: same-host inference is
    deterministic, so reruns are exact; the rounding absorbs the sub-integer
    bar-fill estimate's ~1% resolution and only a genuine misread (a different
    integer level, a renamed skill) breaks the golden.
    """
    return {name: round(level, 2) for name, level in sorted(levels.items())}


def _assert_well_formed_levels(levels: dict) -> None:
    """A {skill: level} map must be string keys and finite, non-negative floats.

    No fixed upper bound: the panel reports accumulated skill points, which run
    into the tens of thousands for a mature account, so any ceiling below that
    would reject correct readings. The golden pins the exact values; this only
    guards the per-entry shape so a mutant that swaps a level's type or sign (or
    a key's type) is caught even when the count survives.
    """
    assert isinstance(levels, dict)
    for name, level in levels.items():
        assert isinstance(name, str) and name, name
        assert isinstance(level, float) and not isinstance(level, bool), name
        assert math.isfinite(level) and level >= 0, (name, level)


# ── Repair window: the full capture -> OCR -> parse pipeline through the seam ──


def test_repair_cost_equivalence(data_regression, monkeypatch, ocr_engine):
    """The repair cost reads consistently through the injected capture seam."""
    capturer = FixtureCapturer(REPAIR_PNG)
    monkeypatch.setattr("backend.ocr.capturer.ScreenCapturer", lambda: capturer)
    monkeypatch.setattr("backend.services.repair_ocr.repair_region", _repair_region)

    result = RepairOcrService(config_service=None).scan_repair_cost()

    assert "error" not in result
    # Pin the success-shape contract, not just the cost: a corruption that
    # returns the right number while blanking or renaming raw_text/confidence
    # must not survive.
    assert set(result) == {"cost_ped", "raw_text", "confidence"}
    cost = result["cost_ped"]
    assert isinstance(cost, (int, float)) and not isinstance(cost, bool)
    assert cost > 0  # the recorded repair panel reads a non-zero cost
    assert isinstance(result["raw_text"], str) and result["raw_text"]
    assert isinstance(result["confidence"], (int, float)) and not isinstance(
        result["confidence"], bool
    )
    data_regression.check({"cost_ped": round(cost, 2), "raw_text": result["raw_text"]})


# ── Skill panel: structured level output per distinct case ────────────────────


def test_skill_full_page_equivalence(data_regression, ocr_engine, tmp_path):
    """A full skill page extracts a stable {skill: level} map."""
    levels = SkillScanCore(None, tmp_path).extract_page_levels(
        SKILL_PAGES[0].read_bytes()
    )

    assert levels, "expected a full page to yield at least one skill"
    _assert_well_formed_levels(levels)
    data_regression.check(_stable_levels(levels))


def test_skill_last_page_equivalence(data_regression, ocr_engine, tmp_path):
    """The final captured page (a partial / edge page) extracts consistently."""
    levels = SkillScanCore(None, tmp_path).extract_page_levels(
        SKILL_PAGES[-1].read_bytes()
    )

    assert levels, "expected the last page to yield at least one skill"
    _assert_well_formed_levels(levels)
    data_regression.check(_stable_levels(levels))


def test_skill_multipage_aggregation_equivalence(data_regression, ocr_engine, tmp_path):
    """Aggregating every captured page yields a stable merged skill map."""
    captures: list[bytes | None] = [p.read_bytes() for p in SKILL_PAGES]
    result = SkillScanManual(None, tmp_path)._extract_levels(captures)

    assert "skills" in result, result.get("error")
    skills = result["skills"]
    assert skills, "expected aggregation to yield at least one skill"
    _assert_well_formed_levels(skills)
    # Every input page is accounted for, so a mutant that drops or double-counts
    # a capture is caught.
    assert result["pages_processed"] == len(captures)

    data_regression.check(_stable_levels(skills))


# ── Seam transparency: capture through the FixtureCapturer == direct bytes ────


def test_skill_capture_seam_is_transparent(monkeypatch, ocr_engine, tmp_path):
    """Driving the real capture path through the seam matches direct extraction."""
    page = SKILL_PAGES[0]
    capturer = FixtureCapturer(page)
    monkeypatch.setattr(
        "backend.services.skill_scan_core.ScreenCapturer", lambda: capturer
    )
    core = SkillScanCore(None, tmp_path)

    tl, br = _skill_region(page)
    captured_png = core.capture_region(tl, br)

    assert captured_png == page.read_bytes()  # seam serves the bytes verbatim
    assert core.extract_page_levels(captured_png) == core.extract_page_levels(
        page.read_bytes()
    )
