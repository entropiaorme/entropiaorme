//! Locate and measure the Entropia Universe game window.
//!
//! Helpers used by the manual scan flow to derive capture regions from
//! the live game window rather than a fixed-resolution preset table,
//! and the focus check the hotbar listener consults before a slot key
//! may move the tracked tool.
//! On non-Windows platforms the helpers return None and callers handle
//! the missing-window case, exactly as the original does. The capture
//! regions compose these lookups with the pure geometry in
//! `scan_presets`.

use crate::scan_presets::{compute_region, ScanPresets};

pub const GAME_TITLE_PREFIX: &str = "Entropia Universe Client";

/// An opaque window handle (the platform window id on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle(pub isize);

/// The visible game window, or None when it cannot be found (always
/// None off Windows).
pub fn find_game_window() -> Option<WindowHandle> {
    platform::find_game_window()
}

/// The window's client area as (x, y, width, height) in screen
/// coordinates, or None for a degenerate or unmeasurable window.
pub fn get_window_geometry(handle: WindowHandle) -> Option<(i64, i64, i64, i64)> {
    platform::get_window_geometry(handle)
}

/// Whether the game client window is currently locatable.
pub fn game_window_present() -> bool {
    find_game_window().is_some()
}

/// Where keyboard focus sits relative to the game client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameFocus {
    /// The game client window holds focus.
    Focused,
    /// The game client is running but another window (or none) holds
    /// focus.
    Unfocused,
    /// The platform cannot tell: no display connection, a window
    /// manager that does not publish the active window, or a game
    /// window this lookup cannot see (a native-Wayland client, say).
    Unknown,
}

/// Where keyboard focus sits relative to the game client. `Unfocused`
/// is only reported when the game window is itself visible to the
/// lookup, so a game this platform layer cannot see reads as `Unknown`
/// rather than as permanently unfocused.
pub fn game_focus() -> GameFocus {
    platform::game_focus()
}

/// The pointer's current position in screen coordinates, or None where
/// the platform cannot answer (headless, or a Wayland session without
/// the X11 bridge). Consumed by the coordinate-capture calibration flow
/// (the cursor position at the moment of a keypress defines a capture
/// corner).
pub fn cursor_position() -> Option<(i64, i64)> {
    platform::cursor_position()
}

fn live_region(anchor: &crate::scan_presets::PanelAnchor) -> Option<([i64; 2], [i64; 2])> {
    let handle = find_game_window()?;
    let geometry = get_window_geometry(handle)?;
    compute_region(anchor, geometry)
}

/// The skill panel capture rect, or None when the game window is
/// absent.
pub fn skill_region(presets: &ScanPresets) -> Option<([i64; 2], [i64; 2])> {
    live_region(&presets.skill)
}

/// The profession panel capture rect, or None when the game window is
/// absent.
pub fn profession_region(presets: &ScanPresets) -> Option<([i64; 2], [i64; 2])> {
    live_region(&presets.profession)
}

/// The repair-cost number capture rect, or None when the game window
/// is absent.
pub fn repair_region(presets: &ScanPresets) -> Option<([i64; 2], [i64; 2])> {
    live_region(&presets.repair)
}

/// The Trade Terminal total-value rectangle. Until the first live
/// calibration this resolves the explicit provisional fallback.
pub fn trade_terminal_region(presets: &ScanPresets) -> Option<([i64; 2], [i64; 2])> {
    live_region(&presets.trade_terminal)
}

/// The auction sale window's capture rect, or None when the game
/// window is absent or the panel has not been calibrated (its
/// uncalibrated anchor is degenerate, which `compute_region` refuses).
pub fn sale_window_region(presets: &ScanPresets) -> Option<([i64; 2], [i64; 2])> {
    live_region(&presets.sale_window)
}

