"""Seeder contract — Protocol and CanonicalRefs every per-domain seeder honours.

Each per-domain seeder implements one ``Seeder``, declares its name and the
names of the seeders it depends on; the driver topologically orders the run,
passing the same ``CanonicalRefs`` (built by the core seeder) to every
seeder so cross-domain identifiers stay consistent.

Read CanonicalRefs to know which mob species exist, which weapons exist in the
equipment library, which quest IDs are valid, etc. Do NOT invent reference
data; per-domain seeders may invent *interactions with* references (kills
against canonical mobs, skill_calibration rows for canonical skills, ledger
entries tagged against canonical sources) but never new canonical references.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol, runtime_checkable


# Canonical brand of synthetic data: a fictional avatar in a fictional career
# with fictional numbers. Per-domain seeders honour the synthetic-only
# contract; every value they write derives from these canonical refs, not
# from live tracker / library / ledger sources.
@dataclass(frozen=True)
class CharacterProfile:
    player_name: str
    chatlog_path: str  # plausible Windows path; file may or may not exist
    theme: str  # "dark" or "light"


@dataclass(frozen=True)
class TimelineAnchor:
    demo_now: float  # epoch seconds; "now" for the synthetic career
    history_window_days: int  # how far back synthetic history extends


@dataclass(frozen=True)
class MobRef:
    species: str  # canonical species name (real EU lore name; public game data)
    maturities: tuple[str, ...]  # ordered list of maturity tiers used in this career


@dataclass(frozen=True)
class ItemRef:
    library_id: (
        int  # row ID assigned by core seeder when it INSERTs into equipment_library
    )
    name: str
    item_type: str  # "weapon" | "armour" | "healing" | "consumable" | "amp" | "scope" | "absorber"
    catalog_id: str | None  # game-data catalog reference, when resolved
    profession: (
        str | None
    )  # for weapons: the profession this serves (Laser, Blaster, Melee, etc.)


@dataclass(frozen=True)
class QuestRef:
    db_id: int  # row ID assigned by core seeder when it INSERTs into quests
    name: str
    category: str
    planet: str
    mob_names: tuple[str, ...]  # references into MobRef.species
    is_chain: bool
    chain_position: int | None  # 1-indexed within chain
    chain_total: int | None
    reward_is_skill: bool
    reward_ped: float


@dataclass(frozen=True)
class PlaylistRef:
    db_id: int
    name: str
    estimated_minutes: int
    immediate_quest_ids: tuple[int, ...]
    long_horizon_quest_ids: tuple[int, ...]


@dataclass(frozen=True)
class TrifectaPresetRef:
    preset_id: str  # the same id stored in settings.json
    name: str
    small_weapon_library_id: int | None
    big_weapon_library_id: int | None
    heal_library_id: int | None


@dataclass(frozen=True)
class HotbarBinding:
    slot: str  # "1".."9", "0"
    library_id: int


@dataclass(frozen=True)
class LiveInjectionScenario:
    """Mock state injectable into HuntTracker for live in-flight overlay shots.

    Used by ``live_injection.py`` (dev-gated) to pre-populate tracker state.
    """

    name: str  # "mid_hunt" | "overlay_menu_open" | "skill_scan_in_progress"
    description: str
    payload: dict  # opaque to contract; consumer schema defined by live_injection.py


@dataclass(frozen=True)
class CanonicalRefs:
    """The canonical reference list. Built by the core seeder; read-only thereafter.

    Per-domain seeders consume this to keep their output mutually consistent.
    """

    character: CharacterProfile
    timeline: TimelineAnchor

    # Reference data (no DB write of these as standalone tables; consumed by domain seeders)
    mobs: tuple[MobRef, ...]
    skill_names: tuple[
        str, ...
    ]  # canonical subset of game-data skills used by this career
    skill_categories: dict[
        str, tuple[str, ...]
    ]  # category -> skill_names belonging to it
    attribute_names: tuple[str, ...]  # the 6 EU attributes
    codex_species: tuple[str, ...]  # subset of MobRef.species exposed in codex (~20-30)

    # Reference data the core seeder also writes to DB (so per-domain seeders use the IDs)
    items: tuple[ItemRef, ...]
    quests: tuple[QuestRef, ...]
    playlists: tuple[PlaylistRef, ...]
    trifecta_preset: TrifectaPresetRef
    hotbar: tuple[HotbarBinding, ...]

    # Live in-flight surface scenarios (for the dev-gated live-injection path)
    live_scenarios: tuple[LiveInjectionScenario, ...]


@runtime_checkable
class Seeder(Protocol):
    """Per-domain seeder contract. One implementation per domain.

    Every seeder MUST:
    - Declare a unique ``name`` (used for logging + dependency declaration).
    - Declare ``depends_on``: names of other seeders this one reads from. The
      core seeder always has name ``"core"`` and is implicitly first; declare
      it in ``depends_on`` if your seeder reads CanonicalRefs (almost always).
    - Implement ``seed``: write rows to the demo DB and/or settings, using
      only canonical references from ``refs``. Do NOT invent new mob species,
      new skill names, new equipment, new quests; pull them from ``refs``.
    - Implement ``validate_synthetic_data``: return a list of human-readable
      violations (empty list = clean). The driver aborts if any seeder
      returns violations.
    """

    name: str
    depends_on: tuple[str, ...]

    def seed(
        self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path
    ) -> None: ...

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]: ...


@dataclass
class SeedRunReport:
    """Summary of a single ``driver.run`` invocation."""

    data_dir: Path
    seeders_run: list[str] = field(default_factory=list)
    rows_written: dict[str, int] = field(
        default_factory=dict
    )  # table -> row count delta
    violations: list[str] = field(
        default_factory=list
    )  # synthetic-data violations (if any)
    demo_now: float = 0.0  # the timeline anchor used
