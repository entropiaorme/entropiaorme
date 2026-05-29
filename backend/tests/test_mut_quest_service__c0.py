"""Mutation-hardening tests for backend.services.quest_service.

Cluster quest_service__c0 - scope: _normalize_quest_name, QuestService.__init__,
_on_session_start, _on_mission_received, get_quests, create_quest, update_quest.

Every test imports the real production module and drives the mutated
line/behaviour through the public API (or a thin direct call where the public
surface routes through the line under test), asserting the exact behaviour a
mutation would break.
"""

from pathlib import Path

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
)
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService, _normalize_quest_name


@pytest.fixture
def db(tmp_path: Path) -> AppDatabase:
    return AppDatabase(tmp_path / "test.db")


@pytest.fixture
def quest_service(db: AppDatabase) -> QuestService:
    return QuestService(db)


# ── _normalize_quest_name ─────────────────────────────────────────────────


def test_normalize_nfkd_to_ascii_lower_strip():
    # NFKD decomposition + ascii-ignore drops the accent; result lowercased
    # and stripped. Pins the normalize form ("NFKD"), the encode/decode codec
    # ("ascii"), the error mode ("ignore"), .strip() and .lower().
    assert _normalize_quest_name("  Crème Brûlée  ") == "creme brulee"


def test_normalize_strips_whitespace_both_ends():
    assert _normalize_quest_name("\t Atrox Hunt \n") == "atrox hunt"


def test_normalize_lowercases():
    assert _normalize_quest_name("ATROX") == "atrox"


def test_normalize_drops_nonascii_entirely():
    # A purely non-ascii string normalises to empty after ascii-ignore.
    assert _normalize_quest_name("日本語") == ""


def test_normalize_keeps_internal_spacing_but_lowercases():
    assert _normalize_quest_name("Iron Challenge: ATROX") == "iron challenge: atrox"


# ── _normalize_quest_name routed through match_quest_by_mission_name ──────


def test_match_quest_uses_normalized_exact_match(quest_service: QuestService):
    quest_service.create_quest({"name": "Café Run"})
    # Mission name carries an accent + different case + surrounding noise.
    matched = quest_service.match_quest_by_mission_name("  CAFE RUN  ")
    assert matched is not None
    assert matched["name"] == "Café Run"


# ── QuestService.__init__ + event subscriptions ───────────────────────────


def test_init_sets_current_session_none(quest_service: QuestService):
    assert quest_service._current_session_id is None


def test_init_no_event_bus_does_not_subscribe(db: AppDatabase):
    # With event_bus=None the constructor must not subscribe; passing a bus
    # later is the only way to wire callbacks. Confirms the truthiness guard.
    svc = QuestService(db, event_bus=None)
    assert svc._current_session_id is None


def test_init_with_event_bus_wires_session_start(db: AppDatabase):
    bus = EventBus()
    svc = QuestService(db, event_bus=bus)
    bus.publish(EVENT_SESSION_STARTED, {"session_id": "abcdef0123456789"})
    assert svc._current_session_id == "abcdef0123456789"


def test_init_with_event_bus_wires_mission_received(db: AppDatabase):
    bus = EventBus()
    svc = QuestService(db, event_bus=bus)
    svc.create_quest({"name": "Atrox Hunt"})
    bus.publish(EVENT_MISSION_RECEIVED, {"mission_name": "Atrox Hunt"})
    quests = svc.get_quests()
    # The mission-received handler started the matched quest.
    assert quests[0]["started_at"] is not None


# ── _on_session_start ─────────────────────────────────────────────────────


def test_on_session_start_stores_session_id(quest_service: QuestService):
    quest_service._on_session_start({"session_id": "sess-1234-abcd"})
    assert quest_service._current_session_id == "sess-1234-abcd"


def test_on_session_start_reads_session_id_key(quest_service: QuestService):
    # The handler must read the "session_id" key specifically; a wrong key
    # would leave it None.
    quest_service._on_session_start({"session_id": "the-right-one", "other": "x"})
    assert quest_service._current_session_id == "the-right-one"


def test_on_session_start_missing_session_id_is_none(quest_service: QuestService):
    # data.get("session_id") with no such key -> None; the log line must use
    # the "?" fallback rather than slicing None (which would raise).
    quest_service._on_session_start({})
    assert quest_service._current_session_id is None


