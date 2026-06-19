mod composition;
mod crash;
mod resources;
mod telemetry;

use std::sync::Mutex;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

#[tauri::command]
fn toggle_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            // The overlay is a pre-spawned hidden window shown without focus, so
            // no focus/visibility event reaches its webview on show. Signal the
            // show explicitly so it can re-read config/runtime state that no
            // backend event announces (otherwise it stays stale until restart).
            let _ = app.emit("overlay-shown", ());
        }
    }
}

#[tauri::command]
fn show_scan_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("scan-overlay") {
        // Position near top-left so the overlay never collides with a
        // bottom-right docked in-game skills/professions panel.
        let _ = window.set_position(tauri::PhysicalPosition::new(40, 40));
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn hide_scan_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("scan-overlay") {
        let _ = window.hide();
    }
}

/// The backend-call IPC seam: the typed request descriptor the frontend's
/// `tauriFetch` sends in place of a loopback HTTP request. `path` carries the
/// `/api/...` path plus any query string; `headers` round-trips the request
/// headers (Content-Type, If-None-Match); `body` is the JSON request body.
#[derive(serde::Deserialize)]
struct ApiRequest {
    method: String,
    path: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    body: Option<String>,
}

/// The response the seam returns, mirroring an HTTP response so the frontend
/// rebuilds a `Response` the existing openapi-fetch client consumes unchanged.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponse {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Dispatch a frontend backend call through the in-process router (no socket):
/// the server side of the IPC transport that replaces the loopback HTTP hop.
/// The shell publishes the composed state to this command only once the native
/// spine is installed; until then (a brief startup window, or permanently if
/// composition declines) the command errors. The frontend's initial reads then
/// fail and are re-driven by the `substrate:native-installed` event the shell
/// emits the moment the spine is live (there is no transport-level retry).
#[tauri::command]
async fn api_request(app: tauri::AppHandle, request: ApiRequest) -> Result<ApiResponse, String> {
    let state = app
        .try_state::<ApiSubstrate>()
        .ok_or("backend substrate not ready")?
        .0
        .clone();
    let body = request.body.unwrap_or_default().into_bytes();
    let response = eo_http::dispatch_in_process(
        state,
        &request.method,
        &request.path,
        &request.headers,
        body,
    )
    .await?;
    Ok(ApiResponse {
        status: response.status,
        status_text: response.status_text,
        headers: response.headers,
        // The first slice carries JSON routes; the raw-bytes capture-PNG route
        // moves to its own base64-returning command in a later slice.
        body: String::from_utf8_lossy(&response.body).into_owned(),
    })
}

/// GET the manual-scan capture preview PNG for `page`, base64-encoded for an
/// `<img>` `data:` URL. The route returns raw image bytes and is excluded from
/// the JSON IPC envelope (`api_request` carries text bodies), so it rides its
/// own command, dispatched through the same in-process router (no socket).
#[tauri::command]
async fn capture_png(app: tauri::AppHandle, page: u32) -> Result<String, String> {
    use base64::Engine as _;
    let state = app
        .try_state::<ApiSubstrate>()
        .ok_or("backend substrate not ready")?
        .0
        .clone();
    let path = format!("/api/scan/skills/capture/{page}");
    let response = eo_http::dispatch_in_process(state, "GET", &path, &[], Vec::new()).await?;
    if response.status != 200 {
        return Err(format!(
            "capture preview unavailable (status {})",
            response.status
        ));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(&response.body))
}

// Holds the substrate's live producer spine so the Tauri exit seam can
// stop it deterministically. The substrate task composes the producers
// inside its own async context (after the database opens) and hands the
// spine here; `RunEvent::Exit` then stops the chat-log tail thread and
// ends any open session before the process tears down. There is no
// graceful-shutdown signal into the substrate's compose path, so this exit
// seam is the producer teardown path.
struct Producers(Mutex<Option<composition::ProducerState>>);

