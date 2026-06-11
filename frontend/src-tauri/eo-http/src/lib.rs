//! HTTP substrate for the EntropiaOrme backend.
//!
//! The strangler-fig seam: an axum application that owns the public
//! loopback address the frontend is wired to, serves natively-ported
//! routes in-process, and reverse-proxies everything else (including the
//! `/api/events` event stream) to the Python sidecar relocated onto a
//! private port. A route flip is "register a native handler"; a source
//! revert is "delete it"; and the runtime arm override ([`arms`]) steers
//! any flipped route back to the live sidecar in an already-shipped build.
//! The route-by-route takeover plan is documented in
//! `backend/architecture/PORT-READINESS.md`.

pub mod arms;
pub mod hydration;
pub mod proxy;
pub mod sse;

use std::future::Future;
use std::sync::{Arc, RwLock};

use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{any, on, MethodFilter, MethodRouter};
use axum::Router;

use crate::arms::{Arm, ArmOverrides};
use crate::proxy::ProxyClient;

/// Shared router state: the pooled upstream client, the sidecar authority,
/// the public-boundary Host allowlist, and the hot-swappable arm-override
/// map.
pub struct AppState {
    client: ProxyClient,
    upstream: String,
    allowed_hosts: [String; 2],
    overrides: RwLock<ArmOverrides>,
}

impl AppState {
    /// `upstream` is the sidecar's `host:port` authority; `public_port` is
    /// the loopback port this router itself serves, from which the inbound
    /// Host allowlist is derived (mirroring the backend's own guard, which
    /// now sees only the rewritten private authority).
    pub fn new(upstream: String, public_port: u16, overrides: ArmOverrides) -> Self {
        Self {
            client: proxy::build_client(),
            upstream,
            allowed_hosts: [
                format!("127.0.0.1:{public_port}"),
                format!("localhost:{public_port}"),
            ],
            overrides: RwLock::new(overrides),
        }
    }

    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// The arm currently serving `route`.
    pub fn arm_for(&self, route: &str) -> Arm {
        self.overrides
            .read()
            .expect("arm override lock never poisoned")
            .arm_for(route)
    }

    /// Replace the override map at runtime (a settings surface can swap
    /// it without restarting the router).
    pub fn set_overrides(&self, overrides: ArmOverrides) {
        *self
            .overrides
            .write()
            .expect("arm override lock never poisoned") = overrides;
    }

    async fn proxy(&self, req: Request) -> Response {
        proxy::forward(&self.client, &self.upstream, req).await
    }
}

async fn proxy_fallback(State(state): State<Arc<AppState>>, req: Request) -> Response {
    state.proxy(req).await
}

/// Public-boundary Host guard, mirroring the backend's own check (which
/// the proxy's Host rewrite would otherwise make a no-op for proxied
/// requests): a present Host header must name this router's loopback
/// authority. An absent Host passes, exactly as the backend's guard
/// treats it; the response shape matches the backend's 403 verbatim.
async fn host_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(host) = req.headers().get(http::header::HOST) {
        let allowed = host
            .to_str()
            .map(|host| state.allowed_hosts.iter().any(|entry| entry == host))
            .unwrap_or(false);
        if !allowed {
            let body = serde_json::json!({ "detail": "Invalid Host header" }).to_string();
            return Response::builder()
                .status(http::StatusCode::FORBIDDEN)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body))
                .expect("static 403 response builds");
        }
    }
    next.run(req).await
}

/// A method router that consults the arm override for `route` on every
/// request: `Native` runs the given handler, `Proxy` forwards to the
/// sidecar. Native route registrations go through this so the runtime
/// kill-switch covers them by construction.
pub fn arm_routed<F, Fut>(
    filter: MethodFilter,
    route: &'static str,
    native: F,
) -> MethodRouter<Arc<AppState>>
where
    F: Fn(Arc<AppState>, Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    on(
        filter,
        move |State(state): State<Arc<AppState>>, req: Request| {
            let native = native.clone();
            async move {
                match state.arm_for(route) {
                    Arm::Native => native(state, req).await,
                    Arm::Proxy => state.proxy(req).await,
                }
            }
        },
    )
}

/// The substrate router: natively-registered routes take precedence, the
/// proxy fallback carries every other method and path to the sidecar, and
/// the Host guard fronts both arms.
pub fn build_router(state: Arc<AppState>) -> Router {
    native_routes(Router::new())
        .fallback(any(proxy_fallback))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            host_guard,
        ))
        .with_state(state)
}

/// Native route registrations, in takeover order; each flip adds one
/// `arm_routed` line here, and deleting a line is the source-level
/// revert.
fn native_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route(
        "/api/health",
        arm_routed(MethodFilter::GET, "/api/health", routes::health),
    )
}

/// The natively-served handlers, one function per taken-over route.
mod routes {
    use std::sync::Arc;

    use axum::extract::Request;
    use axum::response::Response;

    use super::AppState;

    /// The health-check acknowledgement: byte-identical to the backend's
    /// response (the body is the serialised `HealthStatus` model, which
    /// the contract gate ties to the committed API document).
    pub(crate) async fn health(_state: Arc<AppState>, _req: Request) -> Response {
        let body = eo_wire::models::HealthStatus {
            status: "ok".into(),
            extra: serde_json::Map::new(),
        };
        Response::builder()
            .status(http::StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                serde_json::to_string(&body).expect("health body serialises"),
            ))
            .expect("static health response builds")
    }
}

/// Serve the substrate on an already-bound listener until the process
/// exits. The shell spawns this once at setup.
pub async fn serve(listener: tokio::net::TcpListener, state: Arc<AppState>) -> std::io::Result<()> {
    axum::serve(listener, build_router(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_arm_lookup_defaults_native_and_hot_swaps() {
        let state = AppState::new("127.0.0.1:1".into(), 8421, ArmOverrides::empty());
        assert_eq!(state.arm_for("/api/health"), Arm::Native);
        state.set_overrides(ArmOverrides::parse_env_value("/api/health=proxy"));
        assert_eq!(state.arm_for("/api/health"), Arm::Proxy);
    }
}
