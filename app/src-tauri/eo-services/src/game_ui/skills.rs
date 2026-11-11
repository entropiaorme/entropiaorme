//! Reading the SKILLS tab of the skills window, and sweeping its pages.
//!
//! The tab is a paged table: one row per skill with its icon, name,
//! rank, progress bar, percentage and points. Above it sit a category
//! drop-down, the category's total and a pager. The window can be
//! dragged, resized (which changes how many rows show, how wide the
//! columns are, and truncates long names with an ellipsis), maximised,
//! scrolled by part of a row, covered by another window, or drawn at
//! another interface scale. So nothing here assumes where a row or a
//! column sits: the header's own labels name the columns, the table's
//! rule lines divide the rows, the table's scrollbar says whether the
//! whole page is in view, and a row is read only when the whole of it is.
//!
//! A row's points are read exactly or the row is reported unread. Its
//! name is a skill the vocabulary knows, a skill it does not (the game
//! adds skills before the bundled list catches up), or the client's one
//! blank row. One page cannot prove more than that, but a whole sweep
//! can: the panel's total is the sum of every counted skill's whole
//! points, and a skill missing from the sweep that a misread name comes
//! near is that name's skill.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::fuzzy_match::ratio;
use crate::game_ui::geometry::BgrImage;

use super::field::{first_label, label_key, read_run, Recogniser, Remembering};
use super::geometry::{chroma, Pixels, Rect};
use super::locate::{locate, ReadError};
use super::profile::UiProfile;
use super::table::{
    carries_across, column_rules, row_rules, rule_extent, table_shape, Band, Panel,
};
use super::text::{ink_groups, run_right_of, text_runs, TextRun};
use super::validate::{pager, parse, width_agrees, Grammar};
use super::window::Side;

/// The profile key of the skills window.
const WINDOW: &str = "skills";

/// What the SKILLS tab's table looks like, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsTableProfile {
    /// The name column's header label.
    pub name_header: String,
    /// The points column's header label.
    pub points_header: String,
    /// The label before the total, one per way the client words it
    /// (the whole list, or a single category).
    pub total_labels: Vec<String>,
    /// The drop-down's text when no category is chosen.
    pub all_categories: String,
    /// Skills the panel lists but leaves out of its total.
    pub uncounted: Vec<String>,
    /// The table's columns from left to right, by role (`name`,
    /// `points`, and the others), for placing a column whose heading is
    /// drawn over.
    pub columns: Vec<String>,
    /// A share of the row height: how far a row may fall short of the
    /// table's row height, its lettering sit off its middle, or two rows
    /// differ in height, and still count as whole, centred or alike.
    pub row_slack: f32,
    /// The header's height as a share of a row's: the client sizes both
    /// from its font, so the share holds at every interface scale.
    pub header_height_share: f32,
    /// Lettering whose middle lies within this many of a heading's
    /// heights of the heading's own edges is on the headings' line; the
    /// same multiple of the points' height bounds a row's text line.
    pub line_slack: f32,
    /// A name ending within this many of its row's text heights of its
    /// column's right edge reaches the edge, where the client cuts a name
    /// too long for the column with an ellipsis.
    pub cut_reach: f32,
    /// A gap between ink groups wider than this many text heights is not
    /// a space between words: what lies before it is not part of a name.
    pub word_gap: f32,
    /// Fewest letters and digits a name's text needs to be taken as one.
    pub min_name_length: usize,
    /// The points must stand at least this many text heights clear of
    /// their column's left divider. They are right-aligned, so lettering
    /// nearer the divider may have lost its leading digits to it.
    pub points_clearance: f32,
    /// Largest page count a pager reading is believed for.
    pub max_pages: u32,
    /// How near a misread name must be to a skill to be taken as it.
    pub near_miss: NearMissProfile,
    /// What the table's scrollbar looks like.
    pub scrollbar: ScrollbarProfile,
}

/// When a name that spells no skill is taken as a slip of one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NearMissProfile {
    /// Lowest similarity (0 to 100) of the name to a skill, on one page.
    pub score: f64,
    /// By how much the best skill must out-score the next.
    pub margin: f64,
    /// Most letters by which the two may differ in length.
    pub max_length_difference: usize,
    /// Lowest similarity at which a whole sweep takes a name as a skill
    /// the sweep has not otherwise read. Lower than `score`, because only
    /// the skills still missing compete: one substituted character in a
    /// three-letter name clears it.
    pub sweep_score: f64,
}

/// The table's scrollbar: a plain grey thumb to the right of the table,
/// drawn only when the list does not fit the view.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollbarProfile {
    /// Brightness range of the thumb.
    pub thumb_lum: [u8; 2],
    /// Largest channel spread of a thumb pixel: it is a grey.
    pub thumb_max_chroma: u8,
    /// The thumb's width as a share of the row height, `[least, most]`.
    pub thumb_width: [f32; 2],
    /// The shortest thumb, as a share of the row height.
    pub thumb_min_length: f32,
    /// Thumb-coloured pixels outside a thumb, as a share of the row
    /// height, beyond which the scrollbar cannot be made out.
    pub max_stray: f32,
    /// A thumb at least this share of its track's length fills it.
    pub fill_share: f32,
}

/// The skill names and categories a reading is checked against.
#[derive(Debug, Clone, Default)]
pub struct SkillVocabulary {
    names: Vec<String>,
    categories: Vec<String>,
}

impl SkillVocabulary {
    pub fn new(names: Vec<String>, categories: Vec<String>) -> Self {
        Self { names, categories }
    }

    /// From the bundled game data's skill entities: each one's `name`,
    /// and the distinct `category.name`s.
    pub fn from_entities(entities: &[Value]) -> Self {
        let mut names = Vec::new();
        let mut categories: Vec<String> = Vec::new();
        for entity in entities {
            if let Some(name) = entity.get("name").and_then(Value::as_str) {
                names.push(name.to_string());
            }
            if let Some(category) = entity
                .get("category")
                .and_then(|c| c.get("name"))
                .and_then(Value::as_str)
            {
                if !categories.iter().any(|c| c == category) {
                    categories.push(category.to_string());
                }
            }
        }
        Self { names, categories }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// What a row's name cell was read as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowName {
    /// A skill the vocabulary knows, as the vocabulary spells it.
    /// `near_miss` when the text did not spell it whole (a slip of a
    /// character or two, or a name the client cut short), for the caller
    /// to show for checking.
    Known { name: String, near_miss: bool },
    /// Clear text that names no skill the vocabulary knows, nor comes
    /// near one: a skill the game added after the bundled list was made.
    /// It is never given a vocabulary name on the page; a sweep may yet
    /// place it (see [`SkillsSweep::finish`]).
    Unknown,
    /// The row the client itself draws with an empty name cell. Its
    /// points are in the panel's total, so it is kept (and never matched
    /// to a skill) rather than dropped or failed.
    Blank,
}

/// One row read off the table: its points validated, its name known,
/// unknown or blank.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRow {
    pub name: RowName,
    /// The recogniser's text for the name cell, empty for a blank row.
    pub raw_name: String,
    /// The points in the client's notation, `4,763.61`.
    pub points_text: String,
    pub points: f64,
}

impl SkillRow {
    /// The vocabulary's name for the row's skill, when it has one.
    pub fn skill(&self) -> Option<&str> {
        match &self.name {
            RowName::Known { name, .. } => Some(name),
            RowName::Unknown | RowName::Blank => None,
        }
    }
}

/// A whole row in view that could not be read exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct UnreadRow {
    /// The recogniser's text for the name cell, if it had text.
    pub raw_name: Option<String>,
    /// The recogniser's text for the points cell, if it had text.
    pub raw_points: Option<String>,
}

/// One page of the SKILLS tab, as far as it could be read.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsPage {
    /// The drop-down's text, when it reads as a known category or as
    /// the all-categories entry.
    pub category: Option<String>,
    /// The total the panel prints for the category.
    pub total: Option<u64>,
    /// The pager's `(page, page count)`.
    pub page: Option<(u32, u32)>,
    /// Rows whose points read, top to bottom: known skills, skills the
    /// vocabulary does not know, and the blank row.
    pub rows: Vec<SkillRow>,
    /// Whole rows in view whose points did not validate or may have been
    /// cut, or whose name could be more than one skill or could not be
    /// made out.
    pub unread: Vec<UnreadRow>,
    /// Rows only partly in view: scrolled under the header, cut by the
    /// bottom of the view, or covered by another window.
    pub partial: usize,
    /// Every row of the page is in this reading: the table's scrollbar
    /// shows the whole list in view (no thumb, or a thumb that fills its
    /// track), no row is scrolled under the header, cut or covered, and
    /// none went unread. A row naming a skill the vocabulary does not
    /// know is read, so it does not stop a page being whole.
    pub whole: bool,
    /// The table's scrollbar shows rows out of view: the window is too
    /// short to show the whole page at once, however the rows in view
    /// read.
    pub overflows: bool,
}

/// Why no page could be read. Every variant is a refusal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SkillsError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("the profile does not describe the skills table")]
    NotInProfile,
    #[error("the skills window is not open, or its title is covered or off the screen")]
    WindowNotFound,
    #[error("the skills window's extent is not certain (a side is covered, or the desktop scaled the picture)")]
    ExtentUncertain,
    #[error("the skills window runs off the screen")]
    WindowClipped,
    #[error("the table's header is not the SKILLS tab's (another tab is showing, or the header is hidden)")]
    NotSkillsTab,
    #[error("the table's rule lines could not be found")]
    TableNotFound,
}

