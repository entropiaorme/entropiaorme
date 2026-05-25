"""Core seeder — owns the canonical reference list every other seeder reads from.

This seeder lands the foundational substrate (settings, equipment library
rows, quests, playlists, codex species placeholders) and returns a frozen
``CanonicalRefs`` for the per-domain seeders to consume.

Per-domain seeders may invent *interactions with* references (kills against
canonical mobs, skill_calibration rows for canonical skills, ledger entries
tagged against canonical sources) but never new canonical references. If a
seeder needs a new mob species or weapon that isn't in CanonicalRefs, the
right move is to extend the canonical list here and update everyone, not to
fork divergent identifiers per domain.

Synthetic-data invariant: every reference below is fictional EU lore the
public can already look up, paired with a fictional character and fictional
numbers; no real player identifiers, no real ledger values. The character
name is fictional; numbers are plausible-but-fictional; mob and item names
are real EU game lore (public, not user-specific data) so the codex /
equipment catalogue has matching entries to display against.
"""

from __future__ import annotations

import json
import logging
import sqlite3
import time
from collections.abc import Callable
from pathlib import Path

from backend.scripts.demo_seed.contract import (
    CanonicalRefs,
    CharacterProfile,
    HotbarBinding,
    ItemRef,
    LiveInjectionScenario,
    MobRef,
    PlaylistRef,
    QuestRef,
    TimelineAnchor,
    TrifectaPresetRef,
)

log = logging.getLogger(__name__)

NAME = "core"


# ─── Character ───────────────────────────────────────────────────────────────

# Synthetic avatar: fictional, EU-flavoured naming (no relation to real users).
# chatlog_path uses an obvious placeholder shape so any read of the seeded
# settings (e.g. the Settings panel) renders it as self-evidently
# non-functional rather than as a real-looking user path. The file does not
# exist on any system so Settings > Game Connection will display "Not found".
CHARACTER = CharacterProfile(
    player_name="Aria Ven Solana",
    chatlog_path=r"C:\<your-Entropia-Universe-folder>\chat.log",
    theme="dark",
)

# 90-day window of synthetic history; "demo_now" is captured at seed time.
HISTORY_WINDOW_DAYS = 90


# ─── Mobs ────────────────────────────────────────────────────────────────────

# Real EU species names (public game lore). Per-domain seeders (codex / kills
# / analytics) reference these by species + maturity. A seeder may use any
# subset; canonical means "available", not "must use all of these".
MOBS: tuple[MobRef, ...] = (
    MobRef(species="Caboria", maturities=("Young", "Mature", "Old", "Provider")),
    MobRef(species="Atrox", maturities=("Young", "Mature", "Old", "Stalker")),
    MobRef(species="Argonaut", maturities=("Young", "Mature", "Old", "Provider")),
    MobRef(species="Combibo", maturities=("Young", "Mature", "Old")),
    MobRef(species="Daikiba", maturities=("Young", "Mature", "Old", "Provider")),
    MobRef(species="Snablesnot Male", maturities=("Young", "Old", "Provider")),
)

# Codex-exposed species — overlaps with kills / analytics, not necessarily
# 1:1. The codex seeder picks rank progression per species from this list.
CODEX_SPECIES: tuple[str, ...] = tuple(m.species for m in MOBS)


# ─── Skills ──────────────────────────────────────────────────────────────────

# Canonical skill subset. Per-domain seeders write skill_calibrations rows
# against these names (the game data catalogue must contain matching entries
# for character/skills to render).
SKILL_CATEGORIES: dict[str, tuple[str, ...]] = {
    "Combat (Ranged)": (
        "Hit Ability",
        "Damage Ability",
        "Combat Reflexes",
        "Combat Sense",
        "Ranged Laser (Hit)",
        "Ranged Laser (Dmg)",
        "Ranged Blp (Hit)",
        "Ranged Blp (Dmg)",
        "Aim",
        "Anatomy",
    ),
    "Combat (Melee)": (
        "Melee Combat (Hit)",
        "Melee Combat (Dmg)",
        "Coolness",
        "Power Catch",
        "Lightweight Melee Weapons",
    ),
    "Support": (
        "Medical Therapy",
        "Bioregenesis",
        "Diagnosis",
        "First Aid",
    ),
    "Survival": (
        "Evade",
        "Dodge",
        "Athletics",
        "Wounded",
        "Serendipity",
        "Inflict Ranged Damage",
        "Inflict Melee Damage",
    ),
    "Trade": (
        "Reputation",
        "Charisma",
        "Trade",
        "Computer",
        "Engineering",
        "Mechanical Engineering",
    ),
    "Mind": (
        "Intuition",
        "Concentration",
        "Psyche",
        "Will Power",
    ),
}


