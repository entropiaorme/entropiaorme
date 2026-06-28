//! Port of `backend/scripts/check_authoring_lint.py`.
//!
//! Two mechanical authoring rules, diff-scoped to the lines a change adds:
//!   - No em dashes (U+2014) in any added line of a non-exempt file.
//!   - UK spelling in added prose lines (Markdown / plain-text docs, and
//!     comment-only lines in code).
//!
//! Both rules inspect only added lines, never the whole tree: the tree carries
//! pre-existing US spellings and em dashes that predate the discipline, and a
//! lint floor only ratchets up.

use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The em dash, as an escape rather than the literal glyph so this guard's own
/// source does not trip the rule it enforces.
const EM_DASH: char = '\u{2014}';

/// Curated US -> UK spelling map (ported verbatim from the Python `US_TO_UK`).
///
/// Deliberately conservative: it omits words that double as code, CSS, or config
/// tokens (`color` / `center` / `license` and the `-ize` identifier verbs such
/// as `serialize` / `initialize`).
const US_TO_UK: &[(&str, &str)] = &[
    ("behavior", "behaviour"),
    ("behaviors", "behaviours"),
    ("behavioral", "behavioural"),
    ("customize", "customise"),
    ("customizes", "customises"),
    ("customized", "customised"),
    ("customizing", "customising"),
    ("customization", "customisation"),
    ("customizations", "customisations"),
    ("organize", "organise"),
    ("organizes", "organises"),
    ("organized", "organised"),
    ("organizing", "organising"),
    ("organization", "organisation"),
    ("organizations", "organisations"),
    ("analyze", "analyse"),
    ("analyzes", "analyses"),
    ("analyzed", "analysed"),
    ("analyzing", "analysing"),
    ("optimize", "optimise"),
    ("optimizes", "optimises"),
    ("optimized", "optimised"),
    ("optimizing", "optimising"),
    ("optimization", "optimisation"),
    ("optimizations", "optimisations"),
    ("recognize", "recognise"),
    ("recognized", "recognised"),
    ("recognizes", "recognises"),
    ("recognizing", "recognising"),
    ("summarize", "summarise"),
    ("summarized", "summarised"),
    ("summarizes", "summarises"),
    ("summarizing", "summarising"),
    ("minimize", "minimise"),
    ("minimized", "minimised"),
    ("minimizes", "minimises"),
    ("minimizing", "minimising"),
    ("maximize", "maximise"),
    ("maximized", "maximised"),
    ("maximizes", "maximises"),
    ("maximizing", "maximising"),
    ("prioritize", "prioritise"),
    ("prioritized", "prioritised"),
    ("prioritizes", "prioritises"),
    ("prioritizing", "prioritising"),
    ("emphasize", "emphasise"),
    ("emphasized", "emphasised"),
    ("emphasizes", "emphasises"),
    ("emphasizing", "emphasising"),
    ("categorize", "categorise"),
    ("categorized", "categorised"),
    ("categorizes", "categorises"),
    ("categorizing", "categorising"),
    ("capitalize", "capitalise"),
    ("capitalized", "capitalised"),
    ("capitalizes", "capitalises"),
    ("capitalizing", "capitalising"),
    ("catalog", "catalogue"),
    ("catalogs", "catalogues"),
    ("favor", "favour"),
    ("favors", "favours"),
    ("favored", "favoured"),
    ("favorite", "favourite"),
    ("favorites", "favourites"),
    ("honor", "honour"),
    ("honors", "honours"),
    ("honored", "honoured"),
    ("defense", "defence"),
    ("offense", "offence"),
    ("fulfill", "fulfil"),
    ("fulfills", "fulfils"),
    ("canceled", "cancelled"),
    ("canceling", "cancelling"),
    ("labeled", "labelled"),
    ("labeling", "labelling"),
    ("modeled", "modelled"),
    ("modeling", "modelling"),
    ("traveled", "travelled"),
    ("traveling", "travelling"),
];

/// Prose file suffixes whose every line is authored prose for UK spelling.
const PROSE_SUFFIXES: &[&str] = &[".md", ".markdown", ".txt", ".rst"];

/// Leading tokens marking an added line as a comment (comment-only heuristic).
const COMMENT_PREFIXES: &[&str] = &["#", "//", "/*", "*", "<!--", "--", ";"];

