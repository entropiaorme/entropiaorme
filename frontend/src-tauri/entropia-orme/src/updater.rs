//! Auto-updater plumbing: stable/beta channel resolution and the update-check
//! command, for signed update manifests served from entropiaorme.com.
//!
//! The channel preference is persisted via tauri-plugin-store; at check time the
//! updater's endpoints are resolved from that preference (the config-level
//! endpoint in `tauri.conf.json` is the stable default, the runtime override
//! selects the channel's manifest). Integrity rests on two independent guards
//! that this wiring relies on rather than reimplements:
//!
//! - **Tamper:** the updater verifies the manifest signature against the public
//!   key in `tauri.conf.json`; an unsigned or wrongly-signed manifest is
//!   rejected. The matching private key is provisioned out of band (a CI signing
//!   secret), never in the tree.
//! - **Downgrade:** the updater only offers a release whose version is strictly
//!   greater than the running one, so a replayed older (still validly-signed)
//!   manifest cannot roll the install back.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::UpdaterExt;

/// The store file holding the updater preference. A dedicated file (not the
/// shared frontend prefs store) keeps this plumbing self-contained.
const STORE_FILE: &str = "updater.json";
/// The store key holding the selected update channel.
const CHANNEL_KEY: &str = "channel";

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

/// Check the selected channel's manifest for a newer release. Returns the new
/// version string when an update is available, or `None` when up to date.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<String>, String> {
    let channel = read_channel(&app);
    let endpoints = endpoints_for_channel(channel)
        .into_iter()
        .map(|raw| {
            tauri::Url::parse(&raw).map_err(|err| format!("invalid updater endpoint {raw}: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let updater = app
        .updater_builder()
        .endpoints(endpoints)
        .map_err(|err| err.to_string())?
        .build()
        .map_err(|err| err.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version)),
        Ok(None) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
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
    use super::{endpoints_for_channel, Channel};

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
}