#[cfg(windows)]
mod platform {
    use super::{GameFocus, WindowHandle, GAME_TITLE_PREFIX};

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// The window's title, or None for an untitled window or one this
    /// process owns. Reading an in-process window's title sends it a
    /// message and waits on its owning (UI) thread; the focus probe runs
    /// on the keystroke dispatch worker, which the UI thread joins at
    /// shutdown, so that wait could deadlock. The game is never
    /// in-process, so skipping our own windows loses nothing.
    fn window_title(hwnd: HWND) -> Option<String> {
        unsafe {
            let mut owner = 0u32;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut owner as *mut u32));
            if owner == std::process::id() {
                return None;
            }
            let length = GetWindowTextLengthW(hwnd);
            if length == 0 {
                return None;
            }
            let mut buffer = vec![0u16; (length + 1) as usize];
            let copied = GetWindowTextW(hwnd, &mut buffer);
            Some(String::from_utf16_lossy(&buffer[..copied as usize]))
        }
    }

    unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let result = &mut *(lparam.0 as *mut Option<WindowHandle>);
        let Some(title) = window_title(hwnd) else {
            return BOOL(1);
        };
        if title.starts_with(GAME_TITLE_PREFIX) && IsWindowVisible(hwnd).as_bool() {
            *result = Some(WindowHandle(hwnd.0 as isize));
            return BOOL(0);
        }
        BOOL(1)
    }

    pub fn game_focus() -> GameFocus {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
            // No foreground window: activation is mid-transition, so the
            // answer is unknowable rather than "not the game".
            return GameFocus::Unknown;
        }
        if window_title(foreground).is_some_and(|title| title.starts_with(GAME_TITLE_PREFIX)) {
            return GameFocus::Focused;
        }
        if find_game_window().is_some() {
            GameFocus::Unfocused
        } else {
            GameFocus::Unknown
        }
    }

    pub fn find_game_window() -> Option<WindowHandle> {
        let mut result: Option<WindowHandle> = None;
        unsafe {
            // EnumWindows reports failure when the callback halts the
            // enumeration early, which is the found case.
            let _ = EnumWindows(Some(enum_callback), LPARAM(&mut result as *mut _ as isize));
        }
        result
    }

    pub fn get_window_geometry(handle: WindowHandle) -> Option<(i64, i64, i64, i64)> {
        let hwnd = HWND(handle.0 as *mut core::ffi::c_void);
        let mut rect = RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut rect).ok()?;
        }
        let width = i64::from(rect.right - rect.left);
        let height = i64::from(rect.bottom - rect.top);
        if width <= 0 || height <= 0 {
            return None;
        }
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            // A failed conversion (the window vanished between calls)
            // must not return plausible geometry at the wrong origin.
            if !ClientToScreen(hwnd, &mut point).as_bool() {
                return None;
            }
        }
        Some((i64::from(point.x), i64::from(point.y), width, height))
    }

    pub fn cursor_position() -> Option<(i64, i64)> {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point).ok()?;
        }
        Some((i64::from(point.x), i64::from(point.y)))
    }
}

/// The Linux lookup runs over X11: the game client runs under Proton,
/// which renders through XWayland, and the app itself forces the X11
/// GDK backend, so both live on the X server where windows are
/// enumerable and measurable (native-Wayland surfaces are deliberately
/// invisible to clients). Discovery walks the window manager's
/// `_NET_CLIENT_LIST`, matches the title prefix, and geometry
/// translates the client origin to root coordinates: the same
/// title-then-client-area contract as the Windows helpers. The scan
/// lookups open their own short-lived connection (they are per-scan,
/// not hot-path); the focus query answers per hotbar press, so it keeps
/// one connection for the process and reopens it after an error. Any X
/// error folds to None, matching the window-vanished handling on
/// Windows.
///
/// Focus reads the window manager's `_NET_ACTIVE_WINDOW`. Under a
/// Wayland compositor the XWayland window manager keeps it current for
/// X11 clients and points it away from them (at None, or at a
/// compositor-owned placeholder) when a native-Wayland client takes
/// focus, so "is the active window the game" stays answerable even
/// though the Wayland client itself is invisible here.
#[cfg(target_os = "linux")]
mod platform {
    use super::{GameFocus, WindowHandle, GAME_TITLE_PREFIX};