/// Repository-relative path patterns exempt from BOTH rules (ported verbatim).
fn exempt_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)(^|/)LICENSE(\.[^/]*)?$",
            r"(?i)(^|/)THIRD-PARTY-NOTICES(\.[^/]*)?$",
            r"(^|/)node_modules/",
            r"(^|/)package-lock\.json$",
            r"(^|/)Cargo\.lock$",
            r"(?i)\.db$",
            r"(^|/)backend/assets/models/",
            r"(^|/)backend/tests/expected/openapi\.snapshot\.json$",
            r"(^|/)backend/testing/COVERAGE\.md$",
            r"(^|/)frontend/src/lib/api/schema\.d\.ts$",
            r"(^|/)frontend/src-tauri/eo-http/resources/demo_goldens/",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("valid exempt pattern"))
        .collect()
    })
}

/// One alternation over the US spellings, word-bounded and case-insensitive.
///
/// Sorted by length descending so the longest form matches first, exactly as the
/// Python builds its alternation.
fn us_spelling_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let mut words: Vec<&str> = US_TO_UK.iter().map(|(us, _)| *us).collect();
        words.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        let alternation = words.join("|");
        Regex::new(&format!(r"(?i)\b({alternation})\b")).expect("valid spelling alternation")
    })
}

fn us_to_uk(word: &str) -> Option<&'static str> {
    let lower = word.to_lowercase();
    US_TO_UK
        .iter()
        .find(|(us, _)| *us == lower)
        .map(|(_, uk)| *uk)
}

/// True when a repo-relative path is exempt from both authoring rules.
pub fn is_exempt(path: &str) -> bool {
    let posix = path.replace('\\', "/");
    exempt_patterns().iter().any(|p| p.is_match(&posix))
}

/// True when an added line should be checked for UK spelling.
///
/// Prose files (Markdown, plain text) are prose throughout; in code, only a
/// comment-only line counts.
pub fn is_prose_context(path: &str, line: &str) -> bool {
    let posix = path.replace('\\', "/").to_lowercase();
    if PROSE_SUFFIXES.iter().any(|s| posix.ends_with(s)) {
        return true;
    }
    let stripped = line.trim_start();
    COMMENT_PREFIXES.iter().any(|p| stripped.starts_with(p))
}

/// Blank out Markdown inline-code spans so only prose is spell-checked.
///
/// Faithful to the Python `_INLINE_CODE_RE` (a run of backticks closed by an
/// equal-or-longer run, with non-empty content) for the realistic single- and
/// multi-backtick spans in the repo's prose. The regex crate has no
/// backreferences, so the matched-run semantics are implemented by hand here.
fn strip_inline_code(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Maximal opening run of backticks, length n.
        let mut n = 0;
        while i + n < chars.len() && chars[i + n] == '`' {
            n += 1;
        }
        // Look for a closing run of length >= n that leaves >= 1 content char.
        let mut j = i + n;
        let mut close: Option<usize> = None;
        while j < chars.len() {
            if chars[j] == '`' {
                let mut m = 0;
                while j + m < chars.len() && chars[j + m] == '`' {
                    m += 1;
                }
                if m >= n && j > i + n {
                    close = Some(j);
                    break;
                }
                j += m;
            } else {
                j += 1;
            }
        }
        match close {
            Some(c) => {
                out.push(' ');
                i = c + n; // consume exactly n of the closing run
            }
            None => {
                out.push('`');
                i += 1;
            }
        }
    }
    out
}

/// One line a diff adds: its file, new-file line number, and text.
#[derive(Debug, Clone)]
pub struct AddedLine {
    pub path: String,
    pub lineno: usize,
    pub text: String,
}

/// A single authoring-rule violation on an added line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: String, // "em-dash" or "uk-spelling"
    pub detail: String,
}

fn hunk_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").expect("valid hunk pattern")
    })
}

/// Parse the added lines (with new-file line numbers) out of a `-U0` diff.
fn parse_added_lines(diff: &str) -> Vec<AddedLine> {
    let mut out: Vec<AddedLine> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut new_lineno: usize = 0;
    for raw in diff.split('\n') {
        if let Some(target) = raw.strip_prefix("+++ ") {
            let target = target.trim();
            // "+++ b/path" for a tracked change; "+++ /dev/null" for a deletion.
            current_path = target.strip_prefix("b/").map(|p| p.to_string());
            continue;
        }
        if raw.starts_with("@@") {
            if let Some(caps) = hunk_re().captures(raw) {
                new_lineno = caps[1].parse().unwrap_or(new_lineno);
            }
            continue;
        }
        if raw.starts_with('+') && !raw.starts_with("+++") {
            if let Some(path) = &current_path {
                out.push(AddedLine {
                    path: path.clone(),
                    lineno: new_lineno,
                    text: raw[1..].to_string(),
                });
            }
            new_lineno += 1;
        }
    }
    out
}

