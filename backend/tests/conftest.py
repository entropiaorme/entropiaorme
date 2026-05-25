"""Shared pytest configuration for the backend test suite.

The runtime-tier markers (registered in pyproject.toml) are applied here by
test module, so the whole classification lives in one readable place:

- ``fast``:     pure-logic, in-memory, sub-second (every PR)
- ``standard``: db / filesystem / in-process-stateful (every PR)
- ``full``:     device / OCR / listener-touching or slow (nightly; none yet)

A module absent from the map defaults to ``standard`` (the broader, safer tier),
so a new test file always runs on PRs until it is deliberately classified.
"""

import pytest

# Test-module stem -> runtime tier.
_MODULE_TIERS = {
    "test_capturer": "fast",  # mss session is stubbed; no real device touched
    "test_character_calc": "fast",
    "test_chatlog_parser": "fast",
    "test_codex_formulas": "fast",
    "test_cost_engine": "fast",
    "test_scan_completion": "fast",
    "test_chatlog_watcher": "standard",
    "test_codex_service": "standard",
    "test_quests": "standard",
    "test_skill_tracker": "standard",
    "test_tracker_integration": "standard",
}


def pytest_collection_modifyitems(items):
    """Tag every collected test with its module's runtime tier."""
    for item in items:
        tier = _MODULE_TIERS.get(item.path.stem, "standard")
        item.add_marker(getattr(pytest.mark, tier))
