"""Mutation-hardening tests for QuestService analytics + mission matching.

Targets the cluster quest_service__c4, covering four methods:

* ``_compute_session_set_stats``       - per-session economics aggregation
* ``_compute_playlist_reward_stats``   - per-playlist reward aggregation
* ``_sum_session_quest_rewards``       - the reducer both of the above lean on
* ``match_quest_by_mission_name``      - chat.log mission → quest resolution

Each method is exercised through a real ``QuestService`` over a fresh on-disk
SQLite database, mirroring the sibling property suite's fixture so the mutants
meet the genuine production query paths (no mocks, no stubbed cursors).

The asserts pin three kinds of behaviour the mutants break:

1. The exact key *set* of every returned dict (key-rename mutants flip a key to
   ``XXfooXX`` / ``FOO``; the spread into the public analytics payload would then
   carry the wrong key - so the contract is observable).
2. The exact *value* mapped to each key, including which SQL aggregate column
   feeds which key and which default the empty-input branch returns.
3. The mission-matching tie-breaks, length floor, suffix strip, active-only
   scope, and fuzzy threshold boundary.
"""

from pathlib import Path

import pytest

from backend.db.app_database import AppDatabase
from backend.services.quest_service import (
    QuestService,
)

# tracking_sessions / kills / kill_tool_stats are owned by the tracker at
# runtime; the quest + completion + skill_gains tables come from AppDatabase.
# Create the tracker-side tables the analytics queries read from.
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

# The exact contract of the two analytics dicts.
_SESSION_SET_KEYS = {
    "linked_sessions",
    "total_duration",
    "weapon_cost",
    "heal_cost",
    "enhancer_cost",
    "armour_cost",
    "loot_tt",
    "skill_tt",
}
_REWARD_KEYS = {
    "total_reward_ped",
    "total_immediate_reward_ped",
    "total_bonus_reward_ped",
    "total_skill_reward_ped",
    "total_immediate_skill_reward_ped",
    "total_bonus_skill_reward_ped",
    "total_expected_reward_ped",
    "total_expected_immediate_reward_ped",
    "total_expected_bonus_reward_ped",
}


@pytest.fixture
def svc(tmp_path: Path) -> QuestService:
    tmp = tmp_path / "quests.db"
    db = AppDatabase(tmp)
    db.conn.executescript(_TRACKER_SCHEMA)
    db.conn.commit()
    return QuestService(db)


def _finished_session(
    svc: QuestService,
    session_id: str,
    *,
    started_at: float = 1000.0,
    ended_at: float = 2000.0,
    heal_cost: float = 0.0,
    armour_cost: float = 0.0,
) -> None:
    svc._conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, heal_cost, armour_cost) "
        "VALUES (?, ?, ?, 0, ?, ?)",
        (session_id, started_at, ended_at, heal_cost, armour_cost),
    )
    svc._conn.commit()


def _kill(
    svc: QuestService,
    kill_id: str,
    session_id: str,
    *,
    loot_total_ped: float = 0.0,
    enhancer_cost: float = 0.0,
) -> None:
    svc._conn.execute(
        "INSERT INTO kills (id, session_id, loot_total_ped, enhancer_cost) "
        "VALUES (?, ?, ?, ?)",
        (kill_id, session_id, loot_total_ped, enhancer_cost),
    )
    svc._conn.commit()


def _tool(
    svc: QuestService, kill_id: str, *, cost_per_shot: float, shots_fired: int
) -> None:
    svc._conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, cost_per_shot, shots_fired) "
        "VALUES (?, ?, ?)",
        (kill_id, cost_per_shot, shots_fired),
    )
    svc._conn.commit()


def _skill_gain(svc: QuestService, session_id: str, *, ped_value: float) -> None:
    svc._conn.execute(
        "INSERT INTO skill_gains "
        "(session_id, timestamp, skill_name, amount, ped_value, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (session_id, 1500.0, "Test Skill", 1.0, ped_value, 1500.0),
    )
    svc._conn.commit()


