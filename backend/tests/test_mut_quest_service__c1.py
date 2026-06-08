"""Mutation-hardening tests for QuestService cluster quest_service__c1.

Targets: delete_quest, start_quest, complete_quest, cancel_quest,
get_session_link_suggestion, accept_session_link_suggestion.

Each test exercises a specific mutated line/branch and asserts the exact
behaviour the mutation would break, using the real backend service against
an on-disk SQLite database.
"""

import logging
from datetime import datetime
from pathlib import Path

import pytest

from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService
from backend.testing.clock import Clock


@pytest.fixture
def svc(tmp_path: Path) -> QuestService:
    db = AppDatabase(tmp_path / "test.db")
    return QuestService(db)


# ── helpers ────────────────────────────────────────────────────────────────


def _ledger_rows(svc: QuestService) -> list:
    return svc._conn.execute(
        "SELECT id, date, description, amount, tag, type FROM ledger_entries"
    ).fetchall()


def _claim_rows(svc: QuestService) -> list:
    return svc._conn.execute(
        "SELECT id, quest_id, quest_name, ped_value FROM quest_claims"
    ).fetchall()


def _completion_rows(svc: QuestService, session_id: str) -> list:
    return svc._conn.execute(
        "SELECT quest_id, completed_at FROM session_quest_completions "
        "WHERE session_id = ?",
        (session_id,),
    ).fetchall()


def _rendered(caplog, substring: str) -> str:
    """Return the fully %-rendered message of the captured record whose raw
    template contains ``substring``.

    Calling getMessage() forces the %-formatting; a mutant that nulls the
    template or drops a positional arg raises here and the assertion fails.
    """
    for rec in caplog.records:
        if isinstance(rec.msg, str) and substring in rec.msg:
            return rec.getMessage()
    raise AssertionError(f"no log record whose template contains {substring!r}")


# ── delete_quest ─────────────────────────────────────────────────────────────


class TestDeleteQuest:
    def test_soft_delete_clears_active_and_playlist_membership(self, svc: QuestService):
        q = svc.create_quest({"name": "DelMe"})
        pl = svc.create_playlist({"name": "PL", "quest_ids": [q["id"]]})
        # quest is in the playlist before deletion
        playlist_before = svc.get_playlist(pl["id"])
        assert playlist_before is not None
        assert q["id"] in playlist_before["quest_ids"]

        assert svc.delete_quest(q["id"]) is True
        # is_active flipped to 0 (UPDATE actually applied to the matching row)
        assert svc.get_quests(active_only=True) == []
        assert len(svc.get_quests(active_only=False)) == 1
        # playlist membership removed (the DELETE FROM quest_playlist_items ran)
        playlist_after = svc.get_playlist(pl["id"])
        assert playlist_after is not None
        assert playlist_after["quest_ids"] == []

    def test_delete_already_inactive_returns_false_and_keeps_playlist(
        self, svc: QuestService
    ):
        q = svc.create_quest({"name": "DelMe"})
        svc.create_playlist({"name": "PL", "quest_ids": [q["id"]]})
        assert svc.delete_quest(q["id"]) is True
        # second delete: WHERE is_active = 1 matches nothing -> rowcount 0
        assert svc.delete_quest(q["id"]) is False


# ── start_quest ──────────────────────────────────────────────────────────────


class TestStartQuest:
    def test_start_sets_started_at(self, svc: QuestService):
        q = svc.create_quest({"name": "S"})
        assert q["started_at"] is None
        started = svc.start_quest(q["id"])
        assert started is not None
        assert started["started_at"] is not None

    def test_start_inactive_quest_returns_none(self, svc: QuestService):
        """rowcount must be strictly > 0; a soft-deleted quest matches no row
        (WHERE is_active = 1), so start must return None rather than the row."""
        q = svc.create_quest({"name": "Gone"})
        assert svc.delete_quest(q["id"]) is True
        # UPDATE ... WHERE is_active = 1 affects 0 rows -> must be None.
        assert svc.start_quest(q["id"]) is None
        # and started_at on the (inactive) row stays NULL
        inactive_quest = svc.get_quest(q["id"])
        assert inactive_quest is not None
        assert inactive_quest["started_at"] is None


