//! Two whole-tree rules keeping the in-development register honest:
//!
//!   - Rule (no orphan on either side): every `id` declared in
//!     `IN_DEVELOPMENT_SURFACES` has at least one consumer, and every id a
//!     consumer references is declared. A marker with no entry ships an
//!     unexplained badge; an entry with no consumer is a surface that has
//!     since been finished (or removed) while still advertising itself as
//!     unfinished.
//!
//!   - Rule (the published channel is stamped): every step in the release
//!     workflow that builds the app (its frontend, its Rust binary, or both)
//!     sets `ENTROPIAORME_STABLE_CHANNEL` to `1` in its own `env` block. That
//!     stamp is what hides in-development surfaces from people who did not
//!     build the app, and what keeps the backend's in-development features
//!     off in a published build. Losing it, or setting it to anything else,
//!     is silent in CI and visible only to whoever downloads the release, so
//!     it gets a mechanical check.
//!
//!   - Rule (the backend reads the same stamp): the settings module, where
//!     the backend decides whether an in-development feature may run, reads
//!     the stamp by this guard's own name at compile time. A renamed or
//!     removed read would leave a feature on in a published build.
//!
//! Why guards rather than a written convention: a surface lands ahead of its
//! capability precisely when attention is elsewhere, and graduating it is the
//! step most easily forgotten. Without a mechanical check the register decays
//! into scattered dead entries over a few release cycles, which is why the
//! convention ships with its own check.
//!
//! Whole-tree rather than diff-scoped, like the sibling frontend lints: the
//! guarantee is "no orphan anywhere", not "this diff added none".
//!
//! Both rules scan text rather than parsing (xtask keeps its dependency set
//! minimal, and the workspace vendors no YAML parser). The scanning is
//! deliberately structural rather than line-local: comments are stripped
//! before matching so prose cannot register as a consumer or as a stamp, the
//! consumer patterns span lines so multi-line markup is seen, and the channel
//! rule attributes a stamp to the YAML step that contains it so a neighbouring
//! step's stamp cannot vouch for an unstamped build. The residual gap is a
//! marker spelled inside a string literal, which would read as a consumer;
//! that is contrived enough to accept, and it fails safe (it can only keep a
//! register entry alive, never hide a real one).

use regex::Regex;

use crate::git;

const REGISTRY: &str = "app/src/lib/inDevelopment/registry.ts";
const RELEASE_WORKFLOW: &str = ".github/workflows/release.yml";
/// The release workflow's build invocations: each builds the frontend
/// assets and the Rust binary of a published artefact.
const RELEASE_BUILD_MARKERS: &[&str] = &["tauri -- build", "build-installer.ps1"];
/// Invocations that would build the Rust binary alone, should a release
/// step ever do so.
const RUST_BUILD_MARKERS: &[&str] = &["cargo build", "cargo tauri"];
/// Where the backend reads the stamp.
const RUST_CHANNEL_READER: &str = "app/src-tauri/eo-services/src/config_service.rs";
const CHANNEL_STAMP: &str = "ENTROPIAORME_STABLE_CHANNEL";

/// A single lint violation: file, 1-based line number, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub detail: String,
}

/// Blank out comment spans, preserving byte offsets and newlines so a match
/// offset still maps to the original line. Both comment forms are supplied by
/// the caller, because they are language-specific: applying C-style block
/// stripping to YAML would treat a glob such as `dist/*` as a comment opener
/// and blank the rest of the file. `line_openers` are honoured only where they
/// open a line, so a URL or a colour literal mid-line is untouched.
fn scrub(text: &str, block_pairs: &[(&str, &str)], line_openers: &[&str]) -> String {
    let mut out: Vec<u8> = text.as_bytes().to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };

    for &(open, close) in block_pairs {
        let mut from = 0;
        while let Some(rel) = text[from..].find(open) {
            let start = from + rel;
            let end = match text[start + open.len()..].find(close) {
                Some(r) => start + open.len() + r + close.len(),
                None => text.len(),
            };
            blank(&mut out, start, end);
            from = end;
            if from >= text.len() {
                break;
            }
        }
    }

    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        let body = line.trim_start();
        if line_openers.iter().any(|o| body.starts_with(o)) {
            blank(&mut out, offset + indent, offset + line.trim_end().len());
        }
        offset += line.len();
    }

    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Comment-scrubbed TypeScript / Svelte source.
pub fn strip_comments(text: &str) -> String {
    scrub(text, &[("/*", "*/"), ("<!--", "-->")], &["//", "*"])
}

