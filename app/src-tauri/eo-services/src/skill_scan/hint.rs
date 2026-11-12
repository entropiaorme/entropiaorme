//! Why the game window, as it is right now, gives the scan nothing to
//! collect, put as the one thing the player can change.

use crate::game_ui::profile::ScaleError;
use crate::game_ui::seams::FrameUnavailable;
use crate::game_ui::skills::{Admission, SkillsError, SkillsPage};

use super::ScanStop;

/// What stops the current view from being collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanHint {
    /// No game window is showing.
    GameWindowNotFound,
    /// The game window lies wholly outside what can be captured.
    GameWindowOffScreen,
    /// The game window was found but could not be captured.
    CaptureFailed,
    /// The skills window is not open, or its title is covered.
    SkillsWindowNotOpen,
    /// Another window covers part of the skills window's table.
    SkillsWindowCovered,
    /// The skills window runs off the edge of the game window.
    SkillsWindowOffScreen,
    /// The skills window shows another tab than SKILLS.
    NotSkillsTab,
    /// The skills window is too short to show a whole page.
    WindowTooShort,
    /// The page is in view but some of it did not read.
    PageNotRead,
    /// The page belongs to another category, or another page count, than
    /// the pages already collected.
    OtherListing,
    /// The skills window is drawn at an interface size the profile has no
    /// measurements for: no page can be read until the app learns it.
    InterfaceSizeUnreadable,
}

/// What a look that had no frame comes to: a hint while the player can
/// put it right in the game, or the end of the scan when capture was not
/// allowed to start (asking again would put the question to the player
/// on every look).
pub(super) fn frame_hint(unavailable: FrameUnavailable) -> Result<ScanHint, ScanStop> {
    match unavailable {
        FrameUnavailable::WindowNotFound => Ok(ScanHint::GameWindowNotFound),
        FrameUnavailable::OutsideCapturableArea => Ok(ScanHint::GameWindowOffScreen),
        FrameUnavailable::CaptureFailed => Ok(ScanHint::CaptureFailed),
        FrameUnavailable::CaptureNotStarted => Err(ScanStop::CaptureNotAllowed),
    }
}

/// The hint shown, steadied: a hint is shown only once the same one has
/// been seen on two looks running, and goes the same way, so a view that
/// flickers between covered and clear on alternate looks neither flashes
/// on the surfaces nor announces a status change on every look.
#[derive(Debug, Default)]
pub(super) struct SteadyHint {
    shown: Option<ScanHint>,
    /// What the last look saw, when it differs from what is shown.
    pending: Option<Option<ScanHint>>,
}

impl SteadyHint {
    /// Take in what one look saw; answer what to show.
    pub(super) fn observe(&mut self, seen: Option<ScanHint>) -> Option<ScanHint> {
        if seen == self.shown {
            self.pending = None;
        } else if self.pending == Some(seen) {
            self.shown = seen;
            self.pending = None;
        } else {
            self.pending = Some(seen);
        }
        self.shown
    }

    /// The hint shown now.
    pub(super) fn shown(&self) -> Option<ScanHint> {
        self.shown
    }
}

impl From<SkillsError> for ScanHint {
    fn from(error: SkillsError) -> Self {
        match error {
            SkillsError::WindowNotFound => ScanHint::SkillsWindowNotOpen,
            SkillsError::WindowClipped => ScanHint::SkillsWindowOffScreen,
            SkillsError::NotSkillsTab => ScanHint::NotSkillsTab,
            // An edge of the window or its table hidden behind another
            // window is the usual way neither can be made out.
            SkillsError::ExtentUncertain | SkillsError::TableNotFound => {
                ScanHint::SkillsWindowCovered
            }
            // No row of the table in view to tell its scale by: the usual
            // way is another window over it.
            SkillsError::Scale(ScaleError::Unmeasured) => ScanHint::SkillsWindowCovered,
            // The scan only runs with a profile that describes the table
            // and a recogniser that always answers, so these do not arise;
            // were one to, the page simply did not read.
            SkillsError::NoRecogniser | SkillsError::NotInProfile => ScanHint::PageNotRead,
            // A table drawn at an interface scale the profile has no
            // numerals for (or lists twice) cannot be read on any look:
            // showing the page again will not help.
            SkillsError::Scale(ScaleError::NotListed { .. } | ScaleError::Ambiguous { .. }) => {
                ScanHint::InterfaceSizeUnreadable
            }
        }
    }
}

/// What a page that read is missing before it can be collected, judged
/// before it is offered to the sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PageShortfall(Option<ScanHint>);

impl PageShortfall {
    pub(super) fn of(page: &SkillsPage) -> Self {
        if page.whole {
            return Self(None);
        }
        // Rows out of view are the window's height: whatever else is
        // wrong, nothing is collected until it shows the whole page.
        let hint = if page.overflows {
            ScanHint::WindowTooShort
        } else if page.partial == 0 && !page.unread.is_empty() {
            ScanHint::PageNotRead
        } else {
            // With the whole list in view, a row cut short, or a table or
            // scrollbar that cannot be made out, is another window over it.
            ScanHint::SkillsWindowCovered
        };
        Self(Some(hint))
    }