# ── complete_quest ───────────────────────────────────────────────────────────


class TestCompleteQuest:
    def test_complete_clears_started_at(self, svc: QuestService):
        q = svc.create_quest({"name": "C"})
        svc.start_quest(q["id"])
        started_quest = svc.get_quest(q["id"])
        assert started_quest is not None
        assert started_quest["started_at"] is not None
        done = svc.complete_quest(q["id"])
        assert done is not None
        assert done["started_at"] is None

    def test_small_positive_reward_creates_ledger_entry(self, svc: QuestService):
        """reward_ped just above 0 (and at/below 1) must still book a ledger
        entry: the threshold is > 0, not > 1."""
        q = svc.create_quest({"name": "Tiny", "reward_ped": 0.5})
        svc.complete_quest(q["id"])
        rows = _ledger_rows(svc)
        assert len(rows) == 1
        assert rows[0]["amount"] == 0.5
        assert rows[0]["description"] == "Quest: Tiny"
        assert rows[0]["tag"] == "quest_reward"
        assert rows[0]["type"] == "markup"

    def test_ledger_date_is_utc_isoformat(self, svc: QuestService):
        """date_str is built with tz=UTC, so the stored ISO string carries a
        +00:00 offset; a naive (tz=None / missing) datetime would not."""
        q = svc.create_quest({"name": "UtcQuest", "reward_ped": 2.0})
        svc.complete_quest(q["id"])
        rows = _ledger_rows(svc)
        assert len(rows) == 1
        assert rows[0]["date"].endswith("+00:00")

    def test_skill_reward_creates_quest_claim_not_ledger(self, svc: QuestService):
        q = svc.create_quest(
            {"name": "SkillQ", "reward_ped": 3.0, "reward_is_skill": True}
        )
        svc.complete_quest(q["id"])
        claims = _claim_rows(svc)
        assert len(claims) == 1
        assert claims[0]["quest_name"] == "SkillQ"
        assert claims[0]["ped_value"] == 3.0
        assert _ledger_rows(svc) == []

    def test_completion_uses_captured_now_not_a_fresh_clock_read(
        self, svc: QuestService
    ):
        """complete_quest captures the clock's now once and threads that
        SAME timestamp into _record_session_completion. A mutant that passes
        None there forces _record_session_completion to read the clock again,
        recording a later, different timestamp."""
        q = svc.create_quest({"name": "Clock"})  # no reward -> single clock read
        svc._current_session_id = "sessX"

        class _TickingClock(Clock):
            """Each ``now()`` read returns the next queued epoch instant."""

            def __init__(self, epochs):
                self._epochs = iter(epochs)

            def now(self) -> datetime:
                return datetime.fromtimestamp(next(self._epochs))

            def monotonic(self) -> float:
                return 0.0

        # Epochs sit comfortably after 1970 so the naive fromtimestamp
        # round-trip is valid in any host timezone (Windows rejects
        # pre-epoch local instants).
        svc._clock = _TickingClock([1_000_000.0, 2_000_000.0, 3_000_000.0])

        svc.complete_quest(q["id"])
        rows = _completion_rows(svc, "sessX")
        assert len(rows) == 1
        # `now` was the first read (1_000_000.0); a re-read would be
        # 2_000_000.0.
        assert rows[0]["completed_at"] == 1_000_000.0

    def test_skill_claim_log_message_is_exact(self, svc: QuestService, caplog):
        q = svc.create_quest(
            {"name": "Logged", "reward_ped": 4.0, "reward_is_skill": True}
        )
        with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
            svc.complete_quest(q["id"])
        assert (
            _rendered(caplog, "quest claim for")
            == "Auto-created quest claim for 'Logged': 4.00 PES"
        )

    def test_ledger_log_message_is_exact(self, svc: QuestService, caplog):
        q = svc.create_quest({"name": "Ledgered", "reward_ped": 6.5})
        with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
            svc.complete_quest(q["id"])
        assert (
            _rendered(caplog, "ledger entry for quest")
            == "Auto-created ledger entry for quest 'Ledgered': 6.50 PED"
        )


