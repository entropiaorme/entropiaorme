//! Thin git wrapper shared by the guards that inspect history.
//!
//! Every guard that reads a diff or commit range shells out to git through these
//! helpers, mirroring the Python originals' `subprocess.run(["git", ...])` calls.
//! Output is decoded as UTF-8 (lossily): git emits diff content as UTF-8 and the
//! authoring lint inspects non-ASCII characters (the em dash is the whole point),
//! so a lossy UTF-8 decode is required rather than the platform default.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run a git command under `repo_root`, returning stdout on success.
///
/// Fails (Err) when git cannot be spawned or exits non-zero, so a caller that
/// needs the output can fail-closed rather than treat a git error as empty.
pub fn run(args: &[&str], repo_root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("failed to run git {args:?}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {args:?} exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Resolve the repository root (the working tree's top level).
///
/// The Python guards take the repo root as `Path(__file__).resolve().parents[2]`
/// (the directory containing `backend/`); here it is resolved from git so the
/// binary works regardless of where in the tree it is launched from.
pub fn repo_root() -> Result<PathBuf, String> {
    let out = run(&["rev-parse", "--show-toplevel"], Path::new("."))?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err("git rev-parse --show-toplevel returned nothing".to_string());
    }
    Ok(PathBuf::from(trimmed))
}
