//! The settings family: the assembled settings read, the overlay-position
//! read/write, and the partial settings update that re-signals the live
//! producers.
//!
//! The stored `settings.json` bytes are owned by the `ConfigService`,
//! the sole writer, saving whole-file in the canonical on-disk format.
//! The read response shapes match the frontend's hand-written contract
//! (`$lib/types/settings.ts`) field for field.
//!
//! Contract lineage (ADR-0017/0019): two behaviours retired at the
//! typed-command crossing. The pydantic-era `exclude_unset` partial the HTTP layer
//! parsed is now the all-`Option` [`SettingsPatch`] DTO, so the framework
//! 422/500 envelopes it produced (a non-integer overlay coordinate, a
//! structurally-malformed `hotbar` container, an
//! unrenderable surrogate string) become unrepresentable over the typed
//! command rather than validated. And the dead `POST /api/settings/reset`
//! retires unconverted: it has no frontend caller, exactly as the
//! character codex read and the equipment cost endpoint retired with
//! their families.

use std::path::Path;

use eo_services::config_service::load_config_readonly;
use eo_services::paths::DB_FILE_NAME;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};

use crate::Nullable;
use crate::{Api, ApiError};

/// The version the settings response stamps. The crate inherits the
/// workspace version, which the version-stamp parity guard holds in
/// lock-step with the packaged artefacts.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Response DTOs ───────────────────────────────────────────────────

/// The game-connection block: the configured chat-log path, whether it
/// currently resolves to a file, and the player name.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameConnection {
    pub chat_log_path: String,
    pub chat_log_valid: bool,
    pub player_name: String,
}

/// The harvest-guardrail block: the enabled flag and the intended tool
/// id per board-output class (null while a class has no intended tool).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestGuardrailSettings {
    pub enabled: bool,
    pub short_tool_id: Nullable<i64>,
    pub long_tool_id: Nullable<i64>,
    pub huge_tool_id: Nullable<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PassiveEffectKind {
    ReloadSpeed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassiveEffectView {
    pub kind: PassiveEffectKind,
    pub magnitude_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassiveEffectSourceView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub effects: Vec<PassiveEffectView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PassiveEffectInput {
    pub kind: PassiveEffectKind,
    pub magnitude_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PassiveEffectSourceInput {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub effects: Vec<PassiveEffectInput>,
}

/// The full assembled settings response. Field order is the wire order
/// the frontend contract expects (and the HTTP body carried).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub game_connection: GameConnection,
    pub hotbar_hooks_enabled: bool,
    pub repair_ocr_enabled: bool,
    pub developer_mode_enabled: bool,
    /// The session facets the next session snapshots: the designated
    /// name (empty: not declared) and the skill boost (null: not
    /// declared; 0: declared deliberately unboosted).
    pub session_name: String,
    pub declared_skill_boost_percent: Option<i64>,
    /// The slot-to-equipment map, carried through in its stored insertion
    /// order (`serde_json`'s `preserve_order`), so slot "0" stays last.
    pub hotbar: Map<String, Value>,
    /// Weapons carried without a hotbar slot: with the slotted weapons,
    /// the candidates weapon attribution chooses among.
    pub carried_weapon_ids: Vec<i64>,
    pub passive_effect_sources: Vec<PassiveEffectSourceView>,
    pub harvest_guardrail: HarvestGuardrailSettings,
    pub loot_filter_blacklist: Vec<String>,
    pub db_path: String,
    pub app_version: String,
}

/// GET overlay-position: the persisted overlay window coordinates (null
/// until first placed).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPosition {
    pub x: Nullable<i64>,
    pub y: Nullable<i64>,
}

// ── Request DTOs ────────────────────────────────────────────────────

/// The harvest-guardrail block in a settings update. Field names stay
/// in the stored snake_case the config writer re-normalises.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HarvestGuardrailInput {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub short_tool_id: Option<i64>,
    #[serde(default)]
    pub long_tool_id: Option<i64>,
    #[serde(default)]
    pub huge_tool_id: Option<i64>,
}

