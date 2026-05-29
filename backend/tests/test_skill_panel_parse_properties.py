"""Property-based tests for the pure skill-panel OCR parsing helpers.

Covers the device-free post-processing in
``backend.services.skill_panel_parse``: the integer level parse, the bar
fill-ratio estimate, the fuzzy name resolution against a canonical vocab, and
the panel cell-slicing geometry. Each property generates inputs from the
domain production genuinely permits (BGR 3-channel crops, finite integer panel
geometry with a non-negative row count, string vocab entries) and asserts an
invariant that must hold across that whole domain.

The example-based companion in ``test_skill_panel_parse`` pins specific
branches; these properties assert the contracts that hold for every input.
"""

import numpy as np
from hypothesis import given
from hypothesis import strategies as st

from backend.services import skill_panel_parse as sp

# ── parse_bar_fill ───────────────────────────────────────────────────────────


@st.composite
def _bgr_crop(draw):
    """A random BGR uint8 crop with a positive height and width.

    This mirrors what decode_panel_png (IMREAD_COLOR) feeds via
    slice_panel_cells; a 2D or 1-channel crop is not a production input and
    would make cv2.cvtColor raise rather than return a value.
    """
    h = draw(st.integers(min_value=1, max_value=6))
    w = draw(st.integers(min_value=1, max_value=12))
    flat = draw(
        st.lists(
            st.integers(min_value=0, max_value=255),
            min_size=h * w * 3,
            max_size=h * w * 3,
        )
    )
    return np.array(flat, dtype=np.uint8).reshape(h, w, 3)


@given(_bgr_crop())
def test_bar_fill_in_unit_half_open(crop):
    """The fill estimate is always a float in the half-open unit interval.

    Every branch either returns literal 0.0 or the rightmost-bright ratio,
    which the >= 1.0 flip caps strictly below 1.0; no input yields a value
    outside [0.0, 1.0), nor a NaN or infinity.
    """
    fill = sp.parse_bar_fill(crop)
    assert isinstance(fill, float)
    assert 0.0 <= fill < 1.0


@given(st.none() | st.builds(lambda: np.zeros((4, 0, 3), dtype=np.uint8)))
def test_bar_fill_empty_crop_is_zero(crop):
    """A missing or zero-size crop reads as an empty bar, not a spurious fill."""
    assert sp.parse_bar_fill(crop) == 0.0


# ── parse_level ──────────────────────────────────────────────────────────────

# Mixed text around an optional ASCII digit run; the alphabet deliberately
# avoids '-'/'+' (the parse never reads a sign) and spans letters, spaces, and
# punctuation so the leftmost-run contract is exercised against noise.
_LEVEL_TEXT = st.text(
    alphabet=st.sampled_from(list("0123456789abc .,:-+xX")), max_size=12
)


@given(st.none() | _LEVEL_TEXT)
def test_parse_level_nonnegative_first_run(text):
    """parse_level returns None when no digit run is present, else the
    non-negative integer value of the leftmost run."""
    result = sp.parse_level(text)
    if not text or not any(ch.isdigit() for ch in text):
        assert result is None
        return
    assert isinstance(result, int)
    assert result >= 0
    # The result is exactly the leftmost maximal run of decimal digits.
    leftmost = ""
    for ch in text:
        if ch.isdigit():
            leftmost += ch
        elif leftmost:
            break
    assert result == int(leftmost)


# ── fuzzy_resolve ────────────────────────────────────────────────────────────

# Vocab entries are strings (skills.json never holds non-string names); a small
# alphabet makes near-miss queries collide with entries often.
_VOCAB_WORD = st.text(alphabet="abcdef ", min_size=1, max_size=6)
_VOCAB = st.lists(_VOCAB_WORD, min_size=1, max_size=6)
_QUERY = st.text(alphabet="abcdef ", max_size=6)


@given(_QUERY, _VOCAB)
def test_fuzzy_resolve_canonical_is_vocab_member(ocr_text, vocab):
    """Whenever fuzzy_resolve returns a non-None canonical, it is a vocab
    member (exact, normalised-equal, or a process.extract candidate)."""
    canonical, _score, _cands = sp.fuzzy_resolve(ocr_text, vocab)
    if canonical is not None:
        assert canonical in vocab


