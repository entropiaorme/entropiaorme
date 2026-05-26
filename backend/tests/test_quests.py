"""Tests for quest service: CRUD, cooldowns, playlists, completion."""

from datetime import UTC
from pathlib import Path

import pytest

from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService


@pytest.fixture
def quest_service(tmp_path: Path):
    db = AppDatabase(tmp_path / "test.db")
    return QuestService(db)


class TestQuestCRUD:
    def test_create_quest_minimal(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Kill Atrox"})
        assert q["name"] == "Kill Atrox"
        assert q["planet"] == "Calypso"
        assert q["is_active"] == 1

    def test_create_quest_full(self, quest_service: QuestService):
        q = quest_service.create_quest(
            {
                "name": "Iron Challenge: Atrox",
                "planet": "Calypso",
                "waypoint": "/wp [Calypso, 35424, 24577, 130, Atrox]",
                "cooldown_hours": 21,
                "reward_ped": 1.33,
                "reward_is_skill": False,
                "expected_reward_markup_percent": 130.0,
                "notes": "Bring extra ammo",
                "chain_name": "Iron Atrox",
                "chain_position": 1,
                "chain_total": 10,
                "mobs": ["Atrox Young", "Atrox Mature"],
            }
        )
        assert q["name"] == "Iron Challenge: Atrox"
        assert q["cooldown_hours"] == 21
        assert q["reward_ped"] == 1.33
        assert q["expected_reward_markup_percent"] == 130.0
        assert q["mobs"] == ["Atrox Mature", "Atrox Young"]  # sorted alphabetically
        assert q["chain_name"] == "Iron Atrox"
        assert q["chain_position"] == 1
        assert q["chain_total"] == 10

    def test_get_quest(self, quest_service: QuestService):
        created = quest_service.create_quest({"name": "Test"})
        fetched = quest_service.get_quest(created["id"])
        assert fetched is not None
        assert fetched["name"] == "Test"

    def test_get_quest_not_found(self, quest_service: QuestService):
        assert quest_service.get_quest(999) is None

    def test_update_quest(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Original"})
        updated = quest_service.update_quest(
            q["id"], {"name": "Updated", "planet": "Arkadia"}
        )
        assert updated is not None
        assert updated["name"] == "Updated"
        assert updated["planet"] == "Arkadia"

    def test_update_quest_skill_reward_clears_expected_markup(
        self, quest_service: QuestService
    ):
        q = quest_service.create_quest(
            {
                "name": "Original",
                "reward_ped": 5.0,
                "expected_reward_markup_percent": 140.0,
            }
        )
        updated = quest_service.update_quest(q["id"], {"reward_is_skill": True})
        assert updated is not None
        assert updated["reward_is_skill"] == 1
        assert updated["expected_reward_markup_percent"] is None

    def test_update_quest_mobs(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test", "mobs": ["Atrox"]})
        assert q["mobs"] == ["Atrox"]
        updated = quest_service.update_quest(q["id"], {"mobs": ["Foul", "Snablesnot"]})
        assert updated is not None
        assert updated["mobs"] == ["Foul", "Snablesnot"]

    def test_delete_quest(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "ToDelete"})
        assert quest_service.delete_quest(q["id"]) is True
        # Soft-deleted: not in active list
        assert len(quest_service.get_quests()) == 0
        # Still in DB if we ask for all
        assert len(quest_service.get_quests(active_only=False)) == 1

    def test_delete_quest_not_found(self, quest_service: QuestService):
        assert quest_service.delete_quest(999) is False

    def test_list_quests(self, quest_service: QuestService):
        quest_service.create_quest({"name": "A"})
        quest_service.create_quest({"name": "B"})
        quests = quest_service.get_quests()
        assert len(quests) == 2
        assert quests[0]["name"] == "A"
        assert quests[1]["name"] == "B"

    def test_row_to_quest_normalizes_numeric_string_id(
        self, quest_service: QuestService
    ):
        row = {"id": "7", "last_completed_at": None, "cooldown_hours": None}
        quest = quest_service._row_to_quest(row)
        assert quest["id"] == 7

    def test_row_to_quest_normalizes_bytes_id(self, quest_service: QuestService):
        row = {"id": b"11", "last_completed_at": None, "cooldown_hours": None}
        quest = quest_service._row_to_quest(row)
        assert quest["id"] == 11

    def test_scalar_from_row_returns_none_for_malformed_empty_tuple(
        self, quest_service: QuestService
    ):
        assert quest_service._scalar_from_row((), "mob_name") is None


class TestQuestActions:
    def test_start_quest(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test"})
        assert q["started_at"] is None
        started = quest_service.start_quest(q["id"])
        assert started is not None
        assert started["started_at"] is not None

    def test_cancel_started_quest(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test"})
        quest_service.start_quest(q["id"])
        cancelled = quest_service.cancel_quest(q["id"])
        assert cancelled is not None
        assert cancelled["started_at"] is None

    def test_complete_quest(self, quest_service: QuestService):
        q = quest_service.create_quest(
            {
                "name": "Test",
                "cooldown_hours": 24,
                "reward_ped": 5.0,
            }
        )
        quest_service.start_quest(q["id"])
        completed = quest_service.complete_quest(q["id"])
        assert completed is not None
        assert completed["started_at"] is None
        assert completed["last_completed_at"] is not None
        assert completed["cooldown_expires_at"] is not None

    def test_complete_quest_creates_ledger_entry(self, quest_service: QuestService):
        q = quest_service.create_quest(
            {
                "name": "Reward Quest",
                "reward_ped": 3.50,
            }
        )
        quest_service.complete_quest(q["id"])
        # Check ledger entry was created
        row = quest_service._conn.execute(
            "SELECT * FROM ledger_entries WHERE tag = 'quest_reward'"
        ).fetchone()
        assert row is not None
        entry = dict(row)
        assert entry["amount"] == 3.50
        assert "Reward Quest" in entry["description"]

    def test_complete_skill_quest_creates_quest_claim_not_ledger(
        self, quest_service: QuestService
    ):
        """Skill quest completion writes to quest_claims, never the ledger."""
        q = quest_service.create_quest(
            {
                "name": "Skill Reward Quest",
                "reward_ped": 4.0,
                "reward_is_skill": True,
            }
        )
        quest_service.complete_quest(q["id"])

        ledger_row = quest_service._conn.execute(
            "SELECT * FROM ledger_entries WHERE tag = 'quest_reward'"
        ).fetchone()
        assert ledger_row is None

        claim_row = quest_service._conn.execute(
            "SELECT quest_id, quest_name, ped_value FROM quest_claims"
        ).fetchone()
        assert claim_row is not None
        assert claim_row[0] == q["id"]
        assert claim_row[1] == "Skill Reward Quest"
        assert claim_row[2] == 4.0

    def test_complete_quest_no_reward_no_ledger(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "No Reward"})
        quest_service.complete_quest(q["id"])
        row = quest_service._conn.execute(
            "SELECT * FROM ledger_entries WHERE tag = 'quest_reward'"
        ).fetchone()
        assert row is None
        claim_row = quest_service._conn.execute("SELECT * FROM quest_claims").fetchone()
        assert claim_row is None

    def test_complete_records_completion_rows(self, quest_service: QuestService):
        """Each complete_quest call records an append-only completion row.

        With cooldown derived from session_quest_completions, manual
        completions insert a synthetic `manual-<uuid>` row per call so
        repeated completes accumulate rather than collapsing.
        """
        q = quest_service.create_quest({"name": "Test"})
        quest_service.complete_quest(q["id"])
        quest_service.complete_quest(q["id"])
        count = quest_service._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()[0]
        assert count == 2

    def test_complete_without_active_session_still_records_for_cooldown(
        self,
        quest_service: QuestService,
    ):
        """Manual completion (no tracking session) records a synthetic row so
        cooldown still engages."""
        quest_service._conn.execute("""
            CREATE TABLE IF NOT EXISTS tracking_sessions (
                id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
                is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER)
        """)
        quest_service._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        quest_service._conn.commit()

        q = quest_service.create_quest(
            {
                "name": "Link Test",
                "cooldown_hours": 24,
                "reward_ped": 1.0,
            }
        )
        completed = quest_service.complete_quest(q["id"])
        assert completed is not None
        assert completed["cooldown_expires_at"] is not None

        row = quest_service._conn.execute(
            "SELECT session_id FROM session_quest_completions WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()
        assert row is not None
        assert row[0].startswith("manual-")


class TestQuestSchemaMigration:
    def test_cancel_completed_quest_without_reward_undo_keeps_ledger(
        self, quest_service: QuestService
    ):
        q = quest_service.create_quest(
            {
                "name": "Reward Quest",
                "cooldown_hours": 24,
                "reward_ped": 3.50,
            }
        )
        quest_service.complete_quest(q["id"])

        cancelled = quest_service.cancel_quest(q["id"], undo_reward=False)
        assert cancelled is not None
        assert cancelled["cooldown_expires_at"] is None
        assert cancelled["started_at"] is None

        row = quest_service._conn.execute(
            "SELECT COUNT(*) FROM ledger_entries WHERE tag = 'quest_reward' AND description = ?",
            ("Quest: Reward Quest",),
        ).fetchone()
        assert row[0] == 1

    def test_cancel_completed_quest_with_reward_undo_removes_ledger(
        self, quest_service: QuestService
    ):
        q = quest_service.create_quest(
            {
                "name": "Reward Quest",
                "cooldown_hours": 24,
                "reward_ped": 3.50,
            }
        )
        quest_service.complete_quest(q["id"])

        cancelled = quest_service.cancel_quest(q["id"], undo_reward=True)
        assert cancelled is not None
        assert cancelled["cooldown_expires_at"] is None

        row = quest_service._conn.execute(
            "SELECT COUNT(*) FROM ledger_entries WHERE tag = 'quest_reward' AND description = ?",
            ("Quest: Reward Quest",),
        ).fetchone()
        assert row[0] == 0

    def test_cancel_completed_skill_quest_with_reward_undo_removes_claim(
        self, quest_service: QuestService
    ):
        """Skill-reward undo deletes the quest_claims row, not a ledger row."""
        q = quest_service.create_quest(
            {
                "name": "Skill Reward Quest",
                "cooldown_hours": 24,
                "reward_ped": 3.0,
                "reward_is_skill": True,
            }
        )
        quest_service.complete_quest(q["id"])

        before = quest_service._conn.execute(
            "SELECT COUNT(*) FROM quest_claims WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()[0]
        assert before == 1

        quest_service.cancel_quest(q["id"], undo_reward=True)

        after = quest_service._conn.execute(
            "SELECT COUNT(*) FROM quest_claims WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()[0]
        assert after == 0

    def test_complete_no_sessions_at_all(self, quest_service: QuestService):
        """Without a tracking-session row, complete_quest still records a
        completion (synthetic manual key) so cooldown still works."""
        quest_service._conn.execute("""
            CREATE TABLE IF NOT EXISTS tracking_sessions (
                id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
                is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER)
        """)
        quest_service._conn.commit()
        q = quest_service.create_quest({"name": "No Session", "cooldown_hours": 1})
        completed = quest_service.complete_quest(q["id"])
        assert completed is not None
        assert completed["cooldown_expires_at"] is not None

        row = quest_service._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()
        assert row[0] == 1


class TestCooldown:
    def test_no_cooldown(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test"})
        assert q["cooldown_expires_at"] is None

    def test_cooldown_after_completion(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test", "cooldown_hours": 24})
        quest_service.complete_quest(q["id"])
        q2 = quest_service.get_quest(q["id"])
        assert q2 is not None
        assert q2["cooldown_expires_at"] is not None
        # Expires ~24h from now
        from datetime import datetime

        expires = datetime.fromisoformat(q2["cooldown_expires_at"])
        now = datetime.now(UTC)
        diff_hours = (expires - now).total_seconds() / 3600
        assert 23.9 < diff_hours < 24.1

    def test_no_cooldown_hours_no_expiry(self, quest_service: QuestService):
        """Quest with no cooldown_hours should never show cooldown_expires_at."""
        q = quest_service.create_quest({"name": "Test"})
        quest_service.complete_quest(q["id"])
        q2 = quest_service.get_quest(q["id"])
        assert q2 is not None
        assert q2["cooldown_expires_at"] is None

    def test_cancel_completed_quest_removes_current_session_link(self, tmp_path: Path):
        from backend.core.event_bus import EventBus

        bus = EventBus()
        db = AppDatabase(tmp_path / "test.db")
        db.conn.execute("""
            CREATE TABLE IF NOT EXISTS tracking_sessions (
                id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
                is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER)
        """)
        db.conn.commit()
        quest_service = QuestService(db, bus)
        q = quest_service.create_quest(
            {
                "name": "Session Quest",
                "cooldown_hours": 24,
                "reward_ped": 1.0,
            }
        )

        bus.publish("session_started", {"session_id": "sess-1"})
        quest_service.complete_quest(q["id"])

        before = quest_service._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE session_id = 'sess-1' AND quest_id = ?",
            (q["id"],),
        ).fetchone()
        assert before[0] == 1

        quest_service.cancel_quest(q["id"], undo_reward=False)

        after = quest_service._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE session_id = 'sess-1' AND quest_id = ?",
            (q["id"],),
        ).fetchone()
        assert after[0] == 0


class TestPlaylists:
    def test_create_playlist(self, quest_service: QuestService):
        q1 = quest_service.create_quest({"name": "A"})
        q2 = quest_service.create_quest({"name": "B"})
        pl = quest_service.create_playlist(
            {
                "name": "Daily Loop",
                "planet": "Calypso",
                "estimated_minutes": 45,
                "quest_ids": [q1["id"], q2["id"]],
            }
        )
        assert pl["name"] == "Daily Loop"
        assert pl["quest_ids"] == [q1["id"], q2["id"]]
        assert pl["immediate_quest_ids"] == [q1["id"], q2["id"]]
        assert pl["long_horizon_quest_ids"] == []
        assert pl["estimated_minutes"] == 45

    def test_create_playlist_with_long_horizon_group(self, quest_service: QuestService):
        q1 = quest_service.create_quest({"name": "A"})
        q2 = quest_service.create_quest({"name": "B"})
        q3 = quest_service.create_quest({"name": "C"})
        pl = quest_service.create_playlist(
            {
                "name": "Daily Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )
        assert pl["quest_ids"] == [q1["id"], q2["id"], q3["id"]]
        assert pl["immediate_quest_ids"] == [q1["id"], q2["id"]]
        assert pl["long_horizon_quest_ids"] == [q3["id"]]
        assert [item["group_type"] for item in pl["items"]] == [
            "immediate",
            "immediate",
            "long_horizon",
        ]

    def test_update_playlist_reorder(self, quest_service: QuestService):
        q1 = quest_service.create_quest({"name": "A"})
        q2 = quest_service.create_quest({"name": "B"})
        pl = quest_service.create_playlist(
            {
                "name": "Loop",
                "quest_ids": [q1["id"], q2["id"]],
            }
        )
        updated = quest_service.update_playlist(
            pl["id"],
            {
                "quest_ids": [q2["id"], q1["id"]],
            },
        )
        assert updated is not None
        assert updated["quest_ids"] == [q2["id"], q1["id"]]

    def test_update_playlist_reclassify_group(self, quest_service: QuestService):
        q1 = quest_service.create_quest({"name": "A"})
        q2 = quest_service.create_quest({"name": "B"})
        pl = quest_service.create_playlist(
            {
                "name": "Loop",
                "quest_ids": [q1["id"], q2["id"]],
            }
        )
        updated = quest_service.update_playlist(
            pl["id"],
            {
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "long_horizon"},
                ],
            },
        )
        assert updated is not None
        assert updated["immediate_quest_ids"] == [q1["id"]]
        assert updated["long_horizon_quest_ids"] == [q2["id"]]

    def test_delete_playlist(self, quest_service: QuestService):
        pl = quest_service.create_playlist({"name": "Loop"})
        assert quest_service.delete_playlist(pl["id"]) is True
        assert len(quest_service.get_playlists()) == 0

    def test_quest_playlist_ids(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test"})
        pl = quest_service.create_playlist(
            {
                "name": "Loop",
                "quest_ids": [q["id"]],
            }
        )
        q2 = quest_service.get_quest(q["id"])
        assert q2 is not None
        assert pl["id"] in q2["playlist_ids"]

    def test_delete_quest_removes_from_playlist(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Test"})
        pl = quest_service.create_playlist(
            {
                "name": "Loop",
                "quest_ids": [q["id"]],
            }
        )
        quest_service.delete_quest(q["id"])
        pl2 = quest_service.get_playlist(pl["id"])
        assert pl2 is not None
        assert pl2["quest_ids"] == []


class TestMissionMatching:
    """Test quest matching and reward filter for chat.log mission detection."""

    def test_exact_match(self, quest_service: QuestService):
        quest_service.create_quest({"name": "Paneleon Hunter Jameson's Mission"})
        match = quest_service.match_quest_by_mission_name(
            "Paneleon Hunter Jameson's Mission"
        )
        assert match is not None
        assert match["name"] == "Paneleon Hunter Jameson's Mission"

    def test_match_strips_repeatable_suffix(self, quest_service: QuestService):
        quest_service.create_quest({"name": "Paneleon Hunter Jameson's Mission"})
        match = quest_service.match_quest_by_mission_name(
            "Paneleon Hunter Jameson's Mission (repeatable)"
        )
        assert match is not None
        assert match["name"] == "Paneleon Hunter Jameson's Mission"

    def test_substring_match(self, quest_service: QuestService):
        quest_service.create_quest({"name": "Atlas Haven Imperium Ranger Hunt!"})
        match = quest_service.match_quest_by_mission_name(
            "Atlas Haven Imperium Ranger Hunt! (repeatable)"
        )
        assert match is not None

    def test_no_match_returns_none(self, quest_service: QuestService):
        quest_service.create_quest({"name": "Totally Different Quest"})
        match = quest_service.match_quest_by_mission_name("Unknown Mission")
        assert match is None

    def test_short_name_no_false_match(self, quest_service: QuestService):
        """Quest names shorter than 5 chars should not substring-match."""
        quest_service.create_quest({"name": "Hunt"})
        match = quest_service.match_quest_by_mission_name("Paneleon Hunter Mission")
        assert match is None

    def test_case_insensitive_match(self, quest_service: QuestService):
        quest_service.create_quest({"name": "aris - daily hunting 1"})
        match = quest_service.match_quest_by_mission_name("ARIS - Daily Hunting 1")
        assert match is not None

    def test_smart_quote_in_db_straight_in_chatlog(self, quest_service: QuestService):
        """DB has Unicode right single quotation mark (U+2019), chat.log sends straight apostrophe."""
        quest_service.create_quest({"name": "Paneleon Hunter Jameson\u2019s Mission"})
        match = quest_service.match_quest_by_mission_name(
            "Paneleon Hunter Jameson's Mission"
        )
        assert match is not None
        assert match["name"] == "Paneleon Hunter Jameson\u2019s Mission"

    def test_straight_in_db_smart_quote_in_chatlog(self, quest_service: QuestService):
        """Reverse: DB has straight apostrophe, chat.log sends smart quote."""
        quest_service.create_quest({"name": "Paneleon Hunter Jameson's Mission"})
        match = quest_service.match_quest_by_mission_name(
            "Paneleon Hunter Jameson\u2019s Mission"
        )
        assert match is not None
        assert match["name"] == "Paneleon Hunter Jameson's Mission"

    def test_fuzzy_match_minor_typo(self, quest_service: QuestService):
        """Minor typo should still match via fuzzy fallback."""
        quest_service.create_quest({"name": "Zyn'Nix Tempo - Duster Workers"})
        match = quest_service.match_quest_by_mission_name(
            "Zyn'Nix Tempo - Duster Workeks"
        )
        assert match is not None
        assert match["name"] == "Zyn'Nix Tempo - Duster Workers"

    def test_fuzzy_no_match_too_different(self, quest_service: QuestService):
        """Completely different name should not fuzzy-match."""
        quest_service.create_quest({"name": "Zyn'Nix Tempo - Duster Workers"})
        match = quest_service.match_quest_by_mission_name(
            "Totally Unrelated Quest Name"
        )
        assert match is None

    def test_fuzzy_picks_highest_score(self, quest_service: QuestService):
        """When multiple quests exceed threshold, the highest-scoring one wins."""
        quest_service.create_quest({"name": "Zyn'Nix Tempo - Duster Workers"})
        quest_service.create_quest({"name": "Zyn'Nix Tempo - Duster Workeks"})
        # Exact normalised match for the first quest, so it should win
        match = quest_service.match_quest_by_mission_name(
            "Zyn'Nix Tempo - Duster Workers"
        )
        assert match is not None
        assert match["name"] == "Zyn'Nix Tempo - Duster Workers"

    def test_reward_filter_suppresses_ped_reward(self, quest_service: QuestService):
        """Filter identifies loot item matching quest reward_ped and returns its index."""
        quest_service.create_quest(
            {
                "name": "Paneleon Hunter Jameson's Mission",
                "reward_ped": 1.5,
            }
        )
        loot_items = [
            {"item_name": "Universal Ammo", "quantity": 15000, "value": 1.5},
            {"item_name": "Shrapnel", "quantity": 825, "value": 0.0825},
            {"item_name": "Animal Eye Oil", "quantity": 16, "value": 0.80},
        ]
        result = quest_service.quest_reward_filter(
            "Paneleon Hunter Jameson's Mission (repeatable)",
            loot_items,
            [],
        )
        assert result is not None
        assert result["suppress_loot_index"] == 0  # Universal Ammo
        assert result["suppress_skill_index"] is None

        # Verify quest was auto-completed: a completion row exists and
        # cooldown engages where applicable.
        q = quest_service.get_quests()[0]
        count = quest_service._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE quest_id = ?",
            (q["id"],),
        ).fetchone()[0]
        assert count == 1

    def test_reward_filter_zero_ped_suppresses_lowest(
        self, quest_service: QuestService
    ):
        """0 PED reward suppresses the lowest-value item."""
        quest_service.create_quest(
            {
                "name": "Atlas Haven Imperium Ranger Hunt!",
                "reward_ped": 0,
            }
        )
        loot_items = [
            {"item_name": "A.R.C. Faction Badge", "quantity": 3, "value": 0.0},
            {"item_name": "Shrapnel", "quantity": 825, "value": 0.0825},
        ]
        result = quest_service.quest_reward_filter(
            "Atlas Haven Imperium Ranger Hunt! (repeatable)",
            loot_items,
            [],
        )
        assert result is not None
        assert result["suppress_loot_index"] == 0  # Badge (0 PED)

    def test_reward_filter_skill_reward(self, quest_service: QuestService):
        """Skill quest reward suppresses first skill gain and routes the
        reward into quest_claims rather than ledger_entries."""
        quest_service.create_quest(
            {
                "name": "Skill Quest",
                "reward_ped": 5.0,
                "reward_is_skill": True,
            }
        )
        skill_gains = [
            {"skill_name": "Laser Weaponry Technology", "amount": 0.5},
        ]
        result = quest_service.quest_reward_filter(
            "Skill Quest",
            [],
            skill_gains,
        )
        assert result is not None
        assert result["suppress_skill_index"] == 0
        assert result["suppress_loot_index"] is None

        ledger_row = quest_service._conn.execute(
            "SELECT 1 FROM ledger_entries WHERE tag = 'quest_reward'"
        ).fetchone()
        assert ledger_row is None

        claim_row = quest_service._conn.execute(
            "SELECT ped_value FROM quest_claims"
        ).fetchone()
        assert claim_row is not None
        assert claim_row[0] == 5.0

    def test_reward_filter_no_match_returns_none(self, quest_service: QuestService):
        result = quest_service.quest_reward_filter(
            "Unknown Mission",
            [{"item_name": "Shrapnel", "quantity": 100, "value": 1.0}],
            [],
        )
        assert result is None

    def test_reward_filter_no_matching_loot_value(self, quest_service: QuestService):
        """If no loot item matches reward_ped, return None (no suppression)."""
        quest_service.create_quest(
            {
                "name": "Expensive Quest",
                "reward_ped": 100.0,
            }
        )
        loot_items = [
            {"item_name": "Shrapnel", "quantity": 825, "value": 0.0825},
        ]
        result = quest_service.quest_reward_filter(
            "Expensive Quest",
            loot_items,
            [],
        )
        assert result is None

    def test_reward_filter_creates_ledger_entry(self, quest_service: QuestService):
        """Auto-complete via filter creates the ledger entry."""
        quest_service.create_quest(
            {
                "name": "Ledger Quest",
                "reward_ped": 2.0,
            }
        )
        quest_service.quest_reward_filter(
            "Ledger Quest",
            [{"item_name": "UA", "quantity": 20000, "value": 2.0}],
            [],
        )
        row = quest_service._conn.execute(
            "SELECT amount FROM ledger_entries WHERE tag = 'quest_reward'"
        ).fetchone()
        assert row is not None
        assert row[0] == 2.0


class TestAutoStartQuest:
    """Test auto-start from chat.log 'New Mission received' events."""

    def test_auto_start_matches_and_starts(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Paneleon Hunter"})
        assert q["started_at"] is None
        quest_service.start_quest_from_mission("Paneleon Hunter (repeatable)")
        updated = quest_service.get_quest(q["id"])
        assert updated is not None
        assert updated["started_at"] is not None

    def test_auto_start_no_match_is_noop(self, quest_service: QuestService):
        quest_service.create_quest({"name": "Paneleon Hunter"})
        quest_service.start_quest_from_mission("Unknown Mission")
        # No error, quest unchanged
        quests = quest_service.get_quests()
        assert all(q["started_at"] is None for q in quests)

    def test_auto_start_already_started_is_noop(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "Paneleon Hunter"})
        quest_service.start_quest(q["id"])
        original = quest_service.get_quest(q["id"])
        assert original is not None
        quest_service.start_quest_from_mission("Paneleon Hunter")
        updated = quest_service.get_quest(q["id"])
        assert updated is not None
        assert updated["started_at"] == original["started_at"]

    def test_event_bus_triggers_auto_start(self, tmp_path: Path):
        from backend.core.event_bus import EventBus
        from backend.core.events import EVENT_MISSION_RECEIVED

        bus = EventBus()
        db = AppDatabase(tmp_path / "test.db")
        svc = QuestService(db, bus)
        q = svc.create_quest({"name": "Daily Hunt"})
        bus.publish(
            EVENT_MISSION_RECEIVED,
            {
                "type": "mission_received",
                "timestamp": "2026-03-25 10:00:00",
                "mission_name": "Daily Hunt (repeatable)",
            },
        )
        updated = svc.get_quest(q["id"])
        assert updated is not None
        assert updated["started_at"] is not None


class TestSessionQuestCompletions:
    """Test that quest completions during sessions are recorded in the junction table."""

    @pytest.fixture
    def svc_with_events(self, tmp_path: Path):
        from backend.core.event_bus import EventBus

        db = AppDatabase(tmp_path / "test.db")
        # Create tracking_sessions table (normally created by tracking schema)
        db.conn.execute("""
            CREATE TABLE IF NOT EXISTS tracking_sessions (
                id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
                is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER,
                armour_cost REAL DEFAULT 0, heal_cost REAL DEFAULT 0)
        """)
        db.conn.commit()
        bus = EventBus()
        svc = QuestService(db, bus)
        return svc, bus

    def test_completion_recorded_during_session(self, svc_with_events):
        svc, bus = svc_with_events
        q = svc.create_quest({"name": "Test Quest", "reward_ped": 1.5})
        bus.publish("session_started", {"session_id": "sess-abc"})

        svc.quest_reward_filter(
            "Test Quest",
            [{"item_name": "UA", "quantity": 15000, "value": 1.5}],
            [],
        )

        row = svc._conn.execute(
            "SELECT session_id, quest_id FROM session_quest_completions"
        ).fetchone()
        assert row is not None
        assert row[0] == "sess-abc"
        assert row[1] == q["id"]

    def test_recording_without_session_uses_synthetic_key(self, svc_with_events):
        """Without an active tracking session, completions record against a
        synthetic `manual-<uuid>` key so cooldown still derives correctly."""
        svc, bus = svc_with_events
        svc.create_quest({"name": "Test Quest", "reward_ped": 1.0})

        svc.quest_reward_filter(
            "Test Quest",
            [{"item_name": "UA", "quantity": 10000, "value": 1.0}],
            [],
        )

        row = svc._conn.execute(
            "SELECT session_id FROM session_quest_completions"
        ).fetchone()
        assert row is not None
        assert row[0].startswith("manual-")

    def test_duplicate_completion_ignored(self, svc_with_events):
        svc, bus = svc_with_events
        svc.create_quest({"name": "Repeatable", "reward_ped": 1.0})
        bus.publish("session_started", {"session_id": "sess-abc"})

        # Complete same quest twice in same session
        svc.quest_reward_filter(
            "Repeatable", [{"item_name": "UA", "quantity": 10000, "value": 1.0}], []
        )
        svc.quest_reward_filter(
            "Repeatable", [{"item_name": "UA", "quantity": 10000, "value": 1.0}], []
        )

        row = svc._conn.execute(
            "SELECT COUNT(*) FROM session_quest_completions WHERE session_id = 'sess-abc'"
        ).fetchone()
        assert row[0] == 1  # UNIQUE constraint, INSERT OR IGNORE

    def test_session_stop_clears_link_to_prior_session(self, svc_with_events):
        """After the session stops, subsequent completions no longer reference
        the stopped session's id; they record against a synthetic manual key."""
        svc, bus = svc_with_events
        svc.create_quest({"name": "Test Quest", "reward_ped": 1.0})

        bus.publish("session_started", {"session_id": "sess-abc"})
        bus.publish("session_stopped", {"session_id": "sess-abc"})

        svc.quest_reward_filter(
            "Test Quest",
            [{"item_name": "UA", "quantity": 10000, "value": 1.0}],
            [],
        )

        row = svc._conn.execute(
            "SELECT session_id FROM session_quest_completions"
        ).fetchone()
        assert row is not None
        assert row[0] != "sess-abc"
        assert row[0].startswith("manual-")


class TestPlaylistAnalytics:
    """Test playlist analytics with explicit curated session attribution."""

    @pytest.fixture
    def svc_with_tables(self, tmp_path: Path):
        """QuestService with all necessary tables for analytics queries."""
        from backend.core.event_bus import EventBus

        db = AppDatabase(tmp_path / "test.db")
        db.conn.executescript("""
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
        """)
        db.conn.commit()
        bus = EventBus()
        svc = QuestService(db, bus)
        return svc, bus

    def _create_session(self, svc, session_id, started=1000.0, ended=2000.0):
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            (session_id, started, ended),
        )
        svc._conn.commit()

    def _record_completion(self, svc, session_id, quest_id):
        svc._conn.execute(
            "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            (session_id, quest_id),
        )
        svc._conn.commit()

    def test_exact_match_found(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        pl = svc.create_playlist({"name": "Loop", "quest_ids": [q1["id"], q2["id"]]})

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        svc.accept_session_link_suggestion("sess-1")

        result = svc.get_playlist_analytics(pl["id"])
        assert result is not None
        assert result["matched_sessions"] == 1
        assert result["total_reward_ped"] == 3.0

    def test_extra_quest_no_match(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        q3 = svc.create_quest({"name": "C", "reward_ped": 3.0})
        pl = svc.create_playlist({"name": "Loop", "quest_ids": [q1["id"], q2["id"]]})

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        self._record_completion(svc, "sess-1", q3["id"])  # Extra quest

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 0

    def test_immediate_only_match_with_optional_long_horizon(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        q3 = svc.create_quest({"name": "C", "reward_ped": 5.0})
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        svc.accept_session_link_suggestion("sess-1")

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 1
        assert result["quest_count"] == 2
        assert result["long_horizon_quest_count"] == 1
        assert result["total_immediate_reward_ped"] == 3.0
        assert result["total_bonus_reward_ped"] == 0

    def test_long_horizon_only_completion_does_not_match(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        q3 = svc.create_quest({"name": "C", "reward_ped": 5.0})
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q3["id"])

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 0

    def test_missing_quest_no_match(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        pl = svc.create_playlist({"name": "Loop", "quest_ids": [q1["id"], q2["id"]]})

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])  # Only one of two

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 0

    def test_multiple_sessions_some_match(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        pl = svc.create_playlist({"name": "Loop", "quest_ids": [q1["id"], q2["id"]]})

        # Session 1: exact match
        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        svc.accept_session_link_suggestion("sess-1")

        # Session 2: partial (no match)
        self._create_session(svc, "sess-2", started=3000.0, ended=4000.0)
        self._record_completion(svc, "sess-2", q1["id"])

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 1

    def test_empty_playlist(self, svc_with_tables):
        svc, _ = svc_with_tables
        pl = svc.create_playlist({"name": "Empty"})

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 0
        assert result["quest_count"] == 0

    def test_economics_aggregation(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest(
            {"name": "A", "reward_ped": 1.0, "expected_reward_markup_percent": 130.0}
        )
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        q3 = svc.create_quest(
            {"name": "C", "reward_ped": 5.0, "expected_reward_markup_percent": 200.0}
        )
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )

        self._create_session(svc, "sess-1", started=1000.0, ended=2000.0)
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        self._record_completion(svc, "sess-1", q3["id"])
        svc.accept_session_link_suggestion("sess-1")

        # Add kill with loot
        svc._conn.execute(
            "INSERT INTO kills (id, session_id, timestamp, loot_total_ped, enhancer_cost) "
            "VALUES ('k1', 'sess-1', 1500, 5.0, 0.5)"
        )
        svc._conn.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, cost_per_shot, shots_fired) "
            "VALUES ('k1', 'Weapon', 0.01, 100)"
        )
        svc._conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
            "VALUES ('sess-1', 1200, 'Laser', 0.1, 0.05)"
        )
        svc._conn.commit()

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 1
        assert result["total_immediate_reward_ped"] == 3.0
        assert result["total_bonus_reward_ped"] == 5.0
        assert result["total_reward_ped"] == 8.0
        assert result["total_expected_immediate_reward_ped"] == pytest.approx(3.3)
        assert result["total_expected_bonus_reward_ped"] == pytest.approx(10.0)
        assert result["total_expected_reward_ped"] == pytest.approx(13.3)
        assert result["loot_tt"] == pytest.approx(5.0)
        assert result["weapon_cost"] == pytest.approx(1.0)  # 0.01 * 100
        assert result["enhancer_cost"] == pytest.approx(0.5)
        assert result["skill_tt"] == pytest.approx(0.05)

    def test_economics_aggregation_tracks_skill_reward_splits(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest(
            {"name": "A", "reward_ped": 1.0, "expected_reward_markup_percent": 130.0}
        )
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0, "reward_is_skill": True})
        q3 = svc.create_quest({"name": "C", "reward_ped": 5.0, "reward_is_skill": True})
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )

        self._create_session(svc, "sess-1", started=1000.0, ended=2000.0)
        self._record_completion(svc, "sess-1", q1["id"])
        self._record_completion(svc, "sess-1", q2["id"])
        self._record_completion(svc, "sess-1", q3["id"])
        svc.accept_session_link_suggestion("sess-1")

        result = svc.get_playlist_analytics(pl["id"])
        assert result["matched_sessions"] == 1
        assert result["total_immediate_reward_ped"] == pytest.approx(3.0)
        assert result["total_bonus_reward_ped"] == pytest.approx(5.0)
        assert result["total_skill_reward_ped"] == pytest.approx(7.0)
        assert result["total_immediate_skill_reward_ped"] == pytest.approx(2.0)
        assert result["total_bonus_skill_reward_ped"] == pytest.approx(5.0)
        assert result["total_expected_immediate_reward_ped"] == pytest.approx(3.3)
        assert result["total_expected_bonus_reward_ped"] == pytest.approx(5.0)
        assert result["total_expected_reward_ped"] == pytest.approx(8.3)

    def test_get_all_playlist_analytics(self, svc_with_tables):
        svc, _ = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        svc.create_playlist({"name": "Loop1", "quest_ids": [q1["id"]]})
        svc.create_playlist({"name": "Loop2", "quest_ids": [q1["id"]]})

        self._create_session(svc, "sess-1")
        self._record_completion(svc, "sess-1", q1["id"])

        results = svc.get_all_playlist_analytics()
        assert len(results) == 2  # Both playlists returned
        assert all(r["matched_sessions"] == 0 for r in results)


class TestSessionLinkSuggestions:
    @pytest.fixture
    def svc_with_tables(self, tmp_path: Path):
        from backend.core.event_bus import EventBus

        db = AppDatabase(tmp_path / "test.db")
        db.conn.executescript("""
            CREATE TABLE IF NOT EXISTS tracking_sessions (
                id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
                is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER,
                armour_cost REAL DEFAULT 0, heal_cost REAL DEFAULT 0);
        """)
        db.conn.commit()
        svc = QuestService(db, EventBus())
        return svc

    def test_single_quest_suggests_quest(self, svc_with_tables):
        svc = svc_with_tables
        q = svc.create_quest({"name": "Solo"})
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q["id"]),
        )
        svc._conn.commit()

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "quest"
        assert suggestion["quest_id"] == q["id"]
        assert suggestion["quest_name"] == "Solo"

    def test_exact_playlist_suggests_playlist(self, svc_with_tables):
        svc = svc_with_tables
        q1 = svc.create_quest({"name": "A"})
        q2 = svc.create_quest({"name": "B"})
        q3 = svc.create_quest({"name": "C"})
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q1["id"]),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q2["id"]),
        )
        svc._conn.commit()

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "playlist"
        assert suggestion["playlist_id"] == pl["id"]
        assert suggestion["playlist_name"] == "Loop"

    def test_immediate_plus_long_horizon_still_suggests_playlist(self, svc_with_tables):
        svc = svc_with_tables
        q1 = svc.create_quest({"name": "A"})
        q2 = svc.create_quest({"name": "B"})
        q3 = svc.create_quest({"name": "C"})
        pl = svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                    {"quest_id": q3["id"], "group_type": "long_horizon"},
                ],
            }
        )
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        for quest_id in (q1["id"], q2["id"], q3["id"]):
            svc._conn.execute(
                "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
                ("sess-1", quest_id),
            )
        svc._conn.commit()

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "playlist"
        assert suggestion["playlist_id"] == pl["id"]

    def test_multiple_matching_playlists_are_ambiguous(self, svc_with_tables):
        svc = svc_with_tables
        q1 = svc.create_quest({"name": "A"})
        q2 = svc.create_quest({"name": "B"})
        svc.create_playlist({"name": "Loop1", "quest_ids": [q1["id"], q2["id"]]})
        svc.create_playlist({"name": "Loop2", "quest_ids": [q1["id"], q2["id"]]})
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q1["id"]),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q2["id"]),
        )
        svc._conn.commit()

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "none"
        assert suggestion["reason"] == "ambiguous_playlist"

    def test_multiple_non_playlist_completions_are_unclean(self, svc_with_tables):
        svc = svc_with_tables
        q1 = svc.create_quest({"name": "A"})
        q2 = svc.create_quest({"name": "B"})
        q3 = svc.create_quest({"name": "C"})
        svc.create_playlist(
            {
                "name": "Loop",
                "items": [
                    {"quest_id": q1["id"], "group_type": "immediate"},
                    {"quest_id": q2["id"], "group_type": "immediate"},
                ],
            }
        )
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q1["id"]),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q2["id"]),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q3["id"]),
        )
        svc._conn.commit()

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "none"
        assert suggestion["reason"] == "unclean"

    def test_decline_persists_and_blocks_future_prompt(self, svc_with_tables):
        svc = svc_with_tables
        q = svc.create_quest({"name": "Solo"})
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q["id"]),
        )
        svc._conn.commit()

        svc.decline_session_link("sess-1")

        suggestion = svc.get_session_link_suggestion("sess-1")
        assert suggestion["suggestion_type"] == "none"
        assert suggestion["reason"] == "declined"