impl From<ReadError> for SkillsError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::NoRecogniser => SkillsError::NoRecogniser,
            ReadError::NotInProfile(_) => SkillsError::NotInProfile,
            ReadError::WindowNotFound => SkillsError::WindowNotFound,
            ReadError::Field(_) => SkillsError::TableNotFound,
        }
    }
}

/// The recogniser renders the client's ellipsis as one to three dots or
/// as the ellipsis character: the stem before it, and whether there was
/// one.
fn strip_ellipsis(text: &str) -> (&str, bool) {
    let stem = text.trim_end_matches(['.', '\u{2026}', '\u{3002}', '\u{b7}']);
    (stem.trim_end(), stem.len() < text.len())
}

/// What a name cell's text says about its skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameMatch {
    /// It spells this skill whole.
    Exact(String),
    /// It is taken as this skill without spelling it whole: a slip from
    /// it near no other skill, or a name cut to fit that only it begins.
    NearMiss(String),
    /// It is clear text near no skill the vocabulary knows.
    Unknown,
    /// It could be more than one skill, or is not text a name could be:
    /// the row is unread.
    Unclear,
}

/// The skills whose names a key comes within the near-miss length of,
/// scored by similarity, best first.
fn scored<'v>(
    key: &[char],
    names: impl Iterator<Item = &'v String>,
    near: &NearMissProfile,
) -> Vec<(&'v str, f64)> {
    let mut scored: Vec<(&str, f64)> = names
        .filter_map(|name| {
            let name_key = label_key(name);
            (name_key.len().abs_diff(key.len()) <= near.max_length_difference)
                .then(|| (name.as_str(), ratio(key, &name_key)))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}

/// What the best of some scored candidates comes to.
enum Best<'v> {
    /// One candidate at or above the floor, clear of the next.
    One(&'v str),
    /// None reaches the floor.
    None,
    /// Two reach it within the margin of each other.
    Tie,
}

fn single_best<'v>(scored: &[(&'v str, f64)], floor: f64, margin: f64) -> Best<'v> {
    match (scored.first(), scored.get(1)) {
        (Some(&(name, score)), next) if score >= floor => {
            if next.is_none_or(|n| score - n.1 >= margin) {
                Best::One(name)
            } else {
                Best::Tie
            }
        }
        _ => Best::None,
    }
}

/// Resolve a name cell's text to the skill it names. `cut_to_fit` says
/// whether the name runs to the edge of its column, the only place the
/// client cuts one short with an ellipsis.
///
/// The text spells a skill when it matches the skill's name up to case,
/// spacing and punctuation. No skill's name ends in a full stop, so
/// trailing dots are an ellipsis where the name reaches its column's
/// edge and a misreading anywhere else. Behind an ellipsis, a stem that
/// is itself a skill's whole name is that skill (the dots were a
/// misreading, and must not turn it into a longer skill that begins the
/// same way); otherwise the stem must begin exactly one skill's name.
/// Either is a near miss, never exact, since the name was not read whole.
/// Text at the edge without an ellipsis that spells no skill whole is
/// unclear: its ellipsis may simply not have read.
///
/// Text that spells no skill is either a skill the game added after the
/// bundled list was made, or a known skill misread by a character or
/// two. The second must not pass as the first, or that skill would
/// silently miss its update, so before text is called unknown it is
/// compared with every skill of about its length: one at or above the
/// profile's similarity and clear of the next by its margin is taken as
/// a near miss; more than one is unclear. The floor is set high, because
/// a skill the list lacks must not become its nearest neighbour; a
/// sweep, which knows which skills it has already read, looks again
/// with a lower one.
pub fn resolve_name(
    raw: &str,
    vocabulary: &SkillVocabulary,
    cut_to_fit: bool,
    table: &SkillsTableProfile,
) -> NameMatch {
    let near = &table.near_miss;
    let (stem, dotted) = strip_ellipsis(raw.trim());
    let key = label_key(stem);
    if key.len() < table.min_name_length || (dotted && !cut_to_fit) {
        return NameMatch::Unclear;
    }
    let keys: Vec<(&str, Vec<char>)> = vocabulary
        .names
        .iter()
        .map(|name| (name.as_str(), label_key(name)))
        .collect();
    let whole: Vec<&str> = keys
        .iter()
        .filter(|(_, k)| *k == key)
        .map(|(n, _)| *n)
        .collect();
    match (whole.as_slice(), dotted) {
        ([name], false) => return NameMatch::Exact(name.to_string()),
        ([name], true) => return NameMatch::NearMiss(name.to_string()),
        ([_, _, ..], _) => return NameMatch::Unclear,
        ([], _) => {}
    }
    // A name at its column's edge that spells no skill whole may be one
    // the client cut whose ellipsis did not read: it cannot be told from
    // a skill the list lacks, so it is not taken as one.
    if cut_to_fit && !dotted {
        return NameMatch::Unclear;
    }
    if dotted {
        let mut begun = keys
            .iter()
            .filter(|(_, k)| k.len() > key.len() && k.starts_with(&key));
        return match (begun.next(), begun.next()) {
            (Some((name, _)), None) => NameMatch::NearMiss(name.to_string()),
            (Some(_), Some(_)) => NameMatch::Unclear,
            _ => NameMatch::Unknown,
        };
    }
    match single_best(
        &scored(&key, vocabulary.names.iter(), near),
        near.score,
        near.margin,
    ) {
        Best::One(name) => NameMatch::NearMiss(name.to_string()),
        Best::None => NameMatch::Unknown,
        Best::Tie => NameMatch::Unclear,
    }
}

/// The skill whose name a text comes nearest, with its similarity (0 to
/// 100): what a caller can show beside a skill the vocabulary lacks.
pub fn nearest_skill(raw: &str, vocabulary: &SkillVocabulary) -> Option<(String, f64)> {
    let (stem, _) = strip_ellipsis(raw.trim());
    let key = label_key(stem);
    vocabulary
        .names
        .iter()
        .map(|name| (name.clone(), ratio(&key, &label_key(name))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// Resolve the drop-down's text to the category it names.
pub fn resolve_category(
    raw: &str,
    vocabulary: &SkillVocabulary,
    table: &SkillsTableProfile,
) -> Option<String> {
    let key = label_key(raw);
    if key.is_empty() {
        return None;
    }
    std::iter::once(&table.all_categories)
        .chain(vocabulary.categories.iter())
        .find(|category| label_key(category) == key)
        .cloned()
}

/// What a run of text says about the total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalText {
    /// The label and a valid value, in one run.
    Value(u64),
    /// The label alone: the value is the next run along.
    LabelOnly,
}

/// Read a run's text as the total's label, with or without its value.
/// The label may be matched loosely (it is only an anchor); the value
/// must be digits and nothing else.
pub fn parse_total(text: &str, labels: &[String], label_score: f64) -> Option<TotalText> {
    let text = text.trim();
    let digits_from = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_whitespace() || parse(Grammar::Integer, &c.to_string()).is_some())
        .last()
        .map_or(text.len(), |(i, _)| i);
    let (label, value) = text.split_at(digits_from);
    let key = label_key(label);
    let matched = labels
        .iter()
        .any(|l| ratio(&key, &label_key(l)) >= label_score);
    if !matched {
        return None;
    }
    if value.trim().is_empty() {
        return Some(TotalText::LabelOnly);
    }
    parse_integer(value).map(TotalText::Value)
}

fn parse_integer(text: &str) -> Option<u64> {
    let parsed = parse(Grammar::Integer, text)?;
    parsed.text.parse().ok()
}

/// What the table's scrollbar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scrollbar {
    /// No thumb: the whole list fits the view.
    NoThumb,
    /// A thumb spanning these pixel rows.
    Thumb { top: i32, bottom: i32 },
    /// The strip it is drawn in is cut, or holds something other than
    /// one thumb.
    Unknown,
}

