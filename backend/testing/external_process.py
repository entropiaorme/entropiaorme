"""External whole-process backend harness.

Boots the backend as a real subprocess under test mode, drives a
scenario through the synchronous replay route, and captures the three
equivalence surfaces (the ``events.jsonl`` publish stream, the data-dir
SQLite file, the curated hydration GET set) in the exact normalised
form the committed goldens use.

The launch command is a parameter: every consumer here boots the Python
backend, and a second implementation of the same HTTP surface is graded
by pointing one leg's command at it while the capture and comparison
logic stays untouched. Consumed by the external-process equivalence
tests under ``backend/tests/e2e/``; deliberately free of any
test-framework dependency so non-pytest drivers can reuse it.
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
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import IO, Any

import httpx

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

REPO_ROOT = Path(__file__).resolve().parents[2]

#: The reference implementation's launch command; a second implementation
#: of the same surface takes one leg by overriding this per
#: ``ExternalBackendLeg``.
DEFAULT_COMMAND: tuple[str, ...] = (sys.executable, "-m", "backend.main")

_ORIGIN = {"Origin": "tauri://localhost"}
# Generous ceilings: reached only when the child is genuinely broken, never
# routine waits (health converges in well under a second once imports finish).
BOOT_TIMEOUT_S = 60.0
SHUTDOWN_TIMEOUT_S = 20.0


def free_ports(count: int = 1) -> list[int]:
    """OS-assigned free ports, guaranteed distinct from each other.

    Every socket is held open until all ports are read, so one call can
    never hand out the same port twice; the usual race against unrelated
    processes re-binding before the child does remains, as it must.
    """
    sockets = [socket.socket() for _ in range(count)]
    try:
        for sock in sockets:
            sock.bind(("127.0.0.1", 0))
        return [int(sock.getsockname()[1]) for sock in sockets]
    finally:
        for sock in sockets:
            sock.close()


@dataclass(frozen=True)
class LegSurfaces:
    """The three normalised equivalence surfaces of one completed run."""

    fingerprint: str
    db_state: str
    http_responses: dict[str, str]


def expected_surfaces(scenario: Path) -> LegSurfaces:
    """A scenario's committed goldens, in the same shape a leg produces."""
    expected = scenario / "expected"
    return LegSurfaces(
        fingerprint=(expected / "fingerprint.jsonl").read_text(encoding="utf-8"),
        db_state=(expected / "db_state.json").read_text(encoding="utf-8"),
        http_responses={
            path.stem: path.read_text(encoding="utf-8")
            for path in sorted((expected / "http_responses").glob("*.json"))
        },
    )


