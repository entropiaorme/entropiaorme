//! The frontend has one text-search implementation, `app/src/lib/view/search.ts`:
//! every search box, picker, and filter matches through it, so a query behaves
//! the same wherever it is typed (any word order, typo tolerance, quoted
//! phrases, exclusions, ranking). Two whole-tree rules keep it the only one:
//!
//!   - Rule A (no ad hoc matching): a lower-cased string tested with
//!     `includes` / `indexOf` / `startsWith` / `endsWith` on the same line,
//!     in either direction (`name.toLowerCase().includes(q)` or
//!     `name.includes(q.toLowerCase())`), is the hand-rolled substring search
//!     the shared module replaced. Route the match through `filterByQuery`,
//!     `rankByQuery`, `matchesQuery`, or `createSearchIndex` instead.
//!   - Rule B (single matcher home): the fuzzy-matching library is imported
//!     only by the shared module, so no surface can configure a matcher of
//!     its own.
//!
//! Like the polling guard, this is WHOLE-TREE: the tree was driven to zero
//! offending sites, so the guarantee is "zero anywhere". The source set is the
//! `git ls-files`-tracked compiled-source files under `app/src`, tests
//! included. A genuine non-search comparison (an exact, case-insensitive
//! equality, say) is written with `===` and never trips Rule A.

use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The shared search module: the only file permitted to import the matcher.
const SEARCH_HOME: &str = "app/src/lib/view/search.ts";

/// The fuzzy-matching library the shared module wraps.
const MATCHER_PACKAGE: &str = "@leeoniya/ufuzzy";

const SCAN_ROOT: &str = "app/src";
const SCAN_SUFFIXES: &[&str] = &[".svelte", ".ts", ".js", ".mjs", ".cjs", ".jsx", ".tsx"];

/// A lower-cased value tested for containment, either side of the test.
fn adhoc_match_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            // The needle side admits one level of nested calls inside the
            // argument (`query.trim().toLowerCase()`).
            r"to(?:Locale)?LowerCase\(\)\s*\.\s*(?:includes|indexOf|startsWith|endsWith)\s*\(|\.(?:includes|indexOf|startsWith|endsWith)\s*\((?:[^()]|\([^()]*\))*?to(?:Locale)?LowerCase\(\)",
        )
        .expect("valid ad hoc match pattern")
    })
}

/// A module specifier naming the matcher package (import, dynamic import,
/// or re-export).
fn matcher_import_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r#"['"]{}['"/]"#, regex::escape(MATCHER_PACKAGE)))
            .expect("valid matcher import pattern")
    })
}

/// A single lint violation: file, 1-based line number, rule, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: String, // "adhoc-search" or "matcher-import"
    pub detail: String,
}

fn tracked_sources(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", SCAN_ROOT], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| SCAN_SUFFIXES.iter().any(|s| line.ends_with(s)))
        .map(|line| line.to_string())
        .collect())
}

/// Apply both whole-tree rules to one file's text.
pub fn scan_text(path: &str, text: &str) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let posix = path.replace('\\', "/");
    let is_home = posix == SEARCH_HOME;
    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        if adhoc_match_re().is_match(line) {
            findings.push(Finding {
                path: posix.clone(),
                lineno,
                rule: "adhoc-search".to_string(),
                detail: format!(
                    "hand-rolled case-insensitive substring match; search through \
{SEARCH_HOME} (filterByQuery, rankByQuery, matchesQuery, createSearchIndex)"
                ),
            });
        }
        if !is_home && matcher_import_re().is_match(line) {
            findings.push(Finding {
                path: posix.clone(),
                lineno,
                rule: "matcher-import".to_string(),
                detail: format!(
                    "'{MATCHER_PACKAGE}' imported outside {SEARCH_HOME}; use the \
shared search instead of configuring another matcher"
                ),
            });
        }
    }
    findings
}

fn evaluate(repo_root: &std::path::Path) -> Result<Vec<Finding>, String> {
    let mut findings: Vec<Finding> = Vec::new();
    for path in tracked_sources(repo_root)? {
        let full = repo_root.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(text) => findings.extend(scan_text(&path, &text)),
            // A tracked file deleted from the working tree carries no live
            // content to scan.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Any other read failure must fail loudly: a guard that skips an
            // unreadable source can return a false clean.
            Err(e) => return Err(format!("single-search: cannot read {path}: {e}")),
        }
    }
    Ok(findings)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let findings = evaluate(&repo_root)?;

    if findings.is_empty() {
        println!("single-search: every frontend search matches through the shared module.");
        return Ok(0);
    }

    eprintln!(
        "single-search: frontend search outside the shared module.\n\n\
Every search box, picker, and filter matches through {SEARCH_HOME}, so a \
query behaves the same everywhere. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!("\nsingle-search: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(path: &str, text: &str) -> Vec<String> {
        scan_text(path, text).into_iter().map(|f| f.rule).collect()
    }

    #[test]
    fn flags_a_lowercased_haystack_tested_for_containment() {
        for line in [
            "rows.filter((r) => r.name.toLowerCase().includes(q))",
            "label.toLocaleLowerCase().startsWith(needle)",
            "text.toLowerCase() .indexOf(term) >= 0",
            "name.toLowerCase().endsWith(suffix)",
        ] {
            assert_eq!(rules("app/src/a.ts", line), ["adhoc-search"], "{line}");
        }
    }

    #[test]
    fn flags_a_lowercased_needle_tested_for_containment() {
        let f = scan_text(
            "app/src/a.svelte",
            "haystack.includes(query.trim().toLowerCase())",
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "adhoc-search");
    }

    #[test]
    fn allows_exact_comparisons_and_unrelated_lowercasing() {
        for line in [
            "if (tag.toLowerCase() === typed) continue;",
            "const key = name.toLowerCase();",
            "ids.includes(id)",
            "['a', 'b'].includes(e.key)",
        ] {
            assert!(rules("app/src/a.ts", line).is_empty(), "{line}");
        }
    }

    #[test]
    fn rule_a_binds_the_home_module_too() {
        let f = scan_text(SEARCH_HOME, "x.toLowerCase().includes(y)");
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn flags_the_matcher_imported_outside_its_home() {
        for line in [
            "import uFuzzy from '@leeoniya/ufuzzy';",
            "const m = await import(\"@leeoniya/ufuzzy\");",
            "export { default } from '@leeoniya/ufuzzy/dist/uFuzzy.mjs';",
        ] {
            assert_eq!(
                rules("app/src/lib/x.ts", line),
                ["matcher-import"],
                "{line}"
            );
        }
    }

    #[test]
    fn allows_the_matcher_in_its_home() {
        assert!(rules(SEARCH_HOME, "import uFuzzy from '@leeoniya/ufuzzy';").is_empty());
        let win = SEARCH_HOME.replace('/', "\\");
        assert!(rules(&win, "import uFuzzy from '@leeoniya/ufuzzy';").is_empty());
    }

    #[test]
    fn reports_correct_line_numbers() {
        let f = scan_text("app/src/a.ts", "one\ntwo\nx.toLowerCase().includes(y)\n");
        assert_eq!(f[0].lineno, 3);
    }
}
