"""Mob-name lookup against the bundled mobs catalogue.

Used by manual-mob tracking flows for autocomplete and validation.
"""

import logging

from backend.services.game_data_store import GameDataStore

log = logging.getLogger(__name__)


class MobLookupService:
    def __init__(self, game_data: GameDataStore) -> None:
        self.game_data = game_data

    def search_mob_names(self, query: str, limit: int = 10) -> list[dict]:
        """Return exact mob display names as `{maturity} {species}` suggestions."""
        q = query.strip().lower()
        if not q:
            return []
        q_parts = [part for part in q.split() if part]

        results: list[dict] = []
        seen: set[tuple[str, str]] = set()

        for mob in self.game_data.get_entities("mobs"):
            species = (
                (mob.get("species") or {}).get("name") or mob.get("name") or ""
            ).strip()
            if not species:
                continue

            maturities = mob.get("maturities") or []
            if not maturities:
                key = (species, "")
                display = species
                display_lower = display.lower()
                if key not in seen and (
                    q in display_lower or all(part in display_lower for part in q_parts)
                ):
                    seen.add(key)
                    results.append(
                        {"display": display, "species": species, "maturity": ""}
                    )
                continue

            for maturity_entry in maturities:
                maturity = (maturity_entry.get("name") or "").strip()
                display = f"{maturity} {species}" if maturity else species
                key = (species, maturity)
                display_lower = display.lower()
                if key in seen or (
                    q not in display_lower
                    and not all(part in display_lower for part in q_parts)
                ):
                    continue
                seen.add(key)
                results.append(
                    {"display": display, "species": species, "maturity": maturity}
                )

        results.sort(
            key=lambda r: (0 if r["display"].lower().startswith(q) else 1, r["display"])
        )
        return results[:limit]

    def has_mob_name(self, species: str, maturity: str) -> bool:
        """Return True when the exact species/maturity pair exists in the catalogue."""
        species = species.strip()
        maturity = maturity.strip()
        if not species:
            return False

        for mob in self.game_data.get_entities("mobs"):
            cached_species = (
                (mob.get("species") or {}).get("name") or mob.get("name") or ""
            ).strip()
            if cached_species != species:
                continue

            maturities = mob.get("maturities") or []
            if not maturities:
                return maturity == ""

            for maturity_entry in maturities:
                if (maturity_entry.get("name") or "").strip() == maturity:
                    return True
            return False

        return False
