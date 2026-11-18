//! Locate and measure the Entropia Universe game window.
//!
//! The capture of the whole game window for the readers that find what
//! they want in it themselves, and the focus check the hotbar listener
//! consults before a slot key may move the tracked tool. Where the
//! platform cannot find the window the helpers return None and callers
//! handle the missing-window case.

use crate::game_ui::seams::{FrameArea, FrameUnavailable, GameFrame};
use crate::screen_capture::{
    capturable_area, capturable_area_running, capture_region_bgr, capture_region_bgr_running,
    capture_running, start_capture,
};

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

/// The game window's client area captured as one BGR frame, the
/// production [`GameWindowFrame`].
///
/// Only the part of the client area the capturer can see is captured:
/// a window hanging off the screen, at a negative origin, or across the
/// edge of the monitor shared for capture is still read from what is
/// visible of it. A window that cannot be measured (it vanished between
/// the lookup and the measurement, or has no area) counts as not found.
///
/// [`GameWindowFrame`]: crate::game_ui::seams::GameWindowFrame
pub fn capture_game_window(area: FrameArea) -> Result<GameFrame, FrameUnavailable> {
    capture_window(area, true)
}

/// [`capture_game_window`] without ever starting the capturer: where it
/// must be started (the ScreenCast portal) and is not running, the answer
/// is [`FrameUnavailable::CaptureNotStarted`]. What a reader looking on its
/// own schedule captures with, so it never asks the desktop for consent
/// nor reopens a stream the player stopped.
pub fn capture_game_window_running(area: FrameArea) -> Result<GameFrame, FrameUnavailable> {
    capture_window(area, false)
}

fn capture_window(area: FrameArea, may_start: bool) -> Result<GameFrame, FrameUnavailable> {
    let handle = find_game_window().ok_or(FrameUnavailable::WindowNotFound)?;
    let window = get_window_geometry(handle).ok_or(FrameUnavailable::WindowNotFound)?;
    // A part of the window is captured as far as it lies inside it.
    let client = match area {
        FrameArea::Whole => window,
        FrameArea::Part(part) => capturable_part(
            (
                window.0 + part.x0 as i64,
                window.1 + part.y0 as i64,
                part.width() as i64,
                part.height() as i64,
            ),
            window,
        )
        .ok_or(FrameUnavailable::OutsideCapturableArea)?,
    };
    // Asked once, before the area and the pixels, each of which would
    // otherwise try a failed start again and ask for consent twice.
    let started = if may_start {
        start_capture()
    } else {
        capture_running()
    };
    if !started {
        return Err(FrameUnavailable::CaptureNotStarted);
    }
    let limit = if may_start {
        capturable_area()
    } else {
        capturable_area_running()
    };
    let (x, y, w, h) = match limit {
        Some(area) => {
            capturable_part(client, area).ok_or(FrameUnavailable::OutsideCapturableArea)?
        }
        None => client,
    };
    // Timed from here: finding the window and asking what can be captured
    // are over, so what follows is the grab of the pixels alone.
    let taken = std::time::Instant::now();
    let pixels = if may_start {
        capture_region_bgr(x, y, w, h)
    } else {
        capture_region_bgr_running(x, y, w, h)
    };
    pixels
        .map(|image| GameFrame {
            image,
            origin: (x, y),
            offset: (x - window.0, y - window.1),
            age: taken.elapsed(),
        })
        .ok_or(FrameUnavailable::CaptureFailed)
}