def _complete(svc: QuestService, session_id: str, quest_id: int) -> None:
    svc._conn.execute(
        "INSERT OR IGNORE INTO session_quest_completions "
        "(session_id, quest_id) VALUES (?, ?)",
        (session_id, quest_id),
    )
    svc._conn.commit()


# ── _compute_session_set_stats ─────────────────────────────────────────────


class TestComputeSessionSetStats:
    def test_empty_returns_exact_zero_contract(self, svc: QuestService):
        """Empty session set: every documented key present and exactly 0.

        Kills the empty-branch key renames (XX../UPPER) and the 0->1 value
        mutants by pinning both the key set and each value.
        """
        out = svc._compute_session_set_stats([])
        assert set(out) == _SESSION_SET_KEYS
        assert out == dict.fromkeys(_SESSION_SET_KEYS, 0)

    def test_nonempty_keys_and_distinct_values(self, svc: QuestService):
        """Two sessions with distinct per-column values.

        Distinct values per column let column-swap mutants (e.g. duration
        reading sess_row[2], heal reading sess_row[3]) be caught, and the key
        set pins the main-return key renames. Two sessions also force the
        placeholder-join mutant ('?,?' -> '?XX,XX?') into a SQL syntax error.
        """
        _finished_session(
            svc, "s1", started_at=0.0, ended_at=100.0, heal_cost=7.0, armour_cost=3.0
        )
        _finished_session(
            svc,
            "s2",
            started_at=0.0,
            ended_at=400.0,
            heal_cost=11.0,
            armour_cost=5.0,
        )
        _kill(svc, "k1", "s1", loot_total_ped=20.0, enhancer_cost=2.0)
        _kill(svc, "k2", "s2", loot_total_ped=30.0, enhancer_cost=4.0)
        _tool(svc, "k1", cost_per_shot=0.5, shots_fired=10)  # 5.0
        _tool(svc, "k2", cost_per_shot=1.0, shots_fired=20)  # 20.0
        _skill_gain(svc, "s1", ped_value=1.5)
        _skill_gain(svc, "s2", ped_value=2.5)

        out = svc._compute_session_set_stats(["s1", "s2"])

        assert set(out) == _SESSION_SET_KEYS
        # duration = (100-0)+(400-0) = 500; heal = 7+11 = 18; armour = 3+5 = 8.
        # These three are all distinct, so a duration<-heal<-armour column swap
        # changes at least one of them.
        assert out["linked_sessions"] == 2
        assert out["total_duration"] == 500.0
        assert out["heal_cost"] == 18.0
        assert out["armour_cost"] == 8.0
        assert out["weapon_cost"] == 25.0  # 5.0 + 20.0
        assert out["enhancer_cost"] == 6.0  # 2.0 + 4.0
        assert out["loot_tt"] == 50.0  # 20.0 + 30.0
        assert out["skill_tt"] == 4.0  # 1.5 + 2.5

    def test_placeholder_join_two_sessions_does_not_raise(self, svc: QuestService):
        """A 2+ session query must produce valid 'IN (?,?)' SQL.

        The 'XX,XX' join mutant yields 'IN (?XX,XX?)' which is a SQL syntax
        error; a plain successful aggregation here kills it.
        """
        _finished_session(svc, "a", ended_at=1500.0)
        _finished_session(svc, "b", ended_at=1500.0)
        out = svc._compute_session_set_stats(["a", "b"])
        assert out["linked_sessions"] == 2


# ── _compute_playlist_reward_stats ─────────────────────────────────────────


