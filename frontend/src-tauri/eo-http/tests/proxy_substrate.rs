//! Hermetic proxy-substrate tests against a stub upstream.
//!
//! These prove the forwarding contract without a Python toolchain: byte
//! round-trips on the projected golden axes (status, content-type,
//! cache-control, etag, body), Host rewrite to the upstream authority,
//! hop-by-hop consumption, unbuffered streaming (the event-stream `: ready`
//! prompt-flush property), runtime arm dispatch, and the dead-upstream 502.
//! The cross-language sibling (`proxy_fidelity.rs`) re-proves the same axes
//! against the real sidecar.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use eo_http::arms::ArmOverrides;
use eo_http::{arm_routed, build_router, AppState};
use http::header::HOST;
use http_body_util::BodyExt;
use tokio::net::TcpListener;

/// Bind a router on an ephemeral loopback port and serve it.
async fn spawn_server(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// The stub sidecar: canned responses that exercise each forwarding axis.
fn stub_upstream() -> Router {
    Router::new()
        .route(
            "/api/echo",
            post(|req: Request| async move {
                let (parts, body) = req.into_parts();
                let bytes = body.collect().await.unwrap().to_bytes();
                let seen_host = parts
                    .headers
                    .get(HOST)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let seen_custom_conn = parts.headers.contains_key("x-conn-scoped");
                let query = parts.uri.query().unwrap_or("").to_string();
                Response::builder()
                    .status(http::StatusCode::CREATED)
                    .header("content-type", "application/json")
                    .header("cache-control", "no-store")
                    .header("etag", "\"stub-etag-1\"")
                    .header("x-seen-host", seen_host)
                    .header("x-seen-conn-scoped", seen_custom_conn.to_string())
                    .header("x-seen-query", query)
                    .body(Body::from(bytes))
                    .unwrap()
            }),
        )
        .route(
            "/api/stream",
            get(|| async {
                // First frame immediately, then hold the stream open: a
                // buffering proxy would never deliver the first frame.
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<_, std::io::Error>>(4);
                tx.try_send(Ok(hyper::body::Frame::data(bytes::Bytes::from(
                    ": ready\n\n",
                ))))
                .unwrap();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    drop(tx);
                });
                let body = http_body_util::StreamBody::new(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                );
                Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-cache")
                    .header("x-accel-buffering", "no")
                    .body(Body::new(body))
                    .unwrap()
            }),
        )
        .route("/api/native-candidate", get(|| async { "from-upstream" }))
        .fallback(|| async { (http::StatusCode::NOT_FOUND, "stub 404") })
}

async fn spawn_proxy(upstream: SocketAddr, overrides: ArmOverrides) -> (SocketAddr, Arc<AppState>) {
    // Bind before building state: the Host allowlist derives from the
    // public port actually bound.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(AppState::new(upstream.to_string(), addr.port(), overrides));
    let router = build_router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, state)
}

async fn body_bytes(response: Response<hyper::body::Incoming>) -> Vec<u8> {
    response.collect().await.unwrap().to_bytes().to_vec()
}

fn client() -> eo_http::proxy::ProxyClient {
    eo_http::proxy::build_client()
}

#[tokio::test]
async fn round_trips_method_path_query_body_and_projected_headers() {
    let upstream = spawn_server(stub_upstream()).await;
    let (proxy_addr, _state) = spawn_proxy(upstream, ArmOverrides::empty()).await;

    let payload = vec![0u8, 159, 146, 150, 1, 2, 3];
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{proxy_addr}/api/echo?a=1&b=two"))
        .header("content-type", "application/octet-stream")
        .header("connection", "x-conn-scoped")
        .header("x-conn-scoped", "should-not-cross")
        .body(Body::from(payload.clone()))
        .unwrap();
    let response = client().request(req).await.unwrap();

    assert_eq!(response.status(), http::StatusCode::CREATED);
    let headers = response.headers().clone();
    assert_eq!(headers.get("content-type").unwrap(), "application/json");
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(headers.get("etag").unwrap(), "\"stub-etag-1\"");
    // Host crossed rewritten to the upstream authority, not the proxy's.
    assert_eq!(
        headers.get("x-seen-host").unwrap(),
        upstream.to_string().as_str()
    );
    // The Connection-named header was consumed at the proxy hop.
    assert_eq!(headers.get("x-seen-conn-scoped").unwrap(), "false");
    assert_eq!(headers.get("x-seen-query").unwrap(), "a=1&b=two");
    assert_eq!(body_bytes(response).await, payload);
}

