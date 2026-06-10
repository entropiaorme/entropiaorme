//! Drift comparison between tracked levels and fresh scan results,
//! ported from `backend/services/scan_drift.py`.

use serde_json::{Map, Value};

/// Summarise drift between the tracked levels and a fresh scan, or
/// None when the two share no skill names. Shared names are walked in
/// sorted order; the worst entry is the first strictly-largest
/// absolute difference; the percentage guard divides by at least 1.
pub fn summarize_level_drift(
    tracked_levels: &Map<String, Value>,
    scanned_levels: &Map<String, Value>,
) -> Option<Value> {
    let mut shared_names: Vec<&String> = tracked_levels
        .keys()
        .filter(|name| scanned_levels.contains_key(name.as_str()))
        .collect();
    if shared_names.is_empty() {
        return None;
    }
    shared_names.sort();

    let mut total_abs_diff = 0.0;
    let mut total_signed_diff = 0.0;
    let mut total_abs_pct = 0.0;
    let mut worst_name = "";
    let mut worst_tracked = 0.0;
    let mut worst_scanned = 0.0;
    let mut worst_signed_diff = 0.0;
    let mut worst_abs_diff = -1.0;

    for name in &shared_names {
        // Bare float conversion (numeric strings and booleans coerce as
        // the backend's float() does); a value float() would crash on
        // coalesces to 0 instead, per the divergence register.
        let tracked =
            crate::character_calc::python_float_bare(&tracked_levels[name.as_str()]).unwrap_or(0.0);
        let scanned =
            crate::character_calc::python_float_bare(&scanned_levels[name.as_str()]).unwrap_or(0.0);
        let signed_diff = scanned - tracked;
        let abs_diff = signed_diff.abs();
        let abs_pct = abs_diff / scanned.abs().max(1.0) * 100.0;

        total_abs_diff += abs_diff;
        total_signed_diff += signed_diff;
        total_abs_pct += abs_pct;

        if abs_diff > worst_abs_diff {
            worst_name = name.as_str();
            worst_tracked = tracked;
            worst_scanned = scanned;
            worst_signed_diff = signed_diff;
            worst_abs_diff = abs_diff;
        }
    }

    let compared_count = shared_names.len();
    let tracked_only = tracked_levels.len() - compared_count;
    let scan_only = scanned_levels.len() - compared_count;
    Some(serde_json::json!({
        "compared_count": compared_count,
        "tracked_only_count": tracked_only,
        "scan_only_count": scan_only,
        "total_abs_diff": total_abs_diff,
        "avg_abs_diff": total_abs_diff / compared_count as f64,
        "total_signed_diff": total_signed_diff,
        "avg_abs_pct": total_abs_pct / compared_count as f64,
        "worst_name": worst_name,
        "worst_tracked": worst_tracked,
        "worst_scanned": worst_scanned,
        "worst_signed_diff": worst_signed_diff,
        "worst_abs_diff": worst_abs_diff,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn level_map(pairs: &[(&str, f64)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(name, level)| (name.to_string(), json!(level)))
            .collect()
    }

    #[test]
    fn only_counts_subtract_the_shared_set() {
        // 5 tracked, 1 shared: only-count 4 (and not any quotient).
        let tracked = level_map(&[
            ("Shared", 1.0),
            ("T1", 1.0),
            ("T2", 1.0),
            ("T3", 1.0),
            ("T4", 1.0),
        ]);
        let scanned = level_map(&[("Shared", 2.0), ("S1", 1.0), ("S2", 1.0)]);
        let drift = summarize_level_drift(&tracked, &scanned).unwrap();
        assert_eq!(drift["tracked_only_count"], 4);
        assert_eq!(drift["scan_only_count"], 2);
    }

    #[test]
    fn no_shared_names_yields_none() {
        let tracked = level_map(&[("Rifle", 100.0)]);
        let scanned = level_map(&[("Anatomy", 50.0)]);
        assert!(summarize_level_drift(&tracked, &scanned).is_none());
        assert!(summarize_level_drift(&Map::new(), &Map::new()).is_none());
    }

    #[test]
    fn summary_matches_hand_computed_figures() {
        let tracked = level_map(&[("Rifle", 100.0), ("Anatomy", 50.0), ("Only Tracked", 5.0)]);
        let scanned = level_map(&[
            ("Rifle", 104.0),
            ("Anatomy", 48.0),
            ("Only Scanned", 9.0),
            ("Another Scanned", 1.0),
        ]);
        let drift = summarize_level_drift(&tracked, &scanned).unwrap();
        assert_eq!(drift["compared_count"], 2);
        assert_eq!(drift["tracked_only_count"], 1);
        assert_eq!(drift["scan_only_count"], 2);
        assert_eq!(drift["total_abs_diff"], 6.0);
        assert_eq!(drift["avg_abs_diff"], 3.0);
        assert_eq!(drift["total_signed_diff"], 2.0);
        // Anatomy: 2/48*100 = 4.1666..; Rifle: 4/104*100 = 3.8461..
        let expected_pct = (2.0_f64 / 48.0 * 100.0 + 4.0 / 104.0 * 100.0) / 2.0;
        assert_eq!(drift["avg_abs_pct"].as_f64().unwrap(), expected_pct);
        assert_eq!(drift["worst_name"], "Rifle");
        assert_eq!(drift["worst_tracked"], 100.0);
        assert_eq!(drift["worst_scanned"], 104.0);
        assert_eq!(drift["worst_signed_diff"], 4.0);
        assert_eq!(drift["worst_abs_diff"], 4.0);
        let keys: Vec<&String> = drift.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "compared_count",
                "tracked_only_count",
                "scan_only_count",
                "total_abs_diff",
                "avg_abs_diff",
                "total_signed_diff",
                "avg_abs_pct",
                "worst_name",
                "worst_tracked",
                "worst_scanned",
                "worst_signed_diff",
                "worst_abs_diff"
            ]
        );
    }

    #[test]
    fn worst_ties_keep_the_first_sorted_name_and_small_levels_guard_pct() {
        // Both share |diff| 2.0: the strictly-greater comparison keeps
        // the first name in sorted walk order ("Alpha").
        let tracked = level_map(&[("Beta", 10.0), ("Alpha", 20.0)]);
        let scanned = level_map(&[("Beta", 12.0), ("Alpha", 22.0)]);
        let drift = summarize_level_drift(&tracked, &scanned).unwrap();
        assert_eq!(drift["worst_name"], "Alpha");

        // A scanned level under 1 divides by the guard, not the level.
        let tracked = level_map(&[("Tiny", 0.0)]);
        let scanned = level_map(&[("Tiny", 0.5)]);
        let drift = summarize_level_drift(&tracked, &scanned).unwrap();
        assert_eq!(drift["avg_abs_pct"], 50.0);

        // A zero-diff comparison still beats the -1 sentinel.
        let tracked = level_map(&[("Same", 7.0)]);
        let scanned = level_map(&[("Same", 7.0)]);
        let drift = summarize_level_drift(&tracked, &scanned).unwrap();
        assert_eq!(drift["worst_name"], "Same");
        assert_eq!(drift["worst_abs_diff"], 0.0);
    }
}
