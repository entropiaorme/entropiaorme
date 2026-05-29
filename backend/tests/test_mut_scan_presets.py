"""Mutation-hardening tests for ``backend.services.scan_presets``.

Targets the surviving / no-test mutants in the scan_presets cluster:

* ``_load_geometry`` - the existence guard, the JSON read (path / encoding) and
  the exact warning emitted when the data file is present but unreadable.
* ``_parse_cell`` - every field copied off the JSON cell entry.
* ``_build_anchor`` - the parsed cell actually stored in the anchor's dict.
* ``_compute_region`` - the hwnd threaded into the geometry lookup and the
  degeneracy guard's boundary (``<=`` vs ``<`` and ``or`` vs ``and``).
* ``skill_region`` / ``profession_region`` / ``repair_region`` - each preset
  passing its own anchor (not ``None``) into ``_compute_region``.
* ``game_window_present`` - the ``is not None`` truth direction.

The module reads window geometry through ``find_game_window`` /
``get_window_geometry`` (imported into the module namespace from
``eu_window``); tests substitute those two lookups rather than touch a device.
"""

from __future__ import annotations

import json
import logging
from unittest.mock import patch

import pytest

from backend.services import scan_presets as sp
from backend.services.scan_presets import CellGeometry, PanelAnchor

# The exact warning the production module emits when the data file is present
# but unreadable. Reproduced here so any drift in the format string, its args,
# or its casing is caught.
_WARN_TEMPLATE = "panel_geometry.json unreadable, using fallback constants: %s"


# ── _compute_region helpers ──────────────────────────────────────────────────


def _region(geometry, anchor, *, hwnd=1, capture=None):
    """Run ``_compute_region`` with the window lookups stubbed.

    ``capture`` (a list) records the hwnd value ``get_window_geometry`` is
    called with, so a test can assert the real hwnd was threaded through.
    """

    def _geom(arg):
        if capture is not None:
            capture.append(arg)
        return geometry

    with (
        patch.object(sp, "find_game_window", lambda: hwnd),
        patch.object(sp, "get_window_geometry", _geom),
    ):
        return sp._compute_region(anchor)


# ── _load_geometry ───────────────────────────────────────────────────────────


def test_load_geometry_reads_present_file(tmp_path, monkeypatch):
    """A present, valid file is parsed and returned (not short-circuited to {}).

    Kills mutmut_1 (inverted ``not ... .exists()`` early return): with the file
    present the original returns the parsed dict, the mutant returns ``{}``.
    Also kills mutmut_2 (``json.loads(None)``) and mutmut_4 (bogus encoding
    ``"XXutf-8XX"``): both raise an uncaught error instead of returning the dict.
    """
    path = tmp_path / "panel_geometry.json"
    payload = {"skill": {"n_rows": 7, "cells": {}}}
    path.write_text(json.dumps(payload), encoding="utf-8")
    monkeypatch.setattr(sp, "_GEOMETRY_PATH", path)

    assert sp._load_geometry() == payload


def test_load_geometry_missing_file_returns_empty(tmp_path, monkeypatch):
    """An absent file returns ``{}`` via the existence guard.

    Reinforces mutmut_1: the mutant would skip the guard and hit ``read_text``
    on a non-existent path, but the OSError is caught and ``{}`` returned, so
    this asserts the original's guard path produces ``{}`` without ever reading.
    """
    path = tmp_path / "does_not_exist.json"
    monkeypatch.setattr(sp, "_GEOMETRY_PATH", path)

    assert sp._load_geometry() == {}


def test_load_geometry_reads_unicode_payload(tmp_path, monkeypatch):
    """Content is decoded as the parsed structure, exercising the read+parse.

    A defensive guard against the encoding-arg mutants degrading the read for a
    non-ASCII payload written as UTF-8.
    """
    path = tmp_path / "panel_geometry.json"
    payload = {"skill": {"label": " éèê", "cells": {}}}
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    monkeypatch.setattr(sp, "_GEOMETRY_PATH", path)

    assert sp._load_geometry() == payload


