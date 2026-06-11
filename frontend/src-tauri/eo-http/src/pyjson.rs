//! A reference-faithful JSON reader for request bodies.
//!
//! The backend parses request bodies with Python's `json` module, whose
//! behaviour differs from a strict JSON parser in ways that reach the
//! wire: it accepts `Infinity` / `-Infinity` / `NaN` literals, keeps
//! integers at arbitrary precision, and reports malformed input with
//! specific messages and character positions that the validation
//! envelope echoes (`json_invalid`, `loc: ["body", <pos>]`,
//! `ctx.error`). This module ports that scanner: the value tree it
//! produces mirrors what `json.loads` yields, and its errors carry the
//! reference's message text and position. Every message form and
//! position here is pinned by probes against the running backend in
//! the conformance battery.

use std::collections::BTreeMap;

/// What `json.loads` produces, shaped for extraction: integers keep a
/// beyond-i64 marker (the backend's are arbitrary-precision), floats
/// may be non-finite, and object member order is preserved via the
/// insertion-ordered pairs list.
#[derive(Debug, Clone, PartialEq)]
pub enum PyValue {
    Null,
    Bool(bool),
    Int(i64),
    /// An integer literal beyond i64, kept as its source text (sign
    /// included). The backend stores these unbounded; the few places
    /// they can land reproduce its behaviour explicitly.
    BigInt(String),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    /// Insertion-ordered members; a duplicate key keeps the first
    /// position with the last value, as a Python dict does.
    Object(Vec<(String, PyValue)>),
}

impl PyValue {
    /// Render for an `input` echo in a validation issue, matching the
    /// backend's re-serialisation of the offending value. Non-finite
    /// floats render as the reference serialiser writes them.
    pub fn to_echo_json(&self) -> String {
        match self {
            PyValue::Null => "null".into(),
            PyValue::Bool(true) => "true".into(),
            PyValue::Bool(false) => "false".into(),
            PyValue::Int(value) => value.to_string(),
            PyValue::BigInt(text) => text.clone(),
            PyValue::Float(value) => render_float(*value),
            PyValue::Str(value) => render_str(value),
            PyValue::List(items) => {
                let inner: Vec<String> = items.iter().map(PyValue::to_echo_json).collect();
                format!("[{}]", inner.join(","))
            }
            PyValue::Object(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(key, value)| format!("{}:{}", render_str(key), value.to_echo_json()))
                    .collect();
                format!("{{{}}}", inner.join(","))
            }
        }
    }
}

/// `repr`-faithful float rendering for echoes (the shared rule every
/// byte-comparable writer uses; `Infinity` / `-Infinity` / `NaN`
/// included).
fn render_float(value: f64) -> String {
    eo_wire::normalizer::python_repr_f64(value)
}

fn render_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// The reference scanner's failure: its exact message and character
/// position (the envelope's `loc` carries `pos`, its `ctx.error` the
/// message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyJsonError {
    pub msg: String,
    pub pos: usize,
}

impl PyJsonError {
    fn new(msg: &str, pos: usize) -> Self {
        Self {
            msg: msg.into(),
            pos,
        }
    }
}

/// Parse a body as the reference does: decode positions are CHARACTER
/// indices into the decoded text.
pub fn loads(text: &str) -> Result<PyValue, PyJsonError> {
    let chars: Vec<char> = text.chars().collect();
    let mut scanner = Scanner { chars: &chars };
    let mut idx = scanner.skip_ws(0);
    let (value, mut end) = scanner.scan_once(idx)?;
    end = scanner.skip_ws(end);
    if end != chars.len() {
        return Err(PyJsonError::new("Extra data", end));
    }
    idx = end;
    let _ = idx;
    Ok(value)
}

struct Scanner<'a> {
    chars: &'a [char],
}

