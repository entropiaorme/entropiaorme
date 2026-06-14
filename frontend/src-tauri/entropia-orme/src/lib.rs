mod composition;

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

#[cfg(windows)]
struct RuntimeWindowIcons(Mutex<Vec<isize>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
                        spawn_backend_sidecar(app.handle(), Some(sidecar_port));
                        spawn_http_substrate(
                            app.handle().clone(),
                            listener,
                            sidecar_port,
                            app.path().resource_dir().ok(),
                        );
                    }
                    None => spawn_backend_sidecar(app.handle(), None),
                }
            }
            // Dev runs the backend from the dev launcher; the substrate
            // joins in only when that launcher published the backend's
            // private port, which keeps a plain unproxied dev stack working.
            #[cfg(debug_assertions)]
            if let Some(sidecar_port) = dev_sidecar_port() {
                if let Some(listener) = bind_substrate_listener() {
                    spawn_http_substrate(app.handle().clone(), listener, sidecar_port, None);
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
            Err(err) => eprintln!("[icon] runtime icon install failed: {err}"),
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
                eprintln!("[substrate] nonblocking mode failed: {err}");
                return None;
            }
            Some(listener)
        }
        Err(err) => {
            eprintln!("[substrate] public port {port} bind failed: {err}");
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
            eprintln!("[substrate] private port allocation failed: {err}");
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
/// proxying not-yet-ported routes to the sidecar on `sidecar_port`.
/// Native services compose first (inside the task, before the first
/// request); a declined composition serves proxy-only.
fn spawn_http_substrate(
    app: tauri::AppHandle,
    listener: std::net::TcpListener,
    sidecar_port: u16,
    resource_dir: Option<std::path::PathBuf>,
) {
    let overrides = route_arm_overrides();
    tauri::async_runtime::spawn(async move {
        let mut app_state = eo_http::AppState::new(
            format!("127.0.0.1:{sidecar_port}"),
            public_backend_port(),
            overrides,
        )
        // The substrate answers the browser surface (preflights, origin
        // rules, response decoration) exactly as the sidecar's own
        // middleware would, from the same environment inputs.
        .with_cors(eo_http::cors::CorsConfig::from_env());
        if let Some(composed) = composition::compose_native(resource_dir).await {
            // Clone the live tracker out of the producer spine BEFORE it
            // moves into the Tauri-managed holder below, so the producer
            // routes serve over the same `Arc<HuntTracker>` the exit-seam
            // teardown stops.
            app_state = app_state
                .with_hydration(composed.hydration)
                .with_tracker(composed.producers.tracker_handle());
            // Hand the producer spine to the exit seam so it stops the
            // tail thread and ends any session on app close.
            app.manage(Producers(Mutex::new(Some(composed.producers))));
            // Hold the warmed OCR engine for the app's lifetime so the
            // scan consumer routes can pull it when they flip; no exit
            // stop (the session drops with the managed state, the ORT env
            // self-releases at process exit).
            app.manage(OcrEngineState(Mutex::new(composed.ocr_engine)));
        }
        let state = std::sync::Arc::new(app_state);
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("[substrate] listener handoff failed: {err}");
                return;
            }
        };
        if let Err(err) = eo_http::serve(listener, state).await {
            eprintln!("[substrate] server exited: {err}");
        }
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

// In debug builds the call site above is compiled out (dev uses a separately
// launched backend), leaving this function without a caller; silence the
// resulting dead-code lint rather than drop the release-only definition.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn spawn_backend_sidecar(app: &tauri::AppHandle, relocated_port: Option<u16>) {
    let sidecar = match app.shell().sidecar("entropiaorme-backend") {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("[backend] sidecar resolve failed: {err}");
            return;
        }
    };
    // Relocation: the sidecar reads its bind port (and derives its own
    // Host-header guard) from this variable; the substrate's proxy arm
    // rewrites Host to the private authority accordingly.
    let sidecar = match relocated_port {
        Some(port) => sidecar.env("ENTROPIAORME_BACKEND_PORT", port.to_string()),
        None => sidecar,
    };

    let (mut rx, child) = match sidecar.spawn() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("[backend] sidecar spawn failed: {err}");
            return;
        }
    };

    app.manage(SidecarChild(Mutex::new(Some(child))));

    // Drain stdout/stderr — Windows pipe buffers are ~4 KB and an unread
    // pipe stalls the child's logging once it fills.
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) | CommandEvent::Stderr(line) => {
                    if let Ok(s) = std::str::from_utf8(&line) {
                        eprint!("[backend] {s}");
                    }
                }
                _ => {}
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