def test_on_session_start_none_session_id_no_crash(quest_service: QuestService):
    # Explicit None session id: the ternary guard picks "?" so the [:8] slice
    # is never applied to None. If the guard inverted, this raises TypeError.
    quest_service._on_session_start({"session_id": None})
    assert quest_service._current_session_id is None


def test_on_session_start_short_id_logs_without_error(quest_service: QuestService):
    # A truthy id shorter than 8 chars must take the slice branch and not raise.
    quest_service._on_session_start({"session_id": "abc"})
    assert quest_service._current_session_id == "abc"


# ── _on_mission_received ──────────────────────────────────────────────────


def test_on_mission_received_starts_matching_quest(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Kill Atrox"})
    quest_service._on_mission_received({"mission_name": "Kill Atrox"})
    started = quest_service.get_quest(q["id"])
    assert started is not None
    assert started["started_at"] is not None


def test_on_mission_received_empty_name_is_noop(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Kill Atrox"})
    # Empty mission name is falsy -> the guard must skip start_quest_from_mission.
    quest_service._on_mission_received({"mission_name": ""})
    fetched = quest_service.get_quest(q["id"])
    assert fetched is not None
    assert fetched["started_at"] is None


def test_on_mission_received_missing_name_is_noop(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Kill Atrox"})
    # Missing key -> default "" -> falsy -> noop. A wrong default or inverted
    # guard would attempt a match/start.
    quest_service._on_mission_received({})
    fetched = quest_service.get_quest(q["id"])
    assert fetched is not None
    assert fetched["started_at"] is None


def test_on_mission_received_reads_mission_name_key(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Kill Atrox"})
    # Right key must be read; a wrong key would yield "" and skip the start.
    quest_service._on_mission_received({"mission_name": "Kill Atrox", "x": "ignore"})
    fetched = quest_service.get_quest(q["id"])
    assert fetched is not None
    assert fetched["started_at"] is not None


# ── get_quests ────────────────────────────────────────────────────────────


def test_get_quests_active_only_excludes_deleted(quest_service: QuestService):
    keep = quest_service.create_quest({"name": "Keep"})
    drop = quest_service.create_quest({"name": "Drop"})
    quest_service.delete_quest(drop["id"])
    active = quest_service.get_quests(active_only=True)
    names = {q["name"] for q in active}
    assert names == {"Keep"}
    assert keep["id"] in {q["id"] for q in active}


def test_get_quests_all_includes_deleted(quest_service: QuestService):
    quest_service.create_quest({"name": "Keep"})
    drop = quest_service.create_quest({"name": "Drop"})
    quest_service.delete_quest(drop["id"])
    allq = quest_service.get_quests(active_only=False)
    assert {q["name"] for q in allq} == {"Keep", "Drop"}


def test_get_quests_default_is_active_only(quest_service: QuestService):
    drop = quest_service.create_quest({"name": "Drop"})
    quest_service.delete_quest(drop["id"])
    # Default argument must be active_only=True.
    assert quest_service.get_quests() == []


def test_get_quests_ordered_by_created_at_asc(quest_service: QuestService):
    quest_service.create_quest({"name": "First"})
    quest_service.create_quest({"name": "Second"})
    quest_service.create_quest({"name": "Third"})
    names = [q["name"] for q in quest_service.get_quests()]
    assert names == ["First", "Second", "Third"]


def test_get_quests_enriches_mobs(quest_service: QuestService):
    quest_service.create_quest({"name": "Hunt", "mobs": ["Zebra", "Atrox"]})
    q = quest_service.get_quests()[0]
    # mobs enriched and sorted alphabetically by the helper.
    assert q["mobs"] == ["Atrox", "Zebra"]


def test_get_quests_enriches_playlist_ids(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Hunt"})
    pl = quest_service.create_playlist({"name": "PL", "quest_ids": [q["id"]]})
    fetched = quest_service.get_quests()[0]
    assert fetched["playlist_ids"] == [pl["id"]]


def test_get_quests_empty_when_no_quests(quest_service: QuestService):
    assert quest_service.get_quests() == []
    assert quest_service.get_quests(active_only=False) == []


# ── create_quest ──────────────────────────────────────────────────────────


def test_create_quest_planet_default_calypso(quest_service: QuestService):
    q = quest_service.create_quest({"name": "NoPlanet"})
    assert q["planet"] == "Calypso"


def test_create_quest_explicit_planet_overrides_default(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "planet": "Arkadia"})
    assert q["planet"] == "Arkadia"


def test_create_quest_persists_all_scalar_fields(quest_service: QuestService):
    q = quest_service.create_quest(
        {
            "name": "Full",
            "waypoint": "/wp here",
            "cooldown_hours": 12,
            "reward_ped": 7.5,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 120.0,
            "notes": "n",
            "chain_name": "chain",
            "chain_position": 2,
            "chain_total": 5,
            "category": "cat",
            "reward_description": "rd",
        }
    )
    assert q["name"] == "Full"
    assert q["waypoint"] == "/wp here"
    assert q["cooldown_hours"] == 12
    assert q["reward_ped"] == 7.5
    assert q["reward_is_skill"] == 0
    assert q["expected_reward_markup_percent"] == 120.0
    assert q["notes"] == "n"
    assert q["chain_name"] == "chain"
    assert q["chain_position"] == 2
    assert q["chain_total"] == 5
    assert q["category"] == "cat"
    assert q["reward_description"] == "rd"


def test_create_quest_reward_is_skill_true_stored_as_one(quest_service: QuestService):
    q = quest_service.create_quest(
        {"name": "Skill", "reward_ped": 3.0, "reward_is_skill": True}
    )
    assert q["reward_is_skill"] == 1


def test_create_quest_reward_is_skill_false_stored_as_zero(quest_service: QuestService):
    q = quest_service.create_quest(
        {"name": "Liquid", "reward_ped": 3.0, "reward_is_skill": False}
    )
    assert q["reward_is_skill"] == 0


def test_create_quest_reward_is_skill_default_zero(quest_service: QuestService):
    q = quest_service.create_quest({"name": "NoSkillFlag"})
    assert q["reward_is_skill"] == 0


def test_create_quest_skill_reward_nulls_expected_markup(quest_service: QuestService):
    # reward_is_skill truthy -> _normalize_expected_reward_markup returns None.
    q = quest_service.create_quest(
        {
            "name": "SkillReward",
            "reward_ped": 10.0,
            "reward_is_skill": True,
            "expected_reward_markup_percent": 150.0,
        }
    )
    assert q["expected_reward_markup_percent"] is None


def test_create_quest_liquid_reward_keeps_expected_markup(quest_service: QuestService):
    q = quest_service.create_quest(
        {
            "name": "LiquidReward",
            "reward_ped": 10.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 150.0,
        }
    )
    assert q["expected_reward_markup_percent"] == 150.0


def test_create_quest_sets_mobs(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "mobs": ["Atrox", "Foul"]})
    assert q["mobs"] == ["Atrox", "Foul"]


def test_create_quest_no_mobs_means_empty(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    assert q["mobs"] == []


def test_create_quest_empty_mobs_list_means_empty(quest_service: QuestService):
    # mobs=[] is falsy -> the guard must skip _set_quest_mobs; still empty.
    q = quest_service.create_quest({"name": "Q", "mobs": []})
    assert q["mobs"] == []


def test_create_quest_returns_fetched_row_not_none(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    assert q is not None
    # The returned dict is the enriched get_quest result.
    assert "playlist_ids" in q
    assert "mobs" in q
    assert q["id"] is not None


def test_create_quest_persists_to_db(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Persisted"})
    refetched = quest_service.get_quest(q["id"])
    assert refetched is not None
    assert refetched["name"] == "Persisted"


def test_create_quest_optional_fields_default_none(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Bare"})
    assert q["waypoint"] is None
    assert q["cooldown_hours"] is None
    assert q["reward_ped"] is None
    assert q["notes"] is None
    assert q["chain_name"] is None
    assert q["chain_position"] is None
    assert q["chain_total"] is None
    assert q["category"] is None
    assert q["reward_description"] is None


# ── update_quest ──────────────────────────────────────────────────────────


def test_update_quest_not_found_returns_none(quest_service: QuestService):
    assert quest_service.update_quest(99999, {"name": "X"}) is None


def test_update_quest_changes_name(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Old"})
    updated = quest_service.update_quest(q["id"], {"name": "New"})
    assert updated is not None
    assert updated["name"] == "New"


def test_update_quest_only_allowed_fields(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    # "id" and unknown keys are not in the allowed set; they must be ignored,
    # not written. The update should still succeed and not move the row.
    updated = quest_service.update_quest(q["id"], {"bogus": "nope", "planet": "Ark"})
    assert updated is not None
    assert updated["id"] == q["id"]
    assert updated["planet"] == "Ark"


def test_update_quest_reward_is_skill_true_to_one(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "reward_ped": 5.0})
    updated = quest_service.update_quest(q["id"], {"reward_is_skill": True})
    assert updated is not None
    assert updated["reward_is_skill"] == 1


def test_update_quest_reward_is_skill_false_to_zero(quest_service: QuestService):
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "reward_is_skill": True}
    )
    updated = quest_service.update_quest(q["id"], {"reward_is_skill": False})
    assert updated is not None
    assert updated["reward_is_skill"] == 0


def test_update_quest_skill_reward_clears_expected_markup(quest_service: QuestService):
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "expected_reward_markup_percent": 140.0}
    )
    assert q["expected_reward_markup_percent"] == 140.0
    updated = quest_service.update_quest(q["id"], {"reward_is_skill": True})
    assert updated is not None
    assert updated["expected_reward_markup_percent"] is None


def test_update_quest_change_reward_ped_recomputes_markup(quest_service: QuestService):
    # Start liquid with markup, then change reward_ped only. The markup recompute
    # branch fires (because reward_ped is in data) and pulls reward_is_skill /
    # expected_markup from existing, keeping the markup non-null.
    q = quest_service.create_quest(
        {
            "name": "Q",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 130.0,
        }
    )
    updated = quest_service.update_quest(q["id"], {"reward_ped": 8.0})
    assert updated is not None
    assert updated["reward_ped"] == 8.0
    assert updated["expected_reward_markup_percent"] == 130.0


def test_update_quest_set_reward_ped_to_zero_nulls_markup(quest_service: QuestService):
    # reward_ped <= 0 -> normalize returns None even with markup present.
    q = quest_service.create_quest(
        {
            "name": "Q",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 130.0,
        }
    )
    updated = quest_service.update_quest(q["id"], {"reward_ped": 0})
    assert updated is not None
    assert updated["expected_reward_markup_percent"] is None


def test_update_quest_change_markup_only(quest_service: QuestService):
    q = quest_service.create_quest(
        {
            "name": "Q",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 130.0,
        }
    )
    updated = quest_service.update_quest(
        q["id"], {"expected_reward_markup_percent": 200.0}
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] == 200.0


def test_update_quest_no_reward_keys_leaves_markup_untouched(
    quest_service: QuestService,
):
    # Updating only an unrelated field must NOT trigger the markup recompute
    # branch; the stored markup stays exactly as created.
    q = quest_service.create_quest(
        {
            "name": "Q",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 130.0,
        }
    )
    updated = quest_service.update_quest(q["id"], {"notes": "changed"})
    assert updated is not None
    assert updated["notes"] == "changed"
    assert updated["expected_reward_markup_percent"] == 130.0


def test_update_quest_recompute_uses_existing_reward_is_skill(
    quest_service: QuestService,
):
    # Quest is skill-reward. Update only expected_reward_markup_percent: the
    # recompute must read reward_is_skill from existing (truthy) and null the
    # markup despite the supplied value.
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "reward_is_skill": True}
    )
    updated = quest_service.update_quest(
        q["id"], {"expected_reward_markup_percent": 175.0}
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] is None


def test_update_quest_recompute_uses_new_reward_ped_over_existing(
    quest_service: QuestService,
):
    # Provide both reward_ped (new) and expected markup; recompute must use the
    # NEW reward_ped (8 > 0) so markup is retained.
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "reward_is_skill": False}
    )
    updated = quest_service.update_quest(
        q["id"], {"reward_ped": 8.0, "expected_reward_markup_percent": 110.0}
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] == 110.0


