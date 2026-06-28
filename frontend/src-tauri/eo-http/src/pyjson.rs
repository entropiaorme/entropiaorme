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
//! position here is pinned by a committed golden, asserted by the
//! hermetic conformance tests.

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
    /// A string whose source carried a lone surrogate escape. The
    /// reference's strings hold the surrogate code point and crash at
    /// consumption (binding or response rendering); this variant
    /// carries the lossy text plus the first surrogate's code and
    /// character position so each consumption site can reproduce the
    /// reference's reply (the storage-crash 500, or calibrate's
    /// ValueError 400) without the crash. An IGNORED tainted value is
    /// harmless on both arms.
    TaintedStr {
        lossy: String,
        code: u32,
        position: usize,
        /// The length of the leading run of consecutive lone
        /// surrogates at `position` (the reference's codec message
        /// uses a singular form for one and a position range for a
        /// run).
        run: usize,
    },
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
            // Only reachable when a render check was bypassed; the
            // lossy text stands in.
            PyValue::TaintedStr { lossy, .. } => render_str(lossy),
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

    /// Convert to a `serde_json::Value` for a field carried verbatim into a
    /// writer that re-normalises it (the settings PATCH carries `hotbar` and
    /// `trifecta_presets` this way). Returns `None` for a value `serde_json`
    /// cannot represent (a non-finite float, a beyond-`i64` integer, or a
    /// lone-surrogate-tainted string); none of these can occur in the
    /// carried container fields, so `None` means "reproduce the reference's
    /// unrenderable-input reply" at the call site.
    pub fn to_serde_value(&self) -> Option<serde_json::Value> {
        use serde_json::Value;
        Some(match self {
            PyValue::Null => Value::Null,
            PyValue::Bool(value) => Value::Bool(*value),
            PyValue::Int(value) => Value::from(*value),
            PyValue::BigInt(_) => return None,
            PyValue::Float(value) => serde_json::Number::from_f64(*value).map(Value::Number)?,
            PyValue::Str(value) => Value::String(value.clone()),
            PyValue::TaintedStr { .. } => return None,
            PyValue::List(items) => Value::Array(
                items
                    .iter()
                    .map(PyValue::to_serde_value)
                    .collect::<Option<Vec<_>>>()?,
            ),
            PyValue::Object(pairs) => {
                let mut map = serde_json::Map::new();
                for (key, value) in pairs {
                    map.insert(key.clone(), value.to_serde_value()?);
                }
                Value::Object(map)
            }
        })
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

/// How a parse fails: malformed input (the reference's message and
/// position, echoed in the validation envelope), nesting beyond the
/// depth cap (the reference's HTTP layer answers its generic
/// body-parse 400 once ITS parser gives up), undecodable bytes (the
/// same 400: the reference's decode step fails before its scanner), or
/// a lone surrogate escape in an object KEY (tainted VALUES parse into
/// [`PyValue::TaintedStr`] and resolve at consumption; a tainted key
/// cannot, so it answers the serialiser-crash 500 outright, a
/// divergence-register residual for keys in ignored fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyJsonFailure {
    Malformed(PyJsonError),
    TooDeep,
    Undecodable,
    Unrenderable,
}

/// Container nesting beyond this answers the generic body-parse 400.
/// The reference's reply ladder for deep bodies, probed through the
/// route: echoed envelopes render to depth 984 and crash to the
/// plain-text 500 from 985 (the render limit, mirrored exactly by the
/// echo machinery in [`crate::body`]); deep values in IGNORED fields
/// parse and answer 200 up to its parser's own limit (between 10000
/// and 50000 on the probed build), past which the generic 400 takes
/// over. The native cap therefore diverges only in [2000, that parse
/// limit): an ignored-field depth there answers 400 natively where the
/// reference answers 200, and an echoed depth there answers 400 where
/// the reference answers its render 500. Both bands are adversarial
/// nesting no legitimate client reaches, held as a recorded tolerance.
/// The scanner runs on an explicit stack, so parsing never grows the
/// native call stack; the cap bounds the remaining recursive walks
/// over a PARSED value (the destructor, the issue-echo renderer) well
/// inside a worker thread's stack.
const MAX_DEPTH: usize = 2_000;

/// Parse raw body bytes as the reference does: its encoding detection
/// (UTF-8 default, BOM-marked or NUL-patterned UTF-16/32 accepted)
/// followed by a STRICT decode (invalid bytes fail the whole body, the
/// generic 400), then the scanner.
pub fn loads_bytes(bytes: &[u8]) -> Result<PyValue, PyJsonFailure> {
    let text = decode_reference_encodings(bytes).ok_or(PyJsonFailure::Undecodable)?;
    loads(&text)
}

