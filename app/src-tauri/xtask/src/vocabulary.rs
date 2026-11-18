//! The reference and vocabulary dimension of the authoring lint.
//!
//! Three rules over what a change adds to the repository's permanent record:
//! the commit messages in the range, and the prose lines the diff adds
//! (Markdown and plain-text files, comment-only lines in code).
//!
//!   - No references to files the repository does not have. A path-like token
//!     must resolve against the tree at the head of the range: exactly, as a
//!     suffix at a path-component boundary, relative to the file that cites it,
//!     as a path the range itself deletes or renames away, or as a path the
//!     repository declares ignored (a path it knows about, tracked or not).
//!   - No iteration tokens (an upper-case R plus one or two digits, "round 3"):
//!     a message describes the change, not its place in a sequence of attempts.
//!   - No tool-attribution lines ("Generated with [tool](url)"): authorship is
//!     the commit's author and its `Co-Authored-By` trailers.
//!
//! Diff-scoped like the sibling rules in `authoring.rs`: only what the change
//! adds is inspected, so pre-existing content is out of scope by construction.
//! Git trailers (`Co-Authored-By`, `Fixes #N`, and the like) are exempt.
//!
//! The token grammar is deliberately narrow, because prose is full of slashes
//! that are not paths (`start/stop`, `A/B`, action slugs): a Markdown file name
//! with optional directories, a dot-directory-rooted path, and, in commit
//! messages only, a slash path whose last component carries a source or config
//! extension. Prose in a decision record legitimately cites files a later
//! decision removed, so diff text gets only the first two shapes.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use regex::Regex;

use crate::authoring::{is_exempt, is_prose_context, AddedLine};
use crate::git;

/// Exact tokens allowed to name a path the repository does not have (a
/// generated artefact cited in prose, say). Starts empty; extended as real
/// cases appear, never pre-emptively.
const ALLOWED_REFERENCES: &[&str] = &[];

/// A single finding: where it is, which rule, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub location: String,
    pub rule: String, // "absent-path", "iteration-token", or "attribution-line"
    pub detail: String,
    /// For `absent-path`: the normalised candidates the token could denote, so
    /// the caller can drop the finding when the repository declares one ignored.
    pub candidates: Vec<String>,
}

/// One commit in the range: its abbreviated hash and full message.
#[derive(Debug, Clone)]
pub struct Commit {
    pub short_sha: String,
    pub message: String,
}

/// The paths the repository knows at the head of a range.
#[derive(Debug, Default)]
pub struct KnownPaths {
    tracked: HashSet<String>,
    directories: HashSet<String>,
    removed: HashSet<String>,
}

impl KnownPaths {
    /// Build from the tracked files at the head plus the paths the range
    /// removes or renames away (a commit deleting a file names it legitimately).
    pub fn new(
        tracked: impl IntoIterator<Item = String>,
        removed: impl IntoIterator<Item = String>,
    ) -> Self {
        let tracked: HashSet<String> = tracked.into_iter().collect();
        let mut directories = HashSet::new();
        for path in &tracked {
            let parts: Vec<&str> = path.split('/').collect();
            for i in 1..parts.len() {
                directories.insert(parts[..i].join("/"));
            }
        }
        Self {
            tracked,
            directories,
            removed: removed.into_iter().collect(),
        }
    }

    /// The normalised candidate paths a token could denote: as written, and
    /// relative to the citing file's directory when there is one.
    fn candidates(token: &str, context_dir: Option<&str>) -> Vec<String> {
        let path = normalise_token(token);
        let mut out = vec![path.clone()];
        if let Some(dir) = context_dir {
            let joined = join_normalised(dir, &path);
            if joined != path {
                out.push(joined);
            }
        }
        out
    }

    /// True when the token resolves against the known paths.
    fn resolves(&self, candidates: &[String]) -> bool {
        for c in candidates {
            if ALLOWED_REFERENCES.contains(&c.as_str())
                || self.tracked.contains(c)
                || self.directories.contains(c)
                || self.removed.contains(c)
            {
                return true;
            }
        }
        // A repo-relative suffix at a component boundary (`eo-api/src/x.rs` for
        // `app/src-tauri/eo-api/src/x.rs`).
        let suffix = format!("/{}", candidates[0]);
        self.tracked.iter().any(|p| p.ends_with(&suffix))
            || self.directories.iter().any(|p| p.ends_with(&suffix))
            || self.removed.iter().any(|p| p.ends_with(&suffix))
    }
}

