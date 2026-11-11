//! The seams a reader of the live game is composed over.
//!
//! A reader never reaches for the screen or the recogniser itself: the
//! composition root hands it closures, and a test hands it stand-ins
//! that draw their own pixels. Each seam is declared here once, so
//! every reader that needs one takes the same shape.

use std::sync::Arc;

use crate::game_ui::geometry::{BgrImage, Rect};

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

/// Why no frame of the game window could be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameUnavailable {
    /// No game client window is showing, or it has no area to capture
    /// (it is minimised, say). The player can act on this: start the
    /// game, or bring its window back.
    #[error("the game window was not found")]
    WindowNotFound,
    /// The game window is showing but lies wholly outside what the
    /// capturer can see: on another monitor than the one shared for
    /// capture, or entirely off the screen. The player can act on this
    /// too: move the window onto the captured screen.
    #[error("the game window is outside the area that can be captured")]
    OutsideCapturableArea,
    /// Some of the window is within the capturable area but its pixels
    /// could not be had: the platform's capturer failed, or this host
    /// has none.
    #[error("the game window could not be captured")]
    CaptureFailed,
    /// Screen capture could not be started: on a desktop that asks the
    /// player's consent before an application may see the screen, the
    /// consent was declined or not given. Asking again puts the question
    /// to the player again, so a reader that captures on its own schedule
    /// stops here instead of retrying.
    #[error("screen capture was not allowed")]
    CaptureNotStarted,
}

/// Which part of the game window to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameArea {
    /// The whole client area.
    Whole,
    /// A rectangle of it, in the client area's own coordinates: a reader
    /// that knows where its element was last seen captures only there.
    Part(Rect),
}

/// One capture of the game window: its pixels, and where they were taken
/// from, both on the screen and within the game window. A reader finds
/// things in the image's own coordinates; these place them back, and
/// they are taken with the pixels, so they can never belong to another
/// capture.
#[derive(Debug, Clone, PartialEq)]
pub struct GameFrame {
    pub image: BgrImage,
    /// The screen position of the image's top-left pixel: what places an
    /// overlay drawn over something found in the image.
    pub origin: (i64, i64),
    /// The image's top-left pixel within the game window's client area:
    /// what a later capture of only part of the window is asked for in.
    pub offset: (i64, i64),
}

impl GameFrame {
    /// A capture of the whole client area, which sits at `origin` on the
    /// screen.
    pub fn whole(image: BgrImage, origin: (i64, i64)) -> Self {
        Self {
            image,
            origin,
            offset: (0, 0),
        }
    }
}

/// The game window as one frame: the client area of the game's window
/// (everything the game draws, without the desktop's title bar or
/// border), captured as a BGR image at the moment of the call. Where
/// only part of the client area can be captured (the window hangs off
/// the screen, or over the edge of the monitor shared for capture), the
/// frame is that part; the client's windows cut by its edge are found
/// as such.
///
/// A reader takes this rather than a rectangle of the screen because
/// the client's windows can sit anywhere in the game window at any
/// size; the reader finds what it wants in the frame itself.
///
/// The ways of getting nothing are told apart by the error:
/// [`FrameUnavailable::WindowNotFound`] when there is no game window to
/// capture, [`FrameUnavailable::CaptureNotStarted`] when screen capture
/// was not allowed to start, [`FrameUnavailable::OutsideCapturableArea`]
/// when none of the window can be captured, and
/// [`FrameUnavailable::CaptureFailed`] when some of it can but its pixels
/// could not be had.
///
/// Asked for [`FrameArea::Part`], the frame is that rectangle of the
/// client area as far as it can be captured.
pub type GameWindowFrame =
    Arc<dyn Fn(FrameArea) -> Result<GameFrame, FrameUnavailable> + Send + Sync>;