/// The reference's body-encoding detection, ported branch for branch:
/// the UTF-32 BOMs win first (the 32-LE BOM begins with the 16-LE
/// one), then the UTF-16 and UTF-8 BOMs; BOM-less input is judged by
/// the first byte pair alone (a leading NUL means big-endian UTF-16
/// unless the second byte is NUL too, then UTF-32; a NUL second byte
/// means little-endian, 32-bit only when bytes three and four are
/// both NUL), defaulting to UTF-8. Decodes strictly (None on any
/// invalid sequence).
fn decode_reference_encodings(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE, 0x00, 0x00]) {
        return decode_utf32(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        return decode_utf32(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).ok();
    }
    if bytes.len() >= 4 {
        if bytes[0] == 0 {
            return if bytes[1] != 0 {
                decode_utf16(bytes, false)
            } else {
                decode_utf32(bytes, false)
            };
        }
        if bytes[1] == 0 {
            return if bytes[2] != 0 || bytes[3] != 0 {
                decode_utf16(bytes, true)
            } else {
                decode_utf32(bytes, true)
            };
        }
    } else if bytes.len() == 2 {
        if bytes[0] == 0 {
            return decode_utf16(bytes, false);
        }
        if bytes[1] == 0 {
            return decode_utf16(bytes, true);
        }
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units).ok()
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .chunks_exact(4)
        .map(|quad| {
            let value = if little_endian {
                u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]])
            } else {
                u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]])
            };
            char::from_u32(value)
        })
        .collect()
}

/// Parse a body as the reference does: decode positions are CHARACTER
/// indices into the decoded text.
pub fn loads(text: &str) -> Result<PyValue, PyJsonFailure> {
    let chars: Vec<char> = text.chars().collect();
    let scanner = Scanner { chars: &chars };
    let idx = scanner.skip_ws(0);
    let (value, end) = scanner.scan_value(idx)?;
    let end = scanner.skip_ws(end);
    if end != chars.len() {
        return Err(malformed("Extra data", end));
    }
    Ok(value)
}

fn malformed(msg: &str, pos: usize) -> PyJsonFailure {
    PyJsonFailure::Malformed(PyJsonError::new(msg, pos))
}

struct Scanner<'a> {
    chars: &'a [char],
}

/// One open container on the explicit parse stack.
enum Frame {
    Array {
        items: Vec<PyValue>,
    },
    Object {
        pairs: Vec<(String, PyValue)>,
        positions: BTreeMap<String, usize>,
        key: Option<String>,
    },
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

