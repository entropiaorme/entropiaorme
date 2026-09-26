//! Behavioural pins for the codex family over the typed facade: the
//! reads over an empty catalogue (the species listing, the fixed
//! meta-attribute set, a missing-species recommendation), the
//! path-parameter not-found, the recommend rank bound, and the write
//! ladder (calibrate's success and out-of-domain refusal, the claim /
//! unclaim / meta-claim error mapping, a meta-claim success).

use std::path::Path;
use std::sync::Arc;

use eo_api::codex::CodexRecommendTarget;
use eo_api::{Api, ApiError};
use eo_services::clock::RealClock;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;

mod common;

/// The composed facade over a fresh migrated database and an empty
/// catalogue snapshot (no mobs carry codex data, so the species listing
/// and recommendations are empty and every species lookup misses).
async fn codex_api(dir: &Path) -> Api {
    let snapshot = dir.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = Db::open(&data_dir.join("entropia_orme.db"))
        .await
        .expect("migrated database");
    let game_data = Arc::new(GameDataStore::new(&snapshot).expect("empty game-data store"));
    let clock = Arc::new(RealClock::new());
    let handles = common::producer_handles(&db, &data_dir, tokio::runtime::Handle::current()).await;
    Api::new(
        db,
        game_data,
        clock,
        data_dir,
        handles.config_service,
        handles.tracker,
        handles.hotbar,
        handles.watcher,
        handles.skill_tracker,
        handles.skill_scan,
        handles.spacebar,
        handles.repair_ocr,
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reads_answer_the_empty_catalogue_the_backend_way() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // The species listing and a missing-species recommendation are empty
    // over a catalogue with no codex data.
    assert!(api.codex_species().await.unwrap().is_empty());
    assert!(api
        .codex_recommend("X", 4, &[], CodexRecommendTarget::Profession)
        .await
        .unwrap()
        .is_empty());

    // The meta-attribute set is fixed; levels hydrate from the (empty)
    // calibration tables, so every attribute's current level is unset.
    let attributes = api.codex_meta_attributes().await.unwrap();
    let names: Vec<&str> = attributes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Agility",
            "Health",
            "Intelligence",
            "Psyche",
            "Stamina",
            "Strength"
        ]
    );
    assert!(attributes.iter().all(|a| a.current_level.is_none()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_species_ranks_lookup_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    assert_eq!(
        api.codex_species_ranks("No Such").await.unwrap_err(),
        ApiError::not_found("Species 'No Such' not found")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_recommend_rank_bound_is_a_typed_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // The route's rank 422 (below 1, above 25) is now a typed bad_request
    // on the i64 argument.
    for rank in [0, 26] {
        assert_eq!(
            api.codex_recommend("X", rank, &[], CodexRecommendTarget::Profession)
                .await
                .unwrap_err(),
            ApiError::bad_request("rank must be between 1 and 25")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calibrate_writes_and_bounds_the_rank() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // The write succeeds with no catalogue (calibrate sets the rank
    // directly).
    let result = api.codex_calibrate("Sp", 7).await.unwrap();
    assert_eq!(result.species_name, "Sp");
    assert_eq!(result.rank, 7);

    // The out-of-domain rank is the service's bad_request (the HTTP 400).
    assert_eq!(
        api.codex_calibrate("Sp", 26).await.unwrap_err(),
        ApiError::bad_request("Rank must be 0-25")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_error_ladder_maps_invalid_to_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // A claim against a species absent from the catalogue: the service's
    // not-found is a bad_request (idle session, so no suppression fires).
    assert!(matches!(
        api.codex_claim("Notaspecies", 1, "Anatomy")
            .await
            .unwrap_err(),
        ApiError::BadRequest { .. }
    ));

    // Unclaim over a species with no claimed rank: the service's
    // nothing-to-unclaim message, verbatim.
    assert_eq!(
        api.codex_unclaim("Notaspecies").await.unwrap_err(),
        ApiError::bad_request("No claimed rank to unclaim for 'Notaspecies'")
    );

    // A meta claim for a non-attribute is a bad_request.
    assert!(matches!(
        api.codex_meta_claim("Notanattribute").await.unwrap_err(),
        ApiError::BadRequest { .. }
    ));

    // A meta claim for a real attribute succeeds (no catalogue needed).
    let result = api.codex_meta_claim("Health").await.unwrap();
    assert_eq!(result.attribute_name, "Health");
    assert_eq!(result.ped_value, 1.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mastery_writes_map_their_refusals_to_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // A mastery claim against a species absent from the catalogue (idle
    // session, so no suppression fires).
    assert!(matches!(
        api.codex_mastery_claim("Notaspecies", "Aim")
            .await
            .unwrap_err(),
        ApiError::BadRequest { .. }
    ));

    // A mastery unclaim with nothing claimed: the service's message,
    // verbatim.
    assert_eq!(
        api.codex_mastery_unclaim("Notaspecies").await.unwrap_err(),
        ApiError::bad_request("No mastery claim to unclaim for 'Notaspecies'")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mastery_options_are_catalogue_independent() {
    let dir = tempfile::tempdir().unwrap();
    let api = codex_api(dir.path()).await;

    // The eligible skills and their fixed rewards derive from constants,
    // so the full set answers even over an empty catalogue: every
    // cat1-cat3 skill once, no cat4, the three per-category value tiers.
    let options = api
        .codex_mastery_options(&[], CodexRecommendTarget::Profession)
        .await
        .unwrap();
    assert_eq!(options.len(), 36);
    assert!(options.iter().all(|option| option.category != "cat4"));
    let reward = |name: &str| {
        options
            .iter()
            .find(|option| option.skill_name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .reward_ped
    };
    assert_eq!(reward("Aim"), 25.0);
    assert_eq!(reward("Melee Combat"), 15.625);
    assert_eq!(reward("Evade"), 7.8125);
}
