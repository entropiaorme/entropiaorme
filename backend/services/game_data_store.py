"""In-memory catalogue of game constants loaded from JSON snapshots.

The snapshot — per-endpoint JSON files at ``backend/data/snapshot/`` —
is the app's sole source of truth for game-fact data (weapons, mobs,
skills, professions, etc.). The store loads it once at startup and
serves queries from memory.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

log = logging.getLogger(__name__)

# Endpoints whose snapshot file holds a single object (not a list).
_SINGLE_OBJECT_ENDPOINTS = frozenset({"skill_ranks"})


class GameDataStore:
    """Catalogue of game constants read from per-endpoint JSON files."""

    def __init__(self, snapshot_dir: Path) -> None:
        self._dir = snapshot_dir
        self._by_endpoint: dict[str, list[dict[str, Any]]] = {}
        self._load()

    def _load(self) -> None:
        if not self._dir.is_dir():
            log.warning("Game-data snapshot directory not found: %s", self._dir)
            return
        for path in sorted(self._dir.glob("*.json")):
            endpoint = path.stem
            with path.open("r", encoding="utf-8") as fh:
                data = json.load(fh)
            if endpoint in _SINGLE_OBJECT_ENDPOINTS:
                # Wrap so consumers that call get_entities()[0] keep working.
                self._by_endpoint[endpoint] = [data]
            elif isinstance(data, list):
                self._by_endpoint[endpoint] = data
            else:
                log.warning(
                    "Unexpected payload type for %s: %s",
                    endpoint,
                    type(data).__name__,
                )
                self._by_endpoint[endpoint] = []
        log.info(
            "Loaded game-data snapshot: %s",
            ", ".join(f"{ep}={len(items)}" for ep, items in self._by_endpoint.items()),
        )

    # --- Read API ---

    def get_entities(self, endpoint: str) -> list[dict[str, Any]]:
        """Return all entities for an endpoint (empty list if unknown)."""
        return self._by_endpoint.get(endpoint, [])

    def search_entities(
        self,
        query: str,
        endpoint: str | None = None,
        limit: int = 50,
    ) -> list[dict[str, Any]]:
        """Substring match by display name.

        Returns rows shaped ``{endpoint, item_id, item_name, data}``.
        Matches case-insensitively against the entity's display name
        (``species.name`` for mobs, ``name`` everywhere else).
        """
        q = query.lower()
        endpoints = [endpoint] if endpoint else list(self._by_endpoint)
        out: list[dict[str, Any]] = []
        for ep in endpoints:
            for entity in self._by_endpoint.get(ep, []):
                name = self._display_name(entity, ep)
                if not name or q not in name.lower():
                    continue
                out.append(
                    {
                        "endpoint": ep,
                        "item_id": entity.get("id"),
                        "item_name": name,
                        "data": entity,
                    }
                )
                if len(out) >= limit:
                    return out
        return out

    def find_entity(self, endpoint: str, item_id: Any) -> dict[str, Any] | None:
        """Return the entity with matching ``id`` in ``endpoint``, or None."""
        target = str(item_id)
        for entity in self._by_endpoint.get(endpoint, []):
            if str(entity.get("id", "")) == target:
                return entity
        return None

    # --- Introspection ---

    def endpoint_counts(self) -> dict[str, int]:
        return {ep: len(items) for ep, items in self._by_endpoint.items()}

    def total_entities(self) -> int:
        return sum(len(items) for items in self._by_endpoint.values())

    @staticmethod
    def _display_name(entity: dict[str, Any], endpoint: str) -> str | None:
        if endpoint == "mobs":
            species = entity.get("species") or {}
            return species.get("name")
        return entity.get("name")
