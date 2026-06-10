//! Locate and measure the Entropia Universe game window, ported from
//! `backend/services/eu_window.py`.
//!
//! Helpers used by the manual scan flow to derive capture regions from
//! the live game window rather than a fixed-resolution preset table.
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

#[cfg(windows)]
mod platform {
    use super::{WindowHandle, GAME_TITLE_PREFIX};

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let result = &mut *(lparam.0 as *mut Option<WindowHandle>);
        let length = GetWindowTextLengthW(hwnd);
        if length == 0 {
            return BOOL(1);
        }
        let mut buffer = vec![0u16; (length + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        let title = String::from_utf16_lossy(&buffer[..copied as usize]);
        if title.starts_with(GAME_TITLE_PREFIX) && IsWindowVisible(hwnd).as_bool() {
            *result = Some(WindowHandle(hwnd.0 as isize));
            return BOOL(0);
        }
        BOOL(1)
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
}

#[cfg(not(windows))]
mod platform {
    use super::WindowHandle;

    pub fn find_game_window() -> Option<WindowHandle> {
        None
    }

    pub fn get_window_geometry(_handle: WindowHandle) -> Option<(i64, i64, i64, i64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_windows_yield_none_regions() {
        #[cfg(not(windows))]
        {
            assert!(find_game_window().is_none());
            assert!(!game_window_present());
            assert!(get_window_geometry(WindowHandle(1)).is_none());
            let presets = ScanPresets::new(std::path::Path::new("/nonexistent.json"));
            assert!(skill_region(&presets).is_none());
            assert!(profession_region(&presets).is_none());
            assert!(repair_region(&presets).is_none());
        }
        #[cfg(windows)]
        {
            // Headless CI has no game client; the lookups must simply
            // not find one rather than fail.
            let _ = game_window_present();
        }
    }
}