    /// The hint for what the sweep made of the page.
    pub(super) fn hint(self, admission: Admission) -> Option<ScanHint> {
        match admission {
            Admission::Kept | Admission::Conflict => None,
            Admission::NotWhole => self.0.or(Some(ScanHint::PageNotRead)),
            Admission::NoPager | Admission::NoCategory => Some(ScanHint::PageNotRead),
            Admission::OtherListing => Some(ScanHint::OtherListing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::skills::UnreadRow;

    fn page() -> SkillsPage {
        SkillsPage {
            category: Some("ALL CATEGORIES".into()),
            total: Some(1),
            page: Some((1, 2)),
            rows: Vec::new(),
            unread: Vec::new(),
            partial: 0,
            whole: true,
            overflows: false,
        }
    }

    #[test]
    fn a_page_short_of_whole_is_put_as_what_to_change() {
        assert_eq!(PageShortfall::of(&page()).hint(Admission::Kept), None);

        let mut short = page();
        short.whole = false;
        short.overflows = true;
        short.partial = 1;
        assert_eq!(
            PageShortfall::of(&short).hint(Admission::NotWhole),
            Some(ScanHint::WindowTooShort)
        );

        let mut covered = page();
        covered.whole = false;
        covered.partial = 2;
        assert_eq!(
            PageShortfall::of(&covered).hint(Admission::NotWhole),
            Some(ScanHint::SkillsWindowCovered)
        );

        let mut misread = page();
        misread.whole = false;
        misread.unread = vec![UnreadRow {
            raw_name: Some("Aim".into()),
            raw_points: Some("1,44.77".into()),
        }];
        assert_eq!(
            PageShortfall::of(&misread).hint(Admission::NotWhole),
            Some(ScanHint::PageNotRead)
        );

        let mut obscured = page();
        obscured.whole = false;
        assert_eq!(
            PageShortfall::of(&obscured).hint(Admission::NotWhole),
            Some(ScanHint::SkillsWindowCovered)
        );
    }

    #[test]
    fn a_whole_page_the_sweep_turns_away_says_why() {
        let whole = PageShortfall::of(&page());
        assert_eq!(whole.hint(Admission::Conflict), None);
        assert_eq!(whole.hint(Admission::NoPager), Some(ScanHint::PageNotRead));
        assert_eq!(
            whole.hint(Admission::NoCategory),
            Some(ScanHint::PageNotRead)
        );
        assert_eq!(
            whole.hint(Admission::OtherListing),
            Some(ScanHint::OtherListing)
        );
    }

    #[test]
    fn each_refusal_is_put_as_what_to_change() {
        for (error, hint) in [
            (SkillsError::WindowNotFound, ScanHint::SkillsWindowNotOpen),
            (SkillsError::WindowClipped, ScanHint::SkillsWindowOffScreen),
            (SkillsError::NotSkillsTab, ScanHint::NotSkillsTab),
            (SkillsError::ExtentUncertain, ScanHint::SkillsWindowCovered),
            (SkillsError::TableNotFound, ScanHint::SkillsWindowCovered),
            (SkillsError::NoRecogniser, ScanHint::PageNotRead),
            (SkillsError::NotInProfile, ScanHint::PageNotRead),
            (
                SkillsError::Scale(ScaleError::NotListed { measured: 72 }),
                ScanHint::InterfaceSizeUnreadable,
            ),
            (
                SkillsError::Scale(ScaleError::Ambiguous { measured: 57 }),
                ScanHint::InterfaceSizeUnreadable,
            ),
            (
                SkillsError::Scale(ScaleError::Unmeasured),
                ScanHint::SkillsWindowCovered,
            ),
        ] {
            assert_eq!(ScanHint::from(error.clone()), hint, "{error:?}");
        }
        for (unavailable, hint) in [
            (
                FrameUnavailable::WindowNotFound,
                ScanHint::GameWindowNotFound,
            ),
            (
                FrameUnavailable::OutsideCapturableArea,
                ScanHint::GameWindowOffScreen,
            ),
            (FrameUnavailable::CaptureFailed, ScanHint::CaptureFailed),
        ] {
            assert_eq!(frame_hint(unavailable), Ok(hint));
        }
        assert_eq!(
            frame_hint(FrameUnavailable::CaptureNotStarted),
            Err(ScanStop::CaptureNotAllowed)
        );
    }

    #[test]
    fn a_hint_shows_and_clears_only_after_two_looks_alike() {
        let mut steady = SteadyHint::default();
        let covered = Some(ScanHint::SkillsWindowCovered);
        assert_eq!(steady.observe(covered), None);
        assert_eq!(steady.observe(covered), covered);
        // Flickering between covered and clear never moves it.
        for _ in 0..3 {
            assert_eq!(steady.observe(None), covered);
            assert_eq!(steady.observe(covered), covered);
        }
        assert_eq!(steady.observe(None), covered);
        assert_eq!(steady.observe(None), None);
        // Each look seeing a different hint shows none of them.
        let (tab, short) = (Some(ScanHint::NotSkillsTab), Some(ScanHint::WindowTooShort));
        assert_eq!(steady.observe(tab), None);
        assert_eq!(steady.observe(short), None);
        assert_eq!(steady.observe(short), short);
    }
}
