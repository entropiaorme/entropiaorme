"""Shared fixtures for E2E replay harness tests.

The pipeline fixture boots the real ``ChatlogWatcher`` against a temp
file paired with a fresh ``HuntTracker`` over an in-memory SQLite DB.
Tests stream a scenario's lines into the temp file via
``backend.testing.replay.replay_scenario``; the watcher's real tail
loop reads them and publishes events through the bus as it would in
production.

The fixture intentionally uses no test-mode short-circuits: the
watcher runs its full daemon thread, the bus delivers synchronously,
the tracker writes to SQLite. The harness validates production
behaviour, not a stubbed pipeline.

The golden-set fixture pairs the recorded event stream and DB
snapshot with a scenario's ``expected/`` directory, switching between
assert-against-golden mode (default) and write-new-golden mode
(``--update-fingerprints``) per the pytest CLI option registered in
``backend/tests/conftest.py``.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Callable, Iterator

import pytest

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.golden import GoldenSet
from backend.tracking.tracker import HuntTracker


E2E_DIR = Path(__file__).parent
CORPUS_ROOT = E2E_DIR / "corpus"


@pytest.fixture
def corpus_root() -> Path:
    """Absolute path to the scenario corpus root."""
    return CORPUS_ROOT


@pytest.fixture
def in_memory_db() -> Iterator[sqlite3.Connection]:
    """Fresh in-memory SQLite shared across the test's threads."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    try:
        yield db
    finally:
        db.close()


@pytest.fixture
def temp_chatlog(tmp_path: Path) -> Path:
    """Empty chat.log the watcher tails and the test writes into."""
    path = tmp_path / "chat_testing.log"
    path.touch()
    return path


@pytest.fixture
def e2e_pipeline(
    in_memory_db: sqlite3.Connection,
    temp_chatlog: Path,
) -> Iterator[tuple[EventBus, HuntTracker, ChatlogWatcher, Path]]:
    """Boot the harness pipeline against the temp chatlog.

    Yields ``(bus, tracker, watcher, chatlog_path)``. The watcher is
    started here so its tail loop is already polling when the test
    streams in the scenario; the fixture's teardown stops the watcher
    and lets the DB connection close on its own owner-fixture's exit.

    Defaults the tracker's ``player_name`` to the empty string so
    globals correlation is dormant; scenarios that exercise the
    correlation path use ``make_e2e_pipeline`` and pass a non-empty
    ``player_name``.
    """

    bus = EventBus()
    tracker = HuntTracker(bus, in_memory_db)
    watcher = ChatlogWatcher(bus, temp_chatlog)
    watcher.start()
    try:
        yield bus, tracker, watcher, temp_chatlog
    finally:
        watcher.stop()


@pytest.fixture
def make_e2e_pipeline(
    in_memory_db: sqlite3.Connection,
    temp_chatlog: Path,
) -> Iterator[
    Callable[..., tuple[EventBus, HuntTracker, ChatlogWatcher, Path]]
]:
    """Factory variant of ``e2e_pipeline`` accepting custom tracker args.

    A scenario that needs a non-default tracker configuration
    (notably ``player_name`` for globals-correlation scenarios)
    invokes::

        bus, tracker, watcher, chatlog = make_e2e_pipeline(
            player_name="ExpectedPlayer",
        )

    Every watcher the factory spawns is stopped at teardown so the
    fixture is safe to call once per test even if the test exits
    via exception. The factory shares the underlying in-memory DB
    and temp chatlog with the default ``e2e_pipeline`` fixture
    surface, which keeps wiring symmetric across scenario tests.
    """

    spawned: list[ChatlogWatcher] = []

    def _make(**tracker_kwargs) -> tuple[EventBus, HuntTracker, ChatlogWatcher, Path]:
        bus = EventBus()
        tracker = HuntTracker(bus, in_memory_db, **tracker_kwargs)
        watcher = ChatlogWatcher(bus, temp_chatlog)
        watcher.start()
        spawned.append(watcher)
        return bus, tracker, watcher, temp_chatlog

    try:
        yield _make
    finally:
        for watcher in spawned:
            watcher.stop()


@pytest.fixture
def update_fingerprints(request) -> bool:
    """True when the run was invoked with ``--update-fingerprints``."""
    return bool(request.config.getoption("--update-fingerprints"))


@pytest.fixture
def golden_set(update_fingerprints: bool) -> Callable[[Path], GoldenSet]:
    """Factory: build a ``GoldenSet`` for the named scenario directory.

    The factory shape keeps per-test wiring concise (one call yields
    both the recorder to install on the bus and the assert helper for
    end-of-test verification) while sharing the update-mode flag
    across every scenario in the run.
    """

    def _make(scenario_dir: Path) -> GoldenSet:
        return GoldenSet(scenario_dir, update=update_fingerprints)

    return _make
