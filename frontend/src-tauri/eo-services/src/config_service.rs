//! Configuration service, ported from the original Python implementation:
//! typed settings with atomic persistence.
//!
//! Settings live as JSON in `data/settings.json`. Saves are atomic
//! (write `.tmp`, swap into place, keep `.bak`) and merge with whatever
//! is on disk at save time, so keys written by other tooling survive a
//! save by a process that does not know them; the unknown keys are also
//! carried as a typed catch-all on the loaded config, making the
//! carry-forward contract visible. The on-disk byte shape matches the
//! backend's writer: ASCII-escaped JSON, two-space indent, stored key
//! positions preserved on merge, platform line endings.
//!
//! Update semantics mirror the backend service: unknown update keys are
//! ignored; the hotbar always re-normalises to its full slot shape; the
//! trifecta preset list re-validates its active id. Where a stored or
//! submitted value does not fit its typed field, this implementation
//! coalesces or skips instead of carrying the raw value; the divergence
//! register's configuration entry records those cases and their
//! reachability.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const HOTBAR_SLOTS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
pub const DEFAULT_TRIFECTA_PRESET_ID: &str = "default";
pub const DEFAULT_TRIFECTA_PRESET_NAME: &str = "Default";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrifectaPresetConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub small_weapon_id: Option<i64>,
    #[serde(default)]
    pub big_weapon_id: Option<i64>,
    #[serde(default)]
    pub heal_id: Option<i64>,
}

impl TrifectaPresetConfig {
    fn default_preset() -> Self {
        Self {
            id: DEFAULT_TRIFECTA_PRESET_ID.to_string(),
            name: DEFAULT_TRIFECTA_PRESET_NAME.to_string(),
            small_weapon_id: None,
            big_weapon_id: None,
            heal_id: None,
        }
    }
}

/// All user-configurable settings; field order is the serialised order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub chatlog_path: String,
    pub player_name: String,
    pub hotbar_hooks_enabled: bool,
    pub repair_ocr_enabled: bool,
    pub end_of_session_armour_reminder_enabled: bool,
    pub developer_mode_enabled: bool,
    pub mob_tracking_mode: String,
    pub mob_tracking_tag: String,
    pub manual_mob_species: String,
    pub manual_mob_maturity: String,
    pub hotbar: Map<String, Value>,
    pub trifecta_presets: Vec<TrifectaPresetConfig>,
    pub active_trifecta_preset_id: Option<String>,
    pub loot_filter_blacklist: Vec<String>,
    pub overlay_x: Option<i64>,
    pub overlay_y: Option<i64>,
    /// Unknown keys read from disk: the visible carry-forward contract.
    /// Excluded from the known-field serialisation; persistence comes
    /// from the save path re-reading the file and merging by position,
    /// so even keys written after this load survive.
    #[serde(skip)]
    pub extra: Map<String, Value>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut hotbar = Map::new();
        for slot in HOTBAR_SLOTS {
            hotbar.insert(slot.to_string(), Value::Null);
        }
        Self {
            chatlog_path: String::new(),
            player_name: String::new(),
            hotbar_hooks_enabled: false,
            repair_ocr_enabled: false,
            end_of_session_armour_reminder_enabled: false,
            developer_mode_enabled: false,
            mob_tracking_mode: "mob".to_string(),
            mob_tracking_tag: String::new(),
            manual_mob_species: String::new(),
            manual_mob_maturity: String::new(),
            hotbar,
            trifecta_presets: vec![TrifectaPresetConfig::default_preset()],
            active_trifecta_preset_id: Some(DEFAULT_TRIFECTA_PRESET_ID.to_string()),
            loot_filter_blacklist: vec!["Universal Ammo".to_string()],
            overlay_x: None,
            overlay_y: None,
            extra: Map::new(),
        }
    }
}

impl AppConfig {
    /// The default chat-log location under the user's home directory.
    pub fn default_chatlog_path() -> String {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join("Documents")
            .join("Entropia Universe")
            .join("chat.log")
            .to_string_lossy()
            .into_owned()
    }
}

/// The currently active trifecta preset, or None when not resolvable.
pub fn active_trifecta_preset(config: &AppConfig) -> Option<&TrifectaPresetConfig> {
    let active_id = config.active_trifecta_preset_id.as_deref()?;
    if active_id.is_empty() {
        return None;
    }
    config
        .trifecta_presets
        .iter()
        .find(|preset| preset.id == active_id)
}

