//! Reading one labelled field out of a window.
//!
//! The client prints a label beside every figure worth reading
//! (`REPAIR COST:`, `TOTAL AMOUNT:`). A field is found by recognising
//! the runs of text in the window until one reads as its label, then
//! taking the run that stands in the stated relation to it. Nothing
//! about where the field sits in the window is assumed, so the window
//! can be any size; and nothing about its colour, because the client
//! draws the same figure green when it is live and grey when it is not.

use serde::Deserialize;

use crate::fuzzy_match::ratio;
use crate::game_ui::geometry::BgrImage;

use super::geometry::{crop, Rect};
use super::text::{run_below, run_right_of, text_runs, TextProfile, TextRun};
use super::validate::{parse, width_agrees, Grammar, NumeralMetrics, Parsed};

/// The recognition seam: one crop to `(text, confidence)`, or `None`
/// when no recogniser is available.
pub trait Recogniser: Send + Sync {
    fn read(&self, image: &BgrImage) -> Option<(String, f64)>;
}

impl<F> Recogniser for F
where
    F: Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync,
{
    fn read(&self, image: &BgrImage) -> Option<(String, f64)> {
        self(image)
    }
}

/// A recogniser that answers each distinct crop once. A reader that
/// looks for a label and later reads the same runs for their values asks
/// for the same crops twice; recognition dominates a read's cost, so the
/// repeat is answered from memory.
pub struct Remembering<'a> {
    inner: &'a dyn Recogniser,
    /// Crops answered, bucketed by a hash of their pixels; a crop is
    /// answered from memory only when its pixels are the same.
    seen: std::sync::Mutex<RememberedCrops>,
}

type RememberedCrops = std::collections::HashMap<u64, Vec<(BgrImage, Option<(String, f64)>)>>;

impl<'a> Remembering<'a> {
    pub fn new(inner: &'a dyn Recogniser) -> Self {
        Self {
            inner,
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Recogniser for Remembering<'_> {
    fn read(&self, image: &BgrImage) -> Option<(String, f64)> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (image.w, image.h, &image.data).hash(&mut hasher);
        let key = hasher.finish();
        let remembered = self
            .seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .and_then(|bucket| bucket.iter().find(|(crop, _)| crop == image))
            .map(|(_, answer)| answer.clone());
        if let Some(answer) = remembered {
            return answer;
        }
        let answer = self.inner.read(image);
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key)
            .or_default()
            .push((image.clone(), answer.clone()));
        answer
    }
}

/// Where a value stands relative to its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSide {
    /// On the same line, to the label's right.
    RightOf,
    /// On the next line, starting at the label's left edge.
    Below,
}

/// One field, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldProfile {
    /// The label as the client prints it.
    pub label: String,
    /// Where the value stands relative to the label.
    pub value: ValueSide,
    /// The shape the value's text must have.
    pub grammar: Grammar,
}

/// How runs are cropped for the recogniser and how strictly a label
/// must match.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadProfile {
    /// Margin added around a run before it is recognised, as a
    /// fraction of the run's height,
    pub crop_margin: f32,
    /// and never less than this many pixels: a short run's share of its
    /// height would leave the recogniser no border round the glyphs.
    pub min_crop_margin: i32,
    /// A value under its label starts within this many of the label's
    /// heights of the label's left edge.
    pub below_reach: f32,
    /// Lowest similarity (0 to 100) at which a run reads as a label.
    pub label_score: f64,
    /// A run that could hold a label is this many times as wide as it is
    /// tall per character of the label, `[least, most]`, the most
    /// widened by `label_aspect_slack`: lettering is about as wide as
    /// its text is long. Runs outside it are not worth recognising.
    pub label_aspect_per_char: [f32; 2],
    pub label_aspect_slack: f32,
    /// How wide the client's numerals are, for checking a number's
    /// reading against its ink.
    pub numerals: NumeralMetrics,
}

/// Why a field could not be read. Every variant is a refusal: the
/// caller shows an empty field.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FieldError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("the label was not found in the window")]
    LabelNotFound,
    #[error("there is no value beside the label")]
    ValueNotFound,
    #[error("the value read as {raw:?}, which is not a valid value for this field")]
    Invalid { raw: String },
    #[error("the value read as {raw:?}, but the figure on screen is not that wide")]
    WrongWidth { raw: String },
}

/// One field read and validated.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRead {
    pub parsed: Parsed,
    /// The recogniser's text, before validation.
    pub raw: String,
    pub confidence: f64,
    pub label_rect: Rect,
    pub value_rect: Rect,
}

