//! Byte-exact Rust port of the equivalence oracle's shared normaliser.
//!
//! The Python testing oracle (`backend/testing/fingerprint.py`) canonicalises
//! every bus payload, DB row, and HTTP body before golden comparison: UUIDs
//! become sequential `<UUID_N>` symbols, timestamps become `<TS_N>` symbols,
//! floats round to four decimal places, and dict keys sort lexically. The same
//! `Normalizer` instance is shared across the fingerprint, DB-snapshot, and
//! HTTP surfaces so a UUID seen first on the bus and later in a DB column
//! resolves to the same symbol everywhere.
//!
//! This is the Rust half of the cross-language equivalence runner: a native
//! backend that produces the same logical values must, after normalisation,
//! emit goldens byte-identical to the Python ones. "Byte-identical" is literal:
//! the normalised value is serialised through [`to_python_json`], a faithful
//! reimplementation of `json.dumps(..., sort_keys=True, ensure_ascii=False)`
//! including Python's `float.__repr__` formatting, because the goldens are
//! compared as bytes and `ryu`'s default float rendering diverges from
//! Python's (the divergence the differential fuzz exists to catch).
//!
//! ## Domain scope
//!
//! The Python `_walk` also has branches for `datetime`, Pydantic `BaseModel`,
//! and `tuple`. Those are Python-runtime object types: by the time a value
//! reaches the wire (the JSON the native backend actually emits and the
//! oracle compares) a `datetime` is already an ISO string, a `BaseModel` is
//! already its `model_dump(mode="json")` dict, and a `tuple` is already a JSON
//! array. This port therefore operates on the JSON value domain
//! (`serde_json::Value`), which is exactly the wire domain the equivalence
//! check ranges over; the conformance table and differential fuzz range over
//! the same domain.
//!
//! One numeric boundary belongs to the same scoping. Python's `json.loads`
//! parses an integer literal into an arbitrary-precision `int`; serde_json
//! (without the `arbitrary_precision` feature) parses an integer that fits
//! neither `i64` nor `u64` into the `f64` arm instead, where it would render
//! lossily. Every integer the backend actually emits is in range (SQLite row
//! ids are signed 64-bit; counts, quantities, and slots are small; monetary
//! values are floats), so `[i64::MIN, u64::MAX]` IS the wire integer domain.
//! An integer outside it is out of domain in the same sense a `datetime` is:
//! not a value the wire carries. The conformance table and fuzz pin both
//! extremes (`i64::MIN`, `u64::MAX`) and the fuzz ranges over the whole signed-
//! and-unsigned 64-bit span, so the proof covers every in-domain integer.

use std::collections::HashMap;

use serde_json::{Map, Number, Value};

/// Heuristic epoch-second window for treating a bare float as a timestamp.
/// Mirrors `fingerprint.py`: spans 2001-09 through year ~2603, clear of any
/// plausible monetary or counter value the harness scenarios produce.
const EPOCH_MIN: f64 = 1_000_000_000.0;
const EPOCH_MAX: f64 = 20_000_000_000.0;

/// Decimal places a non-epoch float rounds to (matches `FLOAT_PRECISION`).
const FLOAT_PRECISION: usize = 4;

/// Symbol-table key for a normalised timestamp.
///
/// Python keys the timestamp table on the raw value, so an epoch float and an
/// ISO string map to distinct symbols even when they denote the same instant
/// ("distinct raw values map to distinct symbols"). The float is keyed by its
/// exact bit pattern so two equal floats share a symbol while distinct ones do
/// not, matching Python `dict` identity-by-value.
#[derive(PartialEq, Eq, Hash, Clone)]
enum TimestampKey {
    Iso(String),
    Epoch(u64),
}

/// Stable canonicalisation shared across the fingerprint, DB snapshot, and
/// HTTP surfaces. The symbol tables assign `<UUID_N>` / `<TS_N>` in
/// encounter order and reset per scenario, exactly as the Python `Normalizer`.
#[derive(Default)]
pub struct Normalizer {
    uuids: HashMap<String, String>,
    timestamps: HashMap<TimestampKey, String>,
}

