"""Mutation-killing tests for QuestService playlist item handling.

Targets the cluster quest_service__c8, covering
``QuestService._set_playlist_items`` and
``QuestService._normalize_playlist_items``. These two methods translate a
playlist payload (``items`` or ``quest_ids``) into normalised, group-classified
rows in ``quest_playlist_items`` and write them back with explicit sort order
and validated group types.

Each test drives the real ``backend.services.quest_service`` over an on-disk
SQLite database (mirroring the sibling property suite) and asserts the exact
observable behaviour each mutation breaks: the persisted sort order, the
group-type defaulting, the dict/scalar item branch, the group-type validation,
and the items-vs-quest_ids normalisation branch.
"""

from __future__ import annotations

import pytest

from backend.db.app_database import AppDatabase
from backend.services.quest_service import (
    PLAYLIST_GROUP_IMMEDIATE,
    PLAYLIST_GROUP_LONG_HORIZON,
    QuestService,
)

_tmp_factory: pytest.TempPathFactory


@pytest.fixture(autouse=True)
def _bind_tmp_factory(tmp_path_factory: pytest.TempPathFactory) -> None:
    """Root the module's DB temp dirs under pytest's auto-rotated basetemp.

    ``_make_service`` is a plain helper called from test bodies, not through
    the fixture protocol, so it cannot request ``tmp_path_factory`` itself.
    Binding it here keeps every helper-created dir under the tree pytest
    prunes, instead of the OS temp directory an interrupted run never cleans.
    """
    global _tmp_factory
    _tmp_factory = tmp_path_factory


def _make_service() -> QuestService:
    tmp = _tmp_factory.mktemp("quests") / "quests.db"
    db = AppDatabase(tmp)
    return QuestService(db)


def _new_quest(svc: QuestService, name: str) -> int:
    return svc.create_quest({"name": name})["id"]


def _raw_items(svc: QuestService, playlist_id: int) -> list[tuple]:
    """The persisted item rows, in physical sort_order, with raw columns."""
    return svc._conn.execute(
        "SELECT quest_id, sort_order, description, group_type "
        "FROM quest_playlist_items WHERE playlist_id = ? ORDER BY sort_order",
        (playlist_id,),
    ).fetchall()


# --- _normalize_playlist_items: items branch ---


def test_items_branch_preserves_quest_id_description_and_group():
    """A payload with ``items`` keeps each quest_id, description and group_type."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    q2 = _new_quest(svc, "B")
    pl = svc.create_playlist(
        {
            "name": "PL",
            "items": [
                {
                    "quest_id": q1,
                    "description": "first",
                    "group_type": PLAYLIST_GROUP_IMMEDIATE,
                },
                {
                    "quest_id": q2,
                    "description": "second",
                    "group_type": PLAYLIST_GROUP_LONG_HORIZON,
                },
            ],
        }
    )
    by_id = {it["quest_id"]: it for it in pl["items"]}
    assert by_id[q1]["description"] == "first"
    assert by_id[q1]["group_type"] == PLAYLIST_GROUP_IMMEDIATE
    assert by_id[q2]["description"] == "second"
    assert by_id[q2]["group_type"] == PLAYLIST_GROUP_LONG_HORIZON
    assert pl["immediate_quest_ids"] == [q1]
    assert pl["long_horizon_quest_ids"] == [q2]


def test_items_group_type_defaults_to_immediate():
    """An item dict without group_type defaults to immediate, not None/other."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "items": [{"quest_id": q1}]})
    assert pl["items"][0]["group_type"] == PLAYLIST_GROUP_IMMEDIATE
    assert pl["immediate_quest_ids"] == [q1]
    assert pl["long_horizon_quest_ids"] == []


def test_items_description_defaults_to_none():
    """An item dict without description normalises to None (not "")."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "items": [{"quest_id": q1}]})
    assert pl["items"][0]["description"] is None


def test_empty_items_list_is_used_over_quest_ids():
    """An empty (but present) ``items`` list takes the items branch.

    ``items`` is ``[]`` which is not None, so normalisation must use it and
    ignore ``quest_ids`` entirely -> the playlist ends up empty.
    """
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "items": [], "quest_ids": [q1]})
    assert pl["items"] == []
    assert pl["quest_ids"] == []


# --- _normalize_playlist_items: quest_ids branch ---


def test_quest_ids_branch_when_items_absent():
    """With no ``items`` key, quest_ids drive immediate items with None desc."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    q2 = _new_quest(svc, "B")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1, q2]})
    assert pl["quest_ids"] == [q1, q2]
    assert all(it["group_type"] == PLAYLIST_GROUP_IMMEDIATE for it in pl["items"])
    assert all(it["description"] is None for it in pl["items"])
    assert pl["immediate_quest_ids"] == [q1, q2]
    assert pl["long_horizon_quest_ids"] == []