    /// The iterative value scanner: scalars resolve inline, containers
    /// run on the explicit [`Frame`] stack so nesting depth can never
    /// exhaust the native call stack. Every error message and position
    /// reproduces the reference scanner's emission points.
    fn scan_value(&self, start: usize) -> Result<(PyValue, usize), PyJsonFailure> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut idx = start;
        'expect_value: loop {
            // ── Expecting a value at `idx` ──
            let (mut value, mut end) = match self.at(idx) {
                Some('{') => {
                    // The reference's object-entry checks, verbatim
                    // ordering: a literal quote first, then whitespace,
                    // then the empty-object and property-name legs.
                    let mut at_key = idx + 1;
                    if self.at(at_key) != Some('"') {
                        at_key = self.skip_ws(at_key);
                        match self.at(at_key) {
                            Some('}') => (PyValue::Object(Vec::new()), at_key + 1),
                            Some('"') => {
                                idx = self.push_object_frame(&mut stack, at_key)?;
                                continue 'expect_value;
                            }
                            _ => {
                                return Err(malformed(
                                    "Expecting property name enclosed in double quotes",
                                    at_key,
                                ));
                            }
                        }
                    } else {
                        idx = self.push_object_frame(&mut stack, at_key)?;
                        continue 'expect_value;
                    }
                }
                Some('[') => {
                    let after = self.skip_ws(idx + 1);
                    if self.at(after) == Some(']') {
                        (PyValue::List(Vec::new()), after + 1)
                    } else {
                        if stack.len() >= MAX_DEPTH {
                            return Err(PyJsonFailure::TooDeep);
                        }
                        stack.push(Frame::Array { items: Vec::new() });
                        idx = after;
                        continue 'expect_value;
                    }
                }
                _ => self.scan_scalar(idx)?,
            };
            // ── A value completed; feed the enclosing frames ──
            loop {
                match stack.last_mut() {
                    None => return Ok((value, end)),
                    Some(Frame::Array { items }) => {
                        items.push(value);
                        let after = self.skip_ws(end);
                        match self.at(after) {
                            Some(']') => {
                                let Some(Frame::Array { items }) = stack.pop() else {
                                    unreachable!("array frame just observed");
                                };
                                value = PyValue::List(items);
                                end = after + 1;
                            }
                            Some(',') => {
                                let next = self.skip_ws(after + 1);
                                if self.at(next) == Some(']') {
                                    return Err(malformed(
                                        "Illegal trailing comma before end of array",
                                        after,
                                    ));
                                }
                                idx = next;
                                continue 'expect_value;
                            }
                            _ => return Err(malformed("Expecting ',' delimiter", after)),
                        }
                    }
                    Some(Frame::Object {
                        pairs,
                        positions,
                        key,
                    }) => {
                        let key = key.take().expect("a value always follows a key");
                        match positions.get(&key) {
                            Some(&existing) => pairs[existing].1 = value,
                            None => {
                                positions.insert(key.clone(), pairs.len());
                                pairs.push((key, value));
                            }
                        }
                        let after = self.skip_ws(end);
                        match self.at(after) {
                            Some('}') => {
                                let Some(Frame::Object { pairs, .. }) = stack.pop() else {
                                    unreachable!("object frame just observed");
                                };
                                value = PyValue::Object(pairs);
                                end = after + 1;
                            }
                            Some(',') => {
                                let at_key = self.skip_ws(after + 1);
                                match self.at(at_key) {
                                    Some('"') => {
                                        idx = self.read_key_into(&mut stack, at_key)?;
                                        continue 'expect_value;
                                    }
                                    Some('}') => {
                                        return Err(malformed(
                                            "Illegal trailing comma before end of object",
                                            after,
                                        ));
                                    }
                                    _ => {
                                        return Err(malformed(
                                            "Expecting property name enclosed in double quotes",
                                            at_key,
                                        ));
                                    }
                                }
                            }
                            _ => return Err(malformed("Expecting ',' delimiter", after)),
                        }
                    }
                }
            }
        }
    }

    /// Open an object frame at its first key (`at_key` sits on the
    /// quote) and return the first value's start index.
    fn push_object_frame(
        &self,
        stack: &mut Vec<Frame>,
        at_key: usize,
    ) -> Result<usize, PyJsonFailure> {
        if stack.len() >= MAX_DEPTH {
            return Err(PyJsonFailure::TooDeep);
        }
        stack.push(Frame::Object {
            pairs: Vec::new(),
            positions: BTreeMap::new(),
            key: None,
        });
        self.read_key_into(stack, at_key)
    }

    /// Read a `"key":` sequence into the topmost object frame and
    /// return the value's start index. `at_key` sits on the quote. A
    /// tainted KEY cannot resolve at consumption the way a value can
    /// (keys are compared, not bound), so it keeps the outright
    /// serialiser-crash reply (the register's recorded residual).
    fn read_key_into(&self, stack: &mut [Frame], at_key: usize) -> Result<usize, PyJsonFailure> {
        let (scanned, mut idx) = self.scan_string(at_key + 1)?;
        if scanned.taint.is_some() {
            return Err(PyJsonFailure::Unrenderable);
        }
        let parsed_key = scanned.text;
        if self.at(idx) != Some(':') {
            idx = self.skip_ws(idx);
            if self.at(idx) != Some(':') {
                return Err(malformed("Expecting ':' delimiter", idx));
            }
        }
        let Some(Frame::Object { key, .. }) = stack.last_mut() else {
            unreachable!("keys are read only into object frames");
        };
        *key = Some(parsed_key);
        Ok(self.skip_ws(idx + 1))
    }

    /// A scalar at `idx`: string, number, or literal.
    fn scan_scalar(&self, idx: usize) -> Result<(PyValue, usize), PyJsonFailure> {
        let Some(ch) = self.at(idx) else {
            return Err(malformed("Expecting value", idx));
        };
        match ch {
            '"' => {
                let (scanned, end) = self.scan_string(idx + 1)?;
                Ok((scanned.into_value(), end))
            }
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
                .ok_or_else(|| malformed("Expecting value", idx)),
            _ => Err(malformed("Expecting value", idx)),
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
    /// unterminated string reports the OPENING quote's position. A
    /// lone surrogate escape taints the string (the first occurrence's
    /// code and character position) rather than failing: the
    /// reference's strings carry the code point and only fail at
    /// consumption.
    fn scan_string(&self, start: usize) -> Result<(ScannedString, usize), PyJsonFailure> {
        let begin = start - 1;
        let mut out = String::new();
        let mut taint: Option<(u32, usize, usize)> = None;
        let mut chars_out = 0usize;
        let mut idx = start;
        loop {
            let Some(ch) = self.at(idx) else {
                return Err(malformed("Unterminated string starting at", begin));
            };
            match ch {
                '"' => return Ok((ScannedString { text: out, taint }, idx + 1)),
                '\\' => {
                    idx += 1;
                    let Some(esc) = self.at(idx) else {
                        return Err(malformed("Unterminated string starting at", begin));
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
                            match self.scan_unicode_escape(idx)? {
                                UnicodeEscape::Char(ch, next) => {
                                    out.push(ch);
                                    chars_out += 1;
                                    idx = next;
                                }
                                UnicodeEscape::LoneSurrogate(code, next) => {
                                    out.push('\u{FFFD}');
                                    match &mut taint {
                                        None => taint = Some((code, chars_out, 1)),
                                        Some((_, start, run)) if chars_out == *start + *run => {
                                            *run += 1;
                                        }
                                        Some(_) => {}
                                    }
                                    chars_out += 1;
                                    idx = next;
                                }
                            }
                            continue;
                        }
                        _ => {
                            return Err(malformed("Invalid \\escape", idx - 1));
                        }
                    }
                    chars_out += 1;
                    idx += 1;
                }
                ch if (ch as u32) < 0x20 => {
                    return Err(malformed("Invalid control character at", idx));
                }
                ch => {
                    out.push(ch);
                    chars_out += 1;
                    idx += 1;
                }
            }
        }
    }

    /// `\uXXXX`, with surrogate pairing as the reference performs it;
    /// `idx` sits on the `u`. Returns the decoded char and the index
    /// just past the escape.
    fn scan_unicode_escape(&self, idx: usize) -> Result<UnicodeEscape, PyJsonFailure> {
        // The reference reports the escape's `u` position on a short
        // or non-hex group.
        let hex = |at: usize| -> Result<u32, PyJsonFailure> {
            let mut value = 0u32;
            for offset in 0..4 {
                let Some(ch) = self.at(at + offset).and_then(|c| c.to_digit(16)) else {
                    return Err(malformed("Invalid \\uXXXX escape", at - 1));
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
                        .ok_or_else(|| malformed("Invalid \\uXXXX escape", idx + 7))?;
                    return Ok(UnicodeEscape::Char(ch, idx + 11));
                }
            }
        }
        if (0xD800..0xE000).contains(&first) {
            return Ok(UnicodeEscape::LoneSurrogate(first, idx + 5));
        }
        let ch =
            char::from_u32(first).ok_or_else(|| malformed("Invalid \\uXXXX escape", idx + 1))?;
        Ok(UnicodeEscape::Char(ch, idx + 5))
    }
}