/// Letters and digits only, upper-cased: what two readings of the same
/// label have in common whatever the spacing and punctuation.
pub fn label_key(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// The margin a run of `height` is cropped with before recognition.
pub fn crop_margin(height: i32, read: &ReadProfile) -> i32 {
    ((height as f32 * read.crop_margin).round() as i32).max(read.min_crop_margin)
}

/// Recognise one run, cropped with the profile's margin.
pub fn read_run(
    image: &BgrImage,
    run: Rect,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Option<(String, f64)> {
    let margin = crop_margin(run.height(), read);
    recogniser.read(&crop(image, run.padded(margin)))
}

/// Recognise one run cropped as [`read_run`] does, with every column of
/// the crop outside `keep` (its first and last column, in the frame's
/// coordinates) painted `fill`: one word of a phrase read on its own,
/// without the margin taking in its neighbours' letters.
pub fn read_isolated(
    image: &BgrImage,
    run: Rect,
    keep: (i32, i32),
    fill: [u8; 3],
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Option<(String, f64)> {
    let area = run.padded(crop_margin(run.height(), read));
    let mut piece = crop(image, area);
    let x0 = area.x0.max(0);
    for y in 0..piece.h {
        for x in 0..piece.w {
            let fx = x0 + x as i32;
            if fx < keep.0 || fx > keep.1 {
                let i = (y * piece.w + x) * 3;
                piece.data[i..i + 3].copy_from_slice(&[fill[2], fill[1], fill[0]]);
            }
        }
    }
    recogniser.read(&piece)
}

/// Whether `run` is about as long as a label with key `want`: runs that
/// could not hold it are skipped before a recognition is spent on them.
fn could_hold(run: &TextRun, want: &[char], read: &ReadProfile) -> bool {
    let aspect = run.rect.width() as f32 / run.rect.height() as f32;
    let chars = want.len() as f32;
    aspect >= chars * read.label_aspect_per_char[0]
        && aspect <= chars * read.label_aspect_per_char[1] + read.label_aspect_slack
}

/// The run within `runs` that reads as `label`, with its score.
pub fn find_label<'a>(
    image: &BgrImage,
    runs: &'a [TextRun],
    label: &str,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Result<Option<&'a TextRun>, FieldError> {
    let want = label_key(label);
    let mut best: Option<(&TextRun, f64)> = None;
    for run in runs.iter().filter(|run| could_hold(run, &want, read)) {
        let (text, _) =
            read_run(image, run.rect, read, recogniser).ok_or(FieldError::NoRecogniser)?;
        let score = ratio(&label_key(&text), &want);
        if score >= read.label_score && best.is_none_or(|(_, s)| score > s) {
            best = Some((run, score));
        }
    }
    Ok(best.map(|(run, _)| run))
}

/// The first run in `runs` (taken in the order given) that reads as
/// `label`. For a label the client prints once, near the top of a
/// window, such as a table's column heading, runs in reading order
/// reach it after recognising only what lies above it, where
/// [`find_label`] would recognise every row of the table too.
pub fn first_label<'a>(
    image: &BgrImage,
    runs: &'a [TextRun],
    label: &str,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Result<Option<&'a TextRun>, FieldError> {
    let want = label_key(label);
    for run in runs.iter().filter(|run| could_hold(run, &want, read)) {
        let (text, _) =
            read_run(image, run.rect, read, recogniser).ok_or(FieldError::NoRecogniser)?;
        if ratio(&label_key(&text), &want) >= read.label_score {
            return Ok(Some(run));
        }
    }
    Ok(None)
}

/// Read `field` from the window at `window`.
pub fn read_field(
    image: &BgrImage,
    window: Rect,
    field: &FieldProfile,
    text: &TextProfile,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Result<FieldRead, FieldError> {
    let runs = text_runs(image, window, text);
    read_field_in(image, &runs, field, read, recogniser)
}

/// Read `field` from runs already found.
pub fn read_field_in(
    image: &BgrImage,
    runs: &[TextRun],
    field: &FieldProfile,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Result<FieldRead, FieldError> {
    let label = find_label(image, runs, &field.label, read, recogniser)?
        .ok_or(FieldError::LabelNotFound)?;
    let value = match field.value {
        ValueSide::RightOf => run_right_of(label, runs),
        ValueSide::Below => run_below(label, runs, read.below_reach),
    }
    .ok_or(FieldError::ValueNotFound)?;
    let (raw, confidence) =
        read_run(image, value.rect, read, recogniser).ok_or(FieldError::NoRecogniser)?;
    let parsed = parse(field.grammar, &raw).ok_or(FieldError::Invalid { raw: raw.clone() })?;
    if !width_agrees(image, value.rect, &parsed.text, &read.numerals) {
        return Err(FieldError::WrongWidth { raw });
    }
    Ok(FieldRead {
        parsed,
        raw,
        confidence,
        label_rect: label.rect,
        value_rect: value.rect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_isolated_word_is_read_without_its_neighbours() {
        use crate::game_ui::test_support::{blank, bundled_profile, fill};
        let read = bundled_profile().read;
        let mut image = blank(200, 40, [8, 11, 11]);
        fill(&mut image, Rect::new(20, 10, 60, 24), [200, 200, 200]);
        fill(&mut image, Rect::new(64, 10, 100, 24), [100, 200, 200]);
        let seen = std::sync::Mutex::new(Vec::new());
        let recogniser = |crop: &BgrImage| {
            let has = |rgb: [u8; 3]| crop.data.chunks(3).any(|p| p == [rgb[2], rgb[1], rgb[0]]);
            seen.lock()
                .unwrap()
                .push((has([200, 200, 200]), has([100, 200, 200])));
            Some((String::new(), 1.0))
        };
        read_isolated(
            &image,
            Rect::new(20, 10, 60, 24),
            (19, 62),
            [8, 11, 11],
            &read,
            &recogniser,
        );
        read_run(&image, Rect::new(20, 10, 60, 24), &read, &recogniser);
        assert_eq!(*seen.lock().unwrap(), vec![(true, false), (true, true)]);
    }

    #[test]
    fn a_remembered_crop_is_answered_once_and_another_crop_afresh() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let inner = |image: &BgrImage| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some((format!("{}", image.data[0]), 0.9))
        };
        let remembering = Remembering::new(&inner);
        let crop = |v: u8| BgrImage {
            data: vec![v; 12],
            h: 2,
            w: 2,
        };
        assert_eq!(remembering.read(&crop(7)).map(|r| r.0), Some("7".into()));
        assert_eq!(remembering.read(&crop(7)).map(|r| r.0), Some("7".into()));
        assert_eq!(remembering.read(&crop(9)).map(|r| r.0), Some("9".into()));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