def test_load_geometry_logs_exact_warning_on_bad_json(tmp_path, monkeypatch, caplog):
    """Unreadable JSON is swallowed to ``{}`` and logged with the EXACT message.

    Kills the warning mutants on the except branch:

    * mutmut_6 (``log.warning(None, exc)``) and mutmut_9 (dropped ``exc`` arg)
      and mutmut_11 (``%S`` casing) - ``record.getMessage()`` raises, surfacing
      as a test error / mismatch.
    * mutmut_7 (``exc`` replaced by ``None``) - message ends in ``None``.
    * mutmut_8 (``exc`` used as the format string) - message is the bare error.
    * mutmut_10 (``XX...XX`` sentinel) - message has the literal markers.
    """
    path = tmp_path / "panel_geometry.json"
    path.write_text("{ this is not valid json", encoding="utf-8")
    monkeypatch.setattr(sp, "_GEOMETRY_PATH", path)

    with caplog.at_level(logging.WARNING, logger=sp.log.name):
        result = sp._load_geometry()

    assert result == {}
    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert len(warnings) == 1
    record = warnings[0]
    # Reconstruct the JSONDecodeError independently to build the expected text.
    try:
        json.loads("{ this is not valid json")
    except json.JSONDecodeError as exc:
        expected_exc = exc
    expected = _WARN_TEMPLATE % expected_exc
    # getMessage() re-applies msg % args; raises for the None-format / dropped-arg
    # / bad-%S mutants and yields a differing string for the rest.
    assert record.getMessage() == expected


# ── _parse_cell ──────────────────────────────────────────────────────────────


def test_parse_cell_copies_every_field():
    """Each cell field is copied from the entry verbatim, none nulled.

    Kills mutmut_2..6 (each sets one ``CellGeometry`` field to ``None``): the
    distinct asserted values fail if any field is ``None``.
    """
    entry = {
        "x_left": 6,
        "x_right": 259,
        "first_y_top": 31,
        "last_y_top": 306,
        "height": 16,
    }
    cell = sp._parse_cell(entry)
    assert cell == CellGeometry(
        x_left=6, x_right=259, first_y_top=31, last_y_top=306, height=16
    )
    # Field-by-field, so a single nulled field is pinned individually.
    assert cell.x_left == 6
    assert cell.x_right == 259
    assert cell.first_y_top == 31
    assert cell.last_y_top == 306
    assert cell.height == 16
    assert None not in (
        cell.x_left,
        cell.x_right,
        cell.first_y_top,
        cell.last_y_top,
        cell.height,
    )


# ── _build_anchor ────────────────────────────────────────────────────────────


def test_build_anchor_stores_parsed_cell():
    """A successfully parsed cell is stored as the CellGeometry, not ``None``.

    Kills mutmut_11 (``cells[cell_name] = None``).
    """
    fallback = PanelAnchor(width=635, height=331, right_offset=30, bottom_offset=170)
    entry = {
        "n_rows": 12,
        "cells": {
            "name": {
                "x_left": 6,
                "x_right": 259,
                "first_y_top": 31,
                "last_y_top": 306,
                "height": 16,
            }
        },
    }
    built = sp._build_anchor(entry, fallback)
    assert "name" in built.cells
    assert built.cells["name"] is not None
    assert built.cells["name"] == CellGeometry(
        x_left=6, x_right=259, first_y_top=31, last_y_top=306, height=16
    )


# ── _compute_region ──────────────────────────────────────────────────────────


def test_compute_region_threads_real_hwnd():
    """The hwnd from ``find_game_window`` is passed to ``get_window_geometry``.

    Kills mutmut_4 (``get_window_geometry(None)``): the captured argument is the
    real hwnd (4242), not ``None``.
    """
    anchor = PanelAnchor(width=100, height=80, right_offset=5, bottom_offset=7)
    seen: list = []
    rect = _region((0, 0, 500, 500), anchor, hwnd=4242, capture=seen)
    assert rect is not None
    assert seen == [4242]


def test_compute_region_rejects_zero_width_only():
    """A zero-width (but positive-height) anchor is rejected (returns None).

    Kills mutmut_17 (``or`` -> ``and``): with ``and`` only one degenerate axis
    no longer trips the guard, so the mutant returns a rect. Kills mutmut_18
    (``br_x <= tl_x`` -> ``br_x < tl_x``): with ``<`` an exactly-zero width
    (``br_x == tl_x``) no longer trips, so the mutant returns a rect.
    """
    anchor = PanelAnchor(width=0, height=80, right_offset=0, bottom_offset=0)
    rect = _region((0, 0, 500, 500), anchor)
    assert rect is None


