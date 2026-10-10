//! Expected uses to kill a mob of a given HP: a deterministic renewal
//! recursion, solved once per setup on a fine HP grid.
//!
//! Each use misses with probability `1 - p` and otherwise deals a damage
//! roll `d`, so the expected number of uses to remove `h` HP satisfies
//!
//! ```text
//! E(h) = 0                                   for h <= 0
//! E(h) = (1 + p * E_d[E(h - d)]) / p         for h > 0
//! ```
//!
//! where `d` is uniform on `[floor * max, max]` with probability `1 - c`
//! and uniform on `[k * floor * max, k * max]` with probability `c` (a
//! critical multiplies the rolled damage by `k`). Every roll is at least
//! `floor * max > 0`, so `E(h)` depends only on values strictly below
//! `h`, and the recursion is solved bottom-up.
//!
//! The expectation over a uniform roll is an integral of `E`, so the grid
//! carries the running integral `G(x) = integral of E over (0, x]`
//! alongside `E`: each grid value costs O(1) and the whole curve O(N).
//! `E` is continuous for `h > 0` (it jumps only at zero, from 0 to `1 / p`)
//! and piecewise smooth, so linear interpolation with the trapezoid rule
//! is second-order accurate; at 64 grid steps per damage floor the
//! discretisation error is far below a hundredth of a percent.
//!
//! Far beyond one shot the renewal theorem gives the exact asymptote
//! `E(h) -> (h / mu + E[d^2] / (2 mu^2)) / p` (`mu` the mean hit), which
//! the recursion converges to geometrically fast. The grid therefore
//! stops at a reach of [`EXACT_REACH`] critical maxima and continues on
//! the asymptote, offset to meet the grid exactly, so a mob of any HP
//! costs the same small grid.

use super::ComparisonAssumptions;

/// Grid steps per damage floor (the smallest possible hit).
const STEPS_PER_FLOOR: f64 = 64.0;

/// How far the exact grid reaches, in critical maximum hits, before the
/// curve continues on its renewal asymptote.
const EXACT_REACH: f64 = 20.0;

/// The solved expected-uses curve for one setup.
#[derive(Debug, Clone)]
pub struct KillCurve {
    step: f64,
    /// `values[i]` is `E(i * step)` for `i >= 1`; `values[0]` holds the
    /// right limit `E(0+) = 1 / p`, which is what the integrals need.
    values: Vec<f64>,
    hit_rate: f64,
    mean_hit: f64,
    second_moment: f64,
    /// Where the grid ends and the asymptote takes over.
    horizon: f64,
    /// The asymptote's offset at the horizon, so the curve is continuous.
    tail_offset: f64,
}

impl KillCurve {
    /// Solve the curve for a setup whose maximum hit is `max_damage`,
    /// exactly up to `hp_max` or the exact reach, whichever is smaller.
    /// None when the damage or the assumptions cannot define a curve.
    pub fn solve(
        max_damage: f64,
        hp_max: f64,
        assumptions: &ComparisonAssumptions,
    ) -> Option<Self> {
        Self::solve_with_resolution(max_damage, hp_max, assumptions, STEPS_PER_FLOOR)
    }

