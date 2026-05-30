"""Property-based tests for the live game-window locator and measurer.

Covers ``backend.services.eu_window``: ``find_game_window`` (enumerates
top-level windows and returns the HWND of the visible Entropia Universe
client) and ``get_window_geometry`` (reads a window's client rect and maps
its origin to screen coordinates). Both are synchronous Win32 device helpers
with zero coupling to the event bus, tracker, reducers, parser, or session
lifecycle: no game-event sequence can reach or perturb them.

The properties drive the real control flow while substituting the single
OS seam, the module-level ``_user32`` handle, with a fake that feeds
generated window state into the genuine callback / guard logic rather than
touching a live device. Every input is drawn from the domain the OS layer
genuinely produces: arbitrary window titles and visibility flags, and client
rects whose corners span the full integer range (so degenerate and inverted
rects are exercised alongside well-formed ones).
"""

from unittest.mock import patch

from hypothesis import given
from hypothesis import strategies as st

from backend.services import eu_window as ew

# ── find_game_window: title + visibility fake ────────────────────────────────


class _EnumFake:
    """A fake ``_user32`` that replays a fixed list of (hwnd, title, visible)
    windows through the genuine ``enum_callback``.

    ``EnumWindows`` invokes the callback once per window, snapshotting that
    window as the "current" one so the per-hwnd ``GetWindowTextLengthW`` /
    ``GetWindowTextW`` / ``IsWindowVisible`` reads stay mutually consistent
    (mirroring how the real API answers all three for the same HWND). The
    callback returns False to stop enumeration early, exactly as the OS does.
    """

    def __init__(self, windows):
        self._windows = windows
        self._cur: tuple[int, str, bool] | None = None

    def EnumWindows(self, callback, lparam):
        for entry in self._windows:
            self._cur = entry
            if not callback(entry[0], lparam):
                break
        return True

    def GetWindowTextLengthW(self, _hwnd):
        assert self._cur is not None
        return len(self._cur[1])

    def GetWindowTextW(self, _hwnd, buf, _maxcount):
        assert self._cur is not None
        buf.value = self._cur[1]
        return len(self._cur[1])

    def IsWindowVisible(self, _hwnd):
        assert self._cur is not None
        return self._cur[2]


# Titles mix the matching prefix, near-misses, and the empty string so the
# prefix gate, the visibility gate, and the zero-length skip are all exercised.
# The alphabet is restricted to the Basic Multilingual Plane so a Python
# len() equals the UTF-16 code-unit count the Win32 API reports (astral
# characters need surrogate pairs); real client window titles never use them,
# so this matches the domain the OS layer actually produces.
_TEXT = st.text(
    alphabet=st.characters(
        min_codepoint=0x20,
        max_codepoint=0xFFFF,
        blacklist_categories=("Cs",),  # type: ignore[arg-type]
    ),
    max_size=12,
)
_TITLE = st.one_of(
    st.just(""),
    _TEXT,
    st.builds(lambda s: ew.GAME_TITLE_PREFIX + s, _TEXT),
)


@st.composite
def _windows(draw):
    """A list of (hwnd, title, visible) windows with unique HWNDs.

    Unique HWNDs let a returned handle be mapped back to the exact window it
    came from, so the gate predicates can be checked against that window's
    own title and visibility.
    """
    count = draw(st.integers(min_value=0, max_value=6))
    hwnds = draw(
        st.lists(
            st.integers(min_value=1, max_value=10_000),
            min_size=count,
            max_size=count,
            unique=True,
        )
    )
    return [(h, draw(_TITLE), draw(st.booleans())) for h in hwnds]


@given(_windows())
def test_returned_window_passes_every_gate(windows):
    """A non-None result is a window that had a non-empty title starting with
    the client prefix and was visible; otherwise the result is None.

    This is the conjunction the recon vetted as ``title_prefix_match``: the
    only path that appends an HWND requires title.startswith(prefix) and
    IsWindowVisible truthy, and the length==0 skip means a zero-length title
    is never returned.
    """
    by_hwnd = {h: (title, visible) for h, title, visible in windows}
    with patch.object(ew, "_user32", _EnumFake(windows)):
        result = ew.find_game_window()

    if result is None:
        return
    title, visible = by_hwnd[result]
    assert len(title) > 0
    assert title.startswith(ew.GAME_TITLE_PREFIX)
    assert visible


