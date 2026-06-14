//! Native tracking PRODUCER routes (`backend/routers/tracking.py`): the
//! lifecycle pair and the manual-mob autocomplete, served over the live
//! producer-spine tracker rather than the read-only database surface.
//!
//! These three routes differ from the session-READ surface
//! (`tracking_routes`) in their dependency: they touch the live
//! `Arc<HuntTracker>` (its in-memory session state and its DB-backed
//! start/stop) and, for the suggestions, the bundled mobs catalogue.
//! They do NOT write `settings.json`; the three config-writing tracking
//! routes (release-mob, manual-mob-lock, tag-lock) ride the later
//! settings-write cutover.
//!
//!   POST /api/tracking/start                  -> TrackingStartResult
//!   POST /api/tracking/stop                   -> TrackingStopResult
//!   GET  /api/tracking/manual-mob-suggestions -> list[ManualMobSuggestion]
//!
//! Fidelity cruxes mirrored from the reference handlers:
//! - start/stop reply as PLAIN 200s (POST is outside the `/api/tracking`
//!   ETag middleware; the middleware touches only 2xx GETs), and their
//!   `started_at` / `ended_at` render `datetime.isoformat()` (naive, no
//!   offset) via [`naive_isoformat`].
//! - start's attribution gate (`_validate_attribution`): hotbar mode needs
//!   at least one bound `config.hotbar` slot; trifecta mode delegates to
//!   `validate_trifecta`. Both 400 messages match verbatim.
//! - manual-mob-suggestions: the tag-mode 409 (tracking -> the per-session
//!   captured mode; idle -> the live `config.mob_tracking_mode`) fires
//!   BEFORE the empty-`q` short-circuit, and both legs share one body. The
//!   200 legs (success and the empty-`q` `[]`) ARE ETag-scoped (a GET under
//!   `/api/tracking`), the 409 legs are not (non-2xx).

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::config_service::{
    active_trifecta_preset, load_config_readonly, AppConfig, ConfigService,
};
use eo_services::db::DbError;
use eo_services::hotbar_listener::HotbarListener;
use eo_services::mob_lookup_service::{python_whitespace, MobLookupService};
use eo_services::tracker::{naive_isoformat, to_iso_utc, HuntTracker};
use eo_services::trifecta_service::{validate_trifecta, TrifectaPreset};
use serde_json::{json, Map, Value};

use crate::hydration::HydrationState;
use crate::hydration::{
    detail, error_response, internal_error, json_response, plain_json_response,
};
use crate::scan_routes::project;

/// The `TrackingSnapshot` response-model field order (the polymorphic
/// dashboard hydration shape, served `exclude_unset`). The snake-case status
/// trio sits among the camelCase headline numbers exactly as the model
/// declares them; the projection emits whichever keys the active or idle
/// branch set, in this order.
const SNAPSHOT_FIELDS: [&str; 35] = [
    "status",
    "hotbarListenerActive",
    "weaponAttribution",
    "repairOcrEnabled",
    "endOfSessionArmourReminderEnabled",
    "mobEntryMode",
    "currentMob",
    "mobSource",
    "currentTool",
    "trifectaAttribution",
    "recentEvents",
    "session_id",
    "started_at",
    "kill_count",
    "elapsed",
    "cost",
    "returns",
    "pes",
    "net",
    "returnRate",
    "damageDealtTotal",
    "weaponDamageDealt",
    "weaponCost",
    "shotsFiredTotal",
    "criticalHitsTotal",
    "maxDamage",
    "globalsCount",
    "hofsCount",
    "latestKillLoot",
    "multiplierLast",
    "multiplierAvg",
    "multiplierMax",
    "multiplierHistory",
    "cumulativeNetHistory",
    "warnings",
];

/// `_validate_hotbar`: hotbar attribution is workable as long as at least
/// one slot is bound (a non-null library id). The reference does NOT
/// verify the equipment row exists, only that a slot carries a value.
fn validate_hotbar(config: &AppConfig) -> (bool, Option<String>) {
    let any_bound = config
        .hotbar
        .values()
        .any(|library_id| !library_id.is_null());
    if any_bound {
        (true, None)
    } else {
        (
            false,
            Some(
                "Bind at least one hotbar slot in the Equipment page before tracking.".to_string(),
            ),
        )
    }
}

