"""Codex seeder — UPDATEs codex_progress ranks + INSERTs codex_claims history.

Mixed demo posture across the 6 canonical species: 2 untouched (rank 0), 2 mid-
rank in-flight, 1 near-complete, 1 codex-complete (rank 25). Each progressed
species gets a full claim trail (ranks 1..N) with codex-eligible skill picks,
PED values that follow the EU reward formula's shape — (multiplier × base_cost)
/ category_divisor — calibrated to the per-species cost tier, and claimed_at
clustered into a believable ~5-15-day grind window per species (rather than a
flat spread) so the timeline reads as discrete grind sessions, not one
continuous trickle. Plus a small batch of meta-attribute claims using the same
'__meta__' sentinel shape that ``CodexService.meta_claim`` writes at runtime.
"""

from __future__ import annotations

import logging
import random
import sqlite3
from pathlib import Path

from backend.scripts.demo_seed.contract import CanonicalRefs, Seeder

log = logging.getLogger(__name__)

_RNG_SEED = 0xC0DEC0DE

# Sentinel species + value matches CodexService.meta_claim so the runtime read
# path treats demo-seeded meta claims identically to live-recorded ones.
_META_SENTINEL_SPECIES = "__meta__"
_META_PED_VALUE = 1.0
_META_BUDGET_DIVISOR = 5  # 1 meta-claim allowance per 5 mob ranks earned
_META_BUDGET_CAP = 4  # cap demo-data meta claims for visual restraint

_SECONDS_PER_DAY = 86400
_CLAIM_HISTORY_DAYS = 60

# Hand-picked per-species rank targets. Covers the full visual range:
#   0       → faded "0/25" in the species list, "Codex untouched" detail
#   1..24   → colour-coded "X/25", next-rank info card with skill recs
#   25      → green "Codex complete" card, no Claim button
_SPECIES_RANKS: dict[str, int] = {
    "Caboria": 12,  # mid in-flight; pairs with the Caboria codex quest chain
    "Atrox": 22,  # near-complete; pairs with Atrox daily/bounty
    "Argonaut": 8,  # mid in-flight (lower)
    "Combibo": 0,  # untouched
    "Daikiba": 25,  # codex complete
    "Snablesnot Male": 0,  # untouched
}

# Per-species cost tier — proxies for codex_base_cost so PED rewards spread
# realistically. Low-tier mobs (Caboria, Combibo) yield smaller per-rank PES;
# high-tier mobs (Atrox, Daikiba) yield more. Numbers are unitless multipliers
# applied on top of the rank-tier band.
_SPECIES_COST_TIER: dict[str, float] = {
    "Caboria": 0.35,
    "Combibo": 0.35,
    "Argonaut": 0.55,
    "Snablesnot Male": 0.50,
    "Atrox": 1.00,
    "Daikiba": 0.75,
}

# Per-species claim-burst windows: (start_days_back, span_days). Models the
# pattern that a player typically grinds one species at a time, so each
# species' claims cluster into a contiguous window rather than being
# interleaved across the full 60-day history. Ordered chronologically:
# Daikiba completed long ago, Atrox grind recent, Argonaut quick burst.
_SPECIES_BURST_WINDOW: dict[str, tuple[float, float]] = {
    "Daikiba": (58.0, 14.0),  # oldest grind, completed
    "Caboria": (40.0, 7.0),  # mid-history, mid-grind paused at rank 12
    "Atrox": (25.0, 12.0),  # recent big grind, paused at rank 22
    "Argonaut": (8.0, 4.0),  # most recent, quick burst to rank 8
}

