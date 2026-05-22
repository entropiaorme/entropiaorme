"""Test-mode runtime configuration.

``TestModeConfig`` is the process-wide overlay activated when
``ENTROPIA_TEST_MODE=1`` is set at backend startup. Services consult it
once during dependency-injection wiring and choose test-controlled
dependencies (mock keystroke source, fixture capturer, redirected
chatlog path) for the lifetime of the process. No service has a
runtime ``if test_mode`` branch in its hot path.

The R1 round lands the dataclass shape and the env-var loader; later
rounds wire the seam through ``backend/main.py``'s composition root as
each consuming service adopts its part of the harness.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


@dataclass
class TestModeConfig:
    """Runtime overlay flipped on at backend startup under test mode.

    Held as a process-wide singleton; consulted by services at startup
    during dependency-injection wiring. The flag is checked once when
    the composition root chooses which concrete dependency to wire in;
    no service has a runtime ``if test_mode`` branch in its hot path.

    Attributes
    ----------
    enabled:
        Whether the harness is active for this process.
    chatlog_path:
        Path the ``ChatlogWatcher`` should tail in test mode. When
        ``None`` and ``enabled`` is True, the caller derives the path
        from ``scenario_dir / "chat_replay.log"``.
    scenario_dir:
        Root of the currently-loaded scenario directory. Used to locate
        ``chat_replay.log``, ``scan_captures/``, ``keystrokes.jsonl``,
        and ``expected/`` golden files.
    fixture_dir:
        Directory the ``FixtureCapturer`` reads screenshots from.
        Defaults to ``scenario_dir / "scan_captures"`` when unset.
    """

    enabled: bool = False
    chatlog_path: Path | None = None
    scenario_dir: Path | None = None
    fixture_dir: Path | None = None

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "TestModeConfig":
        """Build a config from process environment.

        ``env`` defaults to ``os.environ`` but is overridable for
        deterministic unit testing of the loader itself.
        """

        source = env if env is not None else os.environ

        enabled = source.get("ENTROPIA_TEST_MODE") == "1"
        scenario = source.get("ENTROPIA_TEST_SCENARIO_DIR")
        chatlog = source.get("ENTROPIA_TEST_CHATLOG")
        fixtures = source.get("ENTROPIA_TEST_FIXTURE_DIR")

        scenario_path = Path(scenario) if scenario else None
        chatlog_path = Path(chatlog) if chatlog else (
            scenario_path / "chat_replay.log" if scenario_path else None
        )
        fixture_path = Path(fixtures) if fixtures else (
            scenario_path / "scan_captures" if scenario_path else None
        )

        return cls(
            enabled=enabled,
            chatlog_path=chatlog_path,
            scenario_dir=scenario_path,
            fixture_dir=fixture_path,
        )