impl Normalizer {
    /// A fresh normaliser with empty symbol tables.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all symbol assignments; the next `normalize` call starts from
    /// `<UUID_1>` / `<TS_1>` again.
    pub fn reset(&mut self) {
        self.uuids.clear();
        self.timestamps.clear();
    }

    /// Pre-load raw-to-symbol assignments captured from another
    /// normaliser (the backend dumps its tables so a cross-language
    /// harness reproduces the same numbering when the event stream has
    /// already consumed early symbols). Timestamp keys arrive as the
    /// backend's string form of the raw value: a parseable number seeds
    /// the epoch-keyed entry, anything else the string-keyed one.
    pub fn seed_symbols(&mut self, uuids: &Map<String, Value>, timestamps: &Map<String, Value>) {
        for (raw, symbol) in uuids {
            if let Some(symbol) = symbol.as_str() {
                self.uuids.insert(raw.clone(), symbol.to_string());
            }
        }
        for (raw, symbol) in timestamps {
            let Some(symbol) = symbol.as_str() else {
                continue;
            };
            let key = match raw.parse::<f64>() {
                Ok(epoch) => TimestampKey::Epoch(epoch.to_bits()),
                Err(_) => TimestampKey::Iso(raw.clone()),
            };
            self.timestamps.insert(key, symbol.to_string());
        }
    }

    /// Return the canonical form of `value` (recursive walk).
    pub fn normalize(&mut self, value: &Value) -> Value {
        self.walk(value)
    }

    /// Normalise `value` and serialise it as the Python oracle would, in the
    /// compact `json.dumps(sort_keys=True, ensure_ascii=False)` form the
    /// per-event fingerprint lines use.
    pub fn normalize_to_compact_json(&mut self, value: &Value) -> String {
        let normalised = self.walk(value);
        to_python_json(&normalised, None)
    }

    fn walk(&mut self, value: &Value) -> Value {
        match value {
            // `None`/`bool` first, exactly as Python (where `bool` is an `int`
            // subclass so the ordering is load-bearing). In the JSON domain
            // `Null`/`Bool`/`Number` are already distinct variants, so the
            // ordering is preserved structurally rather than by guard order.
            Value::Null => Value::Null,
            Value::Bool(_) => value.clone(),
            Value::Number(n) => self.walk_number(n),
            Value::String(s) => self.walk_string(s),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.walk(v)).collect()),
            Value::Object(map) => {
                // Sorted-key reconstruction mirrors `{k: _walk(v) for k in
                // sorted(value.keys())}`. The serialiser sorts again under
                // `sort_keys=True`, so order here is belt-and-braces.
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for key in keys {
                    out.insert(key.clone(), self.walk(&map[key]));
                }
                Value::Object(out)
            }
        }
    }

    fn walk_number(&mut self, n: &Number) -> Value {
        // serde_json distinguishes integer (`5`) from float (`5.0`) Numbers as
        // Python's `json.loads` distinguishes `int` from `float`, so the int
        // branch (return as-is) and the float branch (epoch-symbol-or-round)
        // split on `is_f64`. The one gap is an integer literal outside
        // `[i64::MIN, u64::MAX]`: serde_json parses it into `f64` (it lacks
        // `arbitrary_precision`), so it would take the float branch and render
        // lossily, whereas Python keeps it an exact `int`. That range is the
        // wire integer domain (see the module's "Domain scope"): no value the
        // backend emits leaves it, and both extremes are pinned by the
        // conformance/fuzz, so the float branch only ever sees genuine floats.
        if n.is_f64() {
            let f = n.as_f64().expect("is_f64 number yields f64");
            if (EPOCH_MIN..=EPOCH_MAX).contains(&f) {
                return Value::String(self.symbol_for_epoch(f));
            }
            let rounded = round_half_even(f, FLOAT_PRECISION);
            return Value::Number(
                Number::from_f64(rounded).expect("rounding a finite float stays finite"),
            );
        }
        Value::Number(n.clone())
    }

    fn walk_string(&mut self, s: &str) -> Value {
        if is_uuid(s) {
            return Value::String(self.symbol_for_uuid(s));
        }
        if is_iso_prefix(s) {
            return Value::String(self.symbol_for_iso(s));
        }
        Value::String(s.to_string())
    }

    fn symbol_for_uuid(&mut self, value: &str) -> String {
        if let Some(symbol) = self.uuids.get(value) {
            return symbol.clone();
        }
        let symbol = format!("<UUID_{}>", self.uuids.len() + 1);
        self.uuids.insert(value.to_string(), symbol.clone());
        symbol
    }

    fn symbol_for_iso(&mut self, value: &str) -> String {
        self.symbol_for_timestamp(TimestampKey::Iso(value.to_string()))
    }

    fn symbol_for_epoch(&mut self, value: f64) -> String {
        self.symbol_for_timestamp(TimestampKey::Epoch(value.to_bits()))
    }

    fn symbol_for_timestamp(&mut self, key: TimestampKey) -> String {
        if let Some(symbol) = self.timestamps.get(&key) {
            return symbol.clone();
        }
        let symbol = format!("<TS_{}>", self.timestamps.len() + 1);
        self.timestamps.insert(key, symbol.clone());
        symbol
    }
}

