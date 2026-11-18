//! Kept pictures of plates whose name did not read: the instrument for
//! finding out why a mob's name is sometimes not read.
//!
//! The creature watcher keeps nothing it captures. With the player's
//! setting on, and only then, a look that found a locked plate and could
//! not read its name hands the plate's surroundings here, and they are
//! saved as a small picture named for the moment and for why the name was
//! refused. The pictures stay on this computer, in one folder of the app's
//! data, and the folder holds only the newest [`KEPT`]: a plate that goes
//! on not reading is kept once every [`SPACING_MS`], not once a look.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::game_ui::geometry::BgrImage;

/// The most pictures the folder holds; past it the oldest are removed.
pub const KEPT: usize = 200;

/// The least time between two pictures kept, in milliseconds.
pub const SPACING_MS: i64 = 5_000;

/// Where unread plates are kept, when the player has asked for it.
pub struct UnreadPlates {
    dir: PathBuf,
    /// Whether the setting is on, read live at every plate.
    keeping: Arc<dyn Fn() -> bool + Send + Sync>,
    /// When the last picture was kept.
    last_ms: Mutex<Option<i64>>,
}

impl UnreadPlates {
    pub fn new(dir: PathBuf, keeping: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            dir,
            keeping,
            last_ms: Mutex::new(None),
        }
    }

    /// Keep the plate seen at `at_ms` whose name was refused for `why`,
    /// its picture made by `picture` only if it is kept: not with the
    /// setting off, and not within [`SPACING_MS`] of the last one kept.
    /// Best-effort: a picture that cannot be written only logs.
    pub fn keep(&self, at_ms: i64, why: &str, picture: &dyn Fn() -> BgrImage) {
        if !(self.keeping)() {
            return;
        }
        {
            let mut last = self.last_ms.lock().unwrap_or_else(PoisonError::into_inner);
            if last.is_some_and(|last| (at_ms - last).abs() < SPACING_MS) {
                return;
            }
            *last = Some(at_ms);
        }
        // Zero-padded, so the names sort by time.
        let name = format!("{at_ms:015}-{why}.png");
        crate::screen_capture::write_debug_frame(&self.dir, &name, &picture());
        self.prune();
    }

    /// Remove the oldest pictures past [`KEPT`].
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut pictures: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
            .collect();
        if pictures.len() <= KEPT {
            return;
        }
        pictures.sort();
        for old in &pictures[..pictures.len() - KEPT] {
            if let Err(error) = std::fs::remove_file(old) {
                tracing::warn!(target: "eo::capture", %error, "an old unread plate could not be removed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    fn picture() -> BgrImage {
        BgrImage {
            data: vec![40; 4 * 2 * 3],
            h: 2,
            w: 4,
        }
    }

    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn nothing_is_kept_or_even_pictured_with_the_setting_off() {
        let dir = tempfile::tempdir().unwrap();
        let on = Arc::new(AtomicBool::new(false));
        let keeping = on.clone();
        let plates = UnreadPlates::new(
            dir.path().join("unread-plates"),
            Arc::new(move || keeping.load(Ordering::SeqCst)),
        );
        let made = AtomicUsize::new(0);
        let make = || {
            made.fetch_add(1, Ordering::SeqCst);
            picture()
        };
        plates.keep(1_000, "not-found", &make);
        assert_eq!(made.load(Ordering::SeqCst), 0);
        assert!(names(&dir.path().join("unread-plates")).is_empty());

        // Turned on, the next unread plate is kept, named for its moment
        // and for why it did not read.
        on.store(true, Ordering::SeqCst);
        plates.keep(2_000, "not-found", &make);
        assert_eq!(made.load(Ordering::SeqCst), 1);
        assert_eq!(
            names(&dir.path().join("unread-plates")),
            ["000000000002000-not-found.png"]
        );
    }

    #[test]
    fn a_plate_that_goes_on_not_reading_is_kept_once_a_spacing() {
        let dir = tempfile::tempdir().unwrap();
        let plates = UnreadPlates::new(dir.path().to_path_buf(), Arc::new(|| true));
        for at_ms in [10_000, 11_000, 14_999, 15_000, 16_000] {
            plates.keep(at_ms, "covered", &picture);
        }
        assert_eq!(
            names(dir.path()),
            ["000000000010000-covered.png", "000000000015000-covered.png"]
        );
    }

    #[test]
    fn only_the_newest_are_held() {
        let dir = tempfile::tempdir().unwrap();
        let plates = UnreadPlates::new(dir.path().to_path_buf(), Arc::new(|| true));
        let count = KEPT as i64 + 3;
        for n in 0..count {
            plates.keep(n * SPACING_MS, "clipped", &picture);
        }
        let held = names(dir.path());
        assert_eq!(held.len(), KEPT);
        assert_eq!(
            held[0],
            format!("{:015}-clipped.png", 3 * SPACING_MS),
            "the three oldest went"
        );
    }
}
