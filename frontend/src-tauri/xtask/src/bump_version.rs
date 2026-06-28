//! Port of `backend/scripts/bump_version.py`.
//!
//! Writer paired with the `version-stamps` checker: sets the same version string
//! in the three stamps that check governs:
//!   - `frontend/package.json` (top-level `version`),
//!   - `frontend/src-tauri/Cargo.toml` (`[workspace.package] version`),
//!   - `frontend/src-tauri/entropia-orme/tauri.conf.json` (top-level `version`).
//!
//! Edits are surgical: only the version token is rewritten, so file formatting,
//! key order, and comments are preserved and the JSON manifests are not
//! reserialised. The `[workspace.package]` edit is scoped to that table, so
//! `[workspace.dependencies]` version pins are never touched.
//!
//! `Cargo.lock`'s recorded member versions are intentionally left to refresh on
//! the next `cargo` invocation: the three stamps above are the parity contract;
//! the lock is a build artefact. `CURRENT_TOS_VERSION` is a separate namespace
//! and is not touched, mirroring the parity guard.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::git;

const PACKAGE_JSON: &str = "frontend/package.json";
const CARGO_TOML: &str = "frontend/src-tauri/Cargo.toml";
const TAURI_CONF: &str = "frontend/src-tauri/entropia-orme/tauri.conf.json";

/// Semver core with optional pre-release / build metadata. Numeric core parts
/// reject leading zeros (01.2.3 is not valid semver), per the spec.
fn semver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$",
        )
        .expect("valid semver pattern")
    })
}

/// The top-level `"version": "..."` token. Only the first occurrence is
/// rewritten, which is the top-level object key in both JSON manifests.
fn json_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"("version"\s*:\s*")[^"]*(")"#).expect("valid json version pattern"))
}

/// `version = "..."` inside the `[workspace.package]` table only. The `[^[]*?`
/// guard stops the match at the next table header, so `[workspace.dependencies]`
/// pins are out of scope. `(?s)` lets `.` span lines (DOTALL).
fn cargo_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)(\[workspace\.package\][^\[]*?\bversion\s*=\s*")[^"]*(")"#)
            .expect("valid cargo version pattern")
    })
}

/// Rewrite the first version token matched by `pattern` to `version`.
///
/// Errors when the token cannot be located, mirroring the Python's `count != 1`
/// guard (a substitution that reaches zero sites is a failed bump, not a no-op).
fn sub_once(pattern: &Regex, version: &str, text: &str, rel: &str) -> Result<String, String> {
    if !pattern.is_match(text) {
        return Err(format!(
            "bump-version: could not locate the version token in {rel}"
        ));
    }
    let out = pattern.replacen(text, 1, |caps: &regex::Captures| {
        format!("{}{}{}", &caps[1], version, &caps[2])
    });
    Ok(out.into_owned())
}

/// Rewrite all three stamps to `version` in place, preserving formatting.
fn set_version(repo_root: &Path, version: &str) -> Result<(), String> {
    for (rel, pattern) in [
        (PACKAGE_JSON, json_version_re()),
        (TAURI_CONF, json_version_re()),
        (CARGO_TOML, cargo_version_re()),
    ] {
        let path = repo_root.join(rel);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("bump-version: cannot read {rel}: {e}"))?;
        let new_text = sub_once(pattern, version, &text, rel)?;
        // std::fs::write emits the bytes verbatim, so the LF line endings the
        // repo normalises to (.gitattributes eol=lf) are preserved rather than
        // translated to CRLF as a text-mode write on Windows would.
        std::fs::write(&path, new_text.as_bytes())
            .map_err(|e| format!("bump-version: cannot write {rel}: {e}"))?;
    }
    Ok(())
}

/// Read the top-level `version` string from a JSON stamp (post-write verify).
fn read_json_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("bump-version: cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("bump-version: cannot parse {}: {e}", path.display()))?;
    match value.get("version") {
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        Some(other) => Ok(other.to_string()),
        None => Err(format!(
            "bump-version: {} has no top-level 'version' key",
            path.display()
        )),
    }
}

/// Read `[workspace.package] version` from the Cargo manifest (post-write
/// verify), scanning the table the same way the parity checker does.
fn read_cargo_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("bump-version: cannot read {}: {e}", path.display()))?;
    let mut in_workspace_package = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if in_workspace_package {
            if let Some(rest) = line.strip_prefix("version") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let value = rest.trim().trim_matches('"');
                    if !value.is_empty() {
                        return Ok(value.to_string());
                    }
                }
            }
        }
    }
    Err(format!(
        "bump-version: {} has no [workspace.package] version",
        path.display()
    ))
}