/// Whether `s` is a lowercase-hex canonical UUID, matching the Python
/// `UUID_PATTERN` (`^[0-9a-f]{8}-...-[0-9a-f]{12}$`, full match).
fn is_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, &c) in bytes.iter().enumerate() {
        let ok = match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Whether `s` starts with an ISO-8601 timestamp, matching the Python
/// `ISO_PATTERN` (`^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}`, prefix match).
///
/// Python's bare `\d` also matches non-ASCII Unicode decimal digits; the wire
/// domain this port targets only ever carries ASCII timestamps, so the
/// differential fuzz keeps generated strings in the ASCII domain and this
/// check uses `[0-9]`. The boundary is a domain choice, not an ignored
/// divergence: no ASCII payload disagrees with Python here.
fn is_iso_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 19 {
        return false;
    }
    let d = |i: usize| b[i].is_ascii_digit();
    d(0) && d(1)
        && d(2)
        && d(3)
        && b[4] == b'-'
        && d(5)
        && d(6)
        && b[7] == b'-'
        && d(8)
        && d(9)
        && (b[10] == b'T' || b[10] == b' ')
        && d(11)
        && d(12)
        && b[13] == b':'
        && d(14)
        && d(15)
        && b[16] == b':'
        && d(17)
        && d(18)
}

/// Round `x` to `places` decimal places, round-half-to-even, returning the
/// nearest `f64` to that decimal: the semantics of Python's `round(x, places)`.
///
/// Python's `round` rounds the *exact* binary value half-to-even and returns
/// the nearest double to the result. Rust's `{:.N}` formatting is correctly
/// rounded with the same ties-to-even rule, so formatting to `places` and
/// re-parsing reproduces Python's double bit-for-bit (verified against
/// genuine ties such as `0.03125` in the tests below).
///
/// Exposed so ported services that round intermediate figures the way the
/// Python implementation does (e.g. the cost engine's per-line `round(_, 4)`)
/// share one Python-faithful rounding, keeping their figures bit-identical to
/// the oracle when they later fold into a fingerprint golden.
pub fn round_half_even(x: f64, places: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.places$}")
        .parse::<f64>()
        .expect("formatted finite float re-parses")
}

/// Serialise `value` as Python's `json.dumps(value, sort_keys=True,
/// ensure_ascii=False, indent=indent)` would, byte-for-byte.
///
/// `indent = None` is the compact form (item separator `", "`, key separator
/// `": "`) the per-event fingerprint lines use; `indent = Some(n)` is the
/// pretty form (`",\n"`-separated, `n`-space indent) the DB-snapshot and
/// HTTP-response goldens use.
pub fn to_python_json(value: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent, 0);
    out
}