def _flatten_skills() -> tuple[str, ...]:
    out: list[str] = []
    for skills in SKILL_CATEGORIES.values():
        out.extend(skills)
    return tuple(out)


SKILL_NAMES: tuple[str, ...] = _flatten_skills()


# ─── Attributes ──────────────────────────────────────────────────────────────

# Six canonical EU attributes. Per-domain seeders write skill_calibration rows
# for these names just like skills (per the backend's source-agnostic model).
ATTRIBUTE_NAMES: tuple[str, ...] = (
    "Health",
    "Stamina",
    "Agility",
    "Strength",
    "Psyche",
    "Intelligence",
)


# ─── Equipment library ───────────────────────────────────────────────────────

# Canonical items the demo character "owns". Core seeder writes minimal rows
# (name + item_type + null catalog_id + stub properties_json); the equipment
# seeder resolves catalog_ids and enriches properties_json without changing
# the IDs.


# Stub properties_json shape — enough to satisfy the equipment routes' read
# path without breaking. Real cost-per-shot calculation requires resolved
# weapon entities, which the equipment seeder fills in.
def _stub_weapon_props(name: str, profession: str) -> dict:
    return {
        "tool_entity": None,
        "amplifier_entity": None,
        "scope_entity": None,
        "sight_entities": [],
        "absorber_entity": None,
        "damage_enhancers": 0,
        "markup": 100,
        "amp_markup": 100,
        "_demo_seed_stub": True,
        "_demo_profession": profession,
    }


def _stub_heal_props(name: str) -> dict:
    return {
        "tool_entity": None,
        "markup": 100,
        "_demo_seed_stub": True,
    }


def _stub_armour_props(name: str) -> dict:
    return {
        "armour_entity": None,
        "markup": 100,
        "_demo_seed_stub": True,
    }


def _stub_consumable_props(name: str) -> dict:
    return {
        "_demo_seed_stub": True,
    }


# Item declarations as (name, item_type, profession-or-None, props-builder).
# Names match game-data snapshot entities exactly so equipment-domain
# resolution succeeds. Armour items are intentionally absent: the public app
# has no armour-management UI today, so equipping armour rows here would
# produce surfaces that can't render. Hedoc Mayhem is classified as a heal
# tool per real game-data (it is a UL FAP, not a weapon).
_ITEM_SEEDS: tuple[tuple[str, str, str | None, Callable[..., dict]], ...] = (
    # Weapons (5) — laser pistols, blp carbine + pistol, melee blade.
    ("Emik Enigma L1 (L)", "weapon", "Laser Pistoleer", _stub_weapon_props),
    ("Korss H400", "weapon", "Laser Pistoleer", _stub_weapon_props),
    ("Herman CAP-7 Jungle (L)", "weapon", "Blp Carbiner", _stub_weapon_props),
    ("Jester D-1", "weapon", "Blp Pistoleer", _stub_weapon_props),
    ("Castorian Pioneer EnBlade-2 (L)", "weapon", "Swordsman", _stub_weapon_props),
    # Healing tools (3) — including the reclassified Hedoc Mayhem, Adjusted.
    ("Hedoc Mayhem, Adjusted", "healing", None, _stub_heal_props),
    ("Vivo T1", "healing", None, _stub_heal_props),
    ("Vivo T5", "healing", None, _stub_heal_props),
    # Consumable (1).
    ("H-DNA", "consumable", None, _stub_consumable_props),
)


# ─── Quests ──────────────────────────────────────────────────────────────────