/// Load the stored config without writing anything: the read-through
/// for a process that serves `settings.json` reads while another
/// process owns its writes. A missing or unparseable file reads as the
/// defaults (with the home chat-log path) WITHOUT persisting them; a
/// parseable non-object errors exactly as [`ConfigService::new`] does.
pub fn load_config_readonly(data_dir: &Path) -> std::io::Result<AppConfig> {
    let config_path = data_dir.join("settings.json");
    if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path)?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(data)) => return Ok(from_stored(&data)),
            Ok(_) => {
                return Err(std::io::Error::other(
                    "settings file does not contain an object",
                ));
            }
            Err(_) => {}
        }
    }
    Ok(AppConfig {
        chatlog_path: AppConfig::default_chatlog_path(),
        ..AppConfig::default()
    })
}

pub struct ConfigService {
    config_path: PathBuf,
    config: AppConfig,
}

impl ConfigService {
    pub fn new(data_dir: &Path) -> std::io::Result<Self> {
        let config_path = data_dir.join("settings.json");
        let mut service = Self {
            config_path,
            config: AppConfig::default(),
        };
        service.config = service.load()?;
        Ok(service)
    }

    fn load(&self) -> std::io::Result<AppConfig> {
        if self.config_path.exists() {
            // Read failures fail loudly (the backend's would too): a
            // transient lock must never silently reset user settings.
            let raw = std::fs::read_to_string(&self.config_path)?;
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(data)) => return Ok(from_stored(&data)),
                Ok(_) => {
                    // A parseable file of the wrong shape crashes the
                    // backend's loader rather than resetting; mirror it.
                    return Err(std::io::Error::other(
                        "settings file does not contain an object",
                    ));
                }
                Err(_) => {
                    // Unparseable JSON: recovered with saved defaults on
                    // both implementations.
                }
            }
        }
        let config = AppConfig {
            chatlog_path: AppConfig::default_chatlog_path(),
            ..AppConfig::default()
        };
        self.save(&config)?;
        Ok(config)
    }

    pub fn get(&self) -> &AppConfig {
        &self.config
    }

    /// A candidate config with the updates applied, leaving the live
    /// config untouched (round-trips through the stored representation
    /// first, exactly as the backend's clone path does).
    pub fn clone_with_updates(&self, updates: &Map<String, Value>) -> AppConfig {
        let mut candidate = from_stored(&known_fields(&self.config));
        candidate.extra = self.config.extra.clone();
        apply_updates(&mut candidate, updates);
        candidate
    }

    /// Apply partial updates (unknown keys ignored) and save.
    pub fn update(&mut self, updates: &Map<String, Value>) -> std::io::Result<&AppConfig> {
        apply_updates(&mut self.config, updates);
        self.save_current()?;
        Ok(&self.config)
    }

    /// Restore defaults (with the default chat-log path) and save.
    pub fn reset(&mut self) -> std::io::Result<&AppConfig> {
        self.config = AppConfig {
            chatlog_path: AppConfig::default_chatlog_path(),
            ..AppConfig::default()
        };
        self.save_current()?;
        Ok(&self.config)
    }

    /// Whether the configured chat-log path exists and is a file.
    pub fn validate_chatlog(&self) -> bool {
        Path::new(&self.config.chatlog_path).is_file()
    }

    fn save_current(&self) -> std::io::Result<()> {
        self.save(&self.config)
    }

    /// Atomic save: write `.tmp`, swap into place, keep `.bak`. Merges
    /// with any keys already on disk so values written by other tooling
    /// survive, keeping their stored positions.
    fn save(&self, config: &AppConfig) -> std::io::Result<()> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = self.config_path.with_extension("tmp");
        let bak_path = self.config_path.with_extension("bak");

        let mut merged: Map<String, Value> = Map::new();
        if self.config_path.exists() {
            if let Ok(raw) = std::fs::read_to_string(&self.config_path) {
                if let Ok(Value::Object(existing)) = serde_json::from_str::<Value>(&raw) {
                    merged = existing;
                }
            }
        }
        for (key, value) in known_fields(config) {
            merged.insert(key, value);
        }

        let mut body = to_ascii_pretty(&Value::Object(merged));
        if cfg!(windows) {
            body = body.replace('\n', "\r\n");
        }
        std::fs::write(&tmp_path, body)?;

        if self.config_path.exists() {
            let _ = std::fs::rename(&self.config_path, &bak_path);
        }
        std::fs::rename(&tmp_path, &self.config_path)?;
        Ok(())
    }
}

