//! One whole-tree rule over the tracked backend source, enforcing the
//! market-data accounting boundary:
//!
//!   - Rule (no market imports in accounting surfaces): the modules that
//!     compute realised figures (the ledger, the analytics aggregates,
//!     the cost engine, the rollups, the tracker's session accounting,
//!     the quest reward accounting) may not import or reference the
//!     market layer (`market_paste`, `market_service`, the `market_*`
//!     tables, the API market family). Estimated markup is an
//!     informational data class; realised P&L is measured truth. The
//!     boundary is one-directional by design: the market layer MAY read
//!     accounting data (loot composition drives its aggregations), the
//!     accounting layer may never read estimated markup.
//!
//! Whole-tree rather than diff-scoped, like the sibling frontend lints:
//! the guarantee is "no consumer anywhere on the accounting surface".

use crate::git;

/// The accounting surface: tracked Rust sources under these paths may
/// not reference the market layer. Extend the list when a new realised
/// -figure surface is minted; never shrink it to admit a consumer.
const ACCOUNTING_SURFACES: &[&str] = &[
    "app/src-tauri/eo-services/src/analytics.rs",
    "app/src-tauri/eo-services/src/cost_engine.rs",
    "app/src-tauri/eo-services/src/daily_rollup.rs",
    "app/src-tauri/eo-services/src/session_summary.rs",
    "app/src-tauri/eo-services/src/tracker/",
    "app/src-tauri/eo-services/src/quests/",
    "app/src-tauri/eo-api/src/analytics.rs",
    "app/src-tauri/eo-api/src/tracking.rs",
];

/// The reference markers. Import paths and qualified uses of the market
/// modules, plus the market tables so raw SQL cannot smuggle a read
/// past the module boundary.
const MARKET_MARKERS: &[&str] = &[
    "market_paste",
    "market_service",
    "crate::market",
    "eo_api::market",
    "market_observations",
    "market_submissions",
    "market_unit_price_observations",
    "market_manual_markups",
    "market_live_nanocube_marks",
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

/// Apply the rule to one file's text.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let mut findings: Vec<Finding> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if let Some(marker) = MARKET_MARKERS.iter().find(|marker| line.contains(**marker)) {
            findings.push(Finding {
                path: posix.clone(),
                lineno: idx + 1,
                detail: format!(
                    "references the market layer ({marker}); estimated markup is \
informational and never joins a realised figure"
                ),
            });
        }
    }
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
            Err(e) => return Err(format!("market-isolation: cannot read {path}: {e}")),
        }
    }
    Ok(findings)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let findings = evaluate(&repo_root)?;

    if findings.is_empty() {
        println!("market-isolation: no market-layer references on the accounting surface.");
        return Ok(0);
    }

    eprintln!(
        "market-isolation: accounting-boundary violations.\n\nEstimated markup \
(the market layer) is an informational data class: it never joins the ledger, \
the analytics aggregates, or any realised P&L figure. The accounting surface \
may not import or query the market layer. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: {}", f.path, f.lineno, f.detail);
    }

    if warn_only {
        eprintln!("\nmarket-isolation: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_market_import_in_an_accounting_module() {
        let f = scan_text(
            "app/src-tauri/eo-services/src/analytics.rs",
            "use crate::market_service::MarketService;",
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 1);
        assert!(f[0].detail.contains("market_service"));
    }

    #[test]
    fn flags_raw_sql_against_the_market_tables() {
        let text = "// header\nlet q = \"SELECT markup_pct FROM market_observations\";\n";
        let f = scan_text("app/src-tauri/eo-services/src/cost_engine.rs", text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 2);
    }

    #[test]
    fn flags_a_qualified_api_path() {
        let f = scan_text(
            "app/src-tauri/eo-api/src/analytics.rs",
            "let rows = crate::market::MarketOverviewRow { .. };",
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn ignores_unrelated_lines() {
        let f = scan_text(
            "app/src-tauri/eo-services/src/analytics.rs",
            "// realised markup lands here via the ledger, never estimated\nlet x = 1;",
        );
        assert!(f.is_empty());
    }

    #[test]
    fn findings_are_path_normalised() {
        let win = "app/src-tauri/eo-services/src/analytics.rs".replace('/', "\\");
        let f = scan_text(&win, "use crate::market_paste::MarketReading;");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "app/src-tauri/eo-services/src/analytics.rs");
    }
}
