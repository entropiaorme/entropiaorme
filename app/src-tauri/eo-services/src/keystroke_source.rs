//! Keystroke source abstraction.
//!
//! Listeners consume a `KeystrokeSource` rather than touching the OS
//! hook themselves: production wires the Windows low-level keyboard
//! hook or, on Linux, the XInput2 raw-key observer (with an evdev
//! fallback), tests inject through the mock. The input-listening
//! minimisation policy is enforced structurally: a constructor-passed
//! allowlist filters at the hook boundary so out-of-scope keystrokes
//! never enter the application's event stream. The hook callback does
//! no work beyond filtering and enqueueing: one owned worker drains
//! the queue and dispatches to subscribers.

use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeystrokeKind {
    Press,
    Release,
}

impl KeystrokeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            KeystrokeKind::Press => "press",
            KeystrokeKind::Release => "release",
        }
    }
}

/// One observed keystroke: a human-readable key identifier in the
/// listeners' vocabulary ("1", "0", "space"), when it occurred, and
/// the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeystrokeEvent {
    pub key: String,
    pub timestamp: DateTime<Utc>,
    pub kind: KeystrokeKind,
}

pub type KeystrokeCallback = Arc<dyn Fn(&KeystrokeEvent) + Send + Sync>;

/// Abstract source of keystroke events: subscribers register a
/// callback, `start` begins delivery (returning whether the underlying
/// mechanism actually attached), `stop` halts it with subscribers
/// remaining registered.
pub trait KeystrokeSource: Send + Sync {
    fn subscribe(&self, callback: KeystrokeCallback);
    fn start(&self) -> bool;
    fn stop(&self);
}

/// Test-mode source: dispatches injected events to subscribers; events
/// injected while halted are silently dropped, matching the
/// "events only flow while running" contract.
#[derive(Default)]
pub struct MockKeystrokeSource {
    callbacks: Mutex<Vec<KeystrokeCallback>>,
    running: Mutex<bool>,
}

impl MockKeystrokeSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dispatch a synthetic keystroke to all subscribers in
    /// registration order; a no-op while halted.
    pub fn inject(&self, key: &str, timestamp: DateTime<Utc>, kind: KeystrokeKind) {
        if !*self.running.lock().expect("mock running flag") {
            return;
        }
        let event = KeystrokeEvent {
            key: key.to_string(),
            timestamp,
            kind,
        };
        let callbacks: Vec<KeystrokeCallback> =
            self.callbacks.lock().expect("mock callbacks").clone();
        for callback in callbacks {
            callback(&event);
        }
    }
}

impl KeystrokeSource for MockKeystrokeSource {
    fn subscribe(&self, callback: KeystrokeCallback) {
        self.callbacks
            .lock()
            .expect("mock callbacks")
            .push(callback);
    }

    fn start(&self) -> bool {
        *self.running.lock().expect("mock running flag") = true;
        true
    }

    fn stop(&self) {
        *self.running.lock().expect("mock running flag") = false;
    }
}

#[cfg(target_os = "linux")]
use linux_input as platform_hook;
/// The platform module behind [`HookKeystrokeSource`]: the Windows
/// low-level keyboard hook or the Linux observer selection, each exposing
/// the same `start(callbacks, allowlist) -> Option<Running>` surface.
#[cfg(windows)]
use windows_hook as platform_hook;

/// The production source: the platform's global key observer behind the
/// same trait (the Windows low-level keyboard hook; on Linux the XInput2
/// raw-key observer, or the passive evdev reader when no X server
/// answers). On platforms without an implementation `start` stays
/// inert and returns false, exactly as the original does when its hook
/// library is unavailable.
pub struct HookKeystrokeSource {
    // Consumed by the platform hook module; the portable build keeps
    // the field so construction is uniform across platforms.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    allowlist: Option<BTreeSet<String>>,
    callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
    #[cfg(any(windows, target_os = "linux"))]
    state: Mutex<Option<platform_hook::Running>>,
}

