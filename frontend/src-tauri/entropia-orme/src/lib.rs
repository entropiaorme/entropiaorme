mod composition;
mod crash;
mod resources;
mod telemetry;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command as StdCommand;
use std::sync::Mutex;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

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

// Holds the spawned Python backend so we can terminate it on app exit.
// Default Windows process semantics leave the child running when the
// parent exits; without this kill on RunEvent::Exit the sidecar would
// linger (and keep port 8421 bound) past app close.
struct SidecarChild(Mutex<Option<CommandChild>>);

// Holds the substrate's live producer spine so the Tauri exit seam can
// stop it deterministically. The substrate task composes the producers
// inside its own async context (after the database opens) and hands the
// spine here; `RunEvent::Exit` then stops the chat-log tail thread and
// ends any open session before the process tears down. There is no
// graceful-shutdown signal into the substrate's serve loop, so this exit
// seam is the producer teardown path (the same seam that kills the
// sidecar child).
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
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            toggle_overlay,
            show_scan_overlay,
            hide_scan_overlay
        ])
        .setup(|app| {
            // Both uses of `app` compile out on a non-Windows debug build
            // (icon install is Windows-only, the sidecar spawn is
            // release-only); keep the binding alive for that combination.
            let _ = &app;
            #[cfg(windows)]
            install_runtime_window_icons(app.handle());
            // Only the bundled release shell spawns the PyInstaller sidecar.
            // Dev builds talk to a separately launched backend, and the dev
            // sidecar slot holds a placeholder binary that Windows rejects
            // (os error 193); gating the spawn to release keeps that error
            // out of the dev console.
            //
            // Topology: the native HTTP substrate owns the public loopback
            // port the frontend is wired to; the sidecar relocates to a
            // private port behind the reverse proxy. Every failure path
            // degrades to the legacy direct topology (sidecar on the public
            // port, no proxy) so a substrate fault never takes the app down.
            #[cfg(not(debug_assertions))]
            {
                let relocation = bind_substrate_listener()
                    .and_then(|listener| allocate_private_port().map(|port| (listener, port)));
                match relocation {
                    Some((listener, sidecar_port)) => {
                        spawn_backend_sidecar(
                            app.handle(),
                            SidecarSpawn::RelocatedIdle(sidecar_port),
                        );
                        spawn_http_substrate(
                            app.handle().clone(),
                            listener,
                            sidecar_port,
                            app.path().resource_dir().ok(),
                            // The substrate owns production once it relocates
                            // and idles the sidecar; if the native spine never
                            // composes, recover by respawning that sidecar as
                            // the producer so the session is never left with
                            // no producer at all.
                            true,
                        );
                    }
                    None => spawn_backend_sidecar(app.handle(), SidecarSpawn::Legacy),
                }
            }
            // Dev runs the backend from the dev launcher; the substrate
            // joins in only when that launcher published the backend's
            // private port, which keeps a plain unproxied dev stack working.
            #[cfg(debug_assertions)]
            if let Some(sidecar_port) = dev_sidecar_port() {
                if let Some(listener) = bind_substrate_listener() {
                    // Dev's backend is launched (and owned) by the dev
                    // launcher, never idled by us, so there is no relocated
                    // sidecar for the substrate to recover.
                    spawn_http_substrate(app.handle().clone(), listener, sidecar_port, None, false);
                }
            }
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

            // Stop the producer spine first (before the sidecar dies):
            // end any open session so its stop events publish, then stop
            // the chat-log tail thread. The substrate's serve task has no
            // shutdown signal, so this is where the producers wind down.
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

            if let Some(state) = app.try_state::<SidecarChild>() {
                if let Ok(mut guard) = state.0.lock() {
                    if let Some(child) = guard.take() {
                        kill_sidecar_tree(&child);
                        // Dropping the handle after taskkill /T /F has
                        // already terminated the process; no extra
                        // TerminateProcess needed.
                        drop(child);
                    }
                }
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

fn kill_sidecar_tree(child: &CommandChild) {
    // PyInstaller onefile launches a bootloader process that unpacks
    // the bundle and spawns a child python interpreter. CommandChild::kill
    // calls TerminateProcess on just the bootloader; the python child
    // orphans (Windows does not cascade kills). Use taskkill /T /F to
    // walk the process tree.
    let pid = child.pid();
    let mut cmd = StdCommand::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    // GUI-subsystem parent spawning a console-subsystem child (taskkill)
    // without window suppression allocates a conhost console that flashes
    // briefly during the call. CREATE_NO_WINDOW = 0x08000000 keeps the
    // cleanup invisible.
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let _ = cmd.output();
}

/// The public loopback port the frontend dials (`client.ts` and the CSP
/// are baked against it; 8421 unless the dev environment overrides).
fn public_backend_port() -> u16 {
    std::env::var("ENTROPIAORME_BACKEND_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8421)
}

/// Bind the substrate's public listener up front so relocation only
/// happens once the public port is actually ours.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn bind_substrate_listener() -> Option<std::net::TcpListener> {
    let port = public_backend_port();
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            if let Err(err) = listener.set_nonblocking(true) {
                tracing::warn!(target: "eo::substrate", "nonblocking mode failed: {err}");
                return None;
            }
            Some(listener)
        }
        Err(err) => {
            tracing::warn!(target: "eo::substrate", "public port {port} bind failed: {err}");
            None
        }
    }
}

/// An OS-allocated free loopback port for the relocated sidecar.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn allocate_private_port() -> Option<u16> {
    match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener.local_addr().ok().map(|addr| addr.port()),
        Err(err) => {
            tracing::warn!(target: "eo::substrate", "private port allocation failed: {err}");
            None
        }
    }
}