/// The known fields in declaration order, as the stored representation.
fn known_fields(config: &AppConfig) -> Map<String, Value> {
    match serde_json::to_value(config).expect("config serialises") {
        Value::Object(map) => map,
        _ => unreachable!("a struct serialises to an object"),
    }
}

/// Reconstruct a config from stored JSON, handling missing, extra, and
/// malformed fields exactly as the backend does.
fn from_stored(data: &Map<String, Value>) -> AppConfig {
    let defaults = AppConfig::default();
    let string_or = |key: &str, fallback: &str| -> String {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };
    let toggle = |key: &str| -> bool {
        // `bool(data.get(key, False))`: any truthy JSON value enables.
        data.get(key).map(json_truthy).unwrap_or(false)
    };
    let (trifecta_presets, active_id) = normalize_trifecta_presets(
        data.get("trifecta_presets"),
        data.get("active_trifecta_preset_id")
            .and_then(Value::as_str),
    );
    let known: std::collections::BTreeSet<&str> = KNOWN_KEYS.iter().copied().collect();
    let extra: Map<String, Value> = data
        .iter()
        .filter(|(key, _)| !known.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    AppConfig {
        chatlog_path: string_or("chatlog_path", &AppConfig::default_chatlog_path()),
        player_name: string_or("player_name", ""),
        hotbar_hooks_enabled: toggle("hotbar_hooks_enabled"),
        repair_ocr_enabled: toggle("repair_ocr_enabled"),
        end_of_session_armour_reminder_enabled: toggle("end_of_session_armour_reminder_enabled"),
        developer_mode_enabled: toggle("developer_mode_enabled"),
        mob_tracking_mode: string_or("mob_tracking_mode", "mob"),
        mob_tracking_tag: string_or("mob_tracking_tag", ""),
        manual_mob_species: string_or("manual_mob_species", ""),
        manual_mob_maturity: string_or("manual_mob_maturity", ""),
        hotbar: normalize_hotbar(data.get("hotbar")),
        trifecta_presets,
        active_trifecta_preset_id: Some(active_id),
        loot_filter_blacklist: data
            .get("loot_filter_blacklist")
            .and_then(|v| {
                v.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
            })
            .unwrap_or(defaults.loot_filter_blacklist),
        overlay_x: data.get("overlay_x").and_then(Value::as_i64),
        overlay_y: data.get("overlay_y").and_then(Value::as_i64),
        extra,
    }
}

const KNOWN_KEYS: [&str; 16] = [
    "chatlog_path",
    "player_name",
    "hotbar_hooks_enabled",
    "repair_ocr_enabled",
    "end_of_session_armour_reminder_enabled",
    "developer_mode_enabled",
    "mob_tracking_mode",
    "mob_tracking_tag",
    "manual_mob_species",
    "manual_mob_maturity",
    "hotbar",
    "trifecta_presets",
    "active_trifecta_preset_id",
    "loot_filter_blacklist",
    "overlay_x",
    "overlay_y",
];

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Fill any missing hotbar slots so the config always carries the full
/// 1-9,0 shape, in slot order.
fn normalize_hotbar(raw: Option<&Value>) -> Map<String, Value> {
    let source = raw.and_then(Value::as_object);
    let mut hotbar = Map::new();
    for slot in HOTBAR_SLOTS {
        let value = source
            .and_then(|map| map.get(slot))
            .cloned()
            .unwrap_or(Value::Null);
        hotbar.insert(slot.to_string(), value);
    }
    hotbar
}

/// Normalise a stored or submitted preset list: dict entries need a
/// non-empty trimmed id, names fall back to their position, duplicate
/// ids keep the first occurrence, an empty result becomes the default
/// preset, and the active id must name a surviving preset.
fn normalize_trifecta_presets(
    raw: Option<&Value>,
    active_id: Option<&str>,
) -> (Vec<TrifectaPresetConfig>, String) {
    let mut presets: Vec<TrifectaPresetConfig> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(Value::Array(entries)) = raw {
        for (index, entry) in entries.iter().enumerate() {
            let Some(object) = entry.as_object() else {
                continue;
            };
            // `str(raw.get("id") or "")`: any FALSY id (null, false, 0,
            // 0.0, "", empty containers) collapses to the empty string
            // and the entry is skipped.
            let id = object
                .get("id")
                .filter(|v| json_truthy(v))
                .and_then(stringify)
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() {
                continue;
            }
            let name_raw = object
                .get("name")
                .filter(|v| json_truthy(v))
                .and_then(stringify)
                .unwrap_or_default()
                .trim()
                .to_string();
            let name = if name_raw.is_empty() {
                format!("Preset {}", index + 1)
            } else {
                name_raw
            };
            if seen.contains(&id) {
                continue;
            }
            seen.insert(id.clone());
            presets.push(TrifectaPresetConfig {
                id,
                name,
                small_weapon_id: object.get("small_weapon_id").and_then(Value::as_i64),
                big_weapon_id: object.get("big_weapon_id").and_then(Value::as_i64),
                heal_id: object.get("heal_id").and_then(Value::as_i64),
            });
        }
    }
    if presets.is_empty() {
        presets.push(TrifectaPresetConfig::default_preset());
    }
    let active = match active_id {
        Some(candidate) if presets.iter().any(|p| p.id == candidate) => candidate.to_string(),
        _ => presets[0].id.clone(),
    };
    (presets, active)
}

/// Python `str(value)` over the scalar JSON shapes a stored id or name
/// can take (strings pass through; booleans and numbers render as
/// Python renders them). Container-typed ids and names are skipped
/// rather than repr-rendered; see the divergence register.
fn stringify(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(true) => Some("True".to_string()),
        Value::Bool(false) => Some("False".to_string()),
        Value::Number(n) => Some(if let Some(i) = n.as_i64() {
            i.to_string()
        } else if let Some(u) = n.as_u64() {
            u.to_string()
        } else {
            eo_wire::normalizer::python_repr_f64(n.as_f64()?)
        }),
        _ => None,
    }
}

