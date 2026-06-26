"""Unit tests for the pure skill-panel OCR parsing in ``skill_panel_parse``.

This is the device-free post-processing the mutation campaign targets, so the
cases below are written to notice a mutation in each branch: name
normalisation and fuzzy resolution, the level parse, the bar fill-ratio
estimate (contrast guard, threshold, rightmost-bright arithmetic), the
cell-slicing interpolation, and the PNG decode.
"""

import cv2
import numpy as np
import pytest

from backend.services import skill_panel_parse as sp

# ── _norm_name ───────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "raw,expected",
    [
        ("Food Technology", "foodtechnology"),  # whitespace dropped
        ("  Whip  ", "whip"),  # outer whitespace dropped, lowercased
        ("ABC", "abc"),  # case folded
        ("", ""),
    ],
)
def test_norm_name(raw, expected):
    assert sp._norm_name(raw) == expected


def test_norm_name_none_is_empty():
    """A None reading normalises to the empty string rather than crashing."""
    assert sp._norm_name(None) == ""  # type: ignore[arg-type]


# ── parse_level ──────────────────────────────────────────────────────────────


@pytest.mark.parametrize(
    "text,expected",
    [
        ("42", 42),
        ("Level 7", 7),
        ("12abc34", 12),  # first integer run wins
        ("0", 0),
    ],
)
def test_parse_level_reads_first_integer(text, expected):
    assert sp.parse_level(text) == expected


@pytest.mark.parametrize("text", ["", None, "no digits"])
def test_parse_level_none_without_an_integer(text):
    assert sp.parse_level(text) is None


# ── parse_bar_fill ───────────────────────────────────────────────────────────


def _bar_from_cols(values: list[int], height: int = 4) -> np.ndarray:
    """A BGR crop whose column j is the flat grey value ``values[j]``.

    Equal channels mean the BGR->GRAY conversion returns the value itself, so
    the per-column luminance the estimator reads is exactly ``values``.
    """
    crop = np.zeros((height, len(values), 3), dtype=np.uint8)
    for j, v in enumerate(values):
        crop[:, j, :] = v
    return crop


def _gray_bar(width: int, bright_cols: int, height: int = 4) -> np.ndarray:
    """A bar crop: the first ``bright_cols`` columns white, the rest black."""
    return _bar_from_cols([255] * bright_cols + [0] * (width - bright_cols), height)


def test_parse_bar_fill_half_filled():
    """Five bright columns of ten: rightmost bright is index 4, fill = 5/10."""
    assert sp.parse_bar_fill(_gray_bar(10, 5)) == pytest.approx(0.5)


def test_parse_bar_fill_quarter_filled():
    assert sp.parse_bar_fill(_gray_bar(20, 5)) == pytest.approx(0.25)


def test_parse_bar_fill_full_bar_reads_zero():
    """A bar bright to the last column would be a just-levelled bar: flip to 0.0."""
    crop = _gray_bar(10, 10)
    crop[:, 0, :] = 0  # one dark column gives the contrast the estimator needs
    assert sp.parse_bar_fill(crop) == 0.0


def test_parse_bar_fill_low_contrast_is_zero():
    """A uniform crop has no detectable fill edge: 0.0, not a spurious reading."""
    assert sp.parse_bar_fill(np.full((4, 10, 3), 100, dtype=np.uint8)) == 0.0


@pytest.mark.parametrize("crop", [None, np.zeros((4, 0, 3), dtype=np.uint8)])
def test_parse_bar_fill_empty_is_zero(crop):
    assert sp.parse_bar_fill(crop) == 0.0


def test_parse_bar_fill_low_contrast_uses_range_not_sum():
    """Contrast is hi-lo, not hi+lo: a 10-unit spread is low contrast -> 0.0.

    Columns 20 (left half) and 10 (right half): hi-lo = 10 (< 15, so 0.0),
    but hi+lo = 30. Were the guard summing instead of subtracting it would
    proceed and report a 0.5 fill.
    """
    assert sp.parse_bar_fill(_bar_from_cols([20] * 5 + [10] * 5)) == 0.0


def test_parse_bar_fill_contrast_threshold_is_strict_15():
    """A spread of exactly 15 is still low contrast (the guard is `< 15`).

    Columns 15 (left half) and 0 (right half): hi-lo = 15, so the strict `< 15`
    guard does NOT fire and a 0.5 fill is reported. A `<= 15` or `< 16` guard
    would bail to 0.0 instead.
    """
    assert sp.parse_bar_fill(_bar_from_cols([15] * 5 + [0] * 5)) == pytest.approx(0.5)


def test_parse_bar_fill_threshold_is_midpoint():
    """The bright cut is the lo/hi midpoint; a lower divisor would over-count.

    Columns [0, 50, 200, 70, 0]: midpoint threshold 100 marks only the 200
    column bright (rightmost index 2, fill 0.6). Dividing the range by 3
    (threshold ~67) would also mark the 70 column, pushing fill to 0.8.
    """
    assert sp.parse_bar_fill(_bar_from_cols([0, 50, 200, 70, 0])) == pytest.approx(0.6)


def test_parse_bar_fill_single_bright_column_is_measured():
    """One bright column mid-bar yields a real fill, not the empty-set bail.

    Columns [0, 0, 200, 0, 0]: a single column clears the threshold at index 2
    (fill 0.6). Treating "exactly one bright column" as the empty case would
    drop it to 0.0.
    """
    assert sp.parse_bar_fill(_bar_from_cols([0, 0, 200, 0, 0])) == pytest.approx(0.6)


# ── fuzzy_resolve ────────────────────────────────────────────────────────────

VOCAB = ["Whip", "Aim", "Food Technology", "Mining", "Cooking", "Handgun"]