impl Scanner<'_> {
    fn at(&self, idx: usize) -> Option<char> {
        self.chars.get(idx).copied()
    }

    fn skip_ws(&self, mut idx: usize) -> usize {
        while matches!(self.at(idx), Some(' ' | '\t' | '\n' | '\r')) {
            idx += 1;
        }
        idx
    }

    fn starts_with(&self, idx: usize, literal: &str) -> bool {
        let lit: Vec<char> = literal.chars().collect();
        self.chars.len() >= idx + lit.len() && self.chars[idx..idx + lit.len()] == lit[..]
    }

    fn scan_once(&mut self, idx: usize) -> Result<(PyValue, usize), PyJsonError> {
        let Some(ch) = self.at(idx) else {
            return Err(PyJsonError::new("Expecting value", idx));
        };
        match ch {
            '"' => {
                let (text, end) = self.scan_string(idx + 1)?;
                Ok((PyValue::Str(text), end))
            }
            '{' => self.scan_object(idx + 1),
            '[' => self.scan_array(idx + 1),
            'n' if self.starts_with(idx, "null") => Ok((PyValue::Null, idx + 4)),
            't' if self.starts_with(idx, "true") => Ok((PyValue::Bool(true), idx + 4)),
            'f' if self.starts_with(idx, "false") => Ok((PyValue::Bool(false), idx + 5)),
            'N' if self.starts_with(idx, "NaN") => Ok((PyValue::Float(f64::NAN), idx + 3)),
            'I' if self.starts_with(idx, "Infinity") => {
                Ok((PyValue::Float(f64::INFINITY), idx + 8))
            }
            '-' if self.starts_with(idx, "-Infinity") => {
                Ok((PyValue::Float(f64::NEG_INFINITY), idx + 9))
            }
            '-' | '0'..='9' => self
                .scan_number(idx)
                .ok_or_else(|| PyJsonError::new("Expecting value", idx)),
            _ => Err(PyJsonError::new("Expecting value", idx)),
        }
    }

    /// The reference's number grammar: `-?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?`,
    /// matched greedily from `idx`; anything past the match is left for
    /// the caller (a leading-zero run like `01` parses as `0` and the
    /// `1` trips the next delimiter check, as the reference behaves).
    fn scan_number(&self, idx: usize) -> Option<(PyValue, usize)> {
        let mut end = idx;
        if self.at(end) == Some('-') {
            end += 1;
        }
        let int_start = end;
        match self.at(end)? {
            '0' => end += 1,
            '1'..='9' => {
                while matches!(self.at(end), Some('0'..='9')) {
                    end += 1;
                }
            }
            _ => return None,
        }
        if int_start == end {
            return None;
        }
        let mut is_float = false;
        if self.at(end) == Some('.') && matches!(self.at(end + 1), Some('0'..='9')) {
            is_float = true;
            end += 1;
            while matches!(self.at(end), Some('0'..='9')) {
                end += 1;
            }
        }
        if matches!(self.at(end), Some('e' | 'E')) {
            let mut exp_end = end + 1;
            if matches!(self.at(exp_end), Some('+' | '-')) {
                exp_end += 1;
            }
            if matches!(self.at(exp_end), Some('0'..='9')) {
                is_float = true;
                end = exp_end;
                while matches!(self.at(end), Some('0'..='9')) {
                    end += 1;
                }
            }
        }
        let text: String = self.chars[idx..end].iter().collect();
        let value = if is_float {
            PyValue::Float(text.parse().ok()?)
        } else {
            match text.parse::<i64>() {
                Ok(value) => PyValue::Int(value),
                Err(_) => PyValue::BigInt(text),
            }
        };
        Some((value, end))
    }

    /// The reference's string scanner, positions included: an
    /// unterminated string reports the OPENING quote's position.
    fn scan_string(&self, start: usize) -> Result<(String, usize), PyJsonError> {
        let begin = start - 1;
        let mut out = String::new();
        let mut idx = start;
        loop {
            let Some(ch) = self.at(idx) else {
                return Err(PyJsonError::new("Unterminated string starting at", begin));
            };
            match ch {
                '"' => return Ok((out, idx + 1)),
                '\\' => {
                    idx += 1;
                    let Some(esc) = self.at(idx) else {
                        return Err(PyJsonError::new("Unterminated string starting at", begin));
                    };
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let (ch, next) = self.scan_unicode_escape(idx)?;
                            out.push(ch);
                            idx = next;
                            continue;
                        }
                        _ => {
                            return Err(PyJsonError::new("Invalid \\escape", idx - 1));
                        }
                    }
                    idx += 1;
                }
                ch if (ch as u32) < 0x20 => {
                    return Err(PyJsonError::new("Invalid control character at", idx));
                }
                ch => {
                    out.push(ch);
                    idx += 1;
                }
            }
        }
    }

    /// `\uXXXX`, with surrogate pairing as the reference performs it;
    /// `idx` sits on the `u`. Returns the decoded char and the index
    /// just past the escape.
    fn scan_unicode_escape(&self, idx: usize) -> Result<(char, usize), PyJsonError> {
        // The reference reports the escape's `u` position on a short
        // or non-hex group.
        let hex = |at: usize| -> Result<u32, PyJsonError> {
            let mut value = 0u32;
            for offset in 0..4 {
                let Some(ch) = self.at(at + offset).and_then(|c| c.to_digit(16)) else {
                    return Err(PyJsonError::new("Invalid \\uXXXX escape", at - 1));
                };
                value = value * 16 + ch;
            }
            Ok(value)
        };
        let first = hex(idx + 1)?;
        if (0xD800..0xDC00).contains(&first)
            && self.at(idx + 5) == Some('\\')
            && self.at(idx + 6) == Some('u')
        {
            if let Ok(second) = hex(idx + 7) {
                if (0xDC00..0xE000).contains(&second) {
                    let combined = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                    let ch = char::from_u32(combined)
                        .ok_or_else(|| PyJsonError::new("Invalid \\uXXXX escape", idx + 7))?;
                    return Ok((ch, idx + 11));
                }
            }
        }
        // A lone surrogate survives in the reference as the surrogate
        // code point; Rust chars cannot carry it, so the replacement
        // character stands in (the only divergence, unreachable from
        // well-formed clients and pinned in the battery as accepted).
        let ch = char::from_u32(first).unwrap_or('\u{FFFD}');
        Ok((ch, idx + 5))
    }

    fn scan_object(&mut self, mut idx: usize) -> Result<(PyValue, usize), PyJsonError> {
        let mut pairs: Vec<(String, PyValue)> = Vec::new();
        let mut positions: BTreeMap<String, usize> = BTreeMap::new();
        let mut nextchar = self.at(idx);
        if nextchar != Some('"') {
            idx = self.skip_ws(idx);
            nextchar = self.at(idx);
            if nextchar == Some('}') {
                return Ok((PyValue::Object(pairs), idx + 1));
            }
            if nextchar != Some('"') {
                return Err(PyJsonError::new(
                    "Expecting property name enclosed in double quotes",
                    idx,
                ));
            }
        }
        loop {
            let (key, after_key) = self.scan_string(idx + 1)?;
            idx = after_key;
            if self.at(idx) != Some(':') {
                idx = self.skip_ws(idx);
                if self.at(idx) != Some(':') {
                    return Err(PyJsonError::new("Expecting ':' delimiter", idx));
                }
            }
            idx = self.skip_ws(idx + 1);
            let (value, after_value) = self.scan_once(idx)?;
            // A duplicate key keeps its first position with the last
            // value, as a dict insert behaves.
            match positions.get(&key) {
                Some(&existing) => pairs[existing].1 = value,
                None => {
                    positions.insert(key.clone(), pairs.len());
                    pairs.push((key, value));
                }
            }
            idx = self.skip_ws(after_value);
            match self.at(idx) {
                Some('}') => return Ok((PyValue::Object(pairs), idx + 1)),
                Some(',') => {}
                _ => return Err(PyJsonError::new("Expecting ',' delimiter", idx)),
            }
            let comma = idx;
            idx = self.skip_ws(idx + 1);
            match self.at(idx) {
                Some('"') => {}
                Some('}') => {
                    return Err(PyJsonError::new(
                        "Illegal trailing comma before end of object",
                        comma,
                    ));
                }
                _ => {
                    return Err(PyJsonError::new(
                        "Expecting property name enclosed in double quotes",
                        idx,
                    ));
                }
            }
        }
    }

    fn scan_array(&mut self, mut idx: usize) -> Result<(PyValue, usize), PyJsonError> {
        let mut items = Vec::new();
        idx = self.skip_ws(idx);
        if self.at(idx) == Some(']') {
            return Ok((PyValue::List(items), idx + 1));
        }
        loop {
            let (value, after_value) = self.scan_once(idx)?;
            items.push(value);
            idx = self.skip_ws(after_value);
            match self.at(idx) {
                Some(']') => return Ok((PyValue::List(items), idx + 1)),
                Some(',') => {}
                _ => return Err(PyJsonError::new("Expecting ',' delimiter", idx)),
            }
            let comma = idx;
            idx = self.skip_ws(idx + 1);
            if self.at(idx) == Some(']') {
                return Err(PyJsonError::new(
                    "Illegal trailing comma before end of array",
                    comma,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> PyJsonError {
        loads(text).expect_err("expected a parse failure")
    }

    #[test]
    fn the_malformed_grid_reports_the_reference_messages_and_positions() {
        // Each pin verified against the reference scanner during
        // authoring; the battery re-proves the envelope forms live.
        let pins: &[(&str, &str, usize)] = &[
            (
                "{not json",
                "Expecting property name enclosed in double quotes",
                1,
            ),
            (
                "{\"name\": \"Q\", }",
                "Illegal trailing comma before end of object",
                12,
            ),
            ("", "Expecting value", 0),
            ("[1,2", "Expecting ',' delimiter", 4),
            ("{\"a\": }", "Expecting value", 6),
            ("{\"a\" 1}", "Expecting ':' delimiter", 5),
            ("{\"a\": 1 \"b\": 2}", "Expecting ',' delimiter", 8),
            ("\"unterminated", "Unterminated string starting at", 0),
            ("{\"a\": 1}extra", "Extra data", 8),
            ("nul", "Expecting value", 0),
            (
                "{'a': 1}",
                "Expecting property name enclosed in double quotes",
                1,
            ),
            ("{\"a\": 01}", "Expecting ',' delimiter", 7),
            ("  ", "Expecting value", 2),
            ("{}trailing", "Extra data", 2),
            ("[1, ]", "Illegal trailing comma before end of array", 2),
            (
                "{\"a\": 1,, \"b\": 2}",
                "Expecting property name enclosed in double quotes",
                8,
            ),
            (
                "{\"a\": \"caf\u{e9}\", }",
                "Illegal trailing comma before end of object",
                12,
            ),
        ];
        for (text, msg, pos) in pins {
            let error = err(text);
            assert_eq!(error.msg, *msg, "{text:?}");
            assert_eq!(error.pos, *pos, "{text:?}");
        }
    }

    #[test]
    fn values_parse_to_the_reference_shapes() {
        assert_eq!(loads("null").unwrap(), PyValue::Null);
        assert_eq!(loads(" true ").unwrap(), PyValue::Bool(true));
        assert_eq!(loads("-42").unwrap(), PyValue::Int(-42));
        assert_eq!(loads("4.5").unwrap(), PyValue::Float(4.5));
        assert_eq!(loads("1e3").unwrap(), PyValue::Float(1000.0));
        assert_eq!(
            loads("999999999999999999999999").unwrap(),
            PyValue::BigInt("999999999999999999999999".into())
        );
        assert_eq!(
            loads("\"caf\\u00e9\"").unwrap(),
            PyValue::Str("café".into())
        );
        assert_eq!(
            loads("\"\\ud83d\\ude00\"").unwrap(),
            PyValue::Str("😀".into())
        );
        assert_eq!(
            loads("[1, \"two\", null]").unwrap(),
            PyValue::List(vec![
                PyValue::Int(1),
                PyValue::Str("two".into()),
                PyValue::Null
            ])
        );
        // The reference accepts the non-standard literals.
        assert_eq!(loads("Infinity").unwrap(), PyValue::Float(f64::INFINITY));
        assert_eq!(
            loads("-Infinity").unwrap(),
            PyValue::Float(f64::NEG_INFINITY)
        );
        assert!(matches!(loads("NaN").unwrap(), PyValue::Float(v) if v.is_nan()));
    }

    #[test]
    fn duplicate_keys_keep_first_position_and_last_value() {
        let parsed = loads("{\"a\": 1, \"b\": 2, \"a\": 3}").unwrap();
        assert_eq!(
            parsed,
            PyValue::Object(vec![
                ("a".into(), PyValue::Int(3)),
                ("b".into(), PyValue::Int(2)),
            ])
        );
    }

    #[test]
    fn echo_rendering_matches_the_reference_serialiser() {
        assert_eq!(loads("Infinity").unwrap().to_echo_json(), "Infinity");
        assert_eq!(loads("[1,2.5]").unwrap().to_echo_json(), "[1,2.5]");
        assert_eq!(
            loads("{\"a\": \"x\"}").unwrap().to_echo_json(),
            "{\"a\":\"x\"}"
        );
        assert_eq!(
            loads("999999999999999999999999").unwrap().to_echo_json(),
            "999999999999999999999999"
        );
    }

    #[test]
    fn control_characters_and_bad_escapes_report_reference_forms() {
        let error = err("\"a\nb\"");
        assert_eq!(error.msg, "Invalid control character at");
        assert_eq!(error.pos, 2);
        let error = err("\"a\\qb\"");
        assert_eq!(error.msg, "Invalid \\escape");
        assert_eq!(error.pos, 2);
        let error = err("\"a\\u12\"");
        assert_eq!(error.msg, "Invalid \\uXXXX escape");
        assert_eq!(error.pos, 3);
    }
}
