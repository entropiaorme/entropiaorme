//! The reverse-proxy arm: forwards a request to the relocated Python
//! sidecar and streams the response back untouched.
//!
//! Fidelity contract (the axes the HTTP-response goldens project, plus the
//! server-sent-event stream): status, content-type, cache-control, etag and
//! body bytes pass through unmodified; response frames are streamed as they
//! arrive so the event stream's prompt `: ready` flush and keep-alive
//! comments reach the webview unbuffered. Hop-by-hop headers are consumed
//! at each hop per RFC 9110 section 7.6.1: they describe the connection,
//! not the resource, and forwarding them corrupts connection management.

use axum::body::Body;
use axum::response::Response;
use http::header::{HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST};
use http::{Request, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

/// HTTP/1.1 client shared by every proxied request (connection-pooled).
pub type ProxyClient = Client<HttpConnector, Body>;

pub fn build_client() -> ProxyClient {
    let mut connector = HttpConnector::new();
    // Loopback connects resolve immediately or not at all; the bound turns
    // a wedged upstream into a prompt 502 instead of a request hung on the
    // OS connect default. Deliberately no response timeout: the event
    // stream is an unbounded body by design.
    connector.set_connect_timeout(Some(std::time::Duration::from_secs(5)));
    Client::builder(TokioExecutor::new()).build(connector)
}

const HOP_BY_HOP: [HeaderName; 8] = [
    HeaderName::from_static("connection"),
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-authenticate"),
    HeaderName::from_static("proxy-authorization"),
    HeaderName::from_static("te"),
    HeaderName::from_static("trailer"),
    HeaderName::from_static("transfer-encoding"),
    HeaderName::from_static("upgrade"),
];

/// Remove hop-by-hop headers: the fixed RFC set plus any header the
/// Connection header names.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let connection_named: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| token.trim().parse::<HeaderName>().ok())
        .collect();
    for name in connection_named {
        headers.remove(&name);
    }
    for name in &HOP_BY_HOP {
        headers.remove(name);
    }
}

fn bad_gateway(detail: &str) -> Response {
    let body = serde_json::json!({ "detail": detail }).to_string();
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static 502 response builds")
}

/// Forward `req` to `upstream` (a `host:port` authority) and stream the
/// response back. Any transport-level failure maps to 502; the sidecar's
/// own HTTP responses (including its 4xx/5xx) pass through verbatim.
pub async fn forward(client: &ProxyClient, upstream: &str, mut req: Request<Body>) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let uri: http::Uri = match format!("http://{upstream}{path_and_query}").parse() {
        Ok(uri) => uri,
        Err(_) => return bad_gateway("upstream uri rebuild failed"),
    };
    *req.uri_mut() = uri;

    strip_hop_by_hop(req.headers_mut());
    // The sidecar guards its Host header against its own bind authority;
    // a reverse proxy speaks for itself upstream, so the Host is rewritten
    // to the authority actually being dialled.
    match HeaderValue::from_str(upstream) {
        Ok(host) => {
            req.headers_mut().insert(HOST, host);
        }
        Err(_) => return bad_gateway("upstream authority not a valid host header"),
    }

    match client.request(req).await {
        Ok(response) => {
            let (mut parts, body) = response.into_parts();
            strip_hop_by_hop(&mut parts.headers);
            Response::from_parts(parts, Body::new(body))
        }
        Err(_) => bad_gateway("backend upstream unreachable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_fixed_set_and_connection_named() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("close, x-custom"));
        headers.insert("x-custom", HeaderValue::from_static("1"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("etag", HeaderValue::from_static("\"abc\""));
        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
        strip_hop_by_hop(&mut headers);
        assert!(headers.get(CONNECTION).is_none());
        assert!(headers.get("x-custom").is_none());
        assert!(headers.get("transfer-encoding").is_none());
        assert_eq!(headers.get("etag").unwrap(), "\"abc\"");
        assert_eq!(headers.get("cache-control").unwrap(), "no-cache");
    }
}
