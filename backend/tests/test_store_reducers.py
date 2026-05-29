"""Unit tests for the store-reducer state machines.

The apparatus tests in ``backend/tests/e2e/test_consistency_*.py`` pin
the snapshot to event-stream consistency property end-to-end. These
unit tests pin the per-event state transitions on the reducers that
the apparatus tests do not exercise in their current scenarios (in
particular, the ``QuestsReducer`` gating against pre-start and
post-stop ``mission_received`` events, which ``QuestService`` itself
ignores when no session is active).
"""

from __future__ import annotations

from backend.core.events import (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.testing.store_reducers import (
    CodexReducer,
    QuestsReducer,
    ScanReducer,
    TrackingReducer,
)


def test_quests_reducer_ignores_mission_received_before_session_starts() -> None:
    reducer = QuestsReducer()

    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Stray"}
    )

    assert reducer.state["mission_names_received"] == []
    assert reducer.state["session_id"] is None


def test_quests_reducer_folds_mission_received_during_active_session() -> None:
    reducer = QuestsReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "sess-1"})

    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Alpha"}
    )
    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Beta"}
    )

    assert reducer.state["session_id"] == "sess-1"
    assert reducer.state["mission_names_received"] == ["Alpha", "Beta"]


def test_quests_reducer_clears_session_id_on_stop_and_ignores_post_stop_missions() -> (
    None
):
    reducer = QuestsReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "sess-1"})
    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Alpha"}
    )

    reducer.on_event(EVENT_SESSION_STOPPED, {"session_id": "sess-1"})

    assert reducer.state["session_id"] is None
    assert reducer.state["mission_names_received"] == ["Alpha"]

    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Stray"}
    )

    assert reducer.state["mission_names_received"] == ["Alpha"]


def test_quests_reducer_hydrate_adopts_snapshot_and_resumes_folding() -> None:
    reducer = QuestsReducer()
    reducer.hydrate(
        {"session_id": "sess-1", "mission_names_received": ["Alpha", "Beta"]}
    )

    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": "Gamma"}
    )

    assert reducer.state["mission_names_received"] == ["Alpha", "Beta", "Gamma"]


def test_quests_reducer_ignores_non_dict_and_nameless_payloads() -> None:
    reducer = QuestsReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "sess-1"})

    reducer.on_event(EVENT_MISSION_RECEIVED, "not-a-dict")
    reducer.on_event(EVENT_MISSION_RECEIVED, {"type": "mission_received"})
    reducer.on_event(
        EVENT_MISSION_RECEIVED, {"type": "mission_received", "mission_name": ""}
    )

    assert reducer.state["mission_names_received"] == []


def test_tracking_reducer_folds_combat_loot_and_session_lifecycle() -> None:
    reducer = TrackingReducer()
    reducer.on_event(EVENT_SESSION_STARTED, {"session_id": "sess-9"})
    assert reducer.state["status"] == "active"
    assert reducer.state["session_id"] == "sess-9"

    reducer.on_event(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0})
    reducer.on_event(EVENT_COMBAT, {"type": "critical_hit", "amount": 25.0})
    # Defensive types still count as a shot but add no damage.
    reducer.on_event(EVENT_COMBAT, {"type": "target_dodge"})
    reducer.on_event(EVENT_COMBAT, {"type": "target_evade"})

    assert reducer.state["shots_fired_total"] == 4
    assert reducer.state["damage_dealt_total"] == 35.0
    assert reducer.state["critical_hits_total"] == 1

    reducer.on_event(
        EVENT_LOOT_GROUP,
        {"items": [{"value_ped": 5.0}, {"value_ped": 1.5}, "not-a-dict"]},
    )
    assert reducer.state["kill_count"] == 1
    assert reducer.state["returns"] == 6.5

    reducer.on_event(EVENT_SESSION_STOPPED, {"session_id": "sess-9"})
    assert reducer.state["status"] == "idle"
    # Totals persist after stop (the snapshot view still surfaces them).
    assert reducer.state["kill_count"] == 1


def test_tracking_reducer_ignores_non_dict_payloads() -> None:
    reducer = TrackingReducer()
    reducer.on_event(EVENT_COMBAT, "not-a-dict")
    reducer.on_event(EVENT_LOOT_GROUP, None)
    reducer.on_event(EVENT_SESSION_STARTED, "not-a-dict")  # status flips, id untouched
    assert reducer.state["kill_count"] == 0
    assert reducer.state["shots_fired_total"] == 0
    assert reducer.state["session_id"] is None


def test_isolated_surface_reducers_adopt_the_snapshot_on_hydrate() -> None:
    # Scan and codex surfaces are HTTP-mutated: their reducers carry no
    # subscriptions and simply adopt the snapshot wholesale on hydrate.
    for reducer in (ScanReducer(), CodexReducer()):
        snapshot = {"calibrations": 3, "claims": 2, "extra": "kept"}
        reducer.hydrate(snapshot)
        assert reducer.state == snapshot
        # A bus event must not move an isolated surface.
        reducer.on_event(EVENT_COMBAT, {"type": "damage_dealt", "amount": 1.0})
        assert reducer.state == snapshot
