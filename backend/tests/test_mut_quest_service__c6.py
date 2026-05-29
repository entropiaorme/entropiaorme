"""Mutation-hardening tests for QuestService.quest_reward_filter and
QuestService._record_notable_event (campaign cluster quest_service__c6).

These tests exercise the auto-completion / reward-suppression filter and the
notable-event recorder. They assert the exact return value of the filter, the
row written into ``notable_events`` (event_type / mob_or_item / value_ped), and
the exact rendered text of the log lines, so mutations to literals, comparison
operators, dict defaults, branch guards and the notable-event INSERT are caught.
"""

import logging
from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService

# DDL mirroring backend/tracking/schema.py for the two tables the reward filter
# touches when a session is active. Created here because AppDatabase does not
# own the tracking schema and the unit fixture must observe notable_events rows.
_TRACKING_DDL = """
CREATE TABLE IF NOT EXISTS tracking_sessions (
    id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
    is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER,
    armour_cost REAL DEFAULT 0, heal_cost REAL DEFAULT 0);
CREATE TABLE IF NOT EXISTS notable_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL,
    kill_id     TEXT,
    event_type  TEXT NOT NULL,
    mob_or_item TEXT NOT NULL,
    value_ped   REAL NOT NULL,
    timestamp   REAL NOT NULL);
"""

_SESSION_ID = "sess-c6"


@pytest.fixture
def svc(tmp_path: Path):
    """Service with a live tracking session so notable_events rows are written."""
    db = AppDatabase(tmp_path / "test.db")
    db.conn.executescript(_TRACKING_DDL)
    db.conn.commit()
    bus = EventBus()
    service = QuestService(db, bus)
    bus.publish("session_started", {"session_id": _SESSION_ID})
    return service


def _notable_rows(service: QuestService) -> list:
    return service._conn.execute(
        "SELECT event_type, mob_or_item, value_ped FROM notable_events ORDER BY id"
    ).fetchall()


def _last_notable(service: QuestService):
    rows = _notable_rows(service)
    assert rows, "expected a notable_events row to be recorded"
    return rows[-1]


# ───────────────────────── quest_reward_filter: PED match branch ─────────────


def test_ped_match_returns_index_and_records_event(svc: QuestService):
    """A loot item whose value equals reward_ped is suppressed; the notable
    event carries the quest name, the suppressed-item description and the PED."""
    svc.create_quest({"name": "Paneleon Hunter", "reward_ped": 1.5})
    loot = [
        {"item_name": "Shrapnel", "quantity": 825, "value": 0.0825},
        {"item_name": "Universal Ammo", "quantity": 15000, "value": 1.5},
    ]
    result = svc.quest_reward_filter("Paneleon Hunter", loot, [])

    # Exact return value (kills suppress_* key/value mutants in this branch).
    assert result == {"suppress_loot_index": 1, "suppress_skill_index": None}

    event_type, mob_or_item, value_ped = _last_notable(svc)
    assert event_type == "quest_completed"
    assert mob_or_item == "Paneleon Hunter: Universal Ammo (1.50 PED) suppressed"
    assert value_ped == 1.5


def test_ped_match_tie_prefers_first_seen(svc: QuestService):
    """When two items are equidistant from reward_ped, strict ``<`` keeps the
    first (lower index); ``<=`` would drift to the later one."""
    svc.create_quest({"name": "Tie Quest", "reward_ped": 1.0})
    loot = [
        {"item_name": "First", "quantity": 1, "value": 1.0},
        {"item_name": "Second", "quantity": 1, "value": 1.0},
    ]
    result = svc.quest_reward_filter("Tie Quest", loot, [])
    assert result == {"suppress_loot_index": 0, "suppress_skill_index": None}
    assert _last_notable(svc)[1] == "Tie Quest: First (1.00 PED) suppressed"


