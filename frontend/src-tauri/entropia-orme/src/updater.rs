//! Auto-updater plumbing: stable/beta channel resolution, the update-check, and
//! the download + deferred-install apply flow, for signed update manifests
//! served from entropiaorme.com.
//!
//! The channel preference is persisted via tauri-plugin-store; at check time the
//! updater's endpoints are resolved from that preference (the config-level
//! endpoint in `tauri.conf.json` is the stable default, the runtime override
//! selects the channel's manifest). Integrity rests on two independent guards
//! that this wiring relies on rather than reimplements:
//!
//! - **Tamper:** the updater verifies the signature over the downloaded
//!   installer bytes against the public key in `tauri.conf.json`, before the
//!   bytes are ever run; an unsigned or wrongly-signed artefact is rejected. The
//!   matching private key is provisioned out of band (a CI signing secret),
//!   never in the tree. The manifest itself is not signed: tampering with it
//!   cannot make the client run an artefact not signed by the trusted key.
//! - **Downgrade:** the built-in comparator only offers a release whose version
//!   is strictly greater than the running one, so a replayed older (still
//!   validly-signed) manifest cannot roll the install back. We do not override
//!   it.
//!
//! The apply flow is a deliberate **download then deferred install** split: the
//! bytes are fetched in-session (with progress surfaced to the UI) and held, so
//! the install (which forcibly exits and relaunches the process on Windows)
//! happens at a moment the user chooses, never mid-session by surprise.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// The store file holding the updater preference. A dedicated file (not the
/// shared frontend prefs store) keeps this plumbing self-contained.
const STORE_FILE: &str = "updater.json";
/// The store key holding the selected update channel.
const CHANNEL_KEY: &str = "channel";
/// The event the frontend listens on for download progress. App-emitted (not a
/// domain topic), so it is named in the colon-form the rest of the bus uses.
const DOWNLOAD_PROGRESS_EVENT: &str = "updater:download-progress";

/// The release channel an update check follows. Stable is the default; beta is
/// the opt-in shake-down channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    /// Parse a stored preference value, falling back to the default for any
    /// unrecognised or absent value: a corrupt preference must never wedge
    /// updates onto a non-existent channel.
    fn from_pref(value: Option<&str>) -> Self {
        match value {
            Some("beta") => Channel::Beta,
            _ => Channel::Stable,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }
}

/// Metadata about an available update, surfaced to the frontend. `pubDate` is
/// deliberately omitted: the manifest's body (release notes) is what the UI
/// renders, and surfacing the date would pull `time`'s formatting into this
/// crate for no user-visible gain.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The version announced by the manifest.
    pub version: String,
    /// The version currently running.
    pub current_version: String,
    /// The release notes (Markdown), when the manifest carries them.
    pub notes: Option<String>,
}

impl UpdateInfo {
    fn from_update(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone(),
        }
    }
}

/// Download progress, emitted on [`DOWNLOAD_PROGRESS_EVENT`] as bytes arrive.
/// `contentLength` is `None` when the server uses chunked transfer and does not
/// announce a total; the UI falls back to an indeterminate indicator then.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    downloaded: usize,
    content_length: Option<u64>,
}

/// A downloaded-but-not-yet-installed update, held between the `download_update`
/// and `install_update` commands so the deferred-install split fetches once and
/// installs at the user's chosen moment.
struct PendingUpdateInner {
    update: Update,
    bytes: Vec<u8>,
}

/// Managed state for the pending update. Registered once at startup; the lock is
/// never held across an await (download completes before the value is stored,
/// and the value is taken out before install runs), so a std mutex is correct.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<PendingUpdateInner>>);

/// The signed-manifest endpoints for a channel: a per-channel `latest.json`
/// under entropiaorme.com (already permitted by the app CSP's `connect-src`),
/// so switching the channel switches which manifest the updater reads.
fn endpoints_for_channel(channel: Channel) -> Vec<String> {
    vec![format!(
        "https://entropiaorme.com/updates/{}/latest.json",
        channel.as_str()
    )]
}

/// Read the persisted channel preference, defaulting to stable when the store
/// is absent, unreadable, or holds an unrecognised value.
fn read_channel(app: &AppHandle) -> Channel {
    let Ok(store) = app.store(STORE_FILE) else {
        return Channel::default();
    };
    let value = store.get(CHANNEL_KEY);
    Channel::from_pref(value.as_ref().and_then(|v| v.as_str()))
}