class TestComputePlaylistRewardStats:
    def test_empty_returns_exact_zero_contract(self, svc: QuestService):
        """Empty session set: every reward key present and exactly 0.

        Kills the empty-branch reward key renames and 0->1 value mutants.
        """
        out = svc._compute_playlist_reward_stats([], [1], [2])
        assert set(out) == _REWARD_KEYS
        assert out == dict.fromkeys(_REWARD_KEYS, 0)

    def test_bonus_skill_only_filter_excludes_nonskill(self, svc: QuestService):
        """long_horizon (bonus) skill total must filter to skill quests only.

        Two mutants drop ``skill_only=True`` on the bonus-skill reducer call
        (turning it into the default ``None`` = no filter). With a non-skill
        bonus quest carrying a reward, the unfiltered sum would wrongly include
        it, inflating total_bonus_skill_reward_ped above the skill-only sum.
        """
        skill_bonus = svc.create_quest(
            {"name": "Bonus Skill", "reward_ped": 4.0, "reward_is_skill": True}
        )
        cash_bonus = svc.create_quest(
            {"name": "Bonus Cash", "reward_ped": 9.0, "reward_is_skill": False}
        )
        immediate = svc.create_quest(
            {"name": "Immediate", "reward_ped": 2.0, "reward_is_skill": False}
        )
        _finished_session(svc, "s1")
        _complete(svc, "s1", skill_bonus["id"])
        _complete(svc, "s1", cash_bonus["id"])
        _complete(svc, "s1", immediate["id"])

        out = svc._compute_playlist_reward_stats(
            ["s1"],
            [immediate["id"]],
            [skill_bonus["id"], cash_bonus["id"]],
        )
        assert set(out) == _REWARD_KEYS
        # Only the skill bonus (4.0) counts toward bonus skill; the 9.0 cash
        # bonus must be excluded. A dropped skill_only filter would give 13.0.
        assert out["total_bonus_skill_reward_ped"] == 4.0
        # And the rest of the contract stays consistent.
        assert out["total_bonus_reward_ped"] == 13.0  # 4.0 + 9.0
        assert out["total_immediate_reward_ped"] == 2.0
        assert out["total_reward_ped"] == 15.0


# ── _sum_session_quest_rewards ─────────────────────────────────────────────


class TestSumSessionQuestRewards:
    def _world_with_markup(self, svc: QuestService):
        """One non-skill quest, completed in one session, markup 150%."""
        q = svc.create_quest(
            {
                "name": "Markup Quest",
                "reward_ped": 10.0,
                "reward_is_skill": False,
                "expected_reward_markup_percent": 150.0,
            }
        )
        _finished_session(svc, "s1")
        _complete(svc, "s1", q["id"])
        return q

    def test_default_is_raw_not_expected(self, svc: QuestService):
        """Default expected=False sums raw reward_ped, not the markup CASE.

        Kills the ``expected: bool = False`` -> ``True`` default flip: with a
        150% markup the raw sum is 10.0 while the expected sum is 15.0.
        """
        q = self._world_with_markup(svc)
        raw = svc._sum_session_quest_rewards(["s1"], [q["id"]])
        assert raw == 10.0
        expected = svc._sum_session_quest_rewards(["s1"], [q["id"]], expected=True)
        assert expected == 15.0

    def test_skill_only_false_excludes_skill_quests(self, svc: QuestService):
        """skill_only=False must apply 'AND reward_is_skill = 0'.

        Kills the elif mutants: branch swapped to ``is True`` (filter never set,
        so a skill quest leaks in), filter set to ``None`` (SQL 'None' syntax
        error), and the 'XX..XX' literal (SQL syntax error). With a skill quest
        and a cash quest both completed, skill_only=False must sum only cash.
        """
        cash = svc.create_quest(
            {"name": "Cash", "reward_ped": 6.0, "reward_is_skill": False}
        )
        skill = svc.create_quest(
            {"name": "Skill", "reward_ped": 8.0, "reward_is_skill": True}
        )
        _finished_session(svc, "s1")
        _complete(svc, "s1", cash["id"])
        _complete(svc, "s1", skill["id"])

        cash_only = svc._sum_session_quest_rewards(
            ["s1"], [cash["id"], skill["id"]], skill_only=False
        )
        assert cash_only == 6.0
        skill_total = svc._sum_session_quest_rewards(
            ["s1"], [cash["id"], skill["id"]], skill_only=True
        )
        assert skill_total == 8.0

    def test_two_sessions_placeholder_join_valid(self, svc: QuestService):
        """A 2+ session reward sum must build valid 'IN (?,?)' SQL.

        Kills the session placeholder 'XX,XX' join mutant (which only differs
        from the original when there are 2+ session ids).
        """
        q = svc.create_quest(
            {"name": "Shared", "reward_ped": 3.0, "reward_is_skill": False}
        )
        _finished_session(svc, "s1")
        _finished_session(svc, "s2")
        _complete(svc, "s1", q["id"])
        _complete(svc, "s2", q["id"])
        total = svc._sum_session_quest_rewards(["s1", "s2"], [q["id"]])
        assert total == 6.0


