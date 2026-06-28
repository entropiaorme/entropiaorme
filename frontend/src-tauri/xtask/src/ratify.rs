//! The golden-ratification guard (range mode), reimplementing the former Python
//! guard of the same name.
//!
//! Guards against silently ratifying a regression through the goldens workflow.
//! A committed change to a golden file re-ratifies whatever the pipeline
//! currently produces, so in range mode (the pull-request gate) such a change
//! must carry BOTH:
//!   - the `test: regenerate goldens` commit-message marker (consciousness), AND
//!   - a recorded adversarial verdict: a report committed in the same range
//!     carrying an `ORACLE-RATIFICATION ... VERDICT: ratification-sound` block
//!     that names every changed golden set and is no older than the last golden
//!     change in the range (correctness sign-off).
//!
//! This binary ports the range-mode pull-request gate (the only mode CI runs).
//! The staged / working-tree advisory mode and `--warn-only` of the Python
//! original are deliberately not ported: CI invokes range mode exclusively, and
//! the guard must fail-closed, never pass vacuously.
//!
//! Path note: the goldens and ratification reports were relocated out of the
//! retired Python tree. The detected golden paths are now under
//! `frontend/src-tauri/contracts/`, `frontend/src-tauri/fixtures/corpus/`, and
//! `frontend/src-tauri/eo-wire/tests/fixtures/`; the reports live at
//! `frontend/src-tauri/ratifications/<slug>.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::git;

const RATIFICATIONS_PREFIX: &str = "frontend/src-tauri/ratifications/";
const CONTRACTS_PREFIX: &str = "frontend/src-tauri/contracts/";
const CORPUS_PREFIX: &str = "frontend/src-tauri/fixtures/corpus/";
const WIRE_FIXTURES_PREFIX: &str = "frontend/src-tauri/eo-wire/tests/fixtures/";
const EXPECTED_SEGMENT: &str = "/expected/";

/// The documented goldens-regeneration commit-message marker.
fn marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\btest:\s*regenerate\s+goldens\b").expect("valid marker pattern")
    })
}

/// True when a repository-relative path is a committed golden file.
///
/// Covers the contract snapshots (`contracts/*.snapshot.json`), the per-scenario
/// corpus goldens (anything under a corpus `expected/` directory: the
/// fingerprint, the DB-state snapshot, and the HTTP-response goldens), and the
/// eo-wire conformance fixtures (the normaliser conformance set and the
/// yml-family consistency goldens).
pub fn is_golden_path(path: &str) -> bool {
    let posix = path.replace('\\', "/");
    if posix.starts_with(CONTRACTS_PREFIX) && posix.ends_with(".snapshot.json") {
        return true;
    }
    if posix.starts_with(CORPUS_PREFIX) && posix.contains(EXPECTED_SEGMENT) {
        return true;
    }
    posix.starts_with(WIRE_FIXTURES_PREFIX)
}

/// True when a repo-relative path is a committed ratification report.
pub fn is_ratification_artifact(path: &str) -> bool {
    let posix = path.replace('\\', "/");
    posix.starts_with(RATIFICATIONS_PREFIX) && posix.ends_with(".md")
}

/// A coarse identifier for the golden 'set' a changed golden path belongs to.
///
/// Contract snapshots key on their stem (`openapi`, `event_schemas`); corpus
/// scenario goldens key on their scenario directory; eo-wire fixtures key on the
/// conformance-set or yml-family file stem. The verdict's `goldens:` field must
/// name each changed set.
pub fn golden_set_key(path: &str) -> String {
    let posix = path.replace('\\', "/");
    if let Some(rest) = posix.strip_prefix(CONTRACTS_PREFIX) {
        if let Some(stem) = rest.strip_suffix(".snapshot.json") {
            return stem.to_string();
        }
    }
    if let Some(rest) = posix.strip_prefix(WIRE_FIXTURES_PREFIX) {
        // yml_family/<name>.json or <name>.json -> the file stem.
        let file = rest.rsplit('/').next().unwrap_or(rest);
        if let Some(stem) = file.strip_suffix(".json") {
            return stem.to_string();
        }
        return file.to_string();
    }
    if let Some(caps) = scenario_re().captures(&posix) {
        return caps[1].to_string();
    }
    posix
}

fn scenario_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/([^/]+)/expected/").expect("valid scenario pattern"))
}

