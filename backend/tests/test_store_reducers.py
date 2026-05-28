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
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.testing.store_reducers import QuestsReducer


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