def test_quest_ids_missing_yields_empty_playlist():
    """No items and no quest_ids -> empty playlist (default [] for quest_ids)."""
    svc = _make_service()
    pl = svc.create_playlist({"name": "PL"})
    assert pl["items"] == []
    assert pl["quest_ids"] == []


# --- _set_playlist_items: sort order, branches, validation ---


def test_set_items_assigns_incrementing_sort_order():
    """Items are persisted with sort_order 0,1,2... in enumeration order."""
    svc = _make_service()
    qids = [_new_quest(svc, f"Q{i}") for i in range(4)]
    pl = svc.create_playlist({"name": "PL", "quest_ids": qids})
    rows = _raw_items(svc, pl["id"])
    assert [r[0] for r in rows] == qids
    assert [r[1] for r in rows] == [0, 1, 2, 3]


def test_set_items_scalar_branch_via_quest_ids():
    """quest_ids produce scalar items -> persisted as immediate, None desc."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1]})
    rows = _raw_items(svc, pl["id"])
    assert rows[0][0] == q1
    assert rows[0][2] is None
    assert rows[0][3] == PLAYLIST_GROUP_IMMEDIATE


def test_set_items_replaces_existing_rows_on_update():
    """Updating items deletes the old rows first (no stale rows linger)."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    q2 = _new_quest(svc, "B")
    q3 = _new_quest(svc, "C")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1, q2]})
    updated = svc.update_playlist(pl["id"], {"quest_ids": [q3]})
    assert updated is not None
    assert updated["quest_ids"] == [q3]
    rows = _raw_items(svc, pl["id"])
    assert [r[0] for r in rows] == [q3]
    assert [r[1] for r in rows] == [0]


def test_invalid_group_type_raises_value_error_naming_the_value():
    """A group_type outside the allowed set is rejected with a ValueError whose
    message names the offending value (not a bare ``ValueError(None)``)."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    with pytest.raises(ValueError, match="Invalid playlist group type: bogus"):
        svc.create_playlist(
            {"name": "PL", "items": [{"quest_id": q1, "group_type": "bogus"}]}
        )


# --- _set_playlist_items: direct invocation drives the scalar/default paths
# that the public create/update path (which always normalises to full dicts)
# never reaches. These exercise the method's own documented contract: scalar
# items become immediate/None, and a dict without group_type defaults to
# immediate. ---


def test_set_items_dict_without_group_type_defaults_to_immediate():
    """A dict item lacking group_type defaults to immediate (not None).

    With the default removed, group_type becomes None which fails validation;
    the real code must persist it as immediate.
    """
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1]})
    svc._set_playlist_items(pl["id"], [{"quest_id": q1, "description": "d"}])
    svc._conn.commit()
    rows = _raw_items(svc, pl["id"])
    assert len(rows) == 1
    assert rows[0][0] == q1
    assert rows[0][3] == PLAYLIST_GROUP_IMMEDIATE


def test_set_items_scalar_item_persists_real_quest_id():
    """A scalar (int) item is stored under its own quest_id.

    If the scalar branch dropped the id (qid = None), the NOT NULL quest_id
    column would reject the insert, so a successful row with quest_id == q1
    proves the id was carried through.
    """
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    q2 = _new_quest(svc, "B")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1]})
    svc._set_playlist_items(pl["id"], [q1, q2])
    svc._conn.commit()
    rows = _raw_items(svc, pl["id"])
    assert [r[0] for r in rows] == [q1, q2]


def test_set_items_scalar_item_description_is_none():
    """A scalar item gets a NULL description, never an empty string."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1]})
    svc._set_playlist_items(pl["id"], [q1])
    svc._conn.commit()
    rows = _raw_items(svc, pl["id"])
    assert rows[0][2] is None


def test_set_items_scalar_item_group_type_is_immediate():
    """A scalar item is classified immediate and passes validation.

    If the scalar branch set group_type to None it would fail the allowed-set
    check and raise; a persisted 'immediate' row proves it defaulted correctly.
    """
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist({"name": "PL", "quest_ids": [q1]})
    svc._set_playlist_items(pl["id"], [q1])
    svc._conn.commit()
    rows = _raw_items(svc, pl["id"])
    assert rows[0][3] == PLAYLIST_GROUP_IMMEDIATE


def test_long_horizon_group_type_is_accepted_and_persisted():
    """The long_horizon group type passes validation and is stored verbatim."""
    svc = _make_service()
    q1 = _new_quest(svc, "A")
    pl = svc.create_playlist(
        {
            "name": "PL",
            "items": [{"quest_id": q1, "group_type": PLAYLIST_GROUP_LONG_HORIZON}],
        }
    )
    rows = _raw_items(svc, pl["id"])
    assert rows[0][3] == PLAYLIST_GROUP_LONG_HORIZON
    assert pl["long_horizon_quest_ids"] == [q1]
    assert pl["immediate_quest_ids"] == []
