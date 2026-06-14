//! Native manual-scan routes (`backend/routers/scan_manual.py` and the
//! repair-scan leg of `backend/routers/tracking.py`): the user-driven,
//! page-by-page skill scan and the one-shot repair-cost read, served over the
//! composed [`SkillScanManual`] / [`RepairOcrService`].
//!
//! Each verb returns its result as a JSON [`Value`] already in the service's
//! shape; the routes project it into the response model's field order
//! (replicating Pydantic's `response_model_exclude_unset` = the keys the
//! service set, emitted in declaration order) and serialise it the backend's
//! way. The GETs (status, pending, the capture PNG) sit under the `/api/scan`
//! ETag prefix, so they carry the conditional-GET contract; the POST verbs do
//! not, and the service's logical refusals ride in the body of a plain 200
//! (the reference returns the `{"error": ...}` dict, it does not raise), so
//! they reply as plain 200s. Only `pending` and the capture PNG raise a real
//! 404 (their missing-resource leg), and only repair-scan gates a 400.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use eo_services::repair_ocr::RepairOcrService;
use eo_services::skill_scan_manual::SkillScanManual;
use serde_json::{json, Map, Value};

use crate::hydration::{
    conditional_response, detail, error_response, json_response, plain_json_response,
};

// The response-model field orders (Pydantic declaration order). The skill
// status fields appear first on the verbs that extend `ScanManualStatusOrError`
// (capture, undo), with their own field appended, exactly as Pydantic emits a
// subclass's inherited fields before its own.
const STATUS_FIELDS: [&str; 12] = [
    "active",
    "processing",
    "captured_pages",
    "expected_pages",
    "last_scan_time",
    "skills_count",
    "configured",
    "game_window_present",
    "phase",
    "processing_progress",
    "has_pending_result",
    "error",
];
const CAPTURE_FIELDS: [&str; 14] = [
    "active",
    "processing",
    "captured_pages",
    "expected_pages",
    "last_scan_time",
    "skills_count",
    "configured",
    "game_window_present",
    "phase",
    "processing_progress",
    "has_pending_result",
    "error",
    "page",
    "captured",
];
const UNDO_FIELDS: [&str; 13] = [
    "active",
    "processing",
    "captured_pages",
    "expected_pages",
    "last_scan_time",
    "skills_count",
    "configured",
    "game_window_present",
    "phase",
    "processing_progress",
    "has_pending_result",
    "error",
    "undone_page",
];
const ACCEPT_FIELDS: [&str; 3] = ["ok", "skills_persisted", "error"];
const REJECT_FIELDS: [&str; 2] = ["ok", "error"];
const REPAIR_FIELDS: [&str; 4] = ["cost_ped", "raw_text", "confidence", "error"];

/// Project a service value into a response model's field order, emitting only
/// the keys present in the value (Pydantic's `exclude_unset`). The non-exclude
/// models in this surface (the status read, the repair result) always carry
/// their full declared set from the service, so this also yields their
/// complete ordered object; the one declared-optional key absent on success
/// (`error`) is correctly omitted exactly as `extra="allow"` leaves it.
fn project(value: &Value, order: &[&str]) -> Value {
    let mut out = Map::new();
    if let Some(object) = value.as_object() {
        for &field in order {
            if let Some(found) = object.get(field) {
                out.insert(field.to_string(), found.clone());
            }
        }
    }
    Value::Object(out)
}

/// GET /api/scan/skills/status: the full status under the conditional-GET
/// contract (a 2xx GET in the `/api/scan` ETag scope).
pub(crate) fn status(scan: &Arc<SkillScanManual>, if_none_match: Option<&str>) -> Response<Body> {
    json_response(&project(&scan.get_status(), &STATUS_FIELDS), if_none_match)
}

/// POST /api/scan/skills/start: `page_count` is the optional query int the
/// adapter has already validated; the service range-checks it and any refusal
/// rides the plain-200 body.
pub(crate) fn start(scan: &Arc<SkillScanManual>, page_count: Option<i64>) -> Response<Body> {
    plain_json_response(&project(&scan.start(page_count), &STATUS_FIELDS))
}

/// POST /api/scan/skills/capture
pub(crate) fn capture(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.capture_current_page(), &CAPTURE_FIELDS))
}

/// POST /api/scan/skills/cancel
pub(crate) fn cancel(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.cancel(), &STATUS_FIELDS))
}

/// POST /api/scan/skills/undo
pub(crate) fn undo(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.undo_last_capture(), &UNDO_FIELDS))
}

/// POST /api/scan/skills/process: kicks extraction off on a worker thread.
pub(crate) fn process(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.process(), &STATUS_FIELDS))
}

/// POST /api/scan/skills/accept
pub(crate) fn accept(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.accept(), &ACCEPT_FIELDS))
}

/// POST /api/scan/skills/reject
pub(crate) fn reject(scan: &Arc<SkillScanManual>) -> Response<Body> {
    plain_json_response(&project(&scan.reject(), &REJECT_FIELDS))
}

/// GET /api/scan/skills/pending: the held OCR result as a `{name: level}`
/// object, or the reference's 404 when none awaits review. The 200 is
/// ETag-scoped; the 404 (non-2xx) is not.
pub(crate) fn pending(scan: &Arc<SkillScanManual>, if_none_match: Option<&str>) -> Response<Body> {
    match scan.get_pending_result() {
        None => error_response(
            StatusCode::NOT_FOUND,
            &detail("No pending skill scan result"),
        ),
        Some(pairs) => {
            // The held result merges in page order with later pages
            // overwriting duplicates, so the ordered pairs become an object
            // preserving that first-seen order (serde_json's preserve-order
            // map), matching the reference's `dict` serialisation.
            let mut skills = Map::new();
            for (name, level) in pairs {
                skills.insert(name, json!(level));
            }
            json_response(&json!({ "skills": Value::Object(skills) }), if_none_match)
        }
    }
}

/// GET /api/scan/skills/capture/{page}: the stored PNG for a 1-indexed page,
/// or the reference's 404. The 200 PNG rides the same conditional-GET
/// contract the JSON reads do (the ETag middleware covers any media type in
/// scope).
pub(crate) fn capture_png(
    scan: &Arc<SkillScanManual>,
    page: i64,
    if_none_match: Option<&str>,
) -> Response<Body> {
    match scan.get_capture_png(page) {
        None => error_response(StatusCode::NOT_FOUND, &detail("Capture not available")),
        Some(png) => conditional_response(png, "image/png", if_none_match),
    }
}

/// POST /api/tracking/session/{session_id}/repair-scan: run the repair-cost
/// OCR, gated on the `repair_ocr_enabled` config flag (400 when disabled,
/// exactly as the reference). A plain 200 (POST, outside the ETag scope); the
/// failure legs ride the body with the declared fields first, then the extra
/// `error` key, as the `extra="allow"` model serialises them.
pub(crate) fn repair_scan(repair: &Arc<RepairOcrService>, enabled: bool) -> Response<Body> {
    if !enabled {
        return error_response(StatusCode::BAD_REQUEST, &detail("Repair OCR is disabled"));
    }
    plain_json_response(&project(&repair.scan_repair_cost(), &REPAIR_FIELDS))
}
