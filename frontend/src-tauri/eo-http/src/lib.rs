//! HTTP substrate for the EntropiaOrme backend.
//!
//! The single-binary in-process router: an axum application the frontend
//! reaches through the `api_request` Tauri command (no socket), serving
//! every backend route natively in-process. The Python sidecar and the
//! reverse proxy were retired when the backend collapsed into the shell
//! process, so there is no upstream and no per-route arm: a route is simply
//! a registered native handler. The route map is documented in
//! `backend/architecture/PORT-READINESS.md`.

pub mod analytics_routes;
pub mod body;
pub mod character_routes;
pub mod cors;
pub mod demo;
pub mod dev_routes;
pub mod equipment_routes;
pub mod extract;
pub mod hydration;
pub mod native;
pub mod producer_routes;
pub mod pyjson;
pub mod scan_routes;
pub mod settings_routes;
pub mod tracking_routes;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{MethodFilter, MethodRouter};
use axum::Router;

/// Shared router state: the public-boundary Host allowlist and the composed
/// native services. The native-service handles sit behind `RwLock`s so the
/// composition root can install them once the database has opened and the
/// producer spine has stood up; the shell publishes this state to the
/// `api_request` IPC command only after [`AppState::install_native`] has run,
/// so by the time any request dispatches, every handle is present (see the
/// shell's `compose_substrate`).
pub struct AppState {
    allowed_hosts: [String; 2],
    hydration: RwLock<Option<Arc<crate::hydration::HydrationState>>>,
    tracker: RwLock<Option<Arc<eo_services::tracker::HuntTracker>>>,
    chatlog_watcher: RwLock<Option<Arc<eo_services::chatlog_watcher::ChatlogWatcher>>>,
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
    // The bundled demo database path, for the guide-mode `/api/demo` surface.
    // `None` on a substrate built without it: the demo routes answer the 503
    // service-unavailable floor.
    demo_db: Option<PathBuf>,
    // The lazily-built demo services (a parallel hydration + tracker over a
    // writable clone of the demo DB), stood up on first demo access. The inner
    // `None` records a build that could not be served, so the routes degrade
    // gracefully without retrying a hopeless build on every request.
    demo: tokio::sync::OnceCell<Option<Arc<crate::demo::DemoState>>>,
}

/// The composed native services, handed to [`AppState::install_native`] to
/// bring the natively-registered routes up off the service-unavailable floor.
/// Every handle is present (composition either yields the full set or
/// declines), so the bundle carries them by value rather than as options.
pub struct NativeServices {
    pub hydration: Arc<crate::hydration::HydrationState>,
    pub tracker: Arc<eo_services::tracker::HuntTracker>,
    pub chatlog_watcher: Arc<eo_services::chatlog_watcher::ChatlogWatcher>,
    pub config_service: Arc<Mutex<eo_services::config_service::ConfigService>>,
    pub skill_tracker: Arc<eo_services::skill_tracker::SkillTracker>,
    pub skill_scan: Arc<eo_services::skill_scan_manual::SkillScanManual>,
    pub repair_ocr: Arc<eo_services::repair_ocr::RepairOcrService>,
    pub spacebar_listener: Arc<eo_services::spacebar_capture_listener::SpacebarCaptureListener>,
    pub hotbar_listener: Arc<eo_services::hotbar_listener::HotbarListener>,
}

impl AppState {
    /// `public_port` is the nominal loopback authority the inbound Host
    /// allowlist is derived from, mirroring the backend's own origin guard.
    /// An in-process IPC request carries no Host header (the guard admits
    /// that, v0.1.0 parity), so the allowlist only bites a request that
    /// presents an explicit Host, which the IPC transport never does.
    pub fn new(public_port: u16) -> Self {
        Self {
            allowed_hosts: [
                format!("127.0.0.1:{public_port}"),
                format!("localhost:{public_port}"),
            ],
            hydration: RwLock::new(None),
            tracker: RwLock::new(None),
            chatlog_watcher: RwLock::new(None),
            config_service: RwLock::new(None),
            skill_tracker: RwLock::new(None),
            skill_scan: RwLock::new(None),
            repair_ocr: RwLock::new(None),
            spacebar_listener: RwLock::new(None),
            hotbar_listener: RwLock::new(None),
            cors: None,
            data_dir: None,
            demo_db: None,
            demo: tokio::sync::OnceCell::new(),
        }
    }

