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

pub mod analytics_routes;
pub mod arms;
pub mod body;
pub mod character_routes;
pub mod cors;
pub mod dev_routes;
pub mod equipment_routes;
pub mod extract;
pub mod hydration;
pub mod native;
pub mod producer_routes;
pub mod proxy;
pub mod pyjson;
pub mod scan_routes;
pub mod settings_routes;
pub mod sse;
pub mod tracking_routes;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{any, MethodFilter, MethodRouter};
use axum::Router;

use crate::arms::{Arm, ArmOverrides};
use crate::proxy::ProxyClient;

/// Shared router state: the pooled upstream client, the sidecar authority,
/// the public-boundary Host allowlist, the hot-swappable arm-override map,
/// and the composed native services. The native-service handles sit behind
/// `RwLock`s so composition can install them after `serve` has already
/// started: the substrate begins proxy-only and hot-upgrades to native the
/// moment composition succeeds (see [`AppState::install_native`]). That is
/// how a first launch after an upgrade recovers once the sidecar has
/// migrated the database up to the adoptable baseline, without a restart.
pub struct AppState {
    client: ProxyClient,
    upstream: String,
    allowed_hosts: [String; 2],
    overrides: RwLock<ArmOverrides>,
    hydration: RwLock<Option<Arc<crate::hydration::HydrationState>>>,
    tracker: RwLock<Option<Arc<eo_services::tracker::HuntTracker>>>,
    sse_hub: RwLock<Option<Arc<eo_wire::sse::SseHub>>>,
    config_service: RwLock<Option<Arc<Mutex<eo_services::config_service::ConfigService>>>>,
    skill_tracker: RwLock<Option<Arc<eo_services::skill_tracker::SkillTracker>>>,
    skill_scan: RwLock<Option<Arc<eo_services::skill_scan_manual::SkillScanManual>>>,
    repair_ocr: RwLock<Option<Arc<eo_services::repair_ocr::RepairOcrService>>>,
    spacebar_listener:
        RwLock<Option<Arc<eo_services::spacebar_capture_listener::SpacebarCaptureListener>>>,
    hotbar_listener: RwLock<Option<Arc<eo_services::hotbar_listener::HotbarListener>>>,
    cors: Option<cors::CorsConfig>,
    // The resolved data directory, for the hidden dev-tools routes (the
    // developer-mode gate reads it fresh, and the crash-reporting toggle
    // reads/writes the shell-owned `observability.json` there). `None` on a
    // substrate built without it: the dev routes then read as gate-off (404).
    data_dir: Option<PathBuf>,
}

