//! Codex category data: skill categories, reward divisors, rank
//! multipliers, and the per-rank
//! breakdown builder whose camelCase wire shape feeds the codex
//! responses.

use serde::Serialize;

use eo_wire::normalizer::round_half_even;

pub fn reward_divisor(category: &str) -> Option<i64> {
    match category {
        "cat1" => Some(200),
        "cat2" => Some(320),
        "cat3" => Some(640),
        "cat4" => Some(1000),
        _ => None,
    }
}

/// Rank 1-25 kill-cost multipliers (index 0 = rank 1).
pub const CODEX_MULTIPLIERS: [i64; 25] = [
    1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 36, 40, 44, 48, 56, 64, 72, 80, 90, 100,
];

pub const CAT1_SKILLS: &[&str] = &[
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
];

pub const CAT2_SKILLS: &[&str] = &[
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
];

pub const CAT3_SKILLS: &[&str] = &[
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
];

pub const CAT4_SKILLS: &[&str] = &[
    "Analysis",
    "Animal Lore",
    "Biology",
    "Botany",
    "Computer",
    "Explosive Projectile Weaponry Technology",
    "Heavy Weapons",
    "Support Weapon Systems",
    "Zoology",
];

pub fn skills_for_category(category: &str) -> Option<&'static [&'static str]> {
    match category {
        "cat1" => Some(CAT1_SKILLS),
        "cat2" => Some(CAT2_SKILLS),
        "cat3" => Some(CAT3_SKILLS),
        "cat4" => Some(CAT4_SKILLS),
        _ => None,
    }
}

/// The codex category key for a skill, or None when the skill is not in
/// the codex. Searched in category order, exactly as the backend's dict
/// iteration does.
pub fn get_codex_category(skill_name: &str) -> Option<&'static str> {
    ["cat1", "cat2", "cat3", "cat4"]
        .into_iter()
        .find(|category| {
            skills_for_category(category)
                .expect("known category")
                .contains(&skill_name)
        })
}

/// The codex category for a rank (1-25): mod-5 cycling, ranks 1,2 ->
/// cat1; 3,4 -> cat2; 5 -> cat3; repeats.
pub fn get_category_for_rank(rank: i64) -> &'static str {
    match rank % 5 {
        1 | 2 => "cat1",
        3 | 4 => "cat2",
        _ => "cat3",
    }
}

/// True when the rank offers a cat4 bonus skill choice: ranks 5, 15, 25,
/// only for MobLooter codex types.
pub fn is_cat4_rank(rank: i64, codex_type: Option<&str>) -> bool {
    codex_type == Some("MobLooter") && rank % 10 == 5
}

/// Total kill cost to reach a rank: multiplier x base_cost.
pub fn get_rank_cost(rank: i64, base_cost: f64) -> f64 {
    CODEX_MULTIPLIERS[(rank - 1) as usize] as f64 * base_cost
}

/// Skill reward in PED for claiming a rank:
/// (multiplier x base_cost) / divisor.
pub fn get_reward_ped(rank: i64, base_cost: f64, category: &str) -> f64 {
    let cost = get_rank_cost(rank, base_cost);
    let divisor = reward_divisor(category).expect("known category") as f64;
    round_half_even(cost / divisor, 4)
}

/// Ranks between meta rewards: every species pays one meta reward at
/// ranks 5, 10, 15, 20, and 25.
pub const META_INTERVAL: i64 = 5;

/// The most meta rewards a character can claim across all species.
pub const META_CAP: i64 = 750;

/// The planning rate that turns codex cost into PED cycled. Codex
/// progress follows the work done on a species rather than its kill
/// count: on tracked history, kills per cost unit varied about seventy
/// fold across species while PED cycled per cost unit stayed between
/// about 1.2 and 2.8 (one rank window logged live measured 1.16; the
/// others are upper bounds). The rate is a modelled estimate for
/// comparing targets, never an accounting figure.
pub const ESTIMATED_PED_PER_CODEX_COST: f64 = 1.5;

/// The meta breakpoint a species at `rank` reaches next, or `None` once
/// rank 25 (the last breakpoint) is reached. A rank exactly on a
/// breakpoint has already earned that meta, so the next one is five on.
pub fn next_meta_rank(rank: i64) -> Option<i64> {
    let rank = rank.max(0);
    (rank < 25).then(|| (rank / META_INTERVAL + 1) * META_INTERVAL)
}

/// The meta rewards a species at `rank` has earned (one per breakpoint
/// passed, clamped to the 0-25 rank domain).
pub fn metas_earned(rank: i64) -> i64 {
    rank.clamp(0, 25) / META_INTERVAL
}

