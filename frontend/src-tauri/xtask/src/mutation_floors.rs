//! Port of the original Python implementation.
//!
//! Reads a cargo-mutants `outcomes.json` and enforces the per-file mutation
//! score floors below. Scoring matches the campaign's conventions: a mutant
//! counts as caught when a test failed on it OR the mutated build timed out;
//! missed mutants count against the score; unviable mutants (the mutation does
//! not compile) leave the denominator entirely. Files without an adopted floor
//! are held to the strictest bar (any missed mutant fails). Floors only ever
//! ratchet up.

use std::collections::BTreeMap;
use std::path::Path;

/// file (workspace-relative, as cargo-mutants reports it) -> floor %.
///
/// Ported verbatim from the Python `FLOORS` map; see that script for the
/// per-file rationale on each residual-survivor justification.
const FLOORS: &[(&str, f64)] = &[
    ("eo-services/src/cost_engine.rs", 92.0),
    ("eo-services/src/tt_value_curve.rs", 92.0),
    ("eo-services/src/character_calc.rs", 92.0),
    ("eo-services/src/chatlog_parser.rs", 92.0),
    ("eo-services/src/chatlog_watcher.rs", 92.0),
    ("eo-services/src/tracker.rs", 92.0),
    ("eo-services/src/session_summary.rs", 92.0),
    ("eo-services/src/fuzzy_match.rs", 92.0),
    ("eo-services/src/ocr_engine.rs", 82.0),
    ("eo-services/src/skill_panel.rs", 92.0),
    ("eo-services/src/codex.rs", 92.0),
    ("eo-services/src/quests.rs", 92.0),
    ("eo-services/src/difflib.rs", 92.0),
    ("eo-http/src/hydration.rs", 92.0),
    ("eo-wire/src/normalizer.rs", 81.0),
    ("eo-wire/src/http_fingerprint.rs", 97.0),
    ("eo-http/src/pyjson.rs", 79.0),
    ("eo-http/src/body.rs", 78.0),
    ("eo-http/src/native.rs", 98.0),
    ("eo-http/src/character_routes.rs", 85.0),
    ("eo-http/src/equipment_routes.rs", 90.0),
    ("eo-http/src/analytics_routes.rs", 92.0),
    ("eo-http/src/tracking_routes.rs", 90.0),
];

/// Map a score to a shields.io colour band (identical floors to
/// coverage-badge.sh, so the product badges read consistently).
fn colour_band(score: f64) -> &'static str {
    if score >= 90.0 {
        "brightgreen"
    } else if score >= 80.0 {
        "green"
    } else if score >= 70.0 {
        "yellowgreen"
    } else if score >= 60.0 {
        "yellow"
    } else if score >= 50.0 {
        "orange"
    } else {
        "red"
    }
}

#[derive(Default, Clone, Copy)]
struct Counts {
    caught: u32,
    missed: u32,
    timeout: u32,
    #[allow(dead_code)]
    unviable: u32,
}