/// Render `value` exactly as the backend's HTTP layer serialises a
/// response body: `json.dumps(content, ensure_ascii=False,
/// allow_nan=False, separators=(",", ":"))` over the model's own key
/// order. Differs from [`to_python_json`] in both separators (no
/// spaces) and key order (insertion order, not sorted): the goldens'
/// canonical form sorts for diff stability, while the wire carries
/// the models' declared order.
pub fn to_wire_json(value: &Value) -> String {
    let mut out = String::new();
    write_wire_value(&mut out, value);
    out
}

fn write_wire_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_wire_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, entry)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_wire_value(out, entry);
            }
            out.push('}');
        }
    }
}

fn write_value(out: &mut String, value: &Value, indent: Option<usize>, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => write_array(out, items, indent, depth),
        Value::Object(map) => write_object(out, map, indent, depth),
    }
}

fn write_number(out: &mut String, n: &Number) {
    if n.is_f64() {
        out.push_str(&python_repr_f64(n.as_f64().expect("is_f64 yields f64")));
    } else {
        // Integers render identically in Python `str(int)` and Rust.
        out.push_str(&n.to_string());
    }
}

fn write_array(out: &mut String, items: &[Value], indent: Option<usize>, depth: usize) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    match indent {
        None => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, indent, depth);
            }
            out.push(']');
        }
        Some(width) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_newline_indent(out, width, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            push_newline_indent(out, width, depth);
            out.push(']');
        }
    }
}

fn write_object(out: &mut String, map: &Map<String, Value>, indent: Option<usize>, depth: usize) {
    if map.is_empty() {
        out.push_str("{}");
        return;
    }
    // `sort_keys=True`: emit keys in lexical (code-point) order. UTF-8 byte
    // order equals Unicode code-point order, so Rust's `str` ordering matches
    // Python's `str` comparison used by `sorted`.
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    match indent {
        None => {
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_string(out, key);
                out.push_str(": ");
                write_value(out, &map[*key], indent, depth);
            }
            out.push('}');
        }
        Some(width) => {
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_newline_indent(out, width, depth + 1);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, &map[*key], indent, depth + 1);
            }
            push_newline_indent(out, width, depth);
            out.push('}');
        }
    }
}

fn push_newline_indent(out: &mut String, width: usize, depth: usize) {
    out.push('\n');
    for _ in 0..(width * depth) {
        out.push(' ');
    }
}

/// Escape a string exactly as Python's `json` encoder does with
/// `ensure_ascii=False`: `"` and `\` and the short escapes `\b \t \n \f \r`,
/// other C0 control characters as `\uXXXX`, everything else (including all
/// non-ASCII) verbatim. `/` and `DEL` are not escaped, matching Python.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Format a finite `f64` exactly as Python's `repr(float)` (= `json.dumps`'s
/// float rendering): shortest round-tripping digits, with Python's
/// fixed-vs-scientific threshold (`-4 < decpt <= 16`), the trailing `.0` for
/// integer-valued floats, and signed two-digit-minimum exponents.
/// Python `repr(float)` rendering: the shared float-to-text rule every
/// byte-comparable writer uses.
pub fn python_repr_f64(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }

    let negative = value.is_sign_negative();
    let magnitude = value.abs();

    // `ryu` yields the shortest round-tripping decimal; parse it into a digit
    // string plus a decimal-point position, then reformat under Python's rules
    // (which differ from ryu's in threshold, the `+`/zero-padded exponent, and
    // the integer `.0`). Decoupling from ryu's own formatting is the whole
    // point: this is what the differential fuzz proves byte-equal.
    let mut buffer = ryu::Buffer::new();
    let ryu_str = buffer.format_finite(magnitude);
    let (digits, decpt) = parse_ryu(ryu_str);

    let body = if -4 < decpt && decpt <= 16 {
        format_fixed(&digits, decpt)
    } else {
        format_scientific(&digits, decpt)
    };

    if negative {
        format!("-{body}")
    } else {
        body
    }
}