/// The partial settings update: every field optional, only the present
/// ones applied (the `exclude_unset` semantics the pydantic model had).
/// `declared_skill_boost_percent` is a double option so an explicit
/// `null` (withdraw the declaration) stays distinct from an absent field
/// (leave it untouched); every other field is nullless, so a plain
/// `Option` carries the present/absent distinction.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SettingsPatch {
    #[serde(default)]
    pub chatlog_path: Option<String>,
    #[serde(default)]
    pub player_name: Option<String>,
    #[serde(default)]
    pub hotbar_hooks_enabled: Option<bool>,
    #[serde(default)]
    pub repair_ocr_enabled: Option<bool>,
    #[serde(default)]
    pub developer_mode_enabled: Option<bool>,
    #[serde(default)]
    pub session_name: Option<String>,
    /// Double-optioned so the patch can express all three states: absent
    /// leaves the declaration alone, an explicit null withdraws it, and a
    /// number (including 0) declares it.
    #[serde(default, deserialize_with = "double_option")]
    pub declared_skill_boost_percent: Option<Option<i64>>,
    #[serde(default)]
    pub hotbar: Option<Map<String, Value>>,
    #[serde(default)]
    pub carried_weapon_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub passive_effect_sources: Option<Vec<PassiveEffectSourceInput>>,
    #[serde(default)]
    pub harvest_guardrail: Option<HarvestGuardrailInput>,
    #[serde(default)]
    pub loot_filter_blacklist: Option<Vec<String>>,
}

