//! The seams a reader of the live game is composed over.
//!
//! A reader never reaches for the screen or the recogniser itself: the
//! composition root hands it closures, and a test hands it stand-ins
//! that draw their own pixels. Each seam is declared here once, so
//! every reader that needs one takes the same shape.

use std::sync::Arc;

use crate::skill_panel::BgrImage;

/// A screen rectangle found at scan time, as its top-left and
/// bottom-right corners in screen coordinates; `None` when there is
/// nothing to find (the game window is absent, say).
pub type RegionLookup = Arc<dyn Fn() -> Option<([i64; 2], [i64; 2])> + Send + Sync>;

/// A screen rectangle, given as `x`, `y`, width and height in screen
/// coordinates, captured as BGR pixels; `None` when the capture fails.
pub type RegionCapture = Arc<dyn Fn(i64, i64, i64, i64) -> Option<BgrImage> + Send + Sync>;

/// One image to the recogniser's `(text, confidence)`; `None` when this
/// image could not be read: there is no recogniser, or the recogniser
/// failed on it.
pub type FrameReader = Arc<dyn Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync>;
