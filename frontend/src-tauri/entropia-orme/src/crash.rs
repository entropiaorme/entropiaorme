//! Default-off, opt-in, local-only crash reporting with PII scrubbing.
//!
//! When (and only when) the user opts in, a panic writes a PII-scrubbed crash
//! report to a `crash/` subdirectory of the application data directory:
//! backtrace, panic message, thread, and build metadata, nothing else. The
//! report never leaves the machine: there is no network path, no DSN, and no
//! CSP allowance for one. The opt-in defaults OFF, so an out-of-the-box install
//! writes nothing on a panic beyond the standard console message.
//!
//! ## Why a Rust-owned config file, not `settings.json`
//!
//! The opt-in lives in `<data_dir>/observability.json`, a file this shell owns
//! outright, NOT in `settings.json`. `settings.json` is the dual-arm
//! equivalence surface (the native and the frozen Python settings routes are
//! diffed against each other on the real database), and its typed `AppConfig`
//! serialises straight into the settings response; adding a field there would
//! diverge the native arm from the Python arm. A Rust-owned file sidesteps that
//! entirely and keeps the feature behaviour-neutral.
//!
//! ## The remote-sink seam
//!
//! [`render_report`] returns the finished, scrubbed report as a `String`. That
//! is the seam a future opt-in remote sink would consume (after its own
//! explicit transmit consent and a `connect-src` CSP allowance). This module
//! deliberately wires no transport: local-only is the whole of the current
//! scope.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The Rust-owned observability config file, holding the crash-reporting
/// opt-in (and any future shell-owned observability toggles).
const OBSERVABILITY_CONFIG: &str = "observability.json";
const CRASH_REPORTING_KEY: &str = "crash_reporting_enabled";

/// Whether crash reporting is enabled. Reads `observability.json` from the data
/// directory; absent, unreadable, malformed, or missing-the-key all read as
/// `false` (the default-off contract).
pub(crate) fn reporting_enabled(data_dir: &Path) -> bool {
    let raw = match std::fs::read_to_string(data_dir.join(OBSERVABILITY_CONFIG)) {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|value| value.get(CRASH_REPORTING_KEY).and_then(Value::as_bool))
        .unwrap_or(false)
}

/// The personal data to strip from a crash report: the player name and the
/// machine-specific paths (the configured chat-log path and the data
/// directory), both of which embed the OS account name on a typical install.
struct Secrets {
    player_name: String,
    paths: Vec<String>,
}

impl Secrets {
    /// Read the secrets from the live settings (read-only; never writes) plus
    /// the data directory. A missing or unreadable settings file yields empty
    /// secrets, so scrubbing still runs (the path redaction below is the
    /// backstop).
    fn from_data_dir(data_dir: &Path) -> Self {
        let config =
            eo_services::config_service::load_config_readonly(data_dir).unwrap_or_default();
        let mut paths = vec![config.chatlog_path.clone(), data_dir.display().to_string()];
        paths.retain(|path| !path.trim().is_empty());
        // Longest paths first so a path that contains the player name is
        // redacted whole rather than partially.
        paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
        Self {
            player_name: config.player_name,
            paths,
        }
    }
}

/// Strip personal data from report text: exact configured secrets first (the
/// player name and machine paths), then a structural backstop that redacts the
/// account-name segment of any home/profile path that slipped through (a
/// backtrace frame, an OS error string).
fn scrub(text: &str, secrets: &Secrets) -> String {
    let mut out = text.to_string();
    for path in &secrets.paths {
        out = out.replace(path, "<path>");
    }
    if !secrets.player_name.trim().is_empty() {
        out = out.replace(&secrets.player_name, "<player>");
    }
    redact_user_paths(&out)
}

/// Redact the account-name segment immediately following a home/profile-path
/// marker, so a path that matched no configured secret still cannot leak the OS
/// account name.
fn redact_user_paths(text: &str) -> String {
    let mut out = redact_after(text, "\\Users\\");
    out = redact_after(&out, "/home/");
    out = redact_after(&out, "/Users/");
    out
}

