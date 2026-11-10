//! Refusing what the recogniser got wrong.
//!
//! The recogniser reports a confidence, and it cannot be trusted alone:
//! it has read `1,444.77` as `1,44.77` at 96%, collapsing a repeated
//! digit. So a value is accepted on what it says, not on how sure the
//! recogniser was. Each field declares the shape its text must have; a
//! read that does not have exactly that shape is refused, and a refused
//! read is an empty field the user fills in, never a guess.

use serde::Deserialize;

use crate::skill_panel::digit_value;

use super::geometry::{Pixels, Rect};

/// The shapes a field's text can be required to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grammar {
    /// An amount of PED: digits, optionally grouped in threes by commas,
    /// an optional fraction, then `PED`. `27.47542 PED`.
    PedAmount,
    /// A number, optionally grouped in threes by commas, with an
    /// optional fraction. `1,444.77`.
    GroupedDecimal,
    /// Digits only. `67739`.
    Integer,
    /// A number followed by a percent sign. `201.92%`.
    Percent,
    /// A pager's position: page, a slash, page count, with the page
    /// between one and the count. `2 / 13`; its number is the page.
    Pager,
    /// Free text: anything non-empty.
    Text,
}

/// A value that passed its grammar.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// The text in its canonical spelling (ASCII digits, no stray spaces).
    pub text: String,
    /// The number it states, for the numeric grammars.
    pub number: Option<f64>,
}

/// Fold the recogniser's alphabet onto the characters the grammars are
/// written in: fullwidth digits to ASCII, spaces dropped.
fn fold(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match digit_value(c) {
            Some(d) => char::from_digit(d, 10).expect("decimal digit"),
            None => c,
        })
        .collect()
}

/// A number in the client's notation: an integer part that is either
/// ungrouped or grouped in threes by commas (never a mixture, never a
/// short group), then an optional fraction. Returns the value.
fn grouped_number(text: &str) -> Option<f64> {
    let (whole, fraction) = match text.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (text, None),
    };
    if let Some(f) = fraction {
        if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let groups: Vec<&str> = whole.split(',').collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if groups.len() > 1 {
        // Leading group one to three digits and not a bare zero run;
        // every later group exactly three. This is the rule that turns
        // a collapsed digit (`1,44.77`) into a refusal.
        if groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
    }
    let mut plain: String = groups.concat();
    if let Some(f) = fraction {
        plain.push('.');
        plain.push_str(f);
    }
    plain.parse().ok()
}

