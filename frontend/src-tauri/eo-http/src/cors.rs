//! The backend's CORS contract, reproduced byte-for-byte for the
//! natively-served arm.
//!
//! The backend mounts its CORS middleware outermost: a preflight
//! (OPTIONS carrying `Origin` and `Access-Control-Request-Method`)
//! short-circuits before routing, the Host guard, and the origin
//! guard, answering `200 OK` (plain text) with the allow headers when
//! every check passes and `400 Disallowed CORS ...` when one fails;
//! every other response on a request with an allowed `Origin` is
//! decorated with `Access-Control-Allow-Origin` plus `Vary: Origin`.
//! All forms here are pinned by the cross-language battery against the
//! running backend.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};

/// The methods the backend allows, in its configured order (the
/// preflight allow-methods header reproduces this order verbatim).
const ALLOW_METHODS: &[&str] = &["GET", "POST", "PATCH", "PUT", "DELETE", "OPTIONS"];

/// The preflight allow-headers value: the CORS-safelisted names plus
/// the backend's configured `content-type`, sorted case-sensitively
/// exactly as the backend emits them.
const ALLOW_HEADERS_VALUE: &str =
    "Accept, Accept-Language, Content-Language, Content-Type, content-type";

/// The lowercased set a preflight's requested headers must fall
/// within.
const ALLOWED_REQUEST_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "content-language",
    "content-type",
];

/// The origin allowlist and derived header values, mirroring the
/// backend's `ALLOWED_API_ORIGINS` construction.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    allowed_origins: Vec<String>,
}

impl CorsConfig {
    /// The backend's origin set for a given frontend port and optional
    /// per-checkout dev hostname (already validated to end with
    /// `.localhost` by whoever read the environment).
    pub fn new(frontend_port: u16, per_checkout_hostname: Option<&str>) -> Self {
        let mut allowed_origins = vec![
            "tauri://localhost".to_string(),
            "http://tauri.localhost".to_string(),
            format!("http://localhost:{frontend_port}"),
            format!("http://127.0.0.1:{frontend_port}"),
            "https://entropiaorme.localhost".to_string(),
        ];
        if let Some(hostname) = per_checkout_hostname {
            allowed_origins.push(format!("https://{hostname}"));
        }
        Self { allowed_origins }
    }

    /// The backend reads `ENTROPIAORME_FRONTEND_PORT` (default 5173)
    /// and `ENTROPIAORME_HOSTNAME` (a `.localhost` dev hostname); the
    /// same environment drives the substrate's copy of the allowlist.
    pub fn from_env() -> Self {
        let frontend_port = std::env::var("ENTROPIAORME_FRONTEND_PORT")
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
            .unwrap_or(5173);
        let hostname = std::env::var("ENTROPIAORME_HOSTNAME")
            .ok()
            .filter(|name| name.ends_with(".localhost"));
        Self::new(frontend_port, hostname.as_deref())
    }

    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|o| o == origin)
    }

    /// The preflight reply for an OPTIONS request carrying `Origin`
    /// and `Access-Control-Request-Method`: the backend's exact header
    /// set and plain-text body, failures named in its fixed order.
    pub fn preflight_response(&self, headers: &HeaderMap) -> Response<Body> {
        let origin = header_str(headers, header::ORIGIN);
        let requested_method = header_str(headers, header::ACCESS_CONTROL_REQUEST_METHOD);
        let requested_headers = header_str(headers, header::ACCESS_CONTROL_REQUEST_HEADERS);

        let origin_ok = origin.is_some_and(|o| self.origin_allowed(o));
        let method_ok = requested_method.is_some_and(|m| ALLOW_METHODS.contains(&m));
        let headers_ok = requested_headers.is_none_or(|list| {
            list.split(',').all(|name| {
                ALLOWED_REQUEST_HEADERS.contains(&name.trim().to_ascii_lowercase().as_str())
            })
        });

        let mut failures = Vec::new();
        if !origin_ok {
            failures.push("origin");
        }
        if !method_ok {
            failures.push("method");
        }
        if !headers_ok {
            failures.push("headers");
        }

        let (status, body) = if failures.is_empty() {
            (StatusCode::OK, "OK".to_string())
        } else {
            (
                StatusCode::BAD_REQUEST,
                format!("Disallowed CORS {}", failures.join(", ")),
            )
        };
        let mut response = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                ALLOW_METHODS.join(", "),
            )
            .header(header::ACCESS_CONTROL_MAX_AGE, "600")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, ALLOW_HEADERS_VALUE)
            .header(header::VARY, "Origin");
        if origin_ok {
            response = response.header(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                origin.expect("origin checked"),
            );
        }
        response
            .body(Body::from(body))
            .expect("preflight response builds")
    }
}