fn redact_after(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(marker) {
        let (head, after) = rest.split_at(pos + marker.len());
        out.push_str(head);
        // The account segment runs to the next path separator or token
        // delimiter; anything past that is structural, not the name.
        let end = after
            .find(|c: char| matches!(c, '/' | '\\' | '"' | '\'' | ':') || c.is_whitespace())
            .unwrap_or(after.len());
        if end > 0 {
            out.push_str("<user>");
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Render a scrubbed crash report from the extracted panic fields. Pure (the
/// backtrace is captured here, but the inputs are otherwise the caller's), so
/// the rendering and scrubbing are unit-testable without provoking a real
/// panic. Build metadata comes from compile-time constants and `std::env`, so
/// no `build.rs` is needed.
fn render_report(
    panic_message: &str,
    location: Option<&str>,
    thread_name: &str,
    secrets: &Secrets,
) -> String {
    let raw = format!(
        "EntropiaOrme crash report\n\
         version: {version}\n\
         target: {os} {arch}\n\
         profile: {profile}\n\
         thread: {thread}\n\
         location: {location}\n\
         panic: {panic}\n\
         \n\
         backtrace:\n{backtrace}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        thread = thread_name,
        location = location.unwrap_or("unknown"),
        panic = panic_message,
        backtrace = std::backtrace::Backtrace::force_capture(),
    );
    scrub(&raw, secrets)
}

/// Write a finished report to a timestamped file under `<data_dir>/crash/`.
/// Best-effort: the caller (a panic hook) ignores the result.
fn write_report(data_dir: &Path, report: &str) -> std::io::Result<PathBuf> {
    let crash_dir = data_dir.join("crash");
    std::fs::create_dir_all(&crash_dir)?;
    // Real wall-clock is correct here (a crash is a real-time process event,
    // not a replayed one), and uniqueness only needs to avoid same-process
    // collisions.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = crash_dir.join(format!("crash-{stamp}.txt"));
    std::fs::write(&path, report)?;
    Ok(path)
}

/// The body of the panic hook, factored out so it is testable without
/// installing a process-global hook. Writes a scrubbed report ONLY when the
/// opt-in is enabled; otherwise it is a no-op (the default-off contract).
fn handle_panic(data_dir: &Path, message: &str, location: Option<&str>, thread_name: &str) {
    if !reporting_enabled(data_dir) {
        return;
    }
    let secrets = Secrets::from_data_dir(data_dir);
    let report = render_report(message, location, thread_name, &secrets);
    let _ = write_report(data_dir, &report);
}

/// Install the process-wide panic hook. It always preserves the standard panic
/// behaviour (the console message / abort), then, only if the opt-in is
/// currently enabled, writes a scrubbed local crash report. The opt-in is
/// re-read on each panic so a runtime toggle takes effect without a restart.
pub fn install_panic_hook(data_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let message = panic_payload_string(info.payload());
        let location = info.location().map(|loc| loc.to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        handle_panic(&data_dir, &message, location.as_deref(), &thread);
    }));
}

/// Best-effort rendering of a panic payload to a string (panics carry either a
/// `&str` or a `String` in practice).
fn panic_payload_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporting_is_off_by_default_and_when_the_file_is_absent_or_malformed() {
        let dir = tempfile::tempdir().unwrap();
        // Absent file.
        assert!(!reporting_enabled(dir.path()));
        // Malformed file.
        std::fs::write(dir.path().join(OBSERVABILITY_CONFIG), "{not json").unwrap();
        assert!(!reporting_enabled(dir.path()));
        // Present but the key is missing.
        std::fs::write(dir.path().join(OBSERVABILITY_CONFIG), "{}").unwrap();
        assert!(!reporting_enabled(dir.path()));
        // Present and explicitly false.
        std::fs::write(
            dir.path().join(OBSERVABILITY_CONFIG),
            r#"{"crash_reporting_enabled": false}"#,
        )
        .unwrap();
        assert!(!reporting_enabled(dir.path()));
    }

    #[test]
    fn the_scrubber_strips_the_player_name_and_account_paths() {
        let secrets = Secrets {
            player_name: "Frussjager".to_string(),
            paths: vec![
                "C:\\Users\\realaccount\\Documents\\Entropia Universe\\chat.log".to_string(),
            ],
        };
        let text = "panic for Frussjager reading \
                    C:\\Users\\realaccount\\Documents\\Entropia Universe\\chat.log \
                    and also C:\\Users\\realaccount\\AppData\\Roaming\\thing and /home/realaccount/x";
        let scrubbed = scrub(text, &secrets);
        assert!(
            !scrubbed.contains("Frussjager"),
            "player name removed: {scrubbed}"
        );
        assert!(
            !scrubbed.contains("realaccount"),
            "account name removed: {scrubbed}"
        );
        assert!(scrubbed.contains("<player>"));
        assert!(scrubbed.contains("<path>"));
        assert!(scrubbed.contains("<user>"));
    }

    #[test]
    fn redact_user_paths_handles_each_platform_marker() {
        assert_eq!(
            redact_user_paths("C:\\Users\\Mikel\\AppData\\Roaming"),
            "C:\\Users\\<user>\\AppData\\Roaming"
        );
        assert_eq!(
            redact_user_paths("/home/alice/.config"),
            "/home/<user>/.config"
        );
        assert_eq!(
            redact_user_paths("/Users/bob/Library"),
            "/Users/<user>/Library"
        );
        // No marker: unchanged.
        assert_eq!(
            redact_user_paths("E:\\Workspace\\repo"),
            "E:\\Workspace\\repo"
        );
    }

    #[test]
    fn a_panic_writes_no_report_when_reporting_is_off() {
        let dir = tempfile::tempdir().unwrap();
        // Default: off. Simulate the panic-hook body directly.
        handle_panic(dir.path(), "boom", Some("src/x.rs:1:1"), "main");
        assert!(
            !dir.path().join("crash").exists(),
            "the default-off contract: no crash directory, no report"
        );
    }

    #[test]
    fn an_opted_in_panic_writes_a_scrubbed_report_with_build_metadata() {
        let dir = tempfile::tempdir().unwrap();
        // Seed the secrets (settings.json) and enable the opt-in.
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"player_name":"SecretHunter","chatlog_path":"C:\\Users\\realaccount\\chat.log"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join(OBSERVABILITY_CONFIG),
            r#"{"crash_reporting_enabled": true}"#,
        )
        .unwrap();

        handle_panic(
            dir.path(),
            "tracker overflowed near SecretHunter at C:\\Users\\realaccount\\chat.log",
            Some("eo-services/src/tracker.rs:42:7"),
            "chatlog-watcher",
        );

        let crash_dir = dir.path().join("crash");
        let report_path = std::fs::read_dir(&crash_dir)
            .expect("crash dir created")
            .flatten()
            .next()
            .expect("a report file was written")
            .path();
        let report = std::fs::read_to_string(&report_path).unwrap();

        // (b) the expected metadata is present.
        assert!(report.contains("EntropiaOrme crash report"));
        assert!(report.contains(&format!("version: {}", env!("CARGO_PKG_VERSION"))));
        assert!(report.contains("thread: chatlog-watcher"));
        assert!(report.contains("location: eo-services/src/tracker.rs:42:7"));
        assert!(report.contains("backtrace:"));
        // (c) the PII is scrubbed out of the panic message.
        assert!(
            !report.contains("SecretHunter"),
            "player name scrubbed: {report}"
        );
        assert!(
            !report.contains("realaccount"),
            "account name scrubbed: {report}"
        );
    }
}
