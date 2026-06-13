//! Query- and path-parameter extraction, byte-faithful to the backend's
//! HTTP layer.
//!
//! The backend validates request parameters before its handlers run and
//! answers violations with a `422` envelope (`{"detail": [<issue>, ...]}`,
//! one issue object per failed parameter, in route-signature declaration
//! order). Natively-served routes reproduce that contract here: the same
//! issue types (`missing`, `int_parsing`, `literal_error`,
//! `greater_than_equal`, `less_than_equal`), the same messages, the raw
//! request text re-rendered in `input`, and the same envelope shape.
//! Validation responses carry no ETag or Cache-Control header; the
//! conditional-GET middleware applies to handler responses only.
//!
//! Every behaviour here is pinned against the running backend by the
//! cross-language extraction-conformance battery
//! (`tests/extraction_conformance.rs`); the unit tests carry the
//! hand-verified forms those probes grounded.

use axum::body::Body;
use axum::http::{header, Response, StatusCode};
use percent_encoding::percent_decode_str;
use serde_json::{json, Value};

use eo_wire::normalizer::to_wire_json;

/// One parsed query pair, percent-decoded with `+` read as space,
/// exactly as the backend's query parser treats `application/x-www-form-
/// urlencoded` query strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPair {
    pub name: String,
    pub value: String,
}

/// The decoded query string. For a scalar parameter the backend reads
/// the LAST occurrence of a repeated name (`?rank=3&rank=abc` validates
/// `"abc"`); `last` mirrors that.
#[derive(Debug, Default)]
pub struct QueryString {
    pairs: Vec<QueryPair>,
}

impl QueryString {
    /// Parse a raw (still percent-encoded) query string. Empty segments
    /// (`a=1&&b=2`) are skipped; a segment without `=` decodes to an
    /// empty-string value, both as the backend's parser behaves.
    pub fn parse(raw: Option<&str>) -> Self {
        let mut pairs = Vec::new();
        let Some(raw) = raw else {
            return Self { pairs };
        };
        for segment in raw.split('&') {
            if segment.is_empty() {
                continue;
            }
            let (name, value) = match segment.split_once('=') {
                Some((name, value)) => (name, value),
                None => (segment, ""),
            };
            pairs.push(QueryPair {
                name: decode_form_component(name),
                value: decode_form_component(value),
            });
        }
        Self { pairs }
    }

    /// The last occurrence of `name`, or None when absent.
    pub fn last(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .rev()
            .find(|pair| pair.name == name)
            .map(|pair| pair.value.as_str())
    }
}

/// Decode one form-encoded component: `+` is a space, percent escapes
/// decode as UTF-8 (invalid sequences survive lossily, matching the
/// backend's tolerant decode).
fn decode_form_component(raw: &str) -> String {
    let plus_decoded = raw.replace('+', " ");
    percent_decode_str(&plus_decoded)
        .decode_utf8_lossy()
        .into_owned()
}

