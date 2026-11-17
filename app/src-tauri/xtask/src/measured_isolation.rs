//! One whole-tree rule over the tracked backend source, enforcing the
//! measured-figures boundary:
//!
//!   - Rule (measured costs through one resolver): the modules that compute
//!     the session figures ([`surfaces`]: the accounting surface the
//!     market-isolation guard holds (the ledger, the analytics aggregates,
//!     the cost engine, the rollups, the tracker's session accounting, the
//!     quest reward accounting, the cost resolver, and every reader of a
//!     session's costs), plus [`COST_READERS`], readers of a session's
//!     costs that guard cannot hold) may not import or
//!     reference the account readings module (`account`, or its facade
//!     family), nor name its readings or measures. A session's measured
//!     costs reach them as plain rows (`account_session_costs` and its
//!     bucket, line, armour part, gap and top-up tables, and a running
//!     session's costs so far, `account_live_session_costs`), read by
//!     exactly one file, the resolver ([`RESOLVER`]), which chooses the
//!     measured or the tracked figure per bucket (or, for a running
//!     session, blends them) and knows nothing of readings; what it resolves
//!     them to (`session_cost_resolutions` and its buckets and mixed
//!     armour and weapon) is its own, read
//!     elsewhere only through the SQL it hands out. Any other accounting
//!     file naming either set of rows fails, as does any account import
//!     anywhere on the surface, the resolver included. The boundary is one-directional:
//!     the account module MAY read what it needs (the equipment library and
//!     the protection sets name what an item is) and ask the resolver to
//!     refresh what is derived from the rows it stores.
//!
//! What it sees: every `use` statement, joined across lines and expanded
//! through its `{...}` groups, whose path reaches a segment named
//! `account` from `crate`, `super`, `eo_services` or `eo_api` (or from a
//! name one of those roots was renamed to in the same file, including a
//! rename through `self as` inside a group), or starts at a bare `account`
//! segment (as one may after a glob of the crate root); any `account::`
//! path outside a `use`; and the account tables by name, so raw SQL cannot
//! smuggle a read past the module boundary. Comment lines are skipped.
//!
//! What it cannot see: a re-export of the account module's items through a
//! module off the accounting surface (`crate::elsewhere::Reading`), a glob
//! of a module off the surface that re-exports them, or a reference built
//! inside a macro. Tables are matched as substrings of a code line, so a
//! name assembled at run time or split across literals escapes it, and a
//! longer name containing a table's is flagged as that table. Those stay
//! a matter for review.
//!
//! Whole-tree rather than diff-scoped, like its siblings: the guarantee is
//! "no consumer anywhere on the accounting surface but the resolver".

use std::sync::OnceLock;

use regex::Regex;

use crate::git;
use crate::market_isolation::ACCOUNTING_SURFACES;

/// The roots an import of the account module is reached from.
const ROOTS: &[&str] = &["crate", "super", "eo_services", "eo_api"];

/// The account module's tables no accounting file may name: the readings,
/// the measures, and the chat log's verdicts on their gaps.
const TABLES: &[&str] = &[
    "account_readings",
    "account_reading_holdings",
    "account_session_gap_verdicts",
    "account_session_measures",
];

/// The stored measured costs, what was published of a running session's
/// costs so far, and what the resolver keeps of them: rows only
/// [`RESOLVER`] may name.
const COST_TABLES: &[&str] = &[
    "account_live_session_costs",
    "account_session_cost_armour_parts",
    "account_session_cost_buckets",
    "account_session_cost_gaps",
    "account_session_cost_lines",
    "account_session_cost_top_ups",
    "account_session_costs",
    "session_cost_resolution_armour",
    "session_cost_resolution_buckets",
    "session_cost_resolution_weapon",
    "session_cost_resolutions",
];

/// Readers of a session's costs the market-isolation guard cannot hold
/// (a test there reads the market module's own source), held here.
pub const COST_READERS: &[&str] = &["app/src-tauri/eo-services/src/session_rollup.rs"];

/// Every path this guard holds: the shared accounting surface and
/// [`COST_READERS`].
pub fn surfaces() -> impl Iterator<Item = &'static str> {
    ACCOUNTING_SURFACES.iter().chain(COST_READERS).copied()
}