/// The composed native services, handed to [`AppState::install_native`] to
/// flip the natively-registered routes off their proxy fallback. Every
/// handle is present (composition either yields the full set or declines),
/// so the bundle carries them by value rather than as options.
pub struct NativeServices {
    pub hydration: Arc<crate::hydration::HydrationState>,
    pub tracker: Arc<eo_services::tracker::HuntTracker>,
    pub sse_hub: Arc<eo_wire::sse::SseHub>,
    pub config_service: Arc<Mutex<eo_services::config_service::ConfigService>>,
    pub skill_tracker: Arc<eo_services::skill_tracker::SkillTracker>,
    pub skill_scan: Arc<eo_services::skill_scan_manual::SkillScanManual>,
    pub repair_ocr: Arc<eo_services::repair_ocr::RepairOcrService>,
    pub spacebar_listener: Arc<eo_services::spacebar_capture_listener::SpacebarCaptureListener>,
    pub hotbar_listener: Arc<eo_services::hotbar_listener::HotbarListener>,
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
            hydration: RwLock::new(None),
            tracker: RwLock::new(None),
            sse_hub: RwLock::new(None),
            config_service: RwLock::new(None),
            skill_tracker: RwLock::new(None),
            skill_scan: RwLock::new(None),
            repair_ocr: RwLock::new(None),
            spacebar_listener: RwLock::new(None),
            hotbar_listener: RwLock::new(None),
            cors: None,
            data_dir: None,
        }
    }

    /// Attach the backend-mirroring CORS contract: preflights answered
    /// at the substrate, native responses decorated for allowed
    /// origins, and the origin guard enforced ahead of routing. Without
    /// it (substrates predating composition, and tests that do not
    /// exercise the browser surface) preflights and origin rules flow
    /// to the sidecar as before.
    pub fn with_cors(mut self, cors: cors::CorsConfig) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Attach the resolved data directory, enabling the hidden dev-tools
    /// routes (the developer-mode gate and the crash-reporting toggle). Without
    /// it those routes read as gate-off (404).
    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.data_dir = Some(data_dir);
        self
    }

    /// The resolved data directory, when set.
    pub(crate) fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// Whether developer mode is currently enabled, read FRESH from the
    /// settings file on each call (never cached), so the hidden dev-tools gate
    /// reflects a toggle without a restart. The gate for every dev route: when
    /// this is false (the default, and the case with no data dir), those routes
    /// answer 404, keeping them off the equivalence-covered surface and
    /// invisible to a default install.
    pub(crate) fn developer_mode(&self) -> bool {
        self.data_dir
            .as_deref()
            .and_then(|dir| eo_services::config_service::load_config_readonly(dir).ok())
            .map(|config| config.developer_mode_enabled)
            .unwrap_or(false)
    }

    /// Attach the composed native services. Without this (a substrate
    /// built before composition, or composition declined at startup)
    /// every natively-registered route falls back to the proxy arm.
    pub fn with_hydration(mut self, hydration: Arc<crate::hydration::HydrationState>) -> Self {
        self.hydration = RwLock::new(Some(hydration));
        self
    }

    /// The composed native services, when present.
    pub(crate) fn hydration(&self) -> Option<Arc<crate::hydration::HydrationState>> {
        self.hydration
            .read()
            .expect("hydration service lock")
            .clone()
    }

    /// Attach the live producer-spine tracker (the same `Arc<HuntTracker>`
    /// held by the Tauri-managed producer state). Without it (a substrate
    /// built before composition, or composition declined at startup) the
    /// producer routes that need the live tracker fall back to the proxy
    /// arm, exactly like the read surface without [`with_hydration`].
    pub fn with_tracker(mut self, tracker: Arc<eo_services::tracker::HuntTracker>) -> Self {
        self.tracker = RwLock::new(Some(tracker));
        self
    }

    /// The live producer-spine tracker, when composed.
    pub(crate) fn tracker(&self) -> Option<Arc<eo_services::tracker::HuntTracker>> {
        self.tracker.read().expect("tracker service lock").clone()
    }

    /// Attach the live producer-spine SSE hub (the same `Arc<SseHub>` the
    /// producer-bus bridge dispatches onto). Without it (a substrate built
    /// before composition, or composition declined at startup) the
    /// `/api/events` stream falls back to the proxy arm, exactly like the
    /// read surface without [`with_hydration`].
    pub fn with_sse_hub(mut self, sse_hub: Arc<eo_wire::sse::SseHub>) -> Self {
        self.sse_hub = RwLock::new(Some(sse_hub));
        self
    }

    /// The live producer-spine SSE hub, when composed.
    pub(crate) fn sse_hub(&self) -> Option<Arc<eo_wire::sse::SseHub>> {
        self.sse_hub.read().expect("sse hub service lock").clone()
    }

    /// Attach the settings writer (the same `Arc<Mutex<ConfigService>>` the
    /// producer spine holds). Without it (a substrate built before
    /// composition, or composition declined at startup) the settings-write
    /// routes fall back to the proxy arm, exactly like the read surface
    /// without [`with_hydration`].
    pub fn with_config_service(
        mut self,
        config_service: Arc<Mutex<eo_services::config_service::ConfigService>>,
    ) -> Self {
        self.config_service = RwLock::new(Some(config_service));
        self
    }

    /// The settings writer, when composed.
    pub(crate) fn config_service(
        &self,
    ) -> Option<Arc<Mutex<eo_services::config_service::ConfigService>>> {
        self.config_service
            .read()
            .expect("config service lock")
            .clone()
    }

    /// Attach the live producer-spine skill tracker (the same
    /// `Arc<SkillTracker>` held by the producer spine). Without it the codex
    /// claim routes that arm `suppress_next` fall back to the proxy arm.
    pub fn with_skill_tracker(
        mut self,
        skill_tracker: Arc<eo_services::skill_tracker::SkillTracker>,
    ) -> Self {
        self.skill_tracker = RwLock::new(Some(skill_tracker));
        self
    }

    /// The live producer-spine skill tracker, when composed.
    pub(crate) fn skill_tracker(&self) -> Option<Arc<eo_services::skill_tracker::SkillTracker>> {
        self.skill_tracker
            .read()
            .expect("skill tracker service lock")
            .clone()
    }

    /// Attach the composed manual skill-scan service (the OCR scan
    /// state machine). Without it (a substrate built before composition,
    /// composition declined, or the OCR runtime absent off Windows) the
    /// scan routes fall back to the proxy arm.
    pub fn with_skill_scan(
        mut self,
        skill_scan: Arc<eo_services::skill_scan_manual::SkillScanManual>,
    ) -> Self {
        self.skill_scan = RwLock::new(Some(skill_scan));
        self
    }

    /// The composed manual skill-scan service, when present.
    pub(crate) fn skill_scan(
        &self,
    ) -> Option<Arc<eo_services::skill_scan_manual::SkillScanManual>> {
        self.skill_scan
            .read()
            .expect("skill scan service lock")
            .clone()
    }

    /// Attach the composed repair-OCR service. Without it the repair-scan
    /// route falls back to the proxy arm.
    pub fn with_repair_ocr(
        mut self,
        repair_ocr: Arc<eo_services::repair_ocr::RepairOcrService>,
    ) -> Self {
        self.repair_ocr = RwLock::new(Some(repair_ocr));
        self
    }

    /// The composed repair-OCR service, when present.
    pub(crate) fn repair_ocr(&self) -> Option<Arc<eo_services::repair_ocr::RepairOcrService>> {
        self.repair_ocr
            .read()
            .expect("repair ocr service lock")
            .clone()
    }

    /// Attach the composed spacebar-capture listener. Without it the
    /// spacebar-capture toggle route falls back to the proxy arm.
    pub fn with_spacebar_listener(
        mut self,
        spacebar_listener: Arc<eo_services::spacebar_capture_listener::SpacebarCaptureListener>,
    ) -> Self {
        self.spacebar_listener = RwLock::new(Some(spacebar_listener));
        self
    }

    /// The composed spacebar-capture listener, when present.
    pub(crate) fn spacebar_listener(
        &self,
    ) -> Option<Arc<eo_services::spacebar_capture_listener::SpacebarCaptureListener>> {
        self.spacebar_listener
            .read()
            .expect("spacebar listener service lock")
            .clone()
    }

    /// Attach the composed hotbar listener (the same `Arc<HotbarListener>` the
    /// producer spine holds). Without it the snapshot route falls back to the
    /// proxy arm (it reads the listener's running state).
    pub fn with_hotbar_listener(
        mut self,
        hotbar_listener: Arc<eo_services::hotbar_listener::HotbarListener>,
    ) -> Self {
        self.hotbar_listener = RwLock::new(Some(hotbar_listener));
        self
    }

    /// The composed hotbar listener, when present.
    pub(crate) fn hotbar_listener(
        &self,
    ) -> Option<Arc<eo_services::hotbar_listener::HotbarListener>> {
        self.hotbar_listener
            .read()
            .expect("hotbar listener service lock")
            .clone()
    }

    /// Install the composed native services after the substrate is already
    /// serving. Composition runs in a background task off the startup path
    /// (so the substrate answers proxy-only the instant it binds) and calls
    /// this the moment it succeeds: each handle is written under its lock,
    /// and the next request for a natively-registered route reads the
    /// now-present service and runs its native arm instead of the proxy
    /// fallback. A first launch that found the database below the adoptable
    /// baseline (the sidecar had not yet migrated it) recovers here, without
    /// a restart, the moment a retry of composition adopts the migrated
    /// database.
    pub fn install_native(&self, services: NativeServices) {
        *self.hydration.write().expect("hydration service lock") = Some(services.hydration);
        *self.tracker.write().expect("tracker service lock") = Some(services.tracker);
        *self.sse_hub.write().expect("sse hub service lock") = Some(services.sse_hub);
        *self.config_service.write().expect("config service lock") = Some(services.config_service);
        *self
            .skill_tracker
            .write()
            .expect("skill tracker service lock") = Some(services.skill_tracker);
        *self.skill_scan.write().expect("skill scan service lock") = Some(services.skill_scan);
        *self.repair_ocr.write().expect("repair ocr service lock") = Some(services.repair_ocr);
        *self
            .spacebar_listener
            .write()
            .expect("spacebar listener service lock") = Some(services.spacebar_listener);
        *self
            .hotbar_listener
            .write()
            .expect("hotbar listener service lock") = Some(services.hotbar_listener);
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

    pub(crate) async fn proxy(&self, req: Request) -> Response {
        proxy::forward(&self.client, &self.upstream, req).await
    }
}

