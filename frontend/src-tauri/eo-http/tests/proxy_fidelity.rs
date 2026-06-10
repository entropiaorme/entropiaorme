//! Proxy-fidelity self-test against the real Python backend.
//!
//! Boots the actual sidecar on a private loopback port (exactly the
//! relocated-upstream topology the shell runs), serves the substrate in
//! front of it, and diffs a PROXIED response against a DIRECT-to-sidecar
//! response for every checked route on the axes the HTTP-response goldens
//! project: status, content-type, cache-control, etag, plus full body
//! bytes. The event stream is checked for its prompt `: ready` flush and
//! header set through the proxy. Equal etags across arms double as the
//! cross-arm consistency guard for the caching surface.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test proxy_fidelity
//!
//! The interpreter is `$EO_ORACLE_PYTHON` if set, else the local
//! virtualenv (`.venv/Scripts/python.exe` on Windows, `.venv/bin/python`
//! elsewhere).
#![cfg(feature = "cross-language")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use eo_http::arms::ArmOverrides;
use eo_http::{build_router, AppState};
use http_body_util::BodyExt;
use tokio::net::TcpListener;

/// Repo root, three levels above this crate's manifest dir.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn oracle_python() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_ORACLE_PYTHON") {
        return PathBuf::from(explicit);
    }
    let root = repo_root();
    let windows = root.join(".venv/Scripts/python.exe");
    if windows.exists() {
        windows
    } else {
        root.join(".venv/bin/python")
    }
}

/// The backend process, killed on drop.
struct Sidecar {
    child: Child,
    port: u16,
    _data_dir: tempfile::TempDir,
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

fn spawn_sidecar() -> Sidecar {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
    let port = free_port();
    let child = Command::new(oracle_python())
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn backend sidecar");
    Sidecar {
        child,
        port,
        _data_dir: data_dir,
    }
}

fn client() -> eo_http::proxy::ProxyClient {
    eo_http::proxy::build_client()
}

async fn get(authority: &str, path: &str) -> http::Response<hyper::body::Incoming> {
    let req = http::Request::builder()
        .uri(format!("http://{authority}{path}"))
        .header("host", authority)
        .body(Body::empty())
        .unwrap();
    client().request(req).await.expect("request succeeds")
}

async fn wait_healthy(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let authority = format!("127.0.0.1:{port}");
    loop {
        if Instant::now() > deadline {
            panic!("backend never became healthy on {authority}");
        }
        let req = http::Request::builder()
            .uri(format!("http://{authority}/api/health"))
            .body(Body::empty())
            .unwrap();
        if let Ok(response) = client().request(req).await {
            if response.status() == http::StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn spawn_proxy(upstream: String) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(AppState::new(upstream, addr.port(), ArmOverrides::empty()));
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    addr
}

/// The fidelity-checked routes: the curated hydration set (parameterised
/// ids pinned to 1, both arms see the identical response either way),
/// health, and an unknown path for the 404 surface.
const CHECKED_ROUTES: [&str; 12] = [
    "/api/health",
    "/api/tracking/snapshot",
    "/api/tracking/sessions",
    "/api/tracking/session/1",
    "/api/tracking/session/1/quest-link-suggestion",
    "/api/quests",
    "/api/quests/mobs",
    "/api/quests/analytics",
    "/api/quests/playlists",
    "/api/scan/skills/status",
    "/api/codex/meta/attributes",
    "/api/no-such-route",
];

const PROJECTED_HEADERS: [&str; 3] = ["content-type", "cache-control", "etag"];

#[tokio::test]
async fn proxied_responses_match_direct_on_projected_axes() {
    let sidecar = spawn_sidecar();
    wait_healthy(sidecar.port).await;
    let direct_authority = format!("127.0.0.1:{}", sidecar.port);
    let proxy_addr = spawn_proxy(direct_authority.clone()).await;
    let proxy_authority = proxy_addr.to_string();

    for route in CHECKED_ROUTES {
        let direct = get(&direct_authority, route).await;
        let proxied = get(&proxy_authority, route).await;

        assert_eq!(
            direct.status(),
            proxied.status(),
            "status diverged on {route}"
        );
        for header in PROJECTED_HEADERS {
            assert_eq!(
                direct.headers().get(header),
                proxied.headers().get(header),
                "{header} diverged on {route}"
            );
        }
        let direct_body = direct.collect().await.unwrap().to_bytes();
        let proxied_body = proxied.collect().await.unwrap().to_bytes();
        assert_eq!(direct_body, proxied_body, "body diverged on {route}");
    }
}

#[tokio::test]
async fn event_stream_ready_frame_flushes_promptly_through_the_proxy() {
    let sidecar = spawn_sidecar();
    wait_healthy(sidecar.port).await;
    let proxy_addr = spawn_proxy(format!("127.0.0.1:{}", sidecar.port)).await;

    let mut response = get(&proxy_addr.to_string(), "/api/events").await;
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_string()),
        Some("text/event-stream".to_string())
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
    assert_eq!(response.headers().get("x-accel-buffering").unwrap(), "no");

    let first = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match response.body_mut().frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        break data.clone();
                    }
                }
                other => panic!("event stream ended before first frame: {other:?}"),
            }
        }
    })
    .await
    .expect("`: ready` must flush promptly through the proxy");
    assert!(
        first.as_ref().starts_with(b": ready"),
        "unexpected first frame: {:?}",
        String::from_utf8_lossy(&first)
    );
}