    use std::sync::{Mutex, PoisonError};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, MapState, Window};
    use x11rb::rust_connection::RustConnection;

    struct Atoms {
        net_client_list: Atom,
        net_active_window: Atom,
        net_wm_name: Atom,
        utf8_string: Atom,
    }

    impl Atoms {
        fn intern(conn: &impl Connection) -> Option<Self> {
            let intern = |name: &[u8]| -> Option<Atom> {
                Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
            };
            Some(Self {
                net_client_list: intern(b"_NET_CLIENT_LIST")?,
                net_active_window: intern(b"_NET_ACTIVE_WINDOW")?,
                net_wm_name: intern(b"_NET_WM_NAME")?,
                utf8_string: intern(b"UTF8_STRING")?,
            })
        }
    }

    fn window_title(
        conn: &impl Connection,
        window: Window,
        net_wm_name: Atom,
        utf8_string: Atom,
    ) -> Option<String> {
        let reply = conn
            .get_property(false, window, net_wm_name, utf8_string, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if reply.value_len > 0 {
            return String::from_utf8(reply.value).ok();
        }
        // Pre-EWMH fallback (Wine windows occasionally carry only WM_NAME).
        let reply = conn
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        (reply.value_len > 0).then(|| String::from_utf8_lossy(&reply.value).into_owned())
    }

    fn is_game_title(conn: &impl Connection, window: Window, atoms: &Atoms) -> bool {
        window_title(conn, window, atoms.net_wm_name, atoms.utf8_string)
            .is_some_and(|title| title.starts_with(GAME_TITLE_PREFIX))
    }

    fn find_game_window_on(conn: &impl Connection, root: Window, atoms: &Atoms) -> Option<Window> {
        let clients = conn
            .get_property(
                false,
                root,
                atoms.net_client_list,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )
            .ok()?
            .reply()
            .ok()?;
        for window in clients.value32()? {
            if !is_game_title(conn, window, atoms) {
                continue;
            }
            let viewable = conn
                .get_window_attributes(window)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|attributes| attributes.map_state == MapState::VIEWABLE);
            if viewable {
                return Some(window);
            }
        }
        None
    }

    pub fn find_game_window() -> Option<WindowHandle> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::intern(&conn)?;
        find_game_window_on(&conn, root, &atoms).map(|window| WindowHandle(window as isize))
    }

    struct FocusConnection {
        conn: RustConnection,
        root: Window,
        atoms: Atoms,
    }

    impl FocusConnection {
        fn open() -> Option<Self> {
            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots[screen_num].root;
            let atoms = Atoms::intern(&conn)?;
            Some(Self { conn, root, atoms })
        }

        /// None when the connection itself failed, so the caller reopens it.
        fn query(&self) -> Option<GameFocus> {
            let reply = self
                .conn
                .get_property(
                    false,
                    self.root,
                    self.atoms.net_active_window,
                    AtomEnum::WINDOW,
                    0,
                    1,
                )
                .ok()?
                .reply()
                .ok()?;
            let Some(active) = reply.value32().and_then(|mut windows| windows.next()) else {
                // The window manager does not publish the active window.
                return Some(GameFocus::Unknown);
            };
            if active != x11rb::NONE && is_game_title(&self.conn, active, &self.atoms) {
                return Some(GameFocus::Focused);
            }
            Some(
                if find_game_window_on(&self.conn, self.root, &self.atoms).is_some() {
                    GameFocus::Unfocused
                } else {
                    GameFocus::Unknown
                },
            )
        }
    }

    static FOCUS_CONNECTION: Mutex<Option<FocusConnection>> = Mutex::new(None);

    pub fn game_focus() -> GameFocus {
        let mut slot = FOCUS_CONNECTION
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = FocusConnection::open();
        }
        let Some(connection) = slot.as_ref() else {
            return GameFocus::Unknown;
        };
        match connection.query() {
            Some(focus) => focus,
            None => {
                *slot = None;
                GameFocus::Unknown
            }
        }
    }

    pub fn get_window_geometry(handle: WindowHandle) -> Option<(i64, i64, i64, i64)> {
        let window = Window::try_from(u32::try_from(handle.0).ok()?).ok()?;
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen_num].root;
        let geometry = conn.get_geometry(window).ok()?.reply().ok()?;
        let width = i64::from(geometry.width);
        let height = i64::from(geometry.height);
        if width <= 0 || height <= 0 {
            return None;
        }
        // The EWMH client list yields the client window (the WM frame is
        // its parent), so translating its origin to root coordinates is
        // the client area's screen position: the ClientToScreen twin.
        let origin = conn
            .translate_coordinates(window, root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((
            i64::from(origin.dst_x),
            i64::from(origin.dst_y),
            width,
            height,
        ))
    }

    pub fn cursor_position() -> Option<(i64, i64)> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen_num].root;
        let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
        Some((i64::from(pointer.root_x), i64::from(pointer.root_y)))
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::{GameFocus, WindowHandle};

    pub fn find_game_window() -> Option<WindowHandle> {
        None
    }

    pub fn game_focus() -> GameFocus {
        GameFocus::Unknown
    }

    pub fn get_window_geometry(_handle: WindowHandle) -> Option<(i64, i64, i64, i64)> {
        None
    }

    pub fn cursor_position() -> Option<(i64, i64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_windows_yield_none_regions() {
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            assert!(find_game_window().is_none());
            assert!(!game_window_present());
            assert_eq!(game_focus(), GameFocus::Unknown);
            assert!(get_window_geometry(WindowHandle(1)).is_none());
            let presets = ScanPresets::new(std::path::Path::new("/nonexistent.json"));
            assert!(skill_region(&presets).is_none());
            assert!(profession_region(&presets).is_none());
            assert!(repair_region(&presets).is_none());
            assert!(sale_window_region(&presets).is_none());
        }
        #[cfg(target_os = "linux")]
        {
            // Live-session lookups read ambient window state (another
            // process could legitimately carry the title), so a unit
            // test may only assert the environment-independent paths:
            // a handle that cannot be an X window id resolves to None
            // deterministically, on a live session and headless alike.
            assert!(get_window_geometry(WindowHandle(-1)).is_none());
        }
        #[cfg(windows)]
        {
            // Headless CI has no game client; the lookups must simply
            // not find one rather than fail.
            let _ = game_window_present();
        }
    }
}