def test_ped_match_tolerance_inclusive_boundary(svc: QuestService):
    """A diff of exactly 0.02 must match (``<= 0.02``); ``< 0.02`` would reject
    it and suppress nothing. reward 0.04 vs value 0.02 gives diff == 0.02
    exactly in IEEE-754, so this distinguishes ``<=`` from ``<``."""
    svc.create_quest({"name": "Edge Quest", "reward_ped": 0.04})
    loot = [{"item_name": "Near", "quantity": 1, "value": 0.02}]
    result = svc.quest_reward_filter("Edge Quest", loot, [])
    assert result == {"suppress_loot_index": 0, "suppress_skill_index": None}


def test_ped_match_tolerance_upper_bound_rejects_far_item(svc: QuestService):
    """An item 1.0 PED away is outside the 0.02 tolerance: no suppression and
    no result. A widened tolerance (``<= 1.02``) would wrongly suppress it."""
    svc.create_quest({"name": "Far Quest", "reward_ped": 1.0})
    loot = [{"item_name": "Far", "quantity": 1, "value": 2.0}]
    result = svc.quest_reward_filter("Far Quest", loot, [])
    assert result is None
    # No suppression event recorded but the completion event still fires.
    event_type, mob_or_item, value_ped = _last_notable(svc)
    assert mob_or_item == "Far Quest"  # bare quest name, no ": ... suppressed"


def test_ped_match_uses_value_key_not_default(svc: QuestService):
    """The diff is computed from the item's ``value``; a missing-value default
    of 0.0 only applies when the key is absent. With the key present and a
    distractor lacking value, the real value must drive the match."""
    svc.create_quest({"name": "Value Quest", "reward_ped": 5.0})
    loot = [
        {"item_name": "NoValueKey", "quantity": 1},  # missing 'value' -> 0.0
        {"item_name": "Match", "quantity": 1, "value": 5.0},
    ]
    result = svc.quest_reward_filter("Value Quest", loot, [])
    assert result == {"suppress_loot_index": 1, "suppress_skill_index": None}


def test_ped_match_missing_value_default_is_zero(svc: QuestService):
    """When the matching item lacks a 'value' key, the default 0.0 is used so
    diff = |0.0 - reward|. For a tiny reward (0.01) that is within tolerance
    and the item is suppressed; a default of 1.0/None would change the result."""
    svc.create_quest({"name": "Tiny Quest", "reward_ped": 0.01})
    loot = [{"item_name": "NoValue", "quantity": 1}]  # no 'value' key
    result = svc.quest_reward_filter("Tiny Quest", loot, [])
    assert result == {"suppress_loot_index": 0, "suppress_skill_index": None}


def test_ped_item_name_default_used_when_name_missing(svc: QuestService):
    """When the suppressed item has no 'item_name', the description falls back
    to '?'. Pins the ``get('item_name', '?')`` key and default."""
    svc.create_quest({"name": "NoName Quest", "reward_ped": 1.0})
    loot = [{"quantity": 1, "value": 1.0}]  # no item_name
    svc.quest_reward_filter("NoName Quest", loot, [])
    assert _last_notable(svc)[1] == "NoName Quest: ? (1.00 PED) suppressed"


def test_ped_no_match_returns_none_and_warns(svc: QuestService, caplog):
    """reward_ped > 0 but no item within tolerance: returns None, logs a
    warning with the quest name, reward and the (name,value) item list."""
    svc.create_quest({"name": "Pricey Quest", "reward_ped": 100.0})
    loot = [{"item_name": "Shrapnel", "quantity": 1, "value": 0.08}]
    with caplog.at_level(logging.WARNING, logger="backend.services.quest_service"):
        result = svc.quest_reward_filter("Pricey Quest", loot, [])
    assert result is None
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.WARNING]
    assert msgs == [
        "Quest 'Pricey Quest' reward 100.00 PED \u2014 no matching loot item in "
        "tick (items: [('Shrapnel', 0.08)])"
    ]


# ───────────────────────── quest_reward_filter: zero-PED min branch ──────────


