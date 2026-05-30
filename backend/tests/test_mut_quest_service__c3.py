"""Mutation-hardening tests for QuestService playlist analytics.

Targets the surviving/timeout mutants in cluster ``quest_service__c3``:
``QuestService.get_playlist_analytics`` and
``QuestService._find_matching_playlists``.

The mutants fall into three families:

* The literal result dict ``get_playlist_analytics`` returns when a playlist has
  no *immediate* quests (the early ``if not immediate_ids`` branch). The campaign
  mutates every key name (``"k" -> "XXkXX"`` / ``"K"``) and every zero value
  (``0 -> 1``) in that dict. A renamed key drops out of the result (so the exact
  key set is wrong); a flipped value makes a metric non-zero. The first test
  pins the whole dict by exact equality, which fails on either mutation.

* The fallback economics dict ``get_playlist_analytics`` spreads into its result
  when the playlist has immediate quests but no curated linked sessions (the
  ``else`` arm of the ``stats`` expression, ``**stats``), plus the ``playlist_id``
  literal of the final result dict. The second test drives that branch and again
  pins the whole result by exact equality.

* ``_find_matching_playlists`` reads only the *active* playlists. Two mutants
  flip ``active_only=True`` to ``None`` / ``False``, which would fold in
  soft-disabled playlists. The third test plants an inactive playlist that would
  match and asserts it is excluded.

Each test builds a fresh on-disk DB and drives the real production query paths,
mirroring the sibling property suite.
"""

import tempfile
from pathlib import Path

from backend.db.app_database import AppDatabase
from backend.services.quest_service import (
    PLAYLIST_GROUP_IMMEDIATE,
    PLAYLIST_GROUP_LONG_HORIZON,
    QuestService,
)

# tracking_sessions / kills / skill_gains are owned by the tracker at runtime;
# the analytics queries read from them, so create the tracker-side tables the
# sibling suite uses.
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


def _make_service() -> QuestService:
    """A QuestService over a fresh on-disk DB with the tracker tables present."""
    tmp = Path(tempfile.mkdtemp()) / "quests.db"
    db = AppDatabase(tmp)
    db.conn.executescript(_TRACKER_SCHEMA)
    db.conn.commit()
    return QuestService(db)


# --- get_playlist_analytics: empty (no-immediate-quests) result dict ----------


def test_analytics_no_immediate_quests_returns_exact_zero_dict():
    """A playlist with only long-horizon quests takes the ``not immediate_ids``
    branch and returns a fixed, fully zeroed metric dict whose key names and zero
    values are all pinned. Renaming any key or flipping any zero to one breaks the
    exact-equality assertion. (mutants get_playlist_analytics 15-76)
    """
    svc = _make_service()
    q1 = svc.create_quest({"name": "Q1"})["id"]
    q2 = svc.create_quest({"name": "Q2"})["id"]
    # No immediate group at all; two long-horizon quests so the count is non-zero
    # and distinguishable from a renamed/empty key.
    pl = svc.create_playlist(
        {
            "name": "LongOnly",
            "items": [
                {"quest_id": q1, "group_type": PLAYLIST_GROUP_LONG_HORIZON},
                {"quest_id": q2, "group_type": PLAYLIST_GROUP_LONG_HORIZON},
            ],
        }
    )

    result = svc.get_playlist_analytics(pl["id"])

    assert result == {
        "playlist_id": pl["id"],
        "playlist_name": "LongOnly",
        "quest_count": 0,
        "long_horizon_quest_count": 2,
        "matched_sessions": 0,
        "total_reward_ped": 0,
        "total_immediate_reward_ped": 0,
        "total_bonus_reward_ped": 0,
        "total_skill_reward_ped": 0,
        "total_immediate_skill_reward_ped": 0,
        "total_bonus_skill_reward_ped": 0,
        "total_expected_reward_ped": 0,
        "total_expected_immediate_reward_ped": 0,
        "total_expected_bonus_reward_ped": 0,
        "total_duration": 0,
        "weapon_cost": 0,
        "heal_cost": 0,
        "enhancer_cost": 0,
        "armour_cost": 0,
        "loot_tt": 0,
        "skill_tt": 0,
    }
    # Belt-and-braces: each pinned zero key is present under its exact name with
    # value 0 (so a value-flip 0->1 and a key-rename are both individually caught
    # even were dict equality ever loosened).
    for key in (
        "total_reward_ped",
        "total_immediate_reward_ped",
        "total_bonus_reward_ped",
        "total_skill_reward_ped",
        "total_immediate_skill_reward_ped",
        "total_bonus_skill_reward_ped",
        "total_expected_reward_ped",
        "total_expected_immediate_reward_ped",
        "total_expected_bonus_reward_ped",
        "total_duration",
        "weapon_cost",
        "heal_cost",
        "enhancer_cost",
        "armour_cost",
        "loot_tt",
        "skill_tt",
    ):
        assert result[key] == 0


