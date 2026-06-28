//! Damage-based weapon attribution for configured trifecta profiles,
//! ported from the original Python implementation.
//!
//! Each configured weapon carries a damage band; a combat hit
//! attributes to the narrowest band containing its amount (name as the
//! tiebreak). Critical hits widen the bands by the critical
//! multipliers, with one known pattern preferred: when the small
//! weapon could explain the hit as a critical, a big-weapon regular
//! hit explains it better.

pub const CRITICAL_DAMAGE_MIN: f64 = 2.0;
pub const CRITICAL_DAMAGE_MAX: f64 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DamageAttribution {
    pub tool_name: String,
    pub cost_per_shot: f64,
}

#[derive(Debug, Clone)]
struct WeaponDamageProfile {
    name: String,
    min_damage: f64,
    max_damage: f64,
    cost_per_shot: f64,
    role: Option<String>,
}

#[derive(Debug, Clone)]
struct DamageMatch<'a> {
    profile: &'a WeaponDamageProfile,
    low: f64,
    high: f64,
}

impl DamageMatch<'_> {
    fn width(&self) -> f64 {
        self.high - self.low
    }
}

/// Attribute combat damage against the configured trifecta weapons.
#[derive(Default)]
pub struct DamageAttributor {
    profiles: Vec<WeaponDamageProfile>,
}

impl DamageAttributor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.profiles.clear();
    }

    /// Register (or replace) a weapon's damage profile. The original
    /// also carries a base-damage figure that its matching never
    /// reads (a zero falls back to the maximum there); this port's
    /// profile omits the unread field, so the matching surface is
    /// identical.
    pub fn add_weapon_profile(
        &mut self,
        name: &str,
        min_damage: f64,
        max_damage: f64,
        _base_damage: f64,
        cost_per_shot: f64,
        role: Option<&str>,
    ) {
        let profile = WeaponDamageProfile {
            name: name.to_string(),
            min_damage,
            max_damage,
            cost_per_shot,
            role: role.map(str::to_string),
        };
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// The attribution for one hit, or None when nothing matches.
    pub fn match_damage(&self, amount: f64, critical: bool) -> Option<DamageAttribution> {
        if amount <= 0.0 || self.profiles.is_empty() {
            return None;
        }

        let regular_matches = self.matches_for(amount, false);
        let critical_matches = self.matches_for(amount, true);

        let selected = if critical {
            self.prefer_known_crit_pattern(&regular_matches, &critical_matches)
                .or_else(|| narrowest(&critical_matches))
        } else {
            narrowest(&regular_matches)
        };

        selected.map(|matched| DamageAttribution {
            tool_name: matched.profile.name.clone(),
            cost_per_shot: matched.profile.cost_per_shot,
        })
    }

    fn matches_for(&self, amount: f64, critical: bool) -> Vec<DamageMatch<'_>> {
        self.profiles
            .iter()
            .filter_map(|profile| {
                let (low, high) = bounds(profile, critical);
                if low <= amount && amount <= high {
                    Some(DamageMatch { profile, low, high })
                } else {
                    None
                }
            })
            .collect()
    }

    fn prefer_known_crit_pattern<'a>(
        &self,
        regular_matches: &[DamageMatch<'a>],
        critical_matches: &[DamageMatch<'a>],
    ) -> Option<DamageMatch<'a>> {
        let small_weapon_can_crit = critical_matches
            .iter()
            .any(|matched| matched.profile.role.as_deref() == Some("small_weapon"));
        if !small_weapon_can_crit {
            return None;
        }
        let big_regular: Vec<DamageMatch> = regular_matches
            .iter()
            .filter(|matched| matched.profile.role.as_deref() == Some("big_weapon"))
            .cloned()
            .collect();
        narrowest(&big_regular)
    }
}

fn bounds(profile: &WeaponDamageProfile, critical: bool) -> (f64, f64) {
    if critical {
        (
            profile.min_damage * CRITICAL_DAMAGE_MIN,
            profile.max_damage * CRITICAL_DAMAGE_MAX,
        )
    } else {
        (profile.min_damage, profile.max_damage)
    }
}