async fn proxy_fallback(State(state): State<Arc<AppState>>, req: Request) -> Response {
    state.proxy(req).await
}

/// A 403 in the backend's `{"detail": ...}` rendering.
fn forbidden(detail: &str) -> Response {
    let body = serde_json::json!({ "detail": detail }).to_string();
    Response::builder()
        .status(http::StatusCode::FORBIDDEN)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .expect("static 403 response builds")
}

/// The public-boundary guard, mirroring the backend's own API-origin
/// middleware clause for clause: OPTIONS and non-API paths pass
/// untouched; a present Host header must name this router's loopback
/// authority (the proxy's Host rewrite makes the sidecar's own check a
/// no-op for proxied requests); and, when the CORS contract is
/// configured, mutating methods require an allowed Origin while reads
/// reject a present-but-disallowed one. Each 403 body matches the
/// backend's verbatim.
async fn api_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    if req.method() == http::Method::OPTIONS || !req.uri().path().starts_with("/api/") {
        return next.run(req).await;
    }
    if let Some(host) = req.headers().get(http::header::HOST) {
        // The backend lowercases the inbound Host before its check and
        // skips an empty value (its falsy test), as well as an absent
        // header.
        let allowed = host
            .to_str()
            .map(|host| {
                let host = host.to_ascii_lowercase();
                host.is_empty() || state.allowed_hosts.contains(&host)
            })
            .unwrap_or(false);
        if !allowed {
            return forbidden("Invalid Host header");
        }
    }
    if let Some(cors) = &state.cors {
        let origin = req
            .headers()
            .get(http::header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        let mutating = !matches!(
            *req.method(),
            http::Method::GET | http::Method::HEAD | http::Method::OPTIONS
        );
        if mutating {
            if !origin.is_some_and(|o| cors.origin_allowed(o)) {
                return forbidden("Origin header required");
            }
        } else if origin.is_some_and(|o| !cors.origin_allowed(o)) {
            return forbidden("Invalid Origin header");
        }
    }
    next.run(req).await
}