/// Apply partial updates: unknown keys are ignored; hotbar and preset
/// updates re-normalise; a value that does not fit its field is skipped.
fn apply_updates(config: &mut AppConfig, updates: &Map<String, Value>) {
    for (key, value) in updates {
        match key.as_str() {
            "chatlog_path" => assign_string(&mut config.chatlog_path, value),
            "player_name" => assign_string(&mut config.player_name, value),
            "hotbar_hooks_enabled" => assign_bool(&mut config.hotbar_hooks_enabled, value),
            "repair_ocr_enabled" => assign_bool(&mut config.repair_ocr_enabled, value),
            "end_of_session_armour_reminder_enabled" => {
                assign_bool(&mut config.end_of_session_armour_reminder_enabled, value)
            }
            "developer_mode_enabled" => assign_bool(&mut config.developer_mode_enabled, value),
            "mob_tracking_mode" => assign_string(&mut config.mob_tracking_mode, value),
            "mob_tracking_tag" => assign_string(&mut config.mob_tracking_tag, value),
            "manual_mob_species" => assign_string(&mut config.manual_mob_species, value),
            "manual_mob_maturity" => assign_string(&mut config.manual_mob_maturity, value),
            "hotbar" => config.hotbar = normalize_hotbar(Some(value)),
            "trifecta_presets" => {
                let (presets, active) = normalize_trifecta_presets(
                    Some(value),
                    config.active_trifecta_preset_id.as_deref(),
                );
                config.trifecta_presets = presets;
                config.active_trifecta_preset_id = Some(active);
            }
            "active_trifecta_preset_id" => {
                config.active_trifecta_preset_id = match value {
                    Value::Null => None,
                    Value::String(s) => Some(s.clone()),
                    _ => continue,
                };
            }
            "loot_filter_blacklist" => {
                if let Some(items) = value.as_array() {
                    config.loot_filter_blacklist = items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                }
            }
            "overlay_x" => config.overlay_x = value.as_i64(),
            "overlay_y" => config.overlay_y = value.as_i64(),
            _ => {}
        }
    }
    if updates.contains_key("trifecta_presets") || updates.contains_key("active_trifecta_preset_id")
    {
        ensure_active_trifecta_preset(config);
    }
}

fn assign_string(slot: &mut String, value: &Value) {
    if let Some(s) = value.as_str() {
        *slot = s.to_string();
    }
}

fn assign_bool(slot: &mut bool, value: &Value) {
    if let Some(b) = value.as_bool() {
        *slot = b;
    }
}

/// When the active id no longer resolves, the preset list collapses to
/// the default preset, exactly as the backend's fallback does.
fn ensure_active_trifecta_preset(config: &mut AppConfig) {
    if active_trifecta_preset(config).is_some() {
        return;
    }
    let fallback = TrifectaPresetConfig::default_preset();
    config.active_trifecta_preset_id = Some(fallback.id.clone());
    config.trifecta_presets = vec![fallback];
}