/// The part of `window` that lies inside `area`, both given as
/// `(x, y, width, height)`; `None` when they do not overlap.
pub fn capturable_part(
    window: (i64, i64, i64, i64),
    area: (i64, i64, i64, i64),
) -> Option<(i64, i64, i64, i64)> {
    let x0 = window.0.max(area.0);
    let y0 = window.1.max(area.1);
    let x1 = (window.0 + window.2).min(area.0 + area.2);
    let y1 = (window.1 + window.3).min(area.1 + area.3);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
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
    /// The game client is running but another application's window (or
    /// none) holds focus.
    Unfocused,
    /// One of this app's own windows holds focus: the route overlay, the
    /// radar overlay, a control the player clicked. The game is still on
    /// the screen behind it.
    OwnWindow,
    /// The platform cannot tell: no display connection, a window
    /// manager that does not publish the active window, a window whose
    /// owner cannot be told, or a game window this lookup cannot see (a
    /// native-Wayland client, say).
    Unknown,
}

impl GameFocus {
    /// Whether a key pressed now may be one typed into the game: not when
    /// another window, the app's own included, definitely holds focus. A
    /// platform that cannot tell admits it.
    pub fn keys_reach_game(self) -> bool {
        matches!(self, GameFocus::Focused | GameFocus::Unknown)
    }

    /// Whether the game may be read off the screen by a read the app makes
    /// on its own: not while another application definitely holds focus.
    /// One of the app's own windows in front leaves the game on the screen
    /// behind it, and a platform that cannot tell reads.
    pub fn game_in_view(self) -> bool {
        self != GameFocus::Unfocused
    }
}

/// What the platform could tell about the window holding focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Foreground {
    /// Nothing can be told: no window is reported, or activation is
    /// changing hands.
    Untold,
    /// The game's own window.
    Game,
    /// A window of this process.
    Ours,
    /// A window of another process, or no window at all.
    Other,
    /// A window whose owning process cannot be told.
    OwnerUnknown,
}