/// Parse a positive `ryu` shortest-form string into `(significant_digits,
/// decpt)` where `decpt` is the number of digits before the decimal point
/// (the value is `0.<digits> * 10^decpt`). Leading and trailing zeros are
/// stripped so a single canonical `(digits, decpt)` represents the value.
fn parse_ryu(s: &str) -> (Vec<u8>, i32) {
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().expect("ryu exponent parses")),
        None => (s, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };

    let mut digits: Vec<u8> = Vec::with_capacity(int_part.len() + frac_part.len());
    digits.extend(int_part.bytes());
    digits.extend(frac_part.bytes());

    // Decimal point sits after the integer digits, shifted by the exponent.
    let mut decpt = int_part.len() as i32 + exp;

    // Strip leading zeros (each one shifts the point left).
    while digits.len() > 1 && digits[0] == b'0' {
        digits.remove(0);
        decpt -= 1;
    }
    // Strip trailing zeros (the point is unaffected).
    while digits.len() > 1 && *digits.last().expect("non-empty") == b'0' {
        digits.pop();
    }

    (digits, decpt)
}

fn format_fixed(digits: &[u8], decpt: i32) -> String {
    let digit_str = String::from_utf8(digits.to_vec()).expect("ascii digits");
    if decpt <= 0 {
        // 0.000<digits>
        let zeros = "0".repeat((-decpt) as usize);
        format!("0.{zeros}{digit_str}")
    } else if (decpt as usize) >= digit_str.len() {
        // <digits>000.0
        let zeros = "0".repeat(decpt as usize - digit_str.len());
        format!("{digit_str}{zeros}.0")
    } else {
        let split = decpt as usize;
        format!("{}.{}", &digit_str[..split], &digit_str[split..])
    }
}