/// One decoded `\uXXXX` step: a real character, or a lone surrogate
/// the caller taints the surrounding string with.
enum UnicodeEscape {
    Char(char, usize),
    LoneSurrogate(u32, usize),
}

/// A scanned string plus its surrogate taint (the first lone
/// surrogate's code point and character position), when any.
struct ScannedString {
    text: String,
    taint: Option<(u32, usize, usize)>,
}

impl ScannedString {
    fn into_value(self) -> PyValue {
        match self.taint {
            None => PyValue::Str(self.text),
            Some((code, position, run)) => PyValue::TaintedStr {
                lossy: self.text,
                code,
                position,
                run,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> PyJsonError {
        match loads(text).expect_err("expected a parse failure") {
            PyJsonFailure::Malformed(error) => error,
            other => panic!("expected a malformed failure, got {other:?}"),
        }
    }

    #[test]
    fn nesting_beyond_the_cap_fails_too_deep_without_stack_growth() {
        let deep = "[".repeat(50_000) + &"]".repeat(50_000);
        assert_eq!(loads(&deep).unwrap_err(), PyJsonFailure::TooDeep);
        let nested_objects = "{\"k\":".repeat(50_000) + "1" + &"}".repeat(50_000);
        assert_eq!(loads(&nested_objects).unwrap_err(), PyJsonFailure::TooDeep);
        // Inside the cap, depth is just data (parse and drop both run
        // without native-stack growth).
        let fine = "[".repeat(1_900) + &"]".repeat(1_900);
        assert!(loads(&fine).is_ok());
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
