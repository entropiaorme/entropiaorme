//! Port of the original Python implementation.
//!
//! Asserts the app version is identical across the three stamps a release bump
//! must move in lock-step:
//!   - `frontend/package.json` (`version`),
//!   - `frontend/src-tauri/Cargo.toml` (`[workspace.package] version`),
//!   - `frontend/src-tauri/entropia-orme/tauri.conf.json` (`version`).
//!
//! `CURRENT_TOS_VERSION` in `frontend/src/lib/tos.ts` is deliberately NOT part
//! of this check: it versions the terms-of-service document, a separate
//! namespace from the application release.

use std::collections::BTreeSet;
use std::path::Path;

use crate::git;

const PACKAGE_JSON: &str = "frontend/package.json";
const CARGO_TOML: &str = "frontend/src-tauri/Cargo.toml";
const TAURI_CONF: &str = "frontend/src-tauri/entropia-orme/tauri.conf.json";

/// Read the `version` string from a JSON file's top-level object.
fn read_json_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("check-version-stamps: cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("check-version-stamps: cannot parse {}: {e}", path.display()))?;
    match value.get("version") {
        Some(serde_json::Value::String(s)) => Ok(s.clone()),
        // Match Python's str(data["version"]): a non-string version stringifies.
        Some(other) => Ok(other.to_string()),
        None => Err(format!(
            "check-version-stamps: {} has no top-level 'version' key",
            path.display()
        )),
    }
}

/// Read `[workspace.package] version` from the Cargo workspace manifest.
///
/// A focused scan rather than a full TOML parse: the manifest's single
/// version source is the `version = "..."` line inside the `[workspace.package]`
/// table, which members inherit via `version.workspace = true`.
fn read_cargo_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("check-version-stamps: cannot read {}: {e}", path.display()))?;
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
        "check-version-stamps: {} has no [workspace.package] version",
        path.display()
    ))
}

/// The three stamps in declaration order, each mapped to its version string.
fn read_stamps(repo_root: &Path) -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![
        (
            PACKAGE_JSON,
            read_json_version(&repo_root.join(PACKAGE_JSON))?,
        ),
        (CARGO_TOML, read_cargo_version(&repo_root.join(CARGO_TOML))?),
        (TAURI_CONF, read_json_version(&repo_root.join(TAURI_CONF))?),
    ])
}

pub fn run(_args: &[String]) -> Result<i32, String> {
    let repo_root = git::repo_root()?;
    let stamps = read_stamps(&repo_root)?;
    let versions: BTreeSet<&str> = stamps.iter().map(|(_, v)| v.as_str()).collect();

    if versions.len() == 1 {
        let version = versions.iter().next().unwrap();
        println!("check-version-stamps: all app version stamps agree at {version}.");
        for (path, _) in &stamps {
            println!("  {path}");
        }
        return Ok(0);
    }

    eprintln!(
        "check-version-stamps: the application version stamps disagree. A \
release bump must update all of them in lock-step:\n"
    );
    for (path, version) in &stamps {
        eprintln!("  {version}\t{path}");
    }
    eprintln!(
        "\nUpdate every stamp to the same version. (CURRENT_TOS_VERSION in \
frontend/src/lib/tos.ts is a separate namespace and is intentionally \
not part of this check.)"
    );
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn reads_cargo_workspace_package_version() {
        let dir = std::env::temp_dir().join(format!("xtask-vs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            "Cargo.toml",
            "[workspace]\nmembers = [\"a\"]\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        );
        let v = read_cargo_version(&dir.join("Cargo.toml")).unwrap();
        assert_eq!(v, "1.2.3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_member_version_outside_workspace_package() {
        // A `version = "..."` under [package] must not be picked up; only the
        // [workspace.package] table is the source.
        let dir = std::env::temp_dir().join(format!("xtask-vs2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            "Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"9.9.9\"\n\n[workspace.package]\nversion = \"0.2.0\"\n",
        );
        let v = read_cargo_version(&dir.join("Cargo.toml")).unwrap();
        assert_eq!(v, "0.2.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_json_version() {
        let dir = std::env::temp_dir().join(format!("xtask-vs3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            "package.json",
            "{\"name\":\"x\",\"version\":\"0.2.0\"}",
        );
        let v = read_json_version(&dir.join("package.json")).unwrap();
        assert_eq!(v, "0.2.0");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