/// Deserialize a present-but-`null` field to `Some(None)` and an absent
/// field to `None` (paired with `#[serde(default)]`), the distinction a
/// bare `Option<Option<T>>` collapses.
pub(crate) fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl SettingsPatch {
    /// The present fields as the update map the config writer applies
    /// (stored snake_case keys). Absent fields are omitted; the `hotbar`
    /// and `carried_weapon_ids` containers pass through as their raw JSON
    /// value, which the writer re-normalises.
    fn into_updates(self) -> Map<String, Value> {
        let mut updates = Map::new();
        if let Some(value) = self.chatlog_path {
            updates.insert("chatlog_path".into(), Value::String(value));
        }
        if let Some(value) = self.player_name {
            updates.insert("player_name".into(), Value::String(value));
        }
        if let Some(value) = self.hotbar_hooks_enabled {
            updates.insert("hotbar_hooks_enabled".into(), Value::Bool(value));
        }
        if let Some(value) = self.repair_ocr_enabled {
            updates.insert("repair_ocr_enabled".into(), Value::Bool(value));
        }
        if let Some(value) = self.developer_mode_enabled {
            updates.insert("developer_mode_enabled".into(), Value::Bool(value));
        }
        if let Some(value) = self.session_name {
            updates.insert("session_name".into(), Value::String(value));
            // The selection is what writes this facet, so a name arriving
            // by any other route is a free-text declaration and disavows
            // it, the same rule `tracking_session_config` applies. The
            // withdrawal reads back as the protected default, never as no
            // session at all.
            updates.insert("session_definition_id".into(), Value::Null);
        }
        if let Some(value) = self.declared_skill_boost_percent {
            updates.insert(
                "declared_skill_boost_percent".into(),
                match value {
                    Some(percent) => Value::from(percent),
                    None => Value::Null,
                },
            );
        }
        if let Some(value) = self.hotbar {
            updates.insert("hotbar".into(), Value::Object(value));
        }
        if let Some(value) = self.carried_weapon_ids {
            updates.insert("carried_weapon_ids".into(), json!(value));
        }
        if let Some(value) = self.passive_effect_sources {
            updates.insert("passive_effect_sources".into(), json!(value));
        }
        if let Some(value) = self.harvest_guardrail {
            updates.insert("harvest_guardrail".into(), json!(value));
        }
        if let Some(value) = self.loot_filter_blacklist {
            updates.insert("loot_filter_blacklist".into(), json!(value));
        }
        updates
    }
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// The full settings assembly: the config fields, the live chat-log
    /// validity, the resolved db path, and the version stamp. Reads the config fresh from disk, so a read
    /// after a write is coherent (the writer saves before responding).
    pub async fn settings(&self) -> Result<AppSettings, ApiError> {
        let config = load_config_readonly(&self.data_dir)
            .map_err(ApiError::internal("settings config read"))?;
        Ok(AppSettings {
            game_connection: GameConnection {
                chat_log_path: config.chatlog_path.clone(),
                chat_log_valid: Path::new(&config.chatlog_path).is_file(),
                player_name: config.player_name.clone(),
            },
            hotbar_hooks_enabled: config.hotbar_hooks_enabled,
            repair_ocr_enabled: config.repair_ocr_enabled,
            developer_mode_enabled: config.developer_mode_enabled,
            session_name: config.session_name.clone(),
            declared_skill_boost_percent: config
                .declared_skill_boost_percent
                .filter(|percent| *percent >= 0),
            hotbar: config.hotbar.clone(),
            carried_weapon_ids: config.carried_weapon_ids.clone(),
            passive_effect_sources: config
                .passive_effect_sources
                .iter()
                .map(|source| PassiveEffectSourceView {
                    id: source.id.clone(),
                    name: source.name.clone(),
                    enabled: source.enabled,
                    effects: source
                        .effects
                        .iter()
                        .map(|effect| PassiveEffectView {
                            kind: match effect.kind {
                                eo_services::passive_effects::PassiveEffectKind::ReloadSpeed => {
                                    PassiveEffectKind::ReloadSpeed
                                }
                            },
                            magnitude_percent: effect.magnitude_percent,
                        })
                        .collect(),
                })
                .collect(),
            harvest_guardrail: HarvestGuardrailSettings {
                enabled: config.harvest_guardrail.enabled,
                short_tool_id: config.harvest_guardrail.short_tool_id.into(),
                long_tool_id: config.harvest_guardrail.long_tool_id.into(),
                huge_tool_id: config.harvest_guardrail.huge_tool_id.into(),
            },
            loot_filter_blacklist: config.loot_filter_blacklist.clone(),
            db_path: python_path_str(&self.data_dir.join(DB_FILE_NAME)),
            app_version: APP_VERSION.to_string(),
        })
    }

    /// The persisted overlay window position.
    pub async fn settings_overlay_position(&self) -> Result<OverlayPosition, ApiError> {
        let config = load_config_readonly(&self.data_dir)
            .map_err(ApiError::internal("overlay position read"))?;
        Ok(OverlayPosition {
            x: config.overlay_x.into(),
            y: config.overlay_y.into(),
        })
    }

    /// Persist the overlay window position. Unlike the PATCH / reset
    /// writes it carries no producer side effects, so no watcher / hotbar
    /// / tracker signal follows.
    ///
    /// The coordinates are bounds-checked before persistence: a client
    /// that reports a nonsense position (the failure mode observed on a
    /// Wayland backend, where a window cannot read its own global
    /// position and hands back a degenerate value) must not be able to
    /// poison the store with a location no monitor could hold, which
    /// would leave the overlay stranded off-screen on the next restore.
    /// The frontend owns the richer, monitor-geometry-aware guard; this
    /// is the backend's defence-in-depth sanity bound.
    pub async fn settings_set_overlay_position(&self, x: i64, y: i64) -> Result<(), ApiError> {
        if !is_plausible_overlay_position(x, y) {
            return Err(ApiError::bad_request(format!(
                "overlay position ({x}, {y}) is outside the plausible desktop bounds"
            )));
        }
        let mut updates = Map::new();
        updates.insert("overlay_x".into(), json!(x));
        updates.insert("overlay_y".into(), json!(y));
        let mut guard = self
            .config_service
            .lock()
            .map_err(|_| ApiError::invalid_state("config service lock poisoned"))?;
        guard
            .update(&updates)
            .map_err(ApiError::internal("overlay position write"))?;
        Ok(())
    }

    /// Apply a partial settings update: validate and write the present
    /// fields, signal the producers (the watcher on a `chatlog_path`
    /// change, the hotbar gate on a `hotbar_hooks_enabled` change, the
    /// tracker unconditionally so an in-flight session re-reads its
    /// config), and reply with the full assembled settings.
    pub async fn settings_update(&self, patch: SettingsPatch) -> Result<AppSettings, ApiError> {
        let disables_developer_mode = patch.developer_mode_enabled == Some(false);
        if let Some(sources) = patch.passive_effect_sources.as_deref() {
            validate_passive_effect_sources(sources)?;
        }
        let mut updates = patch.into_updates();
        // An empty patch is the backend's 400 (nothing to update).
        if updates.is_empty() {
            return Err(ApiError::bad_request("No fields to update"));
        }
        // Lock, validate, and write inside a block so the (non-`Send`)
        // guard is gone before the `.await` below (the response assembly).
        let (validated_chatlog, hooks_present, hooks_value) = {
            let mut guard = self
                .config_service
                .lock()
                .map_err(|_| ApiError::invalid_state("config service lock poisoned"))?;
            // The candidate validates without mutating live state (the
            // backend's `clone_with_updates`); used for the mob-mode gate.
            let candidate = guard.clone_with_updates(&updates);
            // chatlog_path first (the backend's order): the 400 chain, then
            // the validated/expanduser-normalised path replaces the
            // submitted one so the write and the watcher restart use the
            // canonical form.
            let validated_chatlog = if updates.contains_key("chatlog_path") {
                let raw = updates
                    .get("chatlog_path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let normalised = validate_chatlog_path(raw)?;
                updates.insert("chatlog_path".into(), Value::String(normalised.clone()));
                Some(normalised)
            } else {
                None
            };
            if candidate
                .declared_skill_boost_percent
                .is_some_and(|percent| percent < 0)
            {
                return Err(ApiError::bad_request("Skill boost cannot be negative"));
            }
            let hooks_present = updates.contains_key("hotbar_hooks_enabled");
            guard
                .update(&updates)
                .map_err(ApiError::internal("settings write"))?;
            (
                validated_chatlog,
                hooks_present,
                guard.get().hotbar_hooks_enabled,
            )
        };

        if let Some(path) = validated_chatlog {
            self.watcher.restart(path);
        }
        if hooks_present {
            self.hotbar.set_hotbar_hooks_enabled(hooks_value);
        }
        if disables_developer_mode {
            self.stop_auction_fee_research_for_shell();
        }
        self.tracker.reload_config().await;
        self.settings().await
    }
}

fn validate_passive_effect_sources(sources: &[PassiveEffectSourceInput]) -> Result<(), ApiError> {
    let mut ids = std::collections::BTreeSet::new();
    let mut enabled_reload_speed = 0.0;
    for source in sources {
        if source.id.trim().is_empty() || source.name.trim().is_empty() {
            return Err(ApiError::bad_request(
                "Passive effect sources require an id and name",
            ));
        }
        if !ids.insert(source.id.trim()) {
            return Err(ApiError::bad_request(
                "Passive effect source ids must be unique",
            ));
        }
        if source.effects.is_empty() {
            return Err(ApiError::bad_request(
                "Passive effect sources require at least one effect",
            ));
        }
        for effect in &source.effects {
            if !effect.magnitude_percent.is_finite() {
                return Err(ApiError::bad_request(
                    "Passive effect magnitudes must be finite",
                ));
            }
            if source.enabled && matches!(effect.kind, PassiveEffectKind::ReloadSpeed) {
                enabled_reload_speed += effect.magnitude_percent;
            }
        }
    }
    if !enabled_reload_speed.is_finite() {
        return Err(ApiError::bad_request(
            "Combined reload speed is too large to apply",
        ));
    }
    if enabled_reload_speed <= -100.0 {
        return Err(ApiError::bad_request(
            "Combined reload speed must be greater than -100%",
        ));
    }
    Ok(())
}

/// Mirror the backend's `_validate_chatlog_path`: a non-empty path whose
/// basename is `chat.log` (case-insensitive) and which is an existing
/// file. Returns the expanduser-normalised `str(Path(...))` on success
/// (so the stored value and the watcher restart both use the canonical
/// form), or the bad-request the frontend renders inline.
fn validate_chatlog_path(value: &str) -> Result<String, ApiError> {
    if value.is_empty() {
        return Err(ApiError::bad_request("chat.log path is required"));
    }
    let expanded = expanduser(value);
    let path = Path::new(&expanded);
    let basename_is_chatlog = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("chat.log"));
    if !basename_is_chatlog {
        return Err(ApiError::bad_request(
            "chat.log path must point to a chat.log file",
        ));
    }
    if !path.is_file() {
        return Err(ApiError::bad_request("chat.log path does not exist"));
    }
    Ok(python_path_str(path))
}