@given(_windows())
def test_result_matches_first_eligible_window(windows):
    """The result equals the first window (in enumeration order) that clears
    all three gates, and None when none does.

    Pins down selection, not just membership: enumeration stops at the first
    match, so a later eligible window can never shadow an earlier one.
    """
    expected = next(
        (
            h
            for h, title, visible in windows
            if len(title) > 0 and title.startswith(ew.GAME_TITLE_PREFIX) and visible
        ),
        None,
    )
    with patch.object(ew, "_user32", _EnumFake(windows)):
        assert ew.find_game_window() == expected


# ── get_window_geometry: client-rect fake ────────────────────────────────────


class _GeomFake:
    """A fake ``_user32`` that fills the caller's RECT and POINT structs.

    ``ctypes.byref(x)`` exposes the wrapped struct via ``._obj``, so the fake
    writes the generated corner / origin values straight into the buffers the
    production code passes by reference, driving the genuine width / height
    arithmetic and the degeneracy guard.
    """

    def __init__(self, rect, point):
        self._rect = rect
        self._point = point

    def GetClientRect(self, _hwnd, ref):
        struct = ref._obj
        struct.left, struct.top, struct.right, struct.bottom = self._rect

    def ClientToScreen(self, _hwnd, ref):
        struct = ref._obj
        struct.x, struct.y = self._point


_COORD = st.integers(min_value=-5000, max_value=5000)


@given(
    rect=st.tuples(_COORD, _COORD, _COORD, _COORD),
    point=st.tuples(_COORD, _COORD),
    hwnd=st.integers(min_value=1, max_value=10_000),
)
def test_geometry_is_none_or_strictly_positive_size(rect, point, hwnd):
    """The result is either None or a 4-tuple whose width and height are both
    strictly positive.

    This is the recon's ``geometry_nonzero_or_none``: the only tuple-returning
    path sits behind the ``width <= 0 or height <= 0`` guard, so any returned
    rect has positive area regardless of the (unconstrained) origin.
    """
    with patch.object(ew, "_user32", _GeomFake(rect, point)):
        result = ew.get_window_geometry(hwnd)

    if result is None:
        return
    x, y, width, height = result
    assert width > 0
    assert height > 0
    # The reported size is exactly the client-rect extents, and the origin is
    # exactly the mapped screen point (which may be negative off-screen).
    left, top, right, bottom = rect
    assert width == right - left
    assert height == bottom - top
    assert (x, y) == point


# ── platform-gated early return ──────────────────────────────────────────────


class _ExplodingUser32:
    """Any attribute access fails, proving the gated path makes no OS call."""

    def __getattr__(self, name):  # pragma: no cover - reaching it is the failure
        raise AssertionError(f"gated path must not call _user32.{name}")


@given(hwnd=st.integers(min_value=0, max_value=10_000))
def test_non_windows_platform_returns_none_without_os_calls(hwnd):
    """On a non-win32 platform both helpers return None and touch no OS seam.

    This is the recon's ``platform_gated_none``: the identical first-statement
    guard short-circuits before any ``_user32`` use, so an exploding handle is
    never reached.
    """
    with (
        patch.object(ew.sys, "platform", "linux"),
        patch.object(ew, "_user32", _ExplodingUser32()),
    ):
        assert ew.find_game_window() is None
        assert ew.get_window_geometry(hwnd) is None


@given(hwnd=st.integers(min_value=0, max_value=10_000))
def test_missing_handle_returns_none_without_os_calls(hwnd):
    """When ``_user32`` is None (its non-win32 module-load value) both helpers
    return None; the ``or`` disjunct in the guard covers a monkeypatched None
    even on win32."""
    with patch.object(ew, "_user32", None):
        assert ew.find_game_window() is None
        assert ew.get_window_geometry(hwnd) is None
