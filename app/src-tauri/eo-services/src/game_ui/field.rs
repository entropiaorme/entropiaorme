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
use crate::skill_panel::BgrImage;

use super::geometry::{crop, Rect};
use super::text::{run_below, run_right_of, text_runs, TextProfile, TextRun};
use super::validate::{parse, Grammar, Parsed};

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
pub struct ReadProfile {
    /// Margin added around a run before it is recognised, as a
    /// fraction of the run's height.
    pub crop_margin: f32,
    /// Lowest similarity (0 to 100) at which a run reads as a label.
    pub label_score: f64,
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

/// Recognise one run, cropped with the profile's margin.
pub fn read_run(
    image: &BgrImage,
    run: Rect,
    read: &ReadProfile,
    recogniser: &dyn Recogniser,
) -> Option<(String, f64)> {
    let margin = ((run.height() as f32 * read.crop_margin).round() as i32).max(2);
    recogniser.read(&crop(image, run.padded(margin)))
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
    for run in runs {
        // A label is about as long as its own text: skip runs that
        // could not hold it before spending a recognition on them.
        let aspect = run.rect.width() as f32 / run.rect.height() as f32;
        if aspect < want.len() as f32 * 0.3 || aspect > want.len() as f32 * 2.5 + 4.0 {
            continue;
        }
        let (text, _) =
            read_run(image, run.rect, read, recogniser).ok_or(FieldError::NoRecogniser)?;
        let score = ratio(&label_key(&text), &want);
        if score >= read.label_score && best.is_none_or(|(_, s)| score > s) {
            best = Some((run, score));
        }
    }
    Ok(best.map(|(run, _)| run))
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
        ValueSide::Below => run_below(label, runs),
    }
    .ok_or(FieldError::ValueNotFound)?;
    let (raw, confidence) =
        read_run(image, value.rect, read, recogniser).ok_or(FieldError::NoRecogniser)?;
    let parsed = parse(field.grammar, &raw).ok_or(FieldError::Invalid { raw: raw.clone() })?;
    Ok(FieldRead {
        parsed,
        raw,
        confidence,
        label_rect: label.rect,
        value_rect: value.rect,
    })
}