/// Lowercase and strip non-alphanumerics, so `basic-hunt` == `basic_hunt`.
fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// A parsed `ORACLE-RATIFICATION` verdict block from one report.
#[derive(Debug, Clone)]
pub struct RatificationVerdict {
    pub path: String,
    pub verdict: String,
    pub goldens: Vec<String>,
    #[allow(dead_code)]
    pub range: Option<String>,
}

impl RatificationVerdict {
    pub fn is_sound(&self) -> bool {
        self.verdict == "ratification-sound"
    }
}

fn fence_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)```[^\n]*\n(.*?)```").expect("valid fence pattern"))
}
fn header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)ORACLE-RATIFICATION\b").expect("valid header pattern"))
}
fn verdict_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?im)^[^\S\n]*VERDICT:[^\S\n]*(ratification-sound|regression-suspected|needs-user-judgement)[^\S\n]*$",
        )
        .expect("valid verdict pattern")
    })
}
fn goldens_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?im)^[^\S\n]*goldens:[^\S\n]*(.+?)[^\S\n]*$").expect("valid goldens pattern")
    })
}
fn range_field_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?im)^[^\S\n]*range:[^\S\n]*(.+?)[^\S\n]*$").expect("valid range pattern")
    })
}

/// Parse the verdict block out of a ratification report, or `None`.
///
/// Read only from a fenced code block carrying the `ORACLE-RATIFICATION` header
/// and a recognised `VERDICT:` line, exactly as the convention prescribes, so a
/// `VERDICT:` line in surrounding prose cannot satisfy the gate.
pub fn parse_ratification_artifact(path: &str, text: &str) -> Option<RatificationVerdict> {
    for caps in fence_re().captures_iter(text) {
        let block = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if !header_re().is_match(block) {
            continue;
        }
        let Some(vm) = verdict_re().captures(block) else {
            continue;
        };
        let goldens = goldens_re()
            .captures(block)
            .map(|gm| {
                gm[1]
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|t| !t.is_empty())
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let range = range_field_re()
            .captures(block)
            .map(|rm| rm[1].trim().to_string());
        return Some(RatificationVerdict {
            path: path.to_string(),
            verdict: vm[1].to_lowercase(),
            goldens,
            range,
        });
    }
    None
}

/// True when the verdict's `goldens:` field names `set_key`.
///
/// The set key must appear within one of the goldens tokens after alphanumeric
/// normalisation. The match is one-directional (key within token, not the
/// reverse), so a short or broad token cannot bless a specific set.
fn verdict_covers(verdict: &RatificationVerdict, set_key: &str) -> bool {
    let key = normalise(set_key);
    if key.is_empty() {
        return false;
    }
    verdict
        .goldens
        .iter()
        .any(|token| normalise(token).contains(&key))
}

/// The right-hand side of an `A..B` / `A...B` range (a bare ref is itself).
fn range_tip(commit_range: &str) -> String {
    // Split on two-or-three dots; take the last segment.
    let re = Regex::new(r"\.\.\.?").expect("valid range split");
    let tip = re.split(commit_range).last().unwrap_or("").trim();
    if tip.is_empty() {
        "HEAD".to_string()
    } else {
        tip.to_string()
    }
}

fn changed_paths(repo_root: &Path, commit_range: &str) -> Result<Vec<String>, String> {
    let out = git::run(&["diff", "--name-only", commit_range], repo_root)?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect())
}

fn commit_messages(repo_root: &Path, commit_range: &str) -> Result<Vec<String>, String> {
    let out = git::run(&["log", "--format=%B%x00", commit_range], repo_root)?;
    Ok(out
        .split('\u{0}')
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.to_string())
        .collect())
}

fn file_at(repo_root: &Path, reference: &str, path: &str) -> Option<String> {
    git::run(&["show", &format!("{reference}:{path}")], repo_root).ok()
}

fn commit_index(repo_root: &Path, commit_range: &str) -> Result<BTreeMap<String, usize>, String> {
    let out = git::run(&["rev-list", "--reverse", commit_range], repo_root)?;
    Ok(out
        .split_whitespace()
        .enumerate()
        .map(|(i, sha)| (sha.to_string(), i))
        .collect())
}

fn last_commit_touching(
    repo_root: &Path,
    commit_range: &str,
    paths: &[String],
) -> Result<Option<String>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut args: Vec<&str> = vec!["log", "-1", "--format=%H", commit_range, "--"];
    for p in paths {
        args.push(p.as_str());
    }
    let out = git::run(&args, repo_root)?;
    let trimmed = out.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