/// Strip a trailing slash and a leading `./`.
fn normalise_token(token: &str) -> String {
    let t = token.trim_end_matches('/');
    t.strip_prefix("./").unwrap_or(t).to_string()
}

/// Join `dir` and `path` and collapse `.` / `..` components.
fn join_normalised(dir: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in dir.split('/').chain(path.split('/')) {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn markdown_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:\./|\.\./)*(?:\.?[\w-]+/)*[\w-]+\.md$")
            .expect("valid markdown path pattern")
    })
}

fn dot_directory_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\.[\w-]+/[\w./-]*$").expect("valid dot-directory pattern"))
}

fn source_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:\./|\.\./)*[\w-]+/(?:[\w.-]+/)*[\w.-]+\.(?:rs|ts|js|mjs|cjs|svelte|py|sh|ps1|yml|yaml|json|toml|txt|sql|css|html)$",
        )
        .expect("valid source path pattern")
    })
}

fn link_target_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\]\(([^)\s]+)\)").expect("valid link target pattern"))
}

fn line_reference_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r":\d+(?:-\d+)?$").expect("valid line reference pattern"))
}

/// Iteration tokens: an upper-case R and one or two digits. Case-sensitive on
/// purpose (`r0` is an identifier) and capped at two digits (`R100` is git's
/// rename score).
fn iteration_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bR\d{1,2}\b").expect("valid iteration token pattern"))
}

/// "round 3" in a commit message. Not applied to diff text, where the phrase
/// means numeric rounding.
fn iteration_phrase_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bround[ -]\d+\b").expect("valid iteration phrase pattern"))
}

/// A line that opens with a capitalised attribution verb and names a tool by
/// link, URL, or product name.
fn attribution_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*[^\w\s]*\s*(?:Generated|Authored|Written|Created)\s+(?:with|by)\b.*(?:https?://|\]\(|\bClaude\b|\bChatGPT\b|\bCopilot\b|\bCodex\b|\bCursor\b|\bGemini\b)",
        )
        .expect("valid attribution pattern")
    })
}

/// Git trailer lines and issue references, exempt from every rule.
fn trailer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:co-authored-by|signed-off-by|reviewed-by|acked-by|tested-by|reported-by|suggested-by|helped-by|cc|refs?|see-also|fixes|closes|resolves)(?::\s|\s+#\d)",
        )
        .expect("valid trailer pattern")
    })
}

/// Strip the punctuation prose wraps a token in, and a trailing `:line` reference.
fn trim_token(piece: &str) -> &str {
    let leading: &[char] = &['`', '\'', '"', '(', '[', '{', '<', '*', '_'];
    let trailing: &[char] = &[
        '`', '\'', '"', ')', ']', '}', '>', '.', ',', ';', ':', '!', '?', '*', '_',
    ];
    let t = piece.trim_start_matches(leading).trim_end_matches(trailing);
    let t = match line_reference_re().find(t) {
        Some(m) => &t[..m.start()],
        None => t,
    };
    t.trim_end_matches(trailing)
}

/// The path-shaped tokens in a line: whitespace-separated pieces stripped of
/// surrounding punctuation, plus Markdown link targets. URLs are skipped.
fn path_tokens(line: &str, allow_source_paths: bool) -> Vec<String> {
    let mut pieces: Vec<&str> = line.split_whitespace().collect();
    for caps in link_target_re().captures_iter(line) {
        pieces.push(caps.get(1).expect("link target group").as_str());
    }
    let mut out = Vec::new();
    for piece in pieces {
        if piece.contains("://") {
            continue;
        }
        let token = trim_token(piece);
        if token.is_empty() {
            continue;
        }
        let is_path = markdown_path_re().is_match(token)
            || dot_directory_re().is_match(token)
            || (allow_source_paths && source_path_re().is_match(token));
        if is_path && !out.iter().any(|t| t == token) {
            out.push(token.to_string());
        }
    }
    out
}