    pub(crate) fn solve_with_resolution(
        max_damage: f64,
        hp_max: f64,
        assumptions: &ComparisonAssumptions,
        steps_per_floor: f64,
    ) -> Option<Self> {
        let usable = max_damage.is_finite()
            && max_damage > 0.0
            && hp_max.is_finite()
            && assumptions.is_well_formed();
        if !usable {
            return None;
        }
        let p = assumptions.hit_rate;
        let c = assumptions.critical_chance;
        let k = assumptions.critical_multiplier;
        let low = assumptions.damage_floor * max_damage;
        let high = max_damage;
        let (crit_low, crit_high) = (k * low, k * high);

        let step = low / steps_per_floor;
        let reach = EXACT_REACH * crit_high;
        let horizon_points = (hp_max.max(0.0).min(reach) / step).ceil() as usize + 2;

        let mut values = vec![0.0; horizon_points + 1];
        let mut integral = vec![0.0; horizon_points + 1];
        values[0] = 1.0 / p;

        // The running integral of E over (0, x] at any x already solved,
        // with E interpolated linearly between grid points.
        let integral_at = |values: &[f64], integral: &[f64], x: f64| -> f64 {
            if x <= 0.0 {
                return 0.0;
            }
            let position = x / step;
            let index = position.floor() as usize;
            let t = position - index as f64;
            if t <= 0.0 {
                return integral[index];
            }
            let left = values[index];
            let right = values[index + 1];
            let at_x = left + t * (right - left);
            integral[index] + t * step * (left + at_x) / 2.0
        };

        let normal_weight = (1.0 - c) / (high - low);
        let critical_weight = c / (crit_high - crit_low);
        for i in 1..=horizon_points {
            let h = i as f64 * step;
            let normal = integral_at(&values, &integral, h - low)
                - integral_at(&values, &integral, h - high);
            let critical = if c > 0.0 {
                integral_at(&values, &integral, h - crit_low)
                    - integral_at(&values, &integral, h - crit_high)
            } else {
                0.0
            };
            let expected_after_hit = normal_weight * normal + critical_weight * critical;
            values[i] = (1.0 + p * expected_after_hit) / p;
            integral[i] = integral[i - 1] + step * (values[i - 1] + values[i]) / 2.0;
        }

        // Moments of a landed hit: a mixture of two uniforms.
        let uniform_second = |a: f64, b: f64| (a * a + a * b + b * b) / 3.0;
        let mean_hit = (1.0 - c) * (low + high) / 2.0 + c * (crit_low + crit_high) / 2.0;
        let second_moment =
            (1.0 - c) * uniform_second(low, high) + c * uniform_second(crit_low, crit_high);
        let horizon = horizon_points as f64 * step;
        let mut curve = Self {
            step,
            values,
            hit_rate: p,
            mean_hit,
            second_moment,
            horizon,
            tail_offset: 0.0,
        };
        curve.tail_offset = curve.values[horizon_points] - curve.asymptote(horizon);
        Some(curve)
    }

    /// The renewal asymptote of the curve, without the meeting offset.
    fn asymptote(&self, hp: f64) -> f64 {
        (hp / self.mean_hit + self.second_moment / (2.0 * self.mean_hit * self.mean_hit))
            / self.hit_rate
    }

    /// Expected uses to remove `hp` HP: 0 for a mob with no HP left.
    pub fn expected_uses(&self, hp: f64) -> f64 {
        if hp.is_nan() || hp <= 0.0 {
            return 0.0;
        }
        if hp >= self.horizon {
            return self.asymptote(hp) + self.tail_offset;
        }
        let position = hp / self.step;
        let index = position.floor() as usize;
        let t = position - index as f64;
        let left = self.values[index];
        let right = self.values[index + 1];
        left + t * (right - left)
    }

