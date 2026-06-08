//! HTTP request/response fingerprint emitter: Rust port of
//! `backend/testing/http_fingerprint.py`.
//!
//! For one captured response this reproduces the canonical
//! `(request, normalised-response)` golden the committed
//! `expected/http_responses/<endpoint_id>.json` holds: the header projection
//! (`Content-Type`/`Cache-Control`/`ETag`, with a strong ETag reduced to the
//! `<STRONG_ETAG>` sentinel), the UUID-segment path normalisation, the body
//! walked through the shared [`Normalizer`] (with the wall-clock session
//! `duration` reduced to `<SESSION_DURATION>`), and binary bodies projected to
//! their byte length.

use serde_json::{Map, Value};

use crate::normalizer::{to_python_json, Normalizer};

/// Only these response headers are pinned (lower-cased); everything else
/// varies across environments. Mirrors `PROJECTED_HEADERS`.
const PROJECTED_HEADERS: [&str; 3] = ["content-type", "cache-control", "etag"];

const ETAG_SENTINEL: &str = "<STRONG_ETAG>";
const SESSION_DURATION_SENTINEL: &str = "<SESSION_DURATION>";

/// One captured response in the wire-portable form the raw-capture fixture
/// holds: the request line plus the raw response (status, headers, body bytes).
pub struct RawResponse<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a Map<String, Value>,
    pub status_code: i64,
    pub headers: &'a Map<String, Value>,
    pub body: &'a [u8],
}

