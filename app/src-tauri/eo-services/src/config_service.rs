//! Configuration service: typed settings with atomic persistence.
//!
//! Settings live as JSON in `data/settings.json`. Saves are atomic
//! (write `.tmp`, swap into place, keep `.bak`) and merge with whatever
//! is on disk at save time, so keys written by other tooling survive a
//! save by a process that does not know them; the unknown keys are also
//! carried as a typed catch-all on the loaded config, making the
//! carry-forward contract visible. The on-disk byte shape is the owned
//! canonical format: UTF-8 pretty JSON (two-space indent), stored key
//! positions preserved on merge, platform line endings. Files written
//! by earlier releases with ASCII-escaped text load unchanged.
//!
//! Update semantics: unknown update keys are
//! ignored; the hotbar always re-normalises to its full slot shape; the
//! trifecta preset list re-validates its active id. Where a stored or
//! submitted value does not fit its typed field, this implementation
//! coalesces or skips instead of carrying the raw value; the divergence
//! register's configuration entry records those cases and their
//! reachability.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::passive_effects::{PassiveEffect, PassiveEffectKind, PassiveEffectSource};

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

/// The harvesting tool guardrail: the tool the user intends to use for
/// each board-output class. While enabled, harvest swings whose loot
/// identifies a board class are attributed to its intended tool, and a
/// disagreement with the hotbar-equipped tool is surfaced rather than
/// silently recorded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HarvestGuardrailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub short_tool_id: Option<i64>,
    #[serde(default)]
    pub long_tool_id: Option<i64>,
    #[serde(default)]
    pub huge_tool_id: Option<i64>,
}

/// All user-configurable settings; field order is the serialised order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub chatlog_path: String,
    pub player_name: String,
    pub hotbar_hooks_enabled: bool,
    pub repair_ocr_enabled: bool,
    pub developer_mode_enabled: bool,
    /// The declared-mob facet: the kill-stamp source in force, carried
    /// across sessions so a declaration outlives the session that set it.
    pub manual_mob_species: String,
    pub manual_mob_maturity: String,
    /// The designated session-name facet the next session snapshots.
    pub session_name: String,
    /// The selected session definition the next session starts as an
    /// instance of; `None` is "no definition". Selection also writes
    /// `session_name` (the definition's name), so the two move
    /// together; a free-text rename of the name facet clears this.
    pub session_definition_id: Option<i64>,
    /// The skill-boost facet the next session opens under, three-state:
    /// `None` claims nothing, `Some(0)` declares deliberately-unboosted
    /// play (the baseline an effect is measured against), and `Some(n)`
    /// declares a magnitude. The retired `skill_boost_percent` key could
    /// not hold the middle state (its 0 meant "not declared"), so it is
    /// left to the carry-forward map rather than reinterpreted.
    pub declared_skill_boost_percent: Option<i64>,
    pub hotbar: Map<String, Value>,
    pub trifecta_presets: Vec<TrifectaPresetConfig>,
    pub active_trifecta_preset_id: Option<String>,
    /// Persistent item or condition effects. Each source owns typed effects so
    /// new capabilities and future time-bounded sources can share evaluators.
    pub passive_effect_sources: Vec<PassiveEffectSource>,
    pub harvest_guardrail: HarvestGuardrailConfig,
    pub loot_filter_blacklist: Vec<String>,
    pub overlay_x: Option<i64>,
    pub overlay_y: Option<i64>,
    /// The calibrated screen rectangle of the game's coordinate readout
    /// (the maps feature's capture region); None until the user runs
    /// the two-point calibration flow.
    pub map_coord_region: Option<crate::coord_capture::CoordRegion>,
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
            developer_mode_enabled: false,
            manual_mob_species: String::new(),
            manual_mob_maturity: String::new(),
            session_name: String::new(),
            session_definition_id: None,
            declared_skill_boost_percent: None,
            hotbar,
            trifecta_presets: vec![TrifectaPresetConfig::default_preset()],
            active_trifecta_preset_id: Some(DEFAULT_TRIFECTA_PRESET_ID.to_string()),
            passive_effect_sources: Vec::new(),
            harvest_guardrail: HarvestGuardrailConfig::default(),
            loot_filter_blacklist: vec!["Universal Ammo".to_string()],
            overlay_x: None,
            overlay_y: None,
            map_coord_region: None,
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
    /// The live-config publication: every successful write replaces
    /// the shared snapshot, so read-through consumers (the tracker's
    /// configuration seam) follow settings changes without re-reading
    /// the file per call.
    live: tokio::sync::watch::Sender<std::sync::Arc<AppConfig>>,
}