def test_compute_region_rejects_zero_height_only():
    """A zero-height (but positive-width) anchor is rejected (returns None).

    Kills mutmut_19 (``br_y <= tl_y`` -> ``br_y < tl_y``): with ``<`` an exactly
    zero height (``br_y == tl_y``) no longer trips, so the mutant returns a rect.
    Reinforces mutmut_17 on the height axis.
    """
    anchor = PanelAnchor(width=100, height=0, right_offset=0, bottom_offset=0)
    rect = _region((0, 0, 500, 500), anchor)
    assert rect is None


def test_compute_region_accepts_positive_anchor():
    """A positive-area anchor yields a rect (the non-degenerate path).

    Guards the guard mutants from the opposite side: a genuinely valid anchor
    must still produce a rect, so a mutant that always returns None would fail.
    """
    anchor = PanelAnchor(width=100, height=80, right_offset=5, bottom_offset=7)
    rect = _region((10, 20, 500, 400), anchor)
    assert rect is not None
    (tl_x, tl_y), (br_x, br_y) = rect
    assert br_x - tl_x == 100
    assert br_y - tl_y == 80


# ── skill_region / profession_region / repair_region ────────────────────────


def _patched_window(geometry):
    return (
        patch.object(sp, "find_game_window", lambda: 1),
        patch.object(sp, "get_window_geometry", lambda _hwnd: geometry),
    )


@pytest.mark.parametrize(
    ("func", "expected_w", "expected_h"),
    [
        (sp.skill_region, sp.SKILL_ANCHOR.width, sp.SKILL_ANCHOR.height),
        (sp.profession_region, sp.PROFESSION_ANCHOR.width, sp.PROFESSION_ANCHOR.height),
        (sp.repair_region, sp.REPAIR_ANCHOR.width, sp.REPAIR_ANCHOR.height),
    ],
)
def test_region_helpers_use_their_own_anchor(func, expected_w, expected_h):
    """Each preset feeds its own anchor (not ``None``) to ``_compute_region``.

    Kills the no-test mutants ``skill_region__mutmut_1`` /
    ``profession_region__mutmut_1`` / ``repair_region__mutmut_1`` (all replace
    the anchor with ``None``): with a located window the original returns a rect
    sized to that preset's anchor, while ``_compute_region(None)`` raises
    AttributeError on ``None.right_offset``.
    """
    p_find, p_geom = _patched_window((0, 0, 2000, 2000))
    with p_find, p_geom:
        rect = func()
    assert rect is not None
    (tl_x, tl_y), (br_x, br_y) = rect
    assert br_x - tl_x == expected_w
    assert br_y - tl_y == expected_h


def test_skill_and_profession_anchors_differ():
    """Skill and profession presets produce differently sized rects.

    A cross-check that the per-preset anchor identity matters: if any preset
    were rewired to a shared/None anchor the sizes would no longer match their
    own constants.
    """
    p_find, p_geom = _patched_window((0, 0, 2000, 2000))
    with p_find, p_geom:
        skill = sp.skill_region()
        prof = sp.profession_region()
    assert skill is not None and prof is not None
    (s_tl, s_br) = skill
    (p_tl, p_br) = prof
    assert (s_br[0] - s_tl[0], s_br[1] - s_tl[1]) == (
        sp.SKILL_ANCHOR.width,
        sp.SKILL_ANCHOR.height,
    )
    assert (p_br[0] - p_tl[0], p_br[1] - p_tl[1]) == (
        sp.PROFESSION_ANCHOR.width,
        sp.PROFESSION_ANCHOR.height,
    )


# ── game_window_present ──────────────────────────────────────────────────────


def test_game_window_present_true_when_located():
    """Returns True exactly when a window handle is found.

    Kills ``game_window_present__mutmut_1`` (``is not None`` -> ``is None``):
    the mutant would return False for a located window.
    """
    with patch.object(sp, "find_game_window", lambda: 1234):
        assert sp.game_window_present() is True


def test_game_window_present_false_when_absent():
    """Returns False when no window handle is found.

    Reinforces ``game_window_present__mutmut_1``: the mutant returns True here.
    """
    with patch.object(sp, "find_game_window", lambda: None):
        assert sp.game_window_present() is False
