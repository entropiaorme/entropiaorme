"""Tests for the test-mode runtime configuration overlay.

Pins ``TestModeConfig`` defaults and the ``from_env`` loader: the
scenario directory derives the chatlog and fixture paths unless they
are given explicitly, and an absent test-mode flag yields an inert
config.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.config import TestModeConfig


def test_defaults_are_inert():
    config = TestModeConfig()
    assert config.enabled is False
    assert config.chatlog_path is None
    assert config.scenario_dir is None
    assert config.fixture_dir is None


def test_from_env_empty_is_disabled():
    config = TestModeConfig.from_env({})
    assert config.enabled is False
    assert config.chatlog_path is None
    assert config.scenario_dir is None
    assert config.fixture_dir is None


def test_from_env_enabled_flag():
    config = TestModeConfig.from_env({"ENTROPIA_TEST_MODE": "1"})
    assert config.enabled is True


def test_from_env_other_flag_value_is_disabled():
    config = TestModeConfig.from_env({"ENTROPIA_TEST_MODE": "true"})
    assert config.enabled is False


def test_scenario_dir_derives_chatlog_and_fixture_paths():
    config = TestModeConfig.from_env(
        {
            "ENTROPIA_TEST_MODE": "1",
            "ENTROPIA_TEST_SCENARIO_DIR": "/scenarios/hunt",
        }
    )
    assert config.scenario_dir == Path("/scenarios/hunt")
    assert config.chatlog_path == Path("/scenarios/hunt/chat_replay.log")
    assert config.fixture_dir == Path("/scenarios/hunt/scan_captures")


def test_explicit_chatlog_and_fixture_override_derivation():
    config = TestModeConfig.from_env(
        {
            "ENTROPIA_TEST_MODE": "1",
            "ENTROPIA_TEST_SCENARIO_DIR": "/scenarios/hunt",
            "ENTROPIA_TEST_CHATLOG": "/tmp/custom.log",
            "ENTROPIA_TEST_FIXTURE_DIR": "/tmp/panels",
        }
    )
    assert config.chatlog_path == Path("/tmp/custom.log")
    assert config.fixture_dir == Path("/tmp/panels")


def test_explicit_paths_without_scenario_dir():
    config = TestModeConfig.from_env(
        {
            "ENTROPIA_TEST_CHATLOG": "/tmp/custom.log",
            "ENTROPIA_TEST_FIXTURE_DIR": "/tmp/panels",
        }
    )
    assert config.scenario_dir is None
    assert config.chatlog_path == Path("/tmp/custom.log")
    assert config.fixture_dir == Path("/tmp/panels")
