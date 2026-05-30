"""Property-based tests for the loot include / exclude filter.

Covers ``backend.tracking.loot_filter`` at the service surface: the key
normalisation invariants of ``_key`` (idempotence and canonical normal form)
and the per-entry guarantee that ``normalize_blacklist`` yields only normalised,
non-blank keys.

These are pure functions of their string inputs with no coupling to tracker
state, the event bus, or sessions, so each invariant is quantified directly
over generated strings.
"""

from hypothesis import assume, given
from hypothesis import strategies as st

from backend.tracking.loot_filter import (
    DEFAULT_BLACKLIST,
    _key,
    normalize_blacklist,
)

# A spread of hostile Unicode alongside plain ASCII: casefold-expanders (German
# sharp s, ligatures, dotted/dotless I, capital sharp s, U+0149), Greek sigma
# variants, compatibility letters, and exotic whitespace (NBSP, thin space,
# ideographic space, zero-width space, tab). These are the inputs most likely to
# break casefold-idempotence or to smuggle whitespace into an already-collapsed
# key. Spelled with escapes so no invisible character lurks in the source.
_HOSTILE = (
    "abc ABC"
    "ßﬀﬃİıẞŉ"
    "Σςσ"  # Greek capital/final/lowercase sigma
    "µÅ"  # micro sign, angstrom-ish A-with-ring
    "   　​\t"  # space, NBSP, thin, ideographic, zero-width, tab
)
_NAME = st.text(alphabet=_HOSTILE, min_size=0, max_size=24)


# --- _key normal form ---


@given(_NAME)
def test_key_is_idempotent(name):
    # key_idempotent: _key is a fixed point after a single application, so a
    # blacklist re-keyed at any later point never shifts.
    key = _key(name)
    assert _key(key) == key


@given(_NAME)
def test_key_is_in_canonical_normal_form(name):
    # key_normal_form: the output is casefolded and whitespace-collapsed, with
    # no leading, trailing, or run-internal whitespace and no further casefold.
    key = _key(name)
    assert key == key.casefold()
    assert key == " ".join(key.split())


# --- normalize_blacklist per-entry guarantee ---

# Precondition for the per-entry invariant as stated: a non-empty iterable, so
# the comprehension branch (not the falsy default fallback) is exercised. The
# guarantee also holds for the default branch, asserted separately below.
_BLACKLIST_NAMES = st.lists(_NAME, min_size=1, max_size=8)


@given(_BLACKLIST_NAMES)
def test_blacklist_entries_are_normalised_and_nonblank(names):
    # blacklist_entries_normalised_and_nonblank: every surviving entry is a
    # _key fixed point and non-empty, regardless of how blank or how exotically
    # cased the input names were.
    result = normalize_blacklist(names)
    assert isinstance(result, frozenset)
    for entry in result:
        assert entry == _key(entry)
        assert entry != ""


@given(_BLACKLIST_NAMES)
def test_blacklist_retains_every_non_blank_input(names):
    # Strengthens the above: a name with at least one non-whitespace run always
    # survives (as its key), so the non-blank guarantee is not satisfied by
    # silently discarding everything.
    assume(any(name and name.strip() for name in names))
    result = normalize_blacklist(names)
    for name in names:
        if name and name.strip():
            assert _key(name) in result


def test_default_blacklist_entries_are_normalised_and_nonblank():
    # The falsy-input branch returns DEFAULT_BLACKLIST, which must itself
    # satisfy the same per-entry guarantee.
    for entry in DEFAULT_BLACKLIST:
        assert entry == _key(entry)
        assert entry != ""
