//! Signing in to the Entropia Universe account site, in a window of its
//! own.
//!
//! The player signs in on the site itself, in a normal window the app
//! opens; the app never sees the password. The window has no IPC access:
//! its label matches no capability (a test pins it), so although the
//! webview framework injects its own bridge into it as into every window,
//! the access rules refuse that bridge every command. The app evaluates no
//! script in it. It may navigate only to `https` pages on the account
//! site's host (and the site's bot check, below), and every permission its
//! page asks for (camera, microphone, location, notifications, clipboard)
//! is refused.
//!
//! The window runs on a private, in-memory profile: the session cookie the
//! site sets lives only in that window's own context, which ends with the
//! window, and in the connection's memory. Nothing is written to disk and
//! no other window can use the session. The player signs in once per run.
//!
//! While it is open, the window's own address and cookies are checked
//! every two seconds. Once it shows the signed-in area, a session value is
//! handed to the connection; when the site accepts it the window closes. A
//! value the site refused is tried again no sooner than 15 seconds later.
//! The window lives fifteen minutes at most.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use eo_api::ApiError;
use eo_services::account::site::{SESSION_COOKIE, SITE_ORIGIN};
use eo_services::account::{
    AccountRecorder, ConnectEpoch, ConnectOutcome, MemoryOnly, SessionSecret, SessionStore,
};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::time::Instant;

use crate::commands;

/// The sign-in window's label. No capability may name or match it.
pub const SIGN_IN_WINDOW: &str = "account-sign-in";

/// The page the window opens on; the site sends a visitor who is not
/// signed in to its login page from there.
const SIGN_IN_PAGE: &str = "https://account.entropiauniverse.com/account/inventory";

/// The account site's host.
const SITE_HOST: &str = "account.entropiauniverse.com";

/// The host of the bot check the sign-in page embeds in a frame.
const CHALLENGE_HOST: &str = "challenges.cloudflare.com";

/// The signed-in area of the site: its path, and everything under it.
const SIGNED_IN_AREA: &str = "/account";

/// How often the window is checked while it is open.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// How long the window stays open waiting for a sign-in.
const SIGN_IN_LIFE: Duration = Duration::from_secs(15 * 60);

/// How long a value the site refused waits before it is tried again.
const REFUSED_WAIT: Duration = Duration::from_secs(15);

/// The window this module opened and still owns: its number, or 0 when it
/// owns none. A window carrying the label that this module did not open
/// (or no longer owns) is never adopted.
static OWNED_WINDOW: AtomicU64 = AtomicU64::new(0);
static WINDOWS_OPENED: AtomicU64 = AtomicU64::new(0);

/// The store the shell keeps the session in: memory only, so nothing is
/// kept between runs.
pub fn session_store() -> std::sync::Arc<dyn SessionStore> {
    std::sync::Arc::new(MemoryOnly)
}

fn https_host(url: &Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// Whether the sign-in window may load `url`: an `https` page on the
/// account site's host or the bot check's host (each exact, on its
/// default port, with no credentials in it), or the empty pages a frame
/// starts on. Anything else is refused, so a link off the site does
/// nothing.
pub fn navigation_allowed(url: &Url) -> bool {
    https_host(url, SITE_HOST)
        || https_host(url, CHALLENGE_HOST)
        || matches!(url.as_str(), "about:blank" | "about:srcdoc")
}

/// Whether the window shows the signed-in area of the site.
fn in_signed_in_area(url: &Url) -> bool {
    https_host(url, SITE_HOST)
        && (url.path() == SIGNED_IN_AREA
            || url
                .path()
                .strip_prefix(SIGNED_IN_AREA)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// What the watcher sees on one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sight {
    /// The window is still the one this watcher opened.
    pub mine: bool,
    pub timed_out: bool,
    /// The window shows the signed-in area.
    pub signed_in_area: bool,
    /// The fingerprint of the session value the window holds, if any.
    pub found: Option<u64>,
}

/// What the watcher does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The window is gone or no longer this watcher's: stop.
    End,
    /// Time is up: close the window and stop.
    CloseAndEnd,
    Wait,
    /// Hand the value found to the connection.
    Try,
}

/// Decide one check. `refused` holds when each value the site refused was
/// refused.
pub fn next_step(sight: Sight, refused: &HashMap<u64, Instant>, now: Instant) -> Step {
    if !sight.mine {
        return Step::End;
    }
    if sight.timed_out {
        return Step::CloseAndEnd;
    }
    let Some(found) = sight.found.filter(|_| sight.signed_in_area) else {
        return Step::Wait;
    };
    match refused.get(&found) {
        Some(at) if now < *at + REFUSED_WAIT => Step::Wait,
        _ => Step::Try,
    }
}

/// What a try came to, for the watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tried {
    /// Connected: close the window and stop.
    CloseAndEnd,
    /// The site refused the value: remember when.
    Refused,
    /// Unreachable or overtaken: try again on the next check.
    Again,
}