impl HydrationState {
    /// POST /api/tracking/start
    ///
    /// The attribution gate (`_validate_attribution`) reads the live
    /// config and, in trifecta mode, the equipment library; on success it
    /// starts a session and replies with the lifecycle acknowledgement.
    pub async fn tracking_start(&self, tracker: &std::sync::Arc<HuntTracker>) -> Response<Body> {
        // 409 already-active, BEFORE the attribution gate (handler order).
        if tracker.is_tracking() {
            return error_response(StatusCode::CONFLICT, &detail("Session already active"));
        }

        // `_validate_attribution`: hotbar mode -> at least one bound slot;
        // trifecta mode -> validate_trifecta over the active preset.
        let Ok(config) = load_config_readonly(&self.data_dir) else {
            return internal_error();
        };
        let (ready, message) = if config.hotbar_hooks_enabled {
            validate_hotbar(&config)
        } else {
            let preset = active_trifecta_preset(&config).map(|p| TrifectaPreset {
                small_weapon_id: p.small_weapon_id,
                big_weapon_id: p.big_weapon_id,
                heal_id: p.heal_id,
            });
            match validate_trifecta(&self.db, preset.as_ref()).await {
                Ok((ready, reason)) => (
                    ready,
                    reason.or_else(|| {
                        Some(
                            "Configure the trifecta in the Equipment page before tracking."
                                .to_string(),
                        )
                    }),
                ),
                Err(_) => return internal_error(),
            }
        };
        if !ready {
            let detail_message = message.unwrap_or_else(|| {
                "Configure the trifecta in the Equipment page before tracking.".to_string()
            });
            return error_response(StatusCode::BAD_REQUEST, &detail(&detail_message));
        }

        match tracker.start_session() {
            Ok(session) => plain_json_response(&json!({
                "session_id": session.id,
                "started_at": naive_isoformat(session.start_time),
                "status": "active",
            })),
            Err(_) => internal_error(),
        }
    }