/// Dev launchers publish the externally-started backend's private port
/// here when the dev stack should run through the substrate.
#[cfg(debug_assertions)]
fn dev_sidecar_port() -> Option<u16> {
    std::env::var("ENTROPIAORME_SIDECAR_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
}

/// Serve the strangler router on the already-bound public listener,
/// proxying not-yet-ported routes to the sidecar on `sidecar_port`. The
/// substrate begins serving (proxy-only) the instant it binds; native
/// services compose in the background and hot-install when ready, so a
/// first launch after an upgrade (where the database is briefly below the
/// adoptable baseline while the sidecar migrates it forward) upgrades to
/// native without a restart, instead of standing down proxy-only for the
/// whole session.
fn spawn_http_substrate(
    app: tauri::AppHandle,
    listener: std::net::TcpListener,
    sidecar_port: u16,
    resource_dir: Option<std::path::PathBuf>,
    recover: bool,
) {
    let overrides = route_arm_overrides();
    tauri::async_runtime::spawn(async move {
        let app_state = eo_http::AppState::new(
            format!("127.0.0.1:{sidecar_port}"),
            public_backend_port(),
            overrides,
        )
        // The substrate answers the browser surface (preflights, origin
        // rules, response decoration) exactly as the sidecar's own
        // middleware would, from the same environment inputs.
        .with_cors(eo_http::cors::CorsConfig::from_env())
        // The data dir powers the hidden dev-tools routes (the developer-mode
        // gate and the crash-reporting toggle).
        .with_data_dir(composition::data_dir());
        let state = std::sync::Arc::new(app_state);

        // Compose the native services off the serve path and install them
        // when ready, so the substrate answers (proxy-only) the instant it
        // binds rather than blocking startup on composition. The natively
        // registered routes each fall back to the proxy arm while their
        // service is absent, so the install flips them to native on the next
        // request with no re-registration and no restart.
        compose_and_install(
            app,
            state.clone(),
            resource_dir,
            // Only the release relocated topology spawned an idled sidecar
            // for the substrate to recover into a producer on a permanent
            // decline; dev (and any non-relocated path) passes None.
            recover.then_some(sidecar_port),
        );

        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::error!(target: "eo::substrate", "listener handoff failed: {err}");
                return;
            }
        };
        if let Err(err) = eo_http::serve(listener, state).await {
            tracing::error!(target: "eo::substrate", "server exited: {err}");
        }
    });
}

/// How often the composition retry re-attempts adoption while the database
/// is below the adoptable baseline.
const COMPOSE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// The ceiling on that retry. A first launch after an upgrade waits while
/// the co-bundled sidecar unpacks (a ~180 MB onefile, possibly under
/// on-access scanning) and migrates the database forward; the ceiling is
/// generous enough to cover a slow cold boot while still surrendering to
/// proxy-only if the database never reaches the baseline (a dead sidecar),
/// rather than spinning forever.
const COMPOSE_RETRY_CEILING: std::time::Duration = std::time::Duration::from_secs(120);