pub fn after_try(outcome: ConnectOutcome) -> Tried {
    match outcome {
        ConnectOutcome::Connected => Tried::CloseAndEnd,
        ConnectOutcome::NotAccepted => Tried::Refused,
        ConnectOutcome::Unreachable(_) | ConnectOutcome::Interrupted => Tried::Again,
    }
}

fn recorder(app: &AppHandle) -> Option<std::sync::Arc<AccountRecorder>> {
    commands::facade(app)
        .ok()
        .and_then(|api| api.account_recorder())
}

/// The sign-in window, if it is the one this module opened as `number`.
fn owned_window(app: &AppHandle, number: u64) -> Option<WebviewWindow> {
    (OWNED_WINDOW.load(Ordering::SeqCst) == number)
        .then(|| app.get_webview_window(SIGN_IN_WINDOW))
        .flatten()
}

/// The window closed, by any route: this module owns none now.
pub fn window_gone() {
    OWNED_WINDOW.store(0, Ordering::SeqCst);
}

/// Refuse every permission the remote page asks for.
fn refuse_permissions(window: &WebviewWindow) {
    #[cfg(target_os = "linux")]
    let _ = window.with_webview(|platform| {
        use webkit2gtk::{PermissionRequestExt, WebViewExt};
        platform.inner().connect_permission_request(|_, request| {
            request.deny();
            true
        });
    });
    #[cfg(windows)]
    let _ = window.with_webview(|platform| {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PERMISSION_STATE_DENY;
        use webview2_com::PermissionRequestedEventHandler;
        // SAFETY: the controller and its webview are live for the call;
        // the handler only sets the request's state.
        unsafe {
            if let Ok(webview) = platform.controller().CoreWebView2() {
                let mut token = 0i64;
                let _ = webview.add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                        if let Some(args) = args {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                        }
                        Ok(())
                    })),
                    &mut token,
                );
            }
        }
    });
    #[cfg(not(any(target_os = "linux", windows)))]
    let _ = window;
}

