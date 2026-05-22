"""Shared pytest configuration for the backend test suite.

The runtime-tier markers (registered in pyproject.toml) are applied here by
test module, so the whole classification lives in one readable place:

- ``fast``:     pure-logic, in-memory, sub-second (every PR)
- ``standard``: db / filesystem / in-process-stateful (every PR)
- ``full``:     device / OCR / listener-touching or slow (nightly; none yet)

A module absent from the map defaults to ``standard`` (the broader, safer tier),
so a new test file always runs on PRs until it is deliberately classified.

This module also registers the ``--update-fingerprints`` CLI option so the e2e
harness can flip its golden-file workflow into write mode. Hoisting it to this
backend-root conftest (rather than the e2e subdir conftest) keeps the flag
recognised regardless of which subset of tests is being collected.
"""

import os

import pytest
from hypothesis import settings

# Hypothesis settings profiles, selected via HYPOTHESIS_PROFILE (default "dev").
# Deadlines are disabled so example timing on shared runners never turns a
# deterministic property into a flaky failure (flakes are bugs, not reruns).
settings.register_profile("dev", max_examples=100, deadline=None)
settings.register_profile("ci", max_examples=300, deadline=None, print_blob=True)
settings.register_profile("nightly", max_examples=1000, deadline=None)
# The mutation campaign re-runs the property suites once per mutant and gates on
# the resulting score, so the run must be reproducible: derandomize fixes example
# generation, so a given mutant is killed (or not) identically on every run and
# the score cannot wobble across the floor. A slightly higher example budget than
# `dev` strengthens the kills without the nightly profile's intractable volume.
settings.register_profile("mutation", max_examples=200, deadline=None, derandomize=True)
settings.load_profile(os.environ.get("HYPOTHESIS_PROFILE", "dev"))

# Test-module stem -> runtime tier.
_MODULE_TIERS = {
    "test_capturer": "fast",  # mss session is stubbed; no real device touched
    "test_character_calc": "fast",
    "test_character_calc_properties": "fast",
    "test_chatlog_parser": "fast",
    "test_chatlog_parser_properties": "fast",
    "test_codex_endpoints": "fast",  # request-validation only; no lifespan or DB
    "test_codex_formulas": "fast",
    "test_codex_properties": "fast",
    "test_cost_engine": "fast",
    "test_cost_engine_properties": "fast",
    "test_loot_filter": "fast",
    "test_mob_lookup_service": "fast",
    "test_scan_completion": "fast",
    "test_scan_drift": "fast",
    "test_tool_inference": "fast",
    "test_tt_curve_properties": "fast",
    "test_analytics": "standard",  # AppDatabase-backed + SQL aggregation
    "test_analytics_activity": "standard",
    "test_api_contract": "standard",  # boots the app lifespan + ASGI schemathesis run
    "test_character_endpoints": "standard",
    "test_chatlog_watcher": "standard",
    "test_codex_service": "standard",
    "test_config_service": "standard",
    "test_equipment_endpoints": "standard",
    "test_quests": "standard",
    "test_skill_tracker": "standard",
    "test_tracker_integration": "standard",
    "test_tracking_endpoints": "standard",
    "test_trifecta_service": "standard",
    "test_tracker_stateful": "standard",
}


def pytest_addoption(parser):
    """Register backend-wide pytest CLI options.

    Currently exposes ``--update-fingerprints`` so the e2e harness can
    rewrite scenario goldens. Hoisting the registration to this
    backend-root conftest (rather than the e2e subdir conftest) keeps
    the flag recognised regardless of which subset of tests is being
    collected, so ``pytest backend/tests/test_fingerprint.py
    --update-fingerprints`` does not error on argument parsing.
    """
    parser.addoption(
        "--update-fingerprints",
        action="store_true",
        default=False,
        help=(
            "E2E harness: rewrite scenario goldens with the current run's "
            "output. Surfaces the diff vs the prior golden for review "
            "before writing; default behaviour without the flag asserts "
            "against goldens and fails on divergence."
        ),
    )


def pytest_collection_modifyitems(items):
    """Tag every collected test with its module's runtime tier."""
    for item in items:
        tier = _MODULE_TIERS.get(item.path.stem, "standard")
        item.add_marker(getattr(pytest.mark, tier))
