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
use eo_services::mob_lookup_service::{python_whitespace, MobLookupService};
use eo_services::tracker::{naive_isoformat, HuntTracker};
use eo_services::trifecta_service::{validate_trifecta, TrifectaPreset};
use serde_json::{json, Map, Value};

use crate::hydration::HydrationState;
use crate::hydration::{
    detail, error_response, internal_error, json_response, plain_json_response,
};

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