/// Look for the scrollbar's thumb in `strip`, the space between the
/// table's right edge and the window's, alongside the view.
pub(crate) fn scrollbar<P: Pixels>(
    image: &P,
    strip: Rect,
    pitch: i32,
    profile: &ScrollbarProfile,
) -> Scrollbar {
    let pitch = pitch as f32;
    let min_width = (profile.thumb_width[0] * pitch).round() as i32;
    let max_width = (profile.thumb_width[1] * pitch).round() as i32;
    if strip.width() < min_width
        || strip.x0 < 0
        || strip.y0 < 0
        || strip.x1 >= image.width()
        || strip.y1 >= image.height()
    {
        return Scrollbar::Unknown;
    }
    let thumb_pixel = |x: i32, y: i32| {
        let l = image.lum(x, y);
        l >= profile.thumb_lum[0]
            && l <= profile.thumb_lum[1]
            && chroma(image.rgb(x, y)) <= profile.thumb_max_chroma
    };
    let min_length = (profile.thumb_min_length * pitch).round() as i32;
    // Each column's longest upright run of thumb-coloured pixels, as
    // `(x, first row, last row)`, where it is long enough to be a thumb.
    let mut columns: Vec<(i32, i32, i32)> = Vec::new();
    let mut coloured = 0;
    for x in strip.x0..=strip.x1 {
        let mut best: Option<(i32, i32)> = None;
        let mut run_start: Option<i32> = None;
        for y in strip.y0..=strip.y1 + 1 {
            let on = y <= strip.y1 && thumb_pixel(x, y);
            coloured += i32::from(on);
            match (on, run_start) {
                (true, None) => run_start = Some(y),
                (false, Some(start)) => {
                    if best.is_none_or(|(s, e)| y - 1 - start > e - s) {
                        best = Some((start, y - 1));
                    }
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some((start, end)) = best.filter(|(s, e)| e - s + 1 >= min_length) {
            columns.push((x, start, end));
        }
    }
    let (Some(&(first, _, _)), Some(&(last, _, _))) = (columns.first(), columns.last()) else {
        return if coloured as f32 <= profile.max_stray * pitch {
            Scrollbar::NoThumb
        } else {
            Scrollbar::Unknown
        };
    };
    let width = columns.len() as i32;
    if last - first + 1 != width || width < min_width || width > max_width {
        return Scrollbar::Unknown;
    }
    let top = columns.iter().map(|c| c.1).min().unwrap_or(strip.y0);
    let bottom = columns.iter().map(|c| c.2).max().unwrap_or(strip.y1);
    let thumb_pixels: i32 = columns.iter().map(|c| c.2 - c.1 + 1).sum();
    if (coloured - thumb_pixels) as f32 > profile.max_stray * pitch {
        return Scrollbar::Unknown;
    }
    Scrollbar::Thumb { top, bottom }
}

/// The smallest rectangle holding every run.
fn union(runs: &[&TextRun]) -> Option<Rect> {
    runs.iter().map(|r| r.rect).reduce(|a, b| {
        Rect::new(
            a.x0.min(b.x0),
            a.y0.min(b.y0),
            a.x1.max(b.x1),
            a.y1.max(b.y1),
        )
    })
}

fn centre_y(rect: &Rect) -> i32 {
    (rect.y0 + rect.y1) / 2
}

/// Read the SKILLS tab's current page from a frame.
pub fn read_skills_page(
    image: &BgrImage,
    profile: &UiProfile,
    vocabulary: &SkillVocabulary,
    recogniser: &dyn Recogniser,
) -> Result<SkillsPage, SkillsError> {
    let table = profile
        .windows
        .get(WINDOW)
        .and_then(|w| w.skills_table.as_ref())
        .ok_or(SkillsError::NotInProfile)?;
    // The title, the headings and the bar above the table are each
    // recognised while searching and again while reading.
    let remembering = Remembering::new(recogniser);
    let recogniser: &dyn Recogniser = &remembering;
    let located = locate(image, profile, WINDOW, recogniser)?;
    let window = &located.window;
    if !window.extent_certain() {
        return Err(SkillsError::ExtentUncertain);
    }
    // A side off the screen cuts what is beside it: names on the left,
    // points (right-aligned, so losing their last digits) on the right.
    if [window.left, window.top, window.right].contains(&Side::BeyondFrame) {
        return Err(SkillsError::WindowClipped);
    }
    let runs = located.runs(image, profile);
    let read = |rect: Rect| -> Result<String, SkillsError> {
        read_run(image, rect, &profile.read, recogniser)
            .map(|(text, _)| text)
            .ok_or(SkillsError::NoRecogniser)
    };
    let no_recogniser = |_| SkillsError::NoRecogniser;
    let covered = |rect: Rect| {
        located
            .in_front
            .iter()
            .any(|f| f.intersect(&rect).is_some())
    };

    // The header names the tab: both its name and points headings are
    // the SKILLS tab's alone (the PROFESSIONS tab heads its table with
    // neither), so either one reading confirms the tab. Both are wanted,
    // since each marks its column, but a table scrolled by part of a row
    // draws that row's icon behind the name heading, where it does not
    // read; the points heading at the header's right stands clear of it.
    // Whichever heading is found first anchors the headings' line, whose
    // ink is then grouped afresh to clip away whatever reaches above and
    // below it, and the other heading is looked for along it.
    let find = |runs: &[TextRun], label: &str| -> Result<Option<TextRun>, SkillsError> {
        Ok(first_label(image, runs, label, &profile.read, recogniser)
            .map_err(no_recogniser)?
            .cloned())
    };
    let first_points = find(&runs, &table.points_header)?;
    let anchor = match &first_points {
        Some(points) => points.clone(),
        None => find(&runs, &table.name_header)?.ok_or(SkillsError::NotSkillsTab)?,
    };
    let line_slack = (table.line_slack * anchor.rect.height() as f32).round() as i32;
    let (line_top, line_bottom) = (anchor.rect.y0 - line_slack, anchor.rect.y1 + line_slack);
    let on_label_line = |r: &Rect| {
        let mid = centre_y(r);
        mid >= line_top && mid <= line_bottom
    };
    let line_runs: Vec<TextRun> = text_runs(
        image,
        Rect::new(window.rect.x0, line_top, window.rect.x1, line_bottom),
        &profile.text,
    )
    .into_iter()
    .filter(|r| !covered(r.rect))
    .collect();
    let (name_label, points_label) = match first_points {
        Some(points) => (find(&line_runs, &table.name_header)?, Some(points)),
        None => (
            Some(anchor.clone()),
            find(&line_runs, &table.points_header)?,
        ),
    };

    // Rows are the bands between the table's rule lines.
    let panel = Panel {
        rgb: profile.chrome.panel_rgb,
        tolerance: profile.chrome.panel_tolerance,
    };
    let lines = row_rules(image, window.rect, panel, &profile.rules);
    let bands: Vec<(i32, i32)> = lines
        .windows(2)
        .map(|pair| (pair[0].1 + 1, pair[1].0 - 1))
        .collect();
    let label_mid = centre_y(&anchor.rect);
    let header_index = bands
        .iter()
        .position(|(top, bottom)| label_mid >= *top && label_mid <= *bottom)
        .ok_or(SkillsError::TableNotFound)?;
    let header = bands[header_index];
    let extent = rule_extent(
        image,
        lines[header_index + 1].0,
        (window.rect.x0, window.rect.x1),
        panel,
        &profile.rules,
    )
    .ok_or(SkillsError::TableNotFound)?;
    if [&name_label, &points_label]
        .into_iter()
        .flatten()
        .any(|label| label.rect.x0 < extent.0 || label.rect.x1 > extent.1)
    {
        return Err(SkillsError::TableNotFound);
    }

    // Columns run between the dividers. A column whose heading read is
    // the one holding it, and the heading must lie wholly inside it (a
    // stray upright line through a heading is not a divider); one whose
    // heading did not read is placed by the profile's column order, and
    // only when the dividers show exactly that many columns.
    let reach = profile.rules.divider_reach;
    let mut dividers = column_rules(
        image,
        Rect::new(extent.0 + reach, header.0, extent.1 - reach, header.1),
        &profile.rules,
    );
    // A row scrolled up under the header can show its icon's frame there
    // as one more upright line. A divider carries on across the rule
    // between one row and the next, where icons stop short of it, so
    // with two rows below the header, only lines that do are kept.
    if let (Some(&upper), Some(&lower)) = (bands.get(header_index + 1), bands.get(header_index + 2))
    {
        let height = |(top, bottom): (i32, i32)| bottom - top + 1;
        let alike = (height(upper) - height(lower)).abs() as f32
            <= table.row_slack * height(upper).max(height(lower)) as f32;
        if alike {
            dividers.retain(|&d| {
                carries_across(
                    image,
                    (d - reach, d + reach),
                    (upper.1, lower.0),
                    reach,
                    &profile.rules,
                )
            });
        }
    }
    let mut edges = vec![extent.0 - 1];
    edges.extend(dividers.iter().copied());
    edges.push(extent.1 + 1);
    let column = |label: Option<&TextRun>, key: &str| -> Option<(i32, i32)> {
        let index = match label {
            Some(label) => edges
                .iter()
                .filter(|d| **d < label.rect.x0)
                .count()
                .checked_sub(1)?,
            None if edges.len() == table.columns.len() + 1 => {
                table.columns.iter().position(|c| c == key)?
            }
            None => return None,
        };
        let (left, right) = (*edges.get(index)?, *edges.get(index + 1)?);
        if label.is_some_and(|label| label.rect.x1 >= right) {
            return None;
        }
        Some((left + 1, right - 1))
    };
    let name_column = column(name_label.as_ref(), "name").ok_or(SkillsError::TableNotFound)?;
    let points_column =
        column(points_label.as_ref(), "points").ok_or(SkillsError::TableNotFound)?;
    if name_column.1 >= points_column.0 {
        return Err(SkillsError::TableNotFound);
    }
    // The narrowest room beside the icon that could still hold a name:
    // as wide as a heading set in the header's own small type.
    let least_name_room = name_label
        .as_ref()
        .or(points_label.as_ref())
        .map_or(0, |label| label.rect.width());

    // A band is the table's while its dividers cross it.
    let crosses = |top: i32, bottom: i32| -> bool {
        let mut checked = 0;
        for &d in &dividers {
            let strip = Rect::new(d - reach, top, d + reach, bottom);
            if covered(strip) {
                continue;
            }
            checked += 1;
            if column_rules(image, strip, &profile.rules).is_empty() {
                return false;
            }
        }
        checked > 0
    };
    let below: Vec<Band> = bands[header_index + 1..]
        .iter()
        .map(|&(top, bottom)| Band {
            top,
            bottom,
            ruled: crosses(top, bottom),
        })
        .collect();
    let shape = table_shape(&below, table.row_slack);
    let row_slack = table.row_slack * shape.pitch as f32;
    // The scrollbar's thumb is rule-coloured too, and where it lies
    // against a rule it lengthens it; a thumb shorter than its track
    // leaves some rule in view unlengthened, and one that fills its track
    // means the list fits anyway. So the table ends where the shortest of
    // its rules in view does, leaving out any that a window in front cuts
    // short.
    let table_right = lines[header_index + 1..=header_index + shape.rows.len()]
        .iter()
        .filter_map(|&(y, _)| {
            rule_extent(image, y, (extent.0, extent.1), panel, &profile.rules)
                .map(|(_, x1)| x1)
                .filter(|&x1| !covered(Rect::new(x1 + 1, y, x1 + 1 + reach, y)))
        })
        .min()
        .unwrap_or(extent.1);
    let points_column = (points_column.0, points_column.1.min(table_right));

    // The header has no rule of its own beneath it: the first row's top
    // rule is its bottom edge, and a table scrolled by part of a row
    // slides that row up under the header. Such a header is taller than
    // the client draws it, and the row's lettering shows in it off the
    // headings' line.
    let header_height = header.1 - header.0 + 1;
    let under_header = header_height as f32
        > table.header_height_share * shape.pitch as f32 + row_slack
        || runs.iter().any(|r| {
            r.rect.y0 >= header.0
                && r.rect.y1 <= header.1
                && r.rect.x0 >= extent.0
                && r.rect.x1 <= extent.1
                && !on_label_line(&r.rect)
        });

    let mut rows = Vec::new();
    let mut unread = Vec::new();
    let mut partial = usize::from(under_header);
    for (band, full) in &shape.rows {
        let name_cell = Rect::new(name_column.0, band.top, name_column.1, band.bottom);
        let points_cell = Rect::new(points_column.0, band.top, points_column.1, band.bottom);
        if !*full || covered(name_cell) || covered(points_cell) {
            partial += 1;
            continue;
        }
        let points_runs: Vec<&TextRun> = runs
            .iter()
            .filter(|r| {
                let mid_x = (r.rect.x0 + r.rect.x1) / 2;
                r.rect.y0 >= band.top
                    && r.rect.y1 <= band.bottom
                    && mid_x >= points_cell.x0
                    && mid_x <= points_cell.x1
            })
            .collect();
        let Some(points_rect) = union(&points_runs) else {
            unread.push(UnreadRow {
                raw_name: None,
                raw_points: None,
            });
            continue;
        };
        // Lettering sits on the row's middle; text off it belongs to a
        // row the band only partly shows.
        if (centre_y(&points_rect) - (band.top + band.bottom) / 2).abs() as f32 > row_slack {
            partial += 1;
            continue;
        }
        let text_height = points_rect.height() as f32;
        let raw_points = read(points_rect)?;
        // Right-aligned points that crowd their column's left divider may
        // have lost leading digits to it.
        let clear_of_divider =
            (points_rect.x0 - points_cell.x0) as f32 >= table.points_clearance * text_height;
        let lettering = name_lettering(image, name_cell, &points_rect, table, profile);
        let (raw_name, name) = match lettering {
            // No ink beside the icon at all, in a cell with room for a
            // name: the row the client draws without one. A cell with no
            // such room (a stray line taken for a divider has narrowed
            // it) proves nothing, and the row is unread.
            Lettering::None { room } if room >= least_name_room => (None, Some(RowName::Blank)),
            Lettering::None { .. } => (None, None),
            Lettering::At(rect) => {
                let raw = read(rect)?;
                let cut_to_fit = (name_column.1 - rect.x1) as f32 <= table.cut_reach * text_height;
                let name = match resolve_name(&raw, vocabulary, cut_to_fit, table) {
                    NameMatch::Exact(name) => Some(RowName::Known {
                        name,
                        near_miss: false,
                    }),
                    NameMatch::NearMiss(name) => Some(RowName::Known {
                        name,
                        near_miss: true,
                    }),
                    NameMatch::Unknown => Some(RowName::Unknown),
                    NameMatch::Unclear => None,
                };
                (Some(raw), name)
            }
        };
        // A digit lost from the fraction still parses; its ink is too
        // wide for what was read.
        let points = parse(Grammar::GroupedDecimal, &raw_points)
            .filter(|_| clear_of_divider)
            .filter(|p| width_agrees(image, points_rect, &p.text, &profile.read.numerals));
        match (name, points) {
            (Some(name), Some(points)) => rows.push(SkillRow {
                name,
                raw_name: raw_name.unwrap_or_default(),
                points: points.number.expect("a grouped decimal has a number"),
                points_text: points.text,
            }),
            _ => unread.push(UnreadRow {
                raw_name,
                raw_points: Some(raw_points),
            }),
        }
    }

    // The scrollbar says whether the list fits the view: rows can be out
    // of view above or below whatever the rows in view look like. Its
    // track runs from the header's top to the bottom of the view; when it
    // cannot be made out, the list is not known to fit.
    let view_bottom = shape.rows.last().map(|(band, _)| band.bottom);
    let mut overflows = false;
    let in_view = view_bottom.is_some_and(|bottom| {
        let strip = Rect::new(table_right + 1, header.0, window.rect.x1, bottom);
        if covered(strip) {
            return false;
        }
        match scrollbar(image, strip, shape.pitch, &table.scrollbar) {
            Scrollbar::NoThumb => true,
            Scrollbar::Thumb { top, bottom: end } => {
                let fills =
                    (end - top + 1) as f32 >= table.scrollbar.fill_share * strip.height() as f32;
                overflows = !fills;
                fills
            }
            Scrollbar::Unknown => false,
        }
    });

    // The bar above the table: category, total and pager, each found by
    // what its text says rather than where it sits.
    let toolbar: Vec<TextRun> = runs
        .iter()
        .filter(|r| r.rect.y1 < header.0)
        .cloned()
        .collect();
    let mut category = None;
    let mut total = None;
    let mut page = None;
    for run in &toolbar {
        let text = read(run.rect)?;
        if let Some(found) = pager(&text) {
            // A page count past any listing's is a misread digit.
            page = (found.1 <= table.max_pages).then_some(found);
            continue;
        }
        match parse_total(&text, &table.total_labels, profile.read.label_score) {
            Some(TotalText::Value(value)) => total = Some(value),
            Some(TotalText::LabelOnly) => {
                if let Some(value_run) = run_right_of(run, &toolbar) {
                    total = parse_integer(&read(value_run.rect)?);
                }
            }
            None => {
                if let Some(found) = resolve_category(&text, vocabulary, table) {
                    category = Some(found);
                }
            }
        }
    }

    let table_area = Rect::new(extent.0, header.0, extent.1, window.rect.y1);
    let whole = in_view
        && !under_header
        && shape.rows.first().is_some_and(|(_, full)| *full)
        && partial == 0
        && unread.is_empty()
        && !covered(table_area);
    Ok(SkillsPage {
        category,
        total,
        page,
        rows,
        unread,
        partial,
        whole,
        overflows,
    })
}

/// What a name cell holds beside the icon.
enum Lettering {
    /// Lettering, within this rectangle.
    At(Rect),
    /// No ink at all, in this many pixels of the cell right of the icon.
    None { room: i32 },
}

/// Where a row's name is lettered within its name cell.
///
/// The name sits on the same text line as the row's points; the skill's
/// icon is a picture that reaches above or below that line, with specks
/// of it that may not. The two are told apart by ink groups rather than
/// runs, because a name close in colour to its icon joins the icon's run.
/// The name then begins after the last gap wider than the profile's word
/// gap, which a space between words never is.
fn name_lettering(
    image: &BgrImage,
    cell: Rect,
    points: &Rect,
    table: &SkillsTableProfile,
    profile: &UiProfile,
) -> Lettering {
    let line = points.height() as f32;
    let slack = (table.line_slack * line).round() as i32;
    let gap = (table.word_gap * line).round() as i32;
    let (top, bottom) = (points.y0 - slack, points.y1 + slack);
    let mut groups = ink_groups(image, cell, &profile.text);
    groups.sort_by_key(|g| g.x0);
    let mut icon_right = groups
        .iter()
        .filter(|g| g.y0 < top || g.y1 > bottom)
        .map(|g| g.x1)
        .max();
    for g in &groups {
        if let Some(right) = icon_right {
            if g.x0 <= right {
                icon_right = Some(right.max(g.x1));
            }
        }
    }
    let mut name: Option<Rect> = None;
    for g in groups
        .iter()
        .filter(|g| icon_right.is_none_or(|right| g.x0 > right))
    {
        name = match name {
            Some(r) if g.x0 - r.x1 <= gap => Some(Rect::new(
                r.x0,
                r.y0.min(g.y0),
                r.x1.max(g.x1),
                r.y1.max(g.y1),
            )),
            _ => Some(*g),
        };
    }
    match name {
        Some(rect) => Lettering::At(rect),
        None => Lettering::None {
            room: cell.x1 - icon_right.map_or(cell.x0, |x| x + 1) + 1,
        },
    }
}

/// How a row's name was taken as the skill a sweep holds it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameReading {
    /// The text spelled the skill whole (or the row is the blank one).
    Spelled,
    /// Taken on its own page without spelling it whole: a slip of a
    /// character or two near this skill and no other, or a name the
    /// client cut short that only this skill begins. A reading, flagged
    /// for checking.
    NearMiss,
    /// A name the vocabulary does not know, which the finished sweep
    /// found nearest this skill among the skills it had not otherwise
    /// read, at the looser sweep floor. A likely guess, not a reading.
    Likely,
}

/// One skill in a finished sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepEntry {
    /// `None` for the row the client draws without a name.
    pub name: Option<String>,
    /// How the name was taken as `name`.
    pub reading: NameReading,
    /// The recogniser's text for the name.
    pub raw_name: String,
    pub points_text: String,
    pub points: f64,
    /// The page the row was read on.
    pub page: u32,
}

/// A skill the sweep read whose name it could not place.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownSkill {
    /// The name as the recogniser read it.
    pub raw_name: String,
    pub points_text: String,
    pub points: f64,
    /// The page the row was read on.
    pub page: u32,
}

