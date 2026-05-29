"""Mutation-hardening tests for QuestService.start_quest_from_mission.

Cluster: quest_service__c5.

start_quest_from_mission has exactly two externally observable effects:

  1. It auto-starts a matched, not-yet-started quest via ``start_quest`` (sets
     ``started_at`` on the quest row).
  2. On a successful auto-start it records a ``quest_started`` notable event
     (a ``notable_events`` DB row) when a tracking session is active.

Everything else in the method is logging. The surviving mutants in this cluster
all alter either:

  * the ``log.info``/``log.debug`` *format string* or its *argument list* in one
    of the three log statements (no-match, already-started, started), or
  * the literal arguments passed to ``self._record_notable_event``.

The logging statements are observable through pytest's ``caplog`` fixture: the
captured ``LogRecord`` retains the exact format string (``record.msg``) and the
exact positional argument tuple (``record.args``). Asserting on both pins every
text edit, case flip, ``None`` substitution and dropped-argument mutation
without depending on lazy ``%``-formatting succeeding (it raises for some of the
mutants, but the record - with its mutated ``msg``/``args`` - is still captured).

The ``_record_notable_event`` mutations are killed through the ``notable_events``
row the call writes.
"""

from __future__ import annotations

import logging
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_SESSION_STARTED
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService

LOGGER_NAME = "backend.services.quest_service"

# The three exact format strings emitted by start_quest_from_mission. If any
# mutant edits the text (XX-wrap, case flip) or replaces the literal with None,
# the captured record.msg stops equalling these.
NO_MATCH_MSG = "Mission received '%s' \u2014 no matching quest in DB, ignoring"
ALREADY_MSG = "Quest '%s' already started, skipping auto-start"
STARTED_MSG = "Started quest '%s' (id=%d) from chat.log mission '%s'"


@pytest.fixture
def quest_service(tmp_path: Path) -> QuestService:
    db = AppDatabase(tmp_path / "test.db")
    return QuestService(db)