def test_zero_ped_suppresses_lowest_value_not_index_zero(svc: QuestService):
    """reward_ped == 0 suppresses the lowest-value item, found by a value key.
    The lowest item is at index 1, so a broken key (key=None / no key) that
    falls back to the smallest index (0) is caught."""
    svc.create_quest({"name": "Zero Quest", "reward_ped": 0})
    loot = [
        {"item_name": "Expensive", "quantity": 1, "value": 5.0},
        {"item_name": "Cheap", "quantity": 1, "value": 0.1},
    ]
    result = svc.quest_reward_filter("Zero Quest", loot, [])
    assert result == {"suppress_loot_index": 1, "suppress_skill_index": None}

    event_type, mob_or_item, value_ped = _last_notable(svc)
    assert event_type == "quest_completed"
    assert mob_or_item == "Zero Quest: Cheap suppressed"
    assert value_ped == 0


def test_zero_ped_min_key_reads_value_field(svc: QuestService):
    """The min key reads 'value'; reading the wrong (uppercased / None) key
    would make every item default to 0.0 and pick index 0 instead of the
    genuinely cheapest item at index 2."""
    svc.create_quest({"name": "MinKey Quest", "reward_ped": 0})
    loot = [
        {"item_name": "A", "quantity": 1, "value": 3.0},
        {"item_name": "B", "quantity": 1, "value": 2.0},
        {"item_name": "C", "quantity": 1, "value": 1.0},
    ]
    result = svc.quest_reward_filter("MinKey Quest", loot, [])
    assert result == {"suppress_loot_index": 2, "suppress_skill_index": None}
    assert _last_notable(svc)[1] == "MinKey Quest: C suppressed"


def test_zero_ped_min_key_default_is_zero(svc: QuestService):
    """In the min branch the key reads ``get('value', 0.0)``. An item missing
    its 'value' key must sort as 0.0 (the lowest), so it is suppressed even
    though another item carries an explicit 0.5. A default of 1.0 would pick
    the wrong item; a default of None (or no default) would make ``min`` raise
    TypeError comparing None with a float."""
    svc.create_quest({"name": "DefaultZero Quest", "reward_ped": 0})
    loot = [
        {"item_name": "HasValue", "quantity": 1, "value": 0.5},
        {"item_name": "NoValue", "quantity": 1},  # missing 'value' -> 0.0
    ]
    result = svc.quest_reward_filter("DefaultZero Quest", loot, [])
    assert result == {"suppress_loot_index": 1, "suppress_skill_index": None}
    assert _last_notable(svc)[1] == "DefaultZero Quest: NoValue suppressed"


def test_zero_ped_item_name_default(svc: QuestService):
    """Min branch description falls back to '?' when the item has no name."""
    svc.create_quest({"name": "ZeroNoName", "reward_ped": 0})
    loot = [{"quantity": 1, "value": 0.5}]
    svc.quest_reward_filter("ZeroNoName", loot, [])
    assert _last_notable(svc)[1] == "ZeroNoName: ? suppressed"


def test_reward_threshold_strictly_positive(svc: QuestService):
    """reward_ped of exactly 0 takes the min/cheapest branch (description
    '<item> suppressed'), NOT the PED-match branch ('... (X PED) suppressed').
    ``>= 0`` would route 0 into the match branch and change the description."""
    svc.create_quest({"name": "Threshold Quest", "reward_ped": 0})
    loot = [
        {"item_name": "Only", "quantity": 1, "value": 0.5},
    ]
    svc.quest_reward_filter("Threshold Quest", loot, [])
    assert _last_notable(svc)[1] == "Threshold Quest: Only suppressed"


def test_small_positive_reward_takes_match_branch(svc: QuestService):
    """A reward strictly between 0 and 1 (0.5) must take the PED-match branch
    (description includes the PED). ``> 1`` would push it into the min branch
    and drop the PED from the description."""
    svc.create_quest({"name": "Small Quest", "reward_ped": 0.5})
    loot = [{"item_name": "Coin", "quantity": 1, "value": 0.5}]
    svc.quest_reward_filter("Small Quest", loot, [])
    assert _last_notable(svc)[1] == "Small Quest: Coin (0.50 PED) suppressed"


