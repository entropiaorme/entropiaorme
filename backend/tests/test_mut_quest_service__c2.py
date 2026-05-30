"""Mutation-hardening tests for QuestService playlist + analytics surfaces.

Cluster ``quest_service__c2``: ``get_playlists``, ``get_playlist``,
``create_playlist``, ``update_playlist``, ``delete_playlist``,
``get_quest_analytics``, ``get_all_playlist_analytics``.

Each test drives a real ``QuestService`` over an on-disk SQLite database (the
production query paths) and asserts the exact behaviour a mutant breaks:
default-argument values, the ``WHERE is_active`` filter, dict key spellings and
value sources, the soft-delete rowcount guard, the analytics ``continue`` skip,
and the reward projections.
"""

from pathlib import Path

import pytest

from backend.db.app_database import AppDatabase
from backend.services.quest_service import (
    PLAYLIST_GROUP_IMMEDIATE,
    PLAYLIST_GROUP_LONG_HORIZON,
    QuestService,
)

# The analytics queries read tracker-owned tables that AppDatabase does not
# create; mirror the sibling property suite and add them up front.
_TRACKER_SCHEMA = """
    CREATE TABLE IF NOT EXISTS tracking_sessions (
        id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
        is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER,
        armour_cost REAL DEFAULT 0, heal_cost REAL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS kills (
        id TEXT PRIMARY KEY, session_id TEXT,
        timestamp REAL, mob_name TEXT,
        loot_total_ped REAL DEFAULT 0,
        enhancer_cost REAL DEFAULT 0,
        cost_ped REAL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS kill_tool_stats (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kill_id TEXT, tool_name TEXT,
        cost_per_shot REAL DEFAULT 0, shots_fired INTEGER DEFAULT 0);
    CREATE TABLE IF NOT EXISTS skill_gains (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT, timestamp REAL,
        skill_name TEXT, amount REAL, ped_value REAL);
"""


@pytest.fixture
def svc(tmp_path: Path) -> QuestService:
    db = AppDatabase(tmp_path / "quests.db")
    db.conn.executescript(_TRACKER_SCHEMA)
    db.conn.commit()
    return QuestService(db)


def _finished_session(svc: QuestService, session_id: str) -> None:
    svc._conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
        "VALUES (?, ?, ?, 0)",
        (session_id, 1000.0, 2000.0),
    )
    svc._conn.commit()


def _link_quest_session(svc: QuestService, session_id: str, quest_id: int) -> None:
    """Curate a finished session as a per-quest analytics link."""
    _finished_session(svc, session_id)
    svc._conn.execute(
        "INSERT INTO session_quest_analytics_links "
        "(session_id, link_type, quest_id, playlist_id, linked_at) "
        "VALUES (?, 'quest', ?, NULL, 1500.0)",
        (session_id, quest_id),
    )
    svc._conn.commit()


# ── get_playlists ─────────────────────────────────────────────────────────


def test_get_playlists_active_only_default_excludes_deleted(svc: QuestService):
    """Default active_only=True hides soft-deleted playlists (mutmut_1)."""
    svc.create_playlist({"name": "Keep"})
    drop = svc.create_playlist({"name": "Drop"})
    assert svc.delete_playlist(drop["id"]) is True

    names_default = {p["name"] for p in svc.get_playlists()}
    assert names_default == {"Keep"}

    names_all = {p["name"] for p in svc.get_playlists(active_only=False)}
    assert names_all == {"Keep", "Drop"}


def test_get_playlists_inactive_arg_returns_rows(svc: QuestService):
    """active_only=False must produce valid SQL and return rows (mutmut_6).

    The ``else ""`` branch is spliced straight into the query; a corrupted
    sentinel (``"XXXX"``) would make ``SELECT ... quest_playlists XXXX ORDER
    BY`` a syntax error.
    """
    svc.create_playlist({"name": "A"})
    rows = svc.get_playlists(active_only=False)
    assert [p["name"] for p in rows] == ["A"]


def test_get_playlists_item_keys_and_values(svc: QuestService):
    """quest_ids / items keys and values are populated (mutmut_18-31)."""
    q1 = svc.create_quest({"name": "Q1"})
    q2 = svc.create_quest({"name": "Q2"})
    svc.create_playlist(
        {
            "name": "PL",
            "items": [
                {"quest_id": q1["id"], "group_type": PLAYLIST_GROUP_IMMEDIATE},
                {"quest_id": q2["id"], "group_type": PLAYLIST_GROUP_LONG_HORIZON},
            ],
        }
    )
    pl = svc.get_playlists()[0]

    assert "quest_ids" in pl
    assert pl["quest_ids"] == [q1["id"], q2["id"]]
    assert "items" in pl
    assert [i["quest_id"] for i in pl["items"]] == [q1["id"], q2["id"]]
    assert pl["immediate_quest_ids"] == [q1["id"]]
    assert pl["long_horizon_quest_ids"] == [q2["id"]]


