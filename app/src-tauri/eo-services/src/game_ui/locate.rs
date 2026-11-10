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
use crate::skill_panel::BgrImage;

use super::field::{
    label_key, read_field_in, read_run, FieldError, FieldProfile, FieldRead, Recogniser,
};
use super::geometry::Rect;
use super::profile::UiProfile;
use super::skills::SkillsTableProfile;
use super::text::{text_runs, TextRun};
use super::window::{find_windows, FoundWindow, Side};

/// One kind of client window, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct WindowProfile {
    /// The title as the client prints it.
    pub title: String,
    /// The labelled fields this window carries.
    #[serde(default)]
    pub fields: BTreeMap<String, FieldProfile>,
    /// The skills table, for the window that shows one.
    #[serde(default)]
    pub skills_table: Option<SkillsTableProfile>,
}

/// Where titles are looked for and how strictly they must match.
#[derive(Debug, Clone, Deserialize)]
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
    // The title is at the top-left; a window missing that corner has no
    // readable title.
    if window.top != Side::Seen || window.left == Side::BeyondFrame {
        return Ok(None);
    }
    let strip = Rect::new(
        window.rect.x0,
        window.rect.y0,
        window.rect.x1,
        (window.rect.y0 + profile.titles.strip_height).min(window.rect.y1),
    );
    let mut best: Option<(&str, f64)> = None;
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
        let key = label_key(&text);
        for (name, kind) in &profile.windows {
            let score = ratio(&key, &label_key(&kind.title));
            if score >= profile.titles.score && best.is_none_or(|(_, s)| score > s) {
                best = Some((name.as_str(), score));
            }
        }
    }
    Ok(best.map(|(name, _)| name))
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
    let runs = located.runs(image, profile);
    Ok(read_field_in(
        image,
        &runs,
        field_profile,
        &profile.read,
        recogniser,
    )?)
}