    /// POST /api/tracking/stop
    pub async fn tracking_stop(&self, tracker: &std::sync::Arc<HuntTracker>) -> Response<Body> {
        // 409 no active session, BEFORE the stop call (handler order).
        if !tracker.is_tracking() {
            return error_response(StatusCode::CONFLICT, &detail("No active session"));
        }
        match tracker.stop_session() {
            Ok(Some(session)) => plain_json_response(&json!({
                "session_id": session.id,
                "started_at": naive_isoformat(session.start_time),
                "ended_at": session.end_time.map(naive_isoformat).map_or(Value::Null, Value::from),
                "kill_count": session.kills.len(),
            })),
            // Defensive 500: `is_tracking` was true above, so a None here
            // means a broken invariant, exactly as the reference guards.
            Ok(None) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &detail("Failed to stop the active session"),
            ),
            Err(_) => internal_error(),
        }
    }

    /// GET /api/tracking/manual-mob-suggestions?q=&limit=
    ///
    /// `limit` is already clamped to `max(1, min(limit, 20))` by the
    /// caller's [`crate::extract::query_int_or_default`] + this clamp; an
    /// unparseable `limit` is the 422 the adapter raises before this runs.
    pub async fn tracking_manual_mob_suggestions(
        &self,
        tracker: &std::sync::Arc<HuntTracker>,
        q: &str,
        limit: i64,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        // The tag-mode 409 (both legs share one body) fires BEFORE the
        // empty-`q` short-circuit: tracking consults the per-session
        // captured mode, idle consults the live config.
        let tag_mode = if tracker.is_tracking() {
            tracker.is_session_tag_mode()
        } else {
            match load_config_readonly(&self.data_dir) {
                Ok(config) => config.mob_tracking_mode == "tag",
                Err(_) => return internal_error(),
            }
        };
        if tag_mode {
            return error_response(
                StatusCode::CONFLICT,
                &detail("Tag mode disables manual mob selection"),
            );
        }

        // `q.strip()`-empty short-circuits to `[]` (still ETag-scoped: a
        // 200 GET under `/api/tracking`).
        let query = q.trim_matches(python_whitespace);
        if query.is_empty() {
            return json_response(&json!([]), if_none_match);
        }

        // `max(1, min(limit, 20))`; the lookup borrows the catalogue.
        let bounded = limit.clamp(1, 20) as usize;
        let lookup = MobLookupService::new(&self.game_data);
        let suggestions = lookup.search_mob_names(query, bounded);
        json_response(&Value::Array(suggestions), if_none_match)
    }

    /// POST /api/tracking/release-mob: clear the locked mob or tag. Mirrors
    /// `release_mob`: an active tag-mode session releases the tracker's mob
    /// and clears the config tag; idle in tag mode returns the trimmed
    /// config tag (or null) and clears it; idle in manual mode returns the
    /// stored species display (or null) and clears the manual selection; an
    /// active non-tag session releases the tracker's mob and clears the
    /// manual selection. A plain 200 (POST, outside the ETag scope).
    pub async fn release_mob(
        &self,
        config: &Arc<Mutex<ConfigService>>,
        tracker: &Arc<HuntTracker>,
    ) -> Response<Body> {
        let Ok(mut guard) = config.lock() else {
            // A poisoned lock means a prior holder panicked; degrade to a 500
            // rather than panic this request task (and the endpoint family).
            return internal_error();
        };
        if tracker.is_tracking() && tracker.is_session_tag_mode() {
            let released = tracker.release_current_mob();
            if guard.update(&clear_tag()).is_err() {
                return internal_error();
            }
            return plain_json_response(&json!({ "released": released }));
        }
        if !tracker.is_tracking() {
            if guard.get().mob_tracking_mode == "tag" {
                let trimmed = guard.get().mob_tracking_tag.trim().to_string();
                let released = if trimmed.is_empty() {
                    Value::Null
                } else {
                    Value::String(trimmed)
                };
                if guard.update(&clear_tag()).is_err() {
                    return internal_error();
                }
                return plain_json_response(&json!({ "released": released }));
            }
            let species = guard.get().manual_mob_species.trim().to_string();
            let maturity = guard.get().manual_mob_maturity.trim().to_string();
            let released = mob_display(&species, &maturity);
            if guard.update(&clear_manual_mob()).is_err() {
                return internal_error();
            }
            return plain_json_response(&json!({ "released": released }));
        }
        // Active session, not in tag mode: release the tracker's mob.
        let released = tracker.release_current_mob();
        if guard.update(&clear_manual_mob()).is_err() {
            return internal_error();
        }
        plain_json_response(&json!({ "released": released }))
    }

    /// POST /api/tracking/manual-mob-lock: lock a catalogue mob for manual
    /// kill stamping. 409 in tag mode (active session OR idle config), 400
    /// when the mob is absent from the catalogue. Mirrors `manual_mob_lock`.
    pub async fn manual_mob_lock(
        &self,
        config: &Arc<Mutex<ConfigService>>,
        tracker: &Arc<HuntTracker>,
        species: &str,
        maturity: &str,
    ) -> Response<Body> {
        let Ok(mut guard) = config.lock() else {
            // A poisoned lock means a prior holder panicked; degrade to a 500
            // rather than panic this request task (and the endpoint family).
            return internal_error();
        };
        let idle_tag_mode = !tracker.is_tracking() && guard.get().mob_tracking_mode == "tag";
        if (tracker.is_tracking() && tracker.is_session_tag_mode()) || idle_tag_mode {
            return error_response(
                StatusCode::CONFLICT,
                &detail("Tag mode disables manual mob selection"),
            );
        }
        let species = species.trim();
        let maturity = maturity.trim();
        if !MobLookupService::new(&self.game_data).has_mob_name(species, maturity) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &detail("Mob is not present in the catalogue"),
            );
        }
        let display = if maturity.is_empty() {
            species.to_string()
        } else {
            format!("{maturity} {species}")
        };
        let mut updates = Map::new();
        updates.insert("manual_mob_species".into(), json!(species));
        updates.insert("manual_mob_maturity".into(), json!(maturity));
        if guard.update(&updates).is_err() {
            return internal_error();
        }
        if tracker.is_tracking() && tracker.set_manual_mob(&display, species, maturity).is_err() {
            // The gate already cleared an active, non-tag session, so the only
            // reachable error is the live config having flipped to tag mode
            // since the session started (manual entry disabled): the reference
            // raises there and 500s, after the same config write. Mirror it.
            return internal_error();
        }
        plain_json_response(&json!({
            "mobName": display,
            "species": species,
            "maturity": maturity,
        }))
    }

    /// POST /api/tracking/tag-lock: set the active free-text tag. 409 when
    /// not in tag mode (an active session not in tag mode, or idle config
    /// not in tag mode), 400 on an empty tag. Mirrors `tag_lock`. `tainted`
    /// flags a surrogate body string: it survives validation but the
    /// settings.json write cannot encode it, so (after the gate + empty
    /// check, exactly the reference's order) it is the 500 the encoder
    /// raises rather than a silently-lossy write.
    pub async fn tag_lock(
        &self,
        config: &Arc<Mutex<ConfigService>>,
        tracker: &Arc<HuntTracker>,
        tag: &str,
        tainted: bool,
    ) -> Response<Body> {
        let Ok(mut guard) = config.lock() else {
            // A poisoned lock means a prior holder panicked; degrade to a 500
            // rather than panic this request task (and the endpoint family).
            return internal_error();
        };
        if tracker.is_tracking() {
            if !tracker.is_session_tag_mode() {
                return error_response(
                    StatusCode::CONFLICT,
                    &detail("Active session is not in tag mode"),
                );
            }
        } else if guard.get().mob_tracking_mode != "tag" {
            return error_response(StatusCode::CONFLICT, &detail("Tag mode is not enabled"));
        }
        let tag = tag.trim();
        if tag.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, &detail("Tag cannot be empty"));
        }
        if tainted {
            return internal_error();
        }
        let mut updates = Map::new();
        updates.insert("mob_tracking_tag".into(), json!(tag));
        if guard.update(&updates).is_err() {
            return internal_error();
        }
        if tracker.is_tracking() {
            let _ = tracker.set_manual_tag(tag);
        }
        plain_json_response(&json!({ "tag": tag }))
    }

    /// GET /api/tracking/snapshot: the consolidated dashboard hydration. The
    /// union of the live tracker readout, the configuration- and
    /// runtime-derived envelope (attribution mode, the repair-OCR flag, the
    /// hotbar listener's running state, the trifecta summary), and the
    /// recent-events / warnings feeds. Polymorphic across idle and active; the
    /// `exclude_unset` projection emits each state's own keys in the model's
    /// declaration order (the snake-case status trio kept among the camelCase
    /// numbers as the dashboard reads them). A 2xx GET under the
    /// `/api/tracking` ETag prefix, so it carries the conditional-GET contract.
    pub async fn tracking_snapshot(
        &self,
        tracker: &Arc<HuntTracker>,
        hotbar: &Arc<HotbarListener>,
        if_none_match: Option<&str>,
    ) -> Response<Body> {
        let Ok(config) = load_config_readonly(&self.data_dir) else {
            return internal_error();
        };
        // `_weapon_attribution`: trifecta unless the hotbar hooks are on.
        let weapon_attribution = if config.hotbar_hooks_enabled {
            "hotbar"
        } else {
            "trifecta"
        };
        let trifecta_attribution = if weapon_attribution == "trifecta" {
            match self.trifecta_attribution_summary(&config).await {
                Ok(summary) => summary,
                Err(_) => return internal_error(),
            }
        } else {
            Value::Null
        };
        let readout = match tracker.snapshot() {
            Ok(readout) => readout,
            Err(_) => return internal_error(),
        };
        let current_tool = match &readout.current_tool {
            Some(tool) => Value::String(tool.clone()),
            None => Value::Null,
        };

        let value = match &readout.active {
            None => {
                // The configured manual label hydrates an idle dashboard.
                let (current_mob, mob_source) = configured_manual_label(&config);
                json!({
                    "status": "idle",
                    "hotbarListenerActive": hotbar.is_running(),
                    "weaponAttribution": weapon_attribution,
                    "repairOcrEnabled": config.repair_ocr_enabled,
                    "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
                    "currentTool": current_tool,
                    "trifectaAttribution": trifecta_attribution,
                    "mobEntryMode": config.mob_tracking_mode,
                    "currentMob": current_mob,
                    "mobSource": mob_source,
                    "recentEvents": [],
                })
            }
            Some(active) => {
                let recent_events: Vec<Value> = active
                    .notable_event_rows
                    .iter()
                    .enumerate()
                    .map(|(index, (event_type, mob_or_item, value_ped, ts))| {
                        // Built in the NotableEvent declaration order with the
                        // extra `id` last, exactly as `extra="allow"` emits it.
                        json!({
                            "type": notable_event_category(event_type),
                            "description": notable_event_description(event_type, mob_or_item, *value_ped),
                            "value": *value_ped,
                            "eventType": event_type.clone(),
                            "timestamp": ts_to_iso(*ts),
                            "id": format!("ne-{index}"),
                        })
                    })
                    .collect();
                let warnings: Vec<Value> = active
                    .warnings
                    .iter()
                    // The tracker warning shares NotableEvent's required trio;
                    // its `value` is the model-coerced float zero.
                    .map(|message| json!({"type": "warning", "description": message, "value": 0.0}))
                    .collect();
                json!({
                    "status": "active",
                    "session_id": active.session_id.clone(),
                    "started_at": active.started_at.clone(),
                    "kill_count": active.kill_count,
                    "elapsed": active.elapsed,
                    "cost": active.cost,
                    "returns": active.returns,
                    "pes": active.pes,
                    "net": active.net,
                    "returnRate": active.return_rate,
                    "damageDealtTotal": active.damage_dealt_total,
                    "weaponDamageDealt": active.weapon_damage_dealt,
                    "weaponCost": active.weapon_cost,
                    "shotsFiredTotal": active.shots_fired_total,
                    "criticalHitsTotal": active.critical_hits_total,
                    "maxDamage": active.max_damage,
                    "globalsCount": active.globals_count,
                    "hofsCount": active.hofs_count,
                    "latestKillLoot": active.latest_kill_loot,
                    "multiplierLast": active.multiplier_last,
                    "multiplierAvg": active.multiplier_avg,
                    "multiplierMax": active.multiplier_max,
                    "multiplierHistory": active.multiplier_history.clone(),
                    "cumulativeNetHistory": active.cumulative_net_history.clone(),
                    "hotbarListenerActive": hotbar.is_running(),
                    "weaponAttribution": weapon_attribution,
                    "repairOcrEnabled": config.repair_ocr_enabled,
                    "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
                    "currentTool": current_tool,
                    "trifectaAttribution": trifecta_attribution,
                    "mobEntryMode": active.mob_entry_mode.clone(),
                    "currentMob": active.current_mob.clone(),
                    "mobSource": active.mob_source.clone(),
                    "recentEvents": recent_events,
                    "warnings": warnings,
                })
            }
        };
        json_response(&project(&value, &SNAPSHOT_FIELDS), if_none_match)
    }

    /// `_trifecta_attribution_summary`: the active preset's bound weapon/heal
    /// names plus the preset list, or null when no preset exists and nothing
    /// is bound.
    async fn trifecta_attribution_summary(&self, config: &AppConfig) -> Result<Value, DbError> {
        let active = active_trifecta_preset(config);
        let small = active.and_then(|preset| preset.small_weapon_id);
        let big = active.and_then(|preset| preset.big_weapon_id);
        let heal = active.and_then(|preset| preset.heal_id);
        let presets: Vec<Value> = config
            .trifecta_presets
            .iter()
            .map(|preset| json!({"id": preset.id, "name": preset.name}))
            .collect();
        if presets.is_empty() && small.is_none() && big.is_none() && heal.is_none() {
            return Ok(Value::Null);
        }
        let mut summary = Map::new();
        summary.insert(
            "activePresetId".into(),
            match &config.active_trifecta_preset_id {
                Some(id) => Value::String(id.clone()),
                None => Value::Null,
            },
        );
        summary.insert(
            "presetName".into(),
            match active {
                Some(preset) => Value::String(preset.name.clone()),
                None => Value::Null,
            },
        );
        summary.insert("presets".into(), Value::Array(presets));
        summary.insert(
            "smallWeapon".into(),
            self.equipment_name(small, "weapon").await?,
        );
        summary.insert(
            "bigWeapon".into(),
            self.equipment_name(big, "weapon").await?,
        );
        summary.insert(
            "healTool".into(),
            self.equipment_name(heal, "healing").await?,
        );
        Ok(Value::Object(summary))
    }

    /// The equipment-library name for a bound id and type, or null when the id
    /// is unset or the row is absent.
    async fn equipment_name(&self, id: Option<i64>, item_type: &str) -> Result<Value, DbError> {
        let Some(id) = id else {
            return Ok(Value::Null);
        };
        match self.db.equipment_item(id, item_type).await? {
            Some((_id, name, _properties)) => Ok(Value::String(name)),
            None => Ok(Value::Null),
        }
    }
}