/// Whether a sweep's skills add up to the total the panel prints.
///
/// `Matches` proves that the whole parts of the points read (fractions
/// dropped) add up to the total the panel printed, over the rows the
/// sweep holds. It does not prove the fractions: a misread digit after
/// the decimal point leaves the sum unchanged and is not caught by it.
/// Nor does it prove each row's name; those are checked against the
/// vocabulary, and a sweep with a name it cannot place or a skill read
/// twice is never `Matches`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checksum {
    Matches,
    /// The readings are kept; the caller shows them for review.
    Mismatch {
        sum: u64,
        total: u64,
    },
    /// No page read a total, pages read different totals (a skill rose
    /// during the sweep), or the points could not be summed: nothing to
    /// check against.
    Unverified,
    /// The sweep holds a skill more than once, more than one blank row,
    /// or a name it cannot place between two skills it has not read: its
    /// rows are not one listing, whatever they add up to.
    Inconsistent,
}

/// Every page of a sweep, merged and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepResult {
    pub category: Option<String>,
    /// The skills the vocabulary knows, and the blank row, page by
    /// page, top to bottom.
    pub skills: Vec<SweepEntry>,
    /// Skills this version does not know, in the same order. Their
    /// points are in the checksum, since the game's total counts them.
    pub unknown: Vec<UnknownSkill>,
    /// Names that come equally near two skills the sweep has not read:
    /// in the checksum, but placed nowhere.
    pub unresolved: Vec<UnknownSkill>,
    /// Skills read more than once (`None` for a second blank row).
    pub duplicates: Vec<Option<String>>,
    pub total: Option<u64>,
    pub checksum: Checksum,
}