# ── get_playlist ──────────────────────────────────────────────────────────


def test_get_playlist_returns_expected_row(svc: QuestService):
    """get_playlist resolves by id and carries the classified groups."""
    q = svc.create_quest({"name": "Q"})
    created = svc.create_playlist(
        {"name": "Solo", "quest_ids": [q["id"]], "planet": "Arkadia"}
    )
    fetched = svc.get_playlist(created["id"])
    assert fetched is not None
    assert fetched["name"] == "Solo"
    assert fetched["planet"] == "Arkadia"
    assert fetched["quest_ids"] == [q["id"]]
    assert fetched["immediate_quest_ids"] == [q["id"]]


# ── create_playlist ───────────────────────────────────────────────────────


def test_create_playlist_uses_supplied_planet(svc: QuestService):
    """Supplied planet is stored, not the fallback default (mutmut_11/15/16)."""
    pl = svc.create_playlist({"name": "PL", "planet": "Arkadia"})
    assert pl["planet"] == "Arkadia"
    fetched = svc.get_playlist(pl["id"])
    assert fetched is not None
    assert fetched["planet"] == "Arkadia"


def test_create_playlist_planet_default_is_calypso(svc: QuestService):
    """Omitted planet defaults to exactly 'Calypso' (mutmut_17/18/19)."""
    pl = svc.create_playlist({"name": "PL"})
    assert pl["planet"] == "Calypso"


def test_create_playlist_estimated_minutes_default(svc: QuestService):
    """Omitted estimated_minutes defaults to exactly 30 (mutmut_26)."""
    pl = svc.create_playlist({"name": "PL"})
    assert pl["estimated_minutes"] == 30


def test_create_playlist_supplied_estimated_minutes(svc: QuestService):
    pl = svc.create_playlist({"name": "PL", "estimated_minutes": 45})
    assert pl["estimated_minutes"] == 45


# ── update_playlist ───────────────────────────────────────────────────────


def test_update_playlist_name(svc: QuestService):
    """name is updatable: it must stay in the allowed set (mutmut_5/6/11)."""
    pl = svc.create_playlist({"name": "Before"})
    updated = svc.update_playlist(pl["id"], {"name": "After"})
    assert updated is not None
    assert updated["name"] == "After"
    fetched = svc.get_playlist(pl["id"])
    assert fetched is not None
    assert fetched["name"] == "After"


def test_update_playlist_planet(svc: QuestService):
    """planet is updatable: it must stay in the allowed set (mutmut_7/8)."""
    pl = svc.create_playlist({"name": "PL", "planet": "Calypso"})
    updated = svc.update_playlist(pl["id"], {"planet": "Arkadia"})
    assert updated is not None
    assert updated["planet"] == "Arkadia"


def test_update_playlist_estimated_minutes(svc: QuestService):
    """estimated_minutes is updatable: must stay in the allowed set (mutmut_9/10)."""
    pl = svc.create_playlist({"name": "PL", "estimated_minutes": 30})
    updated = svc.update_playlist(pl["id"], {"estimated_minutes": 99})
    assert updated is not None
    assert updated["estimated_minutes"] == 99


def test_update_playlist_multiple_fields_persist(svc: QuestService):
    """Updating two fields exercises the ', '.join set_clause (mutmut_13-19).

    A broken join separator, a None SQL string, or None/dropped params all
    corrupt the UPDATE so the change never lands (or raises). The assertions
    require both fields to actually persist.
    """
    pl = svc.create_playlist(
        {"name": "Old", "planet": "Calypso", "estimated_minutes": 30}
    )
    updated = svc.update_playlist(pl["id"], {"name": "New", "estimated_minutes": 77})
    assert updated is not None
    assert updated["name"] == "New"
    assert updated["estimated_minutes"] == 77
    refetched = svc.get_playlist(pl["id"])
    assert refetched is not None
    assert refetched["name"] == "New"
    assert refetched["estimated_minutes"] == 77


# ── delete_playlist ───────────────────────────────────────────────────────


def test_delete_playlist_success_and_idempotent(svc: QuestService):
    """First delete returns True; a second (no live row) returns False.

    Guards the rowcount>0 test (mutmut_9: >=0) and the False return
    (mutmut_19: ->True).
    """
    pl = svc.create_playlist({"name": "PL"})
    assert svc.delete_playlist(pl["id"]) is True
    assert svc.delete_playlist(pl["id"]) is False