/// `_configured_manual_label`: the idle-state mob label and its source. Tag
/// mode reports the trimmed free-text tag (or none); manual mode reports the
/// stored species (with maturity) display (or none).
fn configured_manual_label(config: &AppConfig) -> (Value, Value) {
    if config.mob_tracking_mode == "tag" {
        let tag = config.mob_tracking_tag.trim();
        if tag.is_empty() {
            return (Value::Null, Value::Null);
        }
        return (
            Value::String(tag.to_string()),
            Value::String("tag".to_string()),
        );
    }
    let species = config.manual_mob_species.trim();
    let maturity = config.manual_mob_maturity.trim();
    if species.is_empty() {
        return (Value::Null, Value::Null);
    }
    let display = if maturity.is_empty() {
        species.to_string()
    } else {
        format!("{maturity} {species}")
    };
    (Value::String(display), Value::String("manual".to_string()))
}

/// `_ts_to_iso`: a Unix timestamp to an ISO 8601 UTC string (the same
/// `+00:00`-suffixed form the domain events stamp), or null.
fn ts_to_iso(ts: Option<f64>) -> Value {
    match ts {
        Some(ts) => Value::String(to_iso_utc(ts)),
        None => Value::Null,
    }
}

/// `_notable_event_category`: quest / HoF / global from the event-type prefix.
fn notable_event_category(event_type: &str) -> &'static str {
    if event_type.starts_with("quest_") {
        "quest"
    } else if event_type.starts_with("hof_") {
        "hof"
    } else {
        "global"
    }
}

