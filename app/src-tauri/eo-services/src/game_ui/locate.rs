//! Finding a particular window, and what of it can be read.
//!
//! [`find_windows`](super::window::find_windows) recovers rectangles
//! without knowing what they are. A window is told from the others by
//! its title, which the client prints in the bar along its top. Text
//! inside a window is only that window's where no other window lies in
//! front of it, so a located window carries the rectangles in front of
//! it and its runs are filtered through them.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::fuzzy_match::ratio;
use crate::game_ui::geometry::BgrImage;

use super::field::{
    label_key, read_field_in, read_run, FieldError, FieldProfile, FieldRead, FieldScale, Recogniser,
};
use super::geometry::{Pixels, Rect};
use super::profile::{claiming, ScaleError, UiProfile};
use super::sale_queue::SaleQueueProfile;
use super::skills::SkillsTableProfile;
use super::text::{text_runs, TextRun};
use super::window::{find_windows, marker_area, title_marker, FoundWindow, Side};

/// One kind of client window, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowProfile {
    /// The title as the client prints it.
    pub title: String,
    /// The labelled fields this window carries.
    #[serde(default)]
    pub fields: BTreeMap<String, FieldProfile>,
    /// The skills table, for the window that shows one.
    #[serde(default)]
    pub skills_table: Option<SkillsTableProfile>,
    /// The sell queue, for the auction's sell window.
    #[serde(default)]
    pub sale_queue: Option<SaleQueueProfile>,
}

/// Where titles are looked for and how strictly they must match.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleProfile {
    /// Height of the strip along a window's top that holds its title.
    pub strip_height: i32,
    /// Lowest similarity (0 to 100) at which a run reads as a title.
    pub score: f64,
}

/// Why a read produced nothing. Every variant is a refusal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReadError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("the profile describes no window or field called {0:?}")]
    NotInProfile(String),
    #[error("the window is not open, or its title is covered")]
    WindowNotFound,
    #[error(transparent)]
    Field(FieldError),
}

impl From<FieldError> for ReadError {
    fn from(error: FieldError) -> Self {
        match error {
            FieldError::NoRecogniser => ReadError::NoRecogniser,
            other => ReadError::Field(other),
        }
    }
}

/// A window found and identified.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub window: FoundWindow,
    /// Rectangles of the windows drawn in front of this one.
    pub in_front: Vec<Rect>,
}

impl Located {
    /// The window's runs of text that no nearer window covers.
    pub fn runs(&self, image: &BgrImage, profile: &UiProfile) -> Vec<TextRun> {
        text_runs(image, self.window.rect, &profile.text)
            .into_iter()
            .filter(|run| {
                !self
                    .in_front
                    .iter()
                    .any(|front| front.intersect(&run.rect).is_some())
            })
            .collect()
    }

    /// The height in pixels of the window's title marker, which the
    /// client sizes with the interface scale: what tells the scale a
    /// window's labelled fields are drawn at. Unmeasured when the marker's
    /// box is not wholly in the frame, or a window in front reaches into
    /// it: either could cut the marker short. Unmeasured too when the
    /// marker runs to an edge of its box or is not the marker's shape, and
    /// for a window the desktop scaled: its lettering keeps the proportions
    /// of the scale it was drawn at, whatever its marker now measures.
    pub fn marker_height(&self, image: &BgrImage, profile: &UiProfile) -> Result<i32, ScaleError> {
        let window = &self.window;
        if window.scaled_frame || window.top != Side::Seen || window.left == Side::BeyondFrame {
            return Err(ScaleError::Unmeasured);
        }
        let corner = (window.rect.x0, window.rect.y0);
        let area = marker_area(corner, &profile.chrome.marker);
        let frame = Rect::new(0, 0, image.width() - 1, image.height() - 1);
        if area.intersect(&frame) != Some(area)
            || self.in_front.iter().any(|f| f.intersect(&area).is_some())
        {
            return Err(ScaleError::Unmeasured);
        }
        title_marker(image, corner, &profile.chrome.marker)
            .map(|marker| marker.height())
            .ok_or(ScaleError::Unmeasured)
    }