def test_delete_playlist_missing_returns_false(svc: QuestService):
    """Deleting an id that never existed returns False (mutmut_9/19)."""
    assert svc.delete_playlist(99999) is False


# ── get_quest_analytics ───────────────────────────────────────────────────


def test_quest_analytics_skips_unlinked_keeps_linked(svc: QuestService):
    """A quest with no linked sessions is skipped via continue, not break,
    so a later linked quest still appears (mutmut_11: continue->break).

    Quests are ordered by name; 'A...' (unlinked) precedes 'B...' (linked).
    """
    svc.create_quest({"name": "Aardvark"})  # unlinked -> 0 linked sessions
    linked = svc.create_quest({"name": "Beacon"})
    _link_quest_session(svc, "sess-keep", linked["id"])

    rows = svc.get_quest_analytics()
    names = {r["quest_name"] for r in rows}
    assert names == {"Beacon"}


def test_quest_analytics_entry_keys_and_values(svc: QuestService):
    """Exact key spellings and value sources of an analytics entry
    (mutmut_16-33)."""
    q = svc.create_quest(
        {
            "name": "Iron",
            "planet": "Arkadia",
            "category": "iron",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 120.0,
        }
    )
    _link_quest_session(svc, "sess-1", q["id"])

    (row,) = svc.get_quest_analytics()
    assert row["quest_id"] == q["id"]
    assert row["quest_name"] == "Iron"
    assert row["planet"] == "Arkadia"
    assert row["category"] == "iron"
    assert row["reward_ped"] == 5.0
    assert row["reward_is_skill"] is False
    assert row["expected_reward_markup_percent"] == 120.0


def test_quest_analytics_reward_ped_falsy_default_is_zero(svc: QuestService):
    """A quest with no reward_ped reports 0, not 1 (mutmut_27/29)."""
    q = svc.create_quest({"name": "NoReward"})
    _link_quest_session(svc, "sess-2", q["id"])
    (row,) = svc.get_quest_analytics()
    assert row["reward_ped"] == 0
    assert row["total_expected_reward_ped"] == 0  # mutmut_49: qr[4] or 1


def test_quest_analytics_reward_is_skill_true(svc: QuestService):
    """reward_is_skill=True surfaces as True (mutmut_32/33)."""
    q = svc.create_quest(
        {"name": "SkillQuest", "reward_ped": 3.0, "reward_is_skill": True}
    )
    _link_quest_session(svc, "sess-3", q["id"])
    (row,) = svc.get_quest_analytics()
    assert row["reward_is_skill"] is True


def test_quest_analytics_skill_expected_total_ignores_markup(svc: QuestService):
    """For a skill reward the expected total is reward_ped * completions, i.e.
    markup is NOT applied because reward_is_skill is truthy (mutmut_40/50).

    The analytics dict reads expected_reward_markup_percent straight from the
    row, so we force a non-null markup on a skill quest (the normal create
    path nulls it) to make the skill flag the deciding factor.
    """
    q = svc.create_quest(
        {"name": "SkillMarkup", "reward_ped": 10.0, "reward_is_skill": True}
    )
    # Force a markup despite reward_is_skill so the skill flag is what decides
    # whether markup is applied in _expected_reward_total.
    svc._conn.execute(
        "UPDATE quests SET expected_reward_markup_percent = 200.0 WHERE id = ?",
        (q["id"],),
    )
    svc._conn.commit()
    _link_quest_session(svc, "sess-4", q["id"])

    (row,) = svc.get_quest_analytics()
    # One linked session => one completion-equivalent; skill => plain reward_ped.
    assert row["reward_is_skill"] is True
    assert row["total_expected_reward_ped"] == 10.0  # not 10*200/100 = 20.0


# ── get_all_playlist_analytics ────────────────────────────────────────────


def test_all_playlist_analytics_active_only(svc: QuestService):
    """Only active playlists are aggregated; an inactive one is excluded
    (mutmut_2: active_only=None, mutmut_3: active_only=False).

    Soft-deleting via delete_playlist also strips the items (so the playlist
    would yield no stats anyway); instead flip is_active directly so the
    inactive playlist keeps its immediate quest and *would* yield a stats dict
    if the filter were dropped.
    """
    q = svc.create_quest({"name": "Q"})
    active = svc.create_playlist({"name": "Active", "quest_ids": [q["id"]]})
    hidden = svc.create_playlist({"name": "Hidden", "quest_ids": [q["id"]]})
    svc._conn.execute(
        "UPDATE quest_playlists SET is_active = 0 WHERE id = ?",
        (hidden["id"],),
    )
    svc._conn.commit()

    results = svc.get_all_playlist_analytics()
    ids = {r["playlist_id"] for r in results}
    assert ids == {active["id"]}