/// True when a golden was changed later in the range than the sound verdict.
fn is_artifact_stale(
    repo_root: &Path,
    commit_range: &str,
    golden_paths: &[String],
    sound_paths: &[String],
) -> Result<bool, String> {
    if golden_paths.is_empty() || sound_paths.is_empty() {
        return Ok(false);
    }
    let order = commit_index(repo_root, commit_range)?;
    let last_golden = last_commit_touching(repo_root, commit_range, golden_paths)?;
    let last_sound = last_commit_touching(repo_root, commit_range, sound_paths)?;
    let pos = |c: &Option<String>| -> i64 {
        match c {
            Some(sha) => order.get(sha).map(|i| *i as i64).unwrap_or(-1),
            None => -1,
        }
    };
    Ok(pos(&last_golden) > pos(&last_sound))
}

/// Outcome of inspecting a diff for unratified golden changes.
pub struct Evaluation {
    pub golden_paths: Vec<String>,
    pub has_marker: bool,
    pub ratification_artifacts: Vec<String>,
    pub sound_verdicts: Vec<RatificationVerdict>,
    pub unblessed_sets: Vec<String>,
    pub artifact_stale: bool,
}

impl Evaluation {
    pub fn touches_goldens(&self) -> bool {
        !self.golden_paths.is_empty()
    }
    pub fn has_sound_verdict(&self) -> bool {
        !self.sound_verdicts.is_empty()
    }
    /// A clean result against the ratification rule (range mode).
    pub fn ok(&self) -> bool {
        if !self.touches_goldens() {
            return true;
        }
        self.has_marker
            && self.has_sound_verdict()
            && self.unblessed_sets.is_empty()
            && !self.artifact_stale
    }
}

fn evaluate(repo_root: &Path, commit_range: &str) -> Result<Evaluation, String> {
    let paths = changed_paths(repo_root, commit_range)?;
    let mut goldens: Vec<String> = paths
        .iter()
        .filter(|p| is_golden_path(p))
        .cloned()
        .collect();
    goldens.sort();
    let messages = commit_messages(repo_root, commit_range)?;
    let has_marker = messages.iter().any(|m| marker_re().is_match(m));

    let tip = range_tip(commit_range);
    let mut found: Vec<String> = Vec::new();
    let mut verdicts: Vec<RatificationVerdict> = Vec::new();
    for path in &paths {
        if !is_ratification_artifact(path) {
            continue;
        }
        let Some(text) = file_at(repo_root, &tip, path) else {
            continue; // deleted by the tip: not an added/modified artefact
        };
        found.push(path.clone());
        if let Some(v) = parse_ratification_artifact(path, &text) {
            verdicts.push(v);
        }
    }
    found.sort();
    let sound_verdicts: Vec<RatificationVerdict> =
        verdicts.into_iter().filter(|v| v.is_sound()).collect();

    let mut unblessed_sets: Vec<String> = Vec::new();
    let mut artifact_stale = false;
    if !goldens.is_empty() && !sound_verdicts.is_empty() {
        let keys: BTreeSet<String> = goldens.iter().map(|g| golden_set_key(g)).collect();
        unblessed_sets = keys
            .into_iter()
            .filter(|key| !sound_verdicts.iter().any(|v| verdict_covers(v, key)))
            .collect();
        unblessed_sets.sort();
        let sound_paths: Vec<String> = sound_verdicts.iter().map(|v| v.path.clone()).collect();
        artifact_stale = is_artifact_stale(repo_root, commit_range, &goldens, &sound_paths)?;
    }

    Ok(Evaluation {
        golden_paths: goldens,
        has_marker,
        ratification_artifacts: found,
        sound_verdicts,
        unblessed_sets,
        artifact_stale,
    })
}