/// The codex cost of advancing from `from_rank` to `to_rank`: the sum of
/// the cost of every rank in between, `from_rank` exclusive.
pub fn cost_between_ranks(from_rank: i64, to_rank: i64, base_cost: f64) -> f64 {
    (from_rank.max(0) + 1..=to_rank.min(25))
        .map(|rank| get_rank_cost(rank, base_cost))
        .sum()
}

/// The mastery reward's fixed cost basis. The in-game mastery skill
/// picks (observed values 25 / 15.62 / 7.81 PED, display-rounded)
/// decode exactly as 5000 divided by the skill's category divisor
/// (5000/200, 5000/320, 5000/640), so the reward derives from the
/// same divisors as ranks 1-25 with this species-independent cost.
pub const MASTERY_COST: f64 = 5000.0;

/// The categories whose skills are mastery-eligible. Cat4 skills were
/// absent from the observed in-game mastery picker; if a species ever
/// offers them, they would price through the cat4 divisor (5 PED).
pub const MASTERY_CATEGORIES: [&str; 3] = ["cat1", "cat2", "cat3"];

/// The fixed mastery reward for a skill: `MASTERY_COST` over the
/// skill's category divisor. `None` when the skill is not
/// mastery-eligible (cat4 skills and skills outside the codex).
pub fn mastery_reward_ped(skill_name: &str) -> Option<f64> {
    let category = get_codex_category(skill_name)?;
    if !MASTERY_CATEGORIES.contains(&category) {
        return None;
    }
    let divisor = reward_divisor(category).expect("known category") as f64;
    Some(round_half_even(MASTERY_COST / divisor, 4))
}

/// One rank's derived fields, serialising to the exact camelCase wire
/// shape the backend's breakdown dicts carry, in the same key order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RankBreakdown {
    pub rank: i64,
    pub category: &'static str,
    pub cost: f64,
    #[serde(rename = "rewardPed")]
    pub reward_ped: f64,
    #[serde(rename = "cat4Bonus")]
    pub cat4_bonus: bool,
    #[serde(rename = "cat4RewardPed")]
    pub cat4_reward_ped: Option<f64>,
    pub skills: Vec<String>,
    #[serde(rename = "cat4Skills")]
    pub cat4_skills: Vec<String>,
}

