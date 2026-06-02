"""Supervised-worker / no-orphan guard: the backend half of the polling-and-
orphan enforcement pair (its frontend twin is the no-bare-``setInterval`` lint).

The backend has zero ``asyncio.create_task`` workers by design. The one place a
long-lived task could live, the SSE fan-out (``services/event_stream.py``),
deliberately has none: each ``GET /api/events`` connection's drain is owned by
its Starlette response lifecycle, which uvicorn supervises and tears down on
disconnect. Every remaining long-lived worker is an OS thread that is named,
daemonised, owned by a service, and cancelled on shutdown.

This module pins that shape as a *checked* artefact so it cannot silently
regress ("convention without enforcement is the failure mode"). Three guards:

* a static scan asserting no production code spawns a free, unsupervised
  coroutine (``create_task`` / ``ensure_future`` / ``run_in_executor`` /
  ``run_coroutine_threadsafe`` / ``asyncio.run``);
* a static scan asserting every ``threading.Thread`` literal in production code
  is constructed ``daemon=True`` with an explicit ``name=``;
* runtime checks that the app lifespan detaches the SSE hub from the bus on
  shutdown, and that the one app-lifetime worker (the chat.log watcher) actually
  terminates when stopped.

This is the tokio ``JoinSet`` "every spawned worker is owned and joined on
shutdown" invariant pinned ahead of the Rust port.

Scope: the static scan covers all of ``backend/`` except the ``backend/tests/``
suite itself. ``backend/testing/`` IS in scope: it is production-imported
(``keystroke_source`` builds the live input-hook threads; ``recording_controller``
is wired into the lifespan), so excluding it would open a false-negative hole.
The two OS-keyboard-hook listener threads are ``pynput`` ``keyboard.Listener``
instances, not ``threading.Thread`` literals, so the thread-literal scan does not
reach them by construction; they are named at construction (see
``PynputKeystrokeSource``, exercised in ``test_keystroke_source``) and their
owners stop them on shutdown.
"""

from __future__ import annotations

import ast
from pathlib import Path

BACKEND_ROOT = Path(__file__).resolve().parent.parent
TESTS_DIR = BACKEND_ROOT / "tests"

# Attribute names that spawn a free / unsupervised unit of async work. A handler
# that needs background work must instead own a named worker that is cancelled on
# shutdown (the shape that ports to a Rust JoinSet handle).
_FORBIDDEN_ASYNC_SPAWNS = frozenset(
    {"create_task", "ensure_future", "run_in_executor", "run_coroutine_threadsafe"}
)


def _production_py_files() -> list[Path]:
    """Every backend ``*.py`` except the test suite (``backend/tests/``)."""
    return [p for p in BACKEND_ROOT.rglob("*.py") if TESTS_DIR not in p.parents]


def _async_spawn_findings(tree: ast.AST, label: str) -> list[str]:
    """``label:line`` for each forbidden free-coroutine spawn in ``tree``."""
    findings: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        if isinstance(func, ast.Attribute):
            if func.attr in _FORBIDDEN_ASYNC_SPAWNS:
                findings.append(f"{label}:{node.lineno} ({func.attr})")
            elif (
                func.attr == "run"
                and isinstance(func.value, ast.Name)
                and func.value.id == "asyncio"
            ):
                findings.append(f"{label}:{node.lineno} (asyncio.run)")
        elif isinstance(func, ast.Name) and func.id in _FORBIDDEN_ASYNC_SPAWNS:
            # A from-import bare-name spawn (``from asyncio import create_task``):
            # the four names are distinctive enough to match unqualified without
            # false positives. ``run`` is matched in qualified form only, above.
            findings.append(f"{label}:{node.lineno} ({func.id})")
    return findings


def _is_threading_thread_call(node: ast.Call) -> bool:
    """True when ``node`` is a ``threading.Thread(...)`` construction."""
    func = node.func
    return (
        isinstance(func, ast.Attribute)
        and func.attr == "Thread"
        and isinstance(func.value, ast.Name)
        and func.value.id == "threading"
    )


def _thread_findings(tree: ast.AST, label: str) -> list[str]:
    """``label:line`` for each Thread literal lacking ``daemon=True`` + ``name=``."""
    findings: list[str] = []
    for node in ast.walk(tree):
        if not (isinstance(node, ast.Call) and _is_threading_thread_call(node)):
            continue
        kwargs = {kw.arg: kw.value for kw in node.keywords if kw.arg}
        daemon = kwargs.get("daemon")
        is_daemon = isinstance(daemon, ast.Constant) and daemon.value is True
        name = kwargs.get("name")
        # A name= must be present and, if a literal, non-empty; a non-literal
        # (a variable/expression) is accepted as an intentional name.
        has_name = name is not None and not (
            isinstance(name, ast.Constant) and not name.value
        )
        if is_daemon and has_name:
            continue
        missing = []
        if not is_daemon:
            missing.append("daemon=True")
        if not has_name:
            missing.append("name=")
        findings.append(f"{label}:{node.lineno} (missing {', '.join(missing)})")
    return findings


