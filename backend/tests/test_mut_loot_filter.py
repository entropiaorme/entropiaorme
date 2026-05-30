"""Mutation-hardening tests for backend.tracking.loot_filter.

Targets the include-filter in ``normalize_blacklist``:

    frozenset(_key(name) for name in names if name and name.strip())

A mutation rewrote the ``and`` to ``or`` (mutant
``x_normalize_blacklist__mutmut_4``). The two operators agree on empty
strings and on names with content, and diverge ONLY on whitespace-only
names:

* original ``name and name.strip()`` -> ``""`` (falsy) so the name is
  dropped, and the resulting set never contains the empty key.
* mutant ``name or name.strip()`` -> the truthy whitespace string, so the
  name is KEPT; since ``_key("   ") == ""`` the blacklist gains an empty
  ``""`` entry.

The tests below pin the original behaviour: whitespace-only names are
filtered out and never produce an empty-string blacklist key.
"""

from __future__ import annotations

from backend.tracking.loot_filter import (
    DEFAULT_BLACKLIST,
    _key,
    is_tracked_loot,
    normalize_blacklist,
)


def test_key_of_whitespace_only_is_empty_string() -> None:
    # Establishes why the mutant is observable: a blank name keys to "".
    assert _key("   ") == ""
    assert _key(" \t\n ") == ""


def test_whitespace_only_name_is_dropped() -> None:
    # Original: blank name filtered out -> empty set.
    # Mutant (`name or name.strip()`): blank name kept -> frozenset({""}).
    assert normalize_blacklist(["   "]) == frozenset()


def test_whitespace_only_names_do_not_add_empty_key_alongside_real_ones() -> None:
    result = normalize_blacklist(["  ", "Universal Ammo", "\t"])
    # Original keeps only the real entry; mutant additionally injects "".
    assert result == frozenset({"universal ammo"})
    assert "" not in result


def test_blacklist_never_contains_empty_key_blocking_real_items() -> None:
    # The empty key the mutant would inject is functionally a bug: _key("")
    # is "", so an item that keys to "" would be silently excluded from
    # tracking. Pin that a whitespace-only configured name does NOT cause
    # the empty-keyed sentinel to land in the blacklist.
    bl = normalize_blacklist(["    "])
    assert "" not in bl
    # is_tracked_loot keys an unknown/blank item to "" -> with the mutant's
    # injected "" key it would be reported as NOT tracked.
    assert is_tracked_loot("", bl) is True


def test_real_names_still_normalized_and_kept() -> None:
    # Guard the truthy branch is exercised normally (kills nothing on its
    # own but documents intended behaviour and protects the real path).
    assert normalize_blacklist(["  Universal   Ammo  "]) == frozenset(
        {"universal ammo"}
    )


def test_empty_iterable_falls_back_to_default() -> None:
    assert normalize_blacklist([]) == DEFAULT_BLACKLIST
    assert normalize_blacklist(None) == DEFAULT_BLACKLIST
