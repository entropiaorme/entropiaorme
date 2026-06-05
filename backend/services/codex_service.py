"""Codex service — species data, rank breakdowns, claim recording.

Reads species from the bundled game-data catalogue, cross-references
player progress from app_db, and handles claim recording with ledger
+ skill calibration.
"""

import logging

from backend.data.codex_categories import (
    CODEX_SKILL_CATEGORIES,
    build_rank_breakdown,
    get_category_for_rank,
    get_reward_ped,
    is_cat4_rank,
)
from backend.data.tt_value_curve import levels_for_tt_value
from backend.db.app_database import AppDatabase
from backend.services.game_data_store import GameDataStore
from backend.testing.clock import Clock, RealClock

log = logging.getLogger(__name__)


class CodexService:
    """Codex operations: species listing, rank breakdowns, claim recording."""

    def __init__(
        self,
        app_db: AppDatabase,
        game_data: GameDataStore,
        clock: Clock | None = None,
    ):
        self._app_db = app_db
        self._game_data = game_data
        # Time source for claim/progress timestamps; injected so replay
        # scenarios stamp deterministic instants. Defaults to the real clock.
        self._clock = clock or RealClock()

    # ── Species listing ─────────────────────────────────────────────────────

    def get_all_species(self) -> list[dict]:
        """Return all mob species with codex_base_cost, cross-reffed with player rank."""
        mobs = self._game_data.get_entities("mobs")
        # Deduplicate by species name
        species_map: dict[str, dict] = {}
        for mob in mobs:
            species = mob.get("species")
            if not species:
                continue
            name = species.get("name", "")
            if not name or name in species_map:
                continue
            base_cost = species.get("codex_base_cost")
            if base_cost is None:
                continue
            codex_type = species.get("codex_type")
            species_map[name] = {
                "name": name,
                "baseCost": base_cost,
                "codexType": codex_type,
            }

        # Cross-ref with player progress
        rows = self._app_db.conn.execute(
            "SELECT species_name, current_rank FROM codex_progress"
        ).fetchall()
        rank_map = {r["species_name"]: r["current_rank"] for r in rows}

        result = []
        for sp in species_map.values():
            rank = rank_map.get(sp["name"], 0)
            next_rank = rank + 1 if rank < 25 else None
            next_category = get_category_for_rank(next_rank) if next_rank else None
            next_cost = None
            if next_rank:
                from backend.data.codex_categories import get_rank_cost

                next_cost = round(get_rank_cost(next_rank, sp["baseCost"]), 2)

            result.append(
                {
                    "name": sp["name"],
                    "baseCost": sp["baseCost"],
                    "codexType": sp["codexType"],
                    "currentRank": rank,
                    "nextRank": next_rank,
                    "nextCategory": next_category,
                    "nextCost": next_cost,
                }
            )

        # Sort: rank desc then name asc
        result.sort(key=lambda s: (-s["currentRank"], s["name"]))
        return result

    # ── Rank breakdown ──────────────────────────────────────────────────────

    def get_species_ranks(self, species_name: str) -> dict | None:
        """Return 25-rank breakdown for a species, cross-reffed with claims."""
        species = self._find_species(species_name)
        if species is None:
            return None

        breakdown = build_rank_breakdown(species["baseCost"], species["codexType"])

        # Get existing claims for this species
        claims = self._app_db.conn.execute(
            "SELECT rank, skill_name, ped_value, claimed_at FROM codex_claims WHERE species_name = ? ORDER BY rank",
            (species_name,),
        ).fetchall()
        claims_map = {r["rank"]: dict(r) for r in claims}

        # Get current rank
        row = self._app_db.conn.execute(
            "SELECT current_rank FROM codex_progress WHERE species_name = ?",
            (species_name,),
        ).fetchone()
        current_rank = row["current_rank"] if row else 0

        for item in breakdown:
            claim = claims_map.get(item["rank"])
            item["claimed"] = claim is not None
            item["claimedSkill"] = claim["skill_name"] if claim else None
            item["claimedPed"] = claim["ped_value"] if claim else None
            item["isNext"] = item["rank"] == current_rank + 1

        return {
            "speciesName": species_name,
            "baseCost": species["baseCost"],
            "codexType": species["codexType"],
            "currentRank": current_rank,
            "ranks": breakdown,
        }

    # ── Claim ───────────────────────────────────────────────────────────────

    def claim_rank(self, species_name: str, rank: int, skill_name: str) -> dict:
        """Claim a codex rank reward. Validates, records, updates calibration and ledger."""
        species = self._find_species(species_name)
        if species is None:
            raise ValueError(
                f"Species '{species_name}' not found in game-data catalogue"
            )

        # Validate rank is next
        row = self._app_db.conn.execute(
            "SELECT current_rank FROM codex_progress WHERE species_name = ?",
            (species_name,),
        ).fetchone()
        current_rank = row["current_rank"] if row else 0
        if rank != current_rank + 1:
            raise ValueError(f"Expected rank {current_rank + 1}, got {rank}")
        if rank > 25:
            raise ValueError("Maximum rank is 25")

        # Determine category and reward
        category = get_category_for_rank(rank)
        cat4 = is_cat4_rank(rank, species["codexType"])

        # Check if skill is valid for this category
        valid_skills = set(CODEX_SKILL_CATEGORIES[category])
        if cat4:
            valid_skills |= set(CODEX_SKILL_CATEGORIES["cat4"])
        if skill_name not in valid_skills:
            raise ValueError(
                f"Skill '{skill_name}' not valid for rank {rank} (category {category})"
            )

        # Compute reward — cat4 skills use cat4 divisor
        if skill_name in CODEX_SKILL_CATEGORIES.get("cat4", []):
            ped_value = get_reward_ped(rank, species["baseCost"], "cat4")
        else:
            ped_value = get_reward_ped(rank, species["baseCost"], category)

        now = self._clock.now().timestamp()

        with self._app_db.lock:
            # Insert claim — codex_claims is the canonical record for codex PES.
            self._app_db.conn.execute(
                "INSERT INTO codex_claims (species_name, rank, skill_name, ped_value, claimed_at, kind) "
                "VALUES (?, ?, ?, ?, ?, 'rank')",
                (species_name, rank, skill_name, ped_value, now),
            )

            # Upsert codex_progress
            self._app_db.conn.execute(
                "INSERT INTO codex_progress (species_name, current_rank, updated_at) VALUES (?, ?, ?) "
                "ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ?",
                (species_name, rank, now, rank, now),
            )

            # Update skill calibration: add levels from TT reward
            current_level = self._get_skill_level(skill_name)
            if current_level is not None:
                levels_gained = levels_for_tt_value(current_level, ped_value)
                new_level = current_level + levels_gained
                self._app_db.conn.execute(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'codex', ?)",
                    (skill_name, new_level, now),
                )

            self._app_db.conn.commit()

        log.info(
            "Codex claim: %s rank %d → %s (%.4f PES)",
            species_name,
            rank,
            skill_name,
            ped_value,
        )
        return {
            "speciesName": species_name,
            "rank": rank,
            "skillName": skill_name,
            "pedValue": ped_value,
        }

    # ── Calibrate (direct rank set) ────────────────────────────────────────

    def calibrate(self, species_name: str, rank: int) -> dict:
        """Set codex rank directly, no side effects. For manual calibration."""
        if rank < 0 or rank > 25:
            raise ValueError("Rank must be 0-25")
        now = self._clock.now().timestamp()
        self._app_db.conn.execute(
            "INSERT INTO codex_progress (species_name, current_rank, updated_at) VALUES (?, ?, ?) "
            "ON CONFLICT(species_name) DO UPDATE SET current_rank = ?, updated_at = ?",
            (species_name, rank, now, rank, now),
        )
        self._app_db.conn.commit()
        return {"speciesName": species_name, "rank": rank}

    # ── Skill options for a rank ────────────────────────────────────────────

    def get_skill_options(
        self,
        species_name: str,
        rank: int,
        profession: str | None = None,
        target: str = "profession",
    ) -> list[dict]:
        """Return skill choices for a rank, ranked by profession or HP contribution.

        For each skill option, computes:
        - currentLevel: player's calibrated level
        - levelsGained: how many levels the reward PED buys at that level
        - professionWeight: raw weight in the selected profession
        - profContribution: levelsGained × weight / 10000 (actual profession level points)
        - hpIncrease: skill levels needed to gain 1 HP
        - hpGain: HP gained from this reward at the player's current level

        This accounts for diminishing returns: a low-weight skill at a low level
        can contribute more profession progress than a high-weight skill at a high
        level, because the same PED buys more levels earlier on the TT curve.
        """
        species = self._find_species(species_name)
        if species is None:
            return []

        category = get_category_for_rank(rank)
        cat4 = is_cat4_rank(rank, species["codexType"] if species else None)

        # Collect skill names and their reward PED
        skill_entries: list[tuple[str, str, float]] = []  # (name, cat, ped)
        for skill_name in CODEX_SKILL_CATEGORIES[category]:
            ped = get_reward_ped(rank, species["baseCost"], category)
            skill_entries.append((skill_name, category, ped))

        if cat4:
            for skill_name in CODEX_SKILL_CATEGORIES["cat4"]:
                ped = get_reward_ped(rank, species["baseCost"], "cat4")
                skill_entries.append((skill_name, "cat4", ped))

        # Look up profession weights if specified
        weight_map: dict[str, int] = {}
        if profession:
            prof_data = self._game_data.get_entities("professions")
            for p in prof_data:
                if p.get("name") == profession:
                    for se in p.get("skills", []):
                        skill_obj = se.get("skill") or {}
                        name = skill_obj.get("name", "")
                        weight = se.get("weight") or 0
                        if name:
                            weight_map[name] = weight
                    break

        skill_entities = self._game_data.get_entities("skills")
        hp_map: dict[str, float] = {}
        for skill in skill_entities:
            name = skill.get("name", "")
            hp_increase = skill.get("hp_increase")
            if name:
                hp_map[name] = float(hp_increase) if hp_increase is not None else 0.0

        # Build result with contribution analysis
        skills: list[dict] = []
        for skill_name, cat, ped in skill_entries:
            current_level = self._get_skill_level(skill_name)
            levels_gained = levels_for_tt_value(current_level or 0, ped)
            weight = weight_map.get(skill_name, 0)
            prof_contrib = (
                round(levels_gained * weight / 10000, 6) if weight > 0 else 0.0
            )
            hp_increase = hp_map.get(skill_name, 0.0)
            hp_gain = round(levels_gained / hp_increase, 6) if hp_increase > 0 else 0.0

            skills.append(
                {
                    "skillName": skill_name,
                    "category": cat,
                    "rewardPed": ped,
                    "currentLevel": round(current_level, 1)
                    if current_level is not None
                    else None,
                    "levelsGained": round(levels_gained, 2),
                    "professionWeight": weight,
                    "profContribution": prof_contrib,
                    "hpIncrease": round(hp_increase, 2) if hp_increase > 0 else None,
                    "hpGain": hp_gain,
                }
            )

        if target == "hp":
            # Sort: highest HP gain first, then lower current level, then name
            skills.sort(
                key=lambda s: (
                    -s["hpGain"],
                    s["currentLevel"]
                    if s["currentLevel"] is not None
                    else float("inf"),
                    s["skillName"],
                )
            )
        else:
            # Sort: highest profession contribution first, then weight, then name
            skills.sort(
                key=lambda s: (
                    -s["profContribution"],
                    -s["professionWeight"],
                    s["skillName"],
                )
            )

        # Add 1-based rank for skills that are relevant to the active optimisation target
        rank_counter = 0
        for s in skills:
            relevant = s["hpGain"] > 0 if target == "hp" else s["professionWeight"] > 0
            if relevant:
                rank_counter += 1
                s["recommendRank"] = rank_counter
            else:
                s["recommendRank"] = None

        return skills

    # ── Meta codex (attribute rewards) ────────────────────────────────────

    ATTRIBUTES = {"Agility", "Health", "Intelligence", "Psyche", "Stamina", "Strength"}
    META_PED = 1.0

    def meta_claim(self, attribute_name: str) -> dict:
        """Claim a meta codex reward (1 PES into an attribute).

        Meta rewards are earned every 5 mob codex ranks. Always 1 PES,
        always into an attribute. No calibration update (no attribute curve).
        Persisted in codex_claims with kind='meta' alongside rank claims;
        species_name and skill_name carry sentinel/denormalised values to
        satisfy the existing NOT NULL constraints without requiring a
        table rebuild.
        """
        if attribute_name not in self.ATTRIBUTES:
            raise ValueError(
                f"'{attribute_name}' is not an attribute. Valid: {sorted(self.ATTRIBUTES)}"
            )

        now = self._clock.now().timestamp()

        self._app_db.conn.execute(
            "INSERT INTO codex_claims "
            "(species_name, rank, skill_name, ped_value, claimed_at, kind, attribute_name) "
            "VALUES ('__meta__', 0, ?, ?, ?, 'meta', ?)",
            (attribute_name, self.META_PED, now, attribute_name),
        )
        self._app_db.conn.commit()

        log.info("Codex meta claim: %s (1.00 PES)", attribute_name)
        return {
            "attributeName": attribute_name,
            "pedValue": self.META_PED,
        }

    def get_meta_attributes(self) -> list[dict]:
        """Return the 6 attributes with current calibrated levels."""
        result = []
        for attr in sorted(self.ATTRIBUTES):
            level = self._get_skill_level(attr)
            result.append(
                {
                    "name": attr,
                    "currentLevel": round(level, 1) if level is not None else None,
                }
            )
        return result

    # ── Private helpers ─────────────────────────────────────────────────────

    def _find_species(self, species_name: str) -> dict | None:
        """Look up species base_cost and codex_type from the game-data catalogue."""
        mobs = self._game_data.get_entities("mobs")
        for mob in mobs:
            species = mob.get("species")
            if not species:
                continue
            if species.get("name") == species_name:
                base_cost = species.get("codex_base_cost")
                if base_cost is None:
                    return None
                return {
                    "name": species_name,
                    "baseCost": base_cost,
                    "codexType": species.get("codex_type"),
                }
        return None

    def _get_skill_level(self, skill_name: str) -> float | None:
        """Get latest calibrated skill level."""
        with self._app_db.lock:
            row = self._app_db.conn.execute(
                "SELECT level FROM skill_calibrations WHERE skill_name = ? ORDER BY scanned_at DESC LIMIT 1",
                (skill_name,),
            ).fetchone()
        return float(row["level"]) if row else None