/// Build a channel-resolved updater. The `on_before_exit` hook runs the same
/// teardown as a normal exit right before the updater's forced process exit on
/// install: `Update::install` calls `std::process::exit`, which bypasses the
/// Tauri `RunEvent::Exit` seam, so this is the only path that winds the
/// producers, scan input, and live session down cleanly on an updater install.
fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let channel = read_channel(app);
    let endpoints = endpoints_for_channel(channel)
        .into_iter()
        .map(|raw| {
            tauri::Url::parse(&raw).map_err(|err| format!("invalid updater endpoint {raw}: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let app_for_exit = app.clone();
    app.updater_builder()
        .endpoints(endpoints)
        .map_err(|err| err.to_string())?
        .on_before_exit(move || {
            crate::run_exit_teardown(&app_for_exit);
        })
        .build()
        .map_err(|err| err.to_string())
}

/// Check the selected channel's manifest for a newer release. Returns the update
/// metadata when one is available, or `None` when up to date.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = build_updater(&app)?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo::from_update(&update))),
        Ok(None) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

/// Download the available update's signed artefact (verifying its signature),
/// emitting [`DOWNLOAD_PROGRESS_EVENT`] as bytes arrive, and hold it for a
/// subsequent `install_update`. Returns the update metadata. Errors if no update
/// is available.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let updater = build_updater(&app)?;
    let update = updater
        .check()
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "no update available".to_string())?;
    let info = UpdateInfo::from_update(&update);

    let app_for_progress = app.clone();
    let mut downloaded: usize = 0;
    let bytes = update
        .download(
            move |chunk_len, content_length| {
                downloaded = downloaded.saturating_add(chunk_len);
                let _ = app_for_progress.emit(
                    DOWNLOAD_PROGRESS_EVENT,
                    DownloadProgress {
                        downloaded,
                        content_length,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|err| err.to_string())?;

    let pending = app
        .try_state::<PendingUpdate>()
        .ok_or("updater state not ready")?;
    *pending
        .0
        .lock()
        .map_err(|_| "pending update lock poisoned".to_string())? =
        Some(PendingUpdateInner { update, bytes });
    Ok(info)
}

/// Install the previously-downloaded update and relaunch. On Windows this runs
/// the per-user MSI via msiexec and forcibly exits the process before returning
/// (relaunch is driven by the installer's auto-launch), so `restart` is reached
/// only on platforms where install returns control. Errors if nothing has been
/// downloaded.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let pending = {
        let state = app
            .try_state::<PendingUpdate>()
            .ok_or("updater state not ready")?;
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "pending update lock poisoned".to_string())?;
        guard
            .take()
            .ok_or_else(|| "no downloaded update to install".to_string())?
    };
    pending
        .update
        .install(&pending.bytes)
        .map_err(|err| err.to_string())?;
    app.restart()
}

/// The currently-selected update channel (`"stable"` or `"beta"`).
#[tauri::command]
pub fn get_update_channel(app: AppHandle) -> String {
    read_channel(&app).as_str().to_string()
}

/// Persist the selected update channel.
#[tauri::command]
pub fn set_update_channel(app: AppHandle, channel: Channel) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|err| err.to_string())?;
    store.set(CHANNEL_KEY, serde_json::Value::from(channel.as_str()));
    store.save().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{endpoints_for_channel, Channel, DownloadProgress, UpdateInfo};

    #[test]
    fn unknown_or_absent_pref_defaults_to_stable() {
        assert_eq!(Channel::from_pref(None), Channel::Stable);
        assert_eq!(Channel::from_pref(Some("")), Channel::Stable);
        assert_eq!(Channel::from_pref(Some("nightly")), Channel::Stable);
        assert_eq!(Channel::from_pref(Some("stable")), Channel::Stable);
    }

    #[test]
    fn beta_pref_selects_beta() {
        assert_eq!(Channel::from_pref(Some("beta")), Channel::Beta);
    }

    #[test]
    fn each_channel_resolves_to_its_own_signed_manifest() {
        let stable = endpoints_for_channel(Channel::Stable);
        let beta = endpoints_for_channel(Channel::Beta);
        assert_eq!(
            stable,
            vec!["https://entropiaorme.com/updates/stable/latest.json"]
        );
        assert_eq!(
            beta,
            vec!["https://entropiaorme.com/updates/beta/latest.json"]
        );
        assert_ne!(
            stable, beta,
            "the channel must change which manifest is read"
        );
    }

    #[test]
    fn endpoints_stay_on_the_csp_allowed_https_origin() {
        // The app CSP only permits https://entropiaorme.com in connect-src; an
        // endpoint off that origin would be blocked at runtime.
        for channel in [Channel::Stable, Channel::Beta] {
            for endpoint in endpoints_for_channel(channel) {
                assert!(
                    endpoint.starts_with("https://entropiaorme.com/"),
                    "{endpoint} is off the CSP-allowed origin"
                );
            }
        }
    }

    #[test]
    fn channel_round_trips_through_its_pref_string() {
        for channel in [Channel::Stable, Channel::Beta] {
            assert_eq!(Channel::from_pref(Some(channel.as_str())), channel);
        }
    }

    #[test]
    fn update_info_serialises_to_camel_case_for_the_webview() {
        let info = UpdateInfo {
            version: "0.2.0".into(),
            current_version: "0.1.0".into(),
            notes: Some("Fixes".into()),
        };
        let json = serde_json::to_value(&info).expect("serialises");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["currentVersion"], "0.1.0");
        assert_eq!(json["notes"], "Fixes");
        assert!(
            json.get("current_version").is_none(),
            "the wire contract is camelCase, not snake_case"
        );
    }

    #[test]
    fn download_progress_carries_an_optional_total() {
        let known = serde_json::to_value(DownloadProgress {
            downloaded: 1024,
            content_length: Some(4096),
        })
        .expect("serialises");
        assert_eq!(known["downloaded"], 1024);
        assert_eq!(known["contentLength"], 4096);

        let unknown = serde_json::to_value(DownloadProgress {
            downloaded: 1024,
            content_length: None,
        })
        .expect("serialises");
        assert!(
            unknown["contentLength"].is_null(),
            "an absent total surfaces as null so the UI can fall back to indeterminate"
        );
    }
}
