//! Request-body extraction, byte-faithful to the backend's HTTP layer.
//!
//! Bodies parse through the reference-faithful reader ([`pyjson`]) and
//! validate field by field with the backend's lax coercions, pinned by
//! live probes and re-proven by the conformance battery:
//! - strings are strict (no coercion; null is a `string_type` error on
//!   a required field and a value on an optional one);
//! - floats accept numbers, bools, and lax strings (whitespace trim,
//!   digit underscores, `inf`/`nan` names);
//! - bools accept the backend's exact lax set (`yes`/`no`/`on`/`off`/
//!   `true`/`false`/`t`/`f`/`y`/`n`/`1`/`0` case-insensitively, ints
//!   and floats equal to 0 or 1);
//! - ints accept integral floats (`int_from_float` for fractional),
//!   bools, and the same lax strings the query layer accepts; an
//!   integer beyond i64 reproduces the backend's unhandled-overflow
//!   500 at the adapter (its integers are arbitrary-precision and the
//!   storage layer rejects them there).
//!
//! Issue `loc`s are `["body", <field>, ...]`; a missing field echoes
//! the WHOLE body object as `input`; malformed JSON reports
//! `json_invalid` with the reference scanner's message and character
//! position.

use axum::body::Body;
use axum::http::{header, Response, StatusCode};

use crate::extract::Validation;
use crate::pyjson::{loads_bytes, PyJsonFailure, PyValue};