    /// The labelled fields' scale entry for this window: the one entry
    /// claiming its title marker's height, with its index, decided from
    /// the window's frame before anything in it is read.
    pub fn field_scale<'p>(
        &self,
        image: &BgrImage,
        profile: &'p UiProfile,
    ) -> Result<(usize, &'p FieldScale), ScaleError> {
        let height = self.marker_height(image, profile)?;
        claiming(&profile.read.field_scales, height, |s| s.marker_height)
    }
}

/// What the runs in a found window's title strip read as, left to right
/// and top to bottom, leaving out any a nearer window covers. Empty for a
/// window missing its top-left corner, where the title is: it has no
/// readable title.
fn title_readings(
    image: &BgrImage,
    window: &FoundWindow,
    in_front: &[Rect],
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<Vec<(Rect, String)>, ReadError> {
    if window.top != Side::Seen || window.left == Side::BeyondFrame {
        return Ok(Vec::new());
    }
    let strip = Rect::new(
        window.rect.x0,
        window.rect.y0,
        window.rect.x1,
        (window.rect.y0 + profile.titles.strip_height).min(window.rect.y1),
    );
    let mut readings = Vec::new();
    for run in text_runs(image, strip, &profile.text) {
        if in_front
            .iter()
            .any(|front| front.intersect(&run.rect).is_some())
        {
            continue;
        }
        let Some((text, _)) = read_run(image, run.rect, &profile.read, recogniser) else {
            return Err(ReadError::NoRecogniser);
        };
        readings.push((run.rect, text));
    }
    Ok(readings)
}

/// The window kind whose title best matches any of `readings`, if one
/// reaches the floor: its key in the profile.
fn best_kind<'p>(readings: &[(Rect, String)], profile: &'p UiProfile) -> Option<&'p str> {
    let mut best: Option<(&str, f64)> = None;
    for (_, text) in readings {
        let key = label_key(text);
        for (name, kind) in &profile.windows {
            let score = ratio(&key, &label_key(&kind.title));
            if score >= profile.titles.score && best.is_none_or(|(_, s)| score > s) {
                best = Some((name.as_str(), score));
            }
        }
    }
    best.map(|(name, _)| name)
}

/// The title a found window shows, matched against the profile: the
/// key of the best-matching window kind, if any reaches the floor.
fn identify<'p>(
    image: &BgrImage,
    window: &FoundWindow,
    in_front: &[Rect],
    profile: &'p UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<Option<&'p str>, ReadError> {
    let readings = title_readings(image, window, in_front, profile, recogniser)?;
    Ok(best_kind(&readings, profile))
}

/// One window found in a frame, with what its title strip shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Identified {
    pub located: Located,
    /// The profile's key for the kind of window its title names, when
    /// the title reads as one.
    pub kind: Option<String>,
    /// Every run of the title strip that was read, with its text: what a
    /// window kind's `title` entry is written from.
    pub title: Vec<(Rect, String)>,
}