/// The focus state from what the platform told of the foreground window;
/// `game_present` is asked only when another application's window is in
/// front, so that a game this lookup cannot see is never reported as
/// unfocused.
fn focus_of(foreground: Foreground, game_present: impl FnOnce() -> bool) -> GameFocus {
    match foreground {
        Foreground::Untold | Foreground::OwnerUnknown => GameFocus::Unknown,
        Foreground::Game => GameFocus::Focused,
        Foreground::Ours => GameFocus::OwnWindow,
        Foreground::Other if game_present() => GameFocus::Unfocused,
        Foreground::Other => GameFocus::Unknown,
    }
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

#[cfg(windows)]
mod platform {
    use super::{focus_of, Foreground, GameFocus, WindowHandle, GAME_TITLE_PREFIX};

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
        let told = if foreground.0.is_null() {
            // No foreground window: activation is mid-transition, so the
            // answer is unknowable rather than "not the game".
            Foreground::Untold
        } else {
            let mut owner = 0u32;
            let thread =
                unsafe { GetWindowThreadProcessId(foreground, Some(&mut owner as *mut u32)) };
            if thread == 0 {
                Foreground::OwnerUnknown
            } else if owner == std::process::id() {
                Foreground::Ours
            } else if window_title(foreground)
                .is_some_and(|title| title.starts_with(GAME_TITLE_PREFIX))
            {
                Foreground::Game
            } else {
                Foreground::Other
            }
        };
        focus_of(told, || find_game_window().is_some())
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
    use super::{focus_of, Foreground, GameFocus, WindowHandle, GAME_TITLE_PREFIX};

    use std::sync::{Mutex, PoisonError};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, MapState, Window};
    use x11rb::rust_connection::RustConnection;

    struct Atoms {
        net_client_list: Atom,
        net_active_window: Atom,
        net_wm_name: Atom,
        net_wm_pid: Atom,
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
                net_wm_pid: intern(b"_NET_WM_PID")?,
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
            let told = match reply.value32().and_then(|mut windows| windows.next()) {
                // The window manager does not publish the active window.
                None => Foreground::Untold,
                // No window holds focus: none of ours does either.
                Some(x11rb::NONE) => Foreground::Other,
                Some(active) if is_game_title(&self.conn, active, &self.atoms) => Foreground::Game,
                Some(active) => match self.owner(active) {
                    Some(pid) if pid == std::process::id() => Foreground::Ours,
                    Some(_) => Foreground::Other,
                    None => Foreground::OwnerUnknown,
                },
            };
            Some(focus_of(told, || {
                find_game_window_on(&self.conn, self.root, &self.atoms).is_some()
            }))
        }

        /// The process that owns `window`, as its `_NET_WM_PID` says; None
        /// when the window does not say.
        fn owner(&self, window: Window) -> Option<u32> {
            self.conn
                .get_property(
                    false,
                    window,
                    self.atoms.net_wm_pid,
                    AtomEnum::CARDINAL,
                    0,
                    1,
                )
                .ok()?
                .reply()
                .ok()?
                .value32()?
                .next()
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
    fn the_capturable_part_of_a_window_is_its_overlap_with_the_area() {
        let screen = (0, 0, 1920, 1080);
        // Wholly inside: the whole window.
        assert_eq!(
            capturable_part((100, 50, 800, 600), screen),
            Some((100, 50, 800, 600))
        );
        // A negative origin: the part from the screen's corner.
        assert_eq!(
            capturable_part((-200, -30, 800, 600), screen),
            Some((0, 0, 600, 570))
        );
        // Overhanging each side in turn.
        assert_eq!(
            capturable_part((1500, 100, 800, 600), screen),
            Some((1500, 100, 420, 600))
        );
        assert_eq!(
            capturable_part((100, 700, 800, 600), screen),
            Some((100, 700, 800, 380))
        );
        assert_eq!(
            capturable_part((-50, 100, 800, 600), screen),
            Some((0, 100, 750, 600))
        );
        assert_eq!(
            capturable_part((100, -100, 800, 600), screen),
            Some((100, 0, 800, 500))
        );
        // A monitor that does not start at the origin.
        let right_monitor = (1920, 0, 2560, 1440);
        assert_eq!(
            capturable_part((1800, 100, 800, 600), right_monitor),
            Some((1920, 100, 680, 600))
        );
        // No overlap, or only an edge: nothing to capture.
        assert_eq!(capturable_part((100, 100, 800, 600), right_monitor), None);
        assert_eq!(capturable_part((1120, 0, 800, 600), right_monitor), None);
        assert_eq!(capturable_part((0, 1080, 800, 600), screen), None);
    }

    #[test]
    fn focus_is_told_from_whose_window_is_in_front() {
        let present = || true;
        assert_eq!(focus_of(Foreground::Game, present), GameFocus::Focused);
        assert_eq!(focus_of(Foreground::Ours, present), GameFocus::OwnWindow);
        assert_eq!(focus_of(Foreground::Other, present), GameFocus::Unfocused);
        // A game this lookup cannot see is never unfocused.
        assert_eq!(focus_of(Foreground::Other, || false), GameFocus::Unknown);
        // Whose window it is, or which window, cannot be told.
        assert_eq!(
            focus_of(Foreground::OwnerUnknown, present),
            GameFocus::Unknown
        );
        assert_eq!(focus_of(Foreground::Untold, present), GameFocus::Unknown);
        // The app's own window in front: the game stays on the screen for
        // the reads the app makes on its own, but a key typed now is not
        // typed into the game.
        let all = [
            GameFocus::Focused,
            GameFocus::Unfocused,
            GameFocus::OwnWindow,
            GameFocus::Unknown,
        ];
        let in_view: Vec<bool> = all.iter().map(|f| f.game_in_view()).collect();
        let keys: Vec<bool> = all.iter().map(|f| f.keys_reach_game()).collect();
        assert_eq!(in_view, [true, false, true, true]);
        assert_eq!(keys, [true, false, false, true]);
    }

    #[test]
    fn absent_windows_yield_none() {
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            assert!(find_game_window().is_none());
            assert!(!game_window_present());
            assert_eq!(game_focus(), GameFocus::Unknown);
            assert!(get_window_geometry(WindowHandle(1)).is_none());
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