/// Compose the native services off the serve path and install them into the
/// already-serving `state` once ready. Retries ONLY while the database is
/// below the adoptable baseline (the first-launch-after-upgrade race, where
/// the sidecar is still migrating it forward).
///
/// A permanent decline, or the retry ceiling, leaves the substrate proxy-only
/// for the session. In the relocated topology (`recovery_port` is `Some`) the
/// sidecar was spawned idle in anticipation of a native producer spine, so a
/// proxy-only outcome would leave the session with NO producer; recovery then
/// respawns that sidecar as the producer (see [`recover_orphaned_production`]).
/// `recovery_port` is `None` when there is no idled sidecar we own (dev, where
/// the launcher owns the backend).
#[cfg_attr(debug_assertions, allow(dead_code))]
fn compose_and_install(
    app: tauri::AppHandle,
    state: std::sync::Arc<eo_http::AppState>,
    resource_dir: Option<std::path::PathBuf>,
    recovery_port: Option<u16>,
) {
    tauri::async_runtime::spawn(async move {
        let mut waited = std::time::Duration::ZERO;
        let mut announced = false;
        loop {
            match composition::compose_native(resource_dir.clone()).await {
                composition::Composition::Ready(composed) => {
                    install_native_services(&app, &state, composed);
                    return;
                }
                composition::Composition::AwaitingMigration => {
                    if !announced {
                        tracing::info!(
                            target: "eo::substrate",
                            "database below the adoptable baseline; serving proxy-only while the \
                             sidecar migrates it, then upgrading to native"
                        );
                        announced = true;
                    }
                    if waited >= COMPOSE_RETRY_CEILING {
                        tracing::warn!(
                            target: "eo::substrate",
                            "database stayed below the adoptable baseline for {}s; serving \
                             proxy-only for the session",
                            COMPOSE_RETRY_CEILING.as_secs()
                        );
                        recover_orphaned_production(&app, recovery_port);
                        return;
                    }
                    tokio::time::sleep(COMPOSE_RETRY_INTERVAL).await;
                    waited += COMPOSE_RETRY_INTERVAL;
                }
                composition::Composition::Declined => {
                    recover_orphaned_production(&app, recovery_port);
                    return;
                }
            }
        }
    });
}

/// Recover production after the native spine failed to compose. In the
/// relocated topology the sidecar was spawned idle (`RelocatedIdle`) because
/// the substrate's native spine was expected to own production; if that spine
/// never composes the substrate stays proxy-only, and an idled sidecar would
/// leave the session with no chat-log tailer, no tracker, and no OS hooks
/// (live tracking silently dead, while proxied reads keep serving stale
/// state). Respawn the relocated sidecar PRODUCING so it resumes the producer
/// role it holds in the legacy topology; the substrate keeps proxying the
/// public port to it. A no-op when `recovery_port` is `None` (no idled sidecar
/// we own: dev, or the legacy direct topology, where the sidecar already
/// produces).
#[cfg_attr(debug_assertions, allow(dead_code))]
fn recover_orphaned_production(app: &tauri::AppHandle, recovery_port: Option<u16>) {
    let Some(sidecar_port) = recovery_port else {
        return;
    };
    tracing::warn!(
        target: "eo::substrate",
        "native services did not compose; respawning the sidecar as the producer so live \
         tracking continues (serving proxy-only for the session)"
    );
    // Kill the idled relocated sidecar before rebinding its private port: the
    // substrate's proxy authority is fixed at that port, so the producing
    // respawn must reclaim it. The exit seam reads the same handle, so the
    // kill and the re-store both go through the shared `SidecarChild` lock.
    if let Some(state) = app.try_state::<SidecarChild>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(child) = guard.take() {
                kill_sidecar_tree(&child);
                drop(child);
            }
        }
    }
    spawn_backend_sidecar(app, SidecarSpawn::RelocatedProducing(sidecar_port));
}