def test_fuzzy_resolve_exact_match():
    assert sp.fuzzy_resolve("Whip", VOCAB) == ("Whip", 100.0, [("Whip", 100.0)])


def test_fuzzy_resolve_case_and_whitespace_insensitive():
    """`food technology` resolves to the canonical spelling at 100, in the cands."""
    assert sp.fuzzy_resolve("food technology", VOCAB) == (
        "Food Technology",
        100.0,
        [("Food Technology", 100.0)],
    )


def test_fuzzy_resolve_near_miss_returns_top_candidate():
    canonical, score, cands = sp.fuzzy_resolve("Whp", VOCAB)
    assert canonical == "Whip"
    assert 0.0 < score < 100.0
    assert score == cands[0][1]  # the returned score is the top candidate's
    assert cands[0][1] > cands[1][1]  # ...and the ranking is real, not flat


def test_fuzzy_resolve_caps_candidate_list():
    """The candidate list is capped at the configured top-N, not the whole vocab."""
    _, _, cands = sp.fuzzy_resolve("Whp", VOCAB)
    assert len(cands) == sp._FUZZ_TOP_N


def test_fuzzy_resolve_substring_query_resolves_to_container():
    """A substring query resolves to the entry that contains it.

    `Tech` is a substring of `Food Technology`; the WRatio scorer's partial
    component ranks it top despite the length mismatch.
    """
    canonical, _, _ = sp.fuzzy_resolve("Tech", VOCAB)
    assert canonical == "Food Technology"


@pytest.mark.parametrize("text", ["", "   ", None])
def test_fuzzy_resolve_blank_is_unresolved(text):
    assert sp.fuzzy_resolve(text, VOCAB) == (None, 0.0, [])


def test_fuzzy_resolve_empty_vocab_is_unresolved():
    assert sp.fuzzy_resolve("anything", []) == (None, 0.0, [])


def test_fuzzy_resolve_floors_unknown_names_to_none():
    """A read resembling no known skill scores below the floor and is left
    unresolved, rather than force-matched to its nearest vocab entry."""
    canonical, score, cands = sp.fuzzy_resolve("qzxwv", VOCAB)
    assert canonical is None, "below-floor garbage is not force-matched"
    assert score < sp._FUZZY_SCORE_FLOOR
    # The candidate list is still surfaced even though the top is rejected.
    assert cands

    # A genuine typo of a PRESENT skill stays above the floor and resolves
    # (the floor does not discard real reads).
    canonical, score, _ = sp.fuzzy_resolve("Whp", VOCAB)
    assert canonical == "Whip"
    assert score >= sp._FUZZY_SCORE_FLOOR


# ── slice_panel_cells ────────────────────────────────────────────────────────


def _y_indexed_panel(height: int = 16, width: int = 1) -> np.ndarray:
    """A panel whose every pixel on row y holds the value y, so a crop's top
    row reveals the y-offset it was sliced from."""
    col = np.arange(height, dtype=np.uint8).reshape(height, 1, 1)
    return np.tile(col, (1, width, 3))


def test_slice_panel_cells_interpolates_row_offsets():
    """Row y-offsets interpolate linearly between first_y_top and last_y_top.

    Three rows over first=2..last=12 give tops 2, 7, 12. The crop's top pixel
    equals its y-offset, so a sign flip, a swapped divisor/multiplier, a
    last+first mix-up, or an off-by-one row count all move the offsets.
    """
    geom = {
        "n_rows": 3,
        "cells": {
            "name": {
                "x_left": 0,
                "x_right": 1,
                "first_y_top": 2,
                "last_y_top": 12,
                "height": 2,
            }
        },
    }
    crops = sp.slice_panel_cells(_y_indexed_panel(), geom)

    assert [(c.row, c.cell) for c in crops] == [(0, "name"), (1, "name"), (2, "name")]
    assert [int(c.image[0, 0, 0]) for c in crops] == [2, 7, 12]
    # Each crop spans exactly `height` rows (a missing y_bot would run to the
    # panel's bottom edge instead).
    assert [c.image.shape[0] for c in crops] == [2, 2, 2]


def test_slice_panel_cells_interpolates_only_when_multi_row():
    """With two rows the second still interpolates to last_y_top, not first.

    Guards the `n_rows > 1` boundary: were it `n_rows > 2`, a two-row panel
    would skip interpolation and stack both rows at first_y_top.
    """
    geom = {
        "n_rows": 2,
        "cells": {
            "name": {
                "x_left": 0,
                "x_right": 1,
                "first_y_top": 0,
                "last_y_top": 10,
                "height": 2,
            }
        },
    }
    crops = sp.slice_panel_cells(_y_indexed_panel(), geom)

    assert [int(c.image[0, 0, 0]) for c in crops] == [0, 10]


def test_slice_panel_cells_single_row_uses_first_offset():
    geom = {
        "n_rows": 1,
        "cells": {
            "name": {
                "x_left": 0,
                "x_right": 1,
                "first_y_top": 3,
                "last_y_top": 9,
                "height": 2,
            }
        },
    }
    crops = sp.slice_panel_cells(_y_indexed_panel(), geom)

    assert len(crops) == 1
    assert int(crops[0].image[0, 0, 0]) == 3  # first_y_top, no interpolation


# ── decode_panel_png ─────────────────────────────────────────────────────────


def test_decode_panel_png_round_trips():
    img = np.array([[[10, 20, 30], [40, 50, 60]]], dtype=np.uint8)
    ok, buf = cv2.imencode(".png", img)
    assert ok

    decoded = sp.decode_panel_png(buf.tobytes())

    assert decoded.shape == img.shape
    assert np.array_equal(decoded, img)


def test_decode_panel_png_rejects_garbage():
    with pytest.raises(ValueError):
        sp.decode_panel_png(b"not a png")