def test_update_quest_recompute_skill_via_new_value_nulls_markup(
    quest_service: QuestService,
):
    # New reward_is_skill=True with new markup -> normalize returns None.
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "reward_is_skill": False}
    )
    updated = quest_service.update_quest(
        q["id"],
        {"reward_is_skill": True, "expected_reward_markup_percent": 145.0},
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] is None


def test_update_quest_sets_mobs(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "mobs": ["Atrox"]})
    updated = quest_service.update_quest(q["id"], {"mobs": ["Foul", "Snable"]})
    assert updated is not None
    assert updated["mobs"] == ["Foul", "Snable"]


def test_update_quest_empty_mobs_clears(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "mobs": ["Atrox"]})
    # "mobs" key present (even empty) -> _set_quest_mobs called -> cleared.
    updated = quest_service.update_quest(q["id"], {"mobs": []})
    assert updated is not None
    assert updated["mobs"] == []


def test_update_quest_no_mobs_key_keeps_mobs(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "mobs": ["Atrox"]})
    # No "mobs" key -> mobs untouched.
    updated = quest_service.update_quest(q["id"], {"name": "Renamed"})
    assert updated is not None
    assert updated["mobs"] == ["Atrox"]


def test_update_quest_returns_refetched_quest(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    updated = quest_service.update_quest(q["id"], {"planet": "Next"})
    # Returned value is the fresh enriched get_quest, reflecting the write.
    assert updated is not None
    assert updated["planet"] == "Next"
    refetched = quest_service.get_quest(q["id"])
    assert refetched is not None
    assert refetched["planet"] == "Next"


def test_update_quest_empty_data_returns_unchanged_quest(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "planet": "Calypso"})
    # No allowed keys, no reward keys, no mobs -> no UPDATE issued, but the
    # method still returns the (unchanged) quest, not None.
    updated = quest_service.update_quest(q["id"], {})
    assert updated is not None
    assert updated["name"] == "Q"
    assert updated["planet"] == "Calypso"