# ── match_quest_by_mission_name ────────────────────────────────────────────


class TestMatchQuestByMissionName:
    def test_repeatable_suffix_stripped_to_empty(self, svc: QuestService):
        """'(repeatable)' must be replaced with '' (empty), not a marker.

        Quest name 'AB' is too short for the substring path (<5 chars) and the
        fuzzy ratio of 'ab' vs 'abxxxx' is below threshold, so only an *empty*
        replacement leaves an exact match. The sub('XXXX') mutant breaks it.
        """
        q = svc.create_quest({"name": "AB"})
        match = svc.match_quest_by_mission_name("AB (repeatable)")
        assert match is not None
        assert match["id"] == q["id"]

    def test_inactive_quests_are_not_matched(self, svc: QuestService):
        """Only active quests are searched (active_only=True).

        The ``active_only=None`` / ``=False`` mutants would widen the search to
        inactive quests. With the only exact-name match deactivated, the lookup
        must return None rather than the inactive quest.
        """
        q = svc.create_quest({"name": "Deactivated Hunt Mission"})
        # Soft-delete: delete_quest deactivates rather than dropping the row.
        svc.delete_quest(q["id"])
        match = svc.match_quest_by_mission_name("Deactivated Hunt Mission")
        assert match is None

    def test_substring_length_floor_is_five(self, svc: QuestService):
        """A 5-char normalised quest name must be eligible for substring match.

        Kills the floor mutants ``len >= 5`` -> ``> 5`` and ``>= 6``: the quest
        name normalises to exactly 5 chars, is contained in the mission name as
        a substring (but is not an exact match), and its fuzzy ratio is below
        threshold, so it is found only when the floor admits length 5.
        """
        q = svc.create_quest({"name": "Hunts"})  # normalises to 'hunts', len 5
        match = svc.match_quest_by_mission_name("Daily Hunts In The Plains")
        assert match is not None
        assert match["id"] == q["id"]

    def test_fuzzy_tie_breaks_to_first_quest(self, svc: QuestService):
        """On equal fuzzy scores, the earliest-created quest wins.

        Kills ``score > best_score`` -> ``score >= best_score``: two quests
        produce the same (above-threshold) ratio against the mission name; the
        strict comparison keeps the first, the mutant flips to the last.
        """
        first = svc.create_quest({"name": "Alpha Strike Mission X"})
        second = svc.create_quest({"name": "Alpha Strike Mission Y"})
        # Mission equidistant from both: differs by one trailing char from each.
        match = svc.match_quest_by_mission_name("Alpha Strike Mission Z")
        assert match is not None
        assert match["id"] == first["id"]
        assert match["id"] != second["id"]

    def test_fuzzy_threshold_is_inclusive(self, svc: QuestService):
        """A fuzzy score of exactly the threshold (0.8) must match.

        Kills ``best_score >= _FUZZY_THRESHOLD`` -> ``>``: 'abcdz' vs 'abcdy'
        has SequenceMatcher ratio 2*4/(5+5) = 0.8 exactly (no exact match, not a
        substring of each other, no shorter quest interfering), so the inclusive
        comparison returns the quest and the strict one returns None.
        """
        q = svc.create_quest({"name": "abcdz"})
        match = svc.match_quest_by_mission_name("abcdy")
        assert match is not None
        assert match["id"] == q["id"]