/// Decode one path segment: percent escapes decode as UTF-8, `+` stays
/// literal (path segments are not form-encoded).
pub fn decode_path_segment(raw: &str) -> String {
    percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

/// An accumulating validation report; issues land in route-signature
/// declaration order because callers validate parameters in that
/// order. Each issue renders to its wire form at push time (the
/// envelope serialisation is deterministic), which lets body issues
/// echo inputs the strict JSON value model cannot represent (the
/// reference body parser admits non-finite floats and
/// arbitrary-precision integers).
#[derive(Debug, Default)]
pub struct Validation {
    issues: Vec<String>,
    unparsable_body: bool,
    unrenderable: bool,
    binding_taint: bool,
}

impl Validation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_ok(&self) -> bool {
        self.issues.is_empty() && !self.unparsable_body && !self.unrenderable
    }

    /// The body failed to parse in the way the backend answers with
    /// its generic 400 rather than a validation envelope (its parser's
    /// recursion limit); [`Validation::into_response`] then renders
    /// that reply.
    pub fn mark_unparsable_body(&mut self) {
        self.unparsable_body = true;
    }

    /// The reply would carry a value the backend's response serialiser
    /// cannot render (a non-finite float echo, an over-deep echo, a
    /// lone surrogate): the backend crashes with its plain-text 500,
    /// and [`Validation::into_response`] answers the same.
    pub fn mark_unrenderable(&mut self) {
        self.unrenderable = true;
    }

    /// A surrogate-tainted string PASSED validation (the backend's
    /// strings do) and will crash at storage binding if the request
    /// reaches it: validation issues still answer their 422 first,
    /// exactly as the backend orders it, and the caller consults this
    /// only on an otherwise-clean request.
    pub fn note_binding_taint(&mut self) {
        self.binding_taint = true;
    }

    pub fn binding_taint(&self) -> bool {
        self.binding_taint
    }

    fn push_value(&mut self, issue: Value) {
        self.issues.push(to_wire_json(&issue));
    }

    /// A pre-rendered issue: the body extractors' path, whose `input`
    /// echoes may carry forms outside the strict JSON value model.
    pub(crate) fn push_rendered(&mut self, issue: String) {
        self.issues.push(issue);
    }

    /// A required parameter that was absent.
    pub fn missing(&mut self, loc: &str, name: &str) {
        self.push_value(json!({
            "type": "missing",
            "loc": [loc, name],
            "msg": "Field required",
            "input": Value::Null,
        }));
    }

    /// A parameter that failed integer parsing; `raw` re-renders the
    /// request text exactly as received.
    pub fn int_parsing(&mut self, loc: &str, name: &str, raw: &str) {
        self.push_value(json!({
            "type": "int_parsing",
            "loc": [loc, name],
            "msg": "Input should be a valid integer, unable to parse string as an integer",
            "input": raw,
        }));
    }

    /// A literal-set parameter holding a value outside its set; the
    /// message and ctx render the allowed values as the backend does
    /// (single-quoted, "or"-joined).
    pub fn literal(&mut self, loc: &str, name: &str, raw: &str, allowed: &[&str]) {
        let expected = render_expected(allowed);
        self.push_value(json!({
            "type": "literal_error",
            "loc": [loc, name],
            "msg": format!("Input should be {expected}"),
            "input": raw,
            "ctx": {"expected": expected},
        }));
    }

    /// An unparsable float parameter.
    pub fn float_parsing(&mut self, loc: &str, name: &str, raw: &str) {
        self.push_value(json!({
            "type": "float_parsing",
            "loc": [loc, name],
            "msg": "Input should be a valid number, unable to parse string as a number",
            "input": raw,
        }));
    }

    /// A bound violation on an integer parameter; `raw` re-renders the
    /// request text (`"-0"` stays `"-0"`).
    pub fn greater_than_equal(&mut self, loc: &str, name: &str, raw: &str, bound: i64) {
        self.push_value(json!({
            "type": "greater_than_equal",
            "loc": [loc, name],
            "msg": format!("Input should be greater than or equal to {bound}"),
            "input": raw,
            "ctx": {"ge": bound},
        }));
    }

    pub fn less_than_equal(&mut self, loc: &str, name: &str, raw: &str, bound: i64) {
        self.push_value(json!({
            "type": "less_than_equal",
            "loc": [loc, name],
            "msg": format!("Input should be less than or equal to {bound}"),
            "input": raw,
            "ctx": {"le": bound},
        }));
    }

    /// The 422 envelope carrying every accumulated issue (or the
    /// backend's generic body-parse 400 when the body never parsed,
    /// or its plain-text 500 when the reply itself cannot render).
    /// No ETag and no Cache-Control: validation replies bypass the
    /// conditional-GET middleware.
    pub fn into_response(self) -> Response<Body> {
        if self.unrenderable {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from("Internal Server Error"))
                .expect("static 500 builds");
        }
        if self.unparsable_body {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    "{\"detail\":\"There was an error parsing the body\"}",
                ))
                .expect("static 400 builds");
        }
        let body = format!("{{\"detail\":[{}]}}", self.issues.join(","));
        Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("validation envelope builds")
    }
}

pub(crate) fn render_expected(allowed: &[&str]) -> String {
    let quoted: Vec<String> = allowed.iter().map(|v| format!("'{v}'")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., tail] => format!("{} or {}", head.join(", "), tail),
    }
}