def test_update_quest_persists_multiple_fields(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    updated = quest_service.update_quest(
        q["id"], {"name": "N", "planet": "P", "category": "C", "notes": "X"}
    )
    assert updated is not None
    assert updated["name"] == "N"
    assert updated["planet"] == "P"
    assert updated["category"] == "C"
    assert updated["notes"] == "X"


# ── update_quest: every allowed field is individually honoured ─────────────
# The `allowed` set is a literal set of column-name strings. Mutating any one
# string (XX-wrap or upper-case) drops that field from the recognised set, so
# the update is silently ignored. Each test below changes exactly one field
# from a known value and asserts the new value persisted - pinning the exact
# spelling of every allowed-set entry.


def test_update_quest_field_waypoint(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "waypoint": "/wp old"})
    updated = quest_service.update_quest(q["id"], {"waypoint": "/wp NEW"})
    assert updated is not None
    assert updated["waypoint"] == "/wp NEW"


def test_update_quest_field_cooldown_hours(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "cooldown_hours": 10})
    updated = quest_service.update_quest(q["id"], {"cooldown_hours": 24})
    assert updated is not None
    assert updated["cooldown_hours"] == 24


def test_update_quest_field_chain_name(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "chain_name": "old"})
    updated = quest_service.update_quest(q["id"], {"chain_name": "NEWCHAIN"})
    assert updated is not None
    assert updated["chain_name"] == "NEWCHAIN"