/// The narrowest band, name as the tiebreak (the original's tuple-key
/// minimum).
fn narrowest<'a>(matches: &[DamageMatch<'a>]) -> Option<DamageMatch<'a>> {
    matches
        .iter()
        .min_by(|a, b| {
            a.width()
                .partial_cmp(&b.width())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.profile.name.cmp(&b.profile.name))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributor() -> DamageAttributor {
        let mut attributor = DamageAttributor::new();
        attributor.add_weapon_profile("Pistol", 5.0, 10.0, 0.0, 0.05, Some("small_weapon"));
        attributor.add_weapon_profile("Cannon", 20.0, 40.0, 40.0, 0.2, Some("big_weapon"));
        attributor
    }

    #[test]
    fn regular_hits_attribute_to_the_containing_band() {
        let attributor = attributor();
        let hit = attributor.match_damage(7.0, false).unwrap();
        assert_eq!(hit.tool_name, "Pistol");
        assert_eq!(hit.cost_per_shot, 0.05);
        let hit = attributor.match_damage(30.0, false).unwrap();
        assert_eq!(hit.tool_name, "Cannon");
        assert!(attributor.match_damage(15.0, false).is_none());
        assert!(attributor.match_damage(0.0, false).is_none());
        assert!(attributor.match_damage(-1.0, false).is_none());
        assert!(DamageAttributor::new().match_damage(7.0, false).is_none());
    }

    #[test]
    fn overlapping_bands_pick_the_narrowest_then_the_name() {
        let mut attributor = DamageAttributor::new();
        attributor.add_weapon_profile("Wide", 0.0, 100.0, 0.0, 0.1, None);
        attributor.add_weapon_profile("Narrow", 5.0, 15.0, 0.0, 0.2, None);
        let hit = attributor.match_damage(10.0, false).unwrap();
        assert_eq!(hit.tool_name, "Narrow");

        attributor.clear();
        attributor.add_weapon_profile("Beta", 5.0, 15.0, 0.0, 0.1, None);
        attributor.add_weapon_profile("Alpha", 10.0, 20.0, 0.0, 0.2, None);
        let hit = attributor.match_damage(12.0, false).unwrap();
        assert_eq!(hit.tool_name, "Alpha", "equal widths break on the name");
    }

    #[test]
    fn criticals_widen_bands_and_prefer_the_big_regular_pattern() {
        let attributor = attributor();
        // 25.0: inside the small weapon's critical band (10-30) AND the
        // big weapon's regular band (20-40): the known pattern prefers
        // the big regular explanation.
        let hit = attributor.match_damage(25.0, true).unwrap();
        assert_eq!(hit.tool_name, "Cannon");

        // 12.0: only the small weapon's critical band (10-30) explains
        // it; no big regular match, so the critical match stands.
        let hit = attributor.match_damage(12.0, true).unwrap();
        assert_eq!(hit.tool_name, "Pistol");

        // 90.0: only the big weapon's critical band (40-120).
        let hit = attributor.match_damage(90.0, true).unwrap();
        assert_eq!(hit.tool_name, "Cannon");

        // Without the small-weapon role, the pattern never engages.
        let mut role_free = DamageAttributor::new();
        role_free.add_weapon_profile("Pistol", 5.0, 10.0, 0.0, 0.05, None);
        role_free.add_weapon_profile("Cannon", 20.0, 40.0, 0.0, 0.2, Some("big_weapon"));
        let hit = role_free.match_damage(25.0, true).unwrap();
        assert_eq!(hit.tool_name, "Pistol", "narrowest critical band wins");
    }

    #[test]
    fn re_registering_a_name_replaces_its_profile() {
        let mut attributor = attributor();
        attributor.add_weapon_profile("Pistol", 50.0, 60.0, 0.0, 0.5, None);
        assert!(attributor.match_damage(7.0, false).is_none());
        let hit = attributor.match_damage(55.0, false).unwrap();
        assert_eq!(hit.tool_name, "Pistol");
        assert_eq!(hit.cost_per_shot, 0.5);
    }

    #[test]
    fn width_ordering_is_subtraction_not_any_lookalike() {
        // A: width 7.5 but a large high/low ratio; B: width 9 with a
        // small ratio. The narrowest-by-subtraction pick is A; any
        // ratio-shaped ordering would pick B.
        let mut attributor = DamageAttributor::new();
        attributor.add_weapon_profile("Aspect", 0.5, 8.0, 0.0, 0.1, None);
        attributor.add_weapon_profile("Bracket", 6.0, 15.0, 0.0, 0.2, None);
        let hit = attributor.match_damage(7.0, false).unwrap();
        assert_eq!(hit.tool_name, "Aspect");

        // Equal widths with the alphabetically-later name narrower
        // by construction: a constant width would tie and pick the
        // alphabetically-first WIDE band instead.
        let mut tie = DamageAttributor::new();
        tie.add_weapon_profile("Alpha", 0.0, 50.0, 0.0, 0.1, None);
        tie.add_weapon_profile("Zed", 5.0, 9.0, 0.0, 0.2, None);
        let hit = tie.match_damage(7.0, false).unwrap();
        assert_eq!(hit.tool_name, "Zed");
    }

    #[test]
    fn the_zero_amount_guard_beats_a_zero_low_band() {
        let mut attributor = DamageAttributor::new();
        attributor.add_weapon_profile("Floor", 0.0, 10.0, 0.0, 0.1, None);
        assert!(
            attributor.match_damage(0.0, false).is_none(),
            "zero damage never attributes even when a band starts at zero"
        );
    }

    #[test]
    fn clear_removes_every_profile() {
        let mut attributor = attributor();
        attributor.clear();
        assert!(attributor.match_damage(7.0, false).is_none());
        assert!(attributor.match_damage(30.0, false).is_none());
    }

    #[test]
    fn critical_minimum_scales_by_multiplication() {
        let attributor = attributor();
        // The small weapon's critical band starts at 5*2 = 10: amounts
        // below it (which additive or divisive lookalikes would admit)
        // attribute to nothing.
        assert!(attributor.match_damage(8.5, true).is_none());
        assert!(attributor.match_damage(3.0, true).is_none());
        assert!(attributor.match_damage(10.0, true).is_some());
    }
}