// Holds the substrate's warmed OCR engine for the app's lifetime. The
// composition root constructs and warms it (binding the bundled ONNX
// Runtime), then hands it here so it outlives composition and is reachable
// when the scan consumer routes flip to it. Unlike `Producers`, there is
// no exit-seam stop: the engine owns no thread and no subscription, its
// ONNX session drops with this managed state, and the ORT environment
// self-releases via its own process-exit hook. `None` when the runtime or
// model was unavailable (OCR is an optional faculty).
//
// The handle is held to keep the warmed engine (and its ONNX session)
// alive for the substrate's lifetime; the scan routes that read it flip
// in a later cutover, so it is not read directly yet. The `dead_code`
// allow is scoped to this one field (not the struct, not the module) and
// justified by that held-not-read intent, the same shape the producer
// spine documents for its kept-alive bus subscriptions; it is removed
// when the scan routes start reading the handle.
struct OcrEngineState(#[allow(dead_code)] Mutex<Option<std::sync::Arc<composition::OcrEngine>>>);

// Holds the manual-scan input listener and the scan state machine so the exit
// seam tears them down: the spacebar listener detaches its share of the shared
// OS keyboard hook (the hotbar listener detaches the other share via
// `ProducerState::stop`), and the scan resets any in-flight capture state.
// Both are the same `Arc`s the HTTP app state serves the scan routes over.
struct ScanInput {
    spacebar: std::sync::Arc<composition::SpacebarCaptureListener>,
    skill_scan: std::sync::Arc<composition::SkillScanManual>,
}

// Holds the composed HTTP substrate state so the `api_request` IPC command can
// dispatch the in-process router (the loopback-socket replacement). The
// substrate task hands it here once the AppState is built; `api_request` reads
// it per call and errors until then.
struct ApiSubstrate(std::sync::Arc<eo_http::AppState>);

#[cfg(windows)]
struct RuntimeWindowIcons(Mutex<Vec<isize>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install the process-wide tracing subscriber first, before anything
    // else runs, so every diagnostic and every instrumented seam is captured
    // from the first instant. The guard is held for the whole process so the
    // rolling log appender flushes at exit.
    let _telemetry = telemetry::init();

    // Install the default-off, opt-in crash reporter's panic hook. By default
    // it adds nothing to the standard panic behaviour; only when the user has
    // opted in does a panic write a PII-scrubbed, local-only report.
    crash::install_panic_hook(composition::data_dir());

    // Start the periodic resource sampler feeding the drift gauges (the metrics
    // page reads them live; each sample is also logged so the rolling file
    // carries the resource-drift series for long-running-session leak detection).
    resources::spawn_resource_sampler();

    let app = tauri::Builder::default()
        // The shell plugin stays for its `open` API (external links route to
        // the OS browser via `$lib/utils/openExternal`); the sidecar/execute
        // usage was removed with the Python backend at the Phase-9 crossing.
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            toggle_overlay,
            show_scan_overlay,
            hide_scan_overlay,
            api_request,
            capture_png
        ])
        .setup(|app| {
            // `app` is unused on a non-Windows debug build (the runtime icon
            // install is Windows-only); keep the binding alive there.
            let _ = &app;
            #[cfg(windows)]
            install_runtime_window_icons(app.handle());
            // The single pure-Rust binary: the frontend reaches the backend
            // through the in-process IPC command (no inbound socket) and every
            // route is served natively (the Python sidecar was decommissioned
            // at the Phase-9 crossing). Startup composes the native spine off
            // the setup path and publishes it to the IPC command when ready.
            // Dev and release compose identically; the resource dir (the
            // bundled snapshot / model / demo assets) resolves only in the
            // installed build, dev falling back to the repository copies.
            compose_substrate(app.handle().clone(), app.path().resource_dir().ok());
            Ok(())
        })
        .on_window_event(|window, event| {
            // The overlay + scan-overlay windows are configured invisible
            // but still count toward Tauri's "exit when all windows close"
            // tally — closing main alone leaves them open and the app
            // keeps running headless. Treat main-window close as a
            // request to exit the whole app.
            if let WindowEvent::CloseRequested { .. } = event {
                if window.label() == "main" {
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            #[cfg(windows)]
            destroy_runtime_window_icons(app);

            // Stop the producer spine: end any open session so its stop
            // events publish, then stop the chat-log tail thread. The
            // substrate's compose task has no shutdown signal, so this is
            // where the producers wind down.
            if let Some(state) = app.try_state::<Producers>() {
                if let Ok(mut guard) = state.0.lock() {
                    if let Some(producers) = guard.take() {
                        producers.stop();
                    }
                }
            }

            // Tear down the scan input listener and reset the scan: the
            // spacebar listener detaches its share of the shared OS hook.
            if let Some(state) = app.try_state::<ScanInput>() {
                state.spacebar.stop();
                state.skill_scan.shutdown();
            }
        }
    });
}