/// The integer a request string parses to under the backend's lax rules,
/// or None when it does not parse. Grounded by live probes and pinned by
/// the conformance battery:
/// - Unicode whitespace trims from both ends (`" 4 "`, NBSP included).
/// - One optional leading sign; ASCII digits only (fullwidth digits are
///   rejected).
/// - Underscores group digits Python-style: between digits only, never
///   leading, trailing, doubled, or beside the sign.
/// - A float-form string is accepted only when its fraction is all
///   zeros and non-empty (`"4.0000"` parses to 4; `"4."`, `".4"`,
///   `"4.000000001"`, and exponent or hex forms do not parse).
/// - Magnitudes beyond i64 still "parse" for bound-checking purposes
///   (the backend's integers are arbitrary-precision); they resolve by
///   sign in [`BoundedInt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaxInt {
    Value(i64),
    /// Parsed as an integer but beyond i64 in the given direction; the
    /// sign carries enough to resolve any route bound.
    OverflowPositive,
    OverflowNegative,
}

pub fn parse_int_lax(raw: &str) -> Option<LaxInt> {
    let trimmed = raw.trim();
    let (negative, digits_part) = match trimmed.strip_prefix(['+', '-']) {
        Some(rest) => (trimmed.starts_with('-'), rest),
        None => (false, trimmed),
    };
    // Split an integral float form into its digit run.
    let digits_part = match digits_part.split_once('.') {
        Some((integral, fraction)) => {
            if fraction.is_empty() || !fraction.chars().all(|c| c == '0') {
                return None;
            }
            integral
        }
        None => digits_part,
    };
    if digits_part.is_empty() {
        return None;
    }
    // Python-style underscore grouping: strictly between digits.
    let mut cleaned = String::with_capacity(digits_part.len());
    let chars: Vec<char> = digits_part.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        match c {
            '0'..='9' => cleaned.push(*c),
            '_' => {
                let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
                let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
                if !(prev_digit && next_digit) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    match cleaned.parse::<i64>() {
        Ok(value) => Some(LaxInt::Value(if negative { -value } else { value })),
        // All-digit input that exceeds i64: arbitrary-precision in the
        // backend, resolved by sign for bound checks here.
        Err(_) => Some(if negative {
            LaxInt::OverflowNegative
        } else {
            LaxInt::OverflowPositive
        }),
    }
}

/// A required, bounds-checked integer query parameter: the composed
/// validation the backend applies to constrained route integers.
/// Exactly one issue lands per violating parameter: absence is
/// `missing`, unparseable text is `int_parsing`, and a parsed value
/// outside `[ge, le]` re-renders the raw text in the bound issue.
pub fn require_bounded_int(
    validation: &mut Validation,
    query: &QueryString,
    name: &str,
    ge: i64,
    le: i64,
) -> Option<i64> {
    let Some(raw) = query.last(name) else {
        validation.missing("query", name);
        return None;
    };
    match parse_int_lax(raw) {
        None => {
            validation.int_parsing("query", name, raw);
            None
        }
        Some(LaxInt::Value(value)) if value < ge => {
            validation.greater_than_equal("query", name, raw, ge);
            None
        }
        Some(LaxInt::Value(value)) if value > le => {
            validation.less_than_equal("query", name, raw, le);
            None
        }
        Some(LaxInt::OverflowNegative) => {
            validation.greater_than_equal("query", name, raw, ge);
            None
        }
        Some(LaxInt::OverflowPositive) => {
            validation.less_than_equal("query", name, raw, le);
            None
        }
        Some(LaxInt::Value(value)) => Some(value),
    }
}

/// An integer query parameter with a default (`limit: int = 10`): absent
/// takes the default, unparseable text is an `int_parsing` issue, and a
/// magnitude beyond i64 resolves to the i64 bound in its direction
/// (Python's int is unbounded; every native consumer of this clamps the
/// value into a small range, so the saturating bound is faithful).
pub fn query_int_or_default(
    validation: &mut Validation,
    query: &QueryString,
    name: &str,
    default: i64,
) -> Option<i64> {
    match query.last(name) {
        None => Some(default),
        Some(raw) => match parse_int_lax(raw) {
            Some(LaxInt::Value(value)) => Some(value),
            Some(LaxInt::OverflowPositive) => Some(i64::MAX),
            Some(LaxInt::OverflowNegative) => Some(i64::MIN),
            None => {
                validation.int_parsing("query", name, raw);
                None
            }
        },
    }
}

/// A required string query parameter (`missing` when absent; any text,
/// the empty string included, is a value).
pub fn require_str<'q>(
    validation: &mut Validation,
    query: &'q QueryString,
    name: &str,
) -> Option<&'q str> {
    match query.last(name) {
        Some(value) => Some(value),
        None => {
            validation.missing("query", name);
            None
        }
    }
}