/// `_notable_event_label`: the curated label for the known event types, else
/// the category title-cased (`HoF` kept as the special case).
fn notable_event_label(event_type: &str) -> String {
    match event_type {
        "global_kill" => "Global Kill".to_string(),
        "global_item" => "Global Item".to_string(),
        "hof_kill" => "HoF Kill".to_string(),
        "hof_item" => "HoF Item".to_string(),
        "quest_started" => "Quest Started".to_string(),
        "quest_completed" => "Quest Completed".to_string(),
        _ => {
            let category = notable_event_category(event_type);
            if category == "hof" {
                "HoF".to_string()
            } else {
                capitalize(category)
            }
        }
    }
}

/// `_notable_event_description`: the label with the mob or item, and the value
/// in PED for everything but the quest events.
fn notable_event_description(event_type: &str, mob_or_item: &str, value_ped: f64) -> String {
    let label = notable_event_label(event_type);
    if event_type.starts_with("quest_") {
        format!("{label}: {mob_or_item}")
    } else {
        format!("{label}: {mob_or_item} ({value_ped:.2} PED)")
    }
}

/// Python `str.capitalize` over an ASCII category: first letter upper, the
/// rest lower (the category words are already lower-case, so this upper-cases
/// the lead).
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The config update that clears the free-text session tag.
fn clear_tag() -> Map<String, Value> {
    let mut updates = Map::new();
    updates.insert("mob_tracking_tag".into(), json!(""));
    updates
}

/// The config update that clears the stored manual-mob selection.
fn clear_manual_mob() -> Map<String, Value> {
    let mut updates = Map::new();
    updates.insert("manual_mob_species".into(), json!(""));
    updates.insert("manual_mob_maturity".into(), json!(""));
    updates
}

/// The released-mob display value: null when no species, the bare species
/// when no maturity, else `"{maturity} {species}"` (the reference's form).
fn mob_display(species: &str, maturity: &str) -> Value {
    if species.is_empty() {
        Value::Null
    } else if maturity.is_empty() {
        Value::String(species.to_string())
    } else {
        Value::String(format!("{maturity} {species}"))
    }
}
