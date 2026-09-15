//! One whole-tree rule over the tracked backend source, enforcing the
//! latest-in-time convention for database reads:
//!
//!   - Rule (id order is never a time order): a read that means "the
//!     latest in time" orders by the row's timestamp column, with the id
//!     as the tiebreak only. Rows are not guaranteed to arrive in
//!     chronological order (a scan recorded from an older screenshot
//!     after a newer one, a backup restored beside newer rows, a replayed
//!     chat log, a backfill writing older data with higher ids), so a
//!     `MAX(id)` or `ORDER BY id DESC LIMIT 1` that stands in for "the
//!     newest" reports the wrong row. A read that genuinely orders by id
//!     says which allowed case it is, on a one-line comment at the site.
//!
//! The allowed cases:
//!
//! - `id-order: cursor`: a stream position, the highest id seen so far or
//!   the rows on one side of a recorded position.
//! - `id-order: retention`: a window of the most recently appended rows
//!   of an append-only journal.
//! - `id-order: tiebreak`: rows already narrowed to one timestamp; the id
//!   only separates equal instants.
//! - `id-order: insertion`: insertion order is itself the meaning, such as
//!   a seeded default, the current entry of a stream only ever appended
//!   in process order, or a test reading what it just wrote.
//!
//! The scan is line-based over the tracked Rust sources of the backend
//! crates (SQL lives in string literals there). Comment lines are
//! skipped, so prose about the patterns does not trip the guard; the
//! marker itself is a comment and must sit on the flagged line or within
//! the few lines above it, so an annotation reads beside the statement it
//! explains rather than somewhere in the file. Migrations are immutable
//! once applied and are out of scope.
//!
//! Whole-tree rather than diff-scoped, like the sibling guards: every
//! id-ordered read in the tree is annotated, so the guarantee is "no
//! unannotated site anywhere", and a new one cannot land silently.

use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The scanned crates: every tracked Rust source under these roots.
/// The task runner itself is excluded (this guard's own tests spell the
/// patterns out).
const SCAN_ROOTS: &[&str] = &[
    "app/src-tauri/eo-services/",
    "app/src-tauri/eo-api/",
    "app/src-tauri/eo-wire/",
    "app/src-tauri/entropia-orme/",
];

/// The allowed cases an annotated site may name.
const ALLOWED_CASES: &[&str] = &["cursor", "retention", "tiebreak", "insertion"];

/// How far above a flagged line the marker may sit (inclusive of the
/// line itself). SQL string literals span several lines, so the comment
/// above the statement is typically a handful of lines above the
/// pattern it covers.
const MARKER_WINDOW: usize = 12;

/// The id-ordering shapes, one regex each, matched case-insensitively
/// against a source line. An `id`, `rowid`, or `<name>_id` column that
/// leads an aggregate, leads an `ORDER BY` with `DESC` or with a `LIMIT`,
/// or bounds a placeholder comparison.
fn patterns() -> &'static [Regex] {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        [
            // MAX(id), MIN(s2.id), MAX(o2.submission_id), COALESCE(MAX(rowid), 0)
            r"(?i)\b(MAX|MIN)\(\s*(\w+\.)?(id|rowid|\w+_id)\s*\)",
            // ORDER BY id DESC, ORDER BY c.id DESC LIMIT 1
            r"(?i)\bORDER BY\s+(\w+\.)?(id|rowid|\w+_id)\s+DESC\b",
            // ORDER BY id LIMIT 1, ORDER BY i.id ASC LIMIT 1
            r"(?i)\bORDER BY\s+(\w+\.)?(id|rowid|\w+_id)(\s+ASC)?\s+LIMIT\b",
            // d.id > ?1, id < ?, c.id >= :marker
            r"(?i)\b(id|rowid|\w+_id)\s*(>=|<=|>|<)\s*(\?|:\w)",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("valid id-order pattern"))
        .collect()
    })
}

/// The annotation marker: `id-order: <case>` inside a line comment.
fn marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"//.*\bid-order:\s*([a-z-]+)").expect("valid marker pattern"))
}

/// A single lint violation: file, 1-based line number, rule, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: String, // "unannotated" or "unknown-case"
    pub detail: String,
}

/// Repo-relative tracked Rust paths under the scanned crates.
fn tracked_sources(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", "app/src-tauri"], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| line.ends_with(".rs"))
        .filter(|line| SCAN_ROOTS.iter().any(|root| line.starts_with(root)))
        .map(|line| line.to_string())
        .collect())
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("/*")
}

/// The case named by a marker on this line, if the line carries one.
fn marker_case(line: &str) -> Option<&str> {
    marker_re()
        .captures(line)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

/// Apply the rule to one file's text.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let lines: Vec<&str> = text.lines().collect();
    let mut findings: Vec<Finding> = Vec::new();

    // Every marker is checked for a known case, wherever it sits.
    for (idx, line) in lines.iter().enumerate() {
        if let Some(case) = marker_case(line) {
            if !ALLOWED_CASES.contains(&case) {
                findings.push(Finding {
                    path: posix.clone(),
                    lineno: idx + 1,
                    rule: "unknown-case".to_string(),
                    detail: format!(
                        "id-order case {case:?} is not one of {}",
                        ALLOWED_CASES.join(", ")
                    ),
                });
            }
        }
    }

    for (idx, line) in lines.iter().enumerate() {
        if is_comment(line) {
            continue;
        }
        let Some(pattern) = patterns().iter().find(|re| re.is_match(line)) else {
            continue;
        };
        let window_start = idx.saturating_sub(MARKER_WINDOW - 1);
        let annotated = lines[window_start..=idx]
            .iter()
            .any(|candidate| marker_case(candidate).is_some());
        if annotated {
            continue;
        }
        let shape = pattern
            .find(line)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        findings.push(Finding {
            path: posix.clone(),
            lineno: idx + 1,
            rule: "unannotated".to_string(),
            detail: format!(
                "id-ordered read ({shape}) without an id-order annotation; order by the \
timestamp column with id as the tiebreak, or annotate the allowed case \
(id-order: cursor | retention | tiebreak | insertion) within the {MARKER_WINDOW} \
lines above"
            ),
        });
    }
    findings
}

