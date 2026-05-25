"""Codex category data — skill categories, reward divisors, rank multipliers."""

from __future__ import annotations

REWARD_DIVISORS: dict[str, int] = {
    "cat1": 200,
    "cat2": 320,
    "cat3": 640,
    "cat4": 1000,
}

# Rank 1-25 kill-cost multipliers (index 0 = rank 1)
CODEX_MULTIPLIERS: list[int] = [
    1,
    2,
    3,
    4,
    6,
    8,
    10,
    12,
    14,
    16,
    18,
    20,
    24,
    28,
    32,
    36,
    40,
    44,
    48,
    56,
    64,
    72,
    80,
    90,
    100,
]

CODEX_SKILL_CATEGORIES: dict[str, list[str]] = {
    "cat1": [
        "Aim",
        "Anatomy",
        "Athletics",
        "BLP Weaponry Technology",
        "Combat Reflexes",
        "Dexterity",
        "Handgun",
        "Heavy Melee Weapons",
        "Laser Weaponry Technology",
        "Light Melee Weapons",
        "Longblades",
        "Power Fist",
        "Rifle",
        "Shortblades",
        "Weapons Handling",
    ],
    "cat2": [
        "Clubs",
        "Courage",
        "Cryogenics",
        "Diagnosis",
        "Electrokinesis",
        "Inflict Melee Damage",
        "Inflict Ranged Damage",
        "Melee Combat",
        "Perception",
        "Plasma Weaponry Technology",
        "Pyrokinesis",
    ],
    "cat3": [
        "Alertness",
        "Bioregenesis",
        "Bravado",
        "Concentration",
        "Dodge",
        "Evade",
        "First Aid",
        "Telepathy",
        "Translocation",
        "Vehicle Repairing",
    ],
    "cat4": [
        "Analysis",
        "Animal Lore",
        "Biology",
        "Botany",
        "Computer",
        "Explosive Projectile Weaponry Technology",
        "Heavy Weapons",
        "Support Weapon Systems",
        "Zoology",
    ],
}


def get_codex_category(skill_name: str) -> str | None:
    """Return the codex category key for a skill, or None if not in codex."""
    for cat, skills in CODEX_SKILL_CATEGORIES.items():
        if skill_name in skills:
            return cat
    return None


def get_category_for_rank(rank: int) -> str:
    """Return the codex category for a given rank (1-25).

    Mod-5 cycling: ranks 1,2→cat1; 3,4→cat2; 5→cat3; repeats.
    """
    mod = rank % 5
    if mod in (1, 2):
        return "cat1"
    if mod in (3, 4):
        return "cat2"
    return "cat3"


def is_cat4_rank(rank: int, codex_type: str | None) -> bool:
    """True when the rank offers a cat4 bonus skill choice.

    Cat4 bonus on ranks 5, 15, 25 — only for MobLooter codex types.
    """
    return codex_type == "MobLooter" and rank % 10 == 5


def get_rank_cost(rank: int, base_cost: float) -> float:
    """Total kill cost to reach a rank: multiplier × base_cost."""
    return CODEX_MULTIPLIERS[rank - 1] * base_cost


def get_reward_ped(rank: int, base_cost: float, category: str) -> float:
    """Skill reward in PED for claiming a rank.

    Formula: (multiplier × base_cost) / divisor
    """
    cost = get_rank_cost(rank, base_cost)
    divisor = REWARD_DIVISORS[category]
    return round(cost / divisor, 4)


def build_rank_breakdown(base_cost: float, codex_type: str | None) -> list[dict]:
    """Build a 25-item list with all derived fields per rank.

    Returns list of dicts with: rank, category, cost, reward_ped,
    cat4_bonus (bool), cat4_reward_ped (float|None), skills (list[str]).
    """
    result = []
    for rank in range(1, 26):
        category = get_category_for_rank(rank)
        cost = get_rank_cost(rank, base_cost)
        reward = get_reward_ped(rank, base_cost, category)
        cat4 = is_cat4_rank(rank, codex_type)
        cat4_reward = get_reward_ped(rank, base_cost, "cat4") if cat4 else None

        result.append(
            {
                "rank": rank,
                "category": category,
                "cost": round(cost, 2),
                "rewardPed": reward,
                "cat4Bonus": cat4,
                "cat4RewardPed": cat4_reward,
                "skills": list(CODEX_SKILL_CATEGORIES[category]),
                "cat4Skills": list(CODEX_SKILL_CATEGORIES["cat4"]) if cat4 else [],
            }
        )
    return result