/// A cheap, always-current read handle over the live config. Reads
/// never touch the filesystem; they borrow the last published
/// snapshot. (Out-of-band edits to the settings file are picked up at
/// the next start, as before: the process is the only writer while it
/// runs.)
#[derive(Clone)]
pub struct ConfigReader(tokio::sync::watch::Receiver<std::sync::Arc<AppConfig>>);

impl ConfigReader {
    /// The current config snapshot.
    pub fn current(&self) -> std::sync::Arc<AppConfig> {
        self.0.borrow().clone()
    }
}

impl ConfigService {
    pub fn new(data_dir: &Path) -> std::io::Result<Self> {
        let config_path = data_dir.join("settings.json");
        let (live, _) = tokio::sync::watch::channel(std::sync::Arc::new(AppConfig::default()));
        let mut service = Self {
            config_path,
            config: AppConfig::default(),
            live,
        };
        service.config = service.load()?;
        service.publish();
        Ok(service)
    }

    fn load(&self) -> std::io::Result<AppConfig> {
        if self.config_path.exists() {
            // Read failures fail loudly: a transient lock must never
            // silently reset user settings.
            let raw = std::fs::read_to_string(&self.config_path)?;
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(data)) => return Ok(from_stored(&data)),
                Ok(_) => {
                    // A parseable file of the wrong shape errors loudly
                    // rather than resetting user settings.
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

    /// A read handle over the live config for read-through consumers.
    pub fn reader(&self) -> ConfigReader {
        ConfigReader(self.live.subscribe())
    }

    fn publish(&self) {
        self.live
            .send_replace(std::sync::Arc::new(self.config.clone()));
    }

    /// A candidate config with the updates applied, leaving the live
    /// config untouched (round-trips through the stored representation
    /// first, so validation sees exactly what a save would store).
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
        self.publish();
        Ok(&self.config)
    }

    /// Restore defaults (with the default chat-log path) and save.
    pub fn reset(&mut self) -> std::io::Result<&AppConfig> {
        self.config = AppConfig {
            chatlog_path: AppConfig::default_chatlog_path(),
            ..AppConfig::default()
        };
        self.save_current()?;
        self.publish();
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

        let mut body =
            serde_json::to_string_pretty(&Value::Object(merged)).expect("settings serialise");
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

/// Reconstruct a config from stored JSON, tolerating missing, extra,
/// and malformed fields (an inherited tolerance the tests pin).
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
        developer_mode_enabled: toggle("developer_mode_enabled"),
        manual_mob_species: string_or("manual_mob_species", ""),
        manual_mob_maturity: string_or("manual_mob_maturity", ""),
        // One-time legacy inheritance: a store written before the facet
        // model has no session_name key at all, and its tag-mode tag was
        // the de facto session name. Only key absence inherits; a stored
        // empty string is an explicit clear and stays empty.
        session_name: if data.contains_key("session_name") {
            string_or("session_name", "")
        } else if string_or("mob_tracking_mode", "mob") == "tag" {
            string_or("mob_tracking_tag", "")
        } else {
            String::new()
        },
        // A missing key, an explicit null, or a hand-edited non-positive
        // id all read as "no definition selected" rather than failing
        // the whole config load.
        session_definition_id: data
            .get("session_definition_id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0),
        // A missing key, an explicit null, and a hand-edited negative all
        // read as "not declared" rather than failing the whole config load
        // (the same tolerance the other fields carry). A stored 0 is a
        // real declaration here, which is why this key had to be new.
        declared_skill_boost_percent: data
            .get("declared_skill_boost_percent")
            .and_then(Value::as_i64)
            .filter(|percent| *percent >= 0),
        hotbar: normalize_hotbar(data.get("hotbar")),
        trifecta_presets,
        active_trifecta_preset_id: Some(active_id),
        passive_effect_sources: normalize_passive_effect_sources(
            data.get("passive_effect_sources"),
        ),
        harvest_guardrail: normalize_harvest_guardrail(data.get("harvest_guardrail")),
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
        // A malformed stored region reads as uncalibrated rather than
        // failing the whole config load.
        map_coord_region: data
            .get("map_coord_region")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        extra,
    }
}

// The two retired exclusive-capture keys (`mob_tracking_mode`,
// `mob_tracking_tag`) are deliberately absent: they are no longer read
// into the config, and leaving them unknown preserves them verbatim in
// `extra` so the one-time legacy session-name inheritance above still
// has them to read on a store written before the facet model.
//
// `skill_boost_percent` is absent for the same reason and a stronger
// one: its stored 0 meant "no boost declared", which the three-state
// facet splits into "not declared" and "declared zero". Reading it
// forward would turn every existing store's default into a claim the
// user never made, so it stays unknown and carries through untouched.
//
// `end_of_session_armour_reminder_enabled` retired with the stop prompt it
// armed: protection costs are recorded when the player repairs, not when a
// session ends. A stored value carries through untouched like the others.
const KNOWN_KEYS: [&str; 19] = [
    "chatlog_path",
    "player_name",
    "hotbar_hooks_enabled",
    "repair_ocr_enabled",
    "developer_mode_enabled",
    "manual_mob_species",
    "manual_mob_maturity",
    "session_name",
    "session_definition_id",
    "declared_skill_boost_percent",
    "hotbar",
    "trifecta_presets",
    "active_trifecta_preset_id",
    "passive_effect_sources",
    "harvest_guardrail",
    "loot_filter_blacklist",
    "overlay_x",
    "overlay_y",
    "map_coord_region",
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

fn normalize_passive_effect_sources(raw: Option<&Value>) -> Vec<PassiveEffectSource> {
    let Some(items) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    items
        .iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            let id = object.get("id")?.as_str()?.trim().to_string();
            let name = object.get("name")?.as_str()?.trim().to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            let effects = object
                .get("effects")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effect| {
                    let effect = effect.as_object()?;
                    let kind = match effect.get("kind")?.as_str()? {
                        "reload_speed" => PassiveEffectKind::ReloadSpeed,
                        _ => return None,
                    };
                    let magnitude_percent = effect.get("magnitude_percent")?.as_f64()?;
                    magnitude_percent.is_finite().then_some(PassiveEffect {
                        kind,
                        magnitude_percent,
                    })
                })
                .collect::<Vec<_>>();
            if effects.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            Some(PassiveEffectSource {
                id,
                name,
                enabled: object.get("enabled").map(json_truthy).unwrap_or(true),
                effects,
            })
        })
        .collect()
}

