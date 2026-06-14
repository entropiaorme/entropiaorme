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

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::config_service::{active_trifecta_preset, load_config_readonly, AppConfig};
use eo_services::mob_lookup_service::{python_whitespace, MobLookupService};
use eo_services::tracker::{naive_isoformat, HuntTracker};
use eo_services::trifecta_service::{validate_trifecta, TrifectaPreset};
use serde_json::{json, Value};

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
}