# --- get_playlist_analytics: immediate-quests, no curated sessions ------------


def test_analytics_immediate_no_sessions_returns_exact_zero_dict():
    """A playlist *with* immediate quests but no curated linked sessions takes the
    main path: ``stats`` falls to its zeroed ``else`` dict (spread via ``**stats``)
    and the final result dict carries ``playlist_id``. Renaming any economics key
    in that else dict, flipping any of its zeros, or renaming the final
    ``playlist_id`` breaks the exact-equality assertion.
    (mutants get_playlist_analytics 87-104, 112-113)
    """
    svc = _make_service()
    q1 = svc.create_quest({"name": "Q1"})["id"]
    q2 = svc.create_quest({"name": "Q2"})["id"]
    pl = svc.create_playlist(
        {
            "name": "WithImm",
            "items": [
                {"quest_id": q1, "group_type": PLAYLIST_GROUP_IMMEDIATE},
                {"quest_id": q2, "group_type": PLAYLIST_GROUP_LONG_HORIZON},
            ],
        }
    )

    result = svc.get_playlist_analytics(pl["id"])

    assert result == {
        "playlist_id": pl["id"],
        "playlist_name": "WithImm",
        "quest_count": 1,
        "long_horizon_quest_count": 1,
        "total_reward_ped": 0,
        "total_immediate_reward_ped": 0,
        "total_bonus_reward_ped": 0,
        "total_skill_reward_ped": 0,
        "total_immediate_skill_reward_ped": 0,
        "total_bonus_skill_reward_ped": 0,
        "total_expected_reward_ped": 0,
        "total_expected_immediate_reward_ped": 0,
        "total_expected_bonus_reward_ped": 0,
        "matched_sessions": 0,
        "linked_sessions": 0,
        "total_duration": 0,
        "weapon_cost": 0,
        "heal_cost": 0,
        "enhancer_cost": 0,
        "armour_cost": 0,
        "loot_tt": 0,
        "skill_tt": 0,
    }
    # The fallback economics dict is spread via **stats; pin its keys/zeros by
    # exact name and value, and pin the literal playlist_id key of the result.
    assert result["playlist_id"] == pl["id"]
    for key in (
        "weapon_cost",
        "heal_cost",
        "enhancer_cost",
        "armour_cost",
        "skill_tt",
        "linked_sessions",
    ):
        assert result[key] == 0


# --- _find_matching_playlists: only active playlists --------------------------


def test_find_matching_excludes_inactive_playlists():
    """``_find_matching_playlists`` consults only the *active* playlists. An
    inactive playlist whose shape would otherwise match must be excluded. The
    mutants that flip ``active_only=True`` to ``None`` / ``False`` fold in the
    inactive playlist, growing the match list. (mutants _find_matching_playlists 4, 5)
    """
    svc = _make_service()
    q1 = svc.create_quest({"name": "Q1"})["id"]
    q2 = svc.create_quest({"name": "Q2"})["id"]

    # Active playlist: immediate={q1}, scope={q1,q2}. Matches when {q1} completed.
    pl_active = svc.create_playlist(
        {
            "name": "Active",
            "items": [
                {"quest_id": q1, "group_type": PLAYLIST_GROUP_IMMEDIATE},
                {"quest_id": q2, "group_type": PLAYLIST_GROUP_LONG_HORIZON},
            ],
        }
    )
    # Inactive playlist with the same matching shape; mark it inactive WITHOUT
    # deleting its items (delete_playlist would drop the items, hiding the
    # mutation). Direct SQL on the live connection mirrors the sibling suite's
    # raw-insert helpers and exercises the real get_playlists(active_only) filter.
    pl_inactive = svc.create_playlist(
        {
            "name": "Inactive",
            "items": [
                {"quest_id": q1, "group_type": PLAYLIST_GROUP_IMMEDIATE},
                {"quest_id": q2, "group_type": PLAYLIST_GROUP_LONG_HORIZON},
            ],
        }
    )
    svc._conn.execute(
        "UPDATE quest_playlists SET is_active = 0 WHERE id = ?",
        (pl_inactive["id"],),
    )
    svc._conn.commit()

    matched = svc._find_matching_playlists([q1])

    # Only the active playlist matches; the inactive one is filtered out by the
    # active_only=True read. A mutant reading inactive playlists too would also
    # return pl_inactive["id"].
    assert matched == [pl_active["id"]]
    assert pl_inactive["id"] not in matched