/// A literal-set string query parameter with a default: absent takes the
/// default, present-but-outside-the-set is a `literal_error`.
pub fn literal_or_default<'q>(
    validation: &mut Validation,
    query: &'q QueryString,
    name: &str,
    allowed: &[&str],
    default: &'static str,
) -> Option<&'q str> {
    match query.last(name) {
        None => Some(default),
        Some(value) if allowed.contains(&value) => Some(value),
        Some(value) => {
            validation.literal("query", name, value, allowed);
            None
        }
    }
}

/// A required float query parameter, under the backend's lax
/// string-to-float coercion (trim, the `inf`/`nan` names, the
/// whole-string underscore gate).
pub fn require_float(validation: &mut Validation, query: &QueryString, name: &str) -> Option<f64> {
    match query.last(name) {
        None => {
            validation.missing("query", name);
            None
        }
        Some(raw) => match crate::body::lax_float_from_str(raw) {
            Some(value) => Some(value),
            None => {
                validation.float_parsing("query", name, raw);
                None
            }
        },
    }
}

/// A float query parameter with a default (`markup_uplift: float = 0.0`).
pub fn float_or_default(
    validation: &mut Validation,
    query: &QueryString,
    name: &str,
    default: f64,
) -> Option<f64> {
    match query.last(name) {
        None => Some(default),
        Some(raw) => match crate::body::lax_float_from_str(raw) {
            Some(value) => Some(value),
            None => {
                validation.float_parsing("query", name, raw);
                None
            }
        },
    }
}

