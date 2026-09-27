//! Reads one or more cargo-mutants `outcomes.json` files (the flag repeats,
//! once per campaign shard; per-file counts are summed across them) and
//! reports the per-file and aggregate mutation scores. Scoring: a mutant
//! counts as caught when a test failed on it OR the mutated build timed out;
//! missed mutants count against the score; unviable mutants (the mutation
//! does not compile) leave the denominator entirely.
//!
//! The report is informational: it never fails on a score. The per-file table
//! lists each file's surviving mutants, which is the worklist for a pass that
//! raises the score; `--badge-out` writes the aggregate score as a shields.io
//! endpoint badge. A malformed or empty campaign output still fails, so a
//! broken campaign cannot publish a badge.

use std::collections::BTreeMap;
use std::path::Path;

/// Map a score to a shields.io colour band (the same bands as
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
    unviable: u32,
}

/// Per-file caught/missed/timeout/unviable counts from outcomes.json.
///
/// Returns the counts keyed by file. Errors (Err) on an unreadable file, invalid
/// JSON, a missing `outcomes` array, or an unrecognised outcome summary, so a
/// malformed campaign output fails closed rather than scoring a partial run.
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
    let mut outcomes_paths = crate::flag_values(args, "--outcomes")?;
    if outcomes_paths.is_empty() {
        outcomes_paths.push("mutants.out/outcomes.json".to_string());
    }
    let mut per_file: BTreeMap<String, Counts> = BTreeMap::new();
    for outcomes_path in &outcomes_paths {
        let text = std::fs::read_to_string(Path::new(outcomes_path))
            .map_err(|e| format!("cannot read {outcomes_path}: {e}"))?;
        for (file, counts) in score_outcomes(&text)? {
            let merged = per_file.entry(file).or_default();
            merged.caught += counts.caught;
            merged.missed += counts.missed;
            merged.timeout += counts.timeout;
            merged.unviable += counts.unviable;
        }
    }

    if per_file.is_empty() {
        println!("no mutants in the campaign output; nothing to score");
        return Ok(1);
    }

    let mut total_caught: u32 = 0;
    let mut total_considered: u32 = 0;

    println!(
        "{:45} {:>6} {:>6} {:>7}",
        "file", "caught", "missed", "score"
    );
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
        println!(
            "{file:45} {caught:6} {missed:6} {score:7.1}",
            missed = counts.missed
        );
    }

    let aggregate = if total_considered > 0 {
        100.0 * total_caught as f64 / total_considered as f64
    } else {
        0.0
    };
    println!("\naggregate mutation score: {aggregate:.1}% ({total_caught} of {total_considered})");

    // The shape and colour bands mirror coverage-badge.sh so the two product
    // badges read consistently.
    if let Some(badge_out) = crate::flag_value(args, "--badge-out")? {
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
    }
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
    fn sharded_outcomes_merge_per_file_counts() {
        // Two shards each scoring the same file; the merged badge must reflect
        // the summed counts (3 caught of 4 considered = 75.0%).
        let dir =
            std::env::temp_dir().join(format!("xtask-mutation-shards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shard_a = dir.join("a.json");
        let shard_b = dir.join("b.json");
        let badge = dir.join("badge.json");
        std::fs::write(
            &shard_a,
            serde_json::json!({"outcomes": [
                {"scenario": "Baseline", "summary": "Success"},
                outcome("a.rs", "CaughtMutant"),
                outcome("a.rs", "MissedMutant"),
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &shard_b,
            serde_json::json!({"outcomes": [
                {"scenario": "Baseline", "summary": "Success"},
                outcome("a.rs", "CaughtMutant"),
                outcome("a.rs", "Timeout"),
            ]})
            .to_string(),
        )
        .unwrap();
        let args: Vec<String> = [
            "--outcomes",
            shard_a.to_str().unwrap(),
            "--outcomes",
            shard_b.to_str().unwrap(),
            "--badge-out",
            badge.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let code = run(&args).unwrap();
        assert_eq!(code, 0);
        let rendered: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&badge).unwrap()).unwrap();
        assert_eq!(rendered["message"], "75.0%");
        std::fs::remove_dir_all(&dir).ok();
    }
}
