"""Loot-item include/exclude decisions for tracking."""

from __future__ import annotations

from collections.abc import Iterable

DEFAULT_BLACKLIST = frozenset({"universal ammo"})


def _key(name: str) -> str:
    return " ".join(name.casefold().split())


def normalize_blacklist(names: Iterable[str] | None) -> frozenset[str]:
    if not names:
        return DEFAULT_BLACKLIST
    return frozenset(_key(name) for name in names if name and name.strip())


def is_tracked_loot(
    item_name: str, blacklist: frozenset[str] = DEFAULT_BLACKLIST
) -> bool:
    return _key(item_name) not in blacklist
