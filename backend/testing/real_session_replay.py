"""Real-session offline replay cross-check for the db_state-silent writes.

The db_state-silent codex/quest/skill write tables (notably ``skill_gains``)
are mutated by real gameplay in ways the synthetic golden corpus does not
fully pin. This cross-check covers that surface: per real session,

  1. replay the captured chatlog SEGMENT through the FROZEN Python oracle,
     starting from the session's captured DB snapshot, and
  2. diff the resulting db_state fingerprint (over :data:`SILENT_WRITE_CATALOGUE`,
     which adds ``skill_gains`` to the canonical surface) against the Rust
     arm's ACTUAL post-session database.

A session bundle is ``{starting_db.sqlite, chat_replay.log, metadata.yaml}``
(produced for real sessions by the recording controller's DB-capture
extension, kept local-only). The reference the replay is diffed against is
the native arm's post-session DB (``--reference-db``), or a committed,
ratified golden (``--expected``) for the synthetic fixture that
keeps this path green in CI without real data.

Comparison contract: the chatlog-driven write columns
(``skill_gains.timestamp``, chatlog-source ``skill_calibrations.scanned_at``)
reproduce from the segment exactly; the few clock-read stamps (session
lifecycle) are symbolised to ``<TS_N>`` by encounter order, so the cross-check
is a STRUCTURAL / relative-order equality, not an absolute-instant one
(absolute sub-second instants are not reproducible offline). Each DB is
captured with its OWN Normalizer so structurally-equal states symbolise
identically.

Run it:
    python -m backend.testing.real_session_replay \
        --bundle <recorded-session-bundle> --reference-db <rust-post-session.db>
First-pin / regenerate the fixture golden:
    python -m backend.testing.real_session_replay \
        --bundle <bundle> --update
"""

from __future__ import annotations

import argparse
import json
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path
from typing import Any

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.services.quest_service import QuestService
from backend.services.skill_tracker import SkillTracker
from backend.testing import db_snapshot
from backend.testing.diff import diff_snapshot_dicts
from backend.testing.fingerprint import Normalizer
from backend.testing.replay import replay_scenario, wait_for_drain
from backend.tracking.tracker import HuntTracker

STARTING_DB = "starting_db.sqlite"
EXPECTED_GOLDEN = "expected_db_state.json"


def capture_db_state(db_path: Path) -> dict[str, Any]:
    """Capture a SQLite file's db_state over the silent-write catalogue with a
    fresh Normalizer (so two structurally-equal DBs symbolise identically)."""
    conn = sqlite3.connect(str(db_path))
    try:
        return db_snapshot.capture(
            conn, Normalizer(), db_snapshot.SILENT_WRITE_CATALOGUE
        )
    finally:
        conn.close()


def replay_bundle(bundle_dir: Path, *, player_name: str = "") -> dict[str, Any]:
    """Replay ``bundle_dir``'s chatlog segment through the frozen Python oracle
    starting from its DB snapshot; return the resulting db_state (silent-write
    catalogue).

    The producer pipeline mirrors the production composition root
    (``backend/main.py``): a real ``ChatlogWatcher`` feeding a ``HuntTracker``
    plus the ``SkillTracker`` (the ``skill_gains`` writer) and ``QuestService``
    (for the watcher's quest-reward filter), all sharing one ``AppDatabase``
    connection seeded from the starting snapshot. No clock is injected: session
    stamps are symbolised to ``<TS_N>`` by the snapshot Normalizer, matching the
    structural comparison contract.
    """
    starting = bundle_dir / STARTING_DB
    if not starting.is_file():
        raise FileNotFoundError(f"bundle missing {STARTING_DB}: {starting}")
    with tempfile.TemporaryDirectory() as td:
        work_db = Path(td) / "entropia_orme.db"
        shutil.copy(starting, work_db)
        app_db = AppDatabase(str(work_db))
        try:
            bus = EventBus()
            tracker = HuntTracker(bus, app_db.conn, player_name=player_name)
            quest_service = QuestService(app_db, bus)
            # SkillTracker subscribes to EVENT_SKILL_GAIN / session events on
            # construction; the binding is the side effect we need.
            SkillTracker(bus, app_db)
            chatlog = Path(td) / "chat.log"
            chatlog.touch()
            watcher = ChatlogWatcher(
                bus, chatlog, quest_reward_filter=quest_service.quest_reward_filter
            )
            watcher.start()
            try:
                tracker.start_session()
                replay_scenario(bundle_dir, chatlog)
                wait_for_drain(watcher, chatlog)
            finally:
                watcher.stop()
            tracker.stop_session()
            return db_snapshot.capture(
                app_db.conn, Normalizer(), db_snapshot.SILENT_WRITE_CATALOGUE
            )
        finally:
            app_db.close()


