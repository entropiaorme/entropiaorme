"""Capture regions for skill / profession scans, derived from the live game window.

The user docks the relevant in-game panel in the bottom-right corner at default UI
scale (1.0). The panel pixel size at default UI scale is fixed by the EU client
regardless of window resolution, so the only thing that varies between machines is
where the bottom-right corner of the game window sits.

We locate the EU window via :mod:`backend.services.eu_window`, then anchor the
capture region to its bottom-right corner using these constants.

If the EU window can't be found we return ``None`` and the caller surfaces a
"start Entropia Universe first" hint.

Panel-relative grid geometry (row band + column splits) is loaded at import time
from ``backend/data/panel_geometry.json`` when present; values fall back to
panel-anchor-only constants otherwise. The JSON is treated as a shipped data
constant produced by an out-of-tree calibration step.
"""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field
from pathlib import Path

from backend.services.eu_window import find_game_window, get_window_geometry

log = logging.getLogger(__name__)

_GEOMETRY_PATH = Path(__file__).resolve().parents[1] / "data" / "panel_geometry.json"


@dataclass(frozen=True)
class CellGeometry:
    """One cell type's panel-relative rect, with per-row anchors for interpolation.

    ``x_left`` / ``x_right`` are the column's horizontal extents (uniform across
    rows). ``first_y_top`` and ``last_y_top`` are the cell's top-y on the first
    and last visible rows: intermediate rows interpolate linearly via
    ``y_top(r) = round(first_y_top + r * (last_y_top - first_y_top) / (n_rows - 1))``.
    ``height`` is the cell's vertical extent (uniform across rows). Cropping
    a cell on row ``r`` uses ``[x_left : x_right]`` horizontally and
    ``[y_top(r) : y_top(r) + height]`` vertically.
    """

    x_left: int
    x_right: int
    first_y_top: int
    last_y_top: int
    height: int


@dataclass
class PanelAnchor:
    """Bottom-right docked panel dimensions, measured at default UI scale (1.0).

    ``width`` / ``height`` describe the panel rect; ``right_offset`` /
    ``bottom_offset`` are the pixel gap between the panel and the corresponding
    edge of the game window's client area.

    ``n_rows`` and ``cells`` describe the panel-relative grid geometry produced
    by the calibration CLI. ``cells`` is keyed by cell name (the cell vocabulary
    differs per panel: skill uses ``name`` / ``level`` / ``bar``, profession
    uses ``name`` / ``rank_level`` / ``percent`` / ``bar``). The dict is empty
    until calibration has run. Bar geometry is captured for forward
    compatibility; reading code is not yet wired in.
    """

    width: int
    height: int
    right_offset: int = 0
    bottom_offset: int = 0
    n_rows: int | None = None
    cells: dict[str, CellGeometry] = field(default_factory=dict)


_SKILL_FALLBACK = PanelAnchor(width=635, height=331, right_offset=30, bottom_offset=170)
_PROFESSION_FALLBACK = PanelAnchor(
    width=474, height=293, right_offset=31, bottom_offset=161
)
_REPAIR_FALLBACK = PanelAnchor(width=50, height=17, right_offset=48, bottom_offset=86)


def _load_geometry() -> dict:
    if not _GEOMETRY_PATH.exists():
        return {}
    try:
        return json.loads(_GEOMETRY_PATH.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        log.warning("panel_geometry.json unreadable, using fallback constants: %s", exc)
        return {}


def _parse_cell(entry: dict | None) -> CellGeometry | None:
    if not entry:
        return None
    try:
        return CellGeometry(
            x_left=entry["x_left"],
            x_right=entry["x_right"],
            first_y_top=entry["first_y_top"],
            last_y_top=entry["last_y_top"],
            height=entry["height"],
        )
    except (KeyError, TypeError):
        return None


def _build_anchor(entry: dict | None, fallback: PanelAnchor) -> PanelAnchor:
    """Apply a JSON grid-geometry entry on top of the panel-rect fallback.

    Panel rect (``width`` / ``height`` / ``*_offset``) always comes from the
    fallback; the JSON carries ``n_rows`` and ``cells`` (keyed by cell name).
    Returns the fallback unchanged when the JSON entry is absent or empty.
    """
    if not entry:
        return fallback
    raw_cells = entry.get("cells") or {}
    cells: dict[str, CellGeometry] = {}
    for cell_name, raw in raw_cells.items():
        parsed = _parse_cell(raw)
        if parsed is not None:
            cells[cell_name] = parsed
    return PanelAnchor(
        width=fallback.width,
        height=fallback.height,
        right_offset=fallback.right_offset,
        bottom_offset=fallback.bottom_offset,
        n_rows=entry.get("n_rows"),
        cells=cells,
    )


_GEOMETRY = _load_geometry()
SKILL_ANCHOR = _build_anchor(_GEOMETRY.get("skill"), _SKILL_FALLBACK)
PROFESSION_ANCHOR = _build_anchor(_GEOMETRY.get("profession"), _PROFESSION_FALLBACK)
REPAIR_ANCHOR = _REPAIR_FALLBACK


def _compute_region(anchor: PanelAnchor) -> tuple[list[int], list[int]] | None:
    hwnd = find_game_window()
    if hwnd is None:
        return None
    geometry = get_window_geometry(hwnd)
    if geometry is None:
        return None
    win_x, win_y, win_w, win_h = geometry
    br_x = win_x + win_w - anchor.right_offset
    br_y = win_y + win_h - anchor.bottom_offset
    tl_x = br_x - anchor.width
    tl_y = br_y - anchor.height
    if br_x <= tl_x or br_y <= tl_y:
        return None
    return ([tl_x, tl_y], [br_x, br_y])


def skill_region() -> tuple[list[int], list[int]] | None:
    """Return the skill panel capture rect, or None if EU window not found."""
    return _compute_region(SKILL_ANCHOR)


def profession_region() -> tuple[list[int], list[int]] | None:
    """Return the profession panel capture rect, or None if EU window not found."""
    return _compute_region(PROFESSION_ANCHOR)


def repair_region() -> tuple[list[int], list[int]] | None:
    """Return the repair-cost number capture rect, or None if EU window not found.

    Assumes the in-game repair terminal is docked bottom-right of the EU client
    window at default UI scale (1.0).
    """
    return _compute_region(REPAIR_ANCHOR)


def game_window_present() -> bool:
    """Whether the EU client window is currently locatable."""
    return find_game_window() is not None
