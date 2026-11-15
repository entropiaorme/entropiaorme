//! One whole-tree rule over the tracked backend source, enforcing the
//! measured-figures boundary:
//!
//!   - Rule (no account readings in accounting surfaces): the modules that
//!     compute the tracked figures (the same accounting surface the
//!     market-isolation guard holds: the ledger, the analytics aggregates,
//!     the cost engine, the rollups, the tracker's session accounting, the
//!     quest reward accounting) may not import or reference the account
//!     readings module (`account`, its tables, or its facade family).
//!     Figures measured from the account site's readings stand beside the
//!     tracked ones; they never replace or adjust a session's cost, net,
//!     armour cost, healing cost, or loot. The boundary is one-directional:
//!     the account module MAY read what it needs (the equipment library and
//!     the protection sets name what an item is), the accounting layer may
//!     never read a measured figure.
//!
//! What it sees: every `use` statement, joined across lines and expanded
//! through its `{...}` groups, whose path reaches a segment named
//! `account` from `crate`, `super`, `eo_services` or `eo_api` (or from a
//! name one of those roots was renamed to in the same file); any
//! `account::` path outside a `use`; and the account tables by name, so raw
//! SQL cannot smuggle a read past the module boundary. Comment lines are
//! skipped.
//!
//! What it cannot see: a re-export of the account module's items through a
//! module off the accounting surface (`crate::elsewhere::Reading`), a glob
//! import of such a module, or a reference built inside a macro. Those stay
//! a matter for review.
//!
//! Whole-tree rather than diff-scoped, like its siblings: the guarantee is
//! "no consumer anywhere on the accounting surface".

use std::sync::OnceLock;

use regex::Regex;

use crate::git;
use crate::market_isolation::ACCOUNTING_SURFACES;

/// The roots an import of the account module is reached from.
const ROOTS: &[&str] = &["crate", "super", "eo_services", "eo_api"];

/// The account module's tables.
const TABLES: &[&str] = &[
    "account_readings",
    "account_reading_holdings",
    "account_session_measures",
];

/// A single lint violation: file, 1-based line number, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub detail: String,
}

/// Repo-relative tracked Rust paths on the accounting surface.
fn tracked_sources(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", "app/src-tauri"], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| line.ends_with(".rs"))
        .filter(|line| {
            ACCOUNTING_SURFACES
                .iter()
                .any(|surface| line.starts_with(surface) || *line == *surface)
        })
        .map(|line| line.to_string())
        .collect())
}

fn use_start() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(pub(\s*\([^)]*\))?\s+)?use\s+").expect("valid use pattern"))
}

fn account_path() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\baccount\s*::").expect("valid path pattern"))
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("/*")
}

/// Split a use tree's text at its top-level commas.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Every path a use tree names, as `(segments, rename)`.
fn expand(tree: &str, prefix: &[String], out: &mut Vec<(Vec<String>, Option<String>)>) {
    let tree = tree.trim();
    if tree.is_empty() {
        return;
    }
    if let Some(open) = tree.find('{') {
        let head = tree[..open].trim().trim_end_matches("::").trim();
        let mut segments = prefix.to_vec();
        segments.extend(segment_list(head));
        let inner = tree[open + 1..].trim_end().trim_end_matches('}');
        for part in split_top_level(inner) {
            expand(part, &segments, out);
        }
        return;
    }
    let (path, rename) = match tree.split_once(" as ") {
        Some((path, rename)) => (path.trim(), Some(rename.trim().to_string())),
        None => (tree, None),
    };
    let mut segments = prefix.to_vec();
    segments.extend(segment_list(path));
    out.push((segments, rename));
}

fn segment_list(path: &str) -> Vec<String> {
    path.trim()
        .trim_start_matches("::")
        .split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// The use statements of a file, joined across lines, with the line each
/// starts on.
fn use_statements(text: &str) -> Vec<(usize, String)> {
    let mut statements = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        let code = line.split("//").next().unwrap_or_default();
        match open.as_mut() {
            Some((_, statement)) => {
                statement.push(' ');
                statement.push_str(code);
            }
            None => {
                if let Some(found) = use_start().find(code) {
                    open = Some((index + 1, code[found.end()..].to_string()));
                } else {
                    continue;
                }
            }
        }
        if let Some((_, statement)) = &open {
            if let Some(end) = statement.find(';') {
                let (lineno, statement) = open.take().unwrap_or_default();
                statements.push((lineno, statement[..end].to_string()));
            }
        }
    }
    statements
}

/// Apply the rule to one file's text.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let mut findings: Vec<Finding> = Vec::new();
    let statements = use_statements(text);

    // Names the roots are renamed to in this file.
    let mut roots: Vec<String> = ROOTS.iter().map(|root| root.to_string()).collect();
    for (_, statement) in &statements {
        let mut paths = Vec::new();
        expand(statement, &[], &mut paths);
        for (segments, rename) in paths {
            let reaches_a_root = segments.iter().all(|segment| roots.contains(segment));
            if let (true, Some(rename)) = (reaches_a_root && !segments.is_empty(), rename) {
                if rename != "_" {
                    roots.push(rename);
                }
            }
        }
    }

    let mut flagged_lines: Vec<usize> = Vec::new();
    for (lineno, statement) in &statements {
        let mut paths = Vec::new();
        expand(statement, &[], &mut paths);
        let reaches = paths.iter().any(|(segments, _)| {
            segments.first().is_some_and(|first| roots.contains(first))
                && segments.iter().skip(1).any(|segment| segment == "account")
        });
        if reaches {
            flagged_lines.push(*lineno);
            findings.push(Finding {
                path: posix.clone(),
                lineno: *lineno,
                detail: "imports the account readings module; measured figures stand beside \
the tracked ones and never join them"
                    .to_string(),
            });
        }
    }

    let mut in_use = false;
    for (index, line) in text.lines().enumerate() {
        let lineno = index + 1;
        if is_comment(line) {
            continue;
        }
        let code = line.split("//").next().unwrap_or_default();
        if use_start().is_match(code) {
            in_use = true;
        }
        let inside_a_use = in_use;
        if in_use && code.contains(';') {
            in_use = false;
        }
        if flagged_lines.contains(&lineno) {
            continue;
        }
        let detail = if let Some(table) = TABLES.iter().find(|table| line.contains(**table)) {
            Some(format!(
                "references the account readings ({table}); measured figures stand beside \
the tracked ones and never join them"
            ))
        } else if !inside_a_use && account_path().is_match(code) {
            Some(
                "references an account path; measured figures stand beside the tracked ones \
and never join them"
                    .to_string(),
            )
        } else {
            None
        };
        if let Some(detail) = detail {
            findings.push(Finding {
                path: posix.clone(),
                lineno,
                detail,
            });
        }
    }
    findings.sort_by_key(|finding| finding.lineno);
    findings
}