/// Comment-scrubbed YAML. Only `#`, and only where it opens a line: YAML has
/// no block-comment form, and a `#` inside a `run:` command or a colour
/// literal is not a comment.
pub fn strip_yaml_comments(text: &str) -> String {
    scrub(text, &[], &["#"])
}

/// 1-based line number of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Ids declared in the register. Reads the `id` field of each entry literal;
/// the register is a flat `as const` array by design.
pub fn declared_ids(text: &str) -> Vec<(usize, String)> {
    let scrubbed = strip_comments(text);
    let re = Regex::new(r#"(?m)^[ \t]*id[ \t]*:[ \t]*['"]([^'"]+)['"]"#).expect("static regex");
    re.captures_iter(&scrubbed)
        .map(|c| {
            let m = c.get(0).expect("group 0");
            (line_of(&scrubbed, m.start()), c[1].to_string())
        })
        .collect()
}

/// Ids referenced by a consumer: the marker component in markup (its `id`
/// attribute may sit on a later line) and the lookup helper in logic.
pub fn referenced_ids(text: &str) -> Vec<(usize, String)> {
    let scrubbed = strip_comments(text);
    let patterns = [
        // `<InDevelopmentMark ... id="x" ... />`, attributes possibly wrapped
        // across lines. `[^>]*?` cannot cross the tag close, so the id must
        // belong to this element.
        r#"InDevelopmentMark[^>]*?\bid\s*=\s*\{?\s*['"]([^'"]+)['"]"#,
        r#"inDevelopmentSurface\s*\(\s*['"]([^'"]+)['"]"#,
    ];
    let mut ids: Vec<(usize, String)> = Vec::new();
    for pattern in patterns {
        let re = Regex::new(pattern).expect("static regex");
        for c in re.captures_iter(&scrubbed) {
            let m = c.get(0).expect("group 0");
            ids.push((line_of(&scrubbed, m.start()), c[1].to_string()));
        }
    }
    ids.sort();
    ids
}

/// Half-open line ranges of the workflow's YAML sequence items (its steps).
/// A step opens at a `- ` sequence marker and runs to the next one; anything
/// before the first marker is its own leading region so a build invocation
/// there is still attributed somewhere.
fn step_ranges(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut starts: Vec<usize> = vec![0];
    for (idx, line) in lines.iter().enumerate() {
        let body = line.trim_start();
        if idx != 0 && (body.starts_with("- ") || body == "-") {
            starts.push(idx);
        }
    }
    starts.dedup();
    let mut ranges = Vec::new();
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(lines.len());
        ranges.push((*start, end));
    }
    ranges
}

