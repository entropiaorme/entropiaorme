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

The HTTP pipeline fixture boots the full FastAPI lifespan against a
throwaway data directory whose ``settings.json`` redirects the
``ChatlogWatcher`` onto a temp file. Scenarios drive the live HTTP
surface through a ``TestClient`` while replaying chat lines into the
same temp file the in-lifespan watcher tails. The
http-fingerprinter factory builds an ``HttpFingerprinter`` bound to
the named scenario directory, sharing the run's update-mode flag.
"""

from __future__ import annotations

import io
import json
import os
import shutil
import sqlite3
import tempfile
from collections.abc import Callable, Iterator
from contextlib import redirect_stdout
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from backend.core.event_bus import EventBus
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.testing.fingerprint import Normalizer
from backend.testing.golden import GoldenSet
from backend.testing.http_fingerprint import HttpFingerprinter
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
) -> Iterator[Callable[..., tuple[EventBus, HuntTracker, ChatlogWatcher, Path]]]:
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


@pytest.fixture
def e2e_http_pipeline(
    tmp_path: Path,
) -> Iterator[tuple[TestClient, Path]]:
    """Boot the full FastAPI lifespan against a temp data dir + chatlog.

    ``settings.json`` is pre-seeded so the in-lifespan ``ChatlogWatcher``
    tails the temp file the scenario will write into; the demo router's
    DB resolver is pointed at a seeded throwaway, so ``/api/demo/*``
    GETs are reachable during the contract substrate. Developer mode is
    enabled on the live config so the recording router's dev-gated
    surface is exercised in the same shape the contract suite sees.

    Yields ``(client, chatlog_path)``. The watcher inside the lifespan
    is started during ``with TestClient(app)``; the test streams the
    scenario's chat lines into the temp file via the existing
    ``replay_scenario`` helper. Teardown stops the lifespan, restores
    env, and removes the temp data dirs.
    """
    data_dir = Path(tempfile.mkdtemp(prefix="eo_http_fp_data_"))
    demo_dir = Path(tempfile.mkdtemp(prefix="eo_http_fp_demo_"))
    chatlog = tmp_path / "chat_testing.log"
    chatlog.touch()

    # Pre-seed settings.json so the lifespan-built ChatlogWatcher tails
    # the scenario's temp file rather than ~/Documents/Entropia/chat.log.
    (data_dir / "settings.json").write_text(
        json.dumps(
            {
                "chatlog_path": str(chatlog),
                "developer_mode_enabled": True,
            }
        ),
        encoding="utf-8",
    )

    # Import lazily so the env override is in place when backend.main's
    # lifespan reads ENTROPIAORME_DATA_DIR on TestClient enter.
    import backend.routers.demo as demo_module
    from backend.main import BACKEND_PORT, app
    from backend.scripts.demo_seed.__main__ import main as seed_demo

    with redirect_stdout(io.StringIO()):
        seed_demo(["--reseed", "--out", str(demo_dir)])
    demo_db = demo_dir / "entropia_orme.db"
    original_resolver = demo_module._resolve_demo_db_path
    demo_module._resolve_demo_db_path = lambda: demo_db
    demo_module._state["conn"] = None
    demo_module._state["svc"] = None

    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = str(data_dir)

    try:
        with TestClient(app, base_url=f"http://localhost:{BACKEND_PORT}") as client:
            yield client, chatlog
    finally:
        demo_module._resolve_demo_db_path = original_resolver
        demo_module._state["conn"] = None
        demo_module._state["svc"] = None
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir
        # ignore_errors: Windows may briefly hold the SQLite file open
        # past lifespan shutdown via the per-thread connection pool; a
        # leftover temp dir on a stuck handle is preferable to a teardown
        # crash that masks a real test failure.
        shutil.rmtree(data_dir, ignore_errors=True)
        shutil.rmtree(demo_dir, ignore_errors=True)


@pytest.fixture
def http_fingerprinter(
    update_fingerprints: bool,
) -> Callable[[Path], HttpFingerprinter]:
    """Factory: build an ``HttpFingerprinter`` for the scenario directory.

    Shares the run's update-mode flag with the existing ``golden_set``
    factory so a single ``--update-fingerprints`` invocation rewrites
    both the per-scenario fingerprint.jsonl + db_state.json goldens
    and the per-endpoint HTTP-response goldens.
    """

    def _make(scenario_dir: Path) -> HttpFingerprinter:
        return HttpFingerprinter(
            scenario_dir,
            Normalizer(),
            update=update_fingerprints,
        )

    return _make
