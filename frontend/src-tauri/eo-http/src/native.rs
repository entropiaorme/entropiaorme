//! Native route adapters: the HTTP-request face of the proven
//! [`hydration`](crate::hydration) handlers.
//!
//! Each adapter extracts its route's parameters through
//! [`extract`](crate::extract) (reproducing the backend's validation
//! envelopes), calls the corresponding handler, and returns its
//! response. Adapters proxy to the sidecar when the native services are
//! not composed (a substrate built without a database keeps serving
//! every route through the proxy arm, exactly as before composition).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, Response, StatusCode};
use axum::routing::MethodFilter;
use axum::Router;

use crate::extract::{
    decode_path_segment, literal_or_default, require_bounded_int, require_str, QueryString,
    Validation,
};
use crate::{arm_routed, AppState};

/// `If-None-Match`, as the backend's middleware reads it (a non-UTF-8
/// value reads as absent).
fn if_none_match(req: &Request) -> Option<String> {
    req.headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The framework-level 404 the backend serves when no route matches
/// (a percent-encoded slash inside a path parameter de-matches the
/// route there, because the backend decodes the path before matching).
fn router_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"detail\":\"Not Found\"}"))
        .expect("static 404 builds")
}

macro_rules! simple_get {
    ($fn_name:ident, $handler:ident) => {
        async fn $fn_name(state: Arc<AppState>, req: Request) -> Response<Body> {
            let Some(hydration) = state.hydration() else {
                return state.proxy(req).await;
            };
            let inm = if_none_match(&req);
            hydration.$handler(inm.as_deref()).await
        }
    };
}

simple_get!(quests_list, list_quests);
simple_get!(quests_mobs, list_mob_names);
simple_get!(quests_analytics, quest_analytics);
simple_get!(playlists_list, list_playlists);
simple_get!(playlists_analytics, playlist_analytics);
simple_get!(codex_species, codex_species);
simple_get!(codex_meta_attributes, codex_meta_attributes);

/// GET /api/codex/species/{name}/ranks: the one path-parameter route of
/// this surface. The raw segment percent-decodes before the lookup; a
/// decoded slash reproduces the backend's route-level 404.
async fn codex_species_ranks(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let path = req.uri().path();
    let raw_name = path
        .strip_prefix("/api/codex/species/")
        .and_then(|rest| rest.strip_suffix("/ranks"))
        .unwrap_or_default();
    let name = decode_path_segment(raw_name);
    if name.contains('/') {
        return router_not_found();
    }
    let inm = if_none_match(&req);
    hydration.codex_species_ranks(&name, inm.as_deref()).await
}

/// GET /api/codex/recommend: the constrained-parameter route. The
/// extraction layer validates in route-signature order (species_name,
/// rank, profession, target) and answers violations with the backend's
/// 422 envelope.
async fn codex_recommend(state: Arc<AppState>, req: Request) -> Response<Body> {
    let Some(hydration) = state.hydration() else {
        return state.proxy(req).await;
    };
    let query = QueryString::parse(req.uri().query());
    let mut validation = Validation::new();
    let species = require_str(&mut validation, &query, "species_name");
    let rank = require_bounded_int(&mut validation, &query, "rank", 1, 25);
    let profession = query.last("profession");
    let target = literal_or_default(
        &mut validation,
        &query,
        "target",
        &["profession", "hp"],
        "profession",
    );
    if !validation.is_ok() {
        return validation.into_response();
    }
    let (species, rank, target) = (
        species.expect("validated"),
        rank.expect("validated"),
        target.expect("validated"),
    );
    let inm = if_none_match(&req);
    hydration
        .codex_recommend(species, rank, profession, target, inm.as_deref())
        .await
}

/// Register the natively-served quests/codex hydration GETs; one
/// `arm_routed` line per route, the registration order mirroring the
/// takeover record. Each line is individually revertable, and the arm
/// override covers every one at runtime.
pub(crate) fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/quests",
            arm_routed(MethodFilter::GET, "/api/quests", quests_list),
        )
        .route(
            "/api/quests/mobs",
            arm_routed(MethodFilter::GET, "/api/quests/mobs", quests_mobs),
        )
        .route(
            "/api/quests/analytics",
            arm_routed(MethodFilter::GET, "/api/quests/analytics", quests_analytics),
        )
        .route(
            "/api/quests/playlists",
            arm_routed(MethodFilter::GET, "/api/quests/playlists", playlists_list),
        )
        .route(
            "/api/quests/playlists/analytics",
            arm_routed(
                MethodFilter::GET,
                "/api/quests/playlists/analytics",
                playlists_analytics,
            ),
        )
        .route(
            "/api/codex/species",
            arm_routed(MethodFilter::GET, "/api/codex/species", codex_species),
        )
        .route(
            "/api/codex/species/{name}/ranks",
            arm_routed(
                MethodFilter::GET,
                "/api/codex/species/{name}/ranks",
                codex_species_ranks,
            ),
        )
        .route(
            "/api/codex/recommend",
            arm_routed(MethodFilter::GET, "/api/codex/recommend", codex_recommend),
        )
        .route(
            "/api/codex/meta/attributes",
            arm_routed(
                MethodFilter::GET,
                "/api/codex/meta/attributes",
                codex_meta_attributes,
            ),
        )
}
