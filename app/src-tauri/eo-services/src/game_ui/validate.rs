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
}