def cross_check(
    replay_state: dict[str, Any], reference_state: dict[str, Any]
) -> list[tuple[str, str | None]]:
    """Per-table verdicts: ``(table, None)`` when the replay matches the
    reference, ``(table, diff_message)`` when it diverges. Covers every table
    appearing in either state plus the whole silent-write catalogue."""
    tables = sorted(
        {spec.name for spec in db_snapshot.SILENT_WRITE_CATALOGUE}
        | set(replay_state)
        | set(reference_state)
    )
    verdicts: list[tuple[str, str | None]] = []
    for table in tables:
        ref_rows = reference_state.get(table, [])
        got_rows = replay_state.get(table, [])
        if ref_rows == got_rows:
            verdicts.append((table, None))
        else:
            message = diff_snapshot_dicts({table: ref_rows}, {table: got_rows})
            verdicts.append((table, message or "rows differ"))
    return verdicts


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="real_session_replay",
        description=(
            "Replay a real session's chatlog segment from its DB snapshot "
            "through the frozen Python oracle and diff db_state against the "
            "Rust arm's post-session DB (or a committed golden)."
        ),
    )
    parser.add_argument(
        "--bundle", required=True, type=Path, help="session bundle directory"
    )
    parser.add_argument(
        "--reference-db",
        type=Path,
        help="the Rust arm's actual post-session DB to diff against",
    )
    parser.add_argument(
        "--expected",
        type=Path,
        help="committed golden db_state JSON (defaults to <bundle>/"
        + EXPECTED_GOLDEN
        + ")",
    )
    parser.add_argument(
        "--update",
        action="store_true",
        help="write the replay's db_state as the expected golden (first-pin)",
    )
    parser.add_argument("--player-name", default="")
    args = parser.parse_args(argv)

    replay_state = replay_bundle(args.bundle, player_name=args.player_name)

    if args.update:
        out = args.expected or (args.bundle / EXPECTED_GOLDEN)
        out.write_text(
            db_snapshot.serialize(replay_state), encoding="utf-8", newline="\n"
        )
        print(f"[real-session-replay] wrote expected db_state -> {out}")
        return 0

    if args.reference_db is not None:
        reference_state = capture_db_state(args.reference_db)
        ref_label = f"Rust-arm post-session DB ({args.reference_db})"
    else:
        expected = args.expected or (args.bundle / EXPECTED_GOLDEN)
        if not expected.is_file():
            print(
                f"[real-session-replay] no reference: pass --reference-db or "
                f"--expected, or --update to first-pin {expected}",
                file=sys.stderr,
            )
            return 2
        reference_state = json.loads(expected.read_text(encoding="utf-8"))
        ref_label = f"committed golden ({expected})"

    verdicts = cross_check(replay_state, reference_state)
    diverged = [(t, m) for t, m in verdicts if m is not None]
    for table, message in verdicts:
        if message is None:
            print(f"[real-session-replay] {table}: MATCH")
        else:
            print(f"[real-session-replay] {table}: DIVERGENCE\n    {message}")
    print(f"[real-session-replay] reference: {ref_label}")
    if diverged:
        print(
            f"[real-session-replay] db_state divergence in {len(diverged)} "
            f"table(s) of {len(verdicts)}"
        )
        return 1
    print(
        f"[real-session-replay] zero db_state divergence over {len(verdicts)} "
        "tables (codex/quest/skill + skill_gains)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
