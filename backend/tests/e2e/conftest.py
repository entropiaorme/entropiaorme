"""Shared fixtures for E2E replay harness tests.

The pipeline fixture boots the real ``ChatlogWatcher`` against a temp
file paired with a fresh ``HuntTracker`` over an in-memory SQLite DB.
Tests stream a scenario's lines into the temp file via
``backend.testing.replay.replay_scenario``; the watcher's real tail
loop reads them and publishes events through the bus as it would in
production.

The fixture intentionally uses no test-mode short-circuits — the watcher
runs its full daemon thread, the bus delivers synchronously, the
tracker writes to SQLite. The harness validates production behaviour,
not a stubbed pipeline.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Iterator

import pytest

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
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
    """

    bus = EventBus()
    tracker = HuntTracker(bus, in_memory_db)
    watcher = ChatlogWatcher(bus, temp_chatlog)
    watcher.start()
    try:
        yield bus, tracker, watcher, temp_chatlog
    finally:
        watcher.stop()
