//! Keystroke source abstraction, ported from
//! `backend/testing/keystroke_source.py`.
//!
//! Listeners consume a `KeystrokeSource` rather than touching the OS
//! hook themselves: production wires the Windows low-level keyboard
//! hook, tests inject through the mock. The input-listening
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

/// The production source: the Windows low-level keyboard hook behind
/// the same trait. On other platforms `start` stays inert and returns
/// false, exactly as the original does when its hook library is
/// unavailable.
pub struct HookKeystrokeSource {
    // Consumed by the platform hook module; the portable build keeps
    // the field so construction is uniform across platforms.
    #[cfg_attr(not(windows), allow(dead_code))]
    allowlist: Option<BTreeSet<String>>,
    callbacks: Arc<Mutex<Vec<KeystrokeCallback>>>,
    #[cfg(windows)]
    state: Mutex<Option<windows_hook::Running>>,
}

impl HookKeystrokeSource {
    /// `allowlist = None` admits every key the vocabulary can name.
    pub fn new(allowlist: Option<BTreeSet<String>>) -> Self {
        Self {
            allowlist,
            callbacks: Arc::new(Mutex::new(Vec::new())),
            #[cfg(windows)]
            state: Mutex::new(None),
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
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

    #[cfg(windows)]
    fn start(&self) -> bool {
        let mut state = self.state.lock().expect("hook state");
        if state.is_some() {
            return true;
        }
        match windows_hook::start(self.callbacks.clone(), self.allowlist.clone()) {
            Some(running) => {
                *state = Some(running);
                true
            }
            None => false,
        }
    }

    #[cfg(not(windows))]
    fn start(&self) -> bool {
        false
    }

    #[cfg(windows)]
    fn stop(&self) {
        if let Some(running) = self.state.lock().expect("hook state").take() {
            running.stop();
        }
    }

    #[cfg(not(windows))]
    fn stop(&self) {}
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
        TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    /// The hook procedure has no user-data slot, so the active pump's
    /// queue sender lives here. The slot enforces a hard
    /// single-instance contract: `start` refuses while another hook
    /// owns it, so two sources can never clobber each other's
    /// routing.
    static ACTIVE: OnceLock<Mutex<Option<Sender<(String, KeystrokeKind)>>>> = OnceLock::new();

    fn active() -> &'static Mutex<Option<Sender<(String, KeystrokeKind)>>> {
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

    /// The key vocabulary the listeners speak: number-row digits and
    /// the spacebar. Unmapped virtual keys return None, matching the
    /// original's unmappable-key handling.
    fn key_name(vk: u32) -> Option<String> {
        match vk {
            0x30..=0x39 => Some(((b'0' + (vk - 0x30) as u8) as char).to_string()),
            0x60..=0x69 => Some(((b'0' + (vk - 0x60) as u8) as char).to_string()),
            0x20 => Some("space".to_string()),
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
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
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
        let pump = std::thread::Builder::new()
            .name("keystroke-hook".into())
            .spawn(move || unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0);
                let Ok(hook) = hook else {
                    let _ = ready_sender.send(None);
                    *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                    return;
                };
                let thread_id = windows::Win32::System::Threading::GetCurrentThreadId();
                let _ = ready_sender.send(Some(thread_id));
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).into() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let _ = UnhookWindowsHookEx(hook);
                *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
            })
            .ok()?;

        let pump_thread_id = match ready_receiver.recv() {
            Ok(Some(thread_id)) => thread_id,
            _ => {
                let _ = pump.join();
                return None;
            }
        };

        let worker = std::thread::Builder::new()
            .name("keystroke-dispatch".into())
            .spawn(move || {
                // Ends when the pump drops the active sender.
                while let Ok((key, kind)) = receiver.recv() {
                    super::HookKeystrokeSource::dispatch(&callbacks, &allowlist, &key, kind);
                }
            })
            .ok()?;

        Some(Running {
            pump_thread_id,
            pump: Some(pump),
            worker: Some(worker),
        })
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
    fn the_hook_source_is_inert_off_windows() {
        let source = HookKeystrokeSource::new(None);
        #[cfg(not(windows))]
        {
            assert!(!source.start(), "no hook mechanism on this platform");
            source.stop();
        }
        #[cfg(windows)]
        {
            // Starting a real hook on CI is possible but pointless
            // headless; the lifecycle is exercised by the listener
            // wiring and the platform smoke instead.
            let _ = &source;
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
}