/// Whether a request is a CORS preflight in the backend's sense: an
/// OPTIONS carrying both `Origin` and `Access-Control-Request-Method`.
pub fn is_preflight(method: &http::Method, headers: &HeaderMap) -> bool {
    method == http::Method::OPTIONS
        && headers.contains_key(header::ORIGIN)
        && headers.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

/// Decorate a natively-served response for an allowed origin, exactly
/// as the backend's middleware decorates every response: the specific
/// origin echoed, `Origin` appended to `Vary`. Responses that already
/// carry an allow-origin header (the proxied arm, decorated upstream)
/// are left untouched.
pub fn decorate(response: &mut Response<Body>, origin: &HeaderValue) {
    if response
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    {
        return;
    }
    response
        .headers_mut()
        .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    let vary = match response.headers().get(header::VARY) {
        Some(existing) => {
            let existing = existing.to_str().unwrap_or_default();
            HeaderValue::from_str(&format!("{existing}, Origin"))
                .unwrap_or(HeaderValue::from_static("Origin"))
        }
        None => HeaderValue::from_static("Origin"),
    };
    response.headers_mut().insert(header::VARY, vary);
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preflight_headers(origin: &str, method: &str, requested: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, origin.parse().unwrap());
        headers.insert(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            method.parse().unwrap(),
        );
        if let Some(list) = requested {
            headers.insert(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                list.parse().unwrap(),
            );
        }
        headers
    }

    fn body_text(response: Response<Body>) -> (StatusCode, HeaderMap, String) {
        let (parts, body) = response.into_parts();
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                use http_body_util::BodyExt;
                body.collect().await.unwrap().to_bytes().to_vec()
            });
        (
            parts.status,
            parts.headers,
            String::from_utf8(bytes).unwrap(),
        )
    }

    #[test]
    fn the_origin_set_mirrors_the_backend() {
        let cors = CorsConfig::new(5173, Some("entropiaorme-lane.localhost"));
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "https://entropiaorme.localhost",
            "https://entropiaorme-lane.localhost",
        ] {
            assert!(cors.origin_allowed(origin), "{origin}");
        }
        assert!(!cors.origin_allowed("http://evil.example"));
        assert!(!cors.origin_allowed("http://localhost:5174"));
    }

    #[test]
    fn a_passing_preflight_answers_ok_with_the_backend_headers() {
        let cors = CorsConfig::new(5173, None);
        let (status, headers, body) = body_text(cors.preflight_response(&preflight_headers(
            "tauri://localhost",
            "GET",
            None,
        )));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "OK");
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_METHODS).unwrap(),
            "GET, POST, PATCH, PUT, DELETE, OPTIONS"
        );
        assert_eq!(headers.get(header::ACCESS_CONTROL_MAX_AGE).unwrap(), "600");
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_HEADERS).unwrap(),
            ALLOW_HEADERS_VALUE
        );
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "tauri://localhost"
        );
        assert_eq!(headers.get(header::VARY).unwrap(), "Origin");
    }

    #[test]
    fn failing_preflights_name_their_failures_in_order() {
        let cors = CorsConfig::new(5173, None);
        let (status, headers, body) = body_text(cors.preflight_response(&preflight_headers(
            "http://evil.example",
            "GET",
            None,
        )));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "Disallowed CORS origin");
        assert!(
            !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "a disallowed origin is not echoed"
        );

        let (_, _, body) = body_text(cors.preflight_response(&preflight_headers(
            "tauri://localhost",
            "TRACE",
            None,
        )));
        assert_eq!(body, "Disallowed CORS method");

        let (_, _, body) = body_text(cors.preflight_response(&preflight_headers(
            "tauri://localhost",
            "GET",
            Some("if-none-match"),
        )));
        assert_eq!(body, "Disallowed CORS headers");

        let (_, _, body) = body_text(cors.preflight_response(&preflight_headers(
            "http://evil.example",
            "TRACE",
            Some("x-custom"),
        )));
        assert_eq!(body, "Disallowed CORS origin, method, headers");
    }

    #[test]
    fn safelisted_request_headers_pass_case_insensitively() {
        let cors = CorsConfig::new(5173, None);
        let (status, _, _) = body_text(cors.preflight_response(&preflight_headers(
            "tauri://localhost",
            "POST",
            Some("Content-Type, accept"),
        )));
        assert_eq!(status, StatusCode::OK);
    }

    #[test]
    fn decoration_echoes_the_origin_and_appends_vary() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
        decorate(
            &mut response,
            &HeaderValue::from_static("tauri://localhost"),
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "tauri://localhost"
        );
        assert_eq!(response.headers().get(header::VARY).unwrap(), "Origin");

        // An already-decorated (proxied) response is left untouched.
        let mut proxied = Response::builder()
            .status(StatusCode::OK)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "tauri://localhost")
            .header(header::VARY, "Origin")
            .body(Body::empty())
            .unwrap();
        decorate(&mut proxied, &HeaderValue::from_static("tauri://localhost"));
        assert_eq!(proxied.headers().get(header::VARY).unwrap(), "Origin");

        // An existing Vary gains Origin alongside it.
        let mut varied = Response::builder()
            .status(StatusCode::OK)
            .header(header::VARY, "Accept-Encoding")
            .body(Body::empty())
            .unwrap();
        decorate(&mut varied, &HeaderValue::from_static("tauri://localhost"));
        assert_eq!(
            varied.headers().get(header::VARY).unwrap(),
            "Accept-Encoding, Origin"
        );
    }
}