/// `json.dumps(value, indent=2)` with its default ASCII escaping: the
/// settings file's byte shape.
fn to_ascii_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            // Integers render plainly; floats through the Python repr
            // rule (exponent thresholds and signs match json.dumps).
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&eo_wire::normalizer::python_repr_f64(
                    n.as_f64().expect("numeric JSON value"),
                ));
            }
        }
        Value::String(s) => write_escaped_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth + 1));
                write_value(out, item, depth + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth + 1));
                write_escaped_string(out, key);
                out.push_str(": ");
                write_value(out, item, depth + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
    }
}

fn write_escaped_string(out: &mut String, raw: &str) {
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if c.is_ascii() => out.push(c),
            c => {
                // Python's default ensure_ascii: BMP as \uXXXX, beyond
                // the BMP as a surrogate pair.
                let code = c as u32;
                if code <= 0xFFFF {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let reduced = code - 0x10000;
                    let high = 0xD800 + (reduced >> 10);
                    let low = 0xDC00 + (reduced & 0x3FF);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(dir: &Path) -> ConfigService {
        ConfigService::new(dir).unwrap()
    }

    fn read_settings(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("settings.json")).unwrap()
    }

    #[test]
    fn first_load_writes_defaults_with_the_home_chatlog_path() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        assert!(service.get().chatlog_path.ends_with("chat.log"));
        assert_eq!(service.get().mob_tracking_mode, "mob");
        assert_eq!(service.get().loot_filter_blacklist, ["Universal Ammo"]);
        assert!(dir.path().join("settings.json").exists());
    }

    #[test]
    fn save_then_load_is_a_byte_fixed_point() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = service(dir.path());
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Tester"));
        first.update(&updates).unwrap();
        let bytes_one = read_settings(dir.path());

        let mut second = service(dir.path());
        second.update(&Map::new()).unwrap();
        let bytes_two = read_settings(dir.path());
        assert_eq!(bytes_one, bytes_two, "a no-op save must not move bytes");
        assert_eq!(second.get().player_name, "Tester");
    }

    #[test]
    fn unknown_keys_survive_saves_in_their_stored_positions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            "{\n  \"extensionKey\": {\"nested\": [1, 2]},\n  \"player_name\": \"Kept\"\n}",
        )
        .unwrap();
        let mut svc = service(dir.path());
        assert_eq!(svc.get().extra["extensionKey"]["nested"][0], 1);
        assert_eq!(svc.get().player_name, "Kept");

        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Renamed"));
        svc.update(&updates).unwrap();
        let body = read_settings(dir.path());
        let ext = body.find("extensionKey").unwrap();
        let name = body.find("player_name").unwrap();
        assert!(ext < name, "stored position preserved on merge");
        assert!(body.contains("\"player_name\": \"Renamed\""));
        assert!(dir.path().join("settings.bak").exists());
    }

    #[test]
    fn keys_written_by_other_tooling_between_saves_survive() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let raw = read_settings(dir.path());
        let mut on_disk: Map<String, Value> = match serde_json::from_str::<Value>(&raw).unwrap() {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        on_disk.insert("thirdParty".into(), Value::from(true));
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::to_string(&Value::Object(on_disk)).unwrap(),
        )
        .unwrap();

        svc.update(&Map::new()).unwrap();
        assert!(read_settings(dir.path()).contains("thirdParty"));
    }

    #[test]
    fn corrupt_files_recover_to_saved_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "{not json").unwrap();
        let svc = service(dir.path());
        assert_eq!(svc.get().player_name, "");
        let body = read_settings(dir.path());
        assert!(body.contains("\"mob_tracking_mode\": \"mob\""));
    }

    #[test]
    fn preset_normalisation_follows_the_stored_rules() {
        let raw = serde_json::json!([
            {"id": "  ", "name": "skipped: blank id"},
            {"id": "alpha", "name": "", "small_weapon_id": 7},
            {"id": "alpha", "name": "duplicate skipped"},
            {"id": 42, "name": null, "heal_id": 3},
            "not an object",
        ]);
        let (presets, active) = normalize_trifecta_presets(Some(&raw), Some("missing"));
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].id, "alpha");
        assert_eq!(
            presets[0].name, "Preset 2",
            "blank name falls back by position"
        );
        assert_eq!(presets[0].small_weapon_id, Some(7));
        assert_eq!(presets[1].id, "42", "ids stringify");
        assert_eq!(presets[1].name, "Preset 4");
        assert_eq!(
            active, "alpha",
            "unknown active id falls to the first preset"
        );

        let (empty, active) = normalize_trifecta_presets(Some(&serde_json::json!([])), None);
        assert_eq!(empty[0].id, DEFAULT_TRIFECTA_PRESET_ID);
        assert_eq!(active, DEFAULT_TRIFECTA_PRESET_ID);
    }

    #[test]
    fn hotbar_always_normalises_to_the_full_slot_shape() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("hotbar".into(), serde_json::json!({"3": 17}));
        svc.update(&updates).unwrap();
        let hotbar = &svc.get().hotbar;
        assert_eq!(hotbar.len(), 10);
        assert_eq!(hotbar["3"], 17);
        assert_eq!(hotbar["1"], Value::Null);
        let keys: Vec<&String> = hotbar.keys().collect();
        assert_eq!(keys[9], "0", "slot order preserved");
    }

    #[test]
    fn toggles_coerce_truthy_stored_shapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::json!({
                "hotbar_hooks_enabled": 1,
                "repair_ocr_enabled": "yes",
                "end_of_session_armour_reminder_enabled": 0,
                "developer_mode_enabled": null,
            })
            .to_string(),
        )
        .unwrap();
        let svc = service(dir.path());
        assert!(svc.get().hotbar_hooks_enabled);
        assert!(svc.get().repair_ocr_enabled);
        assert!(!svc.get().end_of_session_armour_reminder_enabled);
        assert!(!svc.get().developer_mode_enabled);
    }

    #[test]
    fn unknown_update_keys_are_ignored_and_active_preset_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("no_such_field".into(), Value::from(1));
        updates.insert("active_trifecta_preset_id".into(), Value::from("ghost"));
        svc.update(&updates).unwrap();
        assert_eq!(
            svc.get().active_trifecta_preset_id.as_deref(),
            Some(DEFAULT_TRIFECTA_PRESET_ID),
            "an unresolvable active id collapses to the default preset"
        );
        assert!(svc.get().extra.get("no_such_field").is_none());
    }

    #[test]
    fn ascii_escaping_matches_the_stored_byte_shape() {
        let value = serde_json::json!({"name": "Frussj\u{00e4}ger \u{1F600}", "n": 1.5});
        let body = to_ascii_pretty(&value);
        assert!(body.contains("Frussj\\u00e4ger \\ud83d\\ude00"));
        assert!(body.contains("\"n\": 1.5"));
    }

    #[test]
    fn reset_and_validate_round_out_the_service_surface() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let mut updates = Map::new();
        let log = dir.path().join("chat.log");
        std::fs::write(&log, "x").unwrap();
        updates.insert(
            "chatlog_path".into(),
            Value::from(log.to_string_lossy().into_owned()),
        );
        svc.update(&updates).unwrap();
        assert!(svc.validate_chatlog());
        svc.reset().unwrap();
        assert!(svc.get().chatlog_path.ends_with("chat.log"));
    }

    #[test]
    fn every_field_updates_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let updates: Map<String, Value> = match serde_json::json!({
            "chatlog_path": "/tmp/other.log",
            "player_name": "Each",
            "hotbar_hooks_enabled": true,
            "repair_ocr_enabled": true,
            "end_of_session_armour_reminder_enabled": true,
            "developer_mode_enabled": true,
            "mob_tracking_mode": "tag",
            "mob_tracking_tag": "boss",
            "manual_mob_species": "Atrox",
            "manual_mob_maturity": "Old",
            "hotbar": {"1": 5},
            "trifecta_presets": [{"id": "beta", "name": "Beta", "heal_id": 9}],
            "loot_filter_blacklist": ["Shrapnel"],
            "overlay_x": 11,
            "overlay_y": -4,
        }) {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        svc.update(&updates).unwrap();
        let config = svc.get();
        assert_eq!(config.chatlog_path, "/tmp/other.log");
        assert_eq!(config.player_name, "Each");
        assert!(config.hotbar_hooks_enabled);
        assert!(config.repair_ocr_enabled);
        assert!(config.end_of_session_armour_reminder_enabled);
        assert!(config.developer_mode_enabled);
        assert_eq!(config.mob_tracking_mode, "tag");
        assert_eq!(config.mob_tracking_tag, "boss");
        assert_eq!(config.manual_mob_species, "Atrox");
        assert_eq!(config.manual_mob_maturity, "Old");
        assert_eq!(config.hotbar["1"], 5);
        assert_eq!(config.trifecta_presets[0].id, "beta");
        assert_eq!(config.trifecta_presets[0].heal_id, Some(9));
        assert_eq!(config.active_trifecta_preset_id.as_deref(), Some("beta"));
        assert_eq!(config.loot_filter_blacklist, ["Shrapnel"]);
        assert_eq!(config.overlay_x, Some(11));
        assert_eq!(config.overlay_y, Some(-4));

        // Switching the active id by string to another existing preset
        // takes effect (no fallback involved).
        let mut two_presets = Map::new();
        two_presets.insert(
            "trifecta_presets".into(),
            serde_json::json!([
                {"id": "beta", "name": "Beta"},
                {"id": "gamma", "name": "Gamma"},
            ]),
        );
        svc.update(&two_presets).unwrap();
        assert_eq!(svc.get().active_trifecta_preset_id.as_deref(), Some("beta"));
        let mut switch = Map::new();
        switch.insert("active_trifecta_preset_id".into(), Value::from("gamma"));
        svc.update(&switch).unwrap();
        assert_eq!(
            svc.get().active_trifecta_preset_id.as_deref(),
            Some("gamma")
        );

        // A null active id collapses to the default preset.
        let mut null_id = Map::new();
        null_id.insert("active_trifecta_preset_id".into(), Value::Null);
        svc.update(&null_id).unwrap();
        assert_eq!(
            svc.get().active_trifecta_preset_id.as_deref(),
            Some(DEFAULT_TRIFECTA_PRESET_ID)
        );

        // Reload from disk: the saved state equals the live state.
        let reloaded = service(dir.path());
        assert_eq!(reloaded.get(), svc.get());
    }

    #[test]
    fn clone_with_updates_leaves_the_live_config_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Candidate"));
        let candidate = svc.clone_with_updates(&updates);
        assert_eq!(candidate.player_name, "Candidate");
        assert_eq!(svc.get().player_name, "");
        assert_eq!(candidate.mob_tracking_mode, svc.get().mob_tracking_mode);
    }

    #[test]
    fn reset_restores_the_full_default_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Set"));
        updates.insert("overlay_x".into(), Value::from(9));
        svc.update(&updates).unwrap();
        svc.reset().unwrap();
        let expected = AppConfig {
            chatlog_path: AppConfig::default_chatlog_path(),
            ..AppConfig::default()
        };
        assert_eq!(svc.get(), &expected);
    }

    #[test]
    fn validate_chatlog_is_false_until_the_file_exists() {
        // Point the config at a path under the temp dir rather than relying
        // on the default chatlog path: on a machine with the game installed
        // the default path genuinely exists.
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let log_path = dir.path().join("chat.log");
        let mut updates = Map::new();
        updates.insert(
            "chatlog_path".into(),
            Value::from(log_path.to_string_lossy().as_ref()),
        );
        svc.update(&updates).unwrap();
        assert!(!svc.validate_chatlog());
        std::fs::write(&log_path, "").unwrap();
        assert!(svc.validate_chatlog());
    }

    #[test]
    fn active_preset_resolution_honours_a_valid_stored_id() {
        let mut config = AppConfig {
            trifecta_presets: vec![
                TrifectaPresetConfig {
                    id: "alpha".into(),
                    name: "A".into(),
                    small_weapon_id: None,
                    big_weapon_id: None,
                    heal_id: None,
                },
                TrifectaPresetConfig {
                    id: "beta".into(),
                    name: "B".into(),
                    small_weapon_id: None,
                    big_weapon_id: None,
                    heal_id: None,
                },
            ],
            active_trifecta_preset_id: Some("beta".into()),
            ..AppConfig::default()
        };
        assert_eq!(active_trifecta_preset(&config).unwrap().id, "beta");
        config.active_trifecta_preset_id = Some(String::new());
        assert!(active_trifecta_preset(&config).is_none());
        config.active_trifecta_preset_id = None;
        assert!(active_trifecta_preset(&config).is_none());

        // A valid stored active id survives normalisation untouched.
        let raw = serde_json::json!([{"id": "alpha", "name": "A"}, {"id": "beta", "name": "B"}]);
        let (_, active) = normalize_trifecta_presets(Some(&raw), Some("beta"));
        assert_eq!(active, "beta");
    }

    #[test]
    fn falsy_string_and_collection_toggles_stay_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::json!({
                "hotbar_hooks_enabled": "",
                "repair_ocr_enabled": [],
                "developer_mode_enabled": {},
            })
            .to_string(),
        )
        .unwrap();
        let svc = service(dir.path());
        assert!(!svc.get().hotbar_hooks_enabled);
        assert!(!svc.get().repair_ocr_enabled);
        assert!(!svc.get().developer_mode_enabled);
    }

    #[test]
    fn the_writer_nests_indentation_commas_and_control_escapes_exactly() {
        let value = serde_json::json!({
            "a": [{"x": 1, "y": [true, null]}, 2],
            "b": "ctl\u{0001} end",
        });
        assert_eq!(
            to_ascii_pretty(&value),
            concat!(
                "{\n",
                "  \"a\": [\n",
                "    {\n",
                "      \"x\": 1,\n",
                "      \"y\": [\n",
                "        true,\n",
                "        null\n",
                "      ]\n",
                "    },\n",
                "    2\n",
                "  ],\n",
                "  \"b\": \"ctl\\u0001 end\"\n",
                "}"
            )
        );
    }

    #[test]
    fn wrong_shape_files_error_instead_of_resetting() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "[1, 2]").unwrap();
        assert!(ConfigService::new(dir.path()).is_err());
        // The user's file is untouched.
        assert_eq!(read_settings(dir.path()), "[1, 2]");
    }

    #[test]
    fn registered_skip_paths_fall_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::json!({
                "player_name": 5,
                "overlay_x": 1.5,
                "loot_filter_blacklist": ["Universal Ammo", 5, null],
            })
            .to_string(),
        )
        .unwrap();
        let svc = service(dir.path());
        assert_eq!(svc.get().player_name, "");
        assert_eq!(svc.get().overlay_x, None);
        assert_eq!(svc.get().loot_filter_blacklist, ["Universal Ammo"]);
    }

    #[test]
    fn falsy_and_scalar_preset_ids_follow_the_python_semantics() {
        let raw = serde_json::json!([
            {"id": 0, "name": "skipped"},
            {"id": 0.0, "name": "skipped"},
            {"id": false, "name": "skipped"},
            {"id": true, "name": 1.5},
            {"id": {"container": 1}, "name": "skipped: container id"},
        ]);
        let (presets, _) = normalize_trifecta_presets(Some(&raw), None);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id, "True");
        assert_eq!(presets[0].name, "1.5");
    }

    #[test]
    fn del_and_small_floats_render_like_the_backend_writer() {
        let value = serde_json::json!({"k": "a\u{7f}b", "tiny": 1e-5});
        let body = to_ascii_pretty(&value);
        assert!(body.contains("a\\u007fb"));
        assert!(body.contains("1e-05"));
    }

    #[test]
    #[cfg(windows)]
    fn saved_files_carry_platform_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let _svc = service(dir.path());
        let bytes = std::fs::read(dir.path().join("settings.json")).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn readonly_load_matches_the_service_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_config_readonly(dir.path()).unwrap();
        assert!(loaded.chatlog_path.ends_with("chat.log"));
        assert!(
            !dir.path().join("settings.json").exists(),
            "a missing file must stay missing"
        );

        let mut svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Owner"));
        updates.insert("overlay_x".into(), Value::from(3));
        svc.update(&updates).unwrap();
        let through = load_config_readonly(dir.path()).unwrap();
        assert_eq!(&through, svc.get(), "read-through sees the owner's saves");

        std::fs::write(dir.path().join("settings.json"), "[1]").unwrap();
        assert!(load_config_readonly(dir.path()).is_err());
        std::fs::write(dir.path().join("settings.json"), "{broken").unwrap();
        let recovered = load_config_readonly(dir.path()).unwrap();
        assert_eq!(recovered.player_name, "");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            "{broken",
            "an unparseable file is read past, never rewritten"
        );
    }

    #[test]
    fn stringify_renders_scalars_as_python_does() {
        assert_eq!(stringify(&Value::Bool(true)).as_deref(), Some("True"));
        assert_eq!(stringify(&Value::Bool(false)).as_deref(), Some("False"));
        assert_eq!(stringify(&Value::from(42)).as_deref(), Some("42"));
        assert_eq!(stringify(&Value::from(1.5)).as_deref(), Some("1.5"));
        assert_eq!(stringify(&Value::from("s")).as_deref(), Some("s"));
        assert_eq!(stringify(&serde_json::json!({"a": 1})), None);
        assert_eq!(stringify(&serde_json::json!([1])), None);
    }
}