/// A pager's `page / count`: both plain positive integers without
/// leading zeros, the page no larger than the count. A page past the
/// count is a misread digit, not a position.
pub fn pager(raw: &str) -> Option<(u32, u32)> {
    let text = fold(raw);
    // The recogniser's alphabet has a fullwidth slash as well.
    let (page, count) = text.split_once(['/', '\u{ff0f}'])?;
    let number = |part: &str| -> Option<u32> {
        if part.is_empty() || part.starts_with('0') || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (page, count) = (number(page)?, number(count)?);
    (page <= count).then_some((page, count))
}

/// Check `raw` against `grammar`. `None` is a refusal.
pub fn parse(grammar: Grammar, raw: &str) -> Option<Parsed> {
    let text = fold(raw);
    if text.is_empty() {
        return None;
    }
    match grammar {
        Grammar::PedAmount => {
            let number = grouped_number(text.strip_suffix("PED")?)?;
            let digits = &text[..text.len() - 3];
            Some(Parsed {
                text: format!("{digits} PED"),
                number: Some(number),
            })
        }
        Grammar::GroupedDecimal => {
            let number = grouped_number(&text)?;
            Some(Parsed {
                text,
                number: Some(number),
            })
        }
        Grammar::Integer => {
            if !text.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let number = text.parse().ok()?;
            Some(Parsed {
                text,
                number: Some(number),
            })
        }
        Grammar::Percent => {
            let number = grouped_number(text.strip_suffix('%')?)?;
            Some(Parsed {
                text,
                number: Some(number),
            })
        }
        Grammar::Pager => {
            let (page, count) = pager(&text)?;
            Some(Parsed {
                text: format!("{page} / {count}"),
                number: Some(page as f64),
            })
        }
        Grammar::Text => Some(Parsed {
            text: raw.trim().to_string(),
            number: None,
        }),
    }
}

/// How wide the client's numerals are, in units of a digit's height.
///
/// The grammar cannot see a digit lost from a fraction, or from an
/// ungrouped number: `8,429.5` is as well formed as `8,429.55`. The
/// lettering itself can: the client's numerals have fixed advances, so
/// the text a value is read as predicts how wide its ink must be, and a
/// reading a digit short is most of a digit too narrow.
#[derive(Debug, Clone, Deserialize)]
pub struct NumeralMetrics {
    /// A pixel is ink when its brightest channel reaches this.
    pub min_ink: u8,
    /// Rows holding at least this share of the busiest row's ink are
    /// the digits' body; a comma's tail below them is not.
    pub body_row_share: f32,
    /// Advance of a digit other than one.
    pub digit: f32,
    /// Advance of the digit one.
    pub one: f32,
    /// Advance of a comma.
    pub comma: f32,
    /// Advance of a full stop.
    pub period: f32,
    /// Width of ` PED` after an amount, space included.
    pub ped_suffix: f32,
    /// What the first and last glyph's side bearings take off the sum.
    pub bias: f32,
    /// Largest disagreement accepted. Under half a digit.
    pub tolerance: f32,
}

/// The width `text` should have, in digit heights; `None` when it holds
/// a character the metrics do not cover.
pub fn expected_width(text: &str, metrics: &NumeralMetrics) -> Option<f32> {
    let (number, suffix) = match text.strip_suffix(" PED") {
        Some(number) => (number, metrics.ped_suffix),
        None => (text, 0.0),
    };
    let mut units = metrics.bias + suffix;
    for c in number.chars() {
        units += match c {
            '1' => metrics.one,
            '0' | '2'..='9' => metrics.digit,
            ',' => metrics.comma,
            '.' => metrics.period,
            _ => return None,
        };
    }
    Some(units)
}

/// Width and body height of the ink in `rect`.
fn ink_size<P: Pixels>(image: &P, rect: Rect, metrics: &NumeralMetrics) -> Option<(i32, i32)> {
    let rect = rect.intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1))?;
    let ink = |x: i32, y: i32| {
        let [r, g, b] = image.rgb(x, y);
        r.max(g).max(b) >= metrics.min_ink
    };
    let mut row_ink = Vec::with_capacity(rect.height() as usize);
    let (mut left, mut right) = (i32::MAX, i32::MIN);
    for y in rect.y0..=rect.y1 {
        let mut count = 0;
        for x in rect.x0..=rect.x1 {
            if ink(x, y) {
                count += 1;
                left = left.min(x);
                right = right.max(x);
            }
        }
        row_ink.push(count);
    }
    let busiest = *row_ink.iter().max()?;
    if busiest == 0 {
        return None;
    }
    let floor = (busiest as f32 * metrics.body_row_share).ceil() as i32;
    let top = row_ink.iter().position(|&c| c >= floor)?;
    let bottom = row_ink.iter().rposition(|&c| c >= floor)?;
    Some((right - left + 1, (bottom - top + 1) as i32))
}

