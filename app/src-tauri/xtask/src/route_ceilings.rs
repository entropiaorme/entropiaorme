//! One whole-tree rule over the tracked route files, holding the route-shell
//! decomposition in place:
//!
//!   - Rule (route files stay thin): every tracked `.svelte` file under
//!     `app/src/routes` must stay at or under its line ceiling. Files listed
//!     below carry a frozen per-file ceiling (their size when the guard was
//!     adopted, plus modest headroom); everything else is held to the
//!     default. Ceilings only ever move DOWN: when decomposition shrinks a
//!     file, lower its ceiling in the same change; never raise one to make a
//!     growing file pass. A route outgrowing its ceiling moves logic into a
//!     `lib/features` module (the established decomposition), it does not
//!     negotiate with the guard.
//!
//! Whole-tree rather than diff-scoped, like the sibling frontend guards: the
//! guarantee is "no route file anywhere has regrown", and gradual growth
//! across many small diffs is exactly the failure mode being caught. The
//! stale-entry check fails on any listed file that is no longer tracked, so
//! the frozen surface only shrinks.

use crate::git;

/// Ceiling for any tracked route file without an adopted entry below. New
/// route files are expected to be born decomposed (a thin shell over a
/// `lib/features` module).
const DEFAULT_CEILING: usize = 300;

/// The frozen over-default surface: every route file above the default at
/// adoption time, with headroom for ordinary edits. Entries are removed (or
/// lowered) as decomposition shrinks a file; nothing adds one or raises one.
const CEILINGS: &[(&str, usize)] = &[
    ("app/src/routes/updates/+page.svelte", 425),
    ("app/src/routes/analytics/OverviewTab.svelte", 575),
    ("app/src/routes/news/+page.svelte", 575),
    ("app/src/routes/scan-overlay/+page.svelte", 600),
    ("app/src/routes/settings/+page.svelte", 625),
    ("app/src/routes/character/CodexTab.svelte", 675),
    ("app/src/routes/analytics/LedgerTab.svelte", 700),
    ("app/src/routes/welcome/+page.svelte", 875),
    ("app/src/routes/overlay/+page.svelte", 1020),
];

const SCAN_ROOT: &str = "app/src/routes";
const SCAN_SUFFIX: &str = ".svelte";

/// A single guard violation: file, measured lines, ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lines: usize,
    pub ceiling: usize,
}

/// Repo-relative tracked `.svelte` paths under `app/src/routes`.
fn tracked_routes(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", SCAN_ROOT], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| line.ends_with(SCAN_SUFFIX))
        .map(|line| line.to_string())
        .collect())
}

/// The ceiling a path is held to.
fn ceiling_for(path: &str) -> usize {
    CEILINGS
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, c)| *c)
        .unwrap_or(DEFAULT_CEILING)
}

/// Apply the rule to one file's text.
pub fn check_text(path: &str, text: &str) -> Option<Finding> {
    let posix = path.replace('\\', "/");
    let lines = text.lines().count();
    let ceiling = ceiling_for(&posix);
    (lines > ceiling).then_some(Finding {
        path: posix,
        lines,
        ceiling,
    })
}

/// Measure every tracked route file and return the violations, plus the
/// ceiling entries that no longer exist (stale entries must be pruned so the
/// frozen surface only shrinks).
fn evaluate(repo_root: &std::path::Path) -> Result<(Vec<Finding>, Vec<String>), String> {
    let routes = tracked_routes(repo_root)?;
    let mut findings: Vec<Finding> = Vec::new();
    for path in &routes {
        let full = repo_root.join(path);
        match std::fs::read_to_string(&full) {
            Ok(text) => findings.extend(check_text(path, &text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("check-route-ceilings: cannot read {path}: {e}")),
        }
    }
    let stale: Vec<String> = CEILINGS
        .iter()
        .filter(|(entry, _)| !routes.iter().any(|r| r == entry))
        .map(|(entry, _)| entry.to_string())
        .collect();
    Ok((findings, stale))
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let (findings, stale) = evaluate(&repo_root)?;

    if findings.is_empty() && stale.is_empty() {
        println!("check-route-ceilings: every tracked route file is within its line ceiling.");
        return Ok(0);
    }

    if !findings.is_empty() {
        eprintln!(
            "check-route-ceilings: route files over their line ceiling.\n\n\
Routes are thin shells over lib/features modules; a route outgrowing its \
ceiling moves logic into its feature module. Ceilings only ratchet down. \
Offenders:\n"
        );
        for f in &findings {
            eprintln!("  {}: {} lines (ceiling {})", f.path, f.lines, f.ceiling);
        }
    }
    if !stale.is_empty() {
        eprintln!(
            "\ncheck-route-ceilings: stale ceiling entries (file no longer \
tracked); remove them from CEILINGS so the frozen surface shrinks:\n"
        );
        for entry in &stale {
            eprintln!("  {entry}");
        }
    }

    if warn_only {
        eprintln!("\ncheck-route-ceilings: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: usize) -> String {
        "x\n".repeat(lines)
    }

    #[test]
    fn a_new_route_over_the_default_is_flagged() {
        let f = check_text("app/src/routes/shiny/+page.svelte", &text_of(301)).unwrap();
        assert_eq!(f.lines, 301);
        assert_eq!(f.ceiling, DEFAULT_CEILING);
    }

    #[test]
    fn a_new_route_at_the_default_passes() {
        assert!(check_text("app/src/routes/shiny/+page.svelte", &text_of(300)).is_none());
    }

    #[test]
    fn a_mapped_route_is_held_to_its_own_ceiling() {
        assert!(check_text("app/src/routes/overlay/+page.svelte", &text_of(1000)).is_none());
        let f = check_text("app/src/routes/overlay/+page.svelte", &text_of(1021)).unwrap();
        assert_eq!(f.ceiling, 1020);
    }

    #[test]
    fn findings_are_path_normalised() {
        let win = "app/src/routes/shiny/+page.svelte".replace('/', "\\");
        let f = check_text(&win, &text_of(400)).unwrap();
        assert_eq!(f.path, "app/src/routes/shiny/+page.svelte");
    }

    #[test]
    fn every_adopted_ceiling_exceeds_the_default() {
        // An entry at or under the default would be dead weight; the map
        // carries only the over-default surface.
        for (path, ceiling) in CEILINGS {
            assert!(
                *ceiling > DEFAULT_CEILING,
                "{path}: ceiling {ceiling} not above the default"
            );
        }
    }

    #[test]
    fn ceiling_map_has_the_expected_entry_count() {
        // Guards against an accidental drop when editing the map.
        assert_eq!(CEILINGS.len(), 9);
    }
}
