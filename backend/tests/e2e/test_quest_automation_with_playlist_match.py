"""Acceptance test for ``quest_automation_with_playlist_match``.

Drives two missions through the real chat-replay pipeline with
``QuestService`` wired into the bus and into the ``ChatlogWatcher``'s
``quest_reward_filter`` (mirroring ``main.py``'s production wiring).

Both quests live in a single playlist; on session stop, the
analytics-link suggestion API resolves the session to an
``exact_playlist`` match. Pins:

- ``QuestService`` auto-starts each quest on its received line.
- ``QuestService.quest_reward_filter`` auto-completes each quest on
  its completed line (via the watcher's tick path).
- ``get_session_link_suggestion`` returns the expected
  ``suggestion_type``, ``reason`` and ``playlist_name`` shape.

Uses its own pipeline (an ``AppDatabase`` and a ``ChatlogWatcher``
with the ``quest_reward_filter`` arg set) rather than the shared
``e2e_pipeline`` fixture, because ``QuestService`` requires an
``AppDatabase`` (not a raw ``sqlite3`` connection) and the
production wiring threads its filter into the watcher constructor.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.chatlog_watcher import ChatlogWatcher
from backend.services.quest_service import QuestService
from backend.testing.replay import replay_scenario, wait_for_drain
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def quest_automation_pipeline(
    tmp_path: Path,
) -> Iterator[
    tuple[
        EventBus,
        HuntTracker,
        QuestService,
        ChatlogWatcher,
        Path,
        AppDatabase,
    ]
]:
    """Boot the quest-automation pipeline.

    Yields ``(bus, tracker, quest_service, watcher, chatlog_path, app_db)``
    with the watcher already polling and ``QuestService`` wired into both
    the bus (for ``EVENT_MISSION_RECEIVED``) and the watcher (for the
    tick-time ``quest_reward_filter`` callback).
    """

    chatlog_path = tmp_path / "chat_testing.log"
    chatlog_path.touch()
    app_db = AppDatabase(tmp_path / "test.db")
    bus = EventBus()
    quest_service = QuestService(app_db, event_bus=bus)
    tracker = HuntTracker(bus, app_db.conn)
    watcher = ChatlogWatcher(
        bus,
        chatlog_path,
        quest_reward_filter=quest_service.quest_reward_filter,
    )
    watcher.start()
    try:
        yield bus, tracker, quest_service, watcher, chatlog_path, app_db
    finally:
        watcher.stop()
        app_db.close()


def test_quest_automation_resolves_session_to_playlist_exact_match(
    quest_automation_pipeline,
    corpus_root: Path,
    data_regression,
) -> None:
    """Two missions, same playlist, single session → ``exact_playlist``."""

    bus, tracker, quest_service, watcher, chatlog, app_db = quest_automation_pipeline

    # Pre-populate two quests, each carrying a small liquid reward so
    # completion writes a deterministic ledger row, and bundle them
    # into one playlist named in the scenario's metadata.
    alpha = quest_service.create_quest(
        {"name": "Alpha Hunt", "reward_ped": 1.50, "reward_is_skill": False}
    )
    beta = quest_service.create_quest(
        {"name": "Beta Hunt", "reward_ped": 1.50, "reward_is_skill": False}
    )
    playlist = quest_service.create_playlist(
        {"name": "Alpha + Beta", "quest_ids": [alpha["id"], beta["id"]]}
    )

    session = tracker.start_session()
    session_id = session.id

    scenario = corpus_root / "scripted" / "quest_automation_with_playlist_match"
    replay_scenario(scenario, chatlog)
    wait_for_drain(watcher, chatlog)
    result = tracker.stop_session()

    # Chat-side sanity: one kill per mission.
    assert len(result.kills) == 2
    assert result.kills[0].damage_dealt == pytest.approx(18.0)
    assert result.kills[1].damage_dealt == pytest.approx(22.0)

    # Quest-service sanity: both quests auto-completed (started_at
    # cleared back to None by complete_quest's UPDATE).
    refreshed_alpha = quest_service.get_quest(alpha["id"])
    refreshed_beta = quest_service.get_quest(beta["id"])
    assert refreshed_alpha is not None and refreshed_alpha["started_at"] is None
    assert refreshed_beta is not None and refreshed_beta["started_at"] is None

    # started_at is None also describes a quest that never started or one
    # whose state was cleared without recording a completion or its reward,
    # so pin the completion side effects directly. Each quest must leave
    # exactly one session_quest_completions row keyed to this session, and
    # each 1.50-PED liquid reward must leave exactly one quest_reward ledger
    # row at the expected amount, so a mutant that drops either write while
    # still clearing started_at is caught.
    conn = app_db.conn
    for quest_id in (alpha["id"], beta["id"]):
        completion_count = conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions "
            "WHERE session_id = ? AND quest_id = ?",
            (session_id, quest_id),
        ).fetchone()[0]
        assert completion_count == 1

    for quest_name in ("Alpha Hunt", "Beta Hunt"):
        reward_rows = conn.execute(
            "SELECT amount, type FROM ledger_entries "
            "WHERE tag = 'quest_reward' AND description = ?",
            (f"Quest: {quest_name}",),
        ).fetchall()
        assert len(reward_rows) == 1
        assert reward_rows[0]["amount"] == pytest.approx(1.50)
        assert reward_rows[0]["type"] == "markup"

    # Acceptance: the session resolves to the playlist by exact match.
    suggestion = quest_service.get_session_link_suggestion(session_id)
    assert suggestion["suggestion_type"] == "playlist"
    assert suggestion["reason"] == "exact_playlist"
    assert suggestion["playlist_id"] == playlist["id"]

    data_regression.check(
        {
            "kill_count": len(result.kills),
            "completed_quest_ids_order": _normalise_completed_quest_ids(
                quest_service, session_id
            ),
            "suggestion": {
                "suggestion_type": suggestion["suggestion_type"],
                "reason": suggestion["reason"],
                "playlist_name": suggestion["playlist_name"],
            },
        }
    )


def _normalise_completed_quest_ids(
    quest_service: QuestService, session_id: str
) -> list[str]:
    """Return the completed-quest *names* in completion order.

    The IDs themselves are an autoincrement-derived integer surface
    that drifts under DB-schema reseeds; the names are
    scenario-stable. ``_get_quest_name`` is documented as
    possibly-None for an unknown id, but every id surfaced by
    ``_get_session_completed_quest_ids`` is by construction a row
    that completed in this session and must exist, so a None here
    is a contract bug worth surfacing.
    """
    quest_ids = quest_service._get_session_completed_quest_ids(session_id)
    names: list[str] = []
    for qid in quest_ids:
        name = quest_service._get_quest_name(qid)
        assert name is not None, f"completed quest id {qid} resolved to no name"
        names.append(name)
    return names