class TestCuratedQuestAnalytics:
    @pytest.fixture
    def svc_with_tables(self, tmp_path: Path):
        from backend.core.event_bus import EventBus

        db = AppDatabase(tmp_path / "test.db")
        db.conn.executescript("""
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
        """)
        db.conn.commit()
        svc = QuestService(db, EventBus())
        return svc

    def test_quest_analytics_ignore_raw_completions_until_curated(
        self, svc_with_tables
    ):
        svc = svc_with_tables
        q = svc.create_quest(
            {
                "name": "Solo",
                "reward_ped": 1.5,
                "expected_reward_markup_percent": 150.0,
            }
        )
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q["id"]),
        )
        svc._conn.commit()

        assert svc.get_quest_analytics() == []

        svc.accept_session_link_suggestion("sess-1")
        rows = svc.get_quest_analytics()
        assert len(rows) == 1
        assert rows[0]["quest_id"] == q["id"]
        assert rows[0]["linked_sessions"] == 1
        assert rows[0]["expected_reward_markup_percent"] == 150.0
        assert rows[0]["total_expected_reward_ped"] == pytest.approx(2.25)

    def test_playlist_analytics_ignore_raw_completions_until_curated(
        self, svc_with_tables
    ):
        svc = svc_with_tables
        q1 = svc.create_quest({"name": "A", "reward_ped": 1.0})
        q2 = svc.create_quest({"name": "B", "reward_ped": 2.0})
        pl = svc.create_playlist({"name": "Loop", "quest_ids": [q1["id"], q2["id"]]})
        svc._conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) VALUES (?, ?, ?, 0)",
            ("sess-1", 1000.0, 2000.0),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q1["id"]),
        )
        svc._conn.execute(
            "INSERT INTO session_quest_completions (session_id, quest_id) VALUES (?, ?)",
            ("sess-1", q2["id"]),
        )
        svc._conn.commit()

        before = svc.get_playlist_analytics(pl["id"])
        assert before["matched_sessions"] == 0

        svc.accept_session_link_suggestion("sess-1")
        after = svc.get_playlist_analytics(pl["id"])
        assert after["matched_sessions"] == 1


class TestMobNames:
    def test_get_all_mob_names(self, quest_service: QuestService):
        quest_service.create_quest({"name": "A", "mobs": ["Atrox", "Foul"]})
        quest_service.create_quest({"name": "B", "mobs": ["Foul", "Snablesnot"]})
        mobs = quest_service.get_all_mob_names()
        assert mobs == ["Atrox", "Foul", "Snablesnot"]

    def test_mob_names_exclude_deleted(self, quest_service: QuestService):
        q = quest_service.create_quest({"name": "A", "mobs": ["Atrox"]})
        quest_service.delete_quest(q["id"])
        assert quest_service.get_all_mob_names() == []
