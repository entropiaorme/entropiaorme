"""Property-based and unit tests for loot include / exclude decisions.

Covers ``backend.tracking.loot_filter``, which decides whether a looted item is
tracked or filtered out by a (normalised) blacklist.
"""

from hypothesis import assume, given
from hypothesis import strategies as st

from backend.tracking.loot_filter import (
    DEFAULT_BLACKLIST,
    _key,
    is_tracked_loot,
    normalize_blacklist,
)

# --- _key normalisation ---


@given(st.text())
def test_key_is_idempotent(name):
    assert _key(_key(name)) == _key(name)


@given(st.text())
def test_key_is_casefolded_and_whitespace_collapsed(name):
    key = _key(name)
    assert key == key.casefold()
    assert key == " ".join(key.split())


@given(st.text(), st.integers(min_value=0, max_value=4))
def test_key_ignores_surrounding_whitespace(name, pad):
    assert _key(" " * pad + name + " " * pad) == _key(name)


# --- normalize_blacklist ---


@given(st.one_of(st.none(), st.just(""), st.just([]), st.just(set())))
def test_normalize_blacklist_falsy_returns_default(falsy):
    assert normalize_blacklist(falsy) == DEFAULT_BLACKLIST


@given(st.lists(st.text()))
def test_normalize_blacklist_normalises_and_drops_blanks(names):
    result = normalize_blacklist(names)
    assert isinstance(result, frozenset)
    if not names:  # an empty iterable is falsy and falls back to the default
        assert result == DEFAULT_BLACKLIST
        return
    for entry in result:
        assert entry == _key(entry)  # every retained entry is normalised
        assert entry != ""  # and non-blank
    for name in names:
        if name and name.strip():
            assert _key(name) in result


# --- is_tracked_loot ---


@given(st.text(), st.lists(st.text()))
def test_is_tracked_loot_is_complement_of_membership(item, names):
    blacklist = normalize_blacklist(names)
    assert is_tracked_loot(item, blacklist) == (_key(item) not in blacklist)


@given(st.text())
def test_name_is_filtered_by_a_blacklist_built_from_itself(name):
    assume(name.strip())
    assert is_tracked_loot(name, normalize_blacklist([name])) is False


@given(
    st.sampled_from(
        ["universal ammo", "Universal Ammo", "  UNIVERSAL   AMMO  ", "uNiVeRsAl ammo"]
    )
)
def test_default_blacklist_filters_universal_ammo_case_insensitively(variant):
    assert is_tracked_loot(variant) is False


# --- plain units ---


def test_default_blacklist_contains_universal_ammo():
    assert "universal ammo" in DEFAULT_BLACKLIST


def test_normal_item_is_tracked():
    assert is_tracked_loot("Animal Oil Residue") is True