def _make_service_with_events(tmp_path: Path) -> tuple[QuestService, EventBus]:
    """A QuestService wired to an EventBus and the table _record_notable_event
    needs, plus an EventBus to activate a tracking session."""
    db = AppDatabase(tmp_path / "test.db")
    db.conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS notable_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            kill_id TEXT,
            event_type TEXT,
            mob_or_item TEXT,
            value_ped REAL,
            timestamp REAL
        );
        """
    )
    db.conn.commit()
    bus = EventBus()
    svc = QuestService(db, bus)
    return svc, bus


def _records(caplog: pytest.LogCaptureFixture) -> list[logging.LogRecord]:
    return [r for r in caplog.records if r.name == LOGGER_NAME]


# ── No-match branch: log.info(NO_MATCH_MSG, mission_name) ───────────────────
# Kills mutmut_4 (msg->None), 5 (mission_name->None), 6 (format dropped),
# 7 (arg dropped), 8 (XX-wrap), 9 (lower), 10 (upper).
class TestNoMatchLog:
    def test_no_match_logs_exact_format_and_arg(
        self, quest_service: QuestService, caplog: pytest.LogCaptureFixture
    ) -> None:
        quest_service.create_quest({"name": "Paneleon Hunter"})
        with caplog.at_level(logging.DEBUG, logger=LOGGER_NAME):
            quest_service.start_quest_from_mission("Totally Unknown XYZ")

        recs = _records(caplog)
        assert len(recs) == 1, [r.msg for r in recs]
        rec = recs[0]
        assert rec.levelno == logging.INFO
        # msg pins text/case/None edits (mutmut_4, 6, 8, 9, 10)
        assert rec.msg == NO_MATCH_MSG
        # args pins the mission_name argument (mutmut_5 -> None, 7 -> dropped)
        assert rec.args == ("Totally Unknown XYZ",)
        # and the rendered message is exactly right
        assert (
            rec.getMessage()
            == "Mission received 'Totally Unknown XYZ' \u2014 no matching quest in DB, ignoring"
        )

        # And nothing was started (behavioural backstop).
        assert all(q["started_at"] is None for q in quest_service.get_quests())


# ── Already-started branch: log.debug(ALREADY_MSG, quest["name"]) ───────────
# Kills mutmut_14 (msg->None), 15 (name->None), 16 (format dropped),
# 17 (arg dropped), 18 (XX-wrap), 19 (lower), 20 (upper).
class TestAlreadyStartedLog:
    def test_already_started_logs_exact_format_and_arg(
        self, quest_service: QuestService, caplog: pytest.LogCaptureFixture
    ) -> None:
        q = quest_service.create_quest({"name": "Paneleon Hunter"})
        quest_service.start_quest(q["id"])
        before = quest_service.get_quest(q["id"])
        assert before is not None and before["started_at"] is not None

        with caplog.at_level(logging.DEBUG, logger=LOGGER_NAME):
            quest_service.start_quest_from_mission("Paneleon Hunter")

        recs = _records(caplog)
        assert len(recs) == 1, [r.msg for r in recs]
        rec = recs[0]
        assert rec.levelno == logging.DEBUG
        assert rec.msg == ALREADY_MSG
        assert rec.args == ("Paneleon Hunter",)
        assert (
            rec.getMessage()
            == "Quest 'Paneleon Hunter' already started, skipping auto-start"
        )

        # started_at must be unchanged (the early return must hold).
        after = quest_service.get_quest(q["id"])
        assert after is not None
        assert after["started_at"] == before["started_at"]


# ── Started branch: log.info(STARTED_MSG, name, id, mission) ────────────────
# Kills mutmut_26 (msg->None), 27 (name->None), 28 (id->None), 29 (mission->None),
# 30 (format dropped), 31 (name dropped), 32 (id dropped), 33 (mission dropped),
# 34 (XX-wrap), 35 (lower), 36 (upper).
class TestStartedLog:
    def test_started_logs_exact_format_and_args(
        self, quest_service: QuestService, caplog: pytest.LogCaptureFixture
    ) -> None:
        q = quest_service.create_quest({"name": "Paneleon Hunter"})
        qid = q["id"]
        assert q["started_at"] is None

        with caplog.at_level(logging.DEBUG, logger=LOGGER_NAME):
            quest_service.start_quest_from_mission("Paneleon Hunter (repeatable)")

        recs = _records(caplog)
        # Exactly one record on the success path (the "Started ..." info line).
        assert len(recs) == 1, [r.msg for r in recs]
        rec = recs[0]
        assert rec.levelno == logging.INFO
        assert rec.msg == STARTED_MSG
        assert rec.args == ("Paneleon Hunter", qid, "Paneleon Hunter (repeatable)")
        assert (
            rec.getMessage()
            == f"Started quest 'Paneleon Hunter' (id={qid}) from chat.log mission "
            "'Paneleon Hunter (repeatable)'"
        )

        # Behavioural backstop: the quest is now started.
        updated = quest_service.get_quest(qid)
        assert updated is not None
        assert updated["started_at"] is not None


# ── _record_notable_event("quest_started", quest["name"], 0) ────────────────
# Kills mutmut_41 (type->None), 42 (name->None), 43 (value->None),
# 47 (type "XXquest_startedXX"), 48 (type "QUEST_STARTED"), 51 (value 0->1).
class TestNotableEvent:
    def test_started_records_quest_started_notable_event(
        self, tmp_path: Path
    ) -> None:
        svc, bus = _make_service_with_events(tmp_path)
        # Activate a tracking session so _record_notable_event writes a row.
        bus.publish(EVENT_SESSION_STARTED, {"session_id": "sess-c5"})

        q = svc.create_quest({"name": "Paneleon Hunter"})
        svc.start_quest_from_mission("Paneleon Hunter (repeatable)")

        row = svc._conn.execute(
            "SELECT session_id, event_type, mob_or_item, value_ped "
            "FROM notable_events"
        ).fetchall()
        assert len(row) == 1, row
        session_id, event_type, mob_or_item, value_ped = row[0]

        assert session_id == "sess-c5"
        # event_type literal: kills mutmut_41 (None), 47 (XX-wrap), 48 (upper).
        assert event_type == "quest_started"
        # description == quest name: kills mutmut_42 (None).
        assert mob_or_item == q["name"] == "Paneleon Hunter"
        # value_ped literal 0: kills mutmut_43 (None) and 51 (1).
        assert value_ped == 0
        assert value_ped != 1