# ── cancel_quest ─────────────────────────────────────────────────────────────


def _make_cooling_quest_with_reward(
    svc: QuestService, *, reward_ped: float, is_skill: bool, session_id: str
):
    """Create a quest with a cooldown, then complete it inside a session so it
    is currently cooling and has a recorded reward (ledger or claim)."""
    q = svc.create_quest(
        {
            "name": f"Cool-{reward_ped}-{is_skill}",
            "reward_ped": reward_ped,
            "reward_is_skill": is_skill,
            "cooldown_hours": 24,
        }
    )
    svc._current_session_id = session_id
    svc.complete_quest(q["id"])
    refreshed = svc.get_quest(q["id"])
    assert refreshed is not None
    assert svc._is_quest_cooling(refreshed) is True
    return q


class TestCancelQuest:
    def test_cancel_in_progress_clears_started_at(self, svc: QuestService):
        q = svc.create_quest({"name": "InProg"})
        svc.start_quest(q["id"])
        cancelled = svc.cancel_quest(q["id"])
        assert cancelled is not None
        assert cancelled["started_at"] is None

    def test_cancel_default_does_not_undo_reward(self, svc: QuestService):
        """undo_reward defaults to False: cancelling a cooling quest resets the
        cooldown but must leave the booked ledger entry intact."""
        q = _make_cooling_quest_with_reward(
            svc, reward_ped=5.0, is_skill=False, session_id="s1"
        )
        assert len(_ledger_rows(svc)) == 1
        # default call (no undo_reward) -> ledger entry must survive
        svc.cancel_quest(q["id"])
        assert len(_ledger_rows(svc)) == 1
        # cooldown was reset (completion row deleted)
        assert _completion_rows(svc, "s1") == []

    def test_cancel_undo_reward_removes_small_ledger_entry(self, svc: QuestService):
        """With undo_reward=True a reward in (0, 1] must still be removed: the
        guard is reward_ped > 0, not > 1."""
        q = _make_cooling_quest_with_reward(
            svc, reward_ped=0.5, is_skill=False, session_id="s2"
        )
        assert len(_ledger_rows(svc)) == 1
        svc.cancel_quest(q["id"], undo_reward=True)
        assert _ledger_rows(svc) == []

    def test_cancel_undo_reward_removes_ledger_and_logs(
        self, svc: QuestService, caplog
    ):
        q = _make_cooling_quest_with_reward(
            svc, reward_ped=7.0, is_skill=False, session_id="s3"
        )
        with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
            svc.cancel_quest(q["id"], undo_reward=True)
        assert _ledger_rows(svc) == []
        assert (
            _rendered(caplog, "quest reward ledger entry for")
            == "Removed latest quest reward ledger entry for "
            "'Cool-7.0-False': 7.00 PED"
        )

    def test_cancel_undo_reward_removes_claim_and_logs(self, svc: QuestService, caplog):
        q = _make_cooling_quest_with_reward(
            svc, reward_ped=8.0, is_skill=True, session_id="s4"
        )
        assert len(_claim_rows(svc)) == 1
        with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
            svc.cancel_quest(q["id"], undo_reward=True)
        assert _claim_rows(svc) == []
        assert (
            _rendered(caplog, "quest claim for")
            == "Removed latest quest claim for 'Cool-8.0-True': 8.00 PES"
        )


# ── get_session_link_suggestion ──────────────────────────────────────────────


def _complete_in_session(svc: QuestService, quest_id: int, session_id: str) -> None:
    svc._current_session_id = session_id
    svc.complete_quest(quest_id)
    svc._current_session_id = None


# Every key the suggestion contract must always expose.
_SUGGESTION_KEYS = {
    "suggestion_type",
    "reason",
    "quest_id",
    "quest_name",
    "playlist_id",
    "playlist_name",
}