/// Open the sign-in window, or bring forward the one this module opened.
/// A window carrying the label that this module did not open is closed
/// and replaced. Refused when the connection could not be set up.
#[tauri::command]
pub async fn account_sign_in(app: AppHandle) -> Result<(), ApiError> {
    let Some(recorder) = recorder(&app) else {
        return Err(ApiError::conflict(
            "the account connection is not available",
        ));
    };
    if let Some(window) = app.get_webview_window(SIGN_IN_WINDOW) {
        if OWNED_WINDOW.load(Ordering::SeqCst) != 0 {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            return Ok(());
        }
        let _ = window.destroy();
        // The label frees once the window is gone.
        for _ in 0..40 {
            if app.get_webview_window(SIGN_IN_WINDOW).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let page: Url = SIGN_IN_PAGE
        .parse()
        .map_err(|_| ApiError::invalid_state("the sign-in page is not a URL"))?;
    let window = WebviewWindowBuilder::new(&app, SIGN_IN_WINDOW, WebviewUrl::External(page))
        .title("Sign in at account.entropiauniverse.com")
        .inner_size(1100.0, 850.0)
        .incognito(true)
        .on_navigation(navigation_allowed)
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .build()
        .map_err(ApiError::internal("opening the sign-in window"))?;
    refuse_permissions(&window);
    let number = WINDOWS_OPENED.fetch_add(1, Ordering::SeqCst) + 1;
    OWNED_WINDOW.store(number, Ordering::SeqCst);
    tauri::async_runtime::spawn(watch_for_sign_in(app, number, recorder.epoch()));
    Ok(())
}

/// Read what the window shows: whether it is in the signed-in area, and
/// the session value it holds. Off the async workers: on Windows the
/// cookie store must never be read from a synchronous command or event
/// handler.
async fn look(window: WebviewWindow, hasher: RandomState) -> (bool, Option<(u64, SessionSecret)>) {
    tauri::async_runtime::spawn_blocking(move || {
        let signed_in_area = window.url().is_ok_and(|url| in_signed_in_area(&url));
        if !signed_in_area {
            return (false, None);
        }
        let Ok(site) = SITE_ORIGIN.parse::<Url>() else {
            return (true, None);
        };
        let cookie = window
            .cookies_for_url(site)
            .ok()
            .and_then(|cookies| cookies.into_iter().find(|c| c.name() == SESSION_COOKIE));
        let found = cookie.and_then(|cookie| {
            let value = cookie.value().to_string();
            let fingerprint = hasher.hash_one(&value);
            SessionSecret::new(value)
                .ok()
                .map(|session| (fingerprint, session))
        });
        (true, found)
    })
    .await
    .unwrap_or((false, None))
}

/// Watch the window this module opened as `number`, and hand each session
/// value it shows to the connection (as a connect begun in `epoch`). Ends
/// when the window closes by any route or is replaced, when the site
/// accepts a session, or when the window's time is up (closing it).
async fn watch_for_sign_in(app: AppHandle, number: u64, epoch: ConnectEpoch) {
    let deadline = Instant::now() + SIGN_IN_LIFE;
    // Fingerprints keyed for this watcher alone, dropped with it.
    let hasher = RandomState::new();
    let mut refused: HashMap<u64, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        let window = owned_window(&app, number);
        let now = Instant::now();
        let timed_out = now >= deadline;
        let (signed_in_area, mut found) = match &window {
            Some(window) if !timed_out => look(window.clone(), hasher.clone()).await,
            _ => (false, None),
        };
        let sight = Sight {
            mine: window.is_some(),
            timed_out,
            signed_in_area,
            found: found.as_ref().map(|(fingerprint, _)| *fingerprint),
        };
        match next_step(sight, &refused, now) {
            Step::End => return,
            Step::CloseAndEnd => {
                close_owned(&app, number);
                return;
            }
            Step::Wait => {}
            Step::Try => {
                let (Some((fingerprint, session)), Some(recorder)) = (found.take(), recorder(&app))
                else {
                    continue;
                };
                match after_try(recorder.connect(epoch, session).await) {
                    Tried::CloseAndEnd => {
                        close_owned(&app, number);
                        return;
                    }
                    Tried::Refused => {
                        refused.insert(fingerprint, Instant::now());
                    }
                    Tried::Again => {}
                }
            }
        }
    }
}

/// Close the window this module opened as `number`, if it still is it.
fn close_owned(app: &AppHandle, number: u64) {
    if let Some(window) = owned_window(app, number) {
        window_gone();
        let _ = window.close();
    }
}

/// At launch, when the player has connected before, take up the session
/// kept through the store. The app keeps it in memory only, so today this
/// finds nothing and the connection stays disconnected.
pub fn adopt_stored_session(app: &AppHandle) {
    let Some(recorder) = recorder(app) else {
        return;
    };
    tauri::async_runtime::spawn(eo_services::account::confirm_stored_session(
        recorder,
        session_store(),
    ));
}

/// Disconnect, in an order that always completes: stop the watcher and
/// close the sign-in window, forget the stored session, then disconnect
/// the connection (when there is one).
pub async fn disconnect(app: &AppHandle) {
    let window = app.get_webview_window(SIGN_IN_WINDOW);
    window_gone();
    if let Some(window) = window {
        let _ = window.close();
    }
    let store = session_store();
    let _ = tauri::async_runtime::spawn_blocking(move || store.forget()).await;
    if let Some(recorder) = recorder(app) {
        recorder.disconnect().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str) -> bool {
        navigation_allowed(&url.parse().unwrap())
    }

    #[test]
    fn the_window_navigates_only_to_the_site_and_its_bot_check() {
        for (url, expected) in [
            (SIGN_IN_PAGE, true),
            ("https://account.entropiauniverse.com/login?next=%2F", true),
            (
                "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile",
                true,
            ),
            ("about:blank", true),
            ("about:srcdoc", true),
            // Other hosts, and sub-domains of the allowed ones.
            ("https://www.entropiauniverse.com/", false),
            ("https://evil.account.entropiauniverse.com/", false),
            ("https://account.entropiauniverse.com.example.com/", false),
            ("https://evil.example/account.entropiauniverse.com", false),
            ("https://cloudflare.com/", false),
            ("https://dash.cloudflare.com/", false),
            ("https://evil.challenges.cloudflare.com/", false),
            // Ports, credentials, and other schemes.
            ("https://account.entropiauniverse.com:8443/", false),
            ("https://challenges.cloudflare.com:444/", false),
            ("https://user:pass@account.entropiauniverse.com/", false),
            ("https://user@challenges.cloudflare.com/", false),
            (
                "http://account.entropiauniverse.com/account/inventory",
                false,
            ),
            ("http://challenges.cloudflare.com/", false),
            ("javascript:alert(1)", false),
            ("data:text/html,hi", false),
            ("file:///etc/passwd", false),
            ("blob:https://account.entropiauniverse.com/0b1c", false),
            ("about:config", false),
            ("about:blank#x", false),
            ("tauri://localhost/", false),
        ] {
            assert_eq!(allowed(url), expected, "{url}");
        }
    }

    #[test]
    fn only_the_signed_in_area_is_watched_for_a_session() {
        for (url, expected) in [
            ("https://account.entropiauniverse.com/account", true),
            (
                "https://account.entropiauniverse.com/account/inventory",
                true,
            ),
            ("https://account.entropiauniverse.com/login", false),
            ("https://account.entropiauniverse.com/accounts", false),
            ("https://challenges.cloudflare.com/account/x", false),
        ] {
            assert_eq!(in_signed_in_area(&url.parse().unwrap()), expected, "{url}");
        }
    }

    #[test]
    fn the_sign_in_page_is_on_the_site_the_reader_reads() {
        let page: Url = SIGN_IN_PAGE.parse().unwrap();
        let site: Url = SITE_ORIGIN.parse().unwrap();
        assert_eq!(page.host_str(), site.host_str());
        assert_eq!(page.host_str(), Some(SITE_HOST));
        assert!(in_signed_in_area(&page));
    }

    fn sight(found: Option<u64>) -> Sight {
        Sight {
            mine: true,
            timed_out: false,
            signed_in_area: true,
            found,
        }
    }

    #[test]
    fn the_watcher_tries_a_value_only_in_the_signed_in_area() {
        let now = Instant::now();
        let none = HashMap::new();
        assert_eq!(next_step(sight(Some(7)), &none, now), Step::Try);
        assert_eq!(next_step(sight(None), &none, now), Step::Wait);
        let outside = Sight {
            signed_in_area: false,
            ..sight(Some(7))
        };
        assert_eq!(next_step(outside, &none, now), Step::Wait);
    }

    #[test]
    fn a_refused_value_waits_fifteen_seconds_before_it_is_tried_again() {
        let now = Instant::now();
        let refused = HashMap::from([(7, now)]);
        assert_eq!(next_step(sight(Some(7)), &refused, now), Step::Wait);
        let later = now + Duration::from_secs(14);
        assert_eq!(next_step(sight(Some(7)), &refused, later), Step::Wait);
        let later = now + REFUSED_WAIT;
        assert_eq!(next_step(sight(Some(7)), &refused, later), Step::Try);
        // Another value is tried at once.
        assert_eq!(next_step(sight(Some(8)), &refused, now), Step::Try);
    }

    #[test]
    fn the_watcher_ends_with_its_window_and_closes_it_only_when_time_is_up() {
        let now = Instant::now();
        let none = HashMap::new();
        let gone = Sight {
            mine: false,
            ..sight(Some(7))
        };
        assert_eq!(next_step(gone, &none, now), Step::End);
        // A window that is not its own is never closed by its deadline.
        let gone_and_late = Sight {
            mine: false,
            timed_out: true,
            ..sight(None)
        };
        assert_eq!(next_step(gone_and_late, &none, now), Step::End);
        let late = Sight {
            timed_out: true,
            ..sight(Some(7))
        };
        assert_eq!(next_step(late, &none, now), Step::CloseAndEnd);
    }

    #[test]
    fn only_a_refusal_is_remembered_and_only_a_connection_closes_the_window() {
        assert_eq!(after_try(ConnectOutcome::Connected), Tried::CloseAndEnd);
        assert_eq!(after_try(ConnectOutcome::NotAccepted), Tried::Refused);
        assert_eq!(
            after_try(ConnectOutcome::Unreachable(
                eo_services::account::SiteFailure::Network
            )),
            Tried::Again
        );
        assert_eq!(after_try(ConnectOutcome::Interrupted), Tried::Again);
    }

    #[test]
    fn the_session_is_kept_in_memory_only() {
        assert!(session_store().load().is_none());
    }

    /// Every capability Tauri would load for this app: the files under
    /// `capabilities/` (any depth; one capability, an array of them, or an
    /// object holding a `capabilities` array), and any declared inline in
    /// a `tauri*.conf.json` beside the main one.
    fn every_capability() -> Vec<(String, serde_json::Value)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        fn spread(
            origin: &str,
            value: serde_json::Value,
            out: &mut Vec<(String, serde_json::Value)>,
        ) {
            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        spread(origin, item, out);
                    }
                }
                serde_json::Value::Object(ref object) if object.contains_key("capabilities") => {
                    spread(origin, object["capabilities"].clone(), out);
                }
                other => out.push((origin.to_string(), other)),
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        walk(&root.join("capabilities"), &mut files);
        let mut capabilities = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = match path.extension().and_then(|ext| ext.to_str()) {
                Some("json") => serde_json::from_str(&text).unwrap(),
                other => panic!(
                    "{}: a capability file of kind {other:?} is not read here; check it by hand",
                    path.display()
                ),
            };
            spread(&path.display().to_string(), value, &mut capabilities);
        }
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("tauri") && name.ends_with(".conf.json") {
                let conf: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                let inline = conf["app"]["security"]["capabilities"].clone();
                if !inline.is_null() {
                    spread(&name, inline, &mut capabilities);
                }
            }
        }
        capabilities
    }

    #[test]
    fn no_capability_reaches_the_sign_in_window_or_a_remote_page() {
        let capabilities = every_capability();
        assert!(!capabilities.is_empty());
        for (origin, capability) in &capabilities {
            // A capability named only by identifier (a string) grants
            // nothing to a window by itself.
            let Some(object) = capability.as_object() else {
                continue;
            };
            assert!(
                object.get("remote").is_none(),
                "{origin} declares a remote origin"
            );
            for field in ["windows", "webviews"] {
                for pattern in object
                    .get(field)
                    .and_then(|list| list.as_array())
                    .into_iter()
                    .flatten()
                {
                    let pattern = pattern.as_str().unwrap();
                    let glob = glob::Pattern::new(pattern).unwrap();
                    assert!(
                        !glob.matches(SIGN_IN_WINDOW),
                        "{origin} grants {pattern:?}, which reaches the sign-in window"
                    );
                }
            }
        }
    }

    #[test]
    fn the_glob_reading_matches_as_tauri_reads_a_window_pattern() {
        for (pattern, expected) in [
            ("*", true),
            ("account-*", true),
            ("account-sign-i?", true),
            ("account-[rs]ign-in", true),
            ("[!m]*", true),
            ("main", false),
            ("overlay*", false),
            ("account-[!s]ign-in", false),
        ] {
            assert_eq!(
                glob::Pattern::new(pattern).unwrap().matches(SIGN_IN_WINDOW),
                expected,
                "{pattern}"
            );
        }
    }

    #[test]
    fn no_capability_is_added_at_run_time() {
        let needle = ["add_", "capability("].concat();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        !text.contains(&needle),
                        "{} adds a capability",
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn the_account_commands_are_the_main_window_s_alone() {
        use crate::command_acl::{trusted_commands, ACCOUNT_COMMANDS};
        let trusted = trusted_commands();
        for command in ACCOUNT_COMMANDS {
            assert!(
                !trusted.contains(command),
                "{command} is in the permission every trusted window gets"
            );
            assert!(crate::command_acl::app_commands().contains(command));
        }
        assert!(ACCOUNT_COMMANDS.contains(&"account_sign_in"));
        for (origin, capability) in every_capability() {
            let Some(object) = capability.as_object() else {
                continue;
            };
            let grants_account = object["permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|permission| permission.as_str() == Some("account-commands"));
            if !grants_account {
                continue;
            }
            let windows: Vec<&str> = object["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|window| window.as_str())
                .collect();
            assert_eq!(windows, ["main"], "{origin} grants the account commands");
            assert!(object.get("webviews").is_none());
        }
    }
}