fn golden_diff(repo_root: &Path, paths: &[String], commit_range: &str) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut args: Vec<&str> = vec!["diff", commit_range, "--"];
    for p in paths {
        args.push(p.as_str());
    }
    git::run(&args, repo_root).unwrap_or_default()
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let commit_range = crate::flag_value(args, "--range")?.ok_or_else(|| {
        "check-golden-ratification: --range <BASE>..<HEAD> is required (the guard \
fails closed; it does not run without an explicit range)."
            .to_string()
    })?;
    let repo_root = git::repo_root()?;

    // Fail-closed: any git error resolving the range or reading a report
    // propagates as an Err (non-zero exit), never a vacuous pass.
    let result = evaluate(&repo_root, &commit_range)?;

    if !result.touches_goldens() {
        println!("check-golden-ratification: no golden files touched; nothing to guard.");
        return Ok(0);
    }

    if result.ok() {
        println!(
            "check-golden-ratification: golden change carries the \
'test: regenerate goldens' marker and a recorded adversarial \
'ratification-sound' verdict naming the changed sets; \
ratification is deliberate and signed off."
        );
        println!("Goldens:");
        for path in &result.golden_paths {
            println!("  {path}");
        }
        println!("Ratification artefacts:");
        for path in &result.ratification_artifacts {
            println!("  {path}");
        }
        return Ok(0);
    }

    let listing = result
        .golden_paths
        .iter()
        .map(|p| format!("  {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    let diff = golden_diff(&repo_root, &result.golden_paths, &commit_range);

    if !result.has_marker {
        eprintln!(
            "check-golden-ratification: golden files changed without the \
documented 'test: regenerate goldens' commit-message marker.\n\n\
Regenerating a golden re-ratifies whatever the pipeline currently \
produces, so an unmarked golden change can silently lock in a \
regression. Either:\n  \
- this is a deliberate re-ratification: record it with a commit \
whose subject is 'test: regenerate goldens ...' naming the \
regenerated sets (see TESTING.md), or\n  \
- this is an unintended golden move: revert it and fix the \
underlying change so the goldens hold.\n\n\
Changed golden files:\n{listing}\n"
        );
    } else if !result.has_sound_verdict() {
        eprintln!(
            "check-golden-ratification: golden files changed with the \
'test: regenerate goldens' marker, but no recorded \
'ratification-sound' verdict was present in this range.\n\n\
The marker proves the regeneration was conscious; it cannot prove \
the diff is correct. An adversarial review of the golden diff must \
be recorded, and the resulting report \
(carrying the ORACLE-RATIFICATION ... VERDICT: ratification-sound \
block) must be committed to frontend/src-tauri/ratifications/<slug>.md \
in the same range as the golden change. A verdict artefact from a \
prior regeneration does not count: it must be added or modified in \
this range.\n\n\
Changed golden files:\n{listing}\n"
        );
    } else if result.artifact_stale {
        eprintln!(
            "check-golden-ratification: a 'ratification-sound' verdict is \
present, but a golden was changed later in this range than the \
verdict was recorded.\n\n\
The verdict reviewed the earlier golden state, not the final one, so \
it cannot bless the later edit. Re-run the adversarial review against \
the current diff and re-commit the report (carrying a fresh \
ORACLE-RATIFICATION ... VERDICT: ratification-sound block) so the \
verdict is no earlier than the last golden change in the range.\n\n\
Changed golden files:\n{listing}\n"
        );
    } else {
        let unblessed = result
            .unblessed_sets
            .iter()
            .map(|k| format!("  {k}"))
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!(
            "check-golden-ratification: a 'ratification-sound' verdict is \
present, but it does not name every changed golden set. A verdict \
recorded for one set cannot bless another, so each changed set must \
appear in the verdict's 'goldens:' field.\n\n\
Golden sets without a matching sound verdict:\n{unblessed}\n\n\
Changed golden files:\n{listing}\n"
        );
    }

    if !diff.is_empty() {
        eprintln!("Golden diff for review:\n");
        eprintln!("{diff}");
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_REPORT: &str = "\
# Ratification: typed API response models\n\
\n\
Some prose mentioning VERDICT: regression-suspected should not count.\n\
\n\
```text\n\
ORACLE-RATIFICATION\n\
range: c84cac6..HEAD\n\
goldens: frontend/src-tauri/contracts/openapi.snapshot.json\n\
VERDICT: ratification-sound\n\
```\n";

    #[test]
    fn parses_real_committed_report_format() {
        let v = parse_ratification_artifact("frontend/src-tauri/ratifications/x.md", REAL_REPORT)
            .expect("verdict parsed");
        assert!(v.is_sound());
        assert_eq!(
            v.goldens,
            vec!["frontend/src-tauri/contracts/openapi.snapshot.json"]
        );
        assert_eq!(v.range.as_deref(), Some("c84cac6..HEAD"));
    }

    #[test]
    fn prose_verdict_outside_fence_does_not_count() {
        let text = "Prose: VERDICT: ratification-sound here is just text.\nNo fence at all.";
        assert!(parse_ratification_artifact("p.md", text).is_none());
    }

    #[test]
    fn fence_without_header_is_ignored() {
        let text = "```text\nVERDICT: ratification-sound\n```";
        assert!(parse_ratification_artifact("p.md", text).is_none());
    }

    #[test]
    fn readme_placeholder_block_is_not_a_verdict() {
        // The ratifications/README.md fenced block lists all three verdicts on
        // the VERDICT line, which must NOT parse as a verdict.
        let text = "```text\nORACLE-RATIFICATION\nrange: <commit-range>\n\
goldens: <comma-separated sets reviewed>\n\
VERDICT: ratification-sound | regression-suspected | needs-user-judgement\n```";
        assert!(parse_ratification_artifact("README.md", text).is_none());
    }

    #[test]
    fn golden_paths_classified() {
        assert!(is_golden_path(
            "frontend/src-tauri/contracts/openapi.snapshot.json"
        ));
        assert!(is_golden_path(
            "frontend/src-tauri/contracts/event_schemas.snapshot.json"
        ));
        assert!(is_golden_path(
            "frontend/src-tauri/fixtures/corpus/scripted/basic_hunt_10_events/expected/fingerprint.jsonl"
        ));
        assert!(is_golden_path(
            "frontend/src-tauri/eo-wire/tests/fixtures/normalizer_conformance.json"
        ));
        assert!(is_golden_path(
            "frontend/src-tauri/eo-wire/tests/fixtures/yml_family/hotbar_slot_use.json"
        ));
        // Old backend paths are NO LONGER goldens.
        assert!(!is_golden_path(
            "backend/tests/expected/openapi.snapshot.json"
        ));
        assert!(!is_golden_path("backend/testing/COVERAGE.md"));
        // The ratification report is not a golden.
        assert!(!is_golden_path(
            "frontend/src-tauri/ratifications/typed-api-responses.md"
        ));
    }

    #[test]
    fn ratification_artifact_classified() {
        assert!(is_ratification_artifact(
            "frontend/src-tauri/ratifications/x.md"
        ));
        assert!(is_ratification_artifact(
            "frontend/src-tauri/ratifications/README.md"
        ));
        assert!(!is_ratification_artifact(
            "frontend/src-tauri/ratifications/notes.txt"
        ));
        assert!(!is_ratification_artifact(
            "backend/testing/ratifications/x.md"
        ));
    }

    #[test]
    fn golden_set_keys() {
        assert_eq!(
            golden_set_key("frontend/src-tauri/contracts/openapi.snapshot.json"),
            "openapi"
        );
        assert_eq!(
            golden_set_key("frontend/src-tauri/contracts/event_schemas.snapshot.json"),
            "event_schemas"
        );
        assert_eq!(
            golden_set_key(
                "frontend/src-tauri/fixtures/corpus/scripted/basic_hunt_10_events/expected/fingerprint.jsonl"
            ),
            "basic_hunt_10_events"
        );
        assert_eq!(
            golden_set_key("frontend/src-tauri/eo-wire/tests/fixtures/normalizer_conformance.json"),
            "normalizer_conformance"
        );
        assert_eq!(
            golden_set_key(
                "frontend/src-tauri/eo-wire/tests/fixtures/yml_family/hotbar_slot_use.json"
            ),
            "hotbar_slot_use"
        );
    }

    #[test]
    fn verdict_covers_is_one_directional() {
        let v = RatificationVerdict {
            path: "p.md".to_string(),
            verdict: "ratification-sound".to_string(),
            goldens: vec!["basic_hunt_10_events".to_string()],
            range: None,
        };
        // Key within token: covered.
        assert!(verdict_covers(&v, "basic_hunt_10_events"));
        // A broad token cannot bless a specific set.
        let broad = RatificationVerdict {
            path: "p.md".to_string(),
            verdict: "ratification-sound".to_string(),
            goldens: vec!["hunt".to_string()],
            range: None,
        };
        assert!(!verdict_covers(&broad, "basic_hunt_10_events"));
        // openapi key covered by a longer token.
        let oa = RatificationVerdict {
            path: "p.md".to_string(),
            verdict: "ratification-sound".to_string(),
            goldens: vec!["frontend/src-tauri/contracts/openapi.snapshot.json".to_string()],
            range: None,
        };
        assert!(verdict_covers(&oa, "openapi"));
    }

    #[test]
    fn range_tip_extraction() {
        assert_eq!(range_tip("a..b"), "b");
        assert_eq!(range_tip("a...b"), "b");
        assert_eq!(range_tip("HEAD"), "HEAD");
    }
}
