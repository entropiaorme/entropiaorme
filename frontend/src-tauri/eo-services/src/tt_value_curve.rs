//! TT value curve from the official Entropia Universe wiki chip-in
//! optimiser, ported from `backend/data/tt_value_curve.py`.
//!
//! The anchor data is the same CSV the backend loads (embedded at compile
//! time from the one tracked copy, so the two implementations cannot read
//! different curves). Linear interpolation between monotonic
//! non-decreasing anchors, level 0 anchored to 0.0 PED; all rounding goes
//! through the Python-faithful half-even helper so figures stay
//! bit-identical to the backend's.

use std::sync::OnceLock;

use eo_wire::normalizer::round_half_even;

const CURVE_CSV: &str = include_str!("../../../../backend/data/tt_value_curve.csv");

fn round4(x: f64) -> f64 {
    round_half_even(x, 4)
}

struct Curve {
    levels: Vec<i64>,
    tt_values: Vec<f64>,
}

fn curve() -> &'static Curve {
    static CURVE: OnceLock<Curve> = OnceLock::new();
    CURVE.get_or_init(|| {
        let mut levels = Vec::new();
        let mut tt_values = Vec::new();
        let mut lines = CURVE_CSV.lines();
        let header = lines.next().expect("curve CSV has a header");
        let columns: Vec<&str> = header.trim().split(',').collect();
        let level_idx = columns
            .iter()
            .position(|c| *c == "level")
            .expect("curve CSV has a level column");
        let tt_idx = columns
            .iter()
            .position(|c| *c == "tt_value")
            .expect("curve CSV has a tt_value column");
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            levels.push(
                fields[level_idx]
                    .trim()
                    .parse()
                    .expect("curve level parses as an integer"),
            );
            tt_values.push(
                fields[tt_idx]
                    .trim()
                    .parse()
                    .expect("curve tt_value parses as a float"),
            );
        }
        assert!(!levels.is_empty(), "curve CSV carries anchors");
        Curve { levels, tt_values }
    })
}

/// Cumulative TT value (PED) at a skill level; linear interpolation
/// between anchors.
pub fn tt_value_at(level: f64) -> f64 {
    let curve = curve();
    if level <= 0.0 {
        return 0.0;
    }
    let last_level = *curve.levels.last().expect("non-empty curve") as f64;
    if level >= last_level {
        return *curve.tt_values.last().expect("non-empty curve");
    }
    // bisect_right(levels, level) - 1: the rightmost anchor at or below
    // `level` (partition_point counts anchors whose value <= level).
    let i = curve
        .levels
        .partition_point(|&anchor| (anchor as f64) <= level)
        - 1;
    let lo = curve.levels[i] as f64;
    let hi = curve.levels[i + 1] as f64;
    let t = (level - lo) / (hi - lo);
    round4(curve.tt_values[i] + t * (curve.tt_values[i + 1] - curve.tt_values[i]))
}

/// TT value of a skill gain from `from_level` to `to_level`.
pub fn tt_value_of_gain(from_level: f64, to_level: f64) -> f64 {
    round4(tt_value_at(to_level) - tt_value_at(from_level))
}

/// How many skill levels `ped_value` PED of TT buys starting from
/// `from_level`: the same 64-iteration bisection as the backend, so the
/// returned fraction is bit-identical.
pub fn levels_for_tt_value(from_level: f64, ped_value: f64) -> f64 {
    let curve = curve();
    if ped_value <= 0.0 {
        return 0.0;
    }
    let target_tt = tt_value_at(from_level) + ped_value;
    let mut lo = from_level;
    let mut hi = *curve.levels.last().expect("non-empty curve") as f64;
    if target_tt >= *curve.tt_values.last().expect("non-empty curve") {
        return hi - from_level;
    }
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if tt_value_at(mid) < target_tt {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    round4(lo - from_level)
}

/// The highest level represented by the curve data.
pub fn max_tt_curve_level() -> i64 {
    *curve().levels.last().expect("non-empty curve")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn anchors_and_clamps() {
        assert_eq!(tt_value_at(0.0), 0.0);
        assert_eq!(tt_value_at(-3.5), 0.0);
        let max_level = max_tt_curve_level() as f64;
        let top = tt_value_at(max_level);
        assert_eq!(tt_value_at(max_level + 100.0), top);
        assert!(top > 0.0);
    }

    #[test]
    fn gain_is_difference_of_curve_points() {
        let gain = tt_value_of_gain(10.0, 20.0);
        assert_eq!(gain, round4(tt_value_at(20.0) - tt_value_at(10.0)));
        assert_eq!(tt_value_of_gain(20.0, 10.0), -gain);
    }

    #[test]
    fn zero_or_negative_ped_buys_no_levels() {
        assert_eq!(levels_for_tt_value(10.0, 0.0), 0.0);
        assert_eq!(levels_for_tt_value(10.0, -5.0), 0.0);
    }

    #[test]
    fn beyond_curve_ped_buys_exactly_the_remaining_levels() {
        let max_level = max_tt_curve_level() as f64;
        let everything = tt_value_at(max_level) + 1000.0;
        assert_eq!(levels_for_tt_value(5.0, everything), max_level - 5.0);
    }

    proptest! {
        #[test]
        fn curve_is_monotonic_non_decreasing(a in 0.0f64..10000.0, b in 0.0f64..10000.0) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(tt_value_at(lo) <= tt_value_at(hi));
        }

        #[test]
        fn levels_bought_then_valued_round_trips_within_rounding(
            from in 0.0f64..5000.0,
            ped in 0.0001f64..1000.0,
        ) {
            let bought = levels_for_tt_value(from, ped);
            prop_assert!(bought >= 0.0);
            let value = tt_value_of_gain(from, from + bought);
            // The bisection narrows to ~1e-4 level precision; the value of
            // the bought span stays within a curve-slope step of the spend
            // unless the spend ran past the top anchor.
            let max_level = max_tt_curve_level() as f64;
            if from + bought < max_level {
                prop_assert!((value - ped).abs() < 1.0);
            }
        }
    }
}
