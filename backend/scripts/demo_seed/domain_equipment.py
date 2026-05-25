"""Equipment-enrichment seeder — UPDATEs equipment_library with resolved entities.

The core seeder inserts equipment_library rows with stub ``properties_json``
(``{"_demo_seed_stub": True, ...}``). This seeder runs after core, looks each
canonical item up in the bundled game-data snapshot (``backend/data/snapshot/``),
and rewrites ``catalog_id`` + ``properties_json`` with the real entity-shaped
dict so the Equipment tab renders cost-per-shot, decay, damage profiles
instead of zeros.

Items that don't resolve to an exact match are left with a benign-but-marked
stub (``_unresolved=True``) so the row stays valid; resolution can be
revisited later by editing the canonical-name table below.
"""

from __future__ import annotations

import json
import logging
import sqlite3
from pathlib import Path
from typing import Any

from backend.scripts.demo_seed.contract import CanonicalRefs
from backend.services.game_data_store import GameDataStore

log = logging.getLogger(__name__)


# Per-item resolution plan: name -> (endpoint, catalog_id, optional amp catalog_id).
# Catalogue-derived ids are baked in here (deterministic) rather than re-searching
# at seed time — substring search returns multiple candidates for several names
# and we want a stable pick.
#
# The canonical item list is 9 entries (5 weapons + 3 healings + 1 consumable);
# every name is an exact game-data snapshot match, and every weapon resolves
# cleanly:
#   - Emik Enigma L1 (L)              : weapons/2fdbb468a25f         (light laser pistol, (L))
#   - Korss H400                       : weapons/28cd2bcf0100         (UL laser sniper, trifecta big_weapon)
#   - Herman CAP-7 Jungle (L)          : weapons/f150af5a1607         (limited Blp carbiner)
#   - Jester D-1                       : weapons/e624d41e14ec         (UL Blp pistol)
#   - Castorian Pioneer EnBlade-2 (L)  : weapons/5394b1e1e682         (limited melee blade)
#   - Hedoc Mayhem, Adjusted           : medical_tools/65ba075fe3bc   (UL FAP, reclassified to healing)
#   - Vivo T1                          : medical_tools/f7ed51449e38   (UL heal tool)
#   - Vivo T5                          : medical_tools (exact-name)    (UL heal tool)
#   - H-DNA                            : stimulants/e409b5210482       (UL stimulant)
#
# Markup / amp population / damage enhancer counts are aligned with the
# consumable shape convention (`{catalog_id, entity}`); see the seed-time
# tuning block below.

# Markup tuning notes:
# UL items typically trade at or very near TT per the community convention
# (entropiawiki / EntropiaPlanets); (L) limited items carry a scarcity
# premium driving a 100-150% range. UL items here are pulled tight to TT
# (101-103); (L) limited items sit in the mid-100s (115-125) to demonstrate
# the Base+Amp / Full Setup badge contrast against UL trifecta gear.
#
# Amp population: Korss H400 stays amped (trifecta big_weapon, natural
# premium loadout); the four other weapons stay base, giving the Equipment
# Library a visible mix of Base / Base+Amp enrichment badges at roughly a
# 1-of-5 ratio.
#
# Damage enhancers: Korss H400 carries 1 enhancer; the rest carry 0.

_WEAPON_RESOLUTIONS: dict[str, dict[str, Any]] = {
    "Emik Enigma L1 (L)": {
        "endpoint": "weapons",
        "catalog_id": "2fdbb468a25f",
        "amp_catalog_id": None,
        "amp_endpoint": "weapon_amplifiers",
        "weapon_markup": 118,
        "amp_markup": 100,
        "damage_enhancers": 0,
    },
    "Korss H400": {
        "endpoint": "weapons",
        "catalog_id": "28cd2bcf0100",
        # Enrich with Omegaton A104 amp — classic mid-tier laser amplifier.
        "amp_catalog_id": "7b32a87ee33e",
        "amp_endpoint": "weapon_amplifiers",
        "weapon_markup": 103,
        "amp_markup": 102,
        "damage_enhancers": 1,
    },
    "Herman CAP-7 Jungle (L)": {
        "endpoint": "weapons",
        "catalog_id": "f150af5a1607",
        "amp_catalog_id": None,
        "amp_endpoint": "weapon_amplifiers",
        "weapon_markup": 122,
        "amp_markup": 100,
        "damage_enhancers": 0,
    },
    "Jester D-1": {
        "endpoint": "weapons",
        "catalog_id": "e624d41e14ec",
        # Base weapon (no amp): keeps the visible mix at the target 1-of-5 ratio.
        "amp_catalog_id": None,
        "amp_endpoint": "weapon_amplifiers",
        "weapon_markup": 102,
        "amp_markup": 100,
        "damage_enhancers": 0,
    },
    "Castorian Pioneer EnBlade-2 (L)": {
        "endpoint": "weapons",
        "catalog_id": "5394b1e1e682",
        "amp_catalog_id": None,
        "amp_endpoint": "weapon_amplifiers",
        "weapon_markup": 115,
        "amp_markup": 100,
        "damage_enhancers": 0,
    },
}