/// What became of a page handed to a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Kept, replacing any earlier reading of the same page with the
    /// same skills.
    Kept,
    /// Not every row of the page was read.
    NotWhole,
    /// The pager did not read, so the page's place is unknown.
    NoPager,
    /// The category did not read, so the page's listing is unknown.
    NoCategory,
    /// The page belongs to another listing (another category, or
    /// another page count) than the pages already kept.
    OtherListing,
    /// The page's skills differ from those of the reading already held
    /// for its page number: one of the two pagers may be misread. The
    /// page counts as missing until a later reading agrees with one.
    Conflict,
}

/// What a sweep holds for one page number.
#[derive(Debug, Clone)]
enum Slot {
    Settled(SkillsPage),
    /// Two readings with different skills, the later second.
    Unsettled(SkillsPage, SkillsPage),
}

/// Collects whole pages of the SKILLS tab, in any order, until every
/// page of the listing has been read.
#[derive(Debug, Clone)]
pub struct SkillsSweep {
    uncounted: Vec<String>,
    all_categories: String,
    near_miss: NearMissProfile,
    vocabulary: Vec<String>,
    category: Option<String>,
    count: Option<u32>,
    pages: BTreeMap<u32, Slot>,
}

/// The skills a page holds, as a sweep compares two readings of one
/// page: their names (an unknown one by its text), in sorted order.
fn page_skills(page: &SkillsPage) -> Vec<String> {
    let mut names: Vec<String> = page
        .rows
        .iter()
        .map(|row| match &row.name {
            RowName::Known { name, .. } => name.clone(),
            RowName::Unknown => format!("?{}", row.raw_name.trim()),
            RowName::Blank => String::new(),
        })
        .collect();
    names.sort();
    names
}

impl SkillsSweep {
    pub fn new(table: &SkillsTableProfile, vocabulary: &SkillVocabulary) -> Self {
        Self {
            uncounted: table.uncounted.clone(),
            all_categories: table.all_categories.clone(),
            near_miss: table.near_miss.clone(),
            vocabulary: vocabulary.names.clone(),
            category: None,
            count: None,
            pages: BTreeMap::new(),
        }
    }

    /// Offer a page; the latest whole reading of each page is kept, as
    /// long as it holds the same skills as the reading before it.
    pub fn accept(&mut self, page: SkillsPage) -> Admission {
        if !page.whole {
            return Admission::NotWhole;
        }
        let Some((index, count)) = page.page else {
            return Admission::NoPager;
        };
        let Some(category) = page.category.clone() else {
            return Admission::NoCategory;
        };
        if self.count.is_some_and(|c| c != count)
            || self.category.as_ref().is_some_and(|c| *c != category)
        {
            return Admission::OtherListing;
        }
        self.count = Some(count);
        self.category = Some(category);
        let skills = page_skills(&page);
        let (slot, admission) = match self.pages.remove(&index) {
            None => (Slot::Settled(page), Admission::Kept),
            Some(Slot::Settled(held)) if page_skills(&held) == skills => {
                (Slot::Settled(page), Admission::Kept)
            }
            Some(Slot::Settled(held)) => (Slot::Unsettled(held, page), Admission::Conflict),
            Some(Slot::Unsettled(first, second)) => {
                if page_skills(&first) == skills || page_skills(&second) == skills {
                    (Slot::Settled(page), Admission::Kept)
                } else {
                    (Slot::Unsettled(second, page), Admission::Conflict)
                }
            }
        };
        self.pages.insert(index, slot);
        admission
    }

    /// The reading held for page `index`, once settled.
    pub fn page(&self, index: u32) -> Option<&SkillsPage> {
        match self.pages.get(&index)? {
            Slot::Settled(page) => Some(page),
            Slot::Unsettled(..) => None,
        }
    }

    /// How many pages the listing has, once a kept page has told it.
    pub fn page_count(&self) -> Option<u32> {
        self.count
    }

    /// The pages still to read, counting a page whose readings conflict;
    /// `None` until a page has told how many there are.
    pub fn missing(&self) -> Option<Vec<u32>> {
        let count = self.count?;
        Some(
            (1..=count)
                .filter(|index| self.page(*index).is_none())
                .collect(),
        )
    }

    /// The merged skills and their checksum, once every page is whole
    /// and settled.
    ///
    /// In the all-categories listing, a name the vocabulary does not know
    /// is looked at again here, with what the sweep knows: a skill the
    /// sweep has read elsewhere cannot be this row too. Among the skills
    /// it has not read, one at or above the profile's lower sweep floor, of
    /// about the name's length and clear of the next by the margin, is the
    /// row's likely skill ([`NameReading::Likely`]); two that tie leave the
    /// row unresolved; none near leaves it unknown (a skill the game has
    /// and the list lacks). In a single category's listing nearly every
    /// skill is one the sweep has not read, so the guess would be loose
    /// enough to name almost anything: there an unknown name stays unknown.
    pub fn finish(&self) -> Option<SweepResult> {
        if !self.missing()?.is_empty() {
            return None;
        }
        let pages: Vec<(u32, &SkillsPage)> = self
            .pages
            .keys()
            .filter_map(|index| self.page(*index).map(|page| (*index, page)))
            .collect();
        let rows: Vec<(u32, &SkillRow)> = pages
            .iter()
            .flat_map(|(index, page)| page.rows.iter().map(move |row| (*index, row)))
            .collect();
        let read: Vec<&str> = rows.iter().filter_map(|(_, row)| row.skill()).collect();
        let absent: Vec<&String> = self
            .vocabulary
            .iter()
            .filter(|name| !read.contains(&name.as_str()))
            .collect();
        let placing = self.category.as_deref() == Some(self.all_categories.as_str());
        let mut skills = Vec::new();
        let mut unknown = Vec::new();
        let mut unresolved = Vec::new();
        for &(page, row) in &rows {
            let (name, reading) = match &row.name {
                RowName::Known { name, near_miss } => (
                    Some(name.clone()),
                    if *near_miss {
                        NameReading::NearMiss
                    } else {
                        NameReading::Spelled
                    },
                ),
                RowName::Blank => (None, NameReading::Spelled),
                RowName::Unknown => {
                    let skill = UnknownSkill {
                        raw_name: row.raw_name.clone(),
                        points_text: row.points_text.clone(),
                        points: row.points,
                        page,
                    };
                    let best = if placing {
                        let key = label_key(strip_ellipsis(row.raw_name.trim()).0);
                        let candidates = scored(&key, absent.iter().copied(), &self.near_miss);
                        single_best(
                            &candidates,
                            self.near_miss.sweep_score,
                            self.near_miss.margin,
                        )
                    } else {
                        Best::None
                    };
                    match best {
                        Best::One(name) => (Some(name.to_string()), NameReading::Likely),
                        Best::None => {
                            unknown.push(skill);
                            continue;
                        }
                        Best::Tie => {
                            unresolved.push(skill);
                            continue;
                        }
                    }
                }
            };
            skills.push(SweepEntry {
                name,
                reading,
                raw_name: row.raw_name.clone(),
                points_text: row.points_text.clone(),
                points: row.points,
                page,
            });
        }
        let mut duplicates: Vec<Option<String>> = Vec::new();
        for (i, entry) in skills.iter().enumerate() {
            if skills[..i].iter().any(|e| e.name == entry.name) && !duplicates.contains(&entry.name)
            {
                duplicates.push(entry.name.clone());
            }
        }
        let mut totals = pages.iter().filter_map(|(_, page)| page.total);
        let total = totals.next();
        let agreed = total.filter(|first| totals.all(|t| t == *first));
        // The panel's total counts each skill's whole points: the
        // fraction is dropped, from the text, so no rounding enters.
        let sum = skills
            .iter()
            .filter(|entry| {
                entry
                    .name
                    .as_ref()
                    .is_none_or(|name| !self.uncounted.contains(name))
            })
            .map(|entry| entry.points_text.as_str())
            .chain(unknown.iter().map(|skill| skill.points_text.as_str()))
            .chain(unresolved.iter().map(|skill| skill.points_text.as_str()))
            .try_fold(0u64, |sum, text| sum.checked_add(whole_points(text)?));
        let checksum = match (agreed, sum) {
            _ if !duplicates.is_empty() || !unresolved.is_empty() => Checksum::Inconsistent,
            (Some(total), Some(sum)) if total == sum => Checksum::Matches,
            (Some(total), Some(sum)) => Checksum::Mismatch { sum, total },
            _ => Checksum::Unverified,
        };
        Some(SweepResult {
            category: self.category.clone(),
            skills,
            unknown,
            unresolved,
            duplicates,
            total: agreed,
            checksum,
        })
    }
}