# Canonical quest list — 6 categories, mix of chained / standalone, mixed
# rewards. The quests seeder extends states (cooldowns, started timestamps)
# and links analytics; this list is the source of truth for which quest IDs
# exist.
_QUEST_SEEDS: tuple[dict, ...] = (
    # Codex chain — 3-quest sequence
    {
        "name": "Codex: Caboria I",
        "category": "Codex Chains",
        "planet": "Calypso",
        "mobs": ("Caboria",),
        "chain": ("Caboria Codex", 1, 3),
        "reward_is_skill": True,
        "reward_ped": 25.0,
        "waypoint": "[Calypso, 80000, 78000, 100, Crater Lake]",
        "cooldown_hours": None,
    },
    {
        "name": "Codex: Caboria II",
        "category": "Codex Chains",
        "planet": "Calypso",
        "mobs": ("Caboria",),
        "chain": ("Caboria Codex", 2, 3),
        "reward_is_skill": True,
        "reward_ped": 50.0,
        "waypoint": None,
        "cooldown_hours": None,
    },
    {
        "name": "Codex: Caboria III",
        "category": "Codex Chains",
        "planet": "Calypso",
        "mobs": ("Caboria",),
        "chain": ("Caboria Codex", 3, 3),
        "reward_is_skill": True,
        "reward_ped": 100.0,
        "waypoint": None,
        "cooldown_hours": None,
    },
    # Daily missions — recurring with cooldowns
    {
        "name": "Argonaut Hunt (Daily)",
        "category": "Daily Missions",
        "planet": "Calypso",
        "mobs": ("Argonaut",),
        "chain": None,
        "reward_is_skill": False,
        "reward_ped": 12.0,
        "waypoint": "[Calypso, 65500, 80200, 80, Argus Depths]",
        "cooldown_hours": 22.0,
    },
    {
        "name": "Atrox Cull (Daily)",
        "category": "Daily Missions",
        "planet": "Calypso",
        "mobs": ("Atrox",),
        "chain": None,
        "reward_is_skill": False,
        "reward_ped": 18.0,
        "waypoint": "[Calypso, 75000, 81000, 110, Atrox Stronghold]",
        "cooldown_hours": 22.0,
    },
    {
        "name": "Combibo Patrol",
        "category": "Daily Missions",
        "planet": "Calypso",
        "mobs": ("Combibo",),
        "chain": None,
        "reward_is_skill": False,
        "reward_ped": 8.0,
        "waypoint": None,
        "cooldown_hours": 22.0,
    },
    # NOTE: Iron Missions are an outdated EU concept that no longer exists in
    # current game state — removed from canonical. The "Iron Skilling" playlist
    # that referenced them is also dropped (see _PLAYLIST_SEEDS below).
    # Repeatable bounties
    {
        "name": "Bounty: Atrox Stalker",
        "category": "Bounties",
        "planet": "Calypso",
        "mobs": ("Atrox",),
        "chain": None,
        "reward_is_skill": False,
        "reward_ped": 35.0,
        "waypoint": "[Calypso, 76200, 81500, 105, Stalker Grounds]",
        "cooldown_hours": 168.0,
    },
    {
        "name": "Bounty: Argonaut Provider",
        "category": "Bounties",
        "planet": "Calypso",
        "mobs": ("Argonaut",),
        "chain": None,
        "reward_is_skill": False,
        "reward_ped": 28.0,
        "waypoint": None,
        "cooldown_hours": 168.0,
    },
    # Long-horizon goals
    {
        "name": "Codex Master: Caboria",
        "category": "Long-Horizon",
        "planet": "Calypso",
        "mobs": ("Caboria",),
        "chain": None,
        "reward_is_skill": True,
        "reward_ped": 500.0,
        "waypoint": None,
        "cooldown_hours": None,
    },
    {
        "name": "Codex Master: Daikiba",
        "category": "Long-Horizon",
        "planet": "Calypso",
        "mobs": ("Daikiba",),
        "chain": None,
        "reward_is_skill": True,
        "reward_ped": 500.0,
        "waypoint": None,
        "cooldown_hours": None,
    },
)


# ─── Playlists ───────────────────────────────────────────────────────────────

