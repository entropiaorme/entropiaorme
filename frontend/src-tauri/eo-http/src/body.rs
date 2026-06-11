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
use crate::pyjson::{loads, PyValue};

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
    echo: String,
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

    pub fn echo(&self) -> &str {
        &self.echo
    }
}

/// Read and shape a request body as the backend does. `None` means
/// issues were recorded (or, for `optional_body` routes with an empty
/// body, the caller sees `Ok(None)` via [`read_optional_object`]).
pub fn read_object(
    content_type: Option<&str>,
    bytes: &[u8],
    validation: &mut Validation,
) -> Option<BodyObject> {
    match read_value(content_type, bytes, validation)? {
        PyValue::Object(pairs) => {
            let echo = PyValue::Object(pairs.clone()).to_echo_json();
            Some(BodyObject { pairs, echo })
        }
        other => {
            body_issue(
                validation,
                "model_attributes_type",
                &[],
                "Input should be a valid dictionary or object to extract fields from",
                &other.to_echo_json(),
                None,
            );
            None
        }
    }
}

/// The optional-body variant (a route whose body model defaults to
/// None): an absent body is `Ok(None)`, anything present validates as
/// usual.
pub fn read_optional_object(
    content_type: Option<&str>,
    bytes: &[u8],
    validation: &mut Validation,
) -> Result<Option<BodyObject>, ()> {
    if bytes.is_empty() {
        return Ok(None);
    }
    match read_object(content_type, bytes, validation) {
        Some(object) => Ok(Some(object)),
        None => Err(()),
    }
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
    let is_json = content_type
        .map(|value| {
            let media = value.split(';').next().unwrap_or("").trim();
            media.eq_ignore_ascii_case("application/json") || media.ends_with("+json")
        })
        .unwrap_or(false);
    let text = String::from_utf8_lossy(bytes);
    if !is_json {
        // Without a JSON content type the backend treats the raw text
        // as the submitted value (a string), which then fails the
        // object check with that string echoed.
        return Some(PyValue::Str(text.into_owned()));
    }
    match loads(&text) {
        Ok(value) => Some(value),
        Err(error) => {
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
            body_issue(
                validation,
                "missing",
                &[Loc::Field(name)],
                "Field required",
                object.echo(),
                None,
            );
            None
        }
        Some(PyValue::Str(value)) => Some(value.clone()),
        Some(other) => {
            string_type_issue(validation, &[Loc::Field(name)], other);
            None
        }
    }
}

fn string_type_issue(validation: &mut Validation, loc: &[Loc], value: &PyValue) {
    body_issue(
        validation,
        "string_type",
        loc,
        "Input should be a valid string",
        &value.to_echo_json(),
        None,
    );
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
        Some(other) => {
            string_type_issue(validation, &[Loc::Field(name)], other);
            None
        }
    }
}

/// The backend's lax string-to-float: whitespace trim, Python digit
/// underscores, and the `inf`/`infinity`/`nan` names.
fn lax_float_from_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (negative, rest) = match trimmed.strip_prefix(['+', '-']) {
        Some(rest) => (trimmed.starts_with('-'), rest),
        None => (false, trimmed),
    };
    let lowered = rest.to_ascii_lowercase();
    let magnitude = match lowered.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => {
            // Python float() permits underscores between digits.
            let mut cleaned = String::with_capacity(rest.len());
            let chars: Vec<char> = rest.chars().collect();
            for (index, ch) in chars.iter().enumerate() {
                if *ch == '_' {
                    let prev = index > 0 && chars[index - 1].is_ascii_digit();
                    let next = index + 1 < chars.len() && chars[index + 1].is_ascii_digit();
                    if !(prev && next) {
                        return None;
                    }
                } else {
                    cleaned.push(*ch);
                }
            }
            if cleaned.is_empty() || !cleaned.starts_with(|c: char| c.is_ascii_digit() || c == '.')
            {
                return None;
            }
            cleaned.parse::<f64>().ok()?
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
                body_issue(
                    validation,
                    "float_parsing",
                    &[Loc::Field(name)],
                    "Input should be a valid number, unable to parse string as a number",
                    &value.to_echo_json(),
                    None,
                );
                None
            }
        },
        other => {
            body_issue(
                validation,
                "float_type",
                &[Loc::Field(name)],
                "Input should be a valid number",
                &other.to_echo_json(),
                None,
            );
            None
        }
    }
}

/// The backend's lax boolean set: exact strings (case-insensitive, no
/// whitespace tolerance), 0/1 numbers.
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
    match parsed {
        Some(b) => Some(b),
        None => {
            body_issue(
                validation,
                "bool_parsing",
                &[Loc::Field(name)],
                "Input should be a valid boolean, unable to interpret input",
                &value.to_echo_json(),
                None,
            );
            None
        }
    }
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
                if *v >= i64::MIN as f64 && *v <= i64::MAX as f64 {
                    Ok(Some(BodyInt::Value(*v as i64)))
                } else {
                    Ok(Some(BodyInt::Overflow))
                }
            } else {
                Err("int_from_float")
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
        _ => "Input should be a valid integer",
    };
    body_issue(validation, kind, loc, msg, &value.to_echo_json(), None);
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

/// A required lax int at an explicit location (nested items).
pub fn required_int_at(
    validation: &mut Validation,
    object_pairs: &[(String, PyValue)],
    object_echo: &str,
    name: &'static str,
    loc: &[Loc],
) -> Option<BodyInt> {
    let value = object_pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value);
    match value {
        None => {
            body_issue(
                validation,
                "missing",
                loc,
                "Field required",
                object_echo,
                None,
            );
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
    body_issue(
        validation,
        "list_type",
        loc,
        "Input should be a valid list",
        &value.to_echo_json(),
        None,
    );
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
    fn non_finite_echoes_render_the_reference_forms() {
        let object = parse("{\"f\": Infinity}");
        assert_eq!(object.echo(), "{\"f\":Infinity}");
        let mut v = Validation::new();
        // A required string fed Infinity echoes the non-finite value.
        assert!(required_str(&mut v, &object, "f").is_none());
        let response = v.into_response();
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                use http_body_util::BodyExt;
                response.into_body().collect().await.unwrap().to_bytes()
            });
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("\"input\":Infinity"), "{text}");
    }

    #[test]
    fn optional_bodies_tolerate_absence() {
        let mut v = Validation::new();
        assert!(matches!(read_optional_object(None, b"", &mut v), Ok(None)));
        assert!(v.is_ok());
        let mut v = Validation::new();
        assert!(matches!(
            read_optional_object(Some("application/json"), b"{\"undo_reward\": true}", &mut v),
            Ok(Some(_))
        ));
    }
}
