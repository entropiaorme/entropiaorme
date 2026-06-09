"""Whole-process external equivalence against the committed goldens.

Boots the backend as a real subprocess under test mode, drives the
``basic_hunt_10_events`` scenario purely through env vars and HTTP (no
in-process pokes anywhere), captures the three equivalence surfaces (the
``events.jsonl`` publish stream, the data-dir SQLite file, the curated
hydration GET set) and proves them byte-identical to the scenario's
committed goldens through the same Python emitters the committed
raw-capture fixtures are proven through.

This is the whole-process control leg of the cross-language equivalence
runner: it demonstrates an externally-driven backend process reaches a
drained, fingerprint-comparable state that reproduces the in-process
goldens exactly, so a second backend implementation driven the same way
is graded against the same bytes with this run as the known-good
reference.

The subprocess is terminated gracefully (POSIX signal / Windows console
event), never killed outright on the happy path, so its coverage data
flushes and the shutdown ordering contract (sink closed after every
producer stops) is exercised on every run.
"""

from __future__ import annotations

import json
import os
import signal
import socket
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from typing import IO, Any

import httpx
import pytest

from backend.testing import db_snapshot
from backend.testing.clock_plan import load_clock_plan
from backend.testing.fingerprint import Normalizer
from backend.testing.http_fingerprint import (
    HYDRATION_ENDPOINTS,
    HttpCapture,
    HttpRequest,
    HttpResponse,
    normalise_body,
    normalise_path,
    project_headers,
)

SCENARIO = Path(__file__).parent / "corpus" / "scripted" / "basic_hunt_10_events"
EXPECTED = SCENARIO / "expected"
REPO_ROOT = Path(__file__).resolve().parents[3]

_ORIGIN = {"Origin": "tauri://localhost"}
# Generous ceilings: reached only when the child is genuinely broken, never
# routine waits (health converges in well under a second once imports finish).
BOOT_TIMEOUT_S = 60.0
SHUTDOWN_TIMEOUT_S = 20.0


def _free_port() -> int:
    """An OS-assigned free port for the child to bind."""
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _tail(log_path: Path, lines: int = 40) -> str:
    try:
        return "\n".join(
            log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:]
        )
    except OSError:
        return "<no child log captured>"


def _spawn_backend(
    tmp_path: Path, port: int, data_dir: Path, log_file: IO[bytes]
) -> subprocess.Popen:
    """Boot ``backend.main`` as a subprocess with the external-harness env."""
    plan = load_clock_plan(SCENARIO)
    env = os.environ.copy()
    env.update(
        {
            "ENTROPIAORME_BACKEND_PORT": str(port),
            "ENTROPIAORME_DATA_DIR": str(data_dir),
            "ENTROPIA_TEST_MODE": "1",
            "ENTROPIA_TEST_SCENARIO_DIR": str(SCENARIO),
            "ENTROPIA_TEST_CHATLOG": str(tmp_path / "replay_sink.log"),
            "ENTROPIA_TEST_CLOCK_START": plan.start.isoformat(),
        }
    )

    kwargs: dict[str, Any] = {}
    if sys.platform == "win32":
        # A fresh process group so the graceful console event reaches only the
        # child. The child must SHARE this process's console (no
        # CREATE_NO_WINDOW / CREATE_NEW_CONSOLE): the console ctrl event that
        # delivers CTRL_BREAK can only reach process groups attached to the
        # sender's own console, so a child on a private console would be
        # unreachable and the graceful-shutdown contract below would fail.
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    return subprocess.Popen(
        [sys.executable, "-m", "backend.main"],
        cwd=REPO_ROOT,
        env=env,
        stdout=log_file,
        stderr=subprocess.STDOUT,
        **kwargs,
    )


def _wait_for_health(
    client: httpx.Client, proc: subprocess.Popen, log_path: Path
) -> None:
    """Poll the child's health route until it serves, with a hard deadline.

    An external process offers no condition variable to wait on, so this is
    a deadline poll by necessity; every later synchronisation point goes
    through the synchronous replay command instead.
    """
    deadline = time.monotonic() + BOOT_TIMEOUT_S
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            pytest.fail(
                f"backend exited during boot (rc={proc.returncode}):\n{_tail(log_path)}"
            )
        try:
            if client.get("/api/health").status_code == 200:
                return
        except httpx.TransportError:
            pass
        time.sleep(0.1)
    pytest.fail(f"backend did not serve health within {BOOT_TIMEOUT_S:g}s")