# Each playlist references _QUEST_SEEDS positions (resolved to DB IDs after insert).
# Indices reflect the post-Iron-Mission removal layout:
#   0: Codex: Caboria I       1: Codex: Caboria II      2: Codex: Caboria III
#   3: Argonaut Hunt (Daily)  4: Atrox Cull (Daily)     5: Combibo Patrol
#   6: Bounty: Atrox Stalker  7: Bounty: Argonaut Prov. 8: Codex Master: Caboria
#   9: Codex Master: Daikiba
_PLAYLIST_SEEDS: tuple[dict, ...] = (
    {
        "name": "Quick Dailies",
        "planet": "Calypso",
        "estimated_minutes": 25,
        "immediate_indices": (3, 5),  # Argonaut Daily + Combibo Patrol
        "long_horizon_indices": (8,),  # Codex Master: Caboria
    },
    {
        "name": "Atrox Run",
        "planet": "Calypso",
        "estimated_minutes": 45,
        "immediate_indices": (4, 6),  # Atrox Cull + Atrox Stalker Bounty
        "long_horizon_indices": (),
    },
    {
        "name": "Codex Chain: Caboria",
        "planet": "Calypso",
        "estimated_minutes": 60,
        "immediate_indices": (0, 1, 2),  # Caboria I/II/III chain
        "long_horizon_indices": (8,),  # Codex Master: Caboria
    },
)


# ─── Trifecta + hotbar ───────────────────────────────────────────────────────

# Built after equipment_library is inserted so library_ids are known.
TRIFECTA_PRESET_ID = "demo_default"
TRIFECTA_PRESET_NAME = "Calypso"


# ─── Live-injection scenarios ────────────────────────────────────────────────

# Three injectable in-memory states for live in-flight overlay shots. Consumed
# by ``live_injection.py`` (dev-gated).

LIVE_SCENARIOS: tuple[LiveInjectionScenario, ...] = (
    LiveInjectionScenario(
        name="mid_hunt",
        description=(
            "Mid-hunt at ~12 minutes with 100 kills on Caboria Old (Korss H400). "
            "Headline targets: rate 105.2%, last loot 0.80 PED, avg cost 1.52 PED, "
            "PES 5.02 PED, DPP 3.05, max mult 5.85x on a global kill carrying the "
            "literal loot composition. The priming handler in live_injection.py "
            "consumes only `elapsed_seconds`; the rest of the payload is "
            "descriptive and may drift from the engineered headline if the "
            "handler is later re-tuned — refer to handler constants for the "
            "ground truth."
        ),
        payload={
            "session_active": True,
            "elapsed_seconds": 754,
            "kills": 100,
            "cost_ped": 152.0,
            "returns_ped": 159.9,
            "net_ped": 7.9,
            "return_rate_pct": 105.2,
            "current_mob_species": "Caboria",
            "current_mob_maturity": "Old",
            "current_tool_name": "Korss H400",
            "last_loot_ped": 0.80,
            "pes_ped": 5.02,
            "dpp": 3.05,
        },
    ),
    LiveInjectionScenario(
        name="overlay_menu_open",
        description="Overlay menu open with mob suggestions populated.",
        payload={
            "menu_variant": "mob",
            "search_query": "ar",
            "suggestions": [
                {"species": "Argonaut", "maturity": "Young"},
                {"species": "Argonaut", "maturity": "Mature"},
                {"species": "Argonaut", "maturity": "Old"},
                {"species": "Atrox", "maturity": "Old"},
            ],
        },
    ),
    LiveInjectionScenario(
        name="skill_scan_in_progress",
        description="Skill-scan capture mid-flight: 7 of 12 pages captured.",
        payload={
            "phase": "capturing",
            "captured_pages": 7,
            "expected_pages": 12,
            "configured": True,
            "game_window_present": True,
        },
    ),
)


# ─── Settings posture ────────────────────────────────────────────────────────

LOOT_FILTER_BLACKLIST = ["Universal Ammo", "Sollomate Aurli", "Healer Energizer"]


# ─── Seeder implementation ───────────────────────────────────────────────────


