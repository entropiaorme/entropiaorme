//! One whole-tree rule over the tracked backend source, keeping a quest
//! reward's arrival out of every kill figure:
//!
//!   - Rule (a kill figure reads `counted_kills`): a quest reward handed in
//!     at its giver arrives as a mob-less loot clump, which the tracker
//!     records as a row of `kills`. Once a hand-in claims it, the row is the
//!     reward's evidence and no longer a kill (`kills.reward_claimed_at`).
//!     Every count, per-kill figure, per-mob breakdown, latest-kill read,
//!     multiplier, and projection of kills reads the `counted_kills` view,
//!     which leaves the claimed rows out. A read of the `kills` table itself
//!     names its allowed case on a one-line comment at the site.
//!
//! The allowed cases:
//!
//! - `every-kill: by-id`: one row found by its id or loot source (an edit,
//!   a reconcile, the claim's own validation), never a set of kills.
//! - `every-kill: spend`: sums of spend, shots, damage, or loot over a
//!   session or context. The shots fired between the last kill and the
//!   reward's arrival are real spend recorded on the arrival's row, and its
//!   loot is zeroed when the claim is made, so the sum stays whole.
//! - `every-kill: write`: a write path (a delete, a backfill, a seed) that
//!   must reach every row.
//!
//! The scan is line-based over the tracked Rust sources of the backend
//! crates (SQL lives in string literals there), skipping comment lines and
//! test code: files named `tests.rs` or `*_tests.rs`, anything under a
//! `tests/` directory, and the rest of a file from its `#[cfg(test)]`
//! line. Migrations are immutable once applied and are out of scope.

use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The scanned crates: every tracked Rust source under these roots.
const SCAN_ROOTS: &[&str] = &[
    "app/src-tauri/eo-services/",
    "app/src-tauri/eo-api/",
    "app/src-tauri/eo-wire/",
    "app/src-tauri/entropia-orme/",
];

/// The allowed cases an annotated site may name.
const ALLOWED_CASES: &[&str] = &["by-id", "spend", "write"];

/// How far above a flagged line the marker may sit (inclusive of the line
/// itself), so the comment above a long multi-line statement covers it.
const MARKER_WINDOW: usize = 16;

/// A read of the table itself: `FROM kills` or `JOIN kills`, the view's
/// name aside.
fn table_read_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(FROM|JOIN)\s+kills\b").expect("valid read pattern"))
}

/// The annotation marker: `every-kill: <case>` inside a line comment.
fn marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"//.*\bevery-kill:\s*([a-z-]+)").expect("valid marker pattern"))
}

/// A single lint violation: file, 1-based line number, rule, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: String, // "unannotated" or "unknown-case"
    pub detail: String,
}

fn is_test_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "tests.rs" || name.ends_with("_tests.rs") || path.contains("/tests/")
}

/// Repo-relative tracked Rust paths under the scanned crates, test files
/// aside.
fn tracked_sources(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", "app/src-tauri"], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| line.ends_with(".rs"))
        .filter(|line| SCAN_ROOTS.iter().any(|root| line.starts_with(root)))
        .filter(|line| !is_test_path(line))
        .map(|line| line.to_string())
        .collect())
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("/*")
}

fn marker_case(line: &str) -> Option<&str> {
    marker_re()
        .captures(line)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

/// Apply the rule to one file's text.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let lines: Vec<&str> = text
        .lines()
        .take_while(|line| line.trim() != "#[cfg(test)]")
        .collect();
    let mut findings: Vec<Finding> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if let Some(case) = marker_case(line) {
            if !ALLOWED_CASES.contains(&case) {
                findings.push(Finding {
                    path: posix.clone(),
                    lineno: idx + 1,
                    rule: "unknown-case".to_string(),
                    detail: format!(
                        "every-kill case {case:?} is not one of {}",
                        ALLOWED_CASES.join(", ")
                    ),
                });
            }
        }
    }

    for (idx, line) in lines.iter().enumerate() {
        if is_comment(line) || !table_read_re().is_match(line) {
            continue;
        }
        let window_start = idx.saturating_sub(MARKER_WINDOW - 1);
        let annotated = lines[window_start..=idx]
            .iter()
            .any(|candidate| marker_case(candidate).is_some());
        if annotated {
            continue;
        }
        findings.push(Finding {
            path: posix.clone(),
            lineno: idx + 1,
            rule: "unannotated".to_string(),
            detail: format!(
                "reads the kills table without an every-kill annotation; read counted_kills \
(a claimed quest reward is not a kill), or annotate the allowed case (every-kill: by-id | \
spend | write) within the {MARKER_WINDOW} lines above"
            ),
        });
    }
    findings
}