_HEAL_RESOLUTIONS: dict[str, dict[str, Any]] = {
    "Hedoc Mayhem, Adjusted": {
        "endpoint": "medical_tools",
        "catalog_id": "65ba075fe3bc",
        "markup": 102,
    },
    "Vivo T1": {
        "endpoint": "medical_tools",
        "catalog_id": "f7ed51449e38",
        "markup": 101,
    },
    "Vivo T5": {
        "endpoint": "medical_tools",
        "catalog_id": None,  # resolved by exact-name lookup at seed time
        "markup": 102,
    },
}


# Allow-list of canonical item names — used by validate_synthetic_data.
# 9 items (5 weapons + 3 healings + 1 consumable). Hedoc Mayhem is classified
# as a healing tool here.
_ALLOWED_ITEM_NAMES: frozenset[str] = frozenset(
    {
        "Emik Enigma L1 (L)",
        "Korss H400",
        "Herman CAP-7 Jungle (L)",
        "Jester D-1",
        "Castorian Pioneer EnBlade-2 (L)",
        "Hedoc Mayhem, Adjusted",
        "Vivo T1",
        "Vivo T5",
        "H-DNA",
    }
)


def _find_exact(gds: GameDataStore, endpoint: str, name: str) -> dict | None:
    """Return the entity dict whose display name exactly matches ``name``."""
    for entity in gds.get_entities(endpoint):
        ent_name = (
            entity.get("name")
            if endpoint != "mobs"
            else (entity.get("species") or {}).get("name")
        )
        if ent_name == name:
            return entity
    return None


def _resolve_weapon(
    gds: GameDataStore, item_name: str
) -> tuple[str | None, dict] | None:
    """Resolve a weapon item to (catalog_id, properties_json dict). None if unresolved."""
    plan = _WEAPON_RESOLUTIONS.get(item_name)
    if plan is None:
        return None
    weapon_entity = gds.find_entity(plan["endpoint"], plan["catalog_id"])
    if weapon_entity is None:
        return None

    amp_entity: dict | None = None
    amp_catalog_id: str | None = None
    if plan.get("amp_catalog_id"):
        amp_entity = gds.find_entity(plan["amp_endpoint"], plan["amp_catalog_id"])
        if amp_entity is not None:
            amp_catalog_id = plan["amp_catalog_id"]

    props: dict[str, Any] = {
        "weapon_entity": weapon_entity,
        "weapon_catalog_id": plan["catalog_id"],
        "amp_entity": amp_entity,
        "amp_catalog_id": amp_catalog_id,
        "scope_entity": None,
        "scope_catalog_id": None,
        "absorber_entity": None,
        "absorber_catalog_id": None,
        "damage_enhancers": plan.get("damage_enhancers", 0),
        "weapon_markup": plan.get("weapon_markup", 102),
        "amp_markup": plan.get("amp_markup", 102),
        "scope_markup": 100,
        "absorber_markup": 100,
    }
    return plan["catalog_id"], props


def _resolve_heal(gds: GameDataStore, item_name: str) -> tuple[str | None, dict] | None:
    plan = _HEAL_RESOLUTIONS.get(item_name)
    if plan is None:
        return None

    if plan["catalog_id"]:
        tool_entity = gds.find_entity(plan["endpoint"], plan["catalog_id"])
    else:
        tool_entity = _find_exact(gds, plan["endpoint"], item_name)

    if tool_entity is None:
        return None

    catalog_id = tool_entity.get("id")
    props = {
        "tool_entity": tool_entity,
        "tool_catalog_id": catalog_id,
        "markup": plan.get("markup", 102),
    }
    return catalog_id, props


def _unresolved_props(item_type: str, name: str, profession: str | None = None) -> dict:
    """Benign-but-marked properties_json for an item we couldn't resolve."""
    base = {"_unresolved": True, "name": name}
    if profession:
        base["_demo_profession"] = profession
    if item_type == "weapon":
        base.update(
            {
                "weapon_entity": None,
                "amp_entity": None,
                "scope_entity": None,
                "absorber_entity": None,
                "damage_enhancers": 0,
                "weapon_markup": 100,
                "amp_markup": 100,
            }
        )
    elif item_type == "healing":
        base.update({"tool_entity": None, "markup": 100})
    elif item_type == "armour":
        base.update({"armour_entity": None, "markup": 100})
    return base