#[cfg(windows)]
fn install_runtime_window_icons(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        match windows_runtime_icons::set_for_main_window(&window) {
            Ok(handles) => {
                app.manage(RuntimeWindowIcons(Mutex::new(handles)));
            }
            Err(err) => tracing::warn!(target: "eo::icon", "runtime icon install failed: {err}"),
        }
    }
}

#[cfg(windows)]
fn destroy_runtime_window_icons(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<RuntimeWindowIcons>() {
        if let Ok(mut handles) = state.0.lock() {
            windows_runtime_icons::destroy(handles.drain(..));
        }
    }
}

/// The nominal public loopback authority the inbound Host allowlist is
/// derived from. The frontend reaches the backend over the in-process IPC
/// command and sends no Host header, so this only bites a request presenting
/// an explicit Host (which the IPC transport never does); 8421 unless the dev
/// environment overrides it.
fn public_backend_port() -> u16 {
    std::env::var("ENTROPIAORME_BACKEND_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8421)
}

/// Compose the native backend substrate and publish it to the IPC command.
/// The single pure-Rust binary serves every route natively in-process: there
/// is no socket, no sidecar, and no proxy. This builds the shared `AppState`,
/// composes the native service spine off the setup path, and on success
/// installs the services, publishes the state to the `api_request` command,
/// and signals the frontend (see [`install_native_services`]). Until
/// composition lands the `api_request` command errors; the frontend's initial
/// reads are re-driven by the `substrate:native-installed` event the install
/// emits (there is no transport-level retry). A declined composition is logged
/// and the backend does not come up for the session (an unopenable database, or
/// one below the supported baseline the retired sidecar used to migrate
/// forward).
fn compose_substrate(app: tauri::AppHandle, resource_dir: Option<std::path::PathBuf>) {
    tauri::async_runtime::spawn(async move {
        let state = std::sync::Arc::new(
            eo_http::AppState::new(public_backend_port())
                // Answer the browser surface (preflights, origin rules,
                // response decoration) from the same environment inputs.
                .with_cors(eo_http::cors::CorsConfig::from_env())
                // The data dir powers the hidden dev-tools routes (the
                // developer-mode gate and the crash-reporting toggle).
                .with_data_dir(composition::data_dir())
                // The bundled demo database powers the guide-mode `/api/demo`
                // surface (a writable per-process clone, stood up lazily).
                .with_demo_db_path(composition::demo_db_path(resource_dir.as_ref())),
        );
        match composition::compose_native(resource_dir).await {
            composition::Composition::Ready(composed) => {
                install_native_services(&app, &state, composed);
            }
            composition::Composition::Declined => {
                tracing::error!(
                    target: "eo::substrate",
                    "native services did not compose; the backend is unavailable for this session"
                );
            }
        }
    });
}

/// Install the composed services into the app state and hand the stoppable
/// handles to the Tauri-managed exit seam, then publish the state to the
/// `api_request` IPC command (so the command answers only once every service
/// is present) and signal the frontend that the backend is live.
fn install_native_services(
    app: &tauri::AppHandle,
    state: &std::sync::Arc<eo_http::AppState>,
    composed: composition::Composed,
) {
    // Clones for the exit seam, taken before the originals move into the
    // installed bundle below: the spacebar listener detaches its share of
    // the shared OS hook and the scan resets in-flight state on close.
    let exit_spacebar = composed.spacebar_listener.clone();
    let exit_skill_scan = composed.skill_scan.clone();
    // The producer-spine handles are cloned out of the spine here, BEFORE it
    // moves into the Tauri-managed holder below, so the HTTP routes serve over
    // the same handles the exit-seam teardown stops, and the settings-write
    // route restarts the same `Arc<ChatlogWatcher>` the spine tails on.
    state.install_native(eo_http::NativeServices {
        hydration: composed.hydration,
        tracker: composed.producers.tracker_handle(),
        chatlog_watcher: composed.producers.watcher_handle(),
        config_service: composed.producers.config_service_handle(),
        skill_tracker: composed.producers.skill_tracker_handle(),
        skill_scan: composed.skill_scan.clone(),
        repair_ocr: composed.repair_ocr,
        spacebar_listener: composed.spacebar_listener.clone(),
        hotbar_listener: composed.producers.hotbar_handle(),
    });
    // Forward the producer spine's domain events onto the Tauri event bus, the
    // native replacement for the frontend's old EventSource relay. Registers a
    // consumer on the producer spine's SseHub while the spine is still in hand
    // (it moves into managed state just below).
    spawn_domain_event_bridge(app, &composed.producers.sse_hub_handle());
    // Hand the producer spine to the exit seam so it stops the tail thread,
    // the hotbar listener, and ends any session on close.
    app.manage(Producers(Mutex::new(Some(composed.producers))));
    // Hold the warmed OCR engine for the app's lifetime (no exit stop: the
    // session drops with the managed state, the ORT env self-releases at
    // process exit).
    app.manage(OcrEngineState(Mutex::new(composed.ocr_engine)));
    // Hand the scan input listener and the scan state machine to the exit
    // seam for deterministic teardown.
    app.manage(ScanInput {
        spacebar: exit_spacebar,
        skill_scan: exit_skill_scan,
    });
    // Publish the composed state to the IPC command LAST: until now
    // `api_request` errors with "backend substrate not ready", so by the time
    // any request dispatches every native service is present (there is no
    // absent-service window to fall back from). Then signal the frontend that
    // the backend is live so it (re-)hydrates its
    // initial reads.
    app.manage(ApiSubstrate(state.clone()));
    let _ = app.emit("substrate:native-installed", ());
}

/// Map a dotted domain wire topic to its colon-form Tauri event name (Tauri
/// event names forbid dots). Mirrors the frontend's `toTauriEventName`.
fn domain_topic_to_tauri_event(topic: &str) -> String {
    topic.replace('.', ":")
}

/// Extract the `event:` topic and `data:` payload from one SSE frame
/// (`id: N\nevent: <topic>\ndata: <json>\n\n`). The envelope JSON is in its
/// compact wire form (no embedded newline), so a line scan recovers both
/// fields exactly.
fn parse_domain_frame(frame: &str) -> Option<(&str, &str)> {
    let mut topic = None;
    let mut data = None;
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            topic = Some(rest);
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = Some(rest);
        }
    }
    Some((topic?, data?))
}

