"""Property-based tests for the in-memory game-data catalogue.

Covers ``backend.services.game_data_store.GameDataStore``: the substring
search (``search_entities``), the id lookup (``find_entity``), and the two
introspection counters (``endpoint_counts`` / ``total_entities``). The store
loads per-endpoint JSON snapshots once at construction and is immutable
afterwards, so each generated catalogue is materialised as a fresh snapshot
directory and the store is rebuilt from it.

The snapshot uses two display-name shapes: mobs expose their name under
``species.name``; every other endpoint exposes a top-level ``name``. The
generators below mirror both shapes.
"""

from __future__ import annotations

import json
import shutil
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.services.game_data_store import GameDataStore

# Endpoint names that carry a top-level display name (the common shape).
_PLAIN_ENDPOINTS = ("weapons", "absorbers", "enhancers")

# Tokens kept small and overlapping so generated queries hit both the
# matching and non-matching branches of the search predicate.
_NAMES = st.text(alphabet="AbcXyz ", min_size=0, max_size=8)
_IDS = st.text(alphabet="0123456789w", min_size=1, max_size=4)
_QUERY = st.text(alphabet="AbcXyz ", min_size=0, max_size=6)


def _plain_entity(item_id: str, name: str) -> dict[str, Any]:
    return {"id": item_id, "name": name}


def _mob_entity(item_id: str, name: str) -> dict[str, Any]:
    return {"id": item_id, "species": {"name": name}}


_PLAIN_ENTITY = st.builds(_plain_entity, item_id=_IDS, name=_NAMES)
_MOB_ENTITY = st.builds(_mob_entity, item_id=_IDS, name=_NAMES)

# A generated catalogue: a plain-shaped endpoint plus the mobs endpoint, each
# with zero or more entities. Both shapes coexist so the mob special case and
# the common case are exercised together.
_CATALOGUE = st.fixed_dictionaries(
    {
        "weapons": st.lists(_PLAIN_ENTITY, min_size=0, max_size=6),
        "mobs": st.lists(_MOB_ENTITY, min_size=0, max_size=6),
    }
)


_tmp_factory: pytest.TempPathFactory


@pytest.fixture(autouse=True)
def _bind_tmp_factory(tmp_path_factory: pytest.TempPathFactory) -> None:
    """Root the module's snapshot temp dirs under pytest's auto-rotated basetemp.

    ``_store_from`` is a plain helper called per generated example from the
    property test bodies, not through the fixture protocol, so it cannot
    request ``tmp_path_factory`` itself. Binding it here keeps every
    helper-created dir under the tree pytest prunes, instead of the OS temp
    directory an interrupted run never cleans.
    """
    global _tmp_factory
    _tmp_factory = tmp_path_factory


def _store_from(catalogue: dict[str, list[dict[str, Any]]]) -> GameDataStore:
    """Materialise a catalogue as a snapshot directory and load a store."""
    snap = _tmp_factory.mktemp("snapshot")
    try:
        for endpoint, entities in catalogue.items():
            (snap / f"{endpoint}.json").write_text(
                json.dumps(entities), encoding="utf-8"
            )
        # The store reads every file eagerly in __init__, so the directory is
        # no longer needed once construction returns.
        return GameDataStore(snap)
    finally:
        shutil.rmtree(snap, ignore_errors=True)


def _display_name(entity: dict[str, Any], endpoint: str) -> Any:
    if endpoint == "mobs":
        return (entity.get("species") or {}).get("name")
    return entity.get("name")


# --- search_entities ---


@given(_CATALOGUE, _QUERY)
def test_search_rows_are_drawn_from_the_loaded_entities(catalogue, query):
    """search_results_subset_of_entities.

    Every returned row points back at a real loaded entity: its ``data`` is
    one of the entities under its ``endpoint``, its ``item_id`` is that
    entity's id, and its endpoint is a loaded key.
    """
    store = _store_from(catalogue)
    for row in store.search_entities(query):
        endpoint = row["endpoint"]
        siblings = store.get_entities(endpoint)
        assert any(row["data"] is entity for entity in siblings)
        assert row["item_id"] == row["data"].get("id")
        assert endpoint in store.endpoint_counts()


@given(_CATALOGUE, _QUERY)
def test_search_rows_name_resolves_by_endpoint(catalogue, query):
    """display_name_source_by_endpoint.

    A row's ``item_name`` comes from ``species.name`` for mobs and from the
    top-level ``name`` everywhere else, and is always a non-empty string (the
    search gate drops falsy display names before a row is emitted).
    """
    store = _store_from(catalogue)
    for row in store.search_entities(query):
        expected = _display_name(row["data"], row["endpoint"])
        assert row["item_name"] == expected
        assert isinstance(row["item_name"], str) and row["item_name"]


# --- introspection ---


@given(_CATALOGUE)
def test_total_entities_equals_sum_of_counts(catalogue):
    """total_entities_equals_sum_of_counts.

    The aggregate counter matches the sum of per-endpoint counts, and each
    per-endpoint count matches the length of that endpoint's entity list.
    """
    store = _store_from(catalogue)
    counts = store.endpoint_counts()
    assert store.total_entities() == sum(counts.values())
    for endpoint, count in counts.items():
        assert count == len(store.get_entities(endpoint))


# --- unknown endpoints ---


@given(
    _CATALOGUE,
    st.text(alphabet="AbcXyz_", min_size=1, max_size=12),
    _QUERY,
    _IDS,
)
def test_never_loaded_endpoint_reads_as_empty(catalogue, endpoint, query, item_id):
    """unknown_endpoint_returns_empty.

    For an endpoint the store never loaded, every read is total and yields the
    empty answer: get_entities -> [], targeted search -> [], find_entity ->
    None.
    """
    # Precondition: the endpoint must be one the catalogue did not load.
    if endpoint in catalogue:
        return
    store = _store_from(catalogue)
    assert store.get_entities(endpoint) == []
    assert store.search_entities(query, endpoint=endpoint) == []
    assert store.find_entity(endpoint, item_id) is None