/// Install the composed services into the live, already-serving app state
/// (flipping the native routes off their proxy fallback) and hand the
/// stoppable handles to the Tauri-managed exit seam, exactly as the inline
/// composition did before the retry moved it off the serve path.
#[cfg_attr(debug_assertions, allow(dead_code))]
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
    // The producer-spine handles (tracker, SSE hub, ...) are cloned out of
    // the spine here, BEFORE it moves into the Tauri-managed holder below,
    // so the producer routes serve over the same `Arc<HuntTracker>` the
    // exit-seam teardown stops and the `/api/events` stream serves over the
    // same `Arc<SseHub>` the producer-bus bridge feeds.
    state.install_native(eo_http::NativeServices {
        hydration: composed.hydration,
        tracker: composed.producers.tracker_handle(),
        sse_hub: composed.producers.sse_hub_handle(),
        config_service: composed.producers.config_service_handle(),
        skill_tracker: composed.producers.skill_tracker_handle(),
        skill_scan: composed.skill_scan.clone(),
        repair_ocr: composed.repair_ocr,
        spacebar_listener: composed.spacebar_listener.clone(),
        hotbar_listener: composed.producers.hotbar_handle(),
    });
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
}

/// Runtime per-route arm overrides: a persisted JSON map (path in
/// `ENTROPIAORME_ROUTE_ARMS_FILE`) overlaid by the inline
/// `ENTROPIAORME_ROUTE_ARMS` list. The kill-switch for a misbehaving
/// native route in a shipped build.
fn route_arm_overrides() -> eo_http::arms::ArmOverrides {
    let mut overrides = eo_http::arms::ArmOverrides::empty();
    if let Ok(path) = std::env::var("ENTROPIAORME_ROUTE_ARMS_FILE") {
        overrides = overrides.overlaid(eo_http::arms::ArmOverrides::from_json_file(
            std::path::Path::new(&path),
        ));
    }
    if let Ok(inline) = std::env::var("ENTROPIAORME_ROUTE_ARMS") {
        overrides = overrides.overlaid(eo_http::arms::ArmOverrides::parse_env_value(&inline));
    }
    overrides
}

/// How a sidecar process is spawned, which fixes its environment and so its
/// role.
///
/// When the substrate relocates the sidecar it has taken the public port and
/// owns production: its composed native spine is the sole chat-log tailer,
/// tracker, and OS-hook owner, so the relocated sidecar must serve proxied
/// reads WITHOUT running its own producers (`RelocatedIdle`). Two producers
/// writing the shared database would double-count loot and cost, and two OS
/// keyboard hooks would conflict (the backend's `_producers_idle` documents
/// the same invariant).
///
/// But idling is correct only while the substrate actually produces. If the
/// native spine never composes (a permanent decline, or the
/// first-launch-after-upgrade migration ceiling), the substrate stays
/// proxy-only and an idled sidecar would leave the session with no producer
/// at all. The recovery respawns the relocated sidecar PRODUCING
/// (`RelocatedProducing`) so it resumes the producer role it holds in the
/// legacy topology, with the substrate still proxying the public port to it.
///
/// The legacy spawn (`Legacy`, the only backend, on the public port) always
/// produces.
#[cfg_attr(debug_assertions, allow(dead_code))]
enum SidecarSpawn {
    /// Legacy direct topology: the only backend, on the public port.
    Legacy,
    /// Relocated behind the substrate proxy on a private port, producers idle
    /// (the substrate's native spine owns production).
    RelocatedIdle(u16),
    /// Relocated behind the substrate proxy on a private port, but producing:
    /// the recovery for when the native spine never composed.
    RelocatedProducing(u16),
}

/// The environment variables a sidecar spawn runs under. The relocated forms
/// bind the private port the substrate proxies to; only `RelocatedIdle` gates
/// the producers off. `Legacy` and `RelocatedProducing` both keep producing.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn sidecar_spawn_env(spawn: &SidecarSpawn) -> Vec<(&'static str, String)> {
    match spawn {
        SidecarSpawn::Legacy => Vec::new(),
        SidecarSpawn::RelocatedIdle(port) => vec![
            ("ENTROPIAORME_BACKEND_PORT", port.to_string()),
            ("ENTROPIAORME_PRODUCERS_IDLE", "1".to_string()),
        ],
        SidecarSpawn::RelocatedProducing(port) => {
            vec![("ENTROPIAORME_BACKEND_PORT", port.to_string())]
        }
    }
}