/// The outermost CORS layer, where the backend's stack also puts it: a
/// preflight short-circuits everything (routing, the Host and origin
/// guards) when the contract is configured, and every other response
/// for an allowed Origin is decorated unless the sidecar already did.
async fn cors_layer(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(cors_config) = &state.cors else {
        return next.run(req).await;
    };
    if cors::is_preflight(req.method(), req.headers()) {
        return cors_config.preflight_response(req.headers());
    }
    let origin = req.headers().get(http::header::ORIGIN).cloned();
    let mut response = next.run(req).await;
    if let Some(origin) = origin {
        let allowed = origin
            .to_str()
            .map(|o| cors_config.origin_allowed(o))
            .unwrap_or(false);
        if allowed {
            cors::decorate(&mut response, &origin);
        }
    }
    response
}

/// A per-path registration whose every native method consults the arm
/// override for `route` at request time (`Native` runs the handler,
/// `Proxy` forwards to the sidecar), so the runtime kill-switch covers
/// each registration by construction. Methods the registration does
/// not carry still belong to the sidecar (an unported method on a
/// natively-served path, the bare OPTIONS the backend answers itself):
/// they fall back to the proxy rather than axum's empty 405. HEAD has
/// its own explicit proxy leg because axum otherwise dispatches it
/// into a GET handler with the body stripped, while the backend
/// hard-405s HEAD on its GET routes.
pub struct ArmRoutes {
    route: &'static str,
    method_router: MethodRouter<Arc<AppState>>,
}

impl ArmRoutes {
    pub fn at(route: &'static str) -> Self {
        Self {
            route,
            method_router: MethodRouter::new()
                .on(
                    MethodFilter::HEAD,
                    |State(state): State<Arc<AppState>>, req: Request| async move {
                        state.proxy(req).await
                    },
                )
                .fallback(proxy_fallback),
        }
    }

    pub fn on<F, Fut>(mut self, filter: MethodFilter, native: F) -> Self
    where
        F: Fn(Arc<AppState>, Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let route = self.route;
        self.method_router = self.method_router.on(
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
        );
        self
    }

    pub fn into_method_router(self) -> MethodRouter<Arc<AppState>> {
        self.method_router
    }
}

