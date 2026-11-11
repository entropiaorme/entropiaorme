//! Refusing what the recogniser got wrong.
//!
//! The recogniser reports a confidence, and it cannot be trusted alone:
//! it has read `1,444.77` as `1,44.77` at 96%, collapsing a repeated
//! digit. So a value is accepted on what it says, not on how sure the
//! recogniser was. Each field declares the shape its text must have; a
//! read that does not have exactly that shape is refused, and a refused
//! read is an empty field the user fills in, never a guess.

use serde::Deserialize;

use super::geometry::{Pixels, Rect};

/// The decimal value of a digit character the recogniser can emit: its
/// decode alphabet carries exactly the ASCII and fullwidth digit forms,
/// and a number read in either converts by value.
pub(crate) fn digit_value(ch: char) -> Option<u32> {
    match ch {
        '0'..='9' => Some(ch as u32 - '0' as u32),
        '\u{ff10}'..='\u{ff19}' => Some(ch as u32 - 0xff10),
        _ => None,
    }
}

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
/// short group), with no leading zero unless it is a lone `0`, then an
/// optional fraction. Returns the value.
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
    // The client never pads an amount with zeros: `0.05` and `0` are
    // printed, `0123.45` and `0,123.45` are not. A leading zero is a
    // misread digit (an `8` read as `0` has the same width), not a value.
    if groups[0].starts_with('0') && whole != "0" {
        return None;
    }
    if groups.len() > 1 {
        // Leading group one to three digits; every later group exactly
        // three. This is the rule that turns a collapsed digit
        // (`1,44.77`) into a refusal.
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

/// A whole number printed after its own label, as the client prints a
/// position: `LON 67739`. The text must begin with exactly `label`, then
/// at most one stretch of space, then one unbroken run of between
/// `digits[0]` and `digits[1]` digits without a leading zero, and
/// nothing else. A different label (`L0N`, `LAT` where `LON` is
/// wanted), a letter or a space among the digits, a second run of
/// digits and any punctuation are all things the client never draws
/// there, so each is a refusal rather than something to tidy away. The
/// parsed text is the canonical `LON 67739`.
pub fn labelled_integer(raw: &str, label: &str, digits: [usize; 2]) -> Option<Parsed> {
    // Fullwidth digits fold to ASCII; spaces stay, because a space
    // inside the digits is a refusal, not noise.
    let text: String = raw
        .trim()
        .chars()
        .map(|c| match digit_value(c) {
            Some(d) => char::from_digit(d, 10).expect("decimal digit"),
            None => c,
        })
        .collect();
    let number = text.strip_prefix(label)?.trim_start();
    let count = number.chars().count();
    if count < digits[0]
        || count > digits[1]
        || number.starts_with('0')
        || !number.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(Parsed {
        text: format!("{label} {number}"),
        number: Some(number.parse::<u64>().ok()? as f64),
    })
}

/// Distances printed one after another, each a whole number without a
/// leading zero followed by `unit`, as the client labels the radar's
/// rings: `50 m`, or `50 m100 m` where two labels touch. Spaces between
/// a number and its unit, or between two distances, are allowed;
/// anything else, including a number without its unit, is a refusal.
pub fn distance_labels(raw: &str, unit: &str) -> Option<Vec<u32>> {
    if unit.is_empty() {
        return None;
    }
    let text: String = raw
        .chars()
        .map(|c| match digit_value(c) {
            Some(d) => char::from_digit(d, 10).expect("decimal digit"),
            None => c,
        })
        .collect();
    let mut rest = text.trim();
    let mut out = Vec::new();
    while !rest.is_empty() {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.starts_with('0') {
            return None;
        }
        out.push(digits.parse().ok()?);
        rest = rest[digits.len()..]
            .trim_start()
            .strip_prefix(unit)?
            .trim_start();
    }
    (!out.is_empty()).then_some(out)
}

/// The amount of a [`Grammar::PedAmount`] reading's canonical text,
/// without its unit: `27.47542 PED` is `27.47542`. Other text is returned
/// as it is.
pub fn without_unit(text: &str) -> &str {
    text.strip_suffix(" PED").unwrap_or(text)
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

    #[test]
    fn ascii_and_fullwidth_digits_convert_by_value() {
        assert_eq!(digit_value('0'), Some(0));
        assert_eq!(digit_value('7'), Some(7));
        assert_eq!(digit_value('\u{ff10}'), Some(0));
        assert_eq!(digit_value('\u{ff19}'), Some(9));
        for other in ['a', ' ', ',', '.', '\u{0660}', '\u{ff1a}'] {
            assert_eq!(digit_value(other), None, "{other:?}");
        }
    }

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
    fn a_leading_zero_is_refused_unless_the_whole_part_is_zero() {
        for raw in ["0,123.45", "0123.45", "00.5", "007", "0,000", "00"] {
            assert_eq!(parse(Grammar::GroupedDecimal, raw), None, "{raw:?}");
        }
        assert_eq!(parse(Grammar::PedAmount, "0,123.45 PED"), None);
        assert_eq!(parse(Grammar::PedAmount, "0123.45 PED"), None);
        assert_eq!(parse(Grammar::Percent, "05%"), None);
        assert_eq!(number(Grammar::GroupedDecimal, "0.05"), Some(0.05));
        assert_eq!(number(Grammar::GroupedDecimal, "0"), Some(0.0));
        assert_eq!(number(Grammar::GroupedDecimal, "10"), Some(10.0));
        assert_eq!(number(Grammar::GroupedDecimal, "1,000"), Some(1000.0));
        assert_eq!(number(Grammar::PedAmount, "0.00 PED"), Some(0.0));
    }

    #[test]
    fn the_unit_comes_off_an_amount_and_nothing_else() {
        assert_eq!(without_unit("1,234.56789 PED"), "1,234.56789");
        assert_eq!(without_unit("2.20 PED"), "2.20");
        assert_eq!(without_unit("201.92%"), "201.92%");
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

    #[test]
    fn a_labelled_integer_reads_only_as_its_own_label_and_one_run_of_digits() {
        let read = |raw: &str| labelled_integer(raw, "LON", [4, 6]).map(|p| p.text);
        assert_eq!(read("LON 67739"), Some("LON 67739".into()));
        assert_eq!(read("  LON 67739 "), Some("LON 67739".into()));
        assert_eq!(read("LON67739"), Some("LON 67739".into()));
        assert_eq!(read("LON ６７７３９"), Some("LON 67739".into()));
        assert_eq!(read("LON 1234"), Some("LON 1234".into()));
        assert_eq!(read("LON 123456"), Some("LON 123456".into()));
        assert_eq!(
            labelled_integer("LAT 87740", "LAT", [4, 6]).and_then(|p| p.number),
            Some(87740.0)
        );
        for raw in [
            "L0N 67739",
            "LoN 67739",
            "LAT 87740",
            "LONG 67739",
            "ON 67739",
            "LON",
            "LON 123",
            "LON 1234567",
            "LON 067739",
            "LON 677 39",
            "LON 677a9",
            "LON 6773g",
            "LON 67739.",
            "LON: 67739",
            "LON 67,739",
            "LON 67739 87740",
            "",
        ] {
            assert_eq!(read(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn ring_distances_read_as_numbers_each_with_its_unit() {
        let read = |raw: &str| distance_labels(raw, "m");
        assert_eq!(read("50 m"), Some(vec![50]));
        assert_eq!(read("100m"), Some(vec![100]));
        assert_eq!(read("50 m100 m"), Some(vec![50, 100]));
        assert_eq!(read(" 50m 100m "), Some(vec![50, 100]));
        assert_eq!(read("２００ m"), Some(vec![200]));
        for raw in [
            "", "m", "50", "100", "Eom", "E0m", "50 ml", "050 m", "50 m100", "5O m", "50 km",
            "50.5 m", "50 m E",
        ] {
            assert_eq!(read(raw), None, "{raw:?}");
        }
    }

    fn metrics() -> NumeralMetrics {
        crate::game_ui::test_support::bundled_profile()
            .read
            .numerals
    }

    /// A block of ink as wide as `text` should be at digit height `h`.
    fn lettering(text: &str, h: i32) -> (crate::game_ui::geometry::BgrImage, Rect) {
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
