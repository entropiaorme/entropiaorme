"""Property-based tests for the live-window capture-region geometry.

Covers ``backend.services.scan_presets``: the pure integer arithmetic in
``_compute_region`` (which anchors a fixed-size panel rect to the bottom-right
corner of the located game window) and the ``_build_anchor`` data-file loader
that layers calibration grid geometry on top of a panel-rect fallback.

The module has zero coupling to the event bus, tracker, reducers, or parser:
it is a synchronous Win32 window-geometry helper plus a static JSON read at
import. ``_compute_region`` calls ``find_game_window`` and
``get_window_geometry`` (imported into the module namespace from
``eu_window``), so the properties drive it by substituting those two lookups
with generated window geometry rather than touching a real device. Every input
is drawn from the domain production genuinely permits: positive client
dimensions (``get_window_geometry`` rejects non-positive ones), any integer
window origin (negative origins occur on multi-monitor / off-screen layouts),
and the positive-dimension panel anchors the module actually ships.
"""

from unittest.mock import patch

from hypothesis import given
from hypothesis import strategies as st

from backend.services import scan_presets as sp
from backend.services.scan_presets import PanelAnchor

# Window origins span negatives so multi-monitor / off-screen layouts are
# exercised; client dimensions are strictly positive because get_window_geometry
# returns None for a 0x0 (e.g. minimised) client rect, so _compute_region never
# sees a non-positive width or height from a real window.
_ORIGIN = st.integers(min_value=-4000, max_value=4000)
_EXTENT = st.integers(min_value=1, max_value=8000)
_OFFSET = st.integers(min_value=0, max_value=400)


@st.composite
def _window_geometry(draw):
    """A (x, y, width, height) tuple shaped like get_window_geometry's output."""
    return (draw(_ORIGIN), draw(_ORIGIN), draw(_EXTENT), draw(_EXTENT))


@st.composite
def _anchor(draw):
    """A PanelAnchor with positive panel dimensions, matching the shipped
    fallback constants (the JSON data file can never override width/height)."""
    return PanelAnchor(
        width=draw(st.integers(min_value=1, max_value=2000)),
        height=draw(st.integers(min_value=1, max_value=2000)),
        right_offset=draw(_OFFSET),
        bottom_offset=draw(_OFFSET),
    )


def _patched_region(geometry, anchor):
    """Run _compute_region with the window lookups stubbed to fixed values.

    A context-manager patch (rather than the monkeypatch fixture) keeps the
    substitution scoped per generated input, which @given requires.
    """
    with (
        patch.object(sp, "find_game_window", lambda: 1),
        patch.object(sp, "get_window_geometry", lambda _hwnd: geometry),
    ):
        return sp._compute_region(anchor)


# ── _compute_region ──────────────────────────────────────────────────────────


@given(_window_geometry(), _anchor())
def test_region_corners_strictly_ordered(geometry, anchor):
    """Any returned rect has a strictly smaller top-left than bottom-right.

    The degeneracy guard rejects (returns None) exactly when width <= 0 or
    height <= 0, so a non-None rect always has positive area.
    """
    rect = _patched_region(geometry, anchor)
    assert rect is not None  # positive-dimension anchor never trips the guard
    (tl_x, tl_y), (br_x, br_y) = rect
    assert tl_x < br_x
    assert tl_y < br_y


@given(_window_geometry(), _anchor())
def test_region_size_equals_anchor(geometry, anchor):
    """The rect's width and height equal the anchor's, exactly and
    independently of the window origin or size."""
    rect = _patched_region(geometry, anchor)
    assert rect is not None
    (tl_x, tl_y), (br_x, br_y) = rect
    assert br_x - tl_x == anchor.width
    assert br_y - tl_y == anchor.height


@given(_window_geometry(), _anchor())
def test_bottom_right_anchored_to_window(geometry, anchor):
    """When a rect is returned its bottom-right corner sits at the window's
    bottom-right client edge less the configured offsets."""
    win_x, win_y, win_w, win_h = geometry
    rect = _patched_region(geometry, anchor)
    assert rect is not None
    _tl, (br_x, br_y) = rect
    assert br_x == win_x + win_w - anchor.right_offset
    assert br_y == win_y + win_h - anchor.bottom_offset


@given(_window_geometry(), _anchor(), _ORIGIN, _ORIGIN)
def test_translation_invariance(geometry, anchor, dx, dy):
    """Translating the window by (dx, dy) shifts both rect corners by exactly
    (dx, dy) and leaves the rect's width and height unchanged."""
    win_x, win_y, win_w, win_h = geometry
    base = _patched_region(geometry, anchor)
    moved = _patched_region((win_x + dx, win_y + dy, win_w, win_h), anchor)
    assert base is not None and moved is not None
    (b_tl_x, b_tl_y), (b_br_x, b_br_y) = base
    (m_tl_x, m_tl_y), (m_br_x, m_br_y) = moved
    assert (m_tl_x, m_tl_y) == (b_tl_x + dx, b_tl_y + dy)
    assert (m_br_x, m_br_y) == (b_br_x + dx, b_br_y + dy)
    assert m_br_x - m_tl_x == b_br_x - b_tl_x
    assert m_br_y - m_tl_y == b_br_y - b_tl_y


# ── _build_anchor ────────────────────────────────────────────────────────────

# A JSON entry mirrors panel_geometry.json: an optional row count plus a cells
# dict keyed by cell name. Same-named rect keys (e.g. a stray "width") are
# included to confirm they can never override the fallback panel rect.
_CELL = st.fixed_dictionaries(
    {
        "x_left": st.integers(min_value=0, max_value=64),
        "x_right": st.integers(min_value=0, max_value=128),
        "first_y_top": st.integers(min_value=0, max_value=64),
        "last_y_top": st.integers(min_value=0, max_value=64),
        "height": st.integers(min_value=1, max_value=32),
    }
)
_ENTRY = st.fixed_dictionaries(
    {
        "n_rows": st.none() | st.integers(min_value=0, max_value=12),
        "cells": st.dictionaries(
            st.text(alphabet="abcdef", min_size=1, max_size=4), _CELL, max_size=4
        ),
        # A hostile rect override that the loader must ignore.
        "width": st.integers(min_value=1, max_value=9999),
        "height": st.integers(min_value=1, max_value=9999),
        "right_offset": st.integers(min_value=0, max_value=9999),
        "bottom_offset": st.integers(min_value=0, max_value=9999),
    }
)


@given(_anchor(), st.none() | _ENTRY)
def test_build_anchor_preserves_panel_rect(fallback, entry):
    """The built anchor's panel rect (width / height / offsets) always equals
    the fallback's; the JSON entry can never override it, only contribute
    n_rows and cells. Asserted as field-VALUE equality, since a truthy entry
    constructs a fresh object rather than returning the fallback identity.
    """
    built = sp._build_anchor(entry, fallback)
    assert built.width == fallback.width
    assert built.height == fallback.height
    assert built.right_offset == fallback.right_offset
    assert built.bottom_offset == fallback.bottom_offset
    if not entry:
        # A falsy entry short-circuits to the fallback object unchanged.
        assert built is fallback
    else:
        # n_rows and cells are the only fields sourced from the JSON entry.
        assert built.n_rows == entry["n_rows"]
        assert set(built.cells) == set(entry["cells"])
