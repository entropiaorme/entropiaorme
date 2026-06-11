//! Application data-directory resolution, mirroring the backend's own
//! rules so both arms of the hybrid read and write the same files.
//!
//! The backend resolves its data dir in `backend/main.py`: an
//! `ENTROPIAORME_DATA_DIR` override wins outside frozen builds
//! (absolute as given, relative against the project root); a frozen
//! build uses `%APPDATA%\EntropiaOrme\backend` (home as the fallback
//! root); otherwise the project-root `data/` directory. The shell calls
//! the pure function here with its own notion of those inputs, and the
//! composition root only ever opens the database at the resolved
//! location (the S4 obligation: the path comes from this resolution,
//! never from a caller-supplied string).

use std::path::{Path, PathBuf};

/// The application database's file name within the data dir.
pub const DB_FILE_NAME: &str = "entropia_orme.db";

/// Resolve the data directory from explicitly-passed inputs.
///
/// `override_value` is the `ENTROPIAORME_DATA_DIR` value (when set and
/// non-blank); `project_root` anchors a relative override and the
/// non-frozen default; `frozen` selects the installed-build rule;
/// `appdata_root` is `%APPDATA%` (or the home-directory fallback).
pub fn resolve_data_dir(
    override_value: Option<&str>,
    project_root: &Path,
    frozen: bool,
    appdata_root: &Path,
) -> PathBuf {
    let trimmed = override_value.map(str::trim).filter(|v| !v.is_empty());
    if let Some(value) = trimmed {
        if !frozen {
            let candidate = PathBuf::from(value);
            return if candidate.is_absolute() {
                candidate
            } else {
                project_root.join(candidate)
            };
        }
    }
    if frozen {
        return appdata_root.join("EntropiaOrme").join("backend");
    }
    project_root.join("data")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\repo")
        } else {
            PathBuf::from("/repo")
        }
    }

    fn appdata() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\Users\\u\\AppData\\Roaming")
        } else {
            PathBuf::from("/home/u/.config")
        }
    }

    #[test]
    fn absolute_override_wins_outside_frozen() {
        let abs = if cfg!(windows) {
            "D:\\elsewhere"
        } else {
            "/elsewhere"
        };
        let resolved = resolve_data_dir(Some(abs), &root(), false, &appdata());
        assert_eq!(resolved, PathBuf::from(abs));
    }

    #[test]
    fn relative_override_anchors_to_the_project_root() {
        let resolved = resolve_data_dir(Some("custom"), &root(), false, &appdata());
        assert_eq!(resolved, root().join("custom"));
    }

    #[test]
    fn blank_override_is_no_override() {
        let resolved = resolve_data_dir(Some("   "), &root(), false, &appdata());
        assert_eq!(resolved, root().join("data"));
    }

    #[test]
    fn frozen_ignores_the_override_and_uses_appdata() {
        let resolved = resolve_data_dir(Some("custom"), &root(), true, &appdata());
        assert_eq!(resolved, appdata().join("EntropiaOrme").join("backend"));
    }

    #[test]
    fn default_is_project_root_data() {
        let resolved = resolve_data_dir(None, &root(), false, &appdata());
        assert_eq!(resolved, root().join("data"));
    }
}
