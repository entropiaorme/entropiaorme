//! The native-services composition root.
//!
//! Mirrors the backend's own startup composition (`backend/main.py`):
//! resolve the data directory, open the application database, load the
//! game-data snapshot, and construct the ported services over the real
//! clock. The substrate serves natively-registered routes through the
//! state composed here; when any step declines, the substrate runs
//! proxy-only and the sidecar serves everything, exactly as before the
//! first flip.
//!
//! Composition grows with the takeover: this skeleton carries the
//! hydration read surface; the remaining actors (watcher, trackers,
//! scan services, OCR with its runtime obligations) join it as their
//! routes flip.

use std::path::PathBuf;
use std::sync::Arc;

use eo_http::hydration::HydrationState;
use eo_services::clock::{Clock, RealClock};
use eo_services::db::{AdoptError, Db};
use eo_services::game_data_store::GameDataStore;
use eo_services::paths::{resolve_data_dir, DB_FILE_NAME};

/// The repository root, compiled into dev builds (the manifest dir is
/// `frontend/src-tauri/entropia-orme`). Release builds never read it.
fn dev_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// Where the user's data lives, by the backend's own rules. Release
/// builds are "frozen" in the backend's sense (the installed app);
/// dev builds honour `ENTROPIAORME_DATA_DIR` and the repo default.
fn data_dir() -> PathBuf {
    let override_value = std::env::var("ENTROPIAORME_DATA_DIR").ok();
    let frozen = !cfg!(debug_assertions);
    let appdata_root = std::env::var("APPDATA")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."));
    resolve_data_dir(
        override_value.as_deref(),
        &dev_project_root(),
        frozen,
        &appdata_root,
    )
}

/// Where the game-data snapshot lives: the bundled resource directory
/// in an installed build, the repository copy in dev.
fn snapshot_dir(resource_dir: Option<&PathBuf>) -> PathBuf {
    match resource_dir {
        Some(dir) if !cfg!(debug_assertions) => dir.join("snapshot"),
        _ => dev_project_root()
            .join("backend")
            .join("data")
            .join("snapshot"),
    }
}

/// Compose the native hydration services, or decline with a logged
/// reason. Declining is always safe: the substrate then proxies every
/// route to the sidecar.
pub async fn compose_native(resource_dir: Option<PathBuf>) -> Option<Arc<HydrationState>> {
    compose_with(data_dir(), snapshot_dir(resource_dir.as_ref())).await
}

/// Composition over already-resolved locations (separated from the
/// environment-reading resolution so the decline paths are testable).
async fn compose_with(data_dir: PathBuf, snapshot: PathBuf) -> Option<Arc<HydrationState>> {
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "[composition] data dir {} not creatable ({err}); native services stand down",
            data_dir.display()
        );
        return None;
    }
    let db_path = data_dir.join(DB_FILE_NAME);
    let db = match Db::open_adopted(&db_path).await {
        Ok(db) => db,
        Err(err @ AdoptError::Quarantined { .. }) => {
            // An existing database we cannot adopt is surfaced loudly
            // and left untouched; the sidecar (whose own migration
            // logic governs it as before) keeps serving.
            eprintln!("[composition] {err}");
            return None;
        }
        Err(err) => {
            eprintln!("[composition] database open failed ({err}); native services stand down");
            return None;
        }
    };
    let game_data = match GameDataStore::new(&snapshot) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            eprintln!(
                "[composition] game-data snapshot at {} unreadable ({err}); native services \
                 stand down",
                snapshot.display()
            );
            return None;
        }
    };
    // The store tolerates a missing directory (the backend's
    // warn-and-continue, sensible for its own embedded copy), but an
    // empty store here means the bundled resources are absent or
    // broken: serving game-data-derived responses from it would
    // silently diverge from the sidecar's embedded copy. Stand down
    // and let the proxy serve instead.
    if game_data.total_entities() == 0 {
        eprintln!(
            "[composition] game-data snapshot at {} is empty; native services stand down",
            snapshot.display()
        );
        return None;
    }
    let clock: Arc<dyn Clock> = Arc::new(RealClock::new());
    Some(Arc::new(HydrationState::new(
        db.pool().clone(),
        game_data,
        clock,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_snapshot() -> PathBuf {
        dev_project_root()
            .join("backend")
            .join("data")
            .join("snapshot")
    }

    #[tokio::test]
    async fn composes_over_a_fresh_data_dir_and_the_repo_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let composed = compose_with(dir.path().join("data"), repo_snapshot()).await;
        assert!(composed.is_some(), "fresh-dir composition succeeds");
        assert!(
            dir.path().join("data").join(DB_FILE_NAME).exists(),
            "the database file is created at the resolved location"
        );
    }

    #[tokio::test]
    async fn declines_on_a_quarantined_database_leaving_the_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join(DB_FILE_NAME);
        std::fs::write(&db_path, b"not a database").unwrap();
        let composed = compose_with(data_dir, repo_snapshot()).await;
        assert!(composed.is_none(), "quarantine declines composition");
        assert_eq!(std::fs::read(&db_path).unwrap(), b"not a database");
    }

    #[tokio::test]
    async fn declines_on_a_missing_snapshot_dir() {
        let dir = tempfile::tempdir().unwrap();
        let composed =
            compose_with(dir.path().join("data"), dir.path().join("no-such-snapshot")).await;
        assert!(composed.is_none(), "missing snapshot declines composition");
    }

    #[test]
    fn snapshot_dir_prefers_the_repo_copy_in_dev_builds() {
        let resolved = snapshot_dir(Some(&PathBuf::from("X:/resources")));
        if cfg!(debug_assertions) {
            assert_eq!(resolved, repo_snapshot());
        } else {
            assert_eq!(resolved, PathBuf::from("X:/resources").join("snapshot"));
        }
    }
}