class TestSessionLinkSuggestion:
    def test_no_completions(self, svc: QuestService):
        result = svc.get_session_link_suggestion("empty-sess")
        assert result["suggestion_type"] == "none"
        assert result["reason"] == "no_completions"
        # exact key contract (kills dict-key case-fold mutants on this branch)
        assert set(result) == _SUGGESTION_KEYS
        assert result["quest_name"] is None
        assert result["playlist_name"] is None

    def test_single_quest(self, svc: QuestService):
        q = svc.create_quest({"name": "Solo"})
        _complete_in_session(svc, q["id"], "sess-single")
        result = svc.get_session_link_suggestion("sess-single")
        assert result["suggestion_type"] == "quest"
        assert result["reason"] == "single_quest"
        assert result["quest_id"] == q["id"]
        assert result["quest_name"] == "Solo"
        assert set(result) == _SUGGESTION_KEYS
        assert result["playlist_name"] is None

    def test_exact_playlist(self, svc: QuestService):
        q1 = svc.create_quest({"name": "P1"})
        q2 = svc.create_quest({"name": "P2"})
        pl = svc.create_playlist({"name": "ExactPL", "quest_ids": [q1["id"], q2["id"]]})
        svc._current_session_id = "sess-pl"
        svc.complete_quest(q1["id"])
        svc.complete_quest(q2["id"])
        svc._current_session_id = None
        result = svc.get_session_link_suggestion("sess-pl")
        assert result["suggestion_type"] == "playlist"
        assert result["reason"] == "exact_playlist"
        assert result["playlist_id"] == pl["id"]
        assert result["playlist_name"] == "ExactPL"
        assert set(result) == _SUGGESTION_KEYS
        assert result["quest_name"] is None

    def test_unclean_multi_quest_no_playlist(self, svc: QuestService):
        q1 = svc.create_quest({"name": "U1"})
        q2 = svc.create_quest({"name": "U2"})
        svc._current_session_id = "sess-unclean"
        svc.complete_quest(q1["id"])
        svc.complete_quest(q2["id"])
        svc._current_session_id = None
        result = svc.get_session_link_suggestion("sess-unclean")
        assert result["suggestion_type"] == "none"
        assert result["reason"] == "unclean"
        # final-branch key contract (kills dict-key case-fold on the tail dict)
        assert set(result) == _SUGGESTION_KEYS
        assert result["quest_name"] is None
        assert result["playlist_name"] is None

    def test_existing_quest_link_resolves_names(self, svc: QuestService):
        """An already-linked session reports the real quest name (looked up by
        the row's quest_id), not None."""
        q = svc.create_quest({"name": "LinkedQuest"})
        _complete_in_session(svc, q["id"], "sess-elq")
        # persist a quest link for this session
        accepted = svc.accept_session_link_suggestion("sess-elq")
        assert accepted["suggestion_type"] == "quest"
        # now the suggestion reflects the existing link
        result = svc.get_session_link_suggestion("sess-elq")
        assert result["suggestion_type"] == "none"
        assert result["reason"] == "already_linked"
        assert result["quest_id"] == q["id"]
        assert result["quest_name"] == "LinkedQuest"
        assert set(result) == _SUGGESTION_KEYS

    def test_existing_playlist_link_resolves_names(self, svc: QuestService):
        q1 = svc.create_quest({"name": "PA"})
        q2 = svc.create_quest({"name": "PB"})
        pl = svc.create_playlist(
            {"name": "LinkedPL", "quest_ids": [q1["id"], q2["id"]]}
        )
        svc._current_session_id = "sess-epl"
        svc.complete_quest(q1["id"])
        svc.complete_quest(q2["id"])
        svc._current_session_id = None
        accepted = svc.accept_session_link_suggestion("sess-epl")
        assert accepted["suggestion_type"] == "playlist"
        result = svc.get_session_link_suggestion("sess-epl")
        assert result["suggestion_type"] == "none"
        assert result["reason"] == "already_linked"
        assert result["playlist_id"] == pl["id"]
        assert result["playlist_name"] == "LinkedPL"
        assert set(result) == _SUGGESTION_KEYS


# ── accept_session_link_suggestion ───────────────────────────────────────────


class TestAcceptSessionLinkSuggestion:
    def test_accept_with_no_completions_raises_descriptive_value_error(
        self, svc: QuestService
    ):
        with pytest.raises(ValueError, match="No linkable suggestion for session"):
            svc.accept_session_link_suggestion("nope-sess")