def test_reward_ped_set_but_no_loot_records_no_suppression(svc: QuestService):
    """reward_ped is not None but loot is empty: the AND guard skips both
    branches. With reward_ped == 0 an OR guard would enter and reach
    ``min(range(0))`` which raises ValueError, so a clean None return pins the
    ``and``."""
    svc.create_quest({"name": "EmptyLoot Quest", "reward_ped": 0})
    result = svc.quest_reward_filter("EmptyLoot Quest", [], [])
    assert result is None
    # Completion event still recorded with bare quest name.
    assert _last_notable(svc)[1] == "EmptyLoot Quest"


def test_reward_ped_set_positive_no_loot_no_warning(svc: QuestService, caplog):
    """reward_ped > 0 with empty loot: the AND guard skips the branch entirely,
    so NO 'no matching loot item' warning is logged. An OR guard would enter,
    loop over empty loot and emit that warning."""
    svc.create_quest({"name": "PosEmpty Quest", "reward_ped": 5.0})
    with caplog.at_level(logging.WARNING, logger="backend.services.quest_service"):
        result = svc.quest_reward_filter("PosEmpty Quest", [], [])
    assert result is None
    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert warnings == []


# ───────────────────────── quest_reward_filter: skill branch ─────────────────


def test_skill_reward_suppresses_skill_and_records_pes_event(svc: QuestService):
    """A skill quest suppresses the first skill gain, records a
    'quest_completed_pes' event with the 'skill reward suppressed' suffix."""
    svc.create_quest(
        {"name": "Skill Quest", "reward_ped": 5.0, "reward_is_skill": True}
    )
    result = svc.quest_reward_filter(
        "Skill Quest", [], [{"skill_name": "Laser", "amount": 0.5}]
    )
    assert result == {"suppress_loot_index": None, "suppress_skill_index": 0}

    event_type, mob_or_item, value_ped = _last_notable(svc)
    assert event_type == "quest_completed_pes"
    assert mob_or_item == "Skill Quest: skill reward suppressed"
    assert value_ped == 5.0


def test_skill_event_type_distinct_from_loot_event_type(svc: QuestService):
    """The event_type ternary: skill -> 'quest_completed_pes',
    non-skill -> 'quest_completed'. Verify the non-skill arm too."""
    svc.create_quest({"name": "Loot Quest", "reward_ped": 1.0})
    svc.quest_reward_filter(
        "Loot Quest", [{"item_name": "X", "quantity": 1, "value": 1.0}], []
    )
    assert _last_notable(svc)[0] == "quest_completed"


def test_skill_reward_none_uses_zero_not_one(svc: QuestService):
    """A skill quest with no reward_ped records value_ped via ``reward_ped or
    0`` == 0. ``or 1`` would record 1.0; ``and 0`` is moot here (None)."""
    svc.create_quest({"name": "FreeSkill", "reward_is_skill": True})
    svc.quest_reward_filter("FreeSkill", [], [{"skill_name": "Evade", "amount": 0.1}])
    event_type, mob_or_item, value_ped = _last_notable(svc)
    assert event_type == "quest_completed_pes"
    assert value_ped == 0


def test_value_ped_uses_reward_not_short_circuit_zero(svc: QuestService):
    """For a positive reward, ``reward_ped or 0`` yields the reward (2.0);
    ``reward_ped and 0`` would record 0.0."""
    svc.create_quest({"name": "Value Or Quest", "reward_ped": 2.0})
    svc.quest_reward_filter(
        "Value Or Quest", [{"item_name": "X", "quantity": 1, "value": 2.0}], []
    )
    assert _last_notable(svc)[2] == 2.0


# ───────────────────────── description prefix / suffix ───────────────────────


def test_desc_keeps_quest_name_prefix_with_suffix(svc: QuestService):
    """The description is ``name`` then ``+= ': <suffix>'``. A mutation that
    REPLACES (``desc = f': {suffix}'``) instead of appending would drop the
    quest name prefix."""
    svc.create_quest({"name": "Prefix Quest", "reward_ped": 1.0})
    svc.quest_reward_filter(
        "Prefix Quest", [{"item_name": "Item", "quantity": 1, "value": 1.0}], []
    )
    mob_or_item = _last_notable(svc)[1]
    assert mob_or_item.startswith("Prefix Quest: ")
    assert mob_or_item == "Prefix Quest: Item (1.00 PED) suppressed"