/// The three stamps in declaration order, each mapped to its version string.
fn read_stamps(repo_root: &Path) -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![
        (PACKAGE_JSON, read_json_version(&repo_root.join(PACKAGE_JSON))?),
        (CARGO_TOML, read_cargo_version(&repo_root.join(CARGO_TOML))?),
        (TAURI_CONF, read_json_version(&repo_root.join(TAURI_CONF))?),
    ])
}

pub fn run(args: &[String]) -> Result<i32, String> {
    // Positional version, matching the Python CLI shape (`bump_version 0.2.0`).
    let Some(version) = args.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("bump-version: a target version is required, e.g. 0.2.0");
        return Ok(2);
    };

    if !semver_re().is_match(version) {
        eprintln!("bump-version: {version:?} is not a valid semver (X.Y.Z).");
        return Ok(2);
    }

    let repo_root = git::repo_root()?;
    set_version(&repo_root, version)?;

    // Self-verify through the same stamp-reading logic so the bump cannot claim
    // success on a stamp it failed to reach.
    let stamps = read_stamps(&repo_root)?;
    if stamps.iter().all(|(_, v)| v == version) {
        println!("bump-version: all app version stamps set to {version}.");
        for (path, _) in &stamps {
            println!("  {path}");
        }
        return Ok(0);
    }

    eprintln!("bump-version: post-write parity check FAILED:");
    for (path, v) in &stamps {
        eprintln!("  {v}\t{path}");
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_semver_forms() {
        for v in ["0.2.0", "1.0.0", "10.20.30", "1.2.3-rc.1", "1.2.3+build.5"] {
            assert!(semver_re().is_match(v), "{v} should be valid");
        }
    }

    #[test]
    fn rejects_invalid_semver_forms() {
        for v in ["1.2", "01.2.3", "v1.2.3", "1.2.3.4", "", "abc"] {
            assert!(!semver_re().is_match(v), "{v} should be invalid");
        }
    }

    #[test]
    fn rewrites_json_top_level_version_only() {
        let src = r#"{
  "name": "entropia-orme",
  "version": "0.1.0",
  "dependencies": { "version": "9.9.9" }
}"#;
        let out = sub_once(json_version_re(), "0.2.0", src, PACKAGE_JSON).unwrap();
        assert!(out.contains(r#""version": "0.2.0""#));
        // The nested dependency "version" is left untouched (first match only).
        assert!(out.contains(r#""version": "9.9.9""#));
    }

    #[test]
    fn rewrites_cargo_workspace_package_only() {
        let src = "[workspace.package]\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace.dependencies]\nfoo = { version = \"1.2.3\" }\n";
        let out = sub_once(cargo_version_re(), "0.2.0", src, CARGO_TOML).unwrap();
        assert!(out.contains("version = \"0.2.0\""));
        // The dependency pin under a later table is untouched.
        assert!(out.contains("foo = { version = \"1.2.3\" }"));
    }

    #[test]
    fn preserves_formatting_and_only_changes_the_token() {
        let src = "{\n  \"version\":   \"0.1.0\",\n  \"x\": 1\n}\n";
        let out = sub_once(json_version_re(), "2.0.0", src, PACKAGE_JSON).unwrap();
        assert_eq!(out, "{\n  \"version\":   \"2.0.0\",\n  \"x\": 1\n}\n");
    }

    #[test]
    fn missing_token_is_an_error() {
        let err = sub_once(json_version_re(), "0.2.0", "{\"name\":\"x\"}", PACKAGE_JSON);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("could not locate"));
    }

    #[test]
    fn set_version_round_trips_all_three_stamps() {
        let dir = std::env::temp_dir().join(format!("xtask-bump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, content: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        };
        write(PACKAGE_JSON, "{\n  \"version\": \"0.1.0\"\n}\n");
        write(TAURI_CONF, "{\n  \"version\": \"0.1.0\"\n}\n");
        write(
            CARGO_TOML,
            "[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\nfoo = { version = \"1.0.0\" }\n",
        );

        set_version(&dir, "0.2.0").unwrap();

        let stamps = read_stamps(&dir).unwrap();
        assert!(stamps.iter().all(|(_, v)| v == "0.2.0"));
        // Dependency pin preserved through the workspace-package-scoped edit.
        let cargo = std::fs::read_to_string(dir.join(CARGO_TOML)).unwrap();
        assert!(cargo.contains("foo = { version = \"1.0.0\" }"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
