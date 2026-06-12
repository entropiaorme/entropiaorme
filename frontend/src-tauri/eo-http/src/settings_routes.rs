//! Natively-served settings READ handlers, byte-faithful to
//! `backend/routers/settings.py`: the assembled `AppSettings` response
//! (config fields, live chat-log validation, per-preset trifecta
//! readiness) and the overlay position.
//!
//! Only the reads serve natively this round. The sidecar's
//! configuration service caches the whole config in memory and saves
//! whole-file from that cache, so a second writer to `settings.json`
//! would lose updates; the settings writes also signal live sidecar
//! producers (watcher restart, hotbar listener, tracker reload). All
//! three write routes therefore stay proxied until the producer
//! cutover, and these handlers read the file fresh per request: the
//! sidecar saves before it responds, so a read-through is coherent.

use std::path::Path;

use axum::body::Body;
use axum::http::Response;
use eo_services::config_service::{load_config_readonly, AppConfig};
use eo_services::paths::DB_FILE_NAME;
use eo_services::trifecta_service::{validate_trifecta, TrifectaPreset};
use serde_json::{json, Map, Value};

use crate::hydration::{internal_error, plain_json_response, HydrationState};

/// The version the backend stamps into the settings response. The
/// crate inherits the workspace version, which the version-stamp
/// parity guard holds in lock-step with the packaged artefacts.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

impl HydrationState {
    /// GET /api/settings: the full settings assembly.
    /// (The ETag middleware scopes to the tracking/scan/quests/codex
    /// prefixes; settings reads answer plain 200s, validators ignored.)
    pub async fn settings(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let Ok(config) = load_config_readonly(&self.data_dir) else {
            return internal_error();
        };
        let trifecta = match self.trifecta_block(&config).await {
            Ok(block) => block,
            Err(_) => return internal_error(),
        };

        let mut out = Map::new();
        out.insert(
            "gameConnection".into(),
            json!({
                "chatLogPath": config.chatlog_path,
                "chatLogValid": Path::new(&config.chatlog_path).is_file(),
                "playerName": config.player_name,
            }),
        );
        out.insert(
            "hotbarHooksEnabled".into(),
            json!(config.hotbar_hooks_enabled),
        );
        out.insert("repairOcrEnabled".into(), json!(config.repair_ocr_enabled));
        out.insert(
            "endOfSessionArmourReminderEnabled".into(),
            json!(config.end_of_session_armour_reminder_enabled),
        );
        out.insert(
            "developerModeEnabled".into(),
            json!(config.developer_mode_enabled),
        );
        out.insert("mobTrackingMode".into(), json!(config.mob_tracking_mode));
        out.insert("mobTrackingTag".into(), json!(config.mob_tracking_tag));
        out.insert("hotbar".into(), Value::Object(config.hotbar.clone()));
        out.insert("trifecta".into(), trifecta);
        out.insert(
            "lootFilterBlacklist".into(),
            json!(config.loot_filter_blacklist),
        );
        out.insert(
            "dbPath".into(),
            json!(python_path_str(&self.data_dir.join(DB_FILE_NAME))),
        );
        out.insert("appVersion".into(), json!(APP_VERSION));
        plain_json_response(&Value::Object(out))
    }

    /// GET /api/settings/overlay-position.
    pub async fn overlay_position(&self, _if_none_match: Option<&str>) -> Response<Body> {
        let Ok(config) = load_config_readonly(&self.data_dir) else {
            return internal_error();
        };
        plain_json_response(&json!({"x": config.overlay_x, "y": config.overlay_y}))
    }

    /// The trifecta block: every preset validated against the live
    /// equipment library, with the active preset's readiness lifted to
    /// the top level, mirroring `_build_trifecta_response`.
    async fn trifecta_block(&self, config: &AppConfig) -> Result<Value, eo_services::db::DbError> {
        let mut presets = Vec::new();
        let mut active_ready = false;
        let mut active_message: Option<String> = None;
        let mut active_name: Option<String> = None;

        for preset in &config.trifecta_presets {
            let service_preset = TrifectaPreset {
                small_weapon_id: preset.small_weapon_id,
                big_weapon_id: preset.big_weapon_id,
                heal_id: preset.heal_id,
            };
            let (ready, message) = validate_trifecta(&self.db, Some(&service_preset)).await?;
            presets.push(json!({
                "id": preset.id,
                "name": preset.name,
                "smallWeaponId": preset.small_weapon_id,
                "bigWeaponId": preset.big_weapon_id,
                "healId": preset.heal_id,
                "ready": ready,
                "message": message,
            }));
            if Some(preset.id.as_str()) == config.active_trifecta_preset_id.as_deref() {
                active_ready = ready;
                active_message = message;
                active_name = Some(preset.name.clone());
            }
        }

        Ok(json!({
            "activePresetId": config.active_trifecta_preset_id,
            "activePresetName": active_name,
            "presets": presets,
            "ready": active_ready,
            "message": active_message,
        }))
    }
}

/// `str(pathlib.Path(...))` over the absolute forms the data-dir
/// resolution produces: Windows renders every separator as a
/// backslash (a forward-slash env override still reads back in the
/// native form, as the sidecar's `pathlib` normalisation does); other
/// platforms keep the path as built.
fn python_path_str(path: &Path) -> String {
    #[cfg(windows)]
    {
        use std::path::Component;
        let mut out = String::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => {
                    out.push_str(&prefix.as_os_str().to_string_lossy());
                }
                Component::RootDir => out.push('\\'),
                part => {
                    if !out.is_empty() && !out.ends_with('\\') {
                        out.push('\\');
                    }
                    out.push_str(&part.as_os_str().to_string_lossy());
                }
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn paths_render_in_the_native_windows_form() {
        assert_eq!(
            python_path_str(Path::new("E:/x/data/entropia_orme.db")),
            "E:\\x\\data\\entropia_orme.db",
        );
        assert_eq!(
            python_path_str(Path::new("E:\\already\\native")),
            "E:\\already\\native",
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn paths_render_as_built() {
        assert_eq!(
            python_path_str(Path::new("/tmp/data/x.db")),
            "/tmp/data/x.db"
        );
    }
}