/// The 25-rank breakdown with all derived fields per rank.
pub fn build_rank_breakdown(base_cost: f64, codex_type: Option<&str>) -> Vec<RankBreakdown> {
    (1..=25)
        .map(|rank| {
            let category = get_category_for_rank(rank);
            let cost = get_rank_cost(rank, base_cost);
            let reward = get_reward_ped(rank, base_cost, category);
            let cat4 = is_cat4_rank(rank, codex_type);
            RankBreakdown {
                rank,
                category,
                cost: round_half_even(cost, 2),
                reward_ped: reward,
                cat4_bonus: cat4,
                cat4_reward_ped: cat4.then(|| get_reward_ped(rank, base_cost, "cat4")),
                skills: skills_for_category(category)
                    .expect("known category")
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                cat4_skills: if cat4 {
                    CAT4_SKILLS.iter().map(|s| s.to_string()).collect()
                } else {
                    Vec::new()
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_lookup_finds_each_category_and_misses_unknowns() {
        assert_eq!(get_codex_category("Aim"), Some("cat1"));
        assert_eq!(get_codex_category("Courage"), Some("cat2"));
        assert_eq!(get_codex_category("Evade"), Some("cat3"));
        assert_eq!(get_codex_category("Zoology"), Some("cat4"));
        assert_eq!(get_codex_category("Fishing Rod Technology"), None);
    }

    #[test]
    fn rank_categories_cycle_mod_five_including_the_zero_branch() {
        assert_eq!(get_category_for_rank(1), "cat1");
        assert_eq!(get_category_for_rank(2), "cat1");
        assert_eq!(get_category_for_rank(3), "cat2");
        assert_eq!(get_category_for_rank(4), "cat2");
        assert_eq!(get_category_for_rank(5), "cat3");
        assert_eq!(get_category_for_rank(6), "cat1");
        assert_eq!(get_category_for_rank(10), "cat3");
        assert_eq!(get_category_for_rank(25), "cat3");
    }

    #[test]
    fn cat4_bonus_gates_on_mob_looter_and_ranks_5_15_25() {
        for rank in [5, 15, 25] {
            assert!(is_cat4_rank(rank, Some("MobLooter")));
            assert!(!is_cat4_rank(rank, Some("Other")));
            assert!(!is_cat4_rank(rank, None));
        }
        assert!(!is_cat4_rank(10, Some("MobLooter")));
        assert!(!is_cat4_rank(20, Some("MobLooter")));
    }

    #[test]
    fn rewards_divide_rank_cost_by_the_category_divisor() {
        // rank 5: multiplier 6; cat3 divisor 640.
        assert_eq!(get_rank_cost(5, 10.0), 60.0);
        assert_eq!(
            get_reward_ped(5, 10.0, "cat3"),
            round_half_even(60.0 / 640.0, 4)
        );
    }

    #[test]
    fn next_meta_rank_steps_to_the_following_breakpoint() {
        assert_eq!(next_meta_rank(0), Some(5));
        assert_eq!(next_meta_rank(4), Some(5));
        // On a breakpoint, that meta is already earned.
        assert_eq!(next_meta_rank(5), Some(10));
        assert_eq!(next_meta_rank(12), Some(15));
        assert_eq!(next_meta_rank(24), Some(25));
        assert_eq!(next_meta_rank(25), None);
        assert_eq!(next_meta_rank(-1), Some(5));
    }

    #[test]
    fn metas_earned_counts_breakpoints_passed() {
        assert_eq!(metas_earned(0), 0);
        assert_eq!(metas_earned(4), 0);
        assert_eq!(metas_earned(5), 1);
        assert_eq!(metas_earned(14), 2);
        assert_eq!(metas_earned(25), 5);
        assert_eq!(metas_earned(-3), 0);
        assert_eq!(metas_earned(40), 5);
    }

    #[test]
    fn meta_band_costs_sum_the_rank_multipliers() {
        // Each five-rank band's multipliers sum to 16, 60, 122, 224, 406
        // (828 over all 25 ranks).
        let bands: Vec<f64> = (0..5)
            .map(|band| cost_between_ranks(band * 5, band * 5 + 5, 1.0))
            .collect();
        assert_eq!(bands, vec![16.0, 60.0, 122.0, 224.0, 406.0]);
        assert_eq!(cost_between_ranks(0, 25, 1.0), 828.0);
        // A partial band: ranks 13 to 15 (multipliers 24, 28, 32).
        assert_eq!(cost_between_ranks(12, 15, 2.5), 210.0);
        assert_eq!(cost_between_ranks(25, 30, 1.0), 0.0);
    }

    #[test]
    fn mastery_rewards_pin_the_observed_per_category_values() {
        // The three in-game value tiers, exact (the game UI shows the
        // 2dp roundings 25 / 15.62 / 7.81).
        assert_eq!(mastery_reward_ped("Aim"), Some(25.0));
        assert_eq!(mastery_reward_ped("Rifle"), Some(25.0));
        assert_eq!(mastery_reward_ped("Melee Combat"), Some(15.625));
        assert_eq!(mastery_reward_ped("Courage"), Some(15.625));
        assert_eq!(mastery_reward_ped("Evade"), Some(7.8125));
        assert_eq!(mastery_reward_ped("First Aid"), Some(7.8125));
    }

    #[test]
    fn mastery_rewards_exclude_cat4_and_unknown_skills() {
        assert_eq!(mastery_reward_ped("Zoology"), None);
        assert_eq!(mastery_reward_ped("Animal Lore"), None);
        assert_eq!(mastery_reward_ped("Fishing Rod Technology"), None);
        assert_eq!(mastery_reward_ped(""), None);
    }

    #[test]
    fn breakdown_carries_the_exact_wire_shape() {
        let breakdown = build_rank_breakdown(10.0, Some("MobLooter"));
        assert_eq!(breakdown.len(), 25);
        let rank5 = &breakdown[4];
        assert!(rank5.cat4_bonus);
        assert_eq!(rank5.cat4_skills.len(), CAT4_SKILLS.len());
        assert_eq!(rank5.cat4_reward_ped, Some(get_reward_ped(5, 10.0, "cat4")));
        let rank1 = &breakdown[0];
        assert!(!rank1.cat4_bonus);
        assert!(rank1.cat4_skills.is_empty());
        assert_eq!(rank1.cat4_reward_ped, None);

        let wire = serde_json::to_string(rank1).unwrap();
        let expected_order = [
            "\"rank\":",
            "\"category\":",
            "\"cost\":",
            "\"rewardPed\":",
            "\"cat4Bonus\":",
            "\"cat4RewardPed\":",
            "\"skills\":",
            "\"cat4Skills\":",
        ];
        let mut last = 0;
        for key in expected_order {
            let pos = wire.find(key).unwrap_or_else(|| panic!("{key} missing"));
            assert!(pos > last || last == 0, "{key} out of order");
            last = pos;
        }
        assert!(wire.contains("\"cat4RewardPed\":null"));
    }
}