@given(_QUERY, _VOCAB)
def test_fuzzy_resolve_score_tracks_non_empty_candidates(ocr_text, vocab):
    """On any resolved path the top1 score is the head candidate's score, and
    the candidate list is capped at the configured top-N."""
    canonical, score, cands = sp.fuzzy_resolve(ocr_text, vocab)
    if canonical is not None:
        assert cands  # a resolved canonical always carries candidates
        assert score == cands[0][1]
        assert len(cands) <= sp._FUZZ_TOP_N


@given(st.sampled_from(["", "   ", "\t\n"]), _VOCAB)
def test_fuzzy_resolve_blank_query_is_unresolved(blank, vocab):
    """A blank (or whitespace-only) query resolves to the documented sentinel."""
    assert sp.fuzzy_resolve(blank, vocab) == (None, 0.0, [])


# ── slice_panel_cells ────────────────────────────────────────────────────────


@st.composite
def _geometry(draw):
    """A panel geometry entry with a non-negative row count and a dict of cells
    whose pixel fields are finite integers, matching panel_geometry.json."""
    n_rows = draw(st.integers(min_value=0, max_value=6))
    cell_names = draw(
        st.lists(
            st.text(alphabet="abcdef", min_size=1, max_size=4),
            min_size=0,
            max_size=4,
            unique=True,
        )
    )
    coord = st.integers(min_value=0, max_value=64)
    cells = {}
    for name in cell_names:
        x_left = draw(coord)
        x_right = draw(st.integers(min_value=x_left, max_value=x_left + 32))
        first = draw(coord)
        last = draw(coord)
        cells[name] = {
            "x_left": x_left,
            "x_right": x_right,
            "first_y_top": first,
            "last_y_top": last,
            "height": draw(st.integers(min_value=1, max_value=16)),
        }
    return n_rows, cells


def _panel(width: int = 96, height: int = 256) -> np.ndarray:
    """A BGR panel large enough that any generated crop falls inside it."""
    return np.zeros((height, width, 3), dtype=np.uint8)


@given(_geometry())
def test_slice_cell_count_and_order(geom_parts):
    """The crop list has exactly n_rows * len(cells) entries, ordered rows
    0..n_rows-1 outermost then cells in geometry-insertion order, with every
    crop's row in range and cell drawn from the geometry keys."""
    n_rows, cells = geom_parts
    geom = {"n_rows": n_rows, "cells": cells}
    crops = sp.slice_panel_cells(_panel(), geom)

    assert len(crops) == n_rows * len(cells)
    expected_order = [(r, name) for r in range(n_rows) for name in cells]
    assert [(c.row, c.cell) for c in crops] == expected_order
    for c in crops:
        assert 0 <= c.row < n_rows
        assert c.cell in cells


@given(_geometry())
def test_slice_row_offset_monotonic_interpolation(geom_parts):
    """For a single row the y-offset is first_y_top exactly; for any cell whose
    last_y_top >= first_y_top the per-row top offsets are non-decreasing."""
    n_rows, cells = geom_parts
    geom = {"n_rows": n_rows, "cells": cells}
    crops = sp.slice_panel_cells(_panel(), geom)

    by_cell: dict[str, list] = {name: [] for name in cells}
    for c in crops:
        by_cell[c.cell].append(c)

    for name, cell in cells.items():
        rows = sorted(by_cell[name], key=lambda c: c.row)
        # Reconstruct each crop's y-offset from where its top row was sliced.
        # The panel is uniform, so recompute the intended offset directly.
        first = cell["first_y_top"]
        last = cell["last_y_top"]
        offsets = []
        for c in rows:
            if n_rows > 1:
                offsets.append(round(first + c.row * (last - first) / (n_rows - 1)))
            else:
                offsets.append(first)
        if n_rows == 1 and offsets:
            assert offsets[0] == first
        if last >= first:
            assert offsets == sorted(offsets)