def test_update_quest_field_chain_position(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "chain_position": 1})
    updated = quest_service.update_quest(q["id"], {"chain_position": 7})
    assert updated is not None
    assert updated["chain_position"] == 7


def test_update_quest_field_chain_total(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "chain_total": 3})
    updated = quest_service.update_quest(q["id"], {"chain_total": 9})
    assert updated is not None
    assert updated["chain_total"] == 9


def test_update_quest_field_reward_description(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "reward_description": "old"})
    updated = quest_service.update_quest(q["id"], {"reward_description": "NEWDESC"})
    assert updated is not None
    assert updated["reward_description"] == "NEWDESC"


def test_update_quest_field_reward_ped(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "reward_ped": 1.0})
    updated = quest_service.update_quest(q["id"], {"reward_ped": 42.0})
    assert updated is not None
    assert updated["reward_ped"] == 42.0


def test_update_quest_field_name(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q"})
    updated = quest_service.update_quest(q["id"], {"name": "RenamedField"})
    assert updated is not None
    assert updated["name"] == "RenamedField"


def test_update_quest_field_planet(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "planet": "Calypso"})
    updated = quest_service.update_quest(q["id"], {"planet": "Arkadia"})
    assert updated is not None
    assert updated["planet"] == "Arkadia"


def test_update_quest_field_notes(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "notes": "old"})
    updated = quest_service.update_quest(q["id"], {"notes": "NEWNOTES"})
    assert updated is not None
    assert updated["notes"] == "NEWNOTES"


