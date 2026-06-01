"""Shared pytest configuration for the backend test suite.

The runtime-tier markers (registered in pyproject.toml) are applied here by
test module, so the whole classification lives in one readable place:

- ``fast``:     pure-logic, in-memory, sub-second (every PR)
- ``standard``: db / filesystem / in-process-stateful (every PR)
- ``full``:     the slowest suites: the schemathesis contract suites
                and OCR equivalence (device / OCR / slow). Runs post-merge on a
                push to main and nightly, NOT on the per-PR gate, so a pull
                request waits only on fast + standard.

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
    "test_clock": "fast",
    "test_testing_config": "fast",  # env-loader logic; no real env touched
    "test_game_data_store": "fast",  # loads temp JSON snapshots; no real device
    "test_diff_renderer": "fast",
    "test_http_fingerprint": "fast",  # pure projection helpers
    "test_character_calc": "fast",
    "test_character_calc_properties": "fast",
    "test_chatlog_parser": "fast",
    "test_chatlog_parser_properties": "fast",
    "test_codex_endpoints": "fast",  # request-validation only; no lifespan or DB
    "test_codex_formulas": "fast",
    "test_cost_engine": "fast",
    "test_cost_engine_properties": "fast",
    "test_keystroke_source": "fast",  # pure-logic; pynput hook never started
    "test_loot_filter": "fast",
    "test_mob_lookup_service": "fast",
    "test_scan_completion": "fast",
    "test_scan_drift": "fast",
    "test_tool_inference": "fast",
    "test_tt_curve_properties": "fast",
    # Property suites over pure-logic services (in-memory, sub-second).
    "test_scan_drift_properties": "fast",
    "test_mob_lookup_service_properties": "fast",
    "test_tool_inference_properties": "fast",
    "test_skill_panel_parse_properties": "fast",
    "test_repair_ocr_properties": "fast",
    "test_trifecta_service_properties": "fast",
    "test_scan_presets_properties": "fast",
    "test_eu_window_properties": "fast",
    "test_loot_filter_properties": "fast",
    "test_golden_ratification_guard": "fast",  # pure stdlib + git, no app
    "test_authoring_lint": "fast",  # pure stdlib + git, no app
    "test_classify_change_scope": "fast",  # pure stdlib + git, no app
    "test_version_stamps": "fast",  # pure stdlib, reads tracked manifests
    "test_analytics": "standard",  # AppDatabase-backed + SQL aggregation
    "test_analytics_activity": "standard",
    # full tier: the slowest suites. Runs post-merge (push to main) and
    # nightly, not per-PR, so the per-PR gate stays fast. The per-PR coverage leg
    # still clears the floor without these (the API-surface walk/mutation tests
    # cover the same router branches the contract suites exercise). See ci.yml,
    # nightly.yml, and TESTING.md "Runtime tiers".
    "test_api_contract": "full",  # ASGI schemathesis run over the read surface
    "test_api_contract_with_state": "full",  # schemathesis over replayed state
    "test_ocr_equivalence": "full",  # real ONNX inference vs recorded panels (skips without the corpus)
    "test_character_endpoints": "standard",
    "test_chatlog_watcher": "standard",
    "test_codex_service": "standard",
    "test_config_service": "standard",
    "test_equipment_endpoints": "standard",
    "test_hotbar_listener": "standard",  # off-thread resolver + threading.Event waits
    "test_quests": "standard",
    "test_skill_tracker": "standard",
    "test_spacebar_capture_listener": "standard",  # threading.Event waits on capture
    "test_tracker_integration": "standard",
    "test_tracking_endpoints": "standard",
    "test_trifecta_service": "standard",
    "test_tracker_stateful": "standard",
    # E2E scenario tests: AppDatabase / ChatlogWatcher / off-thread joins.
    "test_hotbar_slot_use": "standard",
    "test_spacebar_scan_capture": "standard",
    "test_quest_automation_with_playlist_match": "standard",
    "test_input_listening_minimisation": "standard",
    "test_consistency_tracking_hunt_midpoint": "standard",
    "test_consistency_negative_control": "standard",
    "test_consistency_quests_mission_lifecycle_midpoint": "standard",
    "test_consistency_scan_isolation_midpoint": "standard",
    "test_consistency_codex_isolation_midpoint": "standard",
    "test_etag": "standard",  # boots the app lifespan via TestClient
    "test_openapi_drift": "fast",  # introspects app.openapi() at module load
    "test_coverage_matrix_drift": "fast",  # renders the matrix from source files
    "test_http_fingerprint_scenarios": "standard",  # per-test FastAPI lifespan
    "test_api_surface_walk": "standard",  # boots the app lifespan via TestClient
    "test_api_surface_mutations": "standard",  # boots the app lifespan via TestClient
    # Property and metamorphic suites that touch a db / filesystem / the app path.
    "test_codex_properties": "standard",  # now drives CodexService over a temp db
    "test_consistency_property": "standard",  # generated sequences through the watcher
    "test_metamorphic": "standard",  # threaded pipeline + in-memory db
    "test_scan_completion_properties": "standard",
    "test_session_summary_properties": "standard",
    "test_config_service_properties": "standard",  # filesystem persistence
    "test_quest_service_properties": "standard",
    "test_game_data_store_properties": "standard",  # filesystem snapshots
    "test_analytics_properties": "standard",
    "test_character_properties": "standard",
    "test_store_reducers_properties": "standard",
    "test_equipment_properties": "standard",
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


def pytest_collection_modifyitems(config, items):
    """Tag every collected test with its module's runtime tier, and (only under
    the ``loadgroup`` scheduler) assign xdist groups so the ``no_xdist`` escape
    hatch is honoured.

    The PR legs parallelise with ``--dist=loadfile``, where each file is the
    grouping unit and these xdist-group markers are ignored, so the loop below
    is inert for the everyday run. Switching a leg to ``--dist=loadgroup``
    activates the grouping: an unmarked test keeps file-level grouping (parity
    with loadfile), while every ``no_xdist`` test collapses onto one shared
    ``serial`` worker so it never runs concurrently with a test in another file.
    Activating the escape hatch is therefore a one-flag change, not a code
    change. No test is marked ``no_xdist`` today (see the survey in
    ``backend/testing/TESTING.md``).
    """
    loadgroup = config.getoption("dist", "no") == "loadgroup"
    for item in items:
        tier = _MODULE_TIERS.get(item.path.stem, "standard")
        item.add_marker(getattr(pytest.mark, tier))
        if loadgroup:
            group = "serial" if item.get_closest_marker("no_xdist") else item.path.stem
            item.add_marker(pytest.mark.xdist_group(group))