fn evaluate(repo_root: &std::path::Path) -> Result<Vec<Finding>, String> {
    let sources = tracked_sources(repo_root)?;
    let mut findings: Vec<Finding> = Vec::new();
    for path in &sources {
        let full = repo_root.join(path);
        match std::fs::read_to_string(&full) {
            Ok(text) => findings.extend(scan_text(path, &text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("measured-isolation: cannot read {path}: {e}")),
        }
    }
    Ok(findings)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let findings = evaluate(&repo_root)?;

    if findings.is_empty() {
        println!("measured-isolation: no account-reading references on the accounting surface.");
        return Ok(0);
    }

    eprintln!(
        "measured-isolation: measured-figure boundary violations.\n\nFigures measured \
from the account site's readings stand beside the tracked figures: they never \
replace or adjust a session's cost, net, armour cost, healing cost, or loot. The \
accounting surface may not import or query the account readings. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: {}", f.path, f.lineno, f.detail);
    }

    if warn_only {
        eprintln!("\nmeasured-isolation: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE: &str = "app/src-tauri/eo-services/src/session_summary.rs";

    fn lines(text: &str) -> Vec<usize> {
        scan_text(SURFACE, text)
            .iter()
            .map(|finding| finding.lineno)
            .collect()
    }

    #[test]
    fn flags_an_account_import_in_an_accounting_module() {
        let f = scan_text(SURFACE, "use crate::account::measure::SessionMeasure;");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 1);
        assert!(f[0].detail.contains("account"));
    }

    #[test]
    fn flags_an_account_module_named_inside_a_group() {
        assert_eq!(lines("use crate::{account, daily_rollup};"), vec![1]);
        assert_eq!(lines("use eo_services::{account::store, db};"), vec![1]);
        assert_eq!(
            lines("pub(crate) use crate::{db::Db, account::{Reading as R}};"),
            vec![1]
        );
    }

    #[test]
    fn flags_a_group_spread_over_lines() {
        let text = "use crate::{\n    daily_rollup,\n    account::{\n        store,\n    },\n};\nfn f() {}\n";
        assert_eq!(lines(text), vec![1]);
    }

    #[test]
    fn flags_a_reference_from_a_module_whose_parent_is_the_crate_root() {
        assert_eq!(lines("// header\nuse super::account::Reading;\n"), vec![2]);
        assert_eq!(lines("use super::super::{account};"), vec![1]);
    }

    #[test]
    fn flags_an_import_behind_a_renamed_root() {
        let text = "use eo_services as services;\nuse services::account::Reading;\n";
        assert_eq!(lines(text), vec![2]);
        let text = "use crate as root;\nuse root::{account};\n";
        assert_eq!(lines(text), vec![2]);
    }

    #[test]
    fn flags_an_account_path_outside_a_use() {
        assert_eq!(
            lines("fn f() {\n    let m = crate::account::measure_session(&w, &r, &k);\n}\n"),
            vec![2]
        );
        assert_eq!(lines("let r: account::Reading = todo!();"), vec![1]);
    }

    #[test]
    fn flags_raw_sql_against_the_account_tables() {
        for table in TABLES {
            let text = format!("let q = \"SELECT * FROM {table}\";");
            assert_eq!(lines(&text), vec![1], "{table}");
        }
    }

    #[test]
    fn ignores_unrelated_identifiers_and_comments() {
        assert!(lines("let account_balance = 1;\nlet bank_account::x = 2;").is_empty());
        assert!(lines("use crate::accounting::Ledger;").is_empty());
        assert!(lines("// see crate::account::store for the readings\nlet x = 1;").is_empty());
        assert!(lines("use crate::{daily_rollup, db};").is_empty());
        assert!(lines("use other_crate::account::Thing;").is_empty());
    }

    #[test]
    fn the_accounting_surface_is_the_one_the_market_boundary_holds() {
        assert!(ACCOUNTING_SURFACES.contains(&"app/src-tauri/eo-services/src/tracker/"));
        assert!(ACCOUNTING_SURFACES.contains(&"app/src-tauri/eo-services/src/session_summary.rs"));
        assert!(!ACCOUNTING_SURFACES
            .iter()
            .any(|surface| surface.contains("/account/")));
    }

    #[test]
    fn findings_are_path_normalised() {
        let win = "app/src-tauri/eo-services/src/analytics.rs".replace('/', "\\");
        let f = scan_text(&win, "use crate::account::Reading;");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "app/src-tauri/eo-services/src/analytics.rs");
    }
}