def _consumable_props_unresolved(name: str) -> dict:
    """Unresolved-consumable shape: keeps the convention top-level keys (`catalog_id`,
    `entity`) so downstream readers find the same shape on resolved + unresolved
    rows; the unresolved marker lives alongside.
    """
    return {"catalog_id": None, "entity": None, "_unresolved": True, "name": name}


class EquipmentSeeder:
    name: str = "equipment"
    depends_on: tuple[str, ...] = ("core",)

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        snapshot_dir = (
            Path(__file__).resolve().parents[3] / "backend" / "data" / "snapshot"
        )
        gds = GameDataStore(snapshot_dir)

        resolved = 0
        unresolved: list[str] = []

        for item in refs.items:
            catalog_id: str | None = None
            props: dict

            if item.item_type == "weapon":
                outcome = _resolve_weapon(gds, item.name)
                if outcome is None:
                    unresolved.append(item.name)
                    log.warning(
                        "equipment seeder: weapon %r unresolved in game-data", item.name
                    )
                    props = _unresolved_props("weapon", item.name, item.profession)
                else:
                    catalog_id, props = outcome
                    resolved += 1
            elif item.item_type == "healing":
                outcome = _resolve_heal(gds, item.name)
                if outcome is None:
                    unresolved.append(item.name)
                    log.warning("equipment seeder: heal tool %r unresolved", item.name)
                    props = _unresolved_props("healing", item.name)
                else:
                    catalog_id, props = outcome
                    resolved += 1
            elif item.item_type == "armour":
                # Snapshot has no armours endpoint — leave unresolved but valid.
                unresolved.append(item.name)
                log.warning(
                    "equipment seeder: armour %r unresolved (no armours endpoint)",
                    item.name,
                )
                props = _unresolved_props("armour", item.name)
            elif item.item_type == "consumable":
                stim_match = _find_exact(gds, "stimulants", item.name)
                if stim_match is not None:
                    catalog_id = stim_match.get("id")
                    # Match the consumable storage convention: top-level {catalog_id, entity}.
                    props = {"catalog_id": catalog_id, "entity": stim_match}
                    resolved += 1
                else:
                    unresolved.append(item.name)
                    props = _consumable_props_unresolved(item.name)
            else:
                log.warning(
                    "equipment seeder: unknown item_type %r for %r — skipping enrichment",
                    item.item_type,
                    item.name,
                )
                continue

            db.execute(
                "UPDATE equipment_library SET catalog_id = ?, properties_json = ? WHERE id = ?",
                (catalog_id, json.dumps(props), item.library_id),
            )

        log.info(
            "equipment seeder: UPDATEd %d rows (%d resolved, %d unresolved).",
            len(refs.items),
            resolved,
            len(unresolved),
        )
        if unresolved:
            log.info(
                "equipment seeder: unresolved canonical items: %s",
                ", ".join(unresolved),
            )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []
        if len(refs.items) != 9:
            violations.append(
                f"refs.items length is {len(refs.items)}, expected 9 canonical items"
            )
        for item in refs.items:
            if item.name not in _ALLOWED_ITEM_NAMES:
                violations.append(
                    f"item name {item.name!r} not in canonical allow-list"
                )
        return violations


SEEDER: "EquipmentSeeder" = EquipmentSeeder()


# Self-test entry point — runs core + equipment seeders against a temp dir.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_equipment_"))
    try:
        report = run(tmp, extra_seeders=[SEEDER])
        print(format_report(report))

        # Quick post-run inspection: verify catalog_id + properties_json shape.
        import sqlite3 as _sq

        conn = _sq.connect(tmp / "entropia_orme.db")
        conn.row_factory = _sq.Row
        rows = conn.execute(
            "SELECT name, item_type, catalog_id, properties_json FROM equipment_library ORDER BY id"
        ).fetchall()
        print()
        print("equipment_library after enrichment:")
        for r in rows:
            props = json.loads(r["properties_json"])
            stub_marker = props.get("_demo_seed_stub")
            unresolved_marker = props.get("_unresolved")
            entity_keys = [
                k
                for k in (
                    "weapon_entity",
                    "tool_entity",
                    "armour_entity",
                    "stim_entity",
                )
                if props.get(k)
            ]
            mark = (
                "stub!"
                if stub_marker
                else ("unresolved" if unresolved_marker else "resolved")
            )
            print(
                f"  [{r['item_type']:10s}] {r['name']:25s} catalog_id={r['catalog_id'] or '-':14s} "
                f"{mark:11s} entities={entity_keys}"
            )
        conn.close()

        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