fn absent_path_finding(location: String, token: &str, candidates: Vec<String>) -> Finding {
    Finding {
        location,
        rule: "absent-path".to_string(),
        detail: format!("'{token}' names a file or directory the repository does not have"),
        candidates,
    }
}

/// Apply the rules to the commit messages in the range.
pub fn scan_messages(commits: &[Commit], known: &KnownPaths) -> Vec<Finding> {
    let mut findings = Vec::new();
    for commit in commits {
        for (idx, line) in commit.message.lines().enumerate() {
            let text = line.trim();
            if text.is_empty() || trailer_re().is_match(text) {
                continue;
            }
            let location = format!("commit {} line {}", commit.short_sha, idx + 1);
            for token in path_tokens(text, true) {
                let candidates = KnownPaths::candidates(&token, None);
                if !known.resolves(&candidates) {
                    findings.push(absent_path_finding(location.clone(), &token, candidates));
                }
            }
            if let Some(m) = iteration_token_re().find(text) {
                findings.push(Finding {
                    location: location.clone(),
                    rule: "iteration-token".to_string(),
                    detail: format!(
                        "iteration token '{}'; describe the change, not its attempt number",
                        m.as_str()
                    ),
                    candidates: Vec::new(),
                });
            } else if let Some(m) = iteration_phrase_re().find(text) {
                findings.push(Finding {
                    location: location.clone(),
                    rule: "iteration-token".to_string(),
                    detail: format!(
                        "iteration phrase '{}'; describe the change, not its attempt number",
                        m.as_str()
                    ),
                    candidates: Vec::new(),
                });
            }
            if attribution_re().is_match(text) {
                findings.push(Finding {
                    location,
                    rule: "attribution-line".to_string(),
                    detail: "tool-attribution line; authorship is the author and the Co-Authored-By trailers".to_string(),
                    candidates: Vec::new(),
                });
            }
        }
    }
    findings
}

/// Apply the rules to the prose lines a diff adds.
pub fn scan_diff(lines: &[AddedLine], known: &KnownPaths) -> Vec<Finding> {
    let mut findings = Vec::new();
    for line in lines {
        if is_exempt(&line.path) || !is_prose_context(&line.path, &line.text) {
            continue;
        }
        let location = format!("{}:{}", line.path, line.lineno);
        let context_dir = line.path.rsplit_once('/').map(|(dir, _)| dir);
        for token in path_tokens(&line.text, false) {
            let candidates = KnownPaths::candidates(&token, context_dir);
            if !known.resolves(&candidates) {
                findings.push(absent_path_finding(location.clone(), &token, candidates));
            }
        }
        if let Some(m) = iteration_token_re().find(&line.text) {
            findings.push(Finding {
                location: location.clone(),
                rule: "iteration-token".to_string(),
                detail: format!(
                    "iteration token '{}'; describe the change, not its attempt number",
                    m.as_str()
                ),
                candidates: Vec::new(),
            });
        }
        if attribution_re().is_match(&line.text) {
            findings.push(Finding {
                location,
                rule: "attribution-line".to_string(),
                detail: "tool-attribution line; authorship is the author and the Co-Authored-By trailers".to_string(),
                candidates: Vec::new(),
            });
        }
    }
    findings
}

/// Drop `absent-path` findings whose token the repository declares ignored.
pub fn drop_ignored(findings: Vec<Finding>, ignored: &HashSet<String>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| f.rule != "absent-path" || !f.candidates.iter().any(|c| ignored.contains(c)))
        .collect()
}