/// `float | None` over a query parameter.
pub fn opt_float(
    validation: &mut Validation,
    query: &QueryString,
    name: &str,
) -> Option<Option<f64>> {
    match query.last(name) {
        None => Some(None),
        Some(raw) => match crate::body::lax_float_from_str(raw) {
            Some(value) => Some(Some(value)),
            None => {
                validation.float_parsing("query", name, raw);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(response: Response<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
        let (parts, body) = response.into_parts();
        let bytes = futures_body(body);
        let etag = parts
            .headers
            .get(header::ETAG)
            .map(|v| v.to_str().unwrap().to_string());
        (parts.status, etag, bytes)
    }

    fn futures_body(body: Body) -> Vec<u8> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                use http_body_util::BodyExt;
                body.collect().await.unwrap().to_bytes().to_vec()
            })
    }

    #[test]
    fn query_decodes_form_components_and_reads_last_occurrence() {
        let q = QueryString::parse(Some("a=1+2&b=%C3%A9&a=abc&c&d=&&e=%2B5"));
        assert_eq!(q.last("a"), Some("abc"));
        assert_eq!(q.last("b"), Some("é"));
        assert_eq!(q.last("c"), Some(""));
        assert_eq!(q.last("d"), Some(""));
        assert_eq!(q.last("e"), Some("+5"));
        assert_eq!(q.last("missing"), None);
        assert_eq!(QueryString::parse(None).last("a"), None);
    }

    #[test]
    fn query_int_or_default_defaults_parses_and_rejects() {
        // Absent -> the supplied default (not a constant).
        let mut v = Validation::new();
        assert_eq!(
            query_int_or_default(&mut v, &QueryString::parse(Some("q=a")), "limit", 10),
            Some(10)
        );
        assert!(v.is_ok());
        // Present and valid -> the parsed value.
        let mut v = Validation::new();
        assert_eq!(
            query_int_or_default(&mut v, &QueryString::parse(Some("limit=5")), "limit", 10),
            Some(5)
        );
        assert!(v.is_ok());
        // Unparseable -> None + an int_parsing violation at ["query","limit"].
        let mut v = Validation::new();
        assert_eq!(
            query_int_or_default(&mut v, &QueryString::parse(Some("limit=abc")), "limit", 10),
            None
        );
        assert!(!v.is_ok());
    }

    #[test]
    fn path_segments_decode_percent_but_not_plus() {
        assert_eq!(decode_path_segment("No%20Such"), "No Such");
        assert_eq!(decode_path_segment("a+b"), "a+b");
        assert_eq!(decode_path_segment("Ber%C3%A7as"), "Berças");
    }

    #[test]
    fn lax_int_accepts_the_probed_acceptance_grid() {
        // Each pin hand-verified against the running backend (see the
        // module doc); the conformance battery re-proves them live.
        assert_eq!(parse_int_lax("4"), Some(LaxInt::Value(4)));
        assert_eq!(parse_int_lax("05"), Some(LaxInt::Value(5)));
        assert_eq!(parse_int_lax("+5"), Some(LaxInt::Value(5)));
        assert_eq!(parse_int_lax("  4  "), Some(LaxInt::Value(4)));
        assert_eq!(parse_int_lax("\u{a0}4"), Some(LaxInt::Value(4)));
        assert_eq!(parse_int_lax("1_0"), Some(LaxInt::Value(10)));
        assert_eq!(parse_int_lax("1_0_0"), Some(LaxInt::Value(100)));
        assert_eq!(parse_int_lax("4.0"), Some(LaxInt::Value(4)));
        assert_eq!(parse_int_lax("4.0000"), Some(LaxInt::Value(4)));
        assert_eq!(parse_int_lax("-0"), Some(LaxInt::Value(0)));
        assert_eq!(parse_int_lax("-3"), Some(LaxInt::Value(-3)));
        assert_eq!(
            parse_int_lax("999999999999999999999999"),
            Some(LaxInt::OverflowPositive)
        );
        assert_eq!(
            parse_int_lax("-999999999999999999999999"),
            Some(LaxInt::OverflowNegative)
        );
    }

    #[test]
    fn lax_int_rejects_the_probed_rejection_grid() {
        for raw in [
            "abc",
            "",
            "4.5",
            "4e0",
            "0x4",
            "４",
            "4.",
            ".4",
            "+ 4",
            "--4",
            "4_",
            "_4",
            "1__0",
            "4.000000001",
            "+-4",
            "_",
            ".",
        ] {
            assert_eq!(parse_int_lax(raw), None, "expected rejection for {raw:?}");
        }
    }

    #[test]
    fn missing_and_parsing_issues_render_the_backend_forms() {
        let mut v = Validation::new();
        v.missing("query", "species_name");
        v.int_parsing("query", "rank", "abc");
        let (status, etag, body) = body_of(v.into_response());
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(etag, None, "validation replies carry no ETag");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "{\"detail\":[\
             {\"type\":\"missing\",\"loc\":[\"query\",\"species_name\"],\"msg\":\"Field required\",\"input\":null},\
             {\"type\":\"int_parsing\",\"loc\":[\"query\",\"rank\"],\"msg\":\"Input should be a valid integer, unable to parse string as an integer\",\"input\":\"abc\"}\
             ]}"
        );
    }

    #[test]
    fn bound_and_literal_issues_render_raw_input_and_ctx() {
        let mut v = Validation::new();
        v.greater_than_equal("query", "rank", "-0", 1);
        v.less_than_equal("query", "rank", "1_0_0", 25);
        v.literal("query", "target", "xx", &["profession", "hp"]);
        let (status, _, body) = body_of(v.into_response());
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "{\"detail\":[\
             {\"type\":\"greater_than_equal\",\"loc\":[\"query\",\"rank\"],\"msg\":\"Input should be greater than or equal to 1\",\"input\":\"-0\",\"ctx\":{\"ge\":1}},\
             {\"type\":\"less_than_equal\",\"loc\":[\"query\",\"rank\"],\"msg\":\"Input should be less than or equal to 25\",\"input\":\"1_0_0\",\"ctx\":{\"le\":25}},\
             {\"type\":\"literal_error\",\"loc\":[\"query\",\"target\"],\"msg\":\"Input should be 'profession' or 'hp'\",\"input\":\"xx\",\"ctx\":{\"expected\":\"'profession' or 'hp'\"}}\
             ]}"
        );
    }

    #[test]
    fn float_extractors_read_present_values() {
        let query = QueryString::parse(Some("a=2.5&b=abc"));
        let mut v = Validation::new();
        assert_eq!(float_or_default(&mut v, &query, "a", 0.0), Some(2.5));
        assert_eq!(
            float_or_default(&mut v, &query, "missing", 7.25),
            Some(7.25)
        );
        assert_eq!(require_float(&mut v, &query, "a"), Some(2.5));
        assert_eq!(opt_float(&mut v, &query, "a"), Some(Some(2.5)));
        assert_eq!(opt_float(&mut v, &query, "missing"), Some(None));
        assert!(v.is_ok());
        assert_eq!(float_or_default(&mut v, &query, "b", 0.0), None);
        assert!(!v.is_ok(), "an unparsable float records its issue");
    }

    #[test]
    fn binding_taint_notes_without_failing_validation() {
        let mut v = Validation::new();
        assert!(!v.binding_taint());
        v.note_binding_taint();
        assert!(v.binding_taint());
        // The taint alone leaves validation passing: issues answer
        // first, the caller consults the taint afterwards.
        assert!(v.is_ok());
    }

    #[test]
    fn composed_extractors_validate_in_declaration_order() {
        let q = QueryString::parse(Some("rank=abc&target=xx"));
        let mut v = Validation::new();
        let species = require_str(&mut v, &q, "species_name");
        let rank = require_bounded_int(&mut v, &q, "rank", 1, 25);
        let target = literal_or_default(&mut v, &q, "target", &["profession", "hp"], "profession");
        assert_eq!(species, None);
        assert_eq!(rank, None);
        assert_eq!(target, None);
        assert!(!v.is_ok());
        let (_, _, body) = body_of(v.into_response());
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let kinds: Vec<&str> = parsed["detail"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["missing", "int_parsing", "literal_error"]);
    }

    #[test]
    fn happy_path_extraction_returns_values_and_defaults() {
        let q = QueryString::parse(Some("species_name=1&rank=05"));
        let mut v = Validation::new();
        assert_eq!(require_str(&mut v, &q, "species_name"), Some("1"));
        assert_eq!(require_bounded_int(&mut v, &q, "rank", 1, 25), Some(5));
        assert_eq!(
            literal_or_default(&mut v, &q, "target", &["profession", "hp"], "profession"),
            Some("profession")
        );
        assert!(v.is_ok());
    }

    #[test]
    fn bounds_accept_their_exact_edges_and_reject_one_past() {
        for (raw, expected) in [("1", Some(1)), ("25", Some(25))] {
            let q = QueryString::parse(Some(&format!("rank={raw}")));
            let mut v = Validation::new();
            assert_eq!(require_bounded_int(&mut v, &q, "rank", 1, 25), expected);
            assert!(v.is_ok(), "{raw} is within bounds");
        }
        for (raw, kind) in [("0", "greater_than_equal"), ("26", "less_than_equal")] {
            let q = QueryString::parse(Some(&format!("rank={raw}")));
            let mut v = Validation::new();
            assert_eq!(require_bounded_int(&mut v, &q, "rank", 1, 25), None);
            let (_, _, body) = body_of(v.into_response());
            let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(parsed["detail"][0]["type"], *kind, "{raw}");
            assert_eq!(parsed["detail"][0]["input"], *raw);
        }
    }

    #[test]
    fn literal_accepts_each_member_of_its_set() {
        for value in ["profession", "hp"] {
            let q = QueryString::parse(Some(&format!("target={value}")));
            let mut v = Validation::new();
            assert_eq!(
                literal_or_default(&mut v, &q, "target", &["profession", "hp"], "profession"),
                Some(value)
            );
            assert!(v.is_ok(), "{value} is in the set");
        }
    }
}