/// Forward the producer spine's domain events onto the Tauri event bus: the
/// native replacement for the frontend's old `EventSource` relay. Registers a
/// consumer on the producer spine's `SseHub`, drains its frames, and re-emits
/// each typed envelope on the colon-form Tauri topic every window subscribes
/// to (`tracking:session:updated`, `scan:status:changed`). The webview sees
/// the identical envelope it parsed off the SSE `data:` field, so the
/// topic-aware consumers are unchanged. The hydrate nudge (a payload-less
/// frame on start) stays frontend-owned: it must fire after the webview is
/// listening, which an emit at install time cannot guarantee on a cold load.
fn spawn_domain_event_bridge(app: &tauri::AppHandle, hub: &std::sync::Arc<eo_wire::sse::SseHub>) {
    let client = hub.register();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let frame = client.next_frame().await;
            let Some((topic, data)) = parse_domain_frame(&frame) else {
                continue;
            };
            match serde_json::from_str::<serde_json::Value>(data) {
                Ok(value) => {
                    let _ = app.emit(domain_topic_to_tauri_event(topic).as_str(), value);
                }
                Err(err) => tracing::warn!(
                    target: "eo::substrate",
                    "dropping a malformed domain frame: {err}"
                ),
            }
        }
    });
}

#[cfg(windows)]
mod windows_runtime_icons {
    use tauri::WebviewWindow;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                HiDpi::GetDpiForWindow,
                WindowsAndMessaging::{
                    DestroyIcon, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
                    ICON_SMALL2, IMAGE_ICON, LR_DEFAULTCOLOR, WM_SETICON,
                },
            },
        },
    };

    const APP_ICON_RESOURCE_ID: u16 = 32512;
    const USER_DEFAULT_SCREEN_DPI: u32 = 96;
    const ICON_SIZES: [i32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

    pub fn set_for_main_window(window: &WebviewWindow) -> Result<Vec<isize>, String> {
        let hwnd = window
            .hwnd()
            .map_err(|err| format!("failed to resolve main HWND: {err}"))?;
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let dpi = if dpi == 0 {
            USER_DEFAULT_SCREEN_DPI
        } else {
            dpi
        };

        // Tauri's default Windows path decodes only the first ICO entry and
        // sets ICON_SMALL. Load from the embedded icon group instead so the
        // taskbar gets a DPI-appropriate ICON_BIG handle.
        let small_icon_size = choose_icon_size(16, dpi);
        let taskbar_icon_size = choose_icon_size(24, dpi);

        let small_icon = load_icon(small_icon_size)?;
        let taskbar_icon = load_icon(taskbar_icon_size)?;

        set_window_icon(hwnd, ICON_SMALL, small_icon);
        set_window_icon(hwnd, ICON_SMALL2, small_icon);
        set_window_icon(hwnd, ICON_BIG, taskbar_icon);

        Ok(vec![small_icon.0 as isize, taskbar_icon.0 as isize])
    }

    pub fn destroy(handles: impl Iterator<Item = isize>) {
        for handle in handles {
            let _ = unsafe { DestroyIcon(HICON(handle as _)) };
        }
    }

    fn choose_icon_size(base_size: u32, dpi: u32) -> i32 {
        let desired = base_size
            .saturating_mul(dpi)
            .div_ceil(USER_DEFAULT_SCREEN_DPI) as i32;
        ICON_SIZES
            .iter()
            .copied()
            .find(|size| *size >= desired)
            .unwrap_or(256)
    }

    fn load_icon(size: i32) -> Result<HICON, String> {
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map(Into::into)
            .ok();
        let resource = PCWSTR::from_raw(APP_ICON_RESOURCE_ID as usize as *const u16);
        let handle =
            unsafe { LoadImageW(module, resource, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) }
                .map_err(|err| format!("failed to load {size}x{size} icon resource: {err}"))?;

        Ok(HICON(handle.0))
    }

    fn set_window_icon(hwnd: HWND, icon_type: u32, icon: HICON) {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(icon_type as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{choose_icon_size, ICON_SIZES};

        #[test]
        fn icon_sizes_are_strictly_ascending() {
            // choose_icon_size's `find` returns the first entry >= desired,
            // which is only the *smallest sufficient* size if the table is
            // sorted; this pins that load-bearing ordering invariant.
            assert!(ICON_SIZES.windows(2).all(|pair| pair[0] < pair[1]));
        }

        #[test]
        fn standard_dpi_maps_to_exact_base_sizes() {
            assert_eq!(choose_icon_size(16, 96), 16);
            assert_eq!(choose_icon_size(24, 96), 24);
        }

        #[test]
        fn scaled_dpi_rounds_up_to_next_available_size() {
            // 150% scaling: 16 -> 24 (exact), 24 -> 36 -> next size up is 48.
            assert_eq!(choose_icon_size(16, 144), 24);
            assert_eq!(choose_icon_size(24, 144), 48);
            // 125% scaling: 16 -> 20 (exact table entry via div_ceil).
            assert_eq!(choose_icon_size(16, 120), 20);
        }

        #[test]
        fn oversized_demand_clamps_to_largest_icon() {
            assert_eq!(choose_icon_size(256, 480), 256);
            // Saturating multiply keeps absurd DPI values from overflowing.
            assert_eq!(choose_icon_size(u32::MAX, u32::MAX), 256);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{domain_topic_to_tauri_event, parse_domain_frame};

    #[test]
    fn domain_topics_namespace_dots_to_colons_for_the_tauri_bus() {
        assert_eq!(
            domain_topic_to_tauri_event("tracking.session.updated"),
            "tracking:session:updated"
        );
        assert_eq!(
            domain_topic_to_tauri_event("scan.status.changed"),
            "scan:status:changed"
        );
    }

    #[test]
    fn a_domain_frame_yields_its_topic_and_compact_envelope() {
        // The exact shape the SseHub frames (see eo_wire::sse): the envelope is
        // compact JSON on one line, so the data field is recovered whole.
        let frame = concat!(
            "id: 7\nevent: scan.status.changed\ndata: ",
            "{\"type\":\"scan.status.changed\",\"event_version\":1,",
            "\"occurred_at\":\"2024-12-31T21:20:00+00:00\",",
            "\"payload\":{\"phase\":\"capturing\"}}\n\n"
        );
        let (topic, data) = parse_domain_frame(frame).expect("a well-formed frame parses");
        assert_eq!(topic, "scan.status.changed");
        let value: serde_json::Value = serde_json::from_str(data).expect("the data is JSON");
        assert_eq!(value["type"], "scan.status.changed");
        assert_eq!(value["payload"]["phase"], "capturing");
    }

    #[test]
    fn a_frame_missing_a_field_does_not_parse() {
        assert!(parse_domain_frame("id: 1\nevent: scan.status.changed\n\n").is_none());
        assert!(parse_domain_frame(": ready\n\n").is_none());
    }

    /// The frontend reaches the backend only through the in-process IPC command,
    /// so the substrate binds no inbound listener (see `compose_substrate`) and
    /// the security policy grants no loopback origin: the CSP carries no
    /// `127.0.0.1`/`8421` in connect-src or img-src. The external news origin,
    /// the IPC scheme, and the base64 image `data:` source survive.
    #[test]
    fn the_security_policy_grants_no_loopback_origin() {
        let conf = include_str!("../tauri.conf.json");
        let after = conf
            .split("\"csp\":")
            .nth(1)
            .expect("the security CSP is configured");
        let csp = after
            .split('"')
            .nth(1)
            .expect("the CSP is a string literal");
        assert!(
            !csp.contains("127.0.0.1") && !csp.contains("8421"),
            "the CSP must grant no loopback origin once the frontend is IPC-only: {csp}"
        );
        assert!(
            csp.contains("https://entropiaorme.com"),
            "the external news origin must survive the loopback strip: {csp}"
        );
        assert!(csp.contains("ipc:"), "the IPC scheme must remain: {csp}");
        assert!(
            csp.contains("img-src 'self' data:"),
            "img-src keeps data: for the base64 capture preview: {csp}"
        );
    }

    /// M4/M6: the bundle ships no Python sidecar. The packaging spec declares
    /// no `externalBin`, and the shell `execute`/sidecar capability is gone
    /// (only `open` survives, for external links), so the installed artefact
    /// carries the single native binary alone.
    #[test]
    fn the_bundle_declares_no_sidecar_binary() {
        let conf = include_str!("../tauri.conf.json");
        assert!(
            !conf.contains("externalBin"),
            "the packaging spec must declare no externalBin once the sidecar is decommissioned"
        );
        let capabilities = include_str!("../capabilities/default.json");
        assert!(
            !capabilities.contains("shell:allow-execute"),
            "the shell execute/sidecar capability must be gone: {capabilities}"
        );
        assert!(
            capabilities.contains("shell:allow-open"),
            "the shell open capability must survive for external links: {capabilities}"
        );
    }
}
