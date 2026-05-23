use std::process::Command as StdCommand;
use std::sync::Mutex;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

#[tauri::command]
fn toggle_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
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
            #[cfg(windows)]
            install_runtime_window_icons(app.handle());
            // Only the bundled release shell spawns the PyInstaller sidecar.
            // Dev builds talk to a separately launched backend, and the dev
            // sidecar slot holds a placeholder binary that Windows rejects
            // (os error 193); gating the spawn to release keeps that error
            // out of the dev console.
            #[cfg(not(debug_assertions))]
            spawn_backend_sidecar(app.handle());
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

// In debug builds the call site above is compiled out (dev uses a separately
// launched backend), leaving this function without a caller; silence the
// resulting dead-code lint rather than drop the release-only definition.
#[cfg_attr(debug_assertions, allow(dead_code))]
fn spawn_backend_sidecar(app: &tauri::AppHandle) {
    let sidecar = match app.shell().sidecar("entropiaorme-backend") {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("[backend] sidecar resolve failed: {err}");
            return;
        }
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
}