/// Scan the tracked backend source and return every finding.
fn evaluate(repo_root: &std::path::Path) -> Result<Vec<Finding>, String> {
    let mut findings: Vec<Finding> = Vec::new();
    for path in tracked_sources(repo_root)? {
        let full = repo_root.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(text) => findings.extend(scan_text(&path, &text)),
            // `git ls-files` enumerates the index, so a tracked file deleted
            // from the working tree (an unstaged deletion mid-edit) is
            // legitimately absent on disk and carries no live content to scan.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Any other read failure must fail loudly: a guard that silently
            // skips an unreadable source can return a false clean.
            Err(e) => return Err(format!("check-id-order: cannot read {path}: {e}")),
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
            "check-id-order: every id-ordered read in the backend source names its \
allowed case."
        );
        return Ok(0);
    }

    eprintln!(
        "check-id-order: id order used where the latest in time is meant, or an \
id-ordered read without its annotation.\n\n\
A read that means \"the latest in time\" orders by the row's timestamp column \
with id as the tiebreak only. A read that genuinely orders by id names its case \
on a one-line comment at the site: id-order: cursor | retention | tiebreak | \
insertion. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!("\ncheck-id-order: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "app/src-tauri/eo-services/src/example.rs";

    #[test]
    fn flags_a_max_id_read_without_a_marker() {
        let text = "let level = conn.query_row(\n    \"SELECT level FROM t WHERE id = (SELECT MAX(id) FROM t)\",\n";
        let f = scan_text(FILE, text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "unannotated");
        assert_eq!(f[0].lineno, 2);
        assert!(f[0].detail.contains("MAX(id)"));
    }

    #[test]
    fn flags_order_by_id_desc_and_limit_forms() {
        for line in [
            "\"SELECT id FROM t ORDER BY id DESC LIMIT 1\"",
            "\"SELECT id FROM t ORDER BY c.id DESC\"",
            "\"SELECT id FROM t ORDER BY id LIMIT 1\"",
            "\"SELECT id FROM t ORDER BY i.id ASC LIMIT 1\"",
            "\"SELECT id FROM t ORDER BY rowid DESC LIMIT 1\"",
            "\"SELECT MAX(o2.submission_id) FROM o o2\"",
            "\"WHERE d.id > ?1 AND d.id <= ?2\"",
            "\"WHERE c.id >= :marker\"",
        ] {
            let f = scan_text(FILE, line);
            assert_eq!(f.len(), 1, "expected one finding for {line}");
            assert_eq!(f[0].rule, "unannotated");
        }
    }

    #[test]
    fn allows_timestamp_first_ordering_with_id_as_tiebreak() {
        for line in [
            "\"ORDER BY scanned_at DESC, id DESC LIMIT 1\"",
            "\"ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1\"",
            "\"SELECT MAX(scanned_at) FROM skill_calibrations\"",
            "\"SELECT id, name FROM t ORDER BY id\"",
            "\"SELECT id FROM t ORDER BY o.item_name, o.id\"",
            "\"WHERE id = ?1\"",
            ".filter(|id| *id > 0)",
            "let id = tx.last_insert_rowid();",
        ] {
            assert!(scan_text(FILE, line).is_empty(), "false positive on {line}");
        }
    }

    #[test]
    fn a_marker_within_the_window_annotates_the_read() {
        let text = "// id-order: cursor (the highest id seen so far)\nlet cursor: i64 = tx.query_row(\n    \"SELECT COALESCE(MAX(id), 0) FROM events\",\n";
        assert!(scan_text(FILE, text).is_empty());
    }

    #[test]
    fn a_marker_too_far_above_does_not_count() {
        let mut text = String::from("// id-order: cursor\n");
        for _ in 0..MARKER_WINDOW {
            text.push_str("let _ = 0;\n");
        }
        text.push_str("\"SELECT MAX(id) FROM events\"\n");
        let f = scan_text(FILE, &text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "unannotated");
    }

    #[test]
    fn every_allowed_case_is_accepted() {
        for case in ALLOWED_CASES {
            let text = format!("// id-order: {case}\n\"SELECT MAX(id) FROM t\"\n");
            assert!(scan_text(FILE, &text).is_empty(), "case {case} rejected");
        }
    }

    #[test]
    fn an_unknown_case_is_a_finding() {
        let text = "// id-order: newest\n\"SELECT MAX(id) FROM t\"\n";
        let f = scan_text(FILE, text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "unknown-case");
        assert_eq!(f[0].lineno, 1);
    }

    #[test]
    fn prose_in_comments_does_not_trip_the_guard() {
        let text = "/// Latest per skill: MAX(scanned_at) with MAX(id) as the tiebreaker.\n//! ORDER BY id DESC is never a time order.\n";
        assert!(scan_text(FILE, text).is_empty());
    }

    #[test]
    fn reports_path_normalised_to_posix() {
        let win = FILE.replace('/', "\\");
        let f = scan_text(&win, "\"SELECT MAX(id) FROM t\"");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, FILE);
    }
}