/// Whether `value` is a strong-format ETag (`"<sha256-hex>"`), matching
/// `STRONG_ETAG_RE` (`^"[0-9a-f]{64}"$`).
fn is_strong_etag(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 66 {
        return false;
    }
    if bytes[0] != b'"' || bytes[65] != b'"' {
        return false;
    }
    bytes[1..65]
        .iter()
        .all(|&c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Project an ETag header into the golden's canonical form (strong -> sentinel,
/// anything else verbatim). Mirrors `normalise_etag`.
fn normalise_etag(value: &str) -> String {
    if is_strong_etag(value) {
        ETAG_SENTINEL.to_string()
    } else {
        value.to_string()
    }
}

/// Filter raw headers to the projection the golden tracks, in `PROJECTED_HEADERS`
/// order. Header names are matched case-insensitively; a missing one is skipped.
/// Mirrors `project_headers` (which builds a dict later serialised sorted).
fn project_headers(raw_headers: &Map<String, Value>) -> Map<String, Value> {
    let mut lowered: Map<String, Value> = Map::new();
    for (key, value) in raw_headers {
        lowered.insert(key.to_lowercase(), value.clone());
    }
    let mut projected = Map::new();
    for name in PROJECTED_HEADERS {
        let Some(Value::String(value)) = lowered.get(name) else {
            continue;
        };
        if name == "etag" {
            projected.insert(name.to_string(), Value::String(normalise_etag(value)));
        } else {
            projected.insert(name.to_string(), Value::String(value.clone()));
        }
    }
    projected
}

/// Whether the 36 bytes at `pos` form a canonical lowercase-hex UUID (the
/// inline, unanchored `_UUID_IN_PATH_RE` shape).
fn uuid_at(bytes: &[u8], pos: usize) -> bool {
    if pos + 36 > bytes.len() {
        return false;
    }
    for offset in 0..36 {
        let c = bytes[pos + offset];
        let ok = match offset {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Replace each UUID segment in `path` with its `<UUID_N>` symbol, sharing the
/// body's symbol table. Mirrors `normalise_path`'s `re.sub` over the inline
/// (unanchored, fixed-length) UUID pattern: leftmost non-overlapping matches.
fn normalise_path(path: &str, normalizer: &mut Normalizer) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if uuid_at(bytes, i) {
            let uuid = &path[i..i + 36];
            let symbol = normalizer.normalize(&Value::String(uuid.to_string()));
            match symbol {
                Value::String(s) => out.push_str(&s),
                _ => out.push_str(uuid),
            }
            i += 36;
        } else {
            // ASCII path; UUID runs are ASCII so a byte step never splits a
            // multi-byte char at a match site. Copy this byte's char.
            let ch = path[i..].chars().next().expect("non-empty remainder");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Render the response body into the golden's canonical form. JSON bodies are
/// parsed, normalised, and have their session `duration` projected; empty
/// bodies become `null`; anything else projects to `{"_binary": true,
/// "byte_length": N}`. Mirrors `normalise_body`.
fn normalise_body(body: &[u8], content_type: Option<&str>, normalizer: &mut Normalizer) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    if let Some(ct) = content_type {
        if ct.to_lowercase().contains("application/json") {
            let decoded: Value = serde_json::from_slice(body).expect("json body parses as JSON");
            return project_session_duration(normalizer.normalize(&decoded));
        }
    }
    let mut binary = Map::new();
    binary.insert("_binary".to_string(), Value::Bool(true));
    binary.insert("byte_length".to_string(), Value::Number(body.len().into()));
    Value::Object(binary)
}

/// Replace any numeric `duration` with the wall-clock sentinel. Mirrors
/// `_project_session_duration` (post-normalisation, so an epoch `duration`
/// already symbolised to a string is left alone; only a numeric one is hit).
fn project_session_duration(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, val) in map {
                out.insert(key, project_session_duration(val));
            }
            if out.get("duration").map(Value::is_number).unwrap_or(false) {
                out.insert(
                    "duration".to_string(),
                    Value::String(SESSION_DURATION_SENTINEL.to_string()),
                );
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(project_session_duration).collect())
        }
        other => other,
    }
}

/// Normalise a captured response into its golden value. The body is normalised
/// before the path, matching the Python `capture`'s order (so a UUID first seen
/// in the body owns the lower symbol when it recurs in the path).
pub fn capture(raw: &RawResponse, normalizer: &mut Normalizer) -> Value {
    let content_type = raw
        .headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "content-type")
        .and_then(|(_, v)| v.as_str());

    let headers = project_headers(raw.headers);
    let body = normalise_body(raw.body, content_type, normalizer);
    let path = normalise_path(raw.path, normalizer);

    let mut request = Map::new();
    request.insert("method".to_string(), Value::String(raw.method.to_string()));
    request.insert("path".to_string(), Value::String(path));
    request.insert("query".to_string(), Value::Object(raw.query.clone()));

    let mut response = Map::new();
    response.insert(
        "status_code".to_string(),
        Value::Number(raw.status_code.into()),
    );
    response.insert("headers".to_string(), Value::Object(headers));
    response.insert("body".to_string(), body);

    let mut golden = Map::new();
    golden.insert("request".to_string(), Value::Object(request));
    golden.insert("response".to_string(), Value::Object(response));
    Value::Object(golden)
}

/// Serialise a captured golden value as the committed golden text (sorted keys,
/// 2-space indent, trailing newline), matching `HttpFingerprinter._write_golden`.
pub fn serialize_capture(capture: &Value) -> String {
    to_python_json(capture, Some(2)) + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn headers(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn strong_etag_reduces_to_sentinel() {
        let hex = "a".repeat(64);
        assert!(is_strong_etag(&format!("\"{hex}\"")));
        assert_eq!(normalise_etag(&format!("\"{hex}\"")), "<STRONG_ETAG>");
        // A weak ETag is kept verbatim so an unexpected shape surfaces.
        assert_eq!(normalise_etag("W/\"abc\""), "W/\"abc\"");
    }

    #[test]
    fn headers_project_to_the_three_pinned_lowercased() {
        let raw = headers(&[
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-cache"),
            ("Content-Length", "42"),
            ("Server", "uvicorn"),
        ]);
        let projected = project_headers(&raw);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected["content-type"], json!("application/json"));
        assert_eq!(projected["cache-control"], json!("no-cache"));
        assert!(!projected.contains_key("content-length"));
    }

    #[test]
    fn path_uuid_segment_becomes_symbol() {
        let mut norm = Normalizer::new();
        let path = normalise_path(
            "/api/tracking/session/11111111-1111-1111-1111-111111111111/quest-link-suggestion",
            &mut norm,
        );
        assert_eq!(path, "/api/tracking/session/<UUID_1>/quest-link-suggestion");
    }

    #[test]
    fn numeric_duration_projects_to_sentinel() {
        let body = json!({"sessions": [{"duration": 42, "id": 1}], "duration": 7.0});
        let projected = project_session_duration(body);
        assert_eq!(projected["duration"], json!("<SESSION_DURATION>"));
        assert_eq!(
            projected["sessions"][0]["duration"],
            json!("<SESSION_DURATION>")
        );
        assert_eq!(projected["sessions"][0]["id"], json!(1));
    }

    #[test]
    fn empty_body_is_null_and_binary_projects_byte_length() {
        let mut norm = Normalizer::new();
        assert_eq!(
            normalise_body(b"", Some("application/json"), &mut norm),
            Value::Null
        );
        assert_eq!(
            normalise_body(&[0u8, 1, 2], Some("image/png"), &mut norm),
            json!({"_binary": true, "byte_length": 3})
        );
    }
}