    /// Attach the backend-mirroring CORS contract: preflights answered
    /// at the substrate, native responses decorated for allowed
    /// origins, and the origin guard enforced ahead of routing. Without
    /// it (substrates predating composition, and tests that do not
    /// exercise the browser surface) no CORS contract is applied: the
    /// browser surface is left undecorated and the origin guard inert.
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

    /// Attach the bundled demo database path, enabling the guide-mode
    /// `/api/demo` surface. Without it those routes answer the 503
    /// service-unavailable floor.
    pub fn with_demo_db_path(mut self, demo_db: PathBuf) -> Self {
        self.demo_db = Some(demo_db);
        self
    }

    /// The bundled demo database path, when set.
    pub(crate) fn demo_db_path(&self) -> Option<PathBuf> {
        self.demo_db.clone()
    }

    /// The lazily-built demo-services cell (built once on first demo access).
    pub(crate) fn demo_cell(&self) -> &tokio::sync::OnceCell<Option<Arc<crate::demo::DemoState>>> {
        &self.demo
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
    /// every natively-registered route answers the 503 service-unavailable
    /// floor.
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
    /// producer routes that need the live tracker answer the 503
    /// service-unavailable floor, exactly like the read surface without
    /// `with_hydration`.
    pub fn with_tracker(mut self, tracker: Arc<eo_services::tracker::HuntTracker>) -> Self {
        self.tracker = RwLock::new(Some(tracker));
        self
    }

    /// The live producer-spine tracker, when composed.
    pub(crate) fn tracker(&self) -> Option<Arc<eo_services::tracker::HuntTracker>> {
        self.tracker.read().expect("tracker service lock").clone()
    }

    /// Attach the live producer-spine chat-log watcher (the same
    /// `Arc<ChatlogWatcher>` the producer spine holds). The settings-write
    /// route restarts it when the watched `chatlog_path` changes (the watcher
    /// captured its path at composition and does not re-read config live).
    pub fn with_chatlog_watcher(
        mut self,
        chatlog_watcher: Arc<eo_services::chatlog_watcher::ChatlogWatcher>,
    ) -> Self {
        self.chatlog_watcher = RwLock::new(Some(chatlog_watcher));
        self
    }

    /// The live producer-spine chat-log watcher, when composed.
    pub(crate) fn chatlog_watcher(
        &self,
    ) -> Option<Arc<eo_services::chatlog_watcher::ChatlogWatcher>> {
        self.chatlog_watcher
            .read()
            .expect("chatlog watcher service lock")
            .clone()
    }

    /// Attach the settings writer (the same `Arc<Mutex<ConfigService>>` the
    /// producer spine holds). Without it (a substrate built before
    /// composition, or composition declined at startup) the settings-write
    /// routes answer the 503 service-unavailable floor, exactly like the
    /// read surface without `with_hydration`.
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
    /// claim routes that arm `suppress_next` answer the 503 service-unavailable floor.
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
    /// scan routes answer the 503 service-unavailable floor.
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
    /// route answers the 503 service-unavailable floor.
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
    /// spacebar-capture toggle route answers the 503 service-unavailable floor.
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
    /// producer spine holds). Without it the snapshot route answers the 503
    /// service-unavailable floor (it reads the listener's running state).
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

    /// Install the composed native services into the shared state. The
    /// composition root calls this once, after the database has opened and
    /// the producer spine has stood up, and the shell publishes the state to
    /// the `api_request` IPC command only afterwards, so every handle is
    /// present before the first request dispatches. Each handle is written
    /// under its lock so the read accessors see the composed service.
    pub fn install_native(&self, services: NativeServices) {
        *self.hydration.write().expect("hydration service lock") = Some(services.hydration);
        *self.tracker.write().expect("tracker service lock") = Some(services.tracker);
        *self
            .chatlog_watcher
            .write()
            .expect("chatlog watcher service lock") = Some(services.chatlog_watcher);
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
}

/// A 503 in the backend's `{"detail": ...}` rendering, returned by a native
/// handler whose composed service is not yet present. The shell publishes the
/// router only after composition installs every service, so this is a
/// defensive floor (never reached on the normal startup path) rather than the
/// retired proxy fallback: a request that somehow arrives mid-composition gets
/// a clean, retryable 503 instead of a panic.
pub(crate) fn service_unavailable() -> Response {
    let body = serde_json::json!({ "detail": "backend services are initialising" }).to_string();
    Response::builder()
        .status(http::StatusCode::SERVICE_UNAVAILABLE)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .expect("static 503 response builds")
}

/// The framework 404 the router serves for an unmatched path, in the
/// backend's `{"detail": "Not Found"}` rendering (the fallback that the
/// reverse proxy used to occupy, now that nothing is forwarded upstream).
async fn not_found() -> Response {
    let body = serde_json::json!({ "detail": "Not Found" }).to_string();
    Response::builder()
        .status(http::StatusCode::NOT_FOUND)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .expect("static 404 response builds")
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
/// authority; and, when the CORS contract is
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
/// for an allowed Origin is decorated unless it already carries the header.
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

/// A 405 in the backend's `{"detail": ...}` rendering, returned for a method
/// no native registration carries on a served path (the proxy used to forward
/// these to the sidecar, which hard-405s an unported method).
fn method_not_allowed() -> Response {
    let body = serde_json::json!({ "detail": "Method Not Allowed" }).to_string();
    Response::builder()
        .status(http::StatusCode::METHOD_NOT_ALLOWED)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .expect("static 405 response builds")
}

/// A multi-method native registration. Each method runs its in-process
/// handler; a method the registration does not carry gets the backend's
/// `{"detail": "Method Not Allowed"}` 405 (set as the method fallback rather
/// than axum's empty 405), and HEAD is bound to the same 405 explicitly so
/// axum does not silently dispatch it into the GET handler with the body
/// stripped (the backend hard-405s HEAD on its GET routes). A path no
/// registration matches falls through to the router's framework 404.
///
/// The constructor still takes the route literal: the collapse into the
/// single-binary shell retired the per-route proxy/native arm override this
/// used to consult, so
/// the path is no longer read here, but it is kept as the inline record of
/// the registration's path (matching the adjacent `.route(...)` key) and the
/// `arm_routed`/`at` names are retained so the route map in [`native`] reads
/// unchanged. The leading underscore marks it deliberately unread.
pub struct ArmRoutes {
    method_router: MethodRouter<Arc<AppState>>,
}

impl ArmRoutes {
    pub fn at(_route: &'static str) -> Self {
        Self {
            method_router: MethodRouter::new()
                .on(MethodFilter::HEAD, || async { method_not_allowed() })
                .fallback(|| async { method_not_allowed() }),
        }
    }

    pub fn on<F, Fut>(mut self, filter: MethodFilter, native: F) -> Self
    where
        F: Fn(Arc<AppState>, Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.method_router = self.method_router.on(
            filter,
            move |State(state): State<Arc<AppState>>, req: Request| {
                let native = native.clone();
                async move { native(state, req).await }
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
/// rewrite, no added header), so it is behaviour-neutral against the native
/// goldens. The path is logged, never the query string, so no
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

/// The in-process router: every backend route is a registered native
/// handler, an unmatched path gets the backend's framework 404, and the
/// guard stack fronts them in the backend's own order (the observe layer
/// outermost, then CORS, then the Host/origin guard).
pub fn build_router(state: Arc<AppState>) -> Router {
    native_routes(Router::new())
        .fallback(not_found)
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

/// The webview's own origin, supplied to a same-process request that arrives
/// without one so the origin guard admits it. Always present in the CORS
/// allowlist (`cors::CorsConfig::new`), independent of the frontend port.
const IN_PROCESS_ORIGIN: &str = "tauri://localhost";

/// Dispatch a request through the in-process router WITHOUT binding a socket:
/// the server side of the Tauri-IPC transport that replaces the loopback HTTP
/// hop. The request runs through the identical stack a client over the socket
/// would hit (the native arms, the Host/origin guard, CORS, and the
/// observe/metrics layer), so behaviour and instrumentation are unchanged;
/// only the transport in front of the router differs.
///
/// A same-process request carries no Host, which the guard admits (an empty
/// Host is the loopback caller). It also carries no Origin: `invoke` is not a
/// network fetch, so unlike the loopback `fetch` it replaces, the browser does
/// not attach one. The origin guard requires an allow-listed Origin on a
/// mutating request, so the transport supplies the webview's own origin here;
/// without it every POST/PATCH/DELETE would be refused with "Origin header
/// required" before routing. A caller-supplied Origin is left untouched, so
/// the guard still refuses an explicitly disallowed one.
pub async fn dispatch_in_process(
    state: Arc<AppState>,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<InProcessResponse, String> {
    use tower::ServiceExt as _;

    // Supply the webview's allow-listed Origin when the caller sent none, so
    // the origin guard admits mutating IPC calls (see the doc comment above).
    let request = if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("origin"))
    {
        build_in_process_request(method, path_and_query, headers, body)?
    } else {
        let mut headers = headers.to_vec();
        headers.push(("origin".to_string(), IN_PROCESS_ORIGIN.to_string()));
        build_in_process_request(method, path_and_query, &headers, body)?
    };

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

/// Native route registrations. Each route is one `native_route` line (here
/// or in [`native`]); the route map mirrors `PORT-READINESS.md`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An unmatched path gets the backend's framework 404 (the fallback the
    /// retired reverse proxy used to occupy), not a proxy forward.
    #[tokio::test]
    async fn an_unmatched_path_is_the_framework_404() {
        use axum::body::Body;
        use tower::ServiceExt;

        let state = Arc::new(AppState::new(8421));
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/nonexistent")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collects");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("404 body is JSON");
        assert_eq!(value["detail"], "Not Found");
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

        let state = Arc::new(AppState::new(8421));
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
    /// handler's Content-Type carried back, the body collected. The transport
    /// supplies the webview's Origin and the request carries no Host (both
    /// admitted), so the dispatch path is exercised exactly as the socket path
    /// would be.
    #[tokio::test]
    async fn dispatch_in_process_routes_a_request_through_the_in_process_router() {
        let state = Arc::new(AppState::new(8421));
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

    /// The public-boundary Host guard still rejects a foreign Host. The IPC
    /// transport never sends a Host header (the guard then admits the request,
    /// v0.1.0 parity), but a request that presents a Host outside the loopback
    /// allowlist is refused with the backend's 403, so the guard's coverage
    /// survives the socket's removal.
    #[tokio::test]
    async fn the_host_guard_rejects_a_foreign_host_over_ipc() {
        let state = Arc::new(AppState::new(8421));
        let response = dispatch_in_process(
            state,
            "GET",
            "/api/health",
            &[("host".to_string(), "rebound.example:8421".to_string())],
            Vec::new(),
        )
        .await
        .expect("the dispatch succeeds");
        assert_eq!(response.status, 403);
        let body: serde_json::Value =
            serde_json::from_slice(&response.body).expect("the 403 body is JSON");
        assert_eq!(body["detail"], "Invalid Host header");
    }

    /// A mutating in-process request that arrives without an Origin (every
    /// `api_request`/`invoke` call: the webview attaches none) is admitted by
    /// the origin guard, because the transport supplies the webview's own
    /// allow-listed Origin. Without it the guard 403s every write before
    /// routing; here the request reaches the router instead (a POST to the
    /// GET-only health route is a 405, not a 403 "Origin header required").
    /// Regression for the loopback-socket removal, which dropped the Origin the
    /// browser used to attach to the equivalent `fetch`.
    #[tokio::test]
    async fn a_mutating_in_process_request_without_an_origin_is_admitted() {
        let state =
            Arc::new(AppState::new(8421).with_cors(crate::cors::CorsConfig::new(5173, None)));
        let response = dispatch_in_process(state, "POST", "/api/health", &[], Vec::new())
            .await
            .expect("the dispatch succeeds");
        assert_eq!(
            response.status, 405,
            "the mutating request passed the origin guard and reached the router \
             (405 method-not-allowed), rather than being refused with a 403"
        );
    }

    /// The supplied Origin is only a fallback for the gap the removed socket
    /// left: a caller that DOES present an explicitly disallowed Origin on a
    /// mutating request is still refused, so the guard's coverage is preserved
    /// (the transport fills the absent-Origin case, it does not blanket-disable
    /// the check).
    #[tokio::test]
    async fn an_explicit_disallowed_origin_on_a_mutating_request_is_refused() {
        let state =
            Arc::new(AppState::new(8421).with_cors(crate::cors::CorsConfig::new(5173, None)));
        let response = dispatch_in_process(
            state,
            "POST",
            "/api/health",
            &[("origin".to_string(), "http://evil.example".to_string())],
            Vec::new(),
        )
        .await
        .expect("the dispatch succeeds");
        assert_eq!(response.status, 403);
        let body: serde_json::Value =
            serde_json::from_slice(&response.body).expect("the 403 body is JSON");
        assert_eq!(body["detail"], "Origin header required");
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