fn evaluate(repo_root: &std::path::Path) -> Result<Vec<Finding>, String> {
    let mut findings: Vec<Finding> = Vec::new();
    for path in tracked_sources(repo_root)? {
        let full = repo_root.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(text) => findings.extend(scan_text(&path, &text)),
            // A tracked file deleted from the working tree mid-edit carries
            // no live content to scan.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Any other read failure fails loudly: a guard that silently
            // skips an unreadable source can return a false clean.
            Err(e) => return Err(format!("counted-kills: cannot read {path}: {e}")),
        }
    }
    Ok(findings)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let findings = evaluate(&repo_root)?;

    if findings.is_empty() {
        println!(
            "counted-kills: every read of the kills table names its case; kill figures read \
counted_kills."
        );
        return Ok(0);
    }

    eprintln!(
        "counted-kills: a read of the kills table without its annotation.\n\n\
A quest reward claimed at a hand-in arrives as a kill row but is not a kill. \
Counts, per-kill figures, breakdowns, the latest kill, multipliers and \
projections read the counted_kills view; a read of the table itself names its \
case on a one-line comment at the site: every-kill: by-id | spend | write. \
Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!("\ncounted-kills: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "app/src-tauri/eo-services/src/example.rs";

    #[test]
    fn flags_a_read_of_the_table() {
        for line in [
            "\"SELECT COUNT(*) FROM kills WHERE session_id = ?\"",
            "\"SELECT k.id FROM kill_loot_items li JOIN kills k ON k.id = li.kill_id\"",
            "\"select count(*) from kills k\"",
        ] {
            let f = scan_text(FILE, line);
            assert_eq!(f.len(), 1, "expected one finding for {line}");
            assert_eq!(f[0].rule, "unannotated");
        }
    }

    #[test]
    fn the_view_and_other_tables_pass() {
        for line in [
            "\"SELECT COUNT(*) FROM counted_kills WHERE session_id = ?\"",
            "\"JOIN counted_kills k ON k.id = li.kill_id\"",
            "\"SELECT * FROM kill_loot_items\"",
            "\"SELECT * FROM session_kill_rollups\"",
            "\"UPDATE kills SET loot_total_ped = 0 WHERE id = ?\"",
        ] {
            assert!(scan_text(FILE, line).is_empty(), "false positive on {line}");
        }
    }

    #[test]
    fn a_marker_within_the_window_annotates_the_read() {
        let text = "// every-kill: spend (the arrival's shots are real spend)\nlet cost: f64 = tx.query_row(\n    \"SELECT SUM(cost_ped) FROM kills WHERE session_id = ?\",\n";
        assert!(scan_text(FILE, text).is_empty());
    }

    #[test]
    fn a_marker_too_far_above_does_not_count() {
        let mut text = String::from("// every-kill: spend\n");
        for _ in 0..MARKER_WINDOW {
            text.push_str("let _ = 0;\n");
        }
        text.push_str("\"SELECT SUM(cost_ped) FROM kills\"\n");
        let f = scan_text(FILE, &text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "unannotated");
    }

    #[test]
    fn every_allowed_case_is_accepted_and_an_unknown_one_is_a_finding() {
        for case in ALLOWED_CASES {
            let text = format!("// every-kill: {case}\n\"SELECT id FROM kills\"\n");
            assert!(scan_text(FILE, &text).is_empty(), "case {case} rejected");
        }
        let f = scan_text(FILE, "// every-kill: all\n\"SELECT id FROM kills\"\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "unknown-case");
    }

    #[test]
    fn test_code_and_prose_are_out_of_scope() {
        let text = "/// Counts FROM kills are wrong here.\nfn f() {}\n#[cfg(test)]\nmod tests {\n    \"SELECT COUNT(*) FROM kills\";\n}\n";
        assert!(scan_text(FILE, text).is_empty());
        assert!(is_test_path(
            "app/src-tauri/eo-services/src/quests/tests.rs"
        ));
        assert!(is_test_path(
            "app/src-tauri/eo-services/src/cycled_reconciliation_tests.rs"
        ));
        assert!(is_test_path(
            "app/src-tauri/eo-api/tests/facade/tracking_facade.rs"
        ));
        assert!(!is_test_path(FILE));
    }
}