/// The whole part of points in the client's notation; `None` when the
/// text is not such a number.
fn whole_points(points_text: &str) -> Option<u64> {
    let whole = points_text.split('.').next()?;
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return None;
    }
    whole.replace(',', "").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{
        blank, bundled_profile, draw_window, fill, skills_drawing_vocabulary, skills_window,
        standard_skills_layout, SkillsLayout, SKILLS_ROW_PITCH, THUMB,
    };

    fn table_profile() -> SkillsTableProfile {
        bundled_profile().windows[WINDOW]
            .skills_table
            .clone()
            .expect("the bundled profile describes the skills table")
    }

    fn vocabulary() -> SkillVocabulary {
        SkillVocabulary::new(
            [
                "Agility",
                "Attachments Technology",
                "BLP Weaponry Technology",
                "Explosive Projectile Weaponry Technology",
                "Laser Weaponry Technology",
                "Manufacture Armor",
                "Manufacture Attachments",
                "Tools Technology",
                "Wood Carving",
                "Promoter Rating",
                "Reputation",
            ]
            .map(String::from)
            .to_vec(),
            ["Combat", "General"].map(String::from).to_vec(),
        )
    }

    /// The skill names the application ships.
    fn bundled_vocabulary() -> SkillVocabulary {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../entropia-orme/resources/snapshot/skills.json");
        let entities: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("skills snapshot"))
                .expect("skills snapshot parses");
        SkillVocabulary::from_entities(&entities)
    }

    fn exact(name: &str) -> NameMatch {
        NameMatch::Exact(name.to_string())
    }

    fn near(name: &str) -> NameMatch {
        NameMatch::NearMiss(name.to_string())
    }

    #[test]
    fn a_name_resolves_when_it_spells_a_skill() {
        let (v, t) = (vocabulary(), table_profile());
        let resolve = |raw: &str| resolve_name(raw, &v, false, &t);
        assert_eq!(resolve("Agility"), exact("Agility"));
        assert_eq!(
            resolve(" blp  weaponry technology "),
            exact("BLP Weaponry Technology")
        );
        assert_eq!(resolve(""), NameMatch::Unclear);
        assert_eq!(resolve("..."), NameMatch::Unclear);
        assert_eq!(resolve("Ag"), NameMatch::Unclear);
        // Trailing dots on a name that does not reach its column's edge
        // are a misreading, whatever they precede.
        assert_eq!(resolve("Attachments Technol..."), NameMatch::Unclear);
        assert_eq!(resolve("Agility."), NameMatch::Unclear);
    }

    #[test]
    fn a_name_cut_to_fit_resolves_as_a_near_miss_of_the_one_skill_it_begins() {
        let (v, t) = (vocabulary(), table_profile());
        let resolve = |raw: &str| resolve_name(raw, &v, true, &t);
        assert_eq!(
            resolve("Attachments Technol..."),
            near("Attachments Technology")
        );
        assert_eq!(
            resolve("Explosive Projectile Weaponry Techn\u{2026}"),
            near("Explosive Projectile Weaponry Technology")
        );
        // The recogniser can render the ellipsis as a single dot.
        assert_eq!(
            resolve("Explosive Projectile Weaponry Techn."),
            near("Explosive Projectile Weaponry Technology")
        );
        // Two skills begin this way: unclear.
        assert_eq!(resolve("Manufacture A..."), NameMatch::Unclear);
        // Without its ellipsis a stem is not a name, even at the edge;
        // a name that just fits its column reads whole.
        assert_eq!(resolve("Attachments Technol"), NameMatch::Unclear);
        assert_eq!(
            resolve("Laser Weaponry Technology"),
            exact("Laser Weaponry Technology")
        );
    }

    #[test]
    fn a_whole_name_before_stray_dots_is_never_a_longer_skill() {
        let (v, t) = (bundled_vocabulary(), table_profile());
        let resolve = |raw: &str| resolve_name(raw, &v, true, &t);
        for (stem, longer) in [
            ("Drilling", "Drilling Expertise"),
            ("Coloring", "Coloring Methodology"),
            ("Miner", "Mineral Sense"),
        ] {
            assert!(v.names().iter().any(|n| n == longer), "{longer}");
            assert_eq!(resolve(&format!("{stem}.")), near(stem), "{stem}");
            assert_eq!(resolve(&format!("{stem}...")), near(stem), "{stem}");
        }
    }

    #[test]
    fn slips_of_a_character_resolve_as_near_misses_of_their_skill() {
        let (v, t) = (bundled_vocabulary(), table_profile());
        let resolve = |raw: &str| resolve_name(raw, &v, false, &t);
        assert_eq!(resolve("Cooliness"), near("Coolness"));
        assert_eq!(resolve("Castingc"), near("Casting"));
        assert_eq!(resolve("Martial Aarts"), near("Martial Arts"));
        assert_eq!(resolve("Couragec"), near("Courage"));
        assert_eq!(resolve("Xenobiologyc"), near("Xenobiology"));
        assert_eq!(resolve("scourgingcc"), near("Scourging"));
    }

    #[test]
    fn a_skill_the_vocabulary_lacks_is_unknown_not_its_neighbour() {
        let (v, t) = (bundled_vocabulary(), table_profile());
        assert!(!v.names().iter().any(|n| n == "Food Technology"));
        assert_eq!(
            resolve_name("Food Technology", &v, false, &t),
            NameMatch::Unknown
        );
        let (nearest, score) = nearest_skill("Food Technology", &v).expect("a nearest skill");
        assert!(score < t.near_miss.score, "{nearest} at {score}");
        // A short name with one character substituted is too far from
        // its skill for a page alone to say which skill it is.
        assert_eq!(resolve_name("Alm", &v, false, &t), NameMatch::Unknown);
        // Near two skills at once: neither is taken.
        let pair =
            SkillVocabulary::new(vec!["Fishing Rod".into(), "Fishing Red".into()], Vec::new());
        assert_eq!(
            resolve_name("Fishing Rad", &pair, false, &t),
            NameMatch::Unclear
        );
    }

    #[test]
    fn categories_resolve_to_the_vocabulary_or_the_all_entry() {
        let (v, t) = (vocabulary(), table_profile());
        assert_eq!(
            resolve_category("ALL CATEGORIES", &v, &t),
            Some("ALL CATEGORIES".into())
        );
        assert_eq!(resolve_category("Combat", &v, &t), Some("Combat".into()));
        assert_eq!(resolve_category("Cornbat", &v, &t), None);
        assert_eq!(resolve_category("SKILLS", &v, &t), None);
    }

    #[test]
    fn the_total_is_read_after_either_label() {
        let labels = table_profile().total_labels;
        let total = |text: &str| parse_total(text, &labels, 80.0);
        assert_eq!(total("Total: 198259"), Some(TotalText::Value(198259)));
        assert_eq!(total("Total:198259"), Some(TotalText::Value(198259)));
        assert_eq!(
            total("Total skill points in category: 61705"),
            Some(TotalText::Value(61705))
        );
        assert_eq!(total("Total:"), Some(TotalText::LabelOnly));
        // The value must be digits alone; other text is not the total.
        assert_eq!(total("Total: 198,259"), None);
        assert_eq!(total("Total: 19825g"), None);
        assert_eq!(total("1 / 13"), None);
        assert_eq!(total("ALL CATEGORIES"), None);
    }

    fn scrollbar_strip(bars: &[Rect]) -> (BgrImage, Rect) {
        let mut image = blank(60, 700, [8, 11, 11]);
        for bar in bars {
            fill(&mut image, *bar, THUMB);
        }
        (image, Rect::new(10, 50, 45, 650))
    }

    #[test]
    fn the_scrollbar_shows_a_thumb_none_or_cannot_be_made_out() {
        let profile = table_profile().scrollbar;
        let (image, strip) = scrollbar_strip(&[]);
        assert_eq!(scrollbar(&image, strip, 57, &profile), Scrollbar::NoThumb);
        let (image, strip) = scrollbar_strip(&[Rect::new(25, 50, 36, 329)]);
        assert_eq!(
            scrollbar(&image, strip, 57, &profile),
            Scrollbar::Thumb {
                top: 50,
                bottom: 329
            }
        );
        // Two bars, or one far too narrow for a thumb, are not a thumb.
        let (image, strip) =
            scrollbar_strip(&[Rect::new(12, 60, 17, 300), Rect::new(30, 60, 41, 300)]);
        assert_eq!(scrollbar(&image, strip, 57, &profile), Scrollbar::Unknown);
        let (image, strip) = scrollbar_strip(&[Rect::new(30, 60, 31, 300)]);
        assert_eq!(scrollbar(&image, strip, 57, &profile), Scrollbar::Unknown);
    }

    fn known(name: &str, points: &str) -> SkillRow {
        SkillRow {
            name: RowName::Known {
                name: name.to_string(),
                near_miss: false,
            },
            raw_name: name.to_string(),
            points_text: points.to_string(),
            points: points.replace(',', "").parse().unwrap_or(0.0),
        }
    }

    fn blank_row(points: &str) -> SkillRow {
        SkillRow {
            name: RowName::Blank,
            raw_name: String::new(),
            ..known("", points)
        }
    }

    fn unknown(raw: &str, points: &str) -> SkillRow {
        SkillRow {
            name: RowName::Unknown,
            ..known(raw, points)
        }
    }

    fn page(index: u32, count: u32, total: u64, rows: Vec<SkillRow>) -> SkillsPage {
        SkillsPage {
            category: Some("ALL CATEGORIES".into()),
            total: Some(total),
            page: Some((index, count)),
            rows,
            unread: Vec::new(),
            partial: 0,
            whole: true,
            overflows: false,
        }
    }

    /// Three pages whose counted whole points add to 4,000: the unnamed
    /// row and the skill the vocabulary lacks count, the uncounted
    /// skills do not, fractions are dropped.
    fn pages(total: u64) -> Vec<SkillsPage> {
        vec![
            page(
                1,
                3,
                total,
                vec![blank_row("1"), known("Agility", "1,268.99")],
            ),
            page(
                2,
                3,
                total,
                vec![
                    known("Promoter Rating", "1,000"),
                    known("Laser Weaponry Technology", "2,499.5"),
                    known("Tools Technology", "1"),
                ],
            ),
            page(
                3,
                3,
                total,
                vec![
                    known("Reputation", "1,000"),
                    known("Wood Carving", "230.01"),
                    unknown("Food Technology", "1"),
                ],
            ),
        ]
    }

    fn sweep() -> SkillsSweep {
        SkillsSweep::new(&table_profile(), &vocabulary())
    }

    #[test]
    fn a_sweep_in_any_order_checks_against_the_total() {
        let mut sweep = sweep();
        assert_eq!(sweep.missing(), None);
        let mut shuffled = pages(4000);
        shuffled.swap(0, 2);
        for (i, page) in shuffled.into_iter().enumerate() {
            assert!(sweep.finish().is_none());
            assert_eq!(sweep.accept(page), Admission::Kept);
            assert_eq!(sweep.missing().map(|m| m.len()), Some(2 - i));
        }
        let result = sweep.finish().expect("every page is in");
        assert_eq!(result.checksum, Checksum::Matches);
        assert_eq!(result.total, Some(4000));
        assert_eq!(result.skills.len(), 7);
        // Its nearest neighbour, Tools Technology, is read elsewhere in
        // the sweep, so the skill the list lacks stays unknown.
        assert_eq!(result.unknown.len(), 1);
        assert_eq!(result.unknown[0].raw_name, "Food Technology");
        assert_eq!(result.skills[0].name, None);
        assert_eq!(result.skills[1].name.as_deref(), Some("Agility"));
    }

    #[test]
    fn a_sweep_that_does_not_add_up_keeps_its_readings() {
        let mut sweep = sweep();
        for page in pages(4001) {
            sweep.accept(page);
        }
        let result = sweep.finish().expect("every page is in");
        assert_eq!(
            result.checksum,
            Checksum::Mismatch {
                sum: 4000,
                total: 4001
            }
        );
        assert_eq!(result.skills.len(), 7);
    }

    #[test]
    fn a_misread_short_name_is_placed_among_the_skills_the_sweep_lacks() {
        let vocabulary = SkillVocabulary::new(
            ["Agility", "Aim", "Analysis"].map(String::from).to_vec(),
            Vec::new(),
        );
        let mut sweep = SkillsSweep::new(&table_profile(), &vocabulary);
        sweep.accept(page(
            1,
            1,
            1341,
            vec![known("Agility", "73.71"), unknown("Alm", "1,268.53")],
        ));
        let result = sweep.finish().expect("the one page is in");
        let aim = &result.skills[1];
        assert_eq!(aim.name.as_deref(), Some("Aim"));
        assert_eq!(aim.reading, NameReading::Likely);
        assert_eq!(aim.raw_name, "Alm");
        assert_eq!(aim.page, 1);
        assert!(result.unknown.is_empty());
        assert_eq!(result.checksum, Checksum::Matches);
    }

    #[test]
    fn in_a_single_category_an_unknown_name_is_never_guessed() {
        let vocabulary = SkillVocabulary::new(
            ["Agility", "Aim", "Analysis"].map(String::from).to_vec(),
            ["Combat"].map(String::from).to_vec(),
        );
        let mut sweep = SkillsSweep::new(&table_profile(), &vocabulary);
        let mut combat = page(
            1,
            1,
            1341,
            vec![known("Agility", "73.71"), unknown("Alm", "1,268.53")],
        );
        combat.category = Some("Combat".into());
        assert_eq!(sweep.accept(combat), Admission::Kept);
        let result = sweep.finish().expect("the one page is in");
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.unknown.len(), 1);
        assert_eq!(result.unknown[0].raw_name, "Alm");
        assert_eq!(result.checksum, Checksum::Matches);
    }

    #[test]
    fn each_entry_says_how_its_name_was_read_and_on_which_page() {
        let mut sweep = sweep();
        let mut all = pages(4000);
        all[1].rows[1] = SkillRow {
            name: RowName::Known {
                name: "Laser Weaponry Technology".into(),
                near_miss: true,
            },
            ..known("Laser Weaponry Technolgy", "2,499.5")
        };
        for page in all {
            sweep.accept(page);
        }
        let result = sweep.finish().expect("every page is in");
        let readings: Vec<(Option<&str>, NameReading, u32)> = result
            .skills
            .iter()
            .map(|s| (s.name.as_deref(), s.reading, s.page))
            .collect();
        assert_eq!(
            readings,
            vec![
                (None, NameReading::Spelled, 1),
                (Some("Agility"), NameReading::Spelled, 1),
                (Some("Promoter Rating"), NameReading::Spelled, 2),
                (Some("Laser Weaponry Technology"), NameReading::NearMiss, 2),
                (Some("Tools Technology"), NameReading::Spelled, 2),
                (Some("Reputation"), NameReading::Spelled, 3),
                (Some("Wood Carving"), NameReading::Spelled, 3),
            ]
        );
        assert_eq!(result.unknown[0].page, 3);
    }

    #[test]
    fn a_name_equally_near_two_missing_skills_is_unresolved() {
        let vocabulary = SkillVocabulary::new(
            ["Agility", "Aim", "Arm"].map(String::from).to_vec(),
            Vec::new(),
        );
        let mut sweep = SkillsSweep::new(&table_profile(), &vocabulary);
        sweep.accept(page(
            1,
            1,
            1341,
            vec![known("Agility", "73.71"), unknown("Alm", "1,268.53")],
        ));
        let result = sweep.finish().expect("the one page is in");
        assert_eq!(result.unresolved.len(), 1);
        assert_eq!(result.checksum, Checksum::Inconsistent);
    }

    #[test]
    fn a_skill_or_a_blank_row_read_twice_is_inconsistent() {
        let mut sweep = sweep();
        let mut all = pages(4000);
        all[2].rows.push(known("Agility", "0"));
        for page in all {
            sweep.accept(page);
        }
        let result = sweep.finish().expect("every page is in");
        assert_eq!(result.duplicates, vec![Some("Agility".to_string())]);
        assert_eq!(result.checksum, Checksum::Inconsistent);

        let mut sweep = self::sweep();
        let mut all = pages(4000);
        all[1].rows.push(blank_row("0"));
        for page in all {
            sweep.accept(page);
        }
        let result = sweep.finish().expect("every page is in");
        assert_eq!(result.duplicates, vec![None]);
        assert_eq!(result.checksum, Checksum::Inconsistent);
    }

    #[test]
    fn a_page_without_a_category_pager_or_whole_reading_is_not_admitted() {
        let mut sweep = sweep();
        let all = pages(4000);
        let mut cut = all[1].clone();
        cut.whole = false;
        assert_eq!(sweep.accept(cut), Admission::NotWhole);
        let mut unpaged = all[0].clone();
        unpaged.page = None;
        assert_eq!(sweep.accept(unpaged), Admission::NoPager);
        let mut uncategorised = all[0].clone();
        uncategorised.category = None;
        assert_eq!(sweep.accept(uncategorised), Admission::NoCategory);
        assert_eq!(sweep.accept(all[0].clone()), Admission::Kept);
        let mut other = all[1].clone();
        other.category = Some("Combat".into());
        assert_eq!(sweep.accept(other), Admission::OtherListing);
        let mut longer = all[1].clone();
        longer.page = Some((2, 4));
        assert_eq!(sweep.accept(longer), Admission::OtherListing);
        assert_eq!(sweep.missing(), Some(vec![2, 3]));
    }

    #[test]
    fn a_reread_with_the_same_skills_replaces_the_earlier_reading() {
        let mut sweep = sweep();
        let all = pages(4000);
        let mut earlier = all[1].clone();
        earlier.rows[1] = known("Laser Weaponry Technology", "2,400.5");
        earlier.rows.reverse();
        assert_eq!(sweep.accept(earlier), Admission::Kept);
        assert_eq!(sweep.accept(all[1].clone()), Admission::Kept);
        assert_eq!(sweep.page(2), Some(&all[1]));
    }

    #[test]
    fn a_reread_with_other_skills_unsettles_the_page_until_one_is_confirmed() {
        let mut sweep = sweep();
        let all = pages(4000);
        for page in all.clone() {
            sweep.accept(page);
        }
        // Page 3's rows under page 2's number: one of the pagers misread.
        let mut misplaced = all[2].clone();
        misplaced.page = Some((2, 3));
        assert_eq!(sweep.accept(misplaced.clone()), Admission::Conflict);
        assert_eq!(sweep.missing(), Some(vec![2]));
        assert!(sweep.finish().is_none());
        assert_eq!(sweep.page(2), None);
        // A third reading agreeing with neither keeps the page unsettled;
        // one agreeing with the first settles it.
        let mut third = all[0].clone();
        third.page = Some((2, 3));
        assert_eq!(sweep.accept(third), Admission::Conflict);
        assert_eq!(sweep.accept(all[1].clone()), Admission::Conflict);
        assert_eq!(sweep.accept(all[1].clone()), Admission::Kept);
        assert_eq!(sweep.page(2), Some(&all[1]));
        assert_eq!(sweep.finish().map(|r| r.checksum), Some(Checksum::Matches));
    }

    #[test]
    fn pages_that_disagree_on_the_total_leave_the_sweep_unverified() {
        let mut sweep = sweep();
        let mut all = pages(4000);
        all[2].total = Some(4003);
        for page in all {
            sweep.accept(page);
        }
        let result = sweep.finish().expect("every page is in");
        assert_eq!(result.checksum, Checksum::Unverified);
        assert_eq!(result.total, None);
    }

    #[test]
    fn points_that_cannot_be_summed_leave_the_sweep_unverified() {
        for points in ["18,446,744,073,709,551,615", "1,2a"] {
            let mut sweep = sweep();
            let mut all = pages(4000);
            all[0].rows.push(known("Attachments Technology", points));
            all[0].rows.push(known(
                "BLP Weaponry Technology",
                "18,446,744,073,709,551,615",
            ));
            for page in all {
                sweep.accept(page);
            }
            let result = sweep.finish().expect("every page is in");
            assert_eq!(result.checksum, Checksum::Unverified, "{points}");
        }
    }

    fn read(layout: &SkillsLayout) -> Result<SkillsPage, SkillsError> {
        let d = skills_window(layout);
        read_skills_page(
            &d.image,
            &bundled_profile(),
            &skills_drawing_vocabulary(),
            &d.recogniser(),
        )
    }

    fn names(page: &SkillsPage) -> Vec<Option<&str>> {
        page.rows.iter().map(SkillRow::skill).collect()
    }

    #[test]
    fn a_whole_page_reads_every_row_and_its_toolbar() {
        let page = read(&standard_skills_layout()).expect("the page reads");
        assert_eq!(page.category.as_deref(), Some("ALL CATEGORIES"));
        assert_eq!(page.total, Some(198259));
        assert_eq!(page.page, Some((2, 13)));
        assert_eq!(
            names(&page),
            vec![None, Some("Agility"), Some("Aim"), Some("Alertness")]
        );
        assert_eq!(page.rows[0].name, RowName::Blank);
        assert_eq!(page.rows[2].points_text, "1,268.53");
        assert_eq!(page.rows[2].points, 1268.53);
        assert_eq!((page.unread.len(), page.partial), (0, 0));
        assert!(page.whole);
    }

    #[test]
    fn a_view_ending_on_a_row_boundary_is_whole_only_when_the_scrollbar_says_so() {
        // The view ends exactly on the last row's rule with a pane below,
        // as when the window is sized to a whole number of rows.
        let mut layout = standard_skills_layout();
        layout.pane = true;
        assert!(read(&layout).expect("the page reads").whole);
        // A thumb shorter than its track: rows are out of view.
        let track = 32 + 4 * SKILLS_ROW_PITCH;
        layout.thumb = Some((0, track * 2 / 3));
        let page = read(&layout).expect("the page reads");
        assert_eq!(page.rows.len(), 4);
        assert!(!page.whole, "{page:?}");
        // A thumb filling its track: the list is all in view.
        layout.thumb = Some((0, track - 3));
        assert!(read(&layout).expect("the page reads").whole);
    }

    #[test]
    fn the_professions_tab_is_refused() {
        let mut layout = standard_skills_layout();
        layout.name_header = ("Profession Name", 100);
        layout.points_header = "Level";
        assert_eq!(read(&layout), Err(SkillsError::NotSkillsTab));
    }

    #[test]
    fn either_heading_establishes_the_tab_and_its_columns() {
        // A heading drawn over (by a row's icon, say) does not read; the
        // other heading confirms the tab and the column order places the
        // first.
        for (name_header, points_header) in [("Baiole ?", "Points"), ("Skill Name", "P0in")] {
            let mut layout = standard_skills_layout();
            layout.name_header = (name_header, 100);
            layout.points_header = points_header;
            let page = read(&layout)
                .unwrap_or_else(|e| panic!("{name_header:?} / {points_header:?}: {e}"));
            assert_eq!(
                names(&page),
                vec![None, Some("Agility"), Some("Aim"), Some("Alertness")]
            );
            assert!(page.whole);
        }
    }

    #[test]
    fn a_skill_the_vocabulary_lacks_is_read_but_a_bad_number_is_not() {
        let mut layout = standard_skills_layout();
        layout.rows[1] = (Some("Food Technology"), "73.71");
        let page = read(&layout).expect("the page reads");
        assert_eq!(page.rows.len(), 4);
        assert_eq!(page.rows[1].name, RowName::Unknown);
        assert_eq!(page.rows[1].raw_name, "Food Technology");
        assert_eq!(page.rows[1].points_text, "73.71");
        assert!(
            page.whole,
            "an unknown skill does not stop a page being whole"
        );

        layout.rows[2] = (Some("Aim"), "1,44.77");
        let page = read(&layout).expect("the page reads");
        assert_eq!(page.rows.len(), 3);
        assert_eq!(page.unread.len(), 1);
        assert_eq!(page.unread[0].raw_points.as_deref(), Some("1,44.77"));
        assert!(!page.whole);
    }

    #[test]
    fn a_short_name_misread_on_its_page_is_placed_by_the_sweep() {
        let mut layout = standard_skills_layout();
        layout.pager = "1 / 1";
        layout.rows[2] = (Some("Alm"), "1,268.53");
        let page = read(&layout).expect("the page reads");
        assert_eq!(page.rows[2].name, RowName::Unknown);
        let mut sweep = SkillsSweep::new(&table_profile(), &skills_drawing_vocabulary());
        assert_eq!(sweep.accept(page), Admission::Kept);
        let result = sweep.finish().expect("the one page is in");
        let aim = &result.skills[2];
        assert_eq!(
            (aim.name.as_deref(), aim.reading),
            (Some("Aim"), NameReading::Likely)
        );
        assert!(result.unknown.is_empty());
    }

    #[test]
    fn points_crowding_their_columns_left_divider_are_unread() {
        let mut layout = standard_skills_layout();
        // The divider is at 760; the points start two pixels after it.
        layout.points_x = 763;
        let page = read(&layout).expect("the page reads");
        assert!(page.rows.is_empty(), "{page:?}");
        assert_eq!(page.unread.len(), 4);
        assert!(!page.whole);
    }

    #[test]
    fn a_name_cell_narrowed_to_its_icon_is_unread_never_blank() {
        // An upright line just past the icon, read as a divider, leaves
        // the name column no wider than the icon: its empty cells prove
        // nothing about a name.
        let mut layout = standard_skills_layout();
        layout.name_header = ("Skill Name", 58);
        layout.stray_divider = Some(65);
        let page = read(&layout).expect("the page reads");
        assert!(
            page.rows.iter().all(|row| row.name != RowName::Blank),
            "{page:?}"
        );
        assert_eq!(page.unread.len(), 4);
        assert!(!page.whole);
    }

    #[test]
    fn a_pager_past_any_listings_length_is_not_read() {
        let mut layout = standard_skills_layout();
        layout.pager = "2 / 1000";
        assert_eq!(read(&layout).expect("the page reads").page, None);
    }

    #[test]
    fn a_page_scrolled_by_part_of_a_row_is_not_whole() {
        let mut layout = standard_skills_layout();
        layout.scrolled = 40;
        let page = read(&layout).expect("the page reads");
        // The first row is under the header; the rest read.
        assert_eq!(
            names(&page),
            vec![Some("Agility"), Some("Aim"), Some("Alertness")]
        );
        assert_eq!(page.partial, 1);
        assert!(!page.whole);
    }

    #[test]
    fn rows_under_a_window_in_front_are_partial() {
        let layout = standard_skills_layout();
        let mut d = skills_window(&layout);
        // A window over the points of the third and fourth rows.
        let header_bottom = layout.window.y0 + 182;
        let front = Rect::new(
            layout.window.x0 + 790,
            header_bottom + 2 * SKILLS_ROW_PITCH + 20,
            1080,
            880,
        );
        draw_window(&mut d.image, front);
        let page = read_skills_page(
            &d.image,
            &bundled_profile(),
            &skills_drawing_vocabulary(),
            &d.recogniser(),
        )
        .expect("the page reads");
        assert_eq!(names(&page), vec![None, Some("Agility")]);
        assert_eq!(page.partial, 2);
        assert!(!page.whole);
    }

    #[test]
    fn a_window_run_off_the_screen_is_refused() {
        let mut layout = standard_skills_layout();
        layout.window = Rect::new(400, 30, 1360, 860);
        assert_eq!(read(&layout), Err(SkillsError::WindowClipped));
    }
}