/// The one accounting file that reads the stored measured costs.
pub const RESOLVER: &str = "app/src-tauri/eo-services/src/session_costs.rs";

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
        .filter(|line| surfaces().any(|surface| line.starts_with(surface) || *line == surface))
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
    // `self` inside a group names the group's own prefix.
    if segments.last().is_some_and(|last| last == "self") {
        segments.pop();
    }
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
            let from_a_root = segments.first().is_some_and(|first| roots.contains(first))
                && segments.iter().skip(1).any(|segment| segment == "account");
            let bare = segments.first().is_some_and(|first| first == "account");
            from_a_root || bare
        });
        if reaches {
            flagged_lines.push(*lineno);
            findings.push(Finding {
                path: posix.clone(),
                lineno: *lineno,
                detail: "imports the account readings module; measured costs reach the \
accounting surface only as rows the cost resolver reads"
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
                "references the account readings ({table}); measured costs reach the \
accounting surface only as rows the cost resolver reads"
            ))
        } else if let Some(table) = COST_TABLES
            .iter()
            .find(|table| line.contains(**table))
            .filter(|_| posix != RESOLVER)
        {
            Some(format!(
                "reads the stored measured costs ({table}) outside the cost resolver \
({RESOLVER}); every reader takes them from the resolver"
            ))
        } else if !inside_a_use && account_path().is_match(code) {
            Some(
                "references an account path; measured costs reach the accounting surface \
only as rows the cost resolver reads"
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
        println!(
            "measured-isolation: no account-reading references on the accounting surface, \
and the measured costs are read by the resolver alone."
        );
        return Ok(0);
    }

    eprintln!(
        "measured-isolation: measured-figure boundary violations.\n\nA session's measured \
costs reach the accounting surface only as the stored cost rows, read by the cost \
resolver ({RESOLVER}) alone. The accounting surface may not import the account \
module or query its readings or measures, and no other accounting file may read \
the cost rows. Offenders:\n"
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
    fn flags_a_bare_account_import_and_a_root_renamed_through_self() {
        let text = "use crate::*;\nuse account::Reading;\n";
        assert_eq!(lines(text), vec![2]);
        let text = "use eo_services::{self as services, db};\nuse services::account::Reading;\n";
        assert_eq!(lines(text), vec![2]);
        let text = "use crate::{self as root};\nuse root::{account};\n";
        assert_eq!(lines(text), vec![2]);
        assert!(lines("use crate::{self as root};\nuse root::db;\n").is_empty());
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
        for table in TABLES.iter().chain(COST_TABLES) {
            let text = format!("let q = \"SELECT * FROM {table}\";");
            assert_eq!(lines(&text), vec![1], "{table}");
        }
    }

    fn resolver_lines(text: &str) -> Vec<usize> {
        scan_text(RESOLVER, text)
            .iter()
            .map(|finding| finding.lineno)
            .collect()
    }

    #[test]
    fn the_resolver_alone_may_read_the_stored_costs() {
        for table in COST_TABLES {
            let text = format!("let q = \"SELECT * FROM {table}\";");
            assert!(resolver_lines(&text).is_empty(), "{table}");
            // The same text anywhere else on the surface fails.
            assert_eq!(lines(&text), vec![1], "{table}");
            let elsewhere = scan_text("app/src-tauri/eo-services/src/daily_rollup.rs", &text);
            assert_eq!(elsewhere.len(), 1, "{table}");
            assert!(elsewhere[0].detail.contains("outside the cost resolver"));
        }
    }

    #[test]
    fn a_running_sessions_costs_so_far_are_read_by_the_resolver_alone() {
        let text = "let q = \"SELECT * FROM account_live_session_costs\";";
        assert!(resolver_lines(text).is_empty());
        for reader in [
            "app/src-tauri/eo-services/src/tracker/session.rs",
            "app/src-tauri/eo-services/src/tracker/cycled.rs",
            "app/src-tauri/eo-api/src/tracking.rs",
        ] {
            let found = scan_text(reader, text);
            assert_eq!(found.len(), 1, "{reader}");
            assert!(found[0].detail.contains("account_live_session_costs"));
            assert!(found[0].detail.contains("outside the cost resolver"));
        }
    }

    #[test]
    fn the_resolver_may_not_read_the_readings_or_import_the_account_module() {
        for table in TABLES {
            let text = format!("let q = \"SELECT * FROM {table}\";");
            assert_eq!(resolver_lines(&text), vec![1], "{table}");
        }
        assert_eq!(
            resolver_lines("use crate::account::costs::SessionCosts;"),
            vec![1]
        );
        assert_eq!(
            resolver_lines("fn f() {\n    let v = crate::account::costs::COSTS_VERSION;\n}\n"),
            vec![2]
        );
    }

    #[test]
    fn the_resolver_is_on_the_accounting_surface() {
        assert!(ACCOUNTING_SURFACES.contains(&RESOLVER));
    }

    #[test]
    fn every_reader_of_a_sessions_costs_is_held() {
        for reader in [
            "app/src-tauri/eo-services/src/tracking_reads.rs",
            "app/src-tauri/eo-services/src/session_rollup.rs",
            "app/src-tauri/eo-services/src/session_weapon_rollup.rs",
            "app/src-tauri/eo-services/src/skilling_forecast.rs",
            "app/src-tauri/eo-services/src/session_definitions.rs",
            "app/src-tauri/eo-services/src/session_expectation.rs",
            "app/src-tauri/eo-services/src/loaded_expectation.rs",
            "app/src-tauri/eo-services/src/maintenance.rs",
            "app/src-tauri/eo-services/src/daily_rollup.rs",
            "app/src-tauri/eo-services/src/session_summary.rs",
        ] {
            assert!(surfaces().any(|surface| surface == reader), "{reader}");
            let text = "use crate::account::costs::SessionCosts;";
            assert_eq!(scan_text(reader, text).len(), 1, "{reader}");
        }
        // The deletion that lets the account module go of a session is not
        // a cost reader.
        assert!(!surfaces().any(|surface| surface.ends_with("session_delete.rs")));
    }

    #[test]
    fn the_resolution_is_the_resolvers_own() {
        for table in [
            "session_cost_resolutions",
            "session_cost_resolution_buckets",
            "session_cost_resolution_weapon",
        ] {
            let text = format!("let q = \"SELECT * FROM {table}\";");
            assert!(resolver_lines(&text).is_empty(), "{table}");
            let elsewhere = scan_text("app/src-tauri/eo-services/src/tracking_reads.rs", &text);
            assert_eq!(elsewhere.len(), 1, "{table}");
            assert!(elsewhere[0].detail.contains(table));
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