/// Apply both authoring rules to a list of added lines.
pub fn scan(lines: &[AddedLine]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for line in lines {
        if is_exempt(&line.path) {
            continue;
        }
        if line.text.contains(EM_DASH) {
            findings.push(Finding {
                path: line.path.clone(),
                lineno: line.lineno,
                rule: "em-dash".to_string(),
                detail: "em dash (U+2014); use a colon, semicolon, parentheses, or a comma instead"
                    .to_string(),
            });
        }
        if is_prose_context(&line.path, &line.text) {
            let prose = strip_inline_code(&line.text);
            for caps in us_spelling_re().captures_iter(&prose) {
                let us = &caps[1];
                if let Some(uk) = us_to_uk(us) {
                    findings.push(Finding {
                        path: line.path.clone(),
                        lineno: line.lineno,
                        rule: "uk-spelling".to_string(),
                        detail: format!("US spelling '{us}'; use UK spelling '{uk}'"),
                    });
                }
            }
        }
    }
    findings
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let commit_range = crate::flag_value(args, "--range")?;
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;

    let range = commit_range.as_deref().unwrap_or("HEAD");
    let diff = git::run(&["diff", "-U0", "--no-color", range], &repo_root)?;
    let findings = scan(&parse_added_lines(&diff));

    if findings.is_empty() {
        println!("check-authoring-lint: no em-dash or UK-spelling issues in the added lines.");
        return Ok(0);
    }

    eprintln!(
        "check-authoring-lint: authoring-discipline issues on newly added \
lines.\n\n\
These rules apply only to lines this change adds; pre-existing content \
is out of scope. Fix the flagged lines (or, for the em-dash rule, \
reword with a colon / semicolon / parentheses / comma).\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!(
            "\ncheck-authoring-lint: --warn-only set; exiting 0 despite the \
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
    fn flags_em_dash_in_any_added_line() {
        let lines = vec![AddedLine {
            path: "src/foo.rs".to_string(),
            lineno: 10,
            text: format!("let x = 1; {} a comment", EM_DASH),
        }];
        let f = scan(&lines);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "em-dash");
    }

    #[test]
    fn us_spelling_only_in_prose_context() {
        // A US spelling in a code line (not a comment) is not flagged.
        let code = vec![AddedLine {
            path: "src/foo.rs".to_string(),
            lineno: 1,
            text: "    let color = optimize();".to_string(),
        }];
        assert!(scan(&code).is_empty());

        // The same word in a comment-only line is flagged.
        let comment = vec![AddedLine {
            path: "src/foo.rs".to_string(),
            lineno: 1,
            text: "    // we optimize the hot path".to_string(),
        }];
        let f = scan(&comment);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "uk-spelling");
        assert!(f[0].detail.contains("optimise"));
    }

    #[test]
    fn markdown_inline_code_is_not_spell_checked() {
        let lines = vec![AddedLine {
            path: "docs/x.md".to_string(),
            lineno: 1,
            text: "The `behavior` DOM property is fine to cite here.".to_string(),
        }];
        assert!(scan(&lines).is_empty());

        // But prose outside the code span is still checked.
        let lines = vec![AddedLine {
            path: "docs/x.md".to_string(),
            lineno: 1,
            text: "We optimize the `behavior` property.".to_string(),
        }];
        let f = scan(&lines);
        assert_eq!(f.len(), 1);
        assert!(f[0].detail.contains("optimise"));
    }

    #[test]
    fn exempt_paths_skip_both_rules() {
        let lines = vec![AddedLine {
            path: "frontend/package-lock.json".to_string(),
            lineno: 1,
            text: format!("optimize {}", EM_DASH),
        }];
        assert!(scan(&lines).is_empty());
    }

    #[test]
    fn parses_added_lines_with_line_numbers() {
        let diff = "diff --git a/x.md b/x.md\n\
--- a/x.md\n\
+++ b/x.md\n\
@@ -1,0 +5,2 @@\n\
+first added\n\
+second added\n";
        let added = parse_added_lines(diff);
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].lineno, 5);
        assert_eq!(added[0].text, "first added");
        assert_eq!(added[1].lineno, 6);
        assert_eq!(added[1].path, "x.md");
    }

    #[test]
    fn deletions_to_dev_null_are_ignored() {
        let diff = "--- a/gone.md\n\
+++ /dev/null\n\
@@ -1,1 +0,0 @@\n";
        assert!(parse_added_lines(diff).is_empty());
    }

    #[test]
    fn strip_inline_code_handles_double_backticks() {
        // ``a `tick` b`` is one span; its contents are blanked.
        let s = strip_inline_code("x ``a `tick` b`` y");
        assert!(!s.contains("tick"));
        assert!(s.contains('x') && s.contains('y'));
    }
}