/// Expand a leading `~` to the user's home directory, mirroring
/// `pathlib.Path.expanduser` for the case the path picker produces (a
/// bare `~` or a `~/...` / `~\...` prefix). Other forms pass through
/// unchanged.
fn expanduser(value: &str) -> String {
    if let Some(rest) = value.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
            if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
            {
                return format!("{}{}", home.to_string_lossy(), rest);
            }
        }
    }
    value.to_string()
}

/// `str(pathlib.Path(...))` over the absolute forms the data-dir
/// resolution produces: Windows renders every separator as a backslash (a
/// forward-slash env override still reads back in the native form, as the
/// Python reference's `pathlib` normalisation does); other platforms keep
/// the path as built.
/// The inclusive coordinate bound (per axis) a persisted overlay
/// position must fall within. X11 window coordinates are `i16`, and any
/// real multi-monitor desktop (including monitors placed left of or
/// above the primary, hence negative origins) fits inside this range;
/// a value beyond it is corruption, not a reachable window location.
const OVERLAY_COORD_BOUND: i64 = 32_767;

/// Whether `(x, y)` is a position some monitor on a real desktop could
/// hold. Pure and total so the guard is unit-testable without a config
/// service; both axes must be within `±OVERLAY_COORD_BOUND`.
fn is_plausible_overlay_position(x: i64, y: i64) -> bool {
    (-OVERLAY_COORD_BOUND..=OVERLAY_COORD_BOUND).contains(&x)
        && (-OVERLAY_COORD_BOUND..=OVERLAY_COORD_BOUND).contains(&y)
}

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

    #[test]
    fn overlay_position_guard_accepts_real_desktop_coordinates() {
        // Origin, a large multi-monitor offset, and negative origins
        // (a monitor left of / above the primary) are all reachable.
        assert!(is_plausible_overlay_position(0, 0));
        assert!(is_plausible_overlay_position(40, 40));
        assert!(is_plausible_overlay_position(5120, 1440));
        assert!(is_plausible_overlay_position(-1920, -1080));
        assert!(is_plausible_overlay_position(32_767, -32_767));
    }

    #[test]
    fn overlay_position_guard_rejects_corruption() {
        // Values no monitor could hold: the store must not be poisoned
        // with a location that would strand the overlay off-screen.
        assert!(!is_plausible_overlay_position(32_768, 0));
        assert!(!is_plausible_overlay_position(0, -32_768));
        assert!(!is_plausible_overlay_position(i64::MAX, i64::MIN));
        assert!(!is_plausible_overlay_position(1_000_000, 1_000_000));
    }

    #[test]
    fn an_absent_field_is_omitted_while_an_explicit_null_boost_withdraws() {
        // A bare Option field: absent stays absent.
        let patch = SettingsPatch {
            player_name: Some("Mikel".into()),
            ..SettingsPatch::default()
        };
        let updates = patch.into_updates();
        assert_eq!(updates.get("player_name"), Some(&json!("Mikel")));
        assert!(!updates.contains_key("chatlog_path"));

        // The double-option boost: present-null lands as a null in the
        // update map (withdraw the declaration), distinct from absent.
        let cleared: SettingsPatch =
            serde_json::from_value(json!({ "declared_skill_boost_percent": null })).unwrap();
        assert_eq!(
            cleared.into_updates().get("declared_skill_boost_percent"),
            Some(&Value::Null)
        );
        let untouched: SettingsPatch = serde_json::from_value(json!({})).unwrap();
        assert!(!untouched
            .into_updates()
            .contains_key("declared_skill_boost_percent"));

        // Carried weapons pass through as the raw list the writer
        // re-normalises; the retired preset keys are no longer accepted
        // into the update.
        let carried: SettingsPatch =
            serde_json::from_value(json!({ "carried_weapon_ids": [4, 4, 7] })).unwrap();
        assert_eq!(
            carried.into_updates().get("carried_weapon_ids"),
            Some(&json!([4, 4, 7]))
        );
        let retired: SettingsPatch =
            serde_json::from_value(json!({ "active_trifecta_preset_id": "p" })).unwrap();
        assert!(retired.into_updates().is_empty());
    }
}