impl HookKeystrokeSource {
    /// `allowlist = None` admits every key the vocabulary can name.
    pub fn new(allowlist: Option<BTreeSet<String>>) -> Self {
        Self {
            allowlist,
            callbacks: Arc::new(Mutex::new(Vec::new())),
            #[cfg(any(windows, target_os = "linux"))]
            state: Mutex::new(None),
        }
    }

    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    fn dispatch(
        callbacks: &Mutex<Vec<KeystrokeCallback>>,
        allowlist: &Option<BTreeSet<String>>,
        key: &str,
        kind: KeystrokeKind,
    ) {
        if let Some(allowlist) = allowlist {
            if !allowlist.contains(key) {
                return;
            }
        }
        let event = KeystrokeEvent {
            key: key.to_string(),
            timestamp: Utc::now(),
            kind,
        };
        let snapshot: Vec<KeystrokeCallback> = callbacks.lock().expect("callbacks").clone();
        for callback in snapshot {
            let _ = catch_unwind(AssertUnwindSafe(|| callback(&event)));
        }
    }
}

impl KeystrokeSource for HookKeystrokeSource {
    fn subscribe(&self, callback: KeystrokeCallback) {
        self.callbacks.lock().expect("callbacks").push(callback);
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn start(&self) -> bool {
        let mut state = self.state.lock().expect("hook state");
        if state.is_some() {
            return true;
        }
        match platform_hook::start(self.callbacks.clone(), self.allowlist.clone()) {
            Some(running) => {
                *state = Some(running);
                true
            }
            None => false,
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn start(&self) -> bool {
        false
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn stop(&self) {
        if let Some(running) = self.state.lock().expect("hook state").take() {
            running.stop();
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn stop(&self) {}
}

/// A reference-counted wrapper over a single [`KeystrokeSource`], so several
/// listeners can share one underlying OS hook. The Windows low-level keyboard
/// hook is single-instance ([`HookKeystrokeSource`] refuses a second
/// concurrent hook through its process-global slot), so two listeners that
/// each owned their own hook would have one stand inert; sharing one source
/// behind this wrapper lets both subscribe and drive `start`/`stop`
/// independently. The inner source attaches on the first `start` and detaches
/// only on the last `stop`, so a listener that wants events keeps the hook
/// live for the others while each still gates on its own running flag. Off the
/// supported platform (or on an attach failure) the inner `start` returns
/// false and the count stays at zero, so a later `start` retries rather than
/// latching a dead source.
pub struct SharedKeystrokeSource {
    inner: Arc<dyn KeystrokeSource>,
    active: Mutex<usize>,
}

impl SharedKeystrokeSource {
    pub fn new(inner: Arc<dyn KeystrokeSource>) -> Self {
        Self {
            inner,
            active: Mutex::new(0),
        }
    }

    fn lock_active(&self) -> std::sync::MutexGuard<'_, usize> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl KeystrokeSource for SharedKeystrokeSource {
    fn subscribe(&self, callback: KeystrokeCallback) {
        self.inner.subscribe(callback);
    }

    fn start(&self) -> bool {
        let mut active = self.lock_active();
        if *active == 0 && !self.inner.start() {
            // The underlying mechanism did not attach; do not count a start
            // that produced no live hook, so the next caller retries.
            return false;
        }
        *active += 1;
        true
    }

    fn stop(&self) {
        let mut active = self.lock_active();
        if *active == 0 {
            return;
        }
        *active -= 1;
        if *active == 0 {
            self.inner.stop();
        }
    }
}

/// The Windows hook plumbing: a dedicated thread installs the
/// low-level keyboard hook and pumps messages; the hook procedure
/// filters by allowlist and hands the worker queue one entry per
/// edge; one owned worker drains it and dispatches to subscribers.
#[cfg(windows)]
mod windows_hook {
    use super::{KeystrokeCallback, KeystrokeKind};
    use std::collections::BTreeSet;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex, OnceLock};

    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
        WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    /// The queue slot the hook procedure reads (it has no user-data
    /// slot of its own). The slot enforces a hard single-instance
    /// contract: `start` refuses while another hook owns it, so two
    /// sources can never clobber each other's routing.
    type ActiveSender = Mutex<Option<Sender<(String, KeystrokeKind)>>>;

    static ACTIVE: OnceLock<ActiveSender> = OnceLock::new();

    fn active() -> &'static ActiveSender {
        ACTIVE.get_or_init(|| Mutex::new(None))
    }

    pub struct Running {
        pump_thread_id: u32,
        pump: Option<std::thread::JoinHandle<()>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    impl Running {
        pub fn stop(self) {
            // Dropping runs the shutdown; the explicit form exists for
            // call-site clarity.
        }

        fn shutdown(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(pump) = self.pump.take() {
                let _ = pump.join();
            }
            // The pump clears the active sender on exit, which ends the
            // worker's queue; clear it again here in case the pump
            // panicked past its own cleanup, so a wedged shutdown can
            // never strand the slot.
            *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// The key vocabulary the listeners speak: number-row digits, the
    /// spacebar, Enter (the coordinate-calibration confirm key), and F (the
    /// game's interact key, which a route can visit trees on).
    /// Unmapped virtual keys return None, matching the original's
    /// unmappable-key handling.
    fn key_name(vk: u32) -> Option<String> {
        match vk {
            0x30..=0x39 => Some(((b'0' + (vk - 0x30) as u8) as char).to_string()),
            0x60..=0x69 => Some(((b'0' + (vk - 0x60) as u8) as char).to_string()),
            0x20 => Some("space".to_string()),
            // VK_RETURN covers both the main Enter and the keypad Enter
            // (the extended-key bit distinguishes them; both confirm).
            0x0D => Some("return".to_string()),
            0x46 => Some("f".to_string()),
            _ => None,
        }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let kind = match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeystrokeKind::Press),
                WM_KEYUP | WM_SYSKEYUP => Some(KeystrokeKind::Release),
                _ => None,
            };
            if let Some(kind) = kind {
                let data = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                if let Some(key) = key_name(data.vkCode) {
                    // Poison-tolerant: the hook procedure sits on an
                    // FFI boundary that must never unwind.
                    let slot = active().lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sender) = slot.as_ref() {
                        let _ = sender.send((key, kind));
                    }
                }
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    pub fn start(
        callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
        allowlist: Option<BTreeSet<String>>,
    ) -> Option<Running> {
        let (sender, receiver) = channel::<(String, KeystrokeKind)>();
        {
            let mut slot = active().lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_some() {
                // Another source owns the hook; refusing keeps the
                // routing unambiguous (the caller reports inert).
                return None;
            }
            *slot = Some(sender);
        }

        let (ready_sender, ready_receiver) = channel::<Option<u32>>();
        let pump = match std::thread::Builder::new()
            .name("keystroke-hook".into())
            .spawn(move || unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0);
                let Ok(hook) = hook else {
                    tracing::warn!(target: "eo::input", "keystroke hook failed to install");
                    let _ = ready_sender.send(None);
                    *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                    return;
                };
                let thread_id = windows::Win32::System::Threading::GetCurrentThreadId();
                tracing::info!(target: "eo::input", thread_id, "keystroke hook installed");
                let _ = ready_sender.send(Some(thread_id));
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).into() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let _ = UnhookWindowsHookEx(hook);
                tracing::info!(target: "eo::input", "keystroke hook removed");
                *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
            }) {
            Ok(pump) => pump,
            Err(_) => {
                // A failed spawn must release the slot or every later
                // start would refuse against a hook that never existed.
                *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                return None;
            }
        };

        let pump_thread_id = match ready_receiver.recv() {
            Ok(Some(thread_id)) => thread_id,
            _ => {
                let _ = pump.join();
                return None;
            }
        };

        let worker = match std::thread::Builder::new()
            .name("keystroke-dispatch".into())
            .spawn(move || {
                // Ends when the pump drops the active sender.
                while let Ok((key, kind)) = receiver.recv() {
                    super::HookKeystrokeSource::dispatch(&callbacks, &allowlist, &key, kind);
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                // Tear the pump down rather than leaving an unhooked
                // stop path: post quit, join, and the pump's own exit
                // clears the slot.
                unsafe {
                    let _ = PostThreadMessageW(pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                let _ = pump.join();
                return None;
            }
        };

        Some(Running {
            pump_thread_id,
            pump: Some(pump),
            worker: Some(worker),
        })
    }
}

/// The Linux observer selection. The game runs under Proton on XWayland,
/// so every key it receives passes through the X server, including keys a
/// remapper injects above the kernel (Steam Input creates no virtual
/// keyboard device; its keys enter at the compositor and reach the X
/// server alone). XInput2 raw key events therefore see exactly what the
/// game sees, where the kernel evdev nodes see only physical keyboards.
/// The evdev reader remains the fallback for a session with no reachable
/// X server, and only for that: once the X connection is up, a later
/// failure leaves the source inert rather than widening it to every
/// kernel keyboard. Only one observer runs at a time: a physical key
/// reaches both layers, so running both would deliver it twice.
#[cfg(target_os = "linux")]
mod linux_input {
    use super::KeystrokeCallback;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    pub enum Running {
        XInput(super::linux_xinput::Running),
        Evdev(super::linux_evdev::Running),
    }

    impl Running {
        pub fn stop(self) {
            match self {
                Running::XInput(running) => running.stop(),
                Running::Evdev(running) => running.stop(),
            }
        }
    }

    pub fn start(
        callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
        allowlist: Option<BTreeSet<String>>,
    ) -> Option<Running> {
        match super::linux_xinput::connect() {
            Some(conn) => {
                super::linux_xinput::start(conn, callbacks, allowlist).map(Running::XInput)
            }
            None => {
                tracing::info!(
                    target: "eo::input",
                    "no X server with XInput 2.1 reachable; falling back to evdev"
                );
                super::linux_evdev::start(callbacks, allowlist).map(Running::Evdev)
            }
        }
    }
}

/// The XInput2 observer: one connection selects raw key press and release
/// events for the master keyboards on the root window (from XInput 2.1 on,
/// raw events reach a root selection whatever the focus or grabs), a
/// reader thread translates them into the shared vocabulary, and one owned
/// worker dispatches. Selecting the master devices only keeps each key to
/// a single event; the slave devices would report it a second time.
#[cfg(target_os = "linux")]
mod linux_xinput {
    use super::{KeystrokeCallback, KeystrokeKind};
    use std::collections::BTreeSet;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use evdev::KeyCode;
    use x11rb::connection::Connection;
    use x11rb::protocol::xinput::{ConnectionExt as _, Device, EventMask, XIEventMask};
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    /// On an evdev-backed X server (XWayland, and Xorg over libinput or
    /// evdev) an X keycode is the kernel key code plus 8. Translating
    /// through it keeps the vocabulary positional and layout-independent,
    /// exactly as the evdev reader and the Windows virtual-key mapping are.
    const EVDEV_KEYCODE_OFFSET: u32 = 8;

    pub struct Running {
        stop: Arc<AtomicBool>,
        reader: Option<std::thread::JoinHandle<()>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    impl Running {
        pub fn stop(self) {
            // Dropping runs the shutdown; the explicit form exists for
            // call-site clarity.
        }

        fn shutdown(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            // The reader dropping its sender ends the worker's queue, so
            // it drains and exits.
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn key_name(keycode: u32) -> Option<&'static str> {
        let code = u16::try_from(keycode.checked_sub(EVDEV_KEYCODE_OFFSET)?).ok()?;
        super::linux_evdev::key_name(KeyCode::new(code))
    }

    fn translate(event: &Event) -> Option<(&'static str, KeystrokeKind)> {
        match event {
            Event::XinputRawKeyPress(raw) => Some((key_name(raw.detail)?, KeystrokeKind::Press)),
            Event::XinputRawKeyRelease(raw) => {
                Some((key_name(raw.detail)?, KeystrokeKind::Release))
            }
            _ => None,
        }
    }

    /// Opens the display, requires XInput 2.1 or later, and selects the raw
    /// key events. Any failure (no display, no extension, an old server)
    /// yields None so the caller falls back to evdev.
    pub(super) fn connect() -> Option<RustConnection> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let version = conn.xinput_xi_query_version(2, 2).ok()?.reply().ok()?;
        if (version.major_version, version.minor_version) < (2, 1) {
            return None;
        }
        let root = conn.setup().roots.get(screen_num)?.root;
        let mask = EventMask {
            deviceid: Device::ALL_MASTER.into(),
            mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE],
        };
        conn.xinput_xi_select_events(root, &[mask])
            .ok()?
            .check()
            .ok()?;
        Some(conn)
    }

    pub fn start(
        conn: RustConnection,
        callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
        allowlist: Option<BTreeSet<String>>,
    ) -> Option<Running> {
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel::<(String, KeystrokeKind)>();

        let stop_flag = stop.clone();
        let reader = std::thread::Builder::new()
            .name("keystroke-xinput".into())
            .spawn(move || {
                tracing::info!(target: "eo::input", "xinput reader attached");
                let mut pollfd = libc::pollfd {
                    fd: conn.stream().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                'read: loop {
                    // Drain everything already buffered before waiting:
                    // poll(2) sees only the socket, not x11rb's queue.
                    loop {
                        match conn.poll_for_event() {
                            Ok(Some(event)) => {
                                if let Some((key, kind)) = translate(&event) {
                                    let _ = sender.send((key.to_string(), kind));
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                // The X server went away (which also
                                // takes the app's own X11 windows with
                                // it). The observer fails closed: it
                                // stays inert until every consumer has
                                // stopped and a fresh start reconnects.
                                tracing::warn!(
                                    target: "eo::input",
                                    %error,
                                    "xinput reader detached"
                                );
                                break 'read;
                            }
                        }
                    }
                    if stop_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    // Timeout or EINTR alike loop back to re-check the
                    // stop flag after draining.
                    unsafe {
                        libc::poll(&mut pollfd, 1, super::linux_evdev::POLL_TIMEOUT_MS);
                    }
                }
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(_) => return None,
        };

        let worker = match std::thread::Builder::new()
            .name("keystroke-dispatch".into())
            .spawn(move || {
                // Ends when the reader drops its sender.
                while let Ok((key, kind)) = receiver.recv() {
                    super::HookKeystrokeSource::dispatch(&callbacks, &allowlist, &key, kind);
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let mut partial = Running {
                    stop,
                    reader: Some(reader),
                    worker: None,
                };
                partial.shutdown();
                return None;
            }
        };

        Some(Running {
            stop,
            reader: Some(reader),
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use x11rb::protocol::xinput::RawKeyPressEvent;

        fn raw(detail: u32) -> RawKeyPressEvent {
            RawKeyPressEvent {
                detail,
                ..Default::default()
            }
        }

        #[test]
        fn x_keycodes_translate_through_the_evdev_offset() {
            // X keycode = kernel code + 8: KEY_1 (2) is 10, KEY_0 (11) is
            // 19, KEY_SPACE (57) is 65, KEY_ENTER (28) is 36, KEY_F (33)
            // is 41, KEY_KP5 (76) is 84.
            assert_eq!(key_name(10), Some("1"));
            assert_eq!(key_name(19), Some("0"));
            assert_eq!(key_name(65), Some("space"));
            assert_eq!(key_name(36), Some("return"));
            assert_eq!(key_name(41), Some("f"));
            assert_eq!(key_name(84), Some("5"));
            // KEY_A (30) stays unmapped; keycodes below the offset and
            // beyond the kernel's code range never alias a mapped key.
            assert_eq!(key_name(38), None);
            assert_eq!(key_name(0), None);
            assert_eq!(key_name(7), None);
            assert_eq!(key_name(u32::MAX), None);
        }

        #[test]
        fn raw_key_events_translate_to_edges_and_others_drop() {
            assert_eq!(
                translate(&Event::XinputRawKeyPress(raw(11))),
                Some(("2", KeystrokeKind::Press))
            );
            assert_eq!(
                translate(&Event::XinputRawKeyRelease(raw(11))),
                Some(("2", KeystrokeKind::Release))
            );
            assert_eq!(translate(&Event::XinputRawKeyPress(raw(38))), None);
            assert_eq!(
                translate(&Event::XinputRawButtonPress(Default::default())),
                None
            );
        }
    }
}

/// The Linux plumbing: passive readers on the kernel evdev nodes. One
/// thread per keyboard-class device polls its file descriptor with a
/// short timeout (so `stop` is honoured promptly) and reads events
/// without ever grabbing the device, so the focused application keeps
/// receiving every key; one owned worker drains the queue and
/// dispatches to subscribers. Requires read access to `/dev/input`
/// (the `input` group); when enumeration finds no readable keyboard
/// the source reports inert, mirroring the Windows hook-install
/// failure path. Unlike the Windows hook there is no process-global
/// single-instance constraint: concurrent readers are kernel-supported,
/// so no slot guard is needed.
#[cfg(target_os = "linux")]
mod linux_evdev {
    use super::{KeystrokeCallback, KeystrokeKind};
    use std::collections::BTreeSet;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use evdev::{Device, EventType, KeyCode};

    /// How long a reader blocks in poll(2) before re-checking the stop
    /// flag: short enough that teardown feels immediate, long enough
    /// that an idle keyboard costs a handful of wakeups per second.
    pub(super) const POLL_TIMEOUT_MS: i32 = 200;

    pub struct Running {
        stop: Arc<AtomicBool>,
        readers: Vec<std::thread::JoinHandle<()>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    impl Running {
        pub fn stop(self) {
            // Dropping runs the shutdown; the explicit form exists for
            // call-site clarity.
        }

        fn shutdown(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            for reader in self.readers.drain(..) {
                let _ = reader.join();
            }
            // The readers dropping their senders ends the worker's
            // queue, so it drains and exits.
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// The key vocabulary the listeners speak, mirroring the Windows
    /// mapping exactly: number-row and keypad digits fold to the same
    /// digit strings, the spacebar is "space", Enter (main and keypad)
    /// is "return", F is "f", everything else is unmapped.
    pub(super) fn key_name(code: KeyCode) -> Option<&'static str> {
        Some(match code {
            KeyCode::KEY_1 | KeyCode::KEY_KP1 => "1",
            KeyCode::KEY_2 | KeyCode::KEY_KP2 => "2",
            KeyCode::KEY_3 | KeyCode::KEY_KP3 => "3",
            KeyCode::KEY_4 | KeyCode::KEY_KP4 => "4",
            KeyCode::KEY_5 | KeyCode::KEY_KP5 => "5",
            KeyCode::KEY_6 | KeyCode::KEY_KP6 => "6",
            KeyCode::KEY_7 | KeyCode::KEY_KP7 => "7",
            KeyCode::KEY_8 | KeyCode::KEY_KP8 => "8",
            KeyCode::KEY_9 | KeyCode::KEY_KP9 => "9",
            KeyCode::KEY_0 | KeyCode::KEY_KP0 => "0",
            KeyCode::KEY_SPACE => "space",
            KeyCode::KEY_ENTER | KeyCode::KEY_KPENTER => "return",
            KeyCode::KEY_F => "f",
            _ => return None,
        })
    }

    /// A device qualifies as a keyboard for our purposes when it emits
    /// key events at all and can produce at least one key in the
    /// vocabulary. This keeps mice (BTN_LEFT is a key event too) and
    /// consumer-control devices out of the reader set.
    fn is_candidate(device: &Device) -> bool {
        if !device.supported_events().contains(EventType::KEY) {
            return false;
        }
        device
            .supported_keys()
            .is_some_and(|keys| keys.iter().any(|code| key_name(code).is_some()))
    }

    /// Kernel evdev key-event values: 0 release, 1 press, 2 autorepeat.
    /// Autorepeat maps to a press, matching the repeated WM_KEYDOWN
    /// stream the Windows hook delivers while a key is held.
    fn kind_for_value(value: i32) -> Option<KeystrokeKind> {
        match value {
            0 => Some(KeystrokeKind::Release),
            1 | 2 => Some(KeystrokeKind::Press),
            _ => None,
        }
    }

    fn set_nonblocking(fd: i32) {
        // A reader must never block in read(2) past the stop flag;
        // poll(2) provides the bounded wait instead.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    pub fn start(
        callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
        allowlist: Option<BTreeSet<String>>,
    ) -> Option<Running> {
        let devices: Vec<(std::path::PathBuf, Device)> = evdev::enumerate()
            .filter(|(_, device)| is_candidate(device))
            .collect();
        if devices.is_empty() {
            tracing::warn!(
                target: "eo::input",
                "no readable keyboard device found; keystroke source inert \
                 (is the user in the input group?)"
            );
            return None;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel::<(String, KeystrokeKind)>();

        let mut readers = Vec::with_capacity(devices.len());
        for (path, mut device) in devices {
            let fd = device.as_raw_fd();
            set_nonblocking(fd);
            let stop_flag = stop.clone();
            let event_sender = sender.clone();
            let label = path.display().to_string();
            let reader = std::thread::Builder::new()
                .name("keystroke-evdev".into())
                .spawn(move || {
                    tracing::info!(target: "eo::input", device = %label, "evdev reader attached");
                    let mut pollfd = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    loop {
                        if stop_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        let ready = unsafe { libc::poll(&mut pollfd, 1, POLL_TIMEOUT_MS) };
                        if ready <= 0 {
                            // Timeout or EINTR: re-check the stop flag.
                            continue;
                        }
                        match device.fetch_events() {
                            Ok(events) => {
                                for event in events {
                                    if event.event_type() != EventType::KEY {
                                        continue;
                                    }
                                    let Some(kind) = kind_for_value(event.value()) else {
                                        continue;
                                    };
                                    if let Some(key) = key_name(KeyCode::new(event.code())) {
                                        let _ = event_sender.send((key.to_string(), kind));
                                    }
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(error) => {
                                // The device went away (unplug, suspend);
                                // this reader retires, the others carry on.
                                tracing::warn!(
                                    target: "eo::input",
                                    device = %label,
                                    %error,
                                    "evdev reader detached"
                                );
                                break;
                            }
                        }
                    }
                });
            match reader {
                Ok(handle) => readers.push(handle),
                Err(_) => {
                    // A failed spawn tears the already-started readers
                    // down through Running's drop.
                    drop(sender);
                    let mut partial = Running {
                        stop,
                        readers,
                        worker: None,
                    };
                    partial.shutdown();
                    return None;
                }
            }
        }
        // The worker's queue must end when the readers exit, so the
        // spawn loop's original sender does not outlive them.
        drop(sender);

        let worker = match std::thread::Builder::new()
            .name("keystroke-dispatch".into())
            .spawn(move || {
                // Ends when the last reader drops its sender.
                while let Ok((key, kind)) = receiver.recv() {
                    super::HookKeystrokeSource::dispatch(&callbacks, &allowlist, &key, kind);
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let mut partial = Running {
                    stop,
                    readers,
                    worker: None,
                };
                partial.shutdown();
                return None;
            }
        };

        Some(Running {
            stop,
            readers,
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_vocabulary_mirrors_the_windows_mapping() {
            for (row, pad, expected) in [
                (KeyCode::KEY_1, KeyCode::KEY_KP1, "1"),
                (KeyCode::KEY_2, KeyCode::KEY_KP2, "2"),
                (KeyCode::KEY_3, KeyCode::KEY_KP3, "3"),
                (KeyCode::KEY_4, KeyCode::KEY_KP4, "4"),
                (KeyCode::KEY_5, KeyCode::KEY_KP5, "5"),
                (KeyCode::KEY_6, KeyCode::KEY_KP6, "6"),
                (KeyCode::KEY_7, KeyCode::KEY_KP7, "7"),
                (KeyCode::KEY_8, KeyCode::KEY_KP8, "8"),
                (KeyCode::KEY_9, KeyCode::KEY_KP9, "9"),
                (KeyCode::KEY_0, KeyCode::KEY_KP0, "0"),
            ] {
                assert_eq!(key_name(row), Some(expected), "number row {row:?}");
                assert_eq!(key_name(pad), Some(expected), "keypad {pad:?}");
            }
            assert_eq!(key_name(KeyCode::KEY_SPACE), Some("space"));
            assert_eq!(key_name(KeyCode::KEY_ENTER), Some("return"));
            assert_eq!(key_name(KeyCode::KEY_KPENTER), Some("return"));
            assert_eq!(key_name(KeyCode::KEY_F), Some("f"));
            for unmapped in [
                KeyCode::KEY_A,
                KeyCode::KEY_ESC,
                KeyCode::KEY_F5,
                KeyCode::KEY_F8,
                KeyCode::KEY_LEFTSHIFT,
            ] {
                assert_eq!(key_name(unmapped), None, "{unmapped:?} must stay unmapped");
            }
        }

        #[test]
        fn autorepeat_counts_as_a_press_and_unknown_values_drop() {
            assert_eq!(kind_for_value(0), Some(KeystrokeKind::Release));
            assert_eq!(kind_for_value(1), Some(KeystrokeKind::Press));
            assert_eq!(kind_for_value(2), Some(KeystrokeKind::Press));
            assert_eq!(kind_for_value(3), None);
        }

        #[test]
        fn set_nonblocking_sets_the_flag_and_preserves_it() {
            unsafe {
                let mut fds = [0i32; 2];
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe(2) for a real fd");
                let read_fd = fds[0];
                let write_fd = fds[1];

                // A fresh pipe fd blocks; the call adds O_NONBLOCK.
                assert_eq!(
                    libc::fcntl(read_fd, libc::F_GETFL) & libc::O_NONBLOCK,
                    0,
                    "a fresh fd blocks"
                );
                set_nonblocking(read_fd);
                assert_ne!(
                    libc::fcntl(read_fd, libc::F_GETFL) & libc::O_NONBLOCK,
                    0,
                    "the call sets O_NONBLOCK"
                );

                // A second call preserves the flag (an OR into the existing
                // flags, never a toggle).
                set_nonblocking(read_fd);
                assert_ne!(
                    libc::fcntl(read_fd, libc::F_GETFL) & libc::O_NONBLOCK,
                    0,
                    "O_NONBLOCK survives a repeat call"
                );

                libc::close(read_fd);
                libc::close(write_fd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-19T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn the_mock_only_delivers_while_running() {
        let source = MockKeystrokeSource::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        source.subscribe(Arc::new(move |event: &KeystrokeEvent| {
            sink.lock().unwrap().push(event.key.clone());
        }));

        source.inject("1", now(), KeystrokeKind::Press);
        assert!(seen.lock().unwrap().is_empty(), "dropped before start");

        assert!(source.start());
        source.inject("2", now(), KeystrokeKind::Press);
        source.stop();
        source.inject("3", now(), KeystrokeKind::Press);
        assert_eq!(*seen.lock().unwrap(), ["2"]);
    }

    #[test]
    fn the_hook_source_lifecycle_is_safe_without_a_start() {
        let source = HookKeystrokeSource::new(None);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            assert!(!source.start(), "no hook mechanism on this platform");
            source.stop();
        }
        #[cfg(any(windows, target_os = "linux"))]
        {
            // Starting a real observer in a unit test would install a
            // process-global keyboard hook (Windows) or attach an X
            // connection or evdev readers whose success depends on the
            // machine's display and device permissions (Linux): both are
            // environment, not logic.
            // The attach paths are exercised by the listener wiring
            // with mock sources and by the platform smoke instead.
            // Stopping an un-started source must be a safe no-op.
            source.stop();
            source.stop();
        }
    }

    #[test]
    fn dispatch_filters_by_allowlist_and_contains_panics() {
        let callbacks: Arc<Mutex<Vec<KeystrokeCallback>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        callbacks
            .lock()
            .unwrap()
            .push(Arc::new(|_: &KeystrokeEvent| panic!("contained")));
        callbacks
            .lock()
            .unwrap()
            .push(Arc::new(move |event: &KeystrokeEvent| {
                sink.lock().unwrap().push(event.key.clone());
            }));
        let allowlist: Option<BTreeSet<String>> =
            Some(["1".to_string(), "space".to_string()].into());

        HookKeystrokeSource::dispatch(&callbacks, &allowlist, "1", KeystrokeKind::Press);
        HookKeystrokeSource::dispatch(&callbacks, &allowlist, "x", KeystrokeKind::Press);
        HookKeystrokeSource::dispatch(&callbacks, &allowlist, "space", KeystrokeKind::Release);
        assert_eq!(*seen.lock().unwrap(), ["1", "space"]);
    }

    #[test]
    fn kind_wire_values_match_the_backend_vocabulary() {
        assert_eq!(KeystrokeKind::Press.as_str(), "press");
        assert_eq!(KeystrokeKind::Release.as_str(), "release");
    }

    #[test]
    fn the_hook_source_keeps_subscribers_for_the_next_start() {
        let source = HookKeystrokeSource::new(None);
        let seen = Arc::new(Mutex::new(0usize));
        let sink = seen.clone();
        source.subscribe(Arc::new(move |_: &KeystrokeEvent| {
            *sink.lock().unwrap() += 1;
        }));
        // The portable dispatch path proves the registration landed.
        HookKeystrokeSource::dispatch(&source.callbacks, &None, "1", KeystrokeKind::Press);
        assert_eq!(*seen.lock().unwrap(), 1);
    }

    #[test]
    fn the_shared_source_refcounts_start_and_stop_over_one_inner() {
        let inner = Arc::new(MockKeystrokeSource::new());
        let shared = SharedKeystrokeSource::new(inner.clone() as Arc<dyn KeystrokeSource>);
        let seen = Arc::new(Mutex::new(0usize));
        let sink = seen.clone();
        shared.subscribe(Arc::new(move |_: &KeystrokeEvent| {
            *sink.lock().unwrap() += 1;
        }));

        // Two listeners start: the inner attaches once and delivers.
        assert!(shared.start());
        assert!(shared.start());
        inner.inject("space", now(), KeystrokeKind::Press);
        assert_eq!(*seen.lock().unwrap(), 1);

        // One stop: the other listener still wants events, so the inner stays
        // attached and keeps delivering.
        shared.stop();
        inner.inject("space", now(), KeystrokeKind::Press);
        assert_eq!(*seen.lock().unwrap(), 2);

        // The last stop detaches the inner: no more delivery. A redundant
        // stop past zero is a no-op.
        shared.stop();
        shared.stop();
        inner.inject("space", now(), KeystrokeKind::Press);
        assert_eq!(
            *seen.lock().unwrap(),
            2,
            "the last stop detached the shared source"
        );

        // A fresh start re-attaches.
        assert!(shared.start());
        inner.inject("space", now(), KeystrokeKind::Press);
        assert_eq!(*seen.lock().unwrap(), 3);
    }
}