fn format_scientific(digits: &[u8], decpt: i32) -> String {
    let digit_str = String::from_utf8(digits.to_vec()).expect("ascii digits");
    let exponent = decpt - 1;
    let mantissa = if digit_str.len() == 1 {
        digit_str.clone()
    } else {
        format!("{}.{}", &digit_str[..1], &digit_str[1..])
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn norm_compact(value: &Value) -> String {
        Normalizer::new().normalize_to_compact_json(value)
    }

    // --- _walk branch coverage ---------------------------------------------

    #[test]
    fn null_and_bool_pass_through() {
        assert_eq!(norm_compact(&json!(null)), "null");
        assert_eq!(norm_compact(&json!(true)), "true");
        assert_eq!(norm_compact(&json!(false)), "false");
    }

    #[test]
    fn integers_pass_through_unrounded() {
        assert_eq!(norm_compact(&json!(0)), "0");
        assert_eq!(norm_compact(&json!(42)), "42");
        assert_eq!(norm_compact(&json!(-17)), "-17");
        assert_eq!(norm_compact(&json!(9_999_999_999_i64)), "9999999999");
    }

    #[test]
    fn integer_valued_float_keeps_its_dot_zero() {
        // The single most load-bearing float case: Python renders 15.0 as
        // "15.0", not "15"; ryu's default Display drops the ".0".
        assert_eq!(norm_compact(&json!(15.0)), "15.0");
        assert_eq!(norm_compact(&json!(100.0)), "100.0");
        assert_eq!(norm_compact(&json!(0.0)), "0.0");
    }

    #[test]
    fn floats_round_to_four_places() {
        assert_eq!(norm_compact(&json!(5.12)), "5.12");
        assert_eq!(norm_compact(&json!(0.05)), "0.05");
        assert_eq!(norm_compact(&json!(0.12)), "0.12");
        // round(0.123456, 4) == 0.1235
        assert_eq!(norm_compact(&json!(0.123_456)), "0.1235");
        // round(2.000049, 4) == 2.0
        assert_eq!(norm_compact(&json!(2.000_049)), "2.0");
    }

    #[test]
    fn rounding_is_ties_to_even() {
        // 0.03125 is exactly representable, so this is a genuine decimal tie at
        // the 4th place: Python round(0.03125, 4) == 0.0312 (2 is even).
        assert_eq!(norm_compact(&json!(0.031_25)), "0.0312");
        // 0.09375 is also exact: round half-to-even at the 4th place -> 0.0938.
        assert_eq!(norm_compact(&json!(0.093_75)), "0.0938");
    }

    #[test]
    fn negative_floats_round_and_render() {
        assert_eq!(norm_compact(&json!(-5.5)), "-5.5");
        assert_eq!(norm_compact(&json!(-0.0001)), "-0.0001");
    }

    #[test]
    fn uuid_strings_become_sequential_symbols() {
        let value = json!([
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
            "11111111-1111-1111-1111-111111111111"
        ]);
        assert_eq!(
            norm_compact(&value),
            r#"["<UUID_1>", "<UUID_2>", "<UUID_1>"]"#
        );
    }

    #[test]
    fn uppercase_uuid_is_not_matched() {
        // UUID_PATTERN is lowercase-hex only.
        let value = json!("AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA");
        assert_eq!(
            norm_compact(&value),
            r#""AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA""#
        );
    }

    #[test]
    fn iso_timestamp_prefix_becomes_symbol() {
        assert_eq!(norm_compact(&json!("2026-01-01T00:00:00")), r#""<TS_1>""#);
        // Prefix match: trailing fractional seconds / offset still match.
        assert_eq!(
            norm_compact(&json!("2026-01-01T00:00:00.123456+00:00")),
            r#""<TS_1>""#
        );
        // Space separator variant.
        assert_eq!(norm_compact(&json!("2026-01-01 00:00:00")), r#""<TS_1>""#);
    }

    #[test]
    fn non_timestamp_strings_pass_through() {
        assert_eq!(norm_compact(&json!("Shrapnel")), r#""Shrapnel""#);
        assert_eq!(norm_compact(&json!("2026-01-01")), r#""2026-01-01""#); // too short
    }

    #[test]
    fn epoch_floats_in_window_become_timestamps() {
        // 1.7e9 is within [1e9, 2e10] -> timestamp symbol, not rounded.
        assert_eq!(norm_compact(&json!(1_700_000_000.0)), r#""<TS_1>""#);
        // Just outside the window -> rounded float.
        assert_eq!(norm_compact(&json!(999_999_999.0)), "999999999.0");
    }

    #[test]
    fn epoch_float_and_iso_string_get_distinct_symbols() {
        // "distinct raw values map to distinct symbols even when semantically
        // the same instant across encodings".
        let value = json!([1_700_000_000.0, "2023-11-14T22:13:20"]);
        assert_eq!(norm_compact(&value), r#"["<TS_1>", "<TS_2>"]"#);
    }

    #[test]
    fn dict_keys_sort_lexically() {
        let value = json!({"b": 1, "a": 2, "c": 3});
        assert_eq!(norm_compact(&value), r#"{"a": 2, "b": 1, "c": 3}"#);
    }

    #[test]
    fn nested_structures_recurse() {
        let value = json!({"items": [{"v": 1.5}, {"v": 2.0}], "n": null});
        assert_eq!(
            norm_compact(&value),
            r#"{"items": [{"v": 1.5}, {"v": 2.0}], "n": null}"#
        );
    }

    // --- shared symbol table across surfaces -------------------------------

    #[test]
    fn one_normalizer_shares_symbols_across_calls() {
        // Mirrors GoldenSet: the bus fingerprint assigns <UUID_1>, then the DB
        // snapshot reuses it and continues the sequence.
        let mut norm = Normalizer::new();
        let bus = json!({"session_id": "11111111-1111-1111-1111-111111111111"});
        assert_eq!(
            norm.normalize_to_compact_json(&bus),
            r#"{"session_id": "<UUID_1>"}"#
        );
        let db_row = json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "kill_id": "22222222-2222-2222-2222-222222222222"
        });
        // Session reuses <UUID_1>; the new kill id is <UUID_2>.
        assert_eq!(
            norm.normalize_to_compact_json(&db_row),
            r#"{"id": "<UUID_1>", "kill_id": "<UUID_2>"}"#
        );
    }

    #[test]
    fn reset_restarts_the_symbol_sequence() {
        let mut norm = Normalizer::new();
        norm.normalize(&json!("11111111-1111-1111-1111-111111111111"));
        norm.reset();
        assert_eq!(
            norm.normalize_to_compact_json(&json!("33333333-3333-3333-3333-333333333333")),
            r#""<UUID_1>""#
        );
    }

    // --- serialiser: separators, escaping, indent --------------------------

    #[test]
    fn compact_separators_match_python_default() {
        // json.dumps default separators are (", ", ": ") when indent is None.
        let value = json!({"a": 1, "b": [2, 3]});
        assert_eq!(to_python_json(&value, None), r#"{"a": 1, "b": [2, 3]}"#);
    }

    #[test]
    fn indented_form_matches_python_indent_two() {
        let value = json!({"b": 1, "a": [2]});
        let expected = "{\n  \"a\": [\n    2\n  ],\n  \"b\": 1\n}";
        assert_eq!(to_python_json(&value, Some(2)), expected);
    }

    #[test]
    fn empty_containers_render_inline_under_indent() {
        assert_eq!(to_python_json(&json!({}), Some(2)), "{}");
        assert_eq!(to_python_json(&json!([]), Some(2)), "[]");
        assert_eq!(
            to_python_json(&json!({"a": [], "b": {}}), Some(2)),
            "{\n  \"a\": [],\n  \"b\": {}\n}"
        );
    }

    #[test]
    fn string_escaping_matches_python_ensure_ascii_false() {
        assert_eq!(to_python_json(&json!("a\"b\\c"), None), r#""a\"b\\c""#);
        assert_eq!(
            to_python_json(&json!("tab\tnew\nline"), None),
            r#""tab\tnew\nline""#
        );
        // C0 control other than the short escapes -> \u00xx.
        assert_eq!(to_python_json(&json!("\u{01}"), None), "\"\\u0001\"");
        // Non-ASCII passes through verbatim (ensure_ascii=False).
        assert_eq!(to_python_json(&json!("café ⚔"), None), "\"café ⚔\"");
        // Forward slash and DEL are not escaped.
        assert_eq!(to_python_json(&json!("a/b\u{7f}"), None), "\"a/b\u{7f}\"");
    }

    // --- python_repr_f64 edge cases ----------------------------------------

    #[test]
    fn repr_scientific_threshold_matches_python() {
        // decpt > 16 -> scientific; repr(1e16) == "1e+16".
        assert_eq!(python_repr_f64(1e16), "1e+16");
        assert_eq!(python_repr_f64(1.5e16), "1.5e+16");
        // decpt == 16 stays fixed; repr(1e15) == "1000000000000000.0".
        assert_eq!(python_repr_f64(1e15), "1000000000000000.0");
        // Negative exponents only occur pre-rounding here, but the formatter
        // must still match Python: repr(1e-05) == "1e-05".
        assert_eq!(python_repr_f64(1e-5), "1e-05");
        assert_eq!(python_repr_f64(1e-4), "0.0001");
        assert_eq!(python_repr_f64(1e100), "1e+100");
    }

    #[test]
    fn repr_handles_negative_zero() {
        assert_eq!(python_repr_f64(-0.0), "-0.0");
        assert_eq!(python_repr_f64(0.0), "0.0");
    }

    #[test]
    fn uuid_recogniser_rejects_wrong_shapes() {
        assert!(is_uuid("11111111-1111-1111-1111-111111111111"));
        assert!(!is_uuid("11111111-1111-1111-1111-11111111111")); // 35 chars
        assert!(!is_uuid("11111111x1111-1111-1111-111111111111")); // wrong sep
        assert!(!is_uuid("g1111111-1111-1111-1111-111111111111")); // non-hex
    }
}