// In debug builds the call site above is compiled out (dev uses a separately
// launched backend), leaving this function without a caller; silence the
// resulting dead-code lint rather than drop the release-only definition.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn spawn_backend_sidecar(app: &tauri::AppHandle, spawn: SidecarSpawn) {
    let sidecar = match app.shell().sidecar("entropiaorme-backend") {
        Ok(cmd) => cmd,
        Err(err) => {
            tracing::error!(target: "eo::sidecar", "sidecar resolve failed: {err}");
            return;
        }
    };
    // A relocated sidecar reads its bind port (and derives its own Host-header
    // guard) from `ENTROPIAORME_BACKEND_PORT`; the substrate's proxy arm
    // rewrites Host to the private authority accordingly. `RelocatedIdle` also
    // gates its producers off (the substrate owns production); `Legacy` and
    // the `RelocatedProducing` recovery keep producing. See `SidecarSpawn`.
    let sidecar = sidecar_spawn_env(&spawn)
        .into_iter()
        .fold(sidecar, |cmd, (key, value)| cmd.env(key, value));

    let (mut rx, child) = match sidecar.spawn() {
        Ok(pair) => pair,
        Err(err) => {
            tracing::error!(target: "eo::sidecar", "sidecar spawn failed: {err}");
            return;
        }
    };

    store_sidecar_child(app, child);

    // Drain stdout/stderr — Windows pipe buffers are ~4 KB and an unread
    // pipe stalls the child's logging once it fills.
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) | CommandEvent::Stderr(line) => {
                    if let Ok(s) = std::str::from_utf8(&line) {
                        // Forward the sidecar's own (already-sanitised)
                        // diagnostic output into the structured logs.
                        tracing::info!(
                            target: "eo::sidecar",
                            "{}",
                            s.trim_end_matches(['\r', '\n'])
                        );
                    }
                }
                _ => {}
            }
        }
    });
}

/// Hold the spawned sidecar for the exit seam. The first spawn manages the
/// holder; a recovery respawn swaps the new child into the existing holder
/// (its idled predecessor was already taken out and killed under the same
/// lock), so the exit seam always kills whichever sidecar is live.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn store_sidecar_child(app: &tauri::AppHandle, child: CommandChild) {
    if let Some(state) = app.try_state::<SidecarChild>() {
        if let Ok(mut guard) = state.0.lock() {
            *guard = Some(child);
        }
    } else {
        app.manage(SidecarChild(Mutex::new(Some(child))));
    }
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
    use super::{sidecar_spawn_env, SidecarSpawn};

    fn env_value<'a>(env: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        env.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_relocated_idle_sidecar_binds_its_port_and_idles_its_producers() {
        // The substrate owns production once it relocates the sidecar, so the
        // sidecar must serve proxied reads WITHOUT running its own producers,
        // or two chat-log tailers would double-count into the shared database.
        let env = sidecar_spawn_env(&SidecarSpawn::RelocatedIdle(18421));
        assert_eq!(
            env_value(&env, "ENTROPIAORME_BACKEND_PORT"),
            Some("18421"),
            "the relocated sidecar binds its private port"
        );
        assert_eq!(
            env_value(&env, "ENTROPIAORME_PRODUCERS_IDLE"),
            Some("1"),
            "and idles its producers"
        );
    }

    #[test]
    fn a_relocated_producing_sidecar_binds_its_port_but_keeps_producing() {
        // The recovery for when the native spine never composes: the substrate
        // stays proxy-only, so the relocated sidecar must resume production (no
        // idle gate) or the session would have no producer at all.
        let env = sidecar_spawn_env(&SidecarSpawn::RelocatedProducing(18421));
        assert_eq!(
            env_value(&env, "ENTROPIAORME_BACKEND_PORT"),
            Some("18421"),
            "the recovery sidecar binds the same private port the proxy targets"
        );
        assert_eq!(
            env_value(&env, "ENTROPIAORME_PRODUCERS_IDLE"),
            None,
            "the recovery sidecar must NOT idle: it is the sole producer"
        );
    }

    #[test]
    fn a_legacy_sidecar_takes_no_relocation_env_and_produces() {
        // The only backend, on the public port: no private bind, no idle gate.
        let env = sidecar_spawn_env(&SidecarSpawn::Legacy);
        assert!(
            env.is_empty(),
            "the legacy spawn carries no relocation env (no bind override, no idle gate)"
        );
    }
}