/// Per-file caught/missed/timeout/unviable counts from outcomes.json.
///
/// Returns the counts keyed by file. Errors (Err) on an unreadable file, invalid
/// JSON, a missing `outcomes` array, or an unrecognised outcome summary, so a
/// malformed campaign output fails closed exactly as the Python `SystemExit`.
fn score_outcomes(text: &str) -> Result<BTreeMap<String, Counts>, String> {
    let data: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("cannot parse outcomes.json: {e}"))?;
    let outcomes = data
        .get("outcomes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "outcomes.json has no 'outcomes' array".to_string())?;

    let mut per_file: BTreeMap<String, Counts> = BTreeMap::new();
    for outcome in outcomes {
        let scenario = &outcome["scenario"];
        // The baseline (unmutated) build is reported as the string "Baseline".
        if scenario.as_str() == Some("Baseline") {
            continue;
        }
        let file = scenario
            .get("Mutant")
            .and_then(|m| m.get("file"))
            .and_then(|f| f.as_str())
            .ok_or_else(|| "an outcome has no scenario.Mutant.file".to_string())?;
        let summary = outcome
            .get("summary")
            .and_then(|s| s.as_str())
            .ok_or_else(|| "an outcome has no summary".to_string())?;
        let counts = per_file.entry(file.to_string()).or_default();
        match summary {
            "CaughtMutant" => counts.caught += 1,
            "MissedMutant" => counts.missed += 1,
            "Timeout" => counts.timeout += 1,
            "Unviable" => counts.unviable += 1,
            other => return Err(format!("unrecognised outcome summary: {other:?}")),
        }
    }
    Ok(per_file)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let outcomes_path = crate::flag_value(args, "--outcomes")?
        .unwrap_or_else(|| "mutants.out/outcomes.json".to_string());
    let text = std::fs::read_to_string(Path::new(&outcomes_path))
        .map_err(|e| format!("cannot read {outcomes_path}: {e}"))?;
    let per_file = score_outcomes(&text)?;

    if per_file.is_empty() {
        println!("no mutants in the campaign output; nothing to score");
        return Ok(1);
    }

    let floors: BTreeMap<&str, f64> = FLOORS.iter().copied().collect();
    let mut failures: Vec<String> = Vec::new();
    let mut total_caught: u32 = 0;
    let mut total_considered: u32 = 0;

    println!("{:45} {:>6} {:>6} {:>7} {:>9}", "file", "caught", "missed", "score", "floor");
    for (file, counts) in &per_file {
        let caught = counts.caught + counts.timeout;
        let denominator = caught + counts.missed;
        total_caught += caught;
        total_considered += denominator;
        let score = if denominator > 0 {
            100.0 * caught as f64 / denominator as f64
        } else {
            100.0
        };
        let floor = floors.get(file.as_str()).copied();
        let bar = match floor {
            Some(f) => format!("{f:.1}"),
            None => "no-missed".to_string(),
        };
        println!(
            "{file:45} {caught:6} {missed:6} {score:7.1} {bar:>9}",
            missed = counts.missed
        );
        match floor {
            Some(f) => {
                if score < f {
                    failures.push(format!("{file}: score {score:.1} below floor {f:.1}"));
                }
            }
            None => {
                if counts.missed > 0 {
                    failures.push(format!(
                        "{file}: {} missed mutant(s) and no adopted floor",
                        counts.missed
                    ));
                }
            }
        }
    }

    // A floor whose file produced no scored mutants is a silently vacuous gate
    // (a rename or deletion would otherwise pass unnoticed). Walked in sorted
    // file order, matching the Python's `sorted(FLOORS.items())`.
    for (file, floor) in &floors {
        if !per_file.contains_key(*file) {
            failures.push(format!(
                "{file}: adopted floor {floor:.1} but no scored mutants \
(renamed or removed? update the floor map)"
            ));
        }
    }

    // Badge-only mode: emit the shields.io endpoint badge for the aggregate
    // score and return without enforcing, so the published badge always reflects
    // reality (the separate enforce invocation, with no --badge-out, is the
    // gate). The shape and colour bands mirror coverage-badge.sh so the two
    // product badges read consistently.
    if let Some(badge_out) = crate::flag_value(args, "--badge-out")? {
        let aggregate = if total_considered > 0 {
            100.0 * total_caught as f64 / total_considered as f64
        } else {
            0.0
        };
        let colour = colour_band(aggregate);
        let badge = serde_json::json!({
            "schemaVersion": 1,
            "label": "mutation score",
            "message": format!("{aggregate:.1}%"),
            "color": colour,
        });
        let rendered = serde_json::to_string(&badge)
            .map_err(|e| format!("cannot render mutation badge: {e}"))?;
        std::fs::write(&badge_out, rendered)
            .map_err(|e| format!("cannot write mutation badge to {badge_out}: {e}"))?;
        println!("wrote mutation badge ({aggregate:.1}%, {colour}) to {badge_out}");
        return Ok(0);
    }

    if !failures.is_empty() {
        eprintln!("\nmutation floors violated:");
        // Match the Python ordering: per-file failures are appended during the
        // sorted per_file walk (BTreeMap iterates sorted), then the
        // missing-floor failures during the sorted-by-file FLOORS walk.
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        return Ok(1);
    }
    println!("\nall mutation floors hold");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(file: &str, summary: &str) -> serde_json::Value {
        serde_json::json!({
            "scenario": {"Mutant": {"file": file}},
            "summary": summary,
        })
    }

    #[test]
    fn timeout_counts_as_caught_and_unviable_leaves_denominator() {
        // The baseline is an outcome object whose `scenario` field is the
        // string "Baseline" (as cargo-mutants reports it); it is skipped.
        let data = serde_json::json!({
            "outcomes": [
                {"scenario": "Baseline", "summary": "Success"},
                outcome("a.rs", "CaughtMutant"),
                outcome("a.rs", "Timeout"),
                outcome("a.rs", "Unviable"),
                outcome("a.rs", "MissedMutant"),
            ]
        });
        let per = score_outcomes(&data.to_string()).unwrap();
        let c = per.get("a.rs").unwrap();
        // caught(1)+timeout(1)=2 caught; missed=1; unviable out of denominator.
        let caught = c.caught + c.timeout;
        let denom = caught + c.missed;
        let score = 100.0 * caught as f64 / denom as f64;
        assert_eq!(caught, 2);
        assert_eq!(denom, 3);
        assert!((score - 66.666_666).abs() < 0.01);
    }

    #[test]
    fn unrecognised_summary_fails_closed() {
        let data = serde_json::json!({"outcomes": [outcome("a.rs", "Bogus")]});
        assert!(score_outcomes(&data.to_string()).is_err());
    }

    #[test]
    fn full_denominator_zero_scores_hundred() {
        let data = serde_json::json!({"outcomes": [outcome("a.rs", "Unviable")]});
        let per = score_outcomes(&data.to_string()).unwrap();
        let c = per.get("a.rs").unwrap();
        let caught = c.caught + c.timeout;
        let denom = caught + c.missed;
        assert_eq!(denom, 0);
    }

    #[test]
    fn floor_map_matches_python_count() {
        // Guards against an accidental drop when transcribing the map.
        assert_eq!(FLOORS.len(), 23);
    }
}