def test_update_quest_field_category(quest_service: QuestService):
    q = quest_service.create_quest({"name": "Q", "category": "old"})
    updated = quest_service.update_quest(q["id"], {"category": "NEWCAT"})
    assert updated is not None
    assert updated["category"] == "NEWCAT"


def test_update_quest_field_expected_markup_persists(quest_service: QuestService):
    # The allowed-set entry "expected_reward_markup_percent" must be honoured.
    # With a liquid reward present, updating only the markup persists it.
    q = quest_service.create_quest(
        {
            "name": "Q",
            "reward_ped": 5.0,
            "reward_is_skill": False,
            "expected_reward_markup_percent": 100.0,
        }
    )
    updated = quest_service.update_quest(
        q["id"], {"expected_reward_markup_percent": 250.0}
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] == 250.0


def test_update_quest_reward_is_skill_truthy_non_one_coerced_to_one(
    quest_service: QuestService,
):
    # The `if key == "reward_is_skill"` branch coerces the value via
    # `1 if val else 0`. If that comparison string is corrupted the raw value
    # is stored instead. Feed a truthy, non-1 value (2) so a stored raw value
    # would differ from the coerced 1.
    q = quest_service.create_quest({"name": "Q", "reward_ped": 5.0})
    updated = quest_service.update_quest(q["id"], {"reward_is_skill": 2})
    assert updated is not None
    assert updated["reward_is_skill"] == 1


def test_update_quest_all_reward_keys_skill_recompute_nulls_markup(
    quest_service: QuestService,
):
    # Provide ALL THREE reward keys at once. The recompute trigger
    # `any(key in data for key in (...))` is True, so normalize runs and,
    # because reward_is_skill is truthy, nulls the markup. If the trigger were
    # inverted to `key not in data`, all three keys present -> any() False ->
    # recompute skipped -> the raw 140.0 from the allowed-loop would persist.
    q = quest_service.create_quest(
        {"name": "Q", "reward_ped": 5.0, "reward_is_skill": False}
    )
    updated = quest_service.update_quest(
        q["id"],
        {
            "reward_ped": 10.0,
            "reward_is_skill": True,
            "expected_reward_markup_percent": 140.0,
        },
    )
    assert updated is not None
    assert updated["expected_reward_markup_percent"] is None
    assert updated["reward_is_skill"] == 1
    assert updated["reward_ped"] == 10.0


# ── _on_mission_received truthy-default mutant ────────────────────────────


def test_on_mission_received_missing_name_does_not_start_phantom_quest(
    quest_service: QuestService,
):
    # The default for the absent "mission_name" key must be the EMPTY string
    # (falsy). A mutant changing it to a non-empty literal (e.g. "XXXX") would
    # be truthy and attempt to match/start a quest of that name. We seed a
    # quest literally named "XXXX" so the mutant would start it, while the real
    # falsy default leaves everything unstarted.
    phantom = quest_service.create_quest({"name": "XXXX"})
    quest_service._on_mission_received({})  # no mission_name key
    fetched = quest_service.get_quest(phantom["id"])
    assert fetched is not None
    assert fetched["started_at"] is None


# ── _on_session_start log line (caplog) ───────────────────────────────────
# The session-id is stored on the preceding line; the log.info call only
# affects the emitted record. These mutants alter the format string, the
# message argument, or the [:8] truncation. Asserting the rendered record text
# pins them. log.info("Quest service tracking session %s", id[:8]).


def test_on_session_start_log_message_text_and_truncation(
    quest_service: QuestService, caplog
):
    import logging

    with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
        quest_service._on_session_start({"session_id": "abcdef0123456789"})
    msgs = [r.getMessage() for r in caplog.records]
    # Exact rendered text: fixed prefix + 8-char truncation of the id.
    assert "Quest service tracking session abcdef01" in msgs
    # The 9th char must NOT appear (guards the [:8] -> [:9] mutant).
    assert "Quest service tracking session abcdef012" not in msgs


def test_on_session_start_log_unknown_session(quest_service: QuestService, caplog):
    import logging

    with caplog.at_level(logging.INFO, logger="backend.services.quest_service"):
        quest_service._on_session_start({})
    msgs = [r.getMessage() for r in caplog.records]
    # Falsy id -> "?" placeholder rendered into the fixed message.
    assert "Quest service tracking session ?" in msgs