/// The subset of `paths` the repository's ignore rules match.
///
/// `git check-ignore` exits 1 when nothing matched, which is a clean answer
/// rather than a failure, so this does not go through the shared helper. The
/// candidates are fed from a separate thread while the output is drained, so
/// a large batch cannot deadlock on a full pipe in either direction.
pub fn ignored_paths(paths: &[String], repo_root: &Path) -> Result<HashSet<String>, String> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut child = Command::new("git")
        .args(["check-ignore", "--no-index", "--stdin"])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git check-ignore: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let batch: Vec<String> = paths.to_vec();
    let feeder = std::thread::spawn(move || -> Result<(), String> {
        for path in batch {
            writeln!(stdin, "{path}")
                .map_err(|e| format!("failed to feed git check-ignore: {e}"))?;
        }
        Ok(())
    });
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for git check-ignore: {e}"))?;
    feeder
        .join()
        .map_err(|_| "the git check-ignore feeder thread panicked".to_string())??;
    match output.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.to_string())
            .collect()),
        _ => Err(format!(
            "git check-ignore exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// The commits in `range`, oldest first.
pub fn commits_in_range(range: &str, repo_root: &Path) -> Result<Vec<Commit>, String> {
    let out = git::run(
        &["log", "--reverse", "--format=%h%x00%B%x1e", range],
        repo_root,
    )?;
    Ok(out
        .split('\u{1e}')
        .filter_map(|entry| {
            let entry = entry.trim_start_matches('\n');
            let (sha, message) = entry.split_once('\0')?;
            Some(Commit {
                short_sha: sha.trim().to_string(),
                message: message.to_string(),
            })
        })
        .collect())
}

/// The known paths for a range (`base..head`), or for the index when there is
/// no range (the local, pre-commit shape).
pub fn known_paths(range: Option<&str>, repo_root: &Path) -> Result<KnownPaths, String> {
    let (tracked, removed) = match range {
        Some(range) => {
            let head = range
                .rsplit_once("..")
                .map(|(_, h)| h.trim_start_matches('.'))
                .unwrap_or(range);
            let tracked = git::run(&["ls-tree", "-r", "--name-only", head], repo_root)?;
            let removed = git::run(
                &["diff", "--name-status", "-M", "--diff-filter=DR", range],
                repo_root,
            )?;
            (tracked, removed)
        }
        None => {
            let tracked = git::run(&["ls-files"], repo_root)?;
            let removed = git::run(
                &["diff", "--name-status", "-M", "--diff-filter=DR", "HEAD"],
                repo_root,
            )?;
            (tracked, removed)
        }
    };
    let removed_paths = removed.lines().filter_map(|line| {
        // "D\tpath" or "R<score>\told\tnew": the path that stops existing.
        let mut fields = line.split('\t');
        let status = fields.next()?;
        let path = fields.next()?;
        (status.starts_with('D') || status.starts_with('R')).then(|| path.to_string())
    });
    Ok(KnownPaths::new(
        tracked.lines().map(|l| l.to_string()),
        removed_paths,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> KnownPaths {
        KnownPaths::new(
            [
                "README.md",
                "docs/src/adr/index.md",
                "docs/src/adr/0001-first.md",
                "app/src-tauri/eo-api/src/settings.rs",
                ".github/workflows/ci.yml",
            ]
            .map(String::from),
            ["app/src-tauri/eo-http/src/lib.rs".to_string()],
        )
    }

    fn commit(message: &str) -> Vec<Commit> {
        vec![Commit {
            short_sha: "abc1234".to_string(),
            message: message.to_string(),
        }]
    }

    fn rules(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    #[test]
    fn message_referencing_an_absent_file_fails() {
        let f = scan_messages(&commit("docs: see notes/plan.md for the plan"), &known());
        assert_eq!(rules(&f), ["absent-path"]);
        assert!(f[0].detail.contains("notes/plan.md"));
        assert_eq!(f[0].location, "commit abc1234 line 1");
    }

    #[test]
    fn clean_message_passes() {
        let f = scan_messages(
            &commit("ci: adopt the next branch\n\nUpdates README.md and .github/workflows/ci.yml.\n\nCo-Authored-By: Someone <s@example.com>"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn suffix_and_removed_paths_resolve() {
        let f = scan_messages(
            &commit("refactor: move eo-api/src/settings.rs and delete eo-http/src/lib.rs"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn prose_slashes_are_not_paths() {
        let f = scan_messages(
            &commit("feat: start/stop the A/B toggle via actions/checkout and svelte/store"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn trailers_and_urls_are_exempt() {
        let f = scan_messages(
            &commit("fix: thing\n\nFixes #12\nRefs: notes/plan.md\nSee https://example.com/notes/plan.md"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn iteration_tokens_are_flagged_but_not_lookalikes() {
        let f = scan_messages(
            &commit("fix: R7 of the fix\n\nround 3 of the same."),
            &known(),
        );
        assert_eq!(rules(&f), ["iteration-token", "iteration-token"]);
        let f = scan_messages(
            &commit("test: rename detection at R100 keeps r0 and r1"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn attribution_lines_are_flagged_but_co_authorship_is_not() {
        let f = scan_messages(
            &commit("feat: x\n\n🤖 Generated with [Tool](https://example.com)\n\nCo-Authored-By: Claude <noreply@example.com>"),
            &known(),
        );
        assert_eq!(rules(&f), ["attribution-line"]);
        let f = scan_messages(
            &commit("build: bindings\n\nGenerated by cargo xtask gen-ts."),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    fn added(path: &str, text: &str) -> AddedLine {
        AddedLine {
            path: path.to_string(),
            lineno: 3,
            text: text.to_string(),
        }
    }

    #[test]
    fn diff_prose_resolves_relative_links_and_flags_absent_dot_directories() {
        let lines = [
            added(
                "docs/src/adr/0002-second.md",
                "See [the first](0001-first.md) and [the index](index.md).",
            ),
            added(
                "app/src-tauri/eo-api/src/lib.rs",
                "// The plan lives in .internal/notes/x.md and HANDBOOK.md",
            ),
            added(
                "app/src-tauri/eo-api/src/lib.rs",
                "let p = \".internal/notes/x.md\"; // code line, not prose",
            ),
        ];
        let f = scan_diff(&lines, &known());
        assert_eq!(rules(&f), ["absent-path", "absent-path"]);
        assert_eq!(f[0].location, "app/src-tauri/eo-api/src/lib.rs:3");
        assert!(f[0].detail.contains(".internal/notes/x.md"));
        assert!(f[1].detail.contains("HANDBOOK.md"));
    }

    #[test]
    fn diff_prose_ignores_source_paths_and_rounding_phrases() {
        let lines = [added(
            "docs/src/adr/0015-old.md",
            "The retired backend/scripts/convert.py did this; round 4 -> 5.",
        )];
        assert!(scan_diff(&lines, &known()).is_empty());
    }

    #[test]
    fn diff_prose_flags_iteration_tokens_and_attribution() {
        let lines = [
            added("README.md", "R2 of the docs."),
            added("README.md", "Generated with [Tool](https://example.com)"),
        ];
        assert_eq!(
            rules(&scan_diff(&lines, &known())),
            ["iteration-token", "attribution-line"]
        );
    }

    #[test]
    fn ignored_candidates_drop_the_finding() {
        let f = scan_diff(
            &[added(
                "README.md",
                "Local data lives under .data/ for the run.",
            )],
            &known(),
        );
        assert_eq!(rules(&f), ["absent-path"]);
        let kept = drop_ignored(f.clone(), &HashSet::new());
        assert_eq!(kept.len(), 1);
        let dropped = drop_ignored(f, &HashSet::from([".data".to_string()]));
        assert!(dropped.is_empty());
    }

    #[test]
    fn tokens_are_trimmed_of_punctuation_and_line_references() {
        let f = scan_messages(
            &commit("docs: (see `README.md:12`), and \"docs/src/adr/index.md\"."),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
        let f = scan_messages(&commit("docs: (see `notes/plan.md:12-14`)."), &known());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].candidates, ["notes/plan.md"]);
    }

    #[test]
    fn dot_slash_prefixed_paths_are_scanned() {
        let f = scan_messages(
            &commit("docs: see ./notes/plan.md and ./README.md"),
            &known(),
        );
        assert_eq!(rules(&f), ["absent-path"]);
        assert_eq!(f[0].candidates, ["notes/plan.md"]);
        let f = scan_messages(
            &commit("refactor: touch ./app/src-tauri/eo-api/src/settings.rs"),
            &known(),
        );
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn join_normalised_collapses_parent_components() {
        assert_eq!(
            join_normalised("docs/src/adr", "../index.md"),
            "docs/src/index.md"
        );
        assert_eq!(join_normalised("docs", "./a/b.md"), "docs/a/b.md");
        assert_eq!(join_normalised("", "x.md"), "x.md");
    }
}