def find_unsupervised_async_spawns() -> list[str]:
    """Scan production code for free-coroutine spawns."""
    findings: list[str] = []
    for path in _production_py_files():
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        findings.extend(
            _async_spawn_findings(tree, str(path.relative_to(BACKEND_ROOT)))
        )
    return findings


def find_unsupervised_threads() -> list[str]:
    """Scan production code for Thread literals missing daemon/name."""
    findings: list[str] = []
    for path in _production_py_files():
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        findings.extend(_thread_findings(tree, str(path.relative_to(BACKEND_ROOT))))
    return findings


# ── Static guards over the live tree ──


def test_no_unsupervised_async_spawns_in_production() -> None:
    """No production code spawns a free / unsupervised coroutine."""
    findings = find_unsupervised_async_spawns()
    assert findings == [], (
        "Production code must not spawn a free, unsupervised coroutine. Route "
        "long-lived work through an owned, named, cancel-on-shutdown worker "
        "(the shape that maps onto a Rust JoinSet handle). Offenders:\n  "
        + "\n  ".join(findings)
    )


def test_every_production_thread_is_named_and_daemon() -> None:
    """Every production ``threading.Thread`` is daemon=True with an explicit name."""
    findings = find_unsupervised_threads()
    assert findings == [], (
        "Every threading.Thread in production must be daemon=True with an "
        "explicit name= (identifiable in a thread dump, owned by its service). "
        "Offenders:\n  " + "\n  ".join(findings)
    )


# ── The scanners themselves must have teeth (a guideline masquerading as a gate
#    is the failure mode this guard exists to prevent): each scanner flags a
#    planted violation and passes a compliant construction. ──


def test_async_spawn_scanner_has_teeth() -> None:
    bad = "import asyncio\nasync def f():\n    asyncio.create_task(g())\n"
    assert _async_spawn_findings(ast.parse(bad), "x.py")
    bad_run = "import asyncio\nasyncio.run(main())\n"
    assert _async_spawn_findings(ast.parse(bad_run), "x.py")
    from_import = "from asyncio import create_task\ncreate_task(g())\n"
    assert _async_spawn_findings(ast.parse(from_import), "x.py")
    good = "import asyncio\nasync def f():\n    await g()\n"
    assert _async_spawn_findings(ast.parse(good), "x.py") == []


def test_thread_scanner_has_teeth() -> None:
    nameless = "import threading\nthreading.Thread(target=f, daemon=True).start()\n"
    assert _thread_findings(ast.parse(nameless), "x.py")
    non_daemon = "import threading\nthreading.Thread(target=f, name='w').start()\n"
    assert _thread_findings(ast.parse(non_daemon), "x.py")
    empty_name = (
        "import threading\nthreading.Thread(target=f, name='', daemon=True).start()\n"
    )
    assert _thread_findings(ast.parse(empty_name), "x.py")
    good = (
        "import threading\nthreading.Thread(target=f, name='w', daemon=True).start()\n"
    )
    assert _thread_findings(ast.parse(good), "x.py") == []


# ── Runtime supervised-shutdown invariants ──


def test_lifespan_detaches_sse_hub_from_bus() -> None:
    """The app lifespan unsubscribes the SSE hub from the bus on shutdown.

    ``event_stream_hub.close()`` runs first in the lifespan teardown so a
    producer's final shutdown event cannot hop a frame onto a closing loop, and
    the bus retains no reference to the hub.
    """
    from fastapi.testclient import TestClient

    from backend.core.domain_events import (
        TOPIC_SCAN_STATUS_CHANGED,
        TOPIC_TRACKING_SESSION_UPDATED,
    )
    from backend.dependencies import get_services
    from backend.main import create_app

    app = create_app()
    with TestClient(app):
        hub = get_services().event_stream_hub
        bus = hub._event_bus
        # Live phase: the hub is subscribed to every forwarded domain topic.
        assert bus.has_subscribers(TOPIC_TRACKING_SESSION_UPDATED)
        assert bus.has_subscribers(TOPIC_SCAN_STATUS_CHANGED)

    # Shutdown ran event_stream_hub.close() first: the hub is detached and idle.
    assert not bus.has_subscribers(TOPIC_TRACKING_SESSION_UPDATED)
    assert not bus.has_subscribers(TOPIC_SCAN_STATUS_CHANGED)
    assert hub.connection_count == 0


def test_chatlog_watcher_thread_terminates_on_stop(tmp_path) -> None:
    """The one app-lifetime worker cooperatively cancels and its thread joins.

    Driven directly (the bare app lifespan never starts it without a real
    chat.log), so the supervised-shutdown invariant is exercised, not assumed.
    """
    from backend.core.event_bus import EventBus
    from backend.services.chatlog_watcher import ChatlogWatcher

    logfile = tmp_path / "chat.log"
    logfile.write_text("", encoding="utf-8")

    watcher = ChatlogWatcher(EventBus(), str(logfile))
    watcher.start()
    thread = watcher._thread
    assert thread is not None
    assert thread.is_alive()
    assert thread.name == "chatlog-watcher"
    assert thread.daemon is True

    watcher.stop()
    assert watcher._thread is None
    assert not thread.is_alive()
