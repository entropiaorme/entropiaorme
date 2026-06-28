//! Port of `backend/scripts/check_no_bare_setinterval.py`.
//!
//! Two whole-tree rules over the tracked frontend source, enforcing the
//! visibility-aware polling discipline so the hidden-window-polling smell (and
//! the retired window-to-window tracking event) cannot grow back:
//!
//!   - Rule A (single-home setInterval): the raw `setInterval(` token may appear
//!     ONLY in the sanctioned helper module
//!     `frontend/src/lib/realtime/useVisiblePoll.ts`. Every other timer-driven
//!     loop must route through `useVisiblePoll` (or its `windowGeometryPoll`
//!     variant), which clears the timer while its surface is hidden.
//!   - Rule B (no legacy lifecycle event): the string `tracking-state-changed`
//!     must not appear anywhere in the frontend source. That window-to-window
//!     event was retired in favour of the typed `tracking:session:updated`
//!     topic; a re-introduction is a regression.
//!
//! This lint is WHOLE-TREE rather than diff-scoped: the tree was driven to zero
//! offending sites, so the guarantee is "zero anywhere", not merely "no new
//! ones". The source set is the `git ls-files`-tracked compiled-source files
//! (`.svelte`, `.ts`, and the `.js` family) under `frontend/src` (tracked-only
//! and deterministic, never descending into `node_modules` or build output).

use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The sole module permitted to hold a raw setInterval: the visibility-gated
/// polling helper that every other timer must route through.
const SETINTERVAL_HOME: &str = "frontend/src/lib/realtime/useVisiblePoll.ts";

/// The retired window-to-window tracking lifecycle event, superseded by the
/// typed `tracking:session:updated` topic. Must not reappear in the frontend.
const LEGACY_EVENT: &str = "tracking-state-changed";

/// Scanned source root: every tracked compiled-source file under the frontend
/// tree. Covers what Vite / SvelteKit bundle, so a poll cannot hide in a
/// .js-family module that a .svelte/.ts-only scan would miss.
const SCAN_ROOT: &str = "frontend/src";
const SCAN_SUFFIXES: &[&str] = &[".svelte", ".ts", ".js", ".mjs", ".cjs", ".jsx", ".tsx"];

/// A bare timer call. Tolerates whitespace before the paren (`setInterval (`),
/// which is valid JS that no formatter in the toolchain would normalise away;
/// the leading word boundary still matches a qualified `window.setInterval(`
/// form.
fn setinterval_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bsetInterval\s*\(").expect("valid setInterval pattern"))
}

/// A single lint violation: file, 1-based line number, rule, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: String, // "bare-setinterval" or "legacy-event"
    pub detail: String,
}

/// Repo-relative tracked compiled-source paths under `frontend/src`.
///
/// `git ls-files` is the enumeration, so the scan is tracked-only and
/// deterministic and never descends into `node_modules` or build artefacts.
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
    let is_home = posix == SETINTERVAL_HOME;
    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        if !is_home && setinterval_re().is_match(line) {
            findings.push(Finding {
                path: posix.clone(),
                lineno,
                rule: "bare-setinterval".to_string(),
                detail: format!(
                    "bare setInterval outside {SETINTERVAL_HOME}; route the poll \
through useVisiblePoll (or windowGeometryPoll)"
                ),
            });
        }
        if line.contains(LEGACY_EVENT) {
            findings.push(Finding {
                path: posix.clone(),
                lineno,
                rule: "legacy-event".to_string(),
                detail: format!(
                    "reference to the retired '{LEGACY_EVENT}' event; use the \
typed 'tracking:session:updated' topic instead"
                ),
            });
        }
    }
    findings
}

/// Scan the tracked frontend source and return every finding.
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
            // Any other read failure (permissions, I/O) must fail loudly: a
            // guard that silently skips an unreadable source can return a false
            // clean.
            Err(e) => return Err(format!("check-no-bare-setinterval: cannot read {path}: {e}")),
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
            "check-no-bare-setinterval: no bare setInterval or retired \
tracking-event references in the frontend source."
        );
        return Ok(0);
    }

    eprintln!(
        "check-no-bare-setinterval: frontend polling-discipline violations.\n\n\
Every timer-driven loop must route through {SETINTERVAL_HOME} \
(useVisiblePoll / windowGeometryPoll), and the retired '{LEGACY_EVENT}' event \
must not reappear. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!(
            "\ncheck-no-bare-setinterval: --warn-only set; exiting 0 despite the \
findings above."
        );
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_bare_setinterval_outside_home() {
        let f = scan_text("frontend/src/lib/foo.ts", "const id = setInterval(tick, 1000);");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "bare-setinterval");
        assert_eq!(f[0].lineno, 1);
    }

    #[test]
    fn allows_setinterval_in_the_sanctioned_home() {
        let f = scan_text(SETINTERVAL_HOME, "  const id = setInterval(poll, ms);");
        assert!(f.is_empty());
    }

    #[test]
    fn home_check_is_path_normalised() {
        // A backslash path must still be recognised as the home module.
        let win = SETINTERVAL_HOME.replace('/', "\\");
        let f = scan_text(&win, "setInterval(poll, ms);");
        assert!(f.is_empty());
    }

    #[test]
    fn tolerates_whitespace_and_qualified_form() {
        let f = scan_text("frontend/src/a.ts", "window.setInterval (fn, 100);");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "bare-setinterval");
    }

    #[test]
    fn does_not_flag_setinterval_substring() {
        // A word-boundary guard means `mySetInterval(` is not a match.
        let f = scan_text("frontend/src/a.ts", "mySetInterval(fn, 100);");
        assert!(f.is_empty());
    }

    #[test]
    fn flags_retired_event_anywhere() {
        let f = scan_text(
            "frontend/src/a.ts",
            "listen('tracking-state-changed', handler);",
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "legacy-event");
    }

    #[test]
    fn flags_retired_event_even_in_the_home_module() {
        // Rule B applies whole-tree, including the setInterval home.
        let f = scan_text(SETINTERVAL_HOME, "// tracking-state-changed is gone");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "legacy-event");
    }

    #[test]
    fn reports_correct_line_numbers() {
        let text = "line one\nsetInterval(fn, 1);\nline three";
        let f = scan_text("frontend/src/a.ts", text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 2);
    }
}