class ExternalBackendLeg:
    """One externally driven backend process and its captured surfaces.

    Lifecycle: ``start`` then ``wait_ready`` then ``replay`` then
    ``capture_http`` then ``shutdown`` then ``surfaces``. ``shutdown`` is
    idempotent and safe from cleanup paths regardless of how far the run
    got; ``surfaces`` reads the on-disk outputs, so it runs after
    ``shutdown`` has flushed and closed them.
    """

    def __init__(
        self,
        scenario: Path,
        work_dir: Path,
        port: int,
        command: Sequence[str] = DEFAULT_COMMAND,
    ) -> None:
        self.scenario = scenario
        self.work_dir = work_dir
        self.port = port
        self.command = tuple(command)
        self.data_dir = work_dir / "data"
        self.log_path = work_dir / "backend_log.txt"
        self._proc: subprocess.Popen[bytes] | None = None
        self._log_file: IO[bytes] | None = None
        self._client: httpx.Client | None = None
        self._captures: list[tuple[str, str, str, int, dict[str, str], bytes]] = []
        self._summary: dict[str, Any] | None = None

    def tail(self, lines: int = 40) -> str:
        """The end of the child's combined output, for diagnostics."""
        try:
            return "\n".join(
                self.log_path.read_text(
                    encoding="utf-8", errors="replace"
                ).splitlines()[-lines:]
            )
        except OSError:
            return "<no child log captured>"

    def start(self) -> None:
        """Boot the backend subprocess with the external-harness env."""
        self.data_dir.mkdir(parents=True)
        # Mirror the golden-generation config seed (developer mode on); the
        # chatlog redirection is test mode's, not settings.json's.
        (self.data_dir / "settings.json").write_text(
            json.dumps({"developer_mode_enabled": True}), encoding="utf-8"
        )
        plan = load_clock_plan(self.scenario)
        env = os.environ.copy()
        env.update(
            {
                "ENTROPIAORME_BACKEND_PORT": str(self.port),
                "ENTROPIAORME_DATA_DIR": str(self.data_dir),
                "ENTROPIA_TEST_MODE": "1",
                "ENTROPIA_TEST_SCENARIO_DIR": str(self.scenario),
                "ENTROPIA_TEST_CHATLOG": str(self.work_dir / "replay_sink.log"),
                "ENTROPIA_TEST_CLOCK_START": plan.start.isoformat(),
            }
        )

        kwargs: dict[str, Any] = {}
        if sys.platform == "win32":
            # A fresh process group so the graceful console event reaches only
            # the child. The child must SHARE this process's console (no
            # CREATE_NO_WINDOW / CREATE_NEW_CONSOLE): the console ctrl event
            # that delivers CTRL_BREAK can only reach process groups attached
            # to the sender's own console, so a child on a private console
            # would be unreachable and the graceful-shutdown contract below
            # would fail.
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        self._log_file = self.log_path.open("wb")
        self._proc = subprocess.Popen(
            list(self.command),
            cwd=REPO_ROOT,
            env=env,
            stdout=self._log_file,
            stderr=subprocess.STDOUT,
            **kwargs,
        )
        self._client = httpx.Client(
            base_url=f"http://127.0.0.1:{self.port}", timeout=30.0
        )

    def wait_ready(self) -> None:
        """Poll the child's health route until it serves, with a deadline.

        An external process offers no condition variable to wait on, so this
        is a deadline poll by necessity; every later synchronisation point
        goes through the synchronous replay command instead.
        """
        proc, client = self._require_started()
        deadline = time.monotonic() + BOOT_TIMEOUT_S
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(
                    f"backend exited during boot (rc={proc.returncode}):\n{self.tail()}"
                )
            try:
                if client.get("/api/health").status_code == 200:
                    return
            except httpx.TransportError:
                pass
            time.sleep(0.1)
        raise RuntimeError(f"backend did not serve health within {BOOT_TIMEOUT_S:g}s")

    @property
    def pid(self) -> int:
        """The running child's process id (for resource measurements)."""
        proc, _ = self._require_started()
        return proc.pid

    def replay(self) -> dict[str, Any]:
        """Drive the loaded scenario; return the settled replay summary.

        The synchronous replay command IS the drain barrier: when it
        returns, every synchronous bus subscriber has settled. The summary
        must report a drained run that streamed the scenario's whole
        (non-empty) chatlog; anything else is a broken run, surfaced here.
        """
        _, client = self._require_started()
        response = client.post("/api/testing/replay", headers=_ORIGIN)
        if response.status_code != 200:
            raise RuntimeError(
                f"replay returned {response.status_code}: {response.text}"
            )
        summary: dict[str, Any] = response.json()
        if not summary.get("drained"):
            raise RuntimeError(f"replay did not drain: {summary}")
        if not (summary["lines_seen"] == summary["lines_streamed"] > 0):
            raise RuntimeError(f"replay line counts diverge or are empty: {summary}")
        self._summary = summary
        return summary

    def capture_http(self) -> None:
        """Capture the hydration set in the canonical order.

        The order matters: the HTTP Normalizer's symbol table must grow
        exactly as the goldens' capture run grew it.
        """
        _, client = self._require_started()
        if self._summary is None:
            raise RuntimeError("capture_http called before replay")
        for endpoint_id, method, path_template in HYDRATION_ENDPOINTS:
            path = path_template.format(session_id=self._summary["session_id"])
            response = client.get(path)
            if response.status_code != 200:
                raise RuntimeError(
                    f"{endpoint_id} ({method} {path}) returned "
                    f"{response.status_code}: {response.text!r}"
                )
            self._captures.append(
                (
                    endpoint_id,
                    method,
                    path,
                    response.status_code,
                    dict(response.headers),
                    response.content,
                )
            )

    def shutdown(self) -> None:
        """Ask the child to shut down and require that it does so cleanly.

        Graceful (POSIX signal / Windows console event), never an outright
        kill on the happy path, so the child's coverage data flushes and the
        shutdown ordering contract (sink closed after every producer stops)
        is exercised on every run. Idempotent.
        """
        if self._client is not None:
            self._client.close()
            self._client = None
        proc = self._proc
        self._proc = None
        try:
            if proc is not None and proc.poll() is None:
                if sys.platform == "win32":
                    proc.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    proc.send_signal(signal.SIGINT)
                try:
                    proc.wait(timeout=SHUTDOWN_TIMEOUT_S)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=10.0)
                    raise RuntimeError(
                        f"backend did not shut down within {SHUTDOWN_TIMEOUT_S:g}s "
                        "of the graceful signal"
                    ) from None
        finally:
            if self._log_file is not None:
                self._log_file.close()
                self._log_file = None

    def surfaces(self) -> LegSurfaces:
        """Normalise the three captured surfaces; call after ``shutdown``."""
        # Surface 1+2: the publish stream then the DB file through ONE shared
        # Normalizer (events assign the early symbols, the catalogue rows
        # continue the table), exactly as the committed goldens were produced.
        normalizer = Normalizer()
        events = [
            json.loads(line)
            for line in (self.data_dir / "events.jsonl")
            .read_text(encoding="utf-8")
            .splitlines()
        ]
        if not events:
            raise RuntimeError(f"no events captured; child log:\n{self.tail()}")
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
        fingerprint = "\n".join(fingerprint_lines) + "\n"

        conn = sqlite3.connect(self.data_dir / "entropia_orme.db")
        try:
            snapshot = db_snapshot.capture(conn, normalizer)
        finally:
            conn.close()
        db_state = db_snapshot.serialize(snapshot)

        # Surface 3: the hydration responses through a fresh Normalizer in
        # the canonical order (body normalised before path, as the
        # fingerprinter orders it).
        http_normalizer = Normalizer()
        http_responses: dict[str, str] = {}
        for (
            endpoint_id,
            method,
            path,
            status_code,
            headers,
            body_bytes,
        ) in self._captures:
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
            http_responses[endpoint_id] = (
                json.dumps(golden, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
            )
        return LegSurfaces(
            fingerprint=fingerprint,
            db_state=db_state,
            http_responses=http_responses,
        )

    def _require_started(self) -> tuple[subprocess.Popen[bytes], httpx.Client]:
        if self._proc is None or self._client is None:
            raise RuntimeError("backend leg is not running (start it first)")
        return self._proc, self._client