# ───────────────────────── log lines (caplog) ───────────────────────────────


def test_no_match_logs_info(svc: QuestService, caplog):
    """No quest matches: returns None, logs the exact info line with the
    mission name interpolated."""
    with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
        result = svc.quest_reward_filter("Totally Unknown Mission", [], [])
    assert result is None
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.INFO]
    assert (
        "Mission 'Totally Unknown Mission' \u2014 no matching quest in DB, "
        "no suppression" in msgs
    )


def test_auto_complete_logs_info(svc: QuestService, caplog):
    """On a match the auto-complete info line renders the quest name, id and
    mission name in that order."""
    q = svc.create_quest({"name": "LogMatch", "reward_ped": 1.0})
    with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
        svc.quest_reward_filter(
            "LogMatch", [{"item_name": "X", "quantity": 1, "value": 1.0}], []
        )
    expected = (
        f"Auto-completed quest 'LogMatch' (id={q['id']}) from chat.log "
        "mission 'LogMatch'"
    )
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.INFO]
    assert expected in msgs


# ───────────────────────── _record_notable_event ────────────────────────────


def test_record_notable_event_inserts_row(svc: QuestService):
    """With an active session, _record_notable_event inserts exactly one row
    with the passed event_type / description / value. Kills the SQL->None,
    params->None, missing-arg and inverted-guard mutants (all of which result
    in no row being written)."""
    svc._record_notable_event("custom_event", "a description", 3.25)
    rows = _notable_rows(svc)
    assert len(rows) == 1
    event_type, mob_or_item, value_ped = rows[0]
    assert event_type == "custom_event"
    assert mob_or_item == "a description"
    assert value_ped == 3.25


def test_record_notable_event_full_columns(svc: QuestService):
    """Verify the full inserted row, including session_id and that kill_id is
    NULL, pinning the column ordering / VALUES tuple of the INSERT."""
    svc._record_notable_event("evt", "desc", 1.0)
    row = svc._conn.execute(
        "SELECT session_id, kill_id, event_type, mob_or_item, value_ped "
        "FROM notable_events"
    ).fetchone()
    assert row["session_id"] == _SESSION_ID
    assert row["kill_id"] is None
    assert row["event_type"] == "evt"
    assert row["mob_or_item"] == "desc"
    assert row["value_ped"] == 1.0


def test_record_notable_event_guard_inactive_session(tmp_path: Path):
    """Without an active session the recorder is a no-op (the ``if not
    session: return`` guard). An inverted guard would attempt an insert."""
    db = AppDatabase(tmp_path / "test.db")
    db.conn.executescript(_TRACKING_DDL)
    db.conn.commit()
    service = QuestService(db)  # no event bus -> no session id
    assert service._current_session_id is None
    service._record_notable_event("evt", "desc", 1.0)
    assert _notable_rows(service) == []


def test_record_notable_event_swallows_db_error(svc: QuestService, caplog):
    """If the INSERT fails (table dropped), the exception is swallowed and a
    debug line is logged. Kills the except-branch log mutants."""
    svc._conn.execute("DROP TABLE notable_events")
    svc._conn.commit()
    with caplog.at_level(logging.DEBUG, logger="backend.services.quest_service"):
        svc._record_notable_event("evt", "desc", 1.0)  # must not raise
    msgs = [r.getMessage() for r in caplog.records if r.levelno == logging.DEBUG]
    assert "Could not record notable event" in msgs


def test_notable_event_value_not_nulled(svc: QuestService):
    """The value_ped argument reaches the row unchanged. A value->None mutation
    would violate NOT NULL and write no row."""
    svc.create_quest({"name": "ValQuest", "reward_ped": 7.0})
    svc.quest_reward_filter(
        "ValQuest", [{"item_name": "Y", "quantity": 1, "value": 7.0}], []
    )
    assert _last_notable(svc)[2] == 7.0