#[tokio::test]
async fn upstream_status_and_404_pass_through() {
    let upstream = spawn_server(stub_upstream()).await;
    let (proxy_addr, _state) = spawn_proxy(upstream, ArmOverrides::empty()).await;

    let req = http::Request::builder()
        .uri(format!("http://{proxy_addr}/api/no-such-route"))
        .body(Body::empty())
        .unwrap();
    let response = client().request(req).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    assert_eq!(body_bytes(response).await, b"stub 404");
}

#[tokio::test]
async fn streams_first_frame_without_waiting_for_stream_end() {
    let upstream = spawn_server(stub_upstream()).await;
    let (proxy_addr, _state) = spawn_proxy(upstream, ArmOverrides::empty()).await;

    let req = http::Request::builder()
        .uri(format!("http://{proxy_addr}/api/stream"))
        .body(Body::empty())
        .unwrap();
    let mut response = client().request(req).await.unwrap();
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
    assert_eq!(response.headers().get("x-accel-buffering").unwrap(), "no");

    // The upstream holds the stream open for 30s; a buffering proxy would
    // time out here instead of delivering the prompt-flush frame.
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match response.body_mut().frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        break data.clone();
                    }
                }
                other => panic!("stream ended before first data frame: {other:?}"),
            }
        }
    })
    .await
    .expect("first frame must arrive promptly through the proxy");
    assert_eq!(first.as_ref(), b": ready\n\n");
}

#[tokio::test]
async fn arm_override_steers_a_native_route_back_to_the_sidecar() {
    let upstream = spawn_server(stub_upstream()).await;

    async fn native_handler(_state: Arc<AppState>, _req: Request) -> Response {
        "from-native".into_response()
    }

    // Same native registration, two override states.
    let make_router = |state: Arc<AppState>| {
        Router::new()
            .route(
                "/api/native-candidate",
                arm_routed(
                    axum::routing::MethodFilter::GET,
                    "/api/native-candidate",
                    native_handler,
                ),
            )
            .fallback(axum::routing::any(
                |axum::extract::State(state): axum::extract::State<Arc<AppState>>,
                 req: Request| async move {
                    eo_http::proxy::forward(
                        &eo_http::proxy::build_client(),
                        state.upstream(),
                        req,
                    )
                    .await
                },
            ))
            .with_state(state)
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(AppState::new(
        upstream.to_string(),
        addr.port(),
        ArmOverrides::empty(),
    ));
    let router = make_router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let fetch = |addr: SocketAddr| async move {
        let req = http::Request::builder()
            .uri(format!("http://{addr}/api/native-candidate"))
            .body(Body::empty())
            .unwrap();
        body_bytes(client().request(req).await.unwrap()).await
    };

    // Default: the native arm serves.
    assert_eq!(fetch(addr).await, b"from-native");

    // Runtime kill-switch: steer the route back to the live sidecar with
    // no rebuild and no router reconstruction.
    state.set_overrides(ArmOverrides::parse_env_value("/api/native-candidate=proxy"));
    assert_eq!(fetch(addr).await, b"from-upstream");

    // And back.
    state.set_overrides(ArmOverrides::empty());
    assert_eq!(fetch(addr).await, b"from-native");
}

#[tokio::test]
async fn foreign_host_header_is_rejected_at_the_public_boundary() {
    let upstream = spawn_server(stub_upstream()).await;
    let (proxy_addr, _state) = spawn_proxy(upstream, ArmOverrides::empty()).await;

    // The backend's own Host guard sees only the rewritten private
    // authority once proxied, so the substrate enforces the public-facing
    // allowlist itself; a DNS-rebound hostname must die here.
    let req = http::Request::builder()
        .uri(format!("http://{proxy_addr}/api/echo"))
        .header("host", "rebound.example:8421")
        .body(Body::empty())
        .unwrap();
    let response = client().request(req).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
    let body: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("403 body is json");
    assert_eq!(body["detail"], "Invalid Host header");
}

#[tokio::test]
async fn dead_upstream_maps_to_502_with_json_detail() {
    // Reserve a port and release it so the dial fails.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);

    let (proxy_addr, _state) = spawn_proxy(dead_addr, ArmOverrides::empty()).await;
    let req = http::Request::builder()
        .uri(format!("http://{proxy_addr}/api/health"))
        .body(Body::empty())
        .unwrap();
    let response = client().request(req).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("502 body is json");
    assert!(body.get("detail").is_some());
}