/// The single-method convenience over [`ArmRoutes`].
pub fn arm_routed<F, Fut>(
    filter: MethodFilter,
    route: &'static str,
    native: F,
) -> MethodRouter<Arc<AppState>>
where
    F: Fn(Arc<AppState>, Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    ArmRoutes::at(route).on(filter, native).into_method_router()
}

/// Observe-only per-request instrumentation. As the OUTERMOST layer it times
/// the whole request (guards, CORS, and the handler) and records one sample
/// into the metrics registry, emitting a structured trace with method, path,
/// and status. It is pure pass-through: it reads the request line and the
/// response status but never mutates the request or the response (no body
/// rewrite, no added header), so it is behaviour-neutral against the proxy
/// and native goldens. The path is logged, never the query string, so no
/// caller-supplied value reaches the logs.
async fn observe(req: Request, next: axum::middleware::Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = std::time::Instant::now();
    let response = next.run(req).await;
    let elapsed = started.elapsed();
    // Exclude the hidden dev-tools routes from the throughput/latency metric:
    // the metrics page's own polling must not inflate the figures it displays.
    if !path.starts_with("/api/dev/") {
        eo_wire::metrics::metrics().record_http_request(elapsed);
    }
    tracing::debug!(
        target: "eo::http",
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        elapsed_us = elapsed.as_micros() as u64,
        "request served"
    );
    response
}

/// The substrate router: natively-registered routes take precedence, the
/// proxy fallback carries every other method and path to the sidecar, and
/// the guard stack fronts both arms in the backend's own order (the
/// observe layer outermost, then CORS, then the Host/origin guard).
pub fn build_router(state: Arc<AppState>) -> Router {
    native_routes(Router::new())
        .fallback(any(proxy_fallback))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            api_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cors_layer,
        ))
        // Outermost: times the full request including the guard/CORS layers.
        .layer(axum::middleware::from_fn(observe))
        .with_state(state)
}

/// The plain, transport-agnostic shape of an in-process dispatch result, so
/// the Tauri command layer can return it without depending on axum/http types.
pub struct InProcessResponse {
    pub status: u16,
    /// The status line's canonical reason phrase, so the frontend's `Response`
    /// keeps the `statusText` a loopback `fetch` would have carried (the error
    /// contract falls back to it on an empty body).
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Build the in-process request from the IPC descriptor (method, path-and-query,
/// headers, body). Extracted from [`dispatch_in_process`] so the transport's
/// request construction is unit-testable without composing the router. An
/// invalid method or URI surfaces as an `Err` string (the command reports it)
/// rather than a panic.
fn build_in_process_request(
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<http::Request<axum::body::Body>, String> {
    let mut builder = http::Request::builder().method(method).uri(path_and_query);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
        .body(axum::body::Body::from(body))
        .map_err(|err| format!("malformed in-process request: {err}"))
}

/// Dispatch a request through the in-process router WITHOUT binding a socket:
/// the server side of the Tauri-IPC transport that replaces the loopback HTTP
/// hop. The request runs through the identical stack a client over the socket
/// would hit (the native arms, the proxy fallback, the Host/origin guard,
/// CORS, and the observe/metrics layer), so behaviour and instrumentation are
/// unchanged; only the transport in front of the router differs. A
/// same-process request carries no Origin/Host, which the guard admits
/// (v0.1.0 parity) and CORS leaves undecorated, exactly as intended.
pub async fn dispatch_in_process(
    state: Arc<AppState>,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<InProcessResponse, String> {
    use tower::ServiceExt as _;

    let request = build_in_process_request(method, path_and_query, headers, body)?;

    // `Router::oneshot` is infallible (its `Error` is `Infallible`): the
    // dispatch itself never errors, so only request construction above can.
    let response = match build_router(state).oneshot(request).await {
        Ok(response) => response,
        Err(infallible) => match infallible {},
    };

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|err| format!("in-process response body read failed: {err}"))?
        .to_vec();

    Ok(InProcessResponse {
        status,
        status_text,
        headers,
        body,
    })
}

/// Native route registrations, in takeover order; each flip adds one
/// `arm_routed` line (here or in [`native`]), and deleting a line is
/// the source-level revert.
fn native_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let router = native::register(router.route(
        "/api/health",
        arm_routed(MethodFilter::GET, "/api/health", routes::health),
    ));
    // The hidden dev-tools routes: native-only (no Python arm, no golden),
    // each self-gated on developer mode so they 404 off by default.
    dev_routes::register(router)
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
    fn state_reports_its_upstream_authority_verbatim() {
        let state = AppState::new("127.0.0.1:9421".into(), 8421, ArmOverrides::empty());
        assert_eq!(state.upstream(), "127.0.0.1:9421");
    }

    #[test]
    fn state_arm_lookup_defaults_native_and_hot_swaps() {
        let state = AppState::new("127.0.0.1:1".into(), 8421, ArmOverrides::empty());
        assert_eq!(state.arm_for("/api/health"), Arm::Native);
        state.set_overrides(ArmOverrides::parse_env_value("/api/health=proxy"));
        assert_eq!(state.arm_for("/api/health"), Arm::Proxy);
    }

    /// Driving a request through the built router records one HTTP-request
    /// sample (the observe layer records one sample per request), and the
    /// response is unchanged: a 200 from the native health handler. The native
    /// `/api/health` arm answers without a sidecar, so the timing layer is
    /// exercised end to end here.
    #[tokio::test]
    async fn a_served_request_records_one_http_sample_without_altering_the_response() {
        use axum::body::Body;
        use tower::ServiceExt;

        let state = Arc::new(AppState::new(
            "127.0.0.1:1".into(),
            8421,
            ArmOverrides::empty(),
        ));
        let router = build_router(state);

        let before = eo_wire::metrics::metrics().snapshot().http_requests;
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), http::StatusCode::OK);
        let after = eo_wire::metrics::metrics().snapshot().http_requests;
        assert!(
            after > before,
            "the observe layer must record the served request (before={before}, after={after})"
        );
    }