/// Normalise a stored or submitted guardrail block: a non-object reads
/// as the disabled default; `enabled` follows the stored-toggle truthy
/// rule; a tool id that is not an integer reads as unset.
fn normalize_harvest_guardrail(raw: Option<&Value>) -> HarvestGuardrailConfig {
    let Some(object) = raw.and_then(Value::as_object) else {
        return HarvestGuardrailConfig::default();
    };
    HarvestGuardrailConfig {
        enabled: object.get("enabled").map(json_truthy).unwrap_or(false),
        short_tool_id: object.get("short_tool_id").and_then(Value::as_i64),
        long_tool_id: object.get("long_tool_id").and_then(Value::as_i64),
        huge_tool_id: object.get("huge_tool_id").and_then(Value::as_i64),
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
/// rather than repr-rendered, a deliberate divergence from the original.
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
            "developer_mode_enabled" => assign_bool(&mut config.developer_mode_enabled, value),
            "manual_mob_species" => assign_string(&mut config.manual_mob_species, value),
            "manual_mob_maturity" => assign_string(&mut config.manual_mob_maturity, value),
            "session_name" => assign_string(&mut config.session_name, value),
            // An explicit null withdraws the selection; a non-positive
            // id is nonsense and reads as a withdrawal too.
            "session_definition_id" => {
                config.session_definition_id = value.as_i64().filter(|id| *id > 0);
            }
            // Stored as given: the settings boundary refuses a negative
            // outright rather than silently coercing it to "no boost".
            // An explicit null withdraws the declaration, which is a
            // distinct write from declaring zero.
            "declared_skill_boost_percent" => {
                if value.is_null() {
                    config.declared_skill_boost_percent = None;
                } else if let Some(percent) = value.as_i64() {
                    config.declared_skill_boost_percent = Some(percent);
                }
            }
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
            "passive_effect_sources" => {
                config.passive_effect_sources = normalize_passive_effect_sources(Some(value));
            }
            "harvest_guardrail" => {
                config.harvest_guardrail = normalize_harvest_guardrail(Some(value));
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
            "map_coord_region" => {
                config.map_coord_region = serde_json::from_value(value.clone()).ok();
            }
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
/// the default preset.
fn ensure_active_trifecta_preset(config: &mut AppConfig) {
    if active_trifecta_preset(config).is_some() {
        return;
    }
    let fallback = TrifectaPresetConfig::default_preset();
    config.active_trifecta_preset_id = Some(fallback.id.clone());
    config.trifecta_presets = vec![fallback];
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
    fn a_discarded_source_does_not_reserve_its_id() {
        // The first entry carries no usable effect, so it is dropped. A later
        // entry under the same id is the one the user declared, and it stands.
        let raw = serde_json::json!([
            { "id": "ring", "name": "Ring", "effects": [{ "kind": "unknown" }] },
            {
                "id": "ring",
                "name": "Ares Ring, Perfected",
                "effects": [{ "kind": "reload_speed", "magnitude_percent": 14.0 }]
            }
        ]);
        let sources = normalize_passive_effect_sources(Some(&raw));
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "Ares Ring, Perfected");
    }

    #[test]
    fn first_load_writes_defaults_with_the_home_chatlog_path() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        assert!(service.get().chatlog_path.ends_with("chat.log"));
        assert_eq!(service.get().manual_mob_species, "");
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
    fn a_reader_follows_the_live_config_after_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let reader = svc.reader();
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Live"));
        svc.update(&updates).unwrap();
        // The write publishes the new snapshot to the read handle.
        assert_eq!(reader.current().player_name, "Live");
    }

    /// A store written under the retired exclusive-capture model still
    /// loads: the tag inherits the session name once, the declaration is
    /// read from its own keys rather than being shadowed by the tag, and
    /// both retired keys survive verbatim so the inheritance stays
    /// available to any store that has not taken it yet.
    #[test]
    fn retired_tag_keys_inherit_the_name_without_shadowing_the_declaration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            "{\n  \"mob_tracking_mode\": \"tag\",\n  \"mob_tracking_tag\": \"85-B, P20\",\n  \"manual_mob_species\": \"Carabok\",\n  \"manual_mob_maturity\": \"Puny\"\n}",
        )
        .unwrap();
        let svc = service(dir.path());
        let config = svc.get();

        assert_eq!(config.manual_mob_species, "Carabok");
        assert_eq!(config.manual_mob_maturity, "Puny");
        assert_eq!(config.session_name, "85-B, P20", "one-time tag inheritance");
        assert_eq!(config.extra["mob_tracking_tag"], Value::from("85-B, P20"));
        assert_eq!(config.extra["mob_tracking_mode"], Value::from("tag"));
    }

    /// The three states must survive a round trip through the store,
    /// because the declaration outlives the session that set it. A
    /// declared zero is the one that pays: it is the baseline a boost's
    /// effect is measured against, and it reads back as `Some(0)` rather
    /// than collapsing into "nothing declared".
    #[test]
    fn the_declared_boost_round_trips_all_three_states() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        assert_eq!(
            svc.get().declared_skill_boost_percent,
            None,
            "a fresh store declares nothing"
        );

        for declared in [Some(0), Some(50), None] {
            let mut updates = Map::new();
            updates.insert(
                "declared_skill_boost_percent".into(),
                serde_json::json!(declared),
            );
            svc.update(&updates).unwrap();
            assert_eq!(svc.get().declared_skill_boost_percent, declared);
            assert_eq!(
                service(dir.path()).get().declared_skill_boost_percent,
                declared,
                "the state survives a reload from disk"
            );
        }
    }

    /// The retired key's stored 0 meant "no boost declared", which the
    /// three-state facet splits in two. Reading it forward would turn
    /// every existing store's default into a claim the user never made,
    /// so it must stay unread and carry through untouched.
    #[test]
    fn the_retired_boost_key_is_carried_not_reinterpreted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            "{\n  \"skill_boost_percent\": 0,\n  \"player_name\": \"Kept\"\n}",
        )
        .unwrap();
        let mut svc = service(dir.path());

        assert_eq!(
            svc.get().declared_skill_boost_percent,
            None,
            "a legacy 0 is not a declaration of deliberately-unboosted play"
        );
        assert_eq!(svc.get().extra["skill_boost_percent"], Value::from(0));

        // And it is still there after a save, like the other retired keys.
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Renamed"));
        svc.update(&updates).unwrap();
        assert!(read_settings(dir.path()).contains("skill_boost_percent"));
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
        assert!(body.contains("\"manual_mob_species\": \"\""));
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
    fn harvest_guardrail_normalises_stored_and_updated_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        assert_eq!(
            svc.get().harvest_guardrail,
            HarvestGuardrailConfig::default(),
            "absent block reads as the disabled default"
        );

        let mut updates = Map::new();
        updates.insert(
            "harvest_guardrail".into(),
            serde_json::json!({
                "enabled": true,
                "short_tool_id": 3,
                "long_tool_id": "not an id",
                "huge_tool_id": null,
            }),
        );
        svc.update(&updates).unwrap();
        assert_eq!(
            svc.get().harvest_guardrail,
            HarvestGuardrailConfig {
                enabled: true,
                short_tool_id: Some(3),
                long_tool_id: None,
                huge_tool_id: None,
            },
            "non-integer ids read as unset"
        );

        // A malformed stored block reads as the disabled default.
        let mut updates = Map::new();
        updates.insert("harvest_guardrail".into(), serde_json::json!("garbled"));
        svc.update(&updates).unwrap();
        assert_eq!(
            svc.get().harvest_guardrail,
            HarvestGuardrailConfig::default()
        );
    }

    #[test]
    fn toggles_coerce_truthy_stored_shapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::json!({
                "hotbar_hooks_enabled": 1,
                "repair_ocr_enabled": "yes",
                "developer_mode_enabled": null,
            })
            .to_string(),
        )
        .unwrap();
        let svc = service(dir.path());
        assert!(svc.get().hotbar_hooks_enabled);
        assert!(svc.get().repair_ocr_enabled);
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
    fn ascii_escaped_legacy_files_load_intact() {
        // Files written by earlier releases carry `\uXXXX` escapes
        // (including surrogate pairs); the read side decodes them, and
        // the next save re-stores the text in the canonical UTF-8 form.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            "{\n  \"player_name\": \"Frussj\\u00e4ger \\ud83d\\ude00\"\n}",
        )
        .unwrap();
        let svc = service(dir.path());
        assert_eq!(svc.get().player_name, "Frussj\u{00e4}ger \u{1F600}");
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
            "developer_mode_enabled": true,
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
        assert!(config.developer_mode_enabled);
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
        assert_eq!(candidate.manual_mob_species, svc.get().manual_mob_species);
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
    fn the_writer_nests_indentation_and_commas_canonically() {
        let value = serde_json::json!({
            "a": [{"x": 1, "y": [true, null]}, 2],
            "b": "end",
        });
        assert_eq!(
            serde_json::to_string_pretty(&value).unwrap(),
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
                "  \"b\": \"end\"\n",
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
    fn non_ascii_text_is_stored_as_utf8_and_reloads_intact() {
        let dir = tempfile::tempdir().unwrap();
        let mut svc = service(dir.path());
        let mut updates = Map::new();
        updates.insert("player_name".into(), Value::from("Frussjäger 😀"));
        svc.update(&updates).unwrap();
        assert!(
            read_settings(dir.path()).contains("Frussjäger 😀"),
            "the canonical format stores text as raw UTF-8"
        );
        let reloaded = service(dir.path());
        assert_eq!(reloaded.get().player_name, "Frussjäger 😀");
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