class CoreSeeder:
    """Writes the foundational substrate and returns CanonicalRefs."""

    name: str = NAME
    depends_on: tuple[str, ...] = ()

    def build_refs(self) -> CanonicalRefs:
        """Build CanonicalRefs in-memory only. DB writes happen in ``seed``.

        Public so the contract / driver can introspect refs before any writes.
        """
        # Core builds the in-memory shape; library_ids and quest db_ids are
        # placeholders here (set to -1) and replaced after DB insert returns
        # the real autoincrement IDs.
        items = tuple(
            ItemRef(
                library_id=-1,
                name=name,
                item_type=item_type,
                catalog_id=None,
                profession=profession,
            )
            for name, item_type, profession, _props in _ITEM_SEEDS
        )

        quests = tuple(
            QuestRef(
                db_id=-1,
                name=q["name"],
                category=q["category"],
                planet=q["planet"],
                mob_names=q["mobs"],
                is_chain=q["chain"] is not None,
                chain_position=q["chain"][1] if q["chain"] else None,
                chain_total=q["chain"][2] if q["chain"] else None,
                reward_is_skill=q["reward_is_skill"],
                reward_ped=q["reward_ped"],
            )
            for q in _QUEST_SEEDS
        )

        playlists = tuple(
            PlaylistRef(
                db_id=-1,
                name=p["name"],
                estimated_minutes=p["estimated_minutes"],
                immediate_quest_ids=(),  # set post-insert
                long_horizon_quest_ids=(),  # set post-insert
            )
            for p in _PLAYLIST_SEEDS
        )

        trifecta = TrifectaPresetRef(
            preset_id=TRIFECTA_PRESET_ID,
            name=TRIFECTA_PRESET_NAME,
            small_weapon_library_id=None,
            big_weapon_library_id=None,
            heal_library_id=None,
        )

        hotbar = tuple(
            HotbarBinding(slot=str(i + 1), library_id=-1)
            for i in range(3)  # slots 1, 2, 3 — bound post-insert
        )
        # Slot 5 = heal, slot 9 = consumable
        hotbar = hotbar + (
            HotbarBinding(slot="5", library_id=-1),
            HotbarBinding(slot="9", library_id=-1),
        )

        return CanonicalRefs(
            character=CHARACTER,
            timeline=TimelineAnchor(
                demo_now=time.time(),
                history_window_days=HISTORY_WINDOW_DAYS,
            ),
            mobs=MOBS,
            skill_names=SKILL_NAMES,
            skill_categories=SKILL_CATEGORIES,
            attribute_names=ATTRIBUTE_NAMES,
            codex_species=CODEX_SPECIES,
            items=items,
            quests=quests,
            playlists=playlists,
            trifecta_preset=trifecta,
            hotbar=hotbar,
            live_scenarios=LIVE_SCENARIOS,
        )

    def seed(
        self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path
    ) -> CanonicalRefs:
        """Write foundational rows + settings.json. Returns refs with real IDs filled.

        Note: returns refs so the driver can replace its frozen reference. The
        Seeder Protocol declares ``-> None`` for type compatibility with
        per-domain seeders that don't mutate refs; the core seeder's return
        type is a deliberate widening the driver special-cases.
        """
        # 1) Equipment library inserts — capture autoincrement IDs back into refs.
        items_with_ids: list[ItemRef] = []
        for _orig, (name, item_type, profession, props_builder) in zip(
            refs.items, _ITEM_SEEDS, strict=False
        ):
            props = (
                props_builder(name)
                if item_type != "weapon"
                else props_builder(name, profession)
            )
            cur = db.execute(
                "INSERT INTO equipment_library (name, item_type, catalog_id, properties_json) "
                "VALUES (?, ?, ?, ?)",
                (name, item_type, None, json.dumps(props)),
            )
            items_with_ids.append(
                ItemRef(
                    library_id=int(cur.lastrowid),
                    name=name,
                    item_type=item_type,
                    catalog_id=None,
                    profession=profession,
                )
            )

        # 2) Quests inserts.
        quests_with_ids: list[QuestRef] = []
        for _orig, qs in zip(refs.quests, _QUEST_SEEDS, strict=False):
            cur = db.execute(
                """
                INSERT INTO quests (
                    name, planet, waypoint, cooldown_hours, reward_ped, reward_is_skill,
                    notes, chain_name, chain_position, chain_total, is_active, category, reward_description
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                """,
                (
                    qs["name"],
                    qs["planet"],
                    qs["waypoint"],
                    qs["cooldown_hours"],
                    qs["reward_ped"],
                    1 if qs["reward_is_skill"] else 0,
                    None,
                    qs["chain"][0] if qs["chain"] else None,
                    qs["chain"][1] if qs["chain"] else None,
                    qs["chain"][2] if qs["chain"] else None,
                    qs["category"],
                    None,
                ),
            )
            qid = int(cur.lastrowid)
            for mob in qs["mobs"]:
                db.execute(
                    "INSERT INTO quest_mobs (quest_id, mob_name) VALUES (?, ?)",
                    (qid, mob),
                )
            quests_with_ids.append(
                QuestRef(
                    db_id=qid,
                    name=qs["name"],
                    category=qs["category"],
                    planet=qs["planet"],
                    mob_names=qs["mobs"],
                    is_chain=qs["chain"] is not None,
                    chain_position=qs["chain"][1] if qs["chain"] else None,
                    chain_total=qs["chain"][2] if qs["chain"] else None,
                    reward_is_skill=qs["reward_is_skill"],
                    reward_ped=qs["reward_ped"],
                )
            )

        # 3) Playlists + items inserts.
        playlists_with_ids: list[PlaylistRef] = []
        for ps in _PLAYLIST_SEEDS:
            cur = db.execute(
                "INSERT INTO quest_playlists (name, planet, estimated_minutes, is_active) "
                "VALUES (?, ?, ?, 1)",
                (ps["name"], ps["planet"], ps["estimated_minutes"]),
            )
            pid = int(cur.lastrowid)

            sort_order = 0
            immediate_ids: list[int] = []
            for qi in ps["immediate_indices"]:
                quest_id = quests_with_ids[qi].db_id
                immediate_ids.append(quest_id)
                db.execute(
                    "INSERT INTO quest_playlist_items (playlist_id, quest_id, sort_order, group_type) "
                    "VALUES (?, ?, ?, 'immediate')",
                    (pid, quest_id, sort_order),
                )
                sort_order += 1

            long_ids: list[int] = []
            for qi in ps["long_horizon_indices"]:
                quest_id = quests_with_ids[qi].db_id
                long_ids.append(quest_id)
                db.execute(
                    "INSERT INTO quest_playlist_items (playlist_id, quest_id, sort_order, group_type) "
                    "VALUES (?, ?, ?, 'long_horizon')",
                    (pid, quest_id, sort_order),
                )
                sort_order += 1

            playlists_with_ids.append(
                PlaylistRef(
                    db_id=pid,
                    name=ps["name"],
                    estimated_minutes=ps["estimated_minutes"],
                    immediate_quest_ids=tuple(immediate_ids),
                    long_horizon_quest_ids=tuple(long_ids),
                )
            )

        # 4) Codex placeholder — rank=0 row for each canonical species. The
        #    codex seeder updates ranks and writes claim history.
        for species in refs.codex_species:
            db.execute(
                "INSERT OR IGNORE INTO codex_progress (species_name, current_rank) VALUES (?, 0)",
                (species,),
            )

        # 5) Resolve hotbar + trifecta library_ids using the now-known equipment IDs.
        item_by_name = {it.name: it for it in items_with_ids}
        hotbar_with_ids = (
            HotbarBinding(
                slot="1", library_id=item_by_name["Emik Enigma L1 (L)"].library_id
            ),
            HotbarBinding(slot="2", library_id=item_by_name["Korss H400"].library_id),
            HotbarBinding(
                slot="3", library_id=item_by_name["Herman CAP-7 Jungle (L)"].library_id
            ),
            HotbarBinding(slot="5", library_id=item_by_name["Vivo T1"].library_id),
            HotbarBinding(slot="9", library_id=item_by_name["H-DNA"].library_id),
        )

        # Trifecta: small weapon (low-tier Blp pistol, ~4-8 dmg), big weapon
        # (mid-tier laser sniper, ~27-55 dmg), heal tool. The damage bands
        # must not overlap so the trifecta validator can attribute kills
        # by signature; Emik Enigma L1 (L) (31-62) was rejected as the
        # small slot because its band overlaps Korss H400 (27.35-54.7),
        # so Jester D-1 takes the small slot for a clean separation.
        trifecta_with_ids = TrifectaPresetRef(
            preset_id=TRIFECTA_PRESET_ID,
            name=TRIFECTA_PRESET_NAME,
            small_weapon_library_id=item_by_name["Jester D-1"].library_id,
            big_weapon_library_id=item_by_name["Korss H400"].library_id,
            heal_library_id=item_by_name["Vivo T1"].library_id,
        )

        # 6) Settings.json.
        self._write_settings(refs, hotbar_with_ids, trifecta_with_ids, data_dir)

        # 7) Manifest: captures the seed run state for downstream tooling / QA.
        # live_scenario_names lists only scenarios with a registered handler;
        # declared-but-unimplemented entries in LIVE_SCENARIOS stay out of
        # the manifest so the bundled artefact never advertises affordances
        # that do not actually wire up at runtime.
        from backend.scripts.demo_seed.live_injection import _SCENARIO_HANDLERS

        manifest = {
            "seed_run_at": time.time(),
            "demo_now": refs.timeline.demo_now,
            "history_window_days": refs.timeline.history_window_days,
            "canonical_version": 1,
            "character_player_name": refs.character.player_name,
            "item_count": len(items_with_ids),
            "quest_count": len(quests_with_ids),
            "playlist_count": len(playlists_with_ids),
            "codex_species_count": len(refs.codex_species),
            "live_scenario_names": [
                s.name for s in refs.live_scenarios if s.name in _SCENARIO_HANDLERS
            ],
        }
        (data_dir / "seed_manifest.json").write_text(
            json.dumps(manifest, indent=2), encoding="utf-8"
        )

        # Return a refs replacement with the IDs filled.
        return CanonicalRefs(
            character=refs.character,
            timeline=refs.timeline,
            mobs=refs.mobs,
            skill_names=refs.skill_names,
            skill_categories=refs.skill_categories,
            attribute_names=refs.attribute_names,
            codex_species=refs.codex_species,
            items=tuple(items_with_ids),
            quests=tuple(quests_with_ids),
            playlists=tuple(playlists_with_ids),
            trifecta_preset=trifecta_with_ids,
            hotbar=hotbar_with_ids,
            live_scenarios=refs.live_scenarios,
        )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []

        # Player name must match the canonical synthetic value; any divergence
        # indicates the seeder constants were edited away from the
        # synthetic-only contract and is reported as a violation.
        if refs.character.player_name != CHARACTER.player_name:
            violations.append(
                f"character.player_name diverged from canonical synthetic value "
                f"(got {refs.character.player_name!r})"
            )

        # Timeline anchor must be within a sane window of seed run time. (A
        # frozen / very old anchor signals a corrupted or replayed seed.)
        now = time.time()
        if abs(refs.timeline.demo_now - now) > 86400:
            violations.append(
                f"timeline.demo_now is more than 24h from current time "
                f"(anchor={refs.timeline.demo_now}, now={now})"
            )

        return violations

    def _write_settings(
        self,
        refs: CanonicalRefs,
        hotbar: tuple[HotbarBinding, ...],
        trifecta: TrifectaPresetRef,
        data_dir: Path,
    ) -> None:
        """Write settings.json matching the AppConfig shape ConfigService expects."""
        # Hotbar shape: dict keyed by all of "1".."9","0", values are ids or None.
        hotbar_by_slot: dict[str, int | None] = {str(i): None for i in range(1, 10)}
        hotbar_by_slot["0"] = None
        for binding in hotbar:
            hotbar_by_slot[binding.slot] = binding.library_id

        config_dict = {
            "chatlog_path": refs.character.chatlog_path,
            "player_name": refs.character.player_name,
            # Demo defaults to trifecta attribution so the guide's overlay-spawn
            # card showcases the trifecta dropdown affordance rather than the
            # static hotbar weapon text. `_weapon_attribution` in the tracking
            # router branches on `not hotbar_hooks_enabled` to return "trifecta".
            "hotbar_hooks_enabled": False,
            "repair_ocr_enabled": True,
            "end_of_session_armour_reminder_enabled": False,
            "mob_tracking_mode": "mob",
            "mob_tracking_tag": "",
            "manual_mob_species": "",
            "manual_mob_maturity": "",
            "hotbar": hotbar_by_slot,
            "trifecta_presets": [
                {
                    "id": trifecta.preset_id,
                    "name": trifecta.name,
                    "small_weapon_id": trifecta.small_weapon_library_id,
                    "big_weapon_id": trifecta.big_weapon_library_id,
                    "heal_id": trifecta.heal_library_id,
                }
            ],
            "active_trifecta_preset_id": trifecta.preset_id,
            "loot_filter_blacklist": list(LOOT_FILTER_BLACKLIST),
            "overlay_x": None,
            "overlay_y": None,
        }
        (data_dir / "settings.json").write_text(
            json.dumps(config_dict, indent=2), encoding="utf-8"
        )
