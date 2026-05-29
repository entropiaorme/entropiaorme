"""Tests for the in-memory game-data catalogue.

``GameDataStore`` loads per-endpoint JSON snapshots once and serves
substring search, id lookup, and introspection from memory. These drive
it against a temp snapshot directory so the load branches (list,
single-object, unexpected payload, missing directory) and the read API
all execute.
"""

from __future__ import annotations

import json
from pathlib import Path

from backend.services.game_data_store import GameDataStore


def _write(snapshot_dir: Path, endpoint: str, payload) -> None:
    (snapshot_dir / f"{endpoint}.json").write_text(
        json.dumps(payload), encoding="utf-8"
    )


def _store(tmp_path: Path) -> GameDataStore:
    snap = tmp_path / "snapshot"
    snap.mkdir()
    _write(
        snap,
        "weapons",
        [
            {"id": "w1", "name": "Sollomate Opalo"},
            {"id": "w2", "name": "Sollomate Justifier"},
        ],
    )
    _write(snap, "mobs", [{"id": "m1", "species": {"name": "Atrox"}}])
    _write(snap, "skill_ranks", {"ranks": [1, 2, 3]})  # single-object endpoint
    _write(snap, "broken", {"not": "a list"})  # unexpected payload -> warning + []
    return GameDataStore(snap)


def test_missing_directory_loads_empty(tmp_path):
    store = GameDataStore(tmp_path / "absent")
    assert store.total_entities() == 0
    assert store.get_entities("weapons") == []


def test_load_classifies_payload_shapes(tmp_path):
    store = _store(tmp_path)
    # List endpoint kept as-is.
    assert len(store.get_entities("weapons")) == 2
    # Single-object endpoint wrapped in a one-element list.
    assert store.get_entities("skill_ranks") == [{"ranks": [1, 2, 3]}]
    # Unexpected (non-list, non-single-object) payload becomes an empty list.
    assert store.get_entities("broken") == []
    assert store.get_entities("unknown_endpoint") == []


def test_search_matches_by_display_name_with_mob_special_case(tmp_path):
    store = _store(tmp_path)
    weapons = store.search_entities("sollomate", endpoint="weapons")
    assert {row["item_name"] for row in weapons} == {
        "Sollomate Opalo",
        "Sollomate Justifier",
    }
    # Mobs expose their name under species.name.
    mobs = store.search_entities("atrox")
    assert any(
        row["endpoint"] == "mobs" and row["item_name"] == "Atrox" for row in mobs
    )


def test_search_honours_the_limit(tmp_path):
    store = _store(tmp_path)
    assert len(store.search_entities("sollomate", endpoint="weapons", limit=1)) == 1


def test_find_entity_and_introspection(tmp_path):
    store = _store(tmp_path)
    assert store.find_entity("weapons", "w2")["name"] == "Sollomate Justifier"
    assert store.find_entity("weapons", "absent") is None
    assert store.endpoint_counts()["weapons"] == 2
    assert store.total_entities() == 2 + 1 + 1 + 0