    /// The mean damage of a landed hit, criticals included.
    pub fn mean_hit(&self) -> f64 {
        self.mean_hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn assumptions(hit_rate: f64, critical_chance: f64) -> ComparisonAssumptions {
        ComparisonAssumptions {
            hit_rate,
            critical_chance,
            ..ComparisonAssumptions::STANDARD
        }
    }

    #[test]
    fn a_sure_hit_without_criticals_matches_the_closed_form() {
        // p = 1, no criticals, rolls uniform on [a, 2a]: E = 1 up to a,
        // 1 + (h - a) / a up to 2a, and 2 + (h - 2a)^2 / (2a^2) up to 3a.
        let curve = KillCurve::solve(10.0, 40.0, &assumptions(1.0, 0.0)).unwrap();
        let a: f64 = 5.0;
        for h in [0.5, 3.0, 5.0, 6.5, 8.0, 10.0, 11.0, 12.5, 14.0, 15.0] {
            let expected = if h <= a {
                1.0
            } else if h <= 2.0 * a {
                1.0 + (h - a) / a
            } else {
                2.0 + (h - 2.0 * a).powi(2) / (2.0 * a * a)
            };
            let solved = curve.expected_uses(h);
            assert!(
                (solved - expected).abs() / expected < 1e-4,
                "E({h}) = {solved}, closed form {expected}"
            );
        }
    }

    #[test]
    fn no_hp_needs_no_uses_and_anything_up_to_the_floor_needs_one_hit() {
        let curve = KillCurve::solve(38.0, 200.0, &ComparisonAssumptions::STANDARD).unwrap();
        assert_eq!(curve.expected_uses(0.0), 0.0);
        assert_eq!(curve.expected_uses(-3.0), 0.0);
        for h in [0.01, 5.0, 19.0] {
            assert!((curve.expected_uses(h) - 1.0 / 0.9).abs() < 1e-9);
        }
    }

    #[test]
    fn the_curve_meets_the_renewal_asymptote_far_beyond_one_shot() {
        // With misses and criticals the solved grid converges onto the
        // exact renewal asymptote, and the tail continues it seamlessly.
        let assumptions = ComparisonAssumptions::STANDARD;
        let curve = KillCurve::solve(38.0, 1.0e6, &assumptions).unwrap();
        let asymptote = |h: f64| {
            (h / curve.mean_hit + curve.second_moment / (2.0 * curve.mean_hit * curve.mean_hit))
                / 0.9
        };
        for h in [15.0 * 38.0, 30.0 * 38.0, 5_000.0, 250_000.0] {
            let relative = (curve.expected_uses(h) - asymptote(h)).abs() / asymptote(h);
            assert!(relative < 1e-5, "E({h}) off its asymptote by {relative}");
        }
        let edge = curve.horizon;
        let below = curve.expected_uses(edge - 1e-9);
        let above = curve.expected_uses(edge + 1e-9);
        assert!(
            (below - above).abs() < 1e-6,
            "{below} vs {above} at the horizon"
        );
    }

    #[test]
    fn the_default_grid_is_within_a_tenth_of_a_percent_of_a_fine_one() {
        // The discretisation target is under 0.5%; the default resolution
        // sits two orders of magnitude inside it.
        let assumptions = ComparisonAssumptions::STANDARD;
        for max in [7.0, 38.0, 156.0] {
            let default = KillCurve::solve(max, 40.0 * max, &assumptions).unwrap();
            let fine =
                KillCurve::solve_with_resolution(max, 40.0 * max, &assumptions, 1024.0).unwrap();
            let mut worst: f64 = 0.0;
            let mut h = 0.13 * max;
            while h < 40.0 * max {
                let reference = fine.expected_uses(h);
                worst = worst.max((default.expected_uses(h) - reference).abs() / reference);
                h += 0.37 * max / 7.0;
            }
            assert!(worst < 1e-3, "max {max}: worst relative error {worst}");
        }
    }

    #[test]
    fn unusable_inputs_define_no_curve() {
        let standard = ComparisonAssumptions::STANDARD;
        assert!(KillCurve::solve(0.0, 100.0, &standard).is_none());
        assert!(KillCurve::solve(f64::NAN, 100.0, &standard).is_none());
        assert!(KillCurve::solve(10.0, f64::INFINITY, &standard).is_none());
        let no_floor = ComparisonAssumptions {
            damage_floor: 0.0,
            ..standard
        };
        assert!(KillCurve::solve(10.0, 100.0, &no_floor).is_none());
        let never_hits = ComparisonAssumptions {
            hit_rate: 0.0,
            ..standard
        };
        assert!(KillCurve::solve(10.0, 100.0, &never_hits).is_none());
    }

    proptest! {
        // Wald's identity bounds the expected uses on both sides: the damage
        // dealt by the killing use is at least the HP and at most the HP plus
        // one critical maximum, and each use deals p * mu on average.
        #[test]
        fn expected_uses_respect_walds_bounds_and_never_fall(
            max in 2.0f64..400.0,
            hit_rate in 0.5f64..1.0,
            critical_chance in 0.0f64..0.2,
            fractions in proptest::collection::vec(0.0f64..60.0, 2..12),
        ) {
            let assumptions = ComparisonAssumptions {
                hit_rate,
                critical_chance,
                ..ComparisonAssumptions::STANDARD
            };
            let curve = KillCurve::solve(max, 60.0 * max, &assumptions).unwrap();
            let per_use = hit_rate * curve.mean_hit();
            let critical_max = assumptions.critical_multiplier * max;
            let mut sorted = fractions.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut previous = 0.0;
            for fraction in sorted {
                let h = fraction * max;
                let uses = curve.expected_uses(h);
                prop_assert!(uses + 1e-9 >= previous);
                if h > 0.0 {
                    prop_assert!(uses >= h / per_use * (1.0 - 1e-6));
                    prop_assert!(uses <= (h + critical_max) / per_use * (1.0 + 1e-6));
                }
                previous = uses;
            }
        }
    }
}