# Per-species codex-eligible skill pools. Every name MUST appear in BOTH
# refs.skill_names AND backend.data.codex_categories.CODEX_SKILL_CATEGORIES
# (validate_synthetic_data enforces the refs.skill_names side; the codex-
# eligibility side is enforced by hand-picking from the known intersection
# below, since codex_categories is not exposed via refs).
#
# Codex-eligible skills appearing in refs.skill_names:
#   cat1: Aim, Anatomy, Athletics, Combat Reflexes
#   cat2: Diagnosis, Inflict Melee Damage, Inflict Ranged Damage
#   cat3: Bioregenesis, Concentration, Dodge, Evade, First Aid
#   cat4: Computer
#
# Community guides (entropiacodex.com) flag Athletics, Anatomy, Combat
# Reflexes, Evade, Perception (not in refs) as the popular evasion/HP
# picks; pools below skew accordingly.
_SPECIES_SKILL_POOL: dict[str, tuple[str, ...]] = {
    "Caboria": (
        "Athletics",
        "Athletics",
        "Athletics",  # weighted: popular evasion pick
        "Anatomy",
        "Anatomy",
        "Aim",
        "Combat Reflexes",
        "Evade",
    ),
    "Atrox": (
        "Athletics",
        "Athletics",
        "Athletics",
        "Combat Reflexes",
        "Combat Reflexes",
        "Anatomy",
        "Evade",
        "Dodge",
        "Aim",
        "Bioregenesis",
    ),
    "Argonaut": (
        "Athletics",
        "Athletics",
        "Aim",
        "Anatomy",
        "Combat Reflexes",
        "Inflict Ranged Damage",
        "Evade",
    ),
    "Combibo": (),
    "Daikiba": (
        "Athletics",
        "Athletics",
        "Athletics",
        "Athletics",
        "Combat Reflexes",
        "Combat Reflexes",
        "Anatomy",
        "Anatomy",
        "Aim",
        "Evade",
        "Dodge",
        "Diagnosis",
        "First Aid",
        "Bioregenesis",
        "Concentration",
    ),
    "Snablesnot Male": (),
}

# Meta attribute picks — Stamina + Intelligence are the community-recommended
# meta picks (entropiacodex.com); Health rounds out the HP-flavoured trio,
# Agility for variety. Listed in claim order (oldest first).
_META_ATTRIBUTES: tuple[str, ...] = ("Stamina", "Intelligence", "Health", "Agility")


def _ped_for_rank(rank: int, cost_tier: float, rng: random.Random) -> float:
    """PES value of a rank reward.

    Shape derived from the EU formula ``(multiplier × base_cost) / divisor``
    plus typical observed bands (rank 1-5 in the 0.2-1.2 PES range; rank
    6-15 spans 0.5-8.0; rank 16-25 spans 3-25), modulated by per-species
    cost tier so a low-tier Caboria rank-12 yields well below a high-tier
    Atrox rank-12.
    """
    if rank <= 5:
        base = rng.uniform(0.20, 1.20)
    elif rank <= 15:
        base = rng.uniform(0.50, 8.00)
    else:
        base = rng.uniform(3.00, 25.00)
    # ±15% jitter on top of cost-tier scaling to avoid suspiciously smooth curves.
    jitter = rng.uniform(0.85, 1.15)
    return round(base * cost_tier * jitter, 4)


def _rank_claimed_at(
    demo_now: float,
    rank: int,
    total_ranks: int,
    burst: tuple[float, float],
    rng: random.Random,
) -> float:
    """Claim timestamps clustered into the species' grind-burst window.

    burst = (start_days_back, span_days). Rank 1 lands near the start of the
    burst; rank N lands near the end. Mild jitter avoids a perfectly even
    cadence within the burst.
    """
    start_days_back, span_days = burst
    # Linear progression across the burst, normalised to (0..1].
    progression = rank / max(total_ranks, 1)
    days_back = start_days_back - progression * span_days
    # ±0.4 day jitter — keeps ordering mostly correct but breaks artificial regularity.
    days_back += rng.uniform(-0.4, 0.4)
    return demo_now - max(0.05, days_back) * _SECONDS_PER_DAY


