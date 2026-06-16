"""Live-DB snapshot primitive for real-session capture.

The real-session replay cross-check needs a session's STARTING database image
(captured before its chatlog segment) and the arm's POST-SESSION image, both
as standalone SQLite files in the recording bundle. ``snapshot_sqlite`` is the
WAL-safe online-backup primitive that produces them from a live connection
without pausing the writer; the recording controller calls it at session
start and stop.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path


def snapshot_sqlite(source: sqlite3.Connection, dest: Path) -> None:
    """Online-backup a live SQLite connection to a standalone file at ``dest``.

    Uses the stdlib :meth:`sqlite3.Connection.backup` API, which is safe to
    run against a live (WAL) database mid-session: it copies a transactionally
    consistent image without requiring the writer to stop. The caller is
    responsible for holding whatever lock guards concurrent use of ``source``
    (the production caller wraps this in the app DB's lock), because a single
    SQLite connection object must not be used concurrently from two threads.
    """
    dest.parent.mkdir(parents=True, exist_ok=True)
    destination = sqlite3.connect(str(dest))
    try:
        source.backup(destination)
    finally:
        destination.close()