/// Every window in `image`, front to back, each identified by its title
/// as [`locate`] identifies the one it is asked for.
pub fn identify_windows(
    image: &BgrImage,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<Vec<Identified>, ReadError> {
    let mut in_front: Vec<Rect> = Vec::new();
    let mut identified = Vec::new();
    for window in find_windows(image, &profile.chrome) {
        let title = title_readings(image, &window, &in_front, profile, recogniser)?;
        let kind = best_kind(&title, profile).map(String::from);
        let rect = window.rect;
        identified.push(Identified {
            located: Located {
                window,
                in_front: in_front.clone(),
            },
            kind,
            title,
        });
        in_front.push(rect);
    }
    Ok(identified)
}

/// Find the window of kind `key` in `image`.
pub fn locate(
    image: &BgrImage,
    profile: &UiProfile,
    key: &str,
    recogniser: &dyn Recogniser,
) -> Result<Located, ReadError> {
    if !profile.windows.contains_key(key) {
        return Err(ReadError::NotInProfile(key.to_string()));
    }
    let found = find_windows(image, &profile.chrome);
    let mut in_front: Vec<Rect> = Vec::new();
    for window in found {
        if identify(image, &window, &in_front, profile, recogniser)? == Some(key) {
            return Ok(Located { window, in_front });
        }
        in_front.push(window.rect);
    }
    Err(ReadError::WindowNotFound)
}

/// Read one field of one window kind from a frame.
pub fn read_window_field(
    image: &BgrImage,
    profile: &UiProfile,
    window: &str,
    field: &str,
    recogniser: &dyn Recogniser,
) -> Result<FieldRead, ReadError> {
    let field_profile = profile
        .windows
        .get(window)
        .and_then(|w| w.fields.get(field))
        .ok_or_else(|| ReadError::NotInProfile(format!("{window}.{field}")))?;
    let located = locate(image, profile, window, recogniser)?;
    // The scale is decided from the window's frame before its figure is
    // read: never by trying each entry's numerals on the reading.
    let (_, scale) = located
        .field_scale(image, profile)
        .map_err(|why| ReadError::Field(FieldError::Scale(why)))?;
    let runs = located.runs(image, profile);
    Ok(read_field_in(
        image,
        &runs,
        field_profile,
        &profile.read,
        &scale.numerals,
        recogniser,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{blank, bundled_profile, draw_window, Lettering};

    #[test]
    fn every_window_is_identified_by_its_title_front_to_back() {
        let profile = bundled_profile();
        let mut frame = Lettering::new(blank(1400, 800, [90, 70, 60]));
        // A repair terminal behind, a window of no known kind in front of
        // it (clear of its title), and a trade terminal whose title the
        // front window covers.
        let repair = Rect::new(60, 60, 660, 400);
        let trade = Rect::new(700, 300, 1300, 700);
        let front = Rect::new(400, 250, 1000, 600);
        draw_window(&mut frame.image, repair);
        draw_window(&mut frame.image, trade);
        frame.text(Rect::new(140, 82, 360, 93), "REPAIR TERMINAL");
        frame.text(Rect::new(780, 322, 990, 333), "TRADE TERMINAL");
        draw_window(&mut frame.image, front);
        let unknown = Rect::new(480, 272, 640, 283);
        frame.text(unknown, "SOMETHING ELSE");
        let read = frame.recogniser();
        let found = identify_windows(&frame.image, &profile, &read).expect("a recogniser");
        let seen: Vec<(Rect, Option<&str>, Vec<&str>)> = found
            .iter()
            .map(|w| {
                (
                    w.located.window.rect,
                    w.kind.as_deref(),
                    w.title.iter().map(|(_, text)| text.as_str()).collect(),
                )
            })
            .collect();
        // The strip's first run is the title marker, which reads as
        // nothing; the title follows it.
        assert_eq!(seen[0], (front, None, vec!["", "SOMETHING ELSE"]));
        assert_eq!(found[0].title[1].0, unknown);
        assert!(found[0].located.in_front.is_empty());
        let behind: Vec<_> = seen[1..].iter().collect();
        assert_eq!(behind.len(), 2, "{seen:?}");
        assert!(behind.contains(&&(repair, Some("repair_terminal"), vec!["", "REPAIR TERMINAL"])));
        // The covered marker and title are not read, so the window has
        // no kind.
        assert!(behind.contains(&&(trade, None, Vec::new())));
        // Each agrees with what the one-window search finds.
        for window in &found {
            if let Some(kind) = &window.kind {
                assert_eq!(
                    locate(&frame.image, &profile, kind, &read).as_ref(),
                    Ok(&window.located)
                );
            }
        }
        assert_eq!(
            locate(&frame.image, &profile, "trade_terminal", &read),
            Err(ReadError::WindowNotFound)
        );
        // Without a recogniser nothing is identified.
        let none = |_: &BgrImage| None;
        assert_eq!(
            identify_windows(&frame.image, &profile, &none),
            Err(ReadError::NoRecogniser)
        );
    }
}