class CodexSeeder:
    name: str = "codex"
    depends_on: tuple[str, ...] = ("core",)

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        rng = random.Random(_RNG_SEED)
        demo_now = refs.timeline.demo_now

        rank_claims_written = 0
        progress_updates = 0

        for species in refs.codex_species:
            target_rank = _SPECIES_RANKS.get(species, 0)
            db.execute(
                "UPDATE codex_progress SET current_rank = ?, updated_at = ? "
                "WHERE species_name = ?",
                (target_rank, demo_now, species),
            )
            progress_updates += 1

            if target_rank == 0:
                continue

            pool = _SPECIES_SKILL_POOL.get(species) or refs.skill_names
            cost_tier = _SPECIES_COST_TIER.get(species, 0.5)
            burst = _SPECIES_BURST_WINDOW.get(species, (30.0, 10.0))
            for rank in range(1, target_rank + 1):
                skill = rng.choice(pool)
                ped_value = _ped_for_rank(rank, cost_tier, rng)
                claimed_at = _rank_claimed_at(demo_now, rank, target_rank, burst, rng)
                db.execute(
                    "INSERT INTO codex_claims "
                    "(species_name, rank, skill_name, ped_value, claimed_at, "
                    " kind, attribute_name) "
                    "VALUES (?, ?, ?, ?, ?, 'rank', NULL)",
                    (species, rank, skill, ped_value, claimed_at),
                )
                rank_claims_written += 1

        # Meta claims — sentinel matches CodexService.meta_claim's INSERT shape.
        total_ranks_earned = sum(_SPECIES_RANKS.values())
        meta_budget = total_ranks_earned // _META_BUDGET_DIVISOR
        meta_count = min(meta_budget, _META_BUDGET_CAP, len(_META_ATTRIBUTES))

        meta_claims_written = 0
        for i in range(meta_count):
            attr = _META_ATTRIBUTES[i]
            # Stagger meta claims across the trailing ~20 days; oldest first.
            slot_seconds = (20 * _SECONDS_PER_DAY) / max(meta_count, 1)
            claimed_at = (
                demo_now
                - (meta_count - i) * slot_seconds
                - rng.uniform(0, _SECONDS_PER_DAY)
            )
            db.execute(
                "INSERT INTO codex_claims "
                "(species_name, rank, skill_name, ped_value, claimed_at, "
                " kind, attribute_name) "
                "VALUES (?, 0, ?, ?, ?, 'meta', ?)",
                (_META_SENTINEL_SPECIES, attr, _META_PED_VALUE, claimed_at, attr),
            )
            meta_claims_written += 1

        log.info(
            "codex seeder: %d codex_progress UPDATEs, %d rank claims + %d meta claims.",
            progress_updates,
            rank_claims_written,
            meta_claims_written,
        )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []

        if len(refs.codex_species) < 5:
            violations.append(
                f"refs.codex_species too small ({len(refs.codex_species)} < 5)"
            )
        if len(refs.skill_names) < 25:
            violations.append(
                f"refs.skill_names too small ({len(refs.skill_names)} < 25)"
            )

        canonical_species = set(refs.codex_species)
        for species in _SPECIES_RANKS:
            if species not in canonical_species:
                violations.append(
                    f"_SPECIES_RANKS references unknown species {species!r}"
                )

        canonical_skills = set(refs.skill_names)
        for species, skills in _SPECIES_SKILL_POOL.items():
            for skill in skills:
                if skill not in canonical_skills:
                    violations.append(
                        f"_SPECIES_SKILL_POOL[{species!r}] uses non-canonical "
                        f"skill {skill!r}"
                    )

        canonical_attrs = set(refs.attribute_names)
        for attr in _META_ATTRIBUTES:
            if attr not in canonical_attrs:
                violations.append(
                    f"_META_ATTRIBUTES uses non-canonical attribute {attr!r}"
                )

        return violations


SEEDER: Seeder = CodexSeeder()


# Self-test entry point — runs core + this seeder against a temp dir.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_codex_"))
    try:
        report = run(tmp, extra_seeders=[SEEDER])
        print(format_report(report))
        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