/// Flag any frontend-producing build step in the release workflow whose own
/// step does not set the channel stamp to `1`.
pub fn channel_stamp_findings(text: &str) -> Vec<Finding> {
    let scrubbed = strip_yaml_comments(text);
    let lines: Vec<&str> = scrubbed.lines().collect();
    let stamp_ok = Regex::new(&format!(
        r#"^[ \t]*{CHANNEL_STAMP}[ \t]*:[ \t]*['"]?1['"]?[ \t]*$"#
    ))
    .expect("static regex");
    let stamp_any = Regex::new(&format!(r#"^[ \t]*{CHANNEL_STAMP}[ \t]*:"#)).expect("static regex");

    let mut findings = Vec::new();
    for (start, end) in step_ranges(&lines) {
        let step = &lines[start..end];
        let Some((offset, marker)) = step.iter().enumerate().find_map(|(i, line)| {
            RELEASE_BUILD_MARKERS
                .iter()
                .chain(RUST_BUILD_MARKERS)
                .find(|m| line.contains(**m))
                .map(|m| (i, *m))
        }) else {
            continue;
        };
        if step.iter().any(|line| stamp_ok.is_match(line)) {
            continue;
        }
        let detail = if step.iter().any(|line| stamp_any.is_match(line)) {
            format!(
                "builds the app ({marker}) but sets {CHANNEL_STAMP} to something other than \
1; a published artefact would carry surfaces still registered as in-development"
            )
        } else {
            format!(
                "builds the app ({marker}) without setting {CHANNEL_STAMP} to 1 in this \
step; a published artefact would carry surfaces still registered as in-development"
            )
        };
        findings.push(Finding {
            path: RELEASE_WORKFLOW.to_string(),
            lineno: start + offset + 1,
            detail,
        });
    }
    findings
}

/// Flag a settings module that does not read the stamp, by this guard's
/// name, at compile time (comments do not count).
pub fn rust_reader_findings(text: &str) -> Vec<Finding> {
    let read = format!("option_env!(\"{CHANNEL_STAMP}\")");
    if strip_comments(text).contains(&read) {
        return Vec::new();
    }
    vec![Finding {
        path: RUST_CHANNEL_READER.to_string(),
        lineno: 1,
        detail: format!(
            "does not read {CHANNEL_STAMP} with `{read}`; the backend could not tell a published \
build and would run its in-development features there"
        ),
    }]
}

/// Repo-relative tracked frontend sources, excluding the register itself and
/// test files (a test may exercise an unregistered id deliberately).
fn tracked_sources(repo_root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = git::run(&["ls-files", "--", "app/src"], repo_root)?;
    Ok(out
        .lines()
        .filter(|line| line.ends_with(".ts") || line.ends_with(".svelte"))
        .filter(|line| *line != REGISTRY)
        .filter(|line| !line.ends_with(".test.ts") && !line.ends_with(".spec.ts"))
        .map(str::to_string)
        .collect())
}

fn evaluate(repo_root: &std::path::Path) -> Result<Vec<Finding>, String> {
    let registry_path = repo_root.join(REGISTRY);
    let registry_text = std::fs::read_to_string(&registry_path)
        .map_err(|e| format!("in-development: cannot read {REGISTRY}: {e}"))?;
    let declared = declared_ids(&registry_text);

    let mut findings = Vec::new();
    let mut referenced: Vec<String> = Vec::new();

    for path in tracked_sources(repo_root)? {
        let full = repo_root.join(&path);
        let text = match std::fs::read_to_string(&full) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("in-development: cannot read {path}: {e}")),
        };
        for (lineno, id) in referenced_ids(&text) {
            if !declared.iter().any(|(_, d)| *d == id) {
                findings.push(Finding {
                    path: path.replace('\\', "/"),
                    lineno,
                    detail: format!(
                        "references in-development surface {id:?}, which is not declared in {REGISTRY}"
                    ),
                });
            }
            referenced.push(id);
        }
    }

    for (lineno, id) in &declared {
        if !referenced.contains(id) {
            findings.push(Finding {
                path: REGISTRY.to_string(),
                lineno: *lineno,
                detail: format!(
                    "declares in-development surface {id:?} with no consumer; if its capability \
landed, delete the entry and its marker"
                ),
            });
        }
    }

    let workflow_path = repo_root.join(RELEASE_WORKFLOW);
    match std::fs::read_to_string(&workflow_path) {
        Ok(text) => findings.extend(channel_stamp_findings(&text)),
        Err(e) => {
            return Err(format!(
                "in-development: cannot read {RELEASE_WORKFLOW}: {e}"
            ))
        }
    }

    let reader_path = repo_root.join(RUST_CHANNEL_READER);
    match std::fs::read_to_string(&reader_path) {
        Ok(text) => findings.extend(rust_reader_findings(&text)),
        Err(e) => {
            return Err(format!(
                "in-development: cannot read {RUST_CHANNEL_READER}: {e}"
            ))
        }
    }

    findings.sort_by(|a, b| (&a.path, a.lineno).cmp(&(&b.path, b.lineno)));
    Ok(findings)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let findings = evaluate(&repo_root)?;

    if findings.is_empty() {
        println!("in-development: register, consumers, and published channel all agree.");
        return Ok(0);
    }

    eprintln!(
        "in-development: register/consumer mismatch.\n\nA surface that ships ahead of its \
capability is declared once in {REGISTRY} and marked where it renders. Graduating it means \
deleting the entry and the marker together, and a published build must stamp \
{CHANNEL_STAMP}=1. Offenders:\n"
    );
    for f in &findings {
        eprintln!("  {}:{}: {}", f.path, f.lineno, f.detail);
    }

    if warn_only {
        eprintln!("\nin-development: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── register and consumer scanning ────────────────────────────────────

    #[test]
    fn reads_declared_ids_from_the_register() {
        let text = "export const IN_DEVELOPMENT_SURFACES = [\n\t{\n\t\tid: 'alpha',\n\t\tsummary: 'x',\n\t},\n];\n";
        assert_eq!(declared_ids(text), vec![(3, "alpha".to_string())]);
    }

    #[test]
    fn reads_a_marker_reference_from_markup() {
        let text = "<InDevelopmentMark id=\"alpha\" />\n";
        assert_eq!(referenced_ids(text), vec![(1, "alpha".to_string())]);
    }

    #[test]
    fn reads_a_helper_reference_from_logic() {
        let text = "const s = inDevelopmentSurface('beta');\n";
        assert_eq!(referenced_ids(text), vec![(1, "beta".to_string())]);
    }

    #[test]
    fn reads_a_marker_whose_attributes_span_lines() {
        let text = "<InDevelopmentMark\n\talign=\"left\"\n\tid=\"alpha\"\n/>\n";
        assert_eq!(referenced_ids(text), vec![(1, "alpha".to_string())]);
    }

    #[test]
    fn tolerates_whitespace_and_braces_around_the_id() {
        assert_eq!(
            referenced_ids("<InDevelopmentMark id = { 'alpha' } />\n"),
            vec![(1, "alpha".to_string())]
        );
    }

    #[test]
    fn ignores_a_marker_named_only_in_a_line_comment() {
        let text = "// <InDevelopmentMark id=\"ghost\" />\n";
        assert!(referenced_ids(text).is_empty());
    }

    #[test]
    fn ignores_a_marker_named_only_in_a_block_comment() {
        let text = "/* see <InDevelopmentMark id=\"ghost\" /> */\nlet x = 1;\n";
        assert!(referenced_ids(text).is_empty());
    }

    #[test]
    fn ignores_a_marker_named_only_in_a_markup_comment() {
        let text = "<!-- <InDevelopmentMark id=\"ghost\" /> -->\n";
        assert!(referenced_ids(text).is_empty());
    }

    #[test]
    fn ignores_an_id_declared_only_in_a_register_comment() {
        let text = "\t\t// id: 'ghost',\n\t\tid: 'real',\n";
        assert_eq!(declared_ids(text), vec![(2, "real".to_string())]);
    }

    #[test]
    fn keeps_a_url_in_a_string_literal_intact() {
        // `//` mid-line is not a comment opener, so the helper call survives.
        let text = "const u = 'https://x.example'; const s = inDevelopmentSurface('beta');\n";
        assert_eq!(referenced_ids(text), vec![(1, "beta".to_string())]);
    }

    #[test]
    fn ignores_a_marker_import_without_an_id() {
        let text = "import { InDevelopmentMark } from '$lib/inDevelopment';\n";
        assert!(referenced_ids(text).is_empty());
    }

    #[test]
    fn does_not_attribute_a_later_elements_id_to_an_idless_marker() {
        let text = "<InDevelopmentMark />\n<Other id=\"alpha\" />\n";
        assert!(referenced_ids(text).is_empty());
    }

    #[test]
    fn tolerates_either_quote_style() {
        assert_eq!(
            referenced_ids("<InDevelopmentMark id='alpha' />\n"),
            vec![(1, "alpha".to_string())]
        );
        assert_eq!(
            declared_ids("\t\tid: \"beta\",\n"),
            vec![(1, "beta".to_string())]
        );
    }

    // ── published-channel stamp ───────────────────────────────────────────

    #[test]
    fn flags_a_rust_build_step_missing_the_channel_stamp() {
        let yml = "      - name: Build\n        run: cargo build --release -p entropia-orme\n";
        let f = channel_stamp_findings(yml);
        assert_eq!(f.len(), 1);
        assert!(f[0].detail.contains("cargo build"));
        let stamped = "      - name: Build\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: '1'\n        run: cargo build --release\n";
        assert!(channel_stamp_findings(stamped).is_empty());
    }

    #[test]
    fn accepts_a_settings_module_that_reads_the_stamp() {
        let text = "pub fn published_build() -> bool {\n    option_env!(\"ENTROPIAORME_STABLE_CHANNEL\") == Some(\"1\")\n}\n";
        assert!(rust_reader_findings(text).is_empty());
    }

    #[test]
    fn flags_a_settings_module_reading_another_name_or_none() {
        let renamed = "option_env!(\"ENTROPIAORME_CHANNEL\") == Some(\"1\")";
        assert_eq!(rust_reader_findings(renamed).len(), 1);
        let commented = "// option_env!(\"ENTROPIAORME_STABLE_CHANNEL\")\nfn published_build() -> bool { false }\n";
        assert_eq!(rust_reader_findings(commented).len(), 1);
        assert_eq!(rust_reader_findings("").len(), 1);
    }

    #[test]
    fn flags_a_frontend_build_step_missing_the_channel_stamp() {
        let yml = "      - name: Build\n        run: npm run tauri -- build --bundles deb\n";
        let f = channel_stamp_findings(yml);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 2);
        assert!(f[0].detail.contains("without setting"));
    }

    #[test]
    fn accepts_a_build_step_whose_env_block_stamps_the_channel() {
        let yml = "      - name: Build\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: '1'\n        run: ./scripts/build-installer.ps1\n";
        assert!(channel_stamp_findings(yml).is_empty());
    }

    #[test]
    fn accepts_a_stamp_declared_after_the_run_line() {
        // YAML does not order mapping keys; the whole step is in scope.
        let yml = "      - name: Build\n        run: ./scripts/build-installer.ps1\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: '1'\n";
        assert!(channel_stamp_findings(yml).is_empty());
    }

    #[test]
    fn rejects_a_stamp_whose_value_is_not_one() {
        let yml = "      - name: Build\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: '0'\n        run: ./scripts/build-installer.ps1\n";
        let f = channel_stamp_findings(yml);
        assert_eq!(f.len(), 1);
        assert!(f[0].detail.contains("something other than"));
    }

    #[test]
    fn rejects_a_build_step_vouched_for_by_a_neighbours_stamp() {
        let yml = "      - name: Stamped\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: '1'\n        run: echo ok\n      - name: Unstamped\n        run: ./scripts/build-installer.ps1\n";
        let f = channel_stamp_findings(yml);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lineno, 6);
    }

    #[test]
    fn ignores_a_stamp_that_appears_only_in_a_comment() {
        let yml = "      - name: Build\n        # ENTROPIAORME_STABLE_CHANNEL: '1'\n        run: ./scripts/build-installer.ps1\n";
        assert_eq!(channel_stamp_findings(yml).len(), 1);
    }

    #[test]
    fn ignores_a_comment_naming_a_build_script() {
        let yml = "        # build-installer.ps1 produces the per-user MSI payload\n";
        assert!(channel_stamp_findings(yml).is_empty());
    }

    #[test]
    fn accepts_an_unquoted_stamp_value() {
        let yml = "      - name: Build\n        env:\n          ENTROPIAORME_STABLE_CHANNEL: 1\n        run: ./scripts/build-installer.ps1\n";
        assert!(channel_stamp_findings(yml).is_empty());
    }

    #[test]
    fn a_glob_earlier_in_the_workflow_does_not_blank_later_steps() {
        // Regression: treating `/*` as a block-comment opener in YAML blanked
        // everything to the next `*/` or EOF, silently disabling the rule for
        // every step after the first glob. Real workflows are full of globs.
        let yml = "      - name: Upload\n        with:\n          path: linux-dist/*.deb\n      - name: Build\n        run: ./scripts/build-installer.ps1\n";
        let f = channel_stamp_findings(yml);
        assert_eq!(
            f.len(),
            1,
            "the build step after a glob must still be checked"
        );
        assert_eq!(f[0].lineno, 5);
    }

    #[test]
    fn an_html_comment_opener_in_yaml_is_not_a_block_comment() {
        let yml =
            "      - name: Build\n        run: echo '<!--' && ./scripts/build-installer.ps1\n";
        assert_eq!(channel_stamp_findings(yml).len(), 1);
    }

    #[test]
    fn the_real_release_workflow_stamps_every_build_step() {
        // Runs the rule over the checked-in workflow, not a fixture. Synthetic
        // cases all passed while a glob in the real file was silently blanking
        // later steps, so the real file is the one that has to be asserted.
        let root = git::repo_root().expect("repo root");
        let text = std::fs::read_to_string(root.join(RELEASE_WORKFLOW))
            .unwrap_or_else(|e| panic!("cannot read {RELEASE_WORKFLOW}: {e}"));

        // Guard against a vacuous pass: if the workflow's build invocations are
        // renamed, the markers match nothing and the rule trivially succeeds.
        for marker in RELEASE_BUILD_MARKERS {
            assert!(
                text.contains(marker),
                "no build invocation matches {marker:?}; the rule would pass vacuously"
            );
        }

        let findings = channel_stamp_findings(&text);
        assert!(
            findings.is_empty(),
            "the release workflow must stamp every frontend build step: {findings:#?}"
        );
    }

    #[test]
    fn flags_every_unstamped_build_step_independently() {
        let yml = "      - name: A\n        run: npm run tauri -- build\n      - name: B\n        run: ./scripts/build-installer.ps1\n";
        assert_eq!(channel_stamp_findings(yml).len(), 2);
    }
}