def _terminate_gracefully(proc: subprocess.Popen) -> None:
    """Ask the child to shut down and require that it does so cleanly."""
    if proc.poll() is not None:
        return
    if sys.platform == "win32":
        proc.send_signal(signal.CTRL_BREAK_EVENT)
    else:
        proc.send_signal(signal.SIGINT)
    try:
        proc.wait(timeout=SHUTDOWN_TIMEOUT_S)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=10.0)
        pytest.fail(
            f"backend did not shut down within {SHUTDOWN_TIMEOUT_S:g}s of the "
            "graceful signal"
        )


def test_external_process_run_reproduces_committed_goldens(tmp_path):
    """Boot, replay, capture, byte-compare: the full external contract."""
    port = _free_port()
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    # Mirror the golden-generation config seed (developer mode on); the
    # chatlog redirection is test mode's, not settings.json's.
    (data_dir / "settings.json").write_text(
        json.dumps({"developer_mode_enabled": True}), encoding="utf-8"
    )

    log_path = tmp_path / "backend_log.txt"
    captures: list[tuple[str, str, str, int, dict[str, str], bytes]] = []
    with log_path.open("wb") as log_file:
        proc = _spawn_backend(tmp_path, port, data_dir, log_file)
        try:
            with httpx.Client(
                base_url=f"http://127.0.0.1:{port}", timeout=30.0
            ) as client:
                _wait_for_health(client, proc, log_path)

                # The synchronous replay command IS the drain barrier: when it
                # returns, every synchronous bus subscriber has settled.
                replay = client.post("/api/testing/replay", headers=_ORIGIN)
                assert replay.status_code == 200, replay.text
                summary = replay.json()
                assert summary["drained"] is True
                assert summary["lines_seen"] == summary["lines_streamed"] > 0

                # Capture the hydration set in the canonical order so the
                # HTTP Normalizer's symbol table grows exactly as the
                # goldens' capture run grew it.
                for endpoint_id, method, path_template in HYDRATION_ENDPOINTS:
                    path = path_template.format(session_id=summary["session_id"])
                    response = client.get(path)
                    assert response.status_code == 200, (
                        f"{endpoint_id} ({method} {path}) returned "
                        f"{response.status_code}: {response.text!r}"
                    )
                    captures.append(
                        (
                            endpoint_id,
                            method,
                            path,
                            response.status_code,
                            dict(response.headers),
                            response.content,
                        )
                    )
        finally:
            _terminate_gracefully(proc)

    # Surface 1+2: the publish stream then the DB file through ONE shared
    # Normalizer (events assign the early symbols, the catalogue rows
    # continue the table), exactly as the committed goldens were produced.
    normalizer = Normalizer()
    events = [
        json.loads(line)
        for line in (data_dir / "events.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    assert events, f"no events captured; child log:\n{_tail(log_path)}"
    fingerprint_lines = [
        json.dumps(
            {
                "topic": event["topic"],
                "payload": normalizer.normalize(event["payload"]),
            },
            sort_keys=True,
            ensure_ascii=False,
        )
        for event in events
    ]
    actual_fingerprint = "\n".join(fingerprint_lines) + "\n"
    assert actual_fingerprint == (EXPECTED / "fingerprint.jsonl").read_text(
        encoding="utf-8"
    )

    conn = sqlite3.connect(data_dir / "entropia_orme.db")
    try:
        snapshot = db_snapshot.capture(conn, normalizer)
    finally:
        conn.close()
    assert db_snapshot.serialize(snapshot) == (EXPECTED / "db_state.json").read_text(
        encoding="utf-8"
    )

    # Surface 3: the hydration responses through a fresh Normalizer in the
    # canonical order (body normalised before path, as the fingerprinter
    # orders it), one committed golden per endpoint.
    http_normalizer = Normalizer()
    for endpoint_id, method, path, status_code, headers, body_bytes in captures:
        content_type = next(
            (v for k, v in headers.items() if k.lower() == "content-type"), None
        )
        # Body before path: the fingerprinter assigns body symbols first.
        body = normalise_body(body_bytes, content_type, http_normalizer)
        normalised_path = normalise_path(path, http_normalizer)
        golden = HttpCapture(
            request=HttpRequest(method=method, path=normalised_path, query={}),
            response=HttpResponse(
                status_code=status_code,
                headers=project_headers(headers),
                body=body,
            ),
        ).to_golden_dict()
        actual = json.dumps(golden, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
        expected = (EXPECTED / "http_responses" / f"{endpoint_id}.json").read_text(
            encoding="utf-8"
        )
        assert actual == expected, f"HTTP golden diverged for {endpoint_id}"