/// One step in an issue's location path under `"body"`.
#[derive(Debug, Clone)]
pub enum Loc {
    Field(&'static str),
    Index(usize),
}

fn render_loc(tail: &[Loc]) -> String {
    let mut parts = vec!["\"body\"".to_string()];
    for step in tail {
        match step {
            Loc::Field(name) => parts.push(format!("\"{name}\"")),
            Loc::Index(index) => parts.push(index.to_string()),
        }
    }
    format!("[{}]", parts.join(","))
}

fn escape_json_str(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// Push a body-scoped issue with a pre-rendered `input` echo.
pub fn body_issue(
    validation: &mut Validation,
    issue_type: &str,
    loc_tail: &[Loc],
    msg: &str,
    input_echo: &str,
    ctx: Option<(&str, String)>,
) {
    let mut rendered = format!(
        "{{\"type\":{},\"loc\":{},\"msg\":{},\"input\":{}",
        escape_json_str(issue_type),
        render_loc(loc_tail),
        escape_json_str(msg),
        input_echo,
    );
    if let Some((key, value_json)) = ctx {
        rendered.push_str(&format!(",\"ctx\":{{\"{key}\":{value_json}}}"));
    }
    rendered.push('}');
    validation.push_rendered(rendered);
}

/// The parsed body object, plus its rendered echo (missing-field
/// issues echo the whole object).
pub struct BodyObject {
    pairs: Vec<(String, PyValue)>,
    echo: Option<String>,
}

impl BodyObject {
    pub fn get(&self, name: &str) -> Option<&PyValue> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    pub fn pairs(&self) -> &[(String, PyValue)] {
        &self.pairs
    }

    /// The whole-object echo, or None when the backend's serialiser
    /// could not render it (the caller marks the 500).
    pub fn echo(&self) -> Option<&str> {
        self.echo.as_deref()
    }
}

/// Read and shape a request body as the backend does. `None` means a
/// reply was recorded on the validation report. A body of top-level
/// JSON `null` means ABSENT to the backend's model binding: a
/// required-body route answers the missing-["body"] envelope.
pub fn read_object(
    content_type: Option<&str>,
    bytes: &[u8],
    validation: &mut Validation,
) -> Option<BodyObject> {
    match read_value(content_type, bytes, validation)? {
        PyValue::Object(pairs) => {
            let whole = PyValue::Object(pairs.clone());
            let echo = echo_renders(&whole, 0).then(|| whole.to_echo_json());
            Some(BodyObject { pairs, echo })
        }
        PyValue::Null => {
            body_issue(validation, "missing", &[], "Field required", "null", None);
            None
        }
        other => {
            let echo = echo_or_unrenderable(validation, &other)?;
            body_issue(
                validation,
                "model_attributes_type",
                &[],
                "Input should be a valid dictionary or object to extract fields from",
                &echo,
                None,
            );
            None
        }
    }
}

/// The optional-body variant (a route whose body model defaults to
/// None): an absent body OR a top-level JSON `null` is `Some(None)`, a
/// present body validates as usual (`Some(Some(..))`), and `None`
/// means a reply was recorded.
pub fn read_optional_object(
    content_type: Option<&str>,
    bytes: &[u8],
    validation: &mut Validation,
) -> Option<Option<BodyObject>> {
    if bytes.is_empty() {
        return Some(None);
    }
    match read_value(content_type, bytes, validation)? {
        PyValue::Null => Some(None),
        PyValue::Object(pairs) => {
            let whole = PyValue::Object(pairs.clone());
            let echo = echo_renders(&whole, 0).then(|| whole.to_echo_json());
            Some(Some(BodyObject { pairs, echo }))
        }
        other => {
            let echo = echo_or_unrenderable(validation, &other)?;
            body_issue(
                validation,
                "model_attributes_type",
                &[],
                "Input should be a valid dictionary or object to extract fields from",
                &echo,
                None,
            );
            None
        }
    }
}

/// Whether the backend's body reader treats `content_type` as JSON:
/// maintype `application` and subtype `json` or `*+json`, both
/// case-insensitive.
fn is_json_content_type(content_type: Option<&str>) -> bool {
    let Some(value) = content_type else {
        return false;
    };
    let media = value.split(';').next().unwrap_or("").trim();
    let Some((maintype, subtype)) = media.split_once('/') else {
        return false;
    };
    maintype.eq_ignore_ascii_case("application")
        && (subtype.eq_ignore_ascii_case("json")
            || subtype
                .rsplit_once('+')
                .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json")))
}

fn read_value(
    content_type: Option<&str>,
    bytes: &[u8],
    validation: &mut Validation,
) -> Option<PyValue> {
    if bytes.is_empty() {
        body_issue(validation, "missing", &[], "Field required", "null", None);
        return None;
    }
    if !is_json_content_type(content_type) {
        // Without a JSON content type the backend treats the raw text
        // as the submitted value (a string), which then fails the
        // object check with that string echoed.
        return Some(PyValue::Str(String::from_utf8_lossy(bytes).into_owned()));
    }
    match loads_bytes(bytes) {
        Ok(value) => Some(value),
        Err(PyJsonFailure::Malformed(error)) => {
            body_issue(
                validation,
                "json_invalid",
                &[Loc::Index(error.pos)],
                "JSON decode error",
                "{}",
                Some(("error", escape_json_str(&error.msg))),
            );
            None
        }
        Err(PyJsonFailure::TooDeep) | Err(PyJsonFailure::Undecodable) => {
            validation.mark_unparsable_body();
            None
        }
        Err(PyJsonFailure::Unrenderable) => {
            validation.mark_unrenderable();
            None
        }
    }
}

/// Render an issue echo unless the backend's serialiser would crash on
/// it (a non-finite float anywhere in the echoed value, or container
/// depth past its render recursion limit): those answer the plain-text
/// 500 instead, as the backend does.
pub(crate) fn echo_or_unrenderable(validation: &mut Validation, value: &PyValue) -> Option<String> {
    if echo_renders(value, 0) {
        Some(value.to_echo_json())
    } else {
        validation.mark_unrenderable();
        None
    }
}

/// The reference's render limit for echoed values: container depth 984
/// renders the envelope, 985 crashes its serialiser (probed through
/// the route, repeatable).
const ECHO_DEPTH_LIMIT: usize = 984;

fn echo_renders(value: &PyValue, depth: usize) -> bool {
    match value {
        PyValue::Float(v) => v.is_finite(),
        // The reference's serialiser crashes on the surrogate the
        // tainted string stands in for.
        PyValue::TaintedStr { .. } => false,
        PyValue::List(items) => {
            depth < ECHO_DEPTH_LIMIT && items.iter().all(|item| echo_renders(item, depth + 1))
        }
        PyValue::Object(pairs) => {
            depth < ECHO_DEPTH_LIMIT
                && pairs
                    .iter()
                    .all(|(_, value)| echo_renders(value, depth + 1))
        }
        _ => true,
    }
}

/// A required strict string. Absent echoes the whole object; a non-
/// string value is a `string_type` error (null included).
pub fn required_str(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<String> {
    match object.get(name) {
        None => {
            match object.echo() {
                Some(echo) => {
                    body_issue(
                        validation,
                        "missing",
                        &[Loc::Field(name)],
                        "Field required",
                        echo,
                        None,
                    );
                }
                None => validation.mark_unrenderable(),
            }
            None
        }
        Some(PyValue::Str(value)) => Some(value.clone()),
        Some(PyValue::TaintedStr { lossy, .. }) => {
            // The reference accepts the string at validation and only
            // crashes at storage binding; the taint defers the 500 to
            // the post-validation check so other fields' 422s win
            // first, as the backend orders it.
            validation.note_binding_taint();
            Some(lossy.clone())
        }
        Some(other) => {
            string_type_issue(validation, &[Loc::Field(name)], other);
            None
        }
    }
}

fn string_type_issue(validation: &mut Validation, loc: &[Loc], value: &PyValue) {
    if let Some(echo) = echo_or_unrenderable(validation, value) {
        body_issue(
            validation,
            "string_type",
            loc,
            "Input should be a valid string",
            &echo,
            None,
        );
    }
}

/// An optional strict string (`str | None`): absent or null is None.
pub fn opt_str(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<String>> {
    match object.get(name) {
        None | Some(PyValue::Null) => Some(None),
        Some(PyValue::Str(value)) => Some(Some(value.clone())),
        Some(PyValue::TaintedStr { lossy, .. }) => {
            validation.note_binding_taint();
            Some(Some(lossy.clone()))
        }
        Some(other) => {
            string_type_issue(validation, &[Loc::Field(name)], other);
            None
        }
    }
}

/// A string field with a default (`str = "..."`).
pub fn str_or_default(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
    default: &str,
) -> Option<String> {
    match object.get(name) {
        None => Some(default.to_string()),
        Some(PyValue::Str(value)) => Some(value.clone()),
        Some(PyValue::TaintedStr { lossy, .. }) => {
            validation.note_binding_taint();
            Some(lossy.clone())
        }
        Some(other) => {
            string_type_issue(validation, &[Loc::Field(name)], other);
            None
        }
    }
}

/// The backend's lax string-to-float: whitespace trim, the
/// `inf`/`infinity`/`nan` names, and its underscore gate (probed:
/// whole-string, not between-digits: no leading, trailing, or doubled
/// underscore, then every underscore strips before parsing, so
/// `"1_.5"`, `"1e_5"`, and `"+_1"` all parse while `"_1.5"`, `"1.5_"`,
/// and `"1__0.5"` do not).
fn lax_float_from_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('_')
        || trimmed.ends_with('_')
        || trimmed.contains("__")
    {
        return None;
    }
    let cleaned: String = trimmed.chars().filter(|c| *c != '_').collect();
    let (negative, rest) = match cleaned.strip_prefix(['+', '-']) {
        Some(rest) => (cleaned.starts_with('-'), rest),
        None => (false, &cleaned[..]),
    };
    let lowered = rest.to_ascii_lowercase();
    let magnitude = match lowered.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => {
            if rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                return None;
            }
            rest.parse::<f64>().ok()?
        }
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// An optional lax float (`float | None`).
pub fn opt_f64(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<f64>> {
    let value = match object.get(name) {
        None | Some(PyValue::Null) => return Some(None),
        Some(value) => value,
    };
    match value {
        PyValue::Float(v) => Some(Some(*v)),
        PyValue::Int(v) => Some(Some(*v as f64)),
        PyValue::BigInt(text) => Some(Some(text.parse().unwrap_or(f64::INFINITY))),
        PyValue::Bool(b) => Some(Some(if *b { 1.0 } else { 0.0 })),
        PyValue::Str(raw) => match lax_float_from_str(raw) {
            Some(v) => Some(Some(v)),
            None => {
                let echo = echo_or_unrenderable(validation, value)?;
                body_issue(
                    validation,
                    "float_parsing",
                    &[Loc::Field(name)],
                    "Input should be a valid number, unable to parse string as a number",
                    &echo,
                    None,
                );
                None
            }
        },
        other => {
            let echo = echo_or_unrenderable(validation, other)?;
            body_issue(
                validation,
                "float_type",
                &[Loc::Field(name)],
                "Input should be a valid number",
                &echo,
                None,
            );
            None
        }
    }
}

/// The backend's lax boolean set: exact strings (case-insensitive, no
/// whitespace tolerance), 0/1 numbers. Failures split the backend's
/// way (probed): coercion-CLASS values with the wrong content (other
/// ints, other integral floats, other strings) are `bool_parsing`;
/// everything else (null, fractional floats, beyond-i64 integers,
/// containers) is `bool_type`.
pub fn bool_or_default(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
    default: bool,
) -> Option<bool> {
    let value = match object.get(name) {
        None => return Some(default),
        Some(value) => value,
    };
    let parsed = match value {
        PyValue::Bool(b) => Some(*b),
        PyValue::Int(0) => Some(false),
        PyValue::Int(1) => Some(true),
        PyValue::Float(v) if *v == 0.0 => Some(false),
        PyValue::Float(v) if *v == 1.0 => Some(true),
        PyValue::Str(raw) => match raw.to_ascii_lowercase().as_str() {
            "true" | "t" | "yes" | "y" | "on" | "1" => Some(true),
            "false" | "f" | "no" | "n" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    };
    if let Some(b) = parsed {
        return Some(b);
    }
    // Integral floats are coercion-class only inside the same
    // exclusive +/-2^63 window the int gate uses; beyond it the
    // backend answers the type error, not the parsing one.
    let coercion_class = matches!(value, PyValue::Int(_) | PyValue::Str(_))
        || matches!(value, PyValue::Float(v) if v.is_finite()
            && v.fract() == 0.0
            && *v > -9_223_372_036_854_775_808.0
            && *v < 9_223_372_036_854_775_808.0);
    let (kind, msg) = if coercion_class {
        (
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input",
        )
    } else {
        ("bool_type", "Input should be a valid boolean")
    };
    let echo = echo_or_unrenderable(validation, value)?;
    body_issue(validation, kind, &[Loc::Field(name)], msg, &echo, None);
    None
}

/// `bool | None`.
pub fn opt_bool(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<bool>> {
    match object.get(name) {
        None | Some(PyValue::Null) => Some(None),
        Some(_) => bool_or_default(validation, object, name, false).map(Some),
    }
}

/// The lax-int outcome for body values; `Overflow` reproduces the
/// backend's arbitrary-precision integers meeting the storage layer.
pub enum BodyInt {
    Value(i64),
    Overflow,
}

fn lax_int(value: &PyValue) -> Result<Option<BodyInt>, &'static str> {
    match value {
        PyValue::Int(v) => Ok(Some(BodyInt::Value(*v))),
        PyValue::BigInt(_) => Ok(Some(BodyInt::Overflow)),
        PyValue::Bool(b) => Ok(Some(BodyInt::Value(i64::from(*b)))),
        PyValue::Float(v) => {
            if v.is_finite() && v.fract() == 0.0 {
                // The backend's float-to-int gate excludes BOTH exact
                // bounds (probed: +/-2^63 floats answer the size 422).
                if *v > -9_223_372_036_854_775_808.0 && *v < 9_223_372_036_854_775_808.0 {
                    Ok(Some(BodyInt::Value(*v as i64)))
                } else {
                    Err("int_parsing_size")
                }
            } else if v.is_finite() {
                Err("int_from_float")
            } else {
                Err("int_parsing_size")
            }
        }
        PyValue::Str(raw) => match crate::extract::parse_int_lax(raw) {
            Some(crate::extract::LaxInt::Value(v)) => Ok(Some(BodyInt::Value(v))),
            Some(_) => Ok(Some(BodyInt::Overflow)),
            None => Err("int_parsing"),
        },
        _ => Err("int_type"),
    }
}

fn int_issue(validation: &mut Validation, loc: &[Loc], kind: &str, value: &PyValue) {
    let msg = match kind {
        "int_from_float" => "Input should be a valid integer, got a number with a fractional part",
        "int_parsing" => "Input should be a valid integer, unable to parse string as an integer",
        "int_parsing_size" => "Unable to parse input string as an integer, exceeded maximum size",
        _ => "Input should be a valid integer",
    };
    if let Some(echo) = echo_or_unrenderable(validation, value) {
        body_issue(validation, kind, loc, msg, &echo, None);
    }
}

/// `int | None`.
pub fn opt_int(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<BodyInt>> {
    let value = match object.get(name) {
        None | Some(PyValue::Null) => return Some(None),
        Some(value) => value,
    };
    match lax_int(value) {
        Ok(parsed) => Some(parsed),
        Err(kind) => {
            int_issue(validation, &[Loc::Field(name)], kind, value);
            None
        }
    }
}

/// A required lax int at an explicit location (nested items). The
/// enclosing object's echo arrives pre-checked (None marks the 500).
pub fn required_int_at(
    validation: &mut Validation,
    object_pairs: &[(String, PyValue)],
    object_echo: Option<&str>,
    name: &'static str,
    loc: &[Loc],
) -> Option<BodyInt> {
    let value = object_pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value);
    match value {
        None => {
            match object_echo {
                Some(echo) => {
                    body_issue(validation, "missing", loc, "Field required", echo, None);
                }
                None => validation.mark_unrenderable(),
            }
            None
        }
        Some(value) => match lax_int(value) {
            Ok(Some(parsed)) => Some(parsed),
            Ok(None) => unreachable!("lax_int never yields absent"),
            Err(kind) => {
                int_issue(validation, loc, kind, value);
                None
            }
        },
    }
}

/// An int field with a default (`int = 30`).
pub fn int_or_default(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
    default: i64,
) -> Option<BodyInt> {
    match object.get(name) {
        None => Some(BodyInt::Value(default)),
        Some(value) => match lax_int(value) {
            Ok(Some(parsed)) => Some(parsed),
            Ok(None) => unreachable!(),
            Err(kind) => {
                int_issue(validation, &[Loc::Field(name)], kind, value);
                None
            }
        },
    }
}

fn list_type_issue(validation: &mut Validation, loc: &[Loc], value: &PyValue) {
    if let Some(echo) = echo_or_unrenderable(validation, value) {
        body_issue(
            validation,
            "list_type",
            loc,
            "Input should be a valid list",
            &echo,
            None,
        );
    }
}

/// `list[str]` with a default of empty; items are strict strings.
pub fn list_of_str_or_default(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Vec<String>> {
    let value = match object.get(name) {
        None => return Some(Vec::new()),
        Some(value) => value,
    };
    let PyValue::List(items) = value else {
        list_type_issue(validation, &[Loc::Field(name)], value);
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match item {
            PyValue::Str(text) => out.push(text.clone()),
            PyValue::TaintedStr { lossy, .. } => {
                validation.note_binding_taint();
                out.push(lossy.clone());
            }
            other => {
                string_type_issue(validation, &[Loc::Field(name), Loc::Index(index)], other);
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// `list[str] | None`.
pub fn opt_list_of_str(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<Vec<String>>> {
    match object.get(name) {
        None | Some(PyValue::Null) => Some(None),
        Some(_) => list_of_str_or_default(validation, object, name).map(Some),
    }
}

/// `list[int]` with a default of empty (playlist quest ids).
pub fn list_of_int_or_default(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Vec<BodyInt>> {
    let value = match object.get(name) {
        None => return Some(Vec::new()),
        Some(value) => value,
    };
    let PyValue::List(items) = value else {
        list_type_issue(validation, &[Loc::Field(name)], value);
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match lax_int(item) {
            Ok(Some(parsed)) => out.push(parsed),
            Ok(None) => unreachable!(),
            Err(kind) => {
                int_issue(
                    validation,
                    &[Loc::Field(name), Loc::Index(index)],
                    kind,
                    item,
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// `list[int] | None`.
pub fn opt_list_of_int(
    validation: &mut Validation,
    object: &BodyObject,
    name: &'static str,
) -> Option<Option<Vec<BodyInt>>> {
    match object.get(name) {
        None | Some(PyValue::Null) => Some(None),
        Some(_) => list_of_int_or_default(validation, object, name).map(Some),
    }
}

/// The backend's unhandled-exception reply, for the legs that
/// reproduce it deliberately (an integer beyond the storage range).
pub fn internal_server_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("Internal Server Error"))
        .expect("static 500 builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> BodyObject {
        let mut validation = Validation::new();
        let object = read_object(Some("application/json"), body.as_bytes(), &mut validation)
            .expect("body parses");
        assert!(validation.is_ok());
        object
    }

    fn issue_json(validation: Validation) -> serde_json::Value {
        let response = validation.into_response();
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                use http_body_util::BodyExt;
                response.into_body().collect().await.unwrap().to_bytes()
            });
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn missing_body_and_non_json_content_types_report_backend_forms() {
        let mut v = Validation::new();
        assert!(read_object(Some("application/json"), b"", &mut v).is_none());
        let parsed = issue_json(v);
        assert_eq!(parsed["detail"][0]["type"], "missing");
        assert_eq!(parsed["detail"][0]["loc"], serde_json::json!(["body"]));

        let mut v = Validation::new();
        assert!(read_object(None, b"{\"name\": \"Q\"}", &mut v).is_none());
        let parsed = issue_json(v);
        assert_eq!(parsed["detail"][0]["type"], "model_attributes_type");
        assert_eq!(parsed["detail"][0]["input"], "{\"name\": \"Q\"}");
    }

    #[test]
    fn malformed_json_reports_the_scanner_position_and_message() {
        let mut v = Validation::new();
        assert!(read_object(Some("application/json"), b"{\"name\": \"Q\", }", &mut v).is_none());
        let parsed = issue_json(v);
        let issue = &parsed["detail"][0];
        assert_eq!(issue["type"], "json_invalid");
        assert_eq!(issue["loc"], serde_json::json!(["body", 12]));
        assert_eq!(issue["msg"], "JSON decode error");
        assert_eq!(
            issue["ctx"]["error"],
            "Illegal trailing comma before end of object"
        );
    }

    #[test]
    fn missing_fields_echo_the_whole_object() {
        let object = parse("{\"planet\": \"Calypso\"}");
        let mut v = Validation::new();
        assert!(required_str(&mut v, &object, "name").is_none());
        let parsed = issue_json(v);
        assert_eq!(
            parsed["detail"][0]["loc"],
            serde_json::json!(["body", "name"])
        );
        assert_eq!(
            parsed["detail"][0]["input"],
            serde_json::json!({"planet": "Calypso"})
        );
    }

    #[test]
    fn lax_coercions_match_the_probed_grid() {
        let object = parse(
            "{\"f1\": \"1.5\", \"f2\": \"1_000\", \"f3\": \"inf\", \"f4\": true, \
             \"b1\": \"yes\", \"b2\": \"No\", \"b3\": 1, \"b4\": 1.0, \
             \"i1\": 2.0, \"i2\": \"05\", \"i3\": true}",
        );
        let mut v = Validation::new();
        assert_eq!(opt_f64(&mut v, &object, "f1"), Some(Some(1.5)));
        assert_eq!(opt_f64(&mut v, &object, "f2"), Some(Some(1000.0)));
        assert_eq!(opt_f64(&mut v, &object, "f3"), Some(Some(f64::INFINITY)));
        assert_eq!(opt_f64(&mut v, &object, "f4"), Some(Some(1.0)));
        assert_eq!(bool_or_default(&mut v, &object, "b1", false), Some(true));
        assert_eq!(bool_or_default(&mut v, &object, "b2", true), Some(false));
        assert_eq!(bool_or_default(&mut v, &object, "b3", false), Some(true));
        assert_eq!(bool_or_default(&mut v, &object, "b4", false), Some(true));
        assert!(matches!(
            opt_int(&mut v, &object, "i1"),
            Some(Some(BodyInt::Value(2)))
        ));
        assert!(matches!(
            opt_int(&mut v, &object, "i2"),
            Some(Some(BodyInt::Value(5)))
        ));
        assert!(matches!(
            opt_int(&mut v, &object, "i3"),
            Some(Some(BodyInt::Value(1)))
        ));
        assert!(v.is_ok());
    }

    #[test]
    fn rejections_match_the_probed_grid() {
        let object = parse(
            "{\"f\": \"abc\", \"b\": \"enabled\", \"b2\": 2, \"b3\": \"TRUE \", \
             \"i\": 2.5, \"l\": \"Atrox\", \"l2\": [\"A\", 5]}",
        );
        let mut v = Validation::new();
        assert!(opt_f64(&mut v, &object, "f").is_none());
        assert!(bool_or_default(&mut v, &object, "b", false).is_none());
        assert!(bool_or_default(&mut v, &object, "b2", false).is_none());
        assert!(bool_or_default(&mut v, &object, "b3", false).is_none());
        assert!(opt_int(&mut v, &object, "i").is_none());
        assert!(list_of_str_or_default(&mut v, &object, "l").is_none());
        assert!(list_of_str_or_default(&mut v, &object, "l2").is_none());
        let parsed = issue_json(v);
        let kinds: Vec<&str> = parsed["detail"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "float_parsing",
                "bool_parsing",
                "bool_parsing",
                "bool_parsing",
                "int_from_float",
                "list_type",
                "string_type"
            ]
        );
        // The list-item error carries its index in the loc.
        assert_eq!(
            parsed["detail"][6]["loc"],
            serde_json::json!(["body", "l2", 1])
        );
    }

    #[test]
    fn overflow_integers_surface_as_overflow_not_panic() {
        let object =
            parse("{\"i\": 999999999999999999999999, \"s\": \"999999999999999999999999\"}");
        let mut v = Validation::new();
        assert!(matches!(
            opt_int(&mut v, &object, "i"),
            Some(Some(BodyInt::Overflow))
        ));
        assert!(matches!(
            opt_int(&mut v, &object, "s"),
            Some(Some(BodyInt::Overflow))
        ));
        assert!(v.is_ok());
    }

    #[test]
    fn non_finite_echoes_answer_the_reference_500() {
        // The backend's response serialiser refuses non-finite floats,
        // so any envelope that would echo one crashes into the
        // plain-text 500 (probed live); the whole-object echo is
        // therefore unavailable and the report renders that 500.
        let object = parse("{\"f\": Infinity}");
        assert_eq!(object.echo(), None);
        let mut v = Validation::new();
        assert!(required_str(&mut v, &object, "name").is_none());
        assert!(!v.is_ok());
        let response = v.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                use http_body_util::BodyExt;
                response.into_body().collect().await.unwrap().to_bytes()
            });
        assert_eq!(bytes.as_ref(), b"Internal Server Error");
        // A non-finite value that VALIDATES (a float field) raises no
        // issue and echoes nothing: the request proceeds.
        let mut v = Validation::new();
        assert_eq!(opt_f64(&mut v, &object, "f"), Some(Some(f64::INFINITY)));
        assert!(v.is_ok());
    }

    #[test]
    fn optional_bodies_tolerate_absence() {
        let mut v = Validation::new();
        assert!(matches!(
            read_optional_object(None, b"", &mut v),
            Some(None)
        ));
        assert!(v.is_ok());
        let mut v = Validation::new();
        assert!(matches!(
            read_optional_object(Some("application/json"), b"{\"undo_reward\": true}", &mut v),
            Some(Some(_))
        ));
    }
}