    /// The in-process IPC dispatch (the loopback-socket replacement that the
    /// frontend's `tauriFetch` calls) drives a request through the SAME router
    /// an HTTP client would hit and returns the transport-agnostic response
    /// shape. Proven end to end against the native `/api/health` arm (no
    /// sidecar, no composition required): a 200 with the health JSON, the
    /// handler's Content-Type carried back, the body collected. A same-process
    /// request carries no Origin/Host, which the guard admits and CORS leaves
    /// undecorated, so the dispatch path is exercised exactly as the socket
    /// path would be.
    #[tokio::test]
    async fn dispatch_in_process_routes_a_request_through_the_in_process_router() {
        let state = Arc::new(AppState::new(
            "127.0.0.1:1".into(),
            8421,
            ArmOverrides::empty(),
        ));
        let response = dispatch_in_process(state, "GET", "/api/health", &[], Vec::new())
            .await
            .expect("the in-process dispatch succeeds");
        assert_eq!(response.status, 200);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, value)| name == "content-type" && value.contains("application/json")),
            "the native handler's Content-Type is carried back: {:?}",
            response.headers
        );
        let body: serde_json::Value =
            serde_json::from_slice(&response.body).expect("the health body is JSON");
        assert_eq!(body["status"], "ok");
    }

    /// The transport's request construction (the IPC-descriptor -> http::Request
    /// half of the in-process dispatch) carries the method, path, query string,
    /// headers, and body verbatim, so a re-pointed call site reaches the router
    /// exactly as a loopback request would.
    #[tokio::test]
    async fn the_in_process_request_carries_method_path_query_headers_and_body() {
        let request = build_in_process_request(
            "POST",
            "/api/settings?dry_run=1&scope=all",
            &[("content-type".to_string(), "application/json".to_string())],
            br#"{"player_name":"Mikel"}"#.to_vec(),
        )
        .expect("the request builds");
        assert_eq!(request.method(), http::Method::POST);
        assert_eq!(request.uri().path(), "/api/settings");
        assert_eq!(request.uri().query(), Some("dry_run=1&scope=all"));
        assert_eq!(
            request
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
        );
        let body = axum::body::to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("the body collects");
        assert_eq!(&body[..], br#"{"player_name":"Mikel"}"#);
    }

    #[test]
    fn the_in_process_request_parses_each_standard_method() {
        for method in ["GET", "POST", "PATCH", "DELETE", "PUT"] {
            let request = build_in_process_request(method, "/api/x", &[], Vec::new())
                .expect("a standard method builds");
            assert_eq!(request.method().as_str(), method);
        }
    }

    #[test]
    fn a_malformed_in_process_request_is_a_clean_error_not_a_panic() {
        // A method token with a space is not a valid HTTP method; the builder
        // surfaces it as an Err the command reports, never a panic.
        assert!(build_in_process_request("BAD METHOD", "/api/x", &[], Vec::new()).is_err());
    }
}