/// Whether the ink in `rect` is as wide as `text` says it should be.
/// Text the metrics do not cover cannot be checked and passes; a
/// rectangle with no ink fails.
pub fn width_agrees<P: Pixels>(
    image: &P,
    rect: Rect,
    text: &str,
    metrics: &NumeralMetrics,
) -> bool {
    let Some(expected) = expected_width(text, metrics) else {
        return true;
    };
    let Some((width, height)) = ink_size(image, rect, metrics) else {
        return false;
    };
    (width as f32 / height as f32 - expected).abs() <= metrics.tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(grammar: Grammar, raw: &str) -> Option<f64> {
        parse(grammar, raw).and_then(|p| p.number)
    }

    #[test]
    fn ped_amounts_read_with_or_without_the_space() {
        assert_eq!(number(Grammar::PedAmount, "27.47542 PED"), Some(27.47542));
        assert_eq!(number(Grammar::PedAmount, "27.47542PED"), Some(27.47542));
        assert_eq!(number(Grammar::PedAmount, "0.00 PED"), Some(0.0));
        assert_eq!(number(Grammar::PedAmount, "1,234.5 PED"), Some(1234.5));
        assert_eq!(
            parse(Grammar::PedAmount, "27.47542PED").map(|p| p.text),
            Some("27.47542 PED".into())
        );
    }

    #[test]
    fn an_amount_without_its_unit_or_with_noise_is_refused() {
        for raw in [
            "27.47542",
            "27.47542 PE",
            "PED",
            "2 7.4x PED",
            "27..5 PED",
            ".5 PED",
            "27. PED",
            "",
        ] {
            assert_eq!(parse(Grammar::PedAmount, raw), None, "{raw:?}");
        }
    }

    #[test]
    fn a_collapsed_digit_breaks_the_grouping_and_is_refused() {
        assert_eq!(number(Grammar::GroupedDecimal, "1,444.77"), Some(1444.77));
        assert_eq!(parse(Grammar::GroupedDecimal, "1,44.77"), None);
        assert_eq!(parse(Grammar::GroupedDecimal, "12,4526.8"), None);
        assert_eq!(parse(Grammar::GroupedDecimal, "1234,567"), None);
        assert_eq!(number(Grammar::GroupedDecimal, "12,452.68"), Some(12452.68));
        assert_eq!(number(Grammar::GroupedDecimal, "1"), Some(1.0));
        assert_eq!(number(Grammar::GroupedDecimal, "198259"), Some(198259.0));
    }

    #[test]
    fn integers_and_percentages() {
        assert_eq!(number(Grammar::Integer, "67739"), Some(67739.0));
        assert_eq!(number(Grammar::Integer, "６７７３９"), Some(67739.0));
        assert_eq!(parse(Grammar::Integer, "67,739"), None);
        assert_eq!(parse(Grammar::Integer, "L0N"), None);
        assert_eq!(number(Grammar::Percent, "201.92%"), Some(201.92));
        assert_eq!(parse(Grammar::Percent, "201.92"), None);
    }

    #[test]
    fn a_pager_reads_as_page_of_count_or_is_refused() {
        let page = |raw: &str| parse(Grammar::Pager, raw).map(|p| p.text);
        assert_eq!(page("2 / 13"), Some("2 / 13".into()));
        assert_eq!(page("13/13"), Some("13 / 13".into()));
        assert_eq!(page("１ / １３"), Some("1 / 13".into()));
        assert_eq!(
            parse(Grammar::Pager, "2 / 13").and_then(|p| p.number),
            Some(2.0)
        );
        for raw in [
            "14 / 13", "0 / 13", "2 /", "/ 13", "2 13", "2 / 1x", "02 / 13", "2 // 13", "",
        ] {
            assert_eq!(parse(Grammar::Pager, raw), None, "{raw:?}");
        }
    }

    fn metrics() -> NumeralMetrics {
        crate::game_ui::test_support::bundled_profile()
            .read
            .numerals
    }

    /// A block of ink as wide as `text` should be at digit height `h`.
    fn lettering(text: &str, h: i32) -> (crate::skill_panel::BgrImage, Rect) {
        use crate::game_ui::test_support::{blank, fill};
        let width = (expected_width(text, &metrics()).expect("covered") * h as f32).round() as i32;
        let mut image = blank(600, 80, [8, 11, 11]);
        let rect = Rect::new(20, 20, 20 + width - 1, 20 + h - 1);
        fill(&mut image, rect, [200, 200, 200]);
        (image, rect.padded(4))
    }

    #[test]
    fn a_reading_a_digit_short_is_too_narrow_for_its_ink() {
        let m = metrics();
        for h in [10, 14, 28] {
            let (image, rect) = lettering("8,429.55", h);
            assert!(width_agrees(&image, rect, "8,429.55", &m), "height {h}");
            assert!(!width_agrees(&image, rect, "8,429.5", &m), "height {h}");
            assert!(!width_agrees(&image, rect, "8,429.555", &m), "height {h}");
            let (image, rect) = lettering("27.47542 PED", h);
            assert!(width_agrees(&image, rect, "27.47542 PED", &m), "height {h}");
            assert!(!width_agrees(&image, rect, "27.4742 PED", &m), "height {h}");
        }
    }

    #[test]
    fn even_the_narrowest_glyphs_are_told_apart() {
        let m = metrics();
        let (image, rect) = lettering("111", 10);
        assert!(width_agrees(&image, rect, "111", &m));
        assert!(!width_agrees(&image, rect, "11", &m));
    }

    #[test]
    fn text_the_metrics_do_not_cover_passes_and_no_ink_fails() {
        let m = metrics();
        let (image, rect) = lettering("123", 10);
        assert!(width_agrees(&image, rect, "Newbie", &m));
        assert!(!width_agrees(
            &image,
            Rect::new(300, 50, 400, 70),
            "123",
            &m
        ));
    }
}
