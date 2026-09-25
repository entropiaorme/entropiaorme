//! The typed IPC commands: one thin `#[tauri::command]` per facade
//! operation, each delegating to the composed [`eo_api::Api`].
//!
//! Commands declare `rename_all = "snake_case"` so the invoke argument
//! keys are exactly the Rust parameter names, which is also what the
//! command manifest records and the generated TypeScript sends; the
//! parity test below holds the registered set and the manifest
//! together. Until composition publishes the facade the commands answer
//! the typed unavailable error (the frontend re-drives its reads on the
//! substrate-installed event).

use std::sync::Arc;

use eo_api::activities::{ActivityOptionsResult, ActivityStateResult, ActivityTargetKind};
use eo_api::analytics::{
    ActivityHistoryEntry, ActivityUndoInput, AnalyticsHarvest, AnalyticsHunting,
    AnalyticsHuntingActivity, AnalyticsOverview, AuctionConfirmInput, AuctionExpireInput,
    AuctionListing, AuctionListingInput, EquipmentListingInput, EquipmentTradeInput,
    HuntingRealisedMarkup, InventoryDraftResolution, InventoryItem, InventoryItemInput,
    InventoryPatch, InventorySaleDraft, InventorySellInput, InventorySellResult, LedgerEntryInput,
    LedgerItem, LedgerPage, LedgerPreset, LedgerPresetInput, LedgerSummary, PrivateSaleInput,
    Profession, RealisedTierMarkup, SaleWindowCapture, ShrapnelConversionInput,
    StockConversionInput, StockPosition, StockRemovalInput,
};
use eo_api::character::{
    ActivityRecommenderQuery, ActivityRecommenderResult, CalibrationStatus,
    CharacterProspectOptions, ComputedCharacterStats, HpOptimizerResult, PathOptimizerResult,
    ProfessionLevel, ProfessionOptimizerResult, ProspectQuery, ProspectResult, SkillLevel,
};
use eo_api::codex::{
    CodexCalibrateResult, CodexClaimResult, CodexMasteryClaimResult, CodexMetaAttribute,
    CodexMetaClaimResult, CodexRecommendTarget, CodexSkillOption, CodexSpecies, CodexSpeciesRanks,
};
use eo_api::dev::{
    AuctionFeeOverlayStatus, AuctionFeeResearchStatus, CompactResult, CrashReportingStatus,
    MetricsSnapshot, RebuildReport,
};
use eo_api::equipment::{
    EquipmentDetail, EquipmentRequest, EquipmentSearchHit, EquipmentSummary, SearchKind,
};
use eo_api::healing::{
    HealingCorrectionTarget, HealingCorrectionTool, HealingOutputClassification, HealingOutputPage,
};
use eo_api::maps::{
    CoordCalibrationStatus, CoordScanResult, MapPin, MapPinInput, MapPinPatch, MapView,
    NavigationPositionResult, NavigationRun, NearbyMapPin, PinConfig, PinConfigEditInput,
    PinConfigInput, PlanetMap, RadarCalibrationStatus, RadarGeometry,
};
use eo_api::market::{
    MarketAuctionPacketThreshold, MarketBreakEven, MarketCommitResult, MarketContributionBatch,
    MarketHarvestData, MarketHistoryPoint, MarketHorizon, MarketMobRankingRow, MarketOverviewRow,
    MarketPastePreview, MarketUnitPriceResult,
};
use eo_api::protection::{
    ProtectionObservationInput, ProtectionObservationOutcome, ProtectionOverview,
    ProtectionRecordingCandidates, ProtectionRepairInput, ProtectionRepairOutcome,
    ProtectionScanResult, ProtectionSessionStatus, ProtectionSetInput, ProtectionSetUpdateInput,
    ProtectionStream, ProtectionUndoTarget,
};
use eo_api::quests::{
    Quest, QuestAnalyticsRow, QuestFamily, QuestFamilyInput, QuestHandInState, QuestInput,
    QuestRewardReviewInput, UnresolvedQuestReward,
};
use eo_api::scan::{
    AcceptResult, CaptureResult, RejectResult, ScanStatus, SkillScanPending, SpacebarResult,
    UndoResult,
};
use eo_api::session_definitions::{SessionDefinition, SessionDefinitionInput};
use eo_api::settings::{AppSettings, OverlayPosition, SettingsPatch};
use eo_api::tracking::{
    ArmourCostResult, DefinitionSelectResult, LootItemEditResult, ManualMobLockResult,
    ManualMobSuggestion, MobEditResult, ReleaseResult, RepairScanResult, SessionConfigResult,
    SessionDetail, SessionIntervals, SessionPage, SessionReassignResult, StartResult, StopResult,
    TrackingSnapshot,
};
use eo_api::weapons::{
    WeaponCorrectionWeapon, WeaponMismatchDecision, WeaponShotGroup, WeaponShotPage,
};
use eo_api::ApiError;
use eo_api::Nullable;
use tauri::Emitter;

/// Holds the composed facade for the typed commands, published by
/// `install_native_services` once every service is present.
pub struct ApiFacade(pub Arc<eo_api::Api>);

/// The composed facade, or the typed not-ready error during the startup
/// window (and permanently if composition declined).
pub(crate) fn facade(app: &tauri::AppHandle) -> Result<Arc<eo_api::Api>, ApiError> {
    use tauri::Manager as _;
    app.try_state::<ApiFacade>()
        .map(|state| state.0.clone())
        .ok_or(ApiError::Unavailable)
}

/// The analytics fixture-backed read for the native-shell e2e build: the
/// live analytics surface is served by typed commands, so the e2e build
/// serves the same committed analytics fixture through these commands
/// (deserialised into their DTO), keeping the visual baselines stable.
#[cfg(feature = "e2e-stub")]
fn e2e_analytics<T: serde::de::DeserializeOwned>(key: &str) -> Result<T, ApiError> {
    serde_json::from_value(crate::e2e_stub::analytics_fixture(key))
        .map_err(ApiError::internal("e2e analytics fixture"))
}

/// The dashboard fixture value under `key`, deserialised into the typed
/// tracking read command's DTO: the live tracking snapshot and
/// session-detail reads migrated to typed commands, so the e2e build
/// serves the same committed dashboard fixture through them (the sessions
/// list rides the analytics fixture; see [`e2e_analytics`]).
#[cfg(feature = "e2e-stub")]
fn e2e_dashboard<T: serde::de::DeserializeOwned>(key: &str) -> Result<T, ApiError> {
    serde_json::from_value(crate::e2e_stub::dashboard_fixture(key))
        .map_err(ApiError::internal("e2e dashboard fixture"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_search(
    app: tauri::AppHandle,
    q: String,
    kind: SearchKind,
) -> Result<Vec<EquipmentSearchHit>, ApiError> {
    facade(&app)?.equipment_search(&q, kind).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_library(app: tauri::AppHandle) -> Result<Vec<EquipmentSummary>, ApiError> {
    facade(&app)?.equipment_library().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_add(
    app: tauri::AppHandle,
    req: EquipmentRequest,
) -> Result<EquipmentSummary, ApiError> {
    facade(&app)?.equipment_add(&req).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_update(
    app: tauri::AppHandle,
    item_id: i64,
    req: EquipmentRequest,
) -> Result<EquipmentSummary, ApiError> {
    facade(&app)?.equipment_update(item_id, &req).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_delete(app: tauri::AppHandle, item_id: i64) -> Result<(), ApiError> {
    facade(&app)?.equipment_delete(item_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn equipment_detail(
    app: tauri::AppHandle,
    item_id: i64,
) -> Result<EquipmentDetail, ApiError> {
    facade(&app)?.equipment_detail(item_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_overview(app: tauri::AppHandle) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_overview().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_set_create(
    app: tauri::AppHandle,
    input: ProtectionSetInput,
) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_set_create(&input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_set_update(
    app: tauri::AppHandle,
    set_id: i64,
    input: ProtectionSetUpdateInput,
) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_set_update(set_id, &input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_set_archive(
    app: tauri::AppHandle,
    set_id: i64,
) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_set_archive(set_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_set_restore(
    app: tauri::AppHandle,
    set_id: i64,
) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_set_restore(set_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_undo(
    app: tauri::AppHandle,
    target: ProtectionUndoTarget,
) -> Result<ProtectionOverview, ApiError> {
    facade(&app)?.protection_undo(target).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_unrecorded_sessions(
    app: tauri::AppHandle,
    session_ids: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    facade(&app)?
        .protection_unrecorded_sessions(session_ids)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_session_status(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<ProtectionSessionStatus, ApiError> {
    facade(&app)?.protection_session_status(session_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_recording_candidates(
    app: tauri::AppHandle,
    stream: ProtectionStream,
) -> Result<ProtectionRecordingCandidates, ApiError> {
    facade(&app)?.protection_recording_candidates(stream).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_observation_confirm(
    app: tauri::AppHandle,
    input: ProtectionObservationInput,
) -> Result<ProtectionObservationOutcome, ApiError> {
    facade(&app)?.protection_observation_confirm(&input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_repair_confirm(
    app: tauri::AppHandle,
    input: ProtectionRepairInput,
) -> Result<ProtectionRepairOutcome, ApiError> {
    facade(&app)?.protection_repair_confirm(&input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn protection_trade_terminal_scan(
    app: tauri::AppHandle,
) -> Result<ProtectionScanResult, ApiError> {
    facade(&app)?.protection_trade_terminal_scan()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn healing_outputs(
    app: tauri::AppHandle,
    session_id: String,
    classification: HealingOutputClassification,
    offset: i64,
    limit: i64,
) -> Result<HealingOutputPage, ApiError> {
    facade(&app)?
        .healing_outputs(session_id, classification, offset, limit)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn healing_correction_tools(
    app: tauri::AppHandle,
    output_id: String,
) -> Result<Vec<HealingCorrectionTool>, ApiError> {
    facade(&app)?.healing_correction_tools(output_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn healing_correct(
    app: tauri::AppHandle,
    target: HealingCorrectionTarget,
) -> Result<SessionDetail, ApiError> {
    facade(&app)?.healing_correct(target).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn healing_correction_undo(
    app: tauri::AppHandle,
    correction_id: String,
) -> Result<SessionDetail, ApiError> {
    facade(&app)?.healing_correction_undo(correction_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn weapon_shots(
    app: tauri::AppHandle,
    session_id: String,
    group: WeaponShotGroup,
    offset: i64,
    limit: i64,
) -> Result<WeaponShotPage, ApiError> {
    facade(&app)?
        .weapon_shots(session_id, group, offset, limit)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn weapon_unpriced_sessions(
    app: tauri::AppHandle,
    session_ids: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    facade(&app)?.weapon_unpriced_sessions(session_ids).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn weapon_correction_weapons(
    app: tauri::AppHandle,
    evidence_id: String,
) -> Result<Vec<WeaponCorrectionWeapon>, ApiError> {
    facade(&app)?.weapon_correction_weapons(evidence_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn weapon_assign(
    app: tauri::AppHandle,
    evidence_id: String,
    equipment_id: i64,
) -> Result<SessionDetail, ApiError> {
    facade(&app)?.weapon_assign(evidence_id, equipment_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn weapon_assignment_undo(
    app: tauri::AppHandle,
    correction_id: String,
) -> Result<SessionDetail, ApiError> {
    facade(&app)?.weapon_assignment_undo(correction_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_calibration(app: tauri::AppHandle) -> Result<CalibrationStatus, ApiError> {
    facade(&app)?.character_calibration().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_stats(app: tauri::AppHandle) -> Result<ComputedCharacterStats, ApiError> {
    facade(&app)?.character_stats().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_skills(app: tauri::AppHandle) -> Result<Vec<SkillLevel>, ApiError> {
    facade(&app)?.character_skills().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_professions(
    app: tauri::AppHandle,
) -> Result<Vec<ProfessionLevel>, ApiError> {
    facade(&app)?.character_professions().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_prospect_options(
    app: tauri::AppHandle,
) -> Result<CharacterProspectOptions, ApiError> {
    facade(&app)?.character_prospect_options().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_prospect(
    app: tauri::AppHandle,
    query: ProspectQuery,
) -> Result<ProspectResult, ApiError> {
    facade(&app)?.character_prospect(&query).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_profession_optimizer(
    app: tauri::AppHandle,
    profession: String,
) -> Result<ProfessionOptimizerResult, ApiError> {
    facade(&app)?
        .character_profession_optimizer(&profession)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_path_optimizer(
    app: tauri::AppHandle,
    profession: String,
    target_level: Option<f64>,
    ped_budget: Option<f64>,
) -> Result<PathOptimizerResult, ApiError> {
    facade(&app)?
        .character_path_optimizer(&profession, target_level, ped_budget)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_hp_optimizer(app: tauri::AppHandle) -> Result<HpOptimizerResult, ApiError> {
    facade(&app)?.character_hp_optimizer().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn character_activity_recommender(
    app: tauri::AppHandle,
    query: ActivityRecommenderQuery,
) -> Result<ActivityRecommenderResult, ApiError> {
    facade(&app)?.character_activity_recommender(&query).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_get(app: tauri::AppHandle) -> Result<AppSettings, ApiError> {
    facade(&app)?.settings().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_overlay_position(app: tauri::AppHandle) -> Result<OverlayPosition, ApiError> {
    facade(&app)?.settings_overlay_position().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_set_overlay_position(
    app: tauri::AppHandle,
    x: i64,
    y: i64,
) -> Result<(), ApiError> {
    facade(&app)?.settings_set_overlay_position(x, y).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_update(
    app: tauri::AppHandle,
    patch: SettingsPatch,
) -> Result<AppSettings, ApiError> {
    facade(&app)?.settings_update(patch).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_species(app: tauri::AppHandle) -> Result<Vec<CodexSpecies>, ApiError> {
    facade(&app)?.codex_species().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_species_ranks(
    app: tauri::AppHandle,
    species_name: String,
) -> Result<CodexSpeciesRanks, ApiError> {
    facade(&app)?.codex_species_ranks(&species_name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_recommend(
    app: tauri::AppHandle,
    species_name: String,
    rank: i64,
    professions: Vec<String>,
    target: CodexRecommendTarget,
) -> Result<Vec<CodexSkillOption>, ApiError> {
    facade(&app)?
        .codex_recommend(&species_name, rank, &professions, target)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_meta_attributes(
    app: tauri::AppHandle,
) -> Result<Vec<CodexMetaAttribute>, ApiError> {
    facade(&app)?.codex_meta_attributes().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_calibrate(
    app: tauri::AppHandle,
    species_name: String,
    rank: i64,
) -> Result<CodexCalibrateResult, ApiError> {
    facade(&app)?.codex_calibrate(&species_name, rank).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_claim(
    app: tauri::AppHandle,
    species_name: String,
    rank: i64,
    skill_name: String,
) -> Result<CodexClaimResult, ApiError> {
    facade(&app)?
        .codex_claim(&species_name, rank, &skill_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_unclaim(
    app: tauri::AppHandle,
    species_name: String,
) -> Result<CodexClaimResult, ApiError> {
    facade(&app)?.codex_unclaim(&species_name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_meta_claim(
    app: tauri::AppHandle,
    attribute_name: String,
) -> Result<CodexMetaClaimResult, ApiError> {
    facade(&app)?.codex_meta_claim(&attribute_name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_mastery_options(
    app: tauri::AppHandle,
    professions: Vec<String>,
    target: CodexRecommendTarget,
) -> Result<Vec<CodexSkillOption>, ApiError> {
    facade(&app)?
        .codex_mastery_options(&professions, target)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_mastery_claim(
    app: tauri::AppHandle,
    species_name: String,
    skill_name: String,
) -> Result<CodexMasteryClaimResult, ApiError> {
    facade(&app)?
        .codex_mastery_claim(&species_name, &skill_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn codex_mastery_unclaim(
    app: tauri::AppHandle,
    species_name: String,
) -> Result<CodexMasteryClaimResult, ApiError> {
    facade(&app)?.codex_mastery_unclaim(&species_name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quests_list(app: tauri::AppHandle) -> Result<Vec<Quest>, ApiError> {
    facade(&app)?.quests_list().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_get(app: tauri::AppHandle, quest_id: i64) -> Result<Quest, ApiError> {
    facade(&app)?.quest_get(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_create(app: tauri::AppHandle, input: QuestInput) -> Result<Quest, ApiError> {
    facade(&app)?.quest_create(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_update(
    app: tauri::AppHandle,
    quest_id: i64,
    input: QuestInput,
) -> Result<Quest, ApiError> {
    facade(&app)?.quest_update(quest_id, input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_delete(app: tauri::AppHandle, quest_id: i64) -> Result<(), ApiError> {
    facade(&app)?.quest_delete(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_start(app: tauri::AppHandle, quest_id: i64) -> Result<Quest, ApiError> {
    facade(&app)?.quest_start(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_complete(app: tauri::AppHandle, quest_id: i64) -> Result<Quest, ApiError> {
    facade(&app)?.quest_complete(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_hand_in_begin(
    app: tauri::AppHandle,
    quest_id: i64,
) -> Result<QuestHandInState, ApiError> {
    facade(&app)?.quest_hand_in_begin(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_hand_in_state(
    app: tauri::AppHandle,
    quest_id: i64,
) -> Result<QuestHandInState, ApiError> {
    facade(&app)?.quest_hand_in_state(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_hand_in_wait(
    app: tauri::AppHandle,
    quest_id: i64,
    after_clump_id: i64,
) -> Result<QuestHandInState, ApiError> {
    facade(&app)?
        .quest_hand_in_wait(quest_id, after_clump_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_hand_in_cancel(app: tauri::AppHandle, quest_id: i64) -> Result<(), ApiError> {
    facade(&app)?.quest_hand_in_cancel(quest_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_hand_in_confirm(
    app: tauri::AppHandle,
    quest_id: i64,
    clump_id: i64,
) -> Result<(), ApiError> {
    facade(&app)?
        .quest_hand_in_confirm(quest_id, clump_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_cancel(
    app: tauri::AppHandle,
    quest_id: i64,
    undo_reward: bool,
) -> Result<Quest, ApiError> {
    facade(&app)?.quest_cancel(quest_id, undo_reward).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quests_mobs(app: tauri::AppHandle) -> Result<Vec<String>, ApiError> {
    facade(&app)?.quests_mobs().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quests_analytics(app: tauri::AppHandle) -> Result<Vec<QuestAnalyticsRow>, ApiError> {
    facade(&app)?.quests_analytics().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_rewards_unresolved(
    app: tauri::AppHandle,
) -> Result<Vec<UnresolvedQuestReward>, ApiError> {
    facade(&app)?.quest_rewards_unresolved().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_reward_review(
    app: tauri::AppHandle,
    input: QuestRewardReviewInput,
) -> Result<(), ApiError> {
    facade(&app)?.quest_reward_review(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_families_list(app: tauri::AppHandle) -> Result<Vec<QuestFamily>, ApiError> {
    facade(&app)?.quest_families_list().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_family_create(
    app: tauri::AppHandle,
    input: QuestFamilyInput,
) -> Result<QuestFamily, ApiError> {
    facade(&app)?.quest_family_create(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_family_update(
    app: tauri::AppHandle,
    family_id: i64,
    input: QuestFamilyInput,
) -> Result<QuestFamily, ApiError> {
    facade(&app)?.quest_family_update(family_id, input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quest_family_delete(app: tauri::AppHandle, family_id: i64) -> Result<(), ApiError> {
    facade(&app)?.quest_family_delete(family_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_definitions_list(
    app: tauri::AppHandle,
    include_inactive: Option<bool>,
) -> Result<Vec<SessionDefinition>, ApiError> {
    facade(&app)?
        .session_definitions_list(include_inactive)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_definition_create(
    app: tauri::AppHandle,
    input: SessionDefinitionInput,
) -> Result<SessionDefinition, ApiError> {
    facade(&app)?.session_definition_create(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_definition_update(
    app: tauri::AppHandle,
    definition_id: i64,
    input: SessionDefinitionInput,
) -> Result<SessionDefinition, ApiError> {
    facade(&app)?
        .session_definition_update(definition_id, input)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_definition_archive(
    app: tauri::AppHandle,
    definition_id: i64,
) -> Result<SessionDefinition, ApiError> {
    facade(&app)?
        .session_definition_archive(definition_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_definition_restore(
    app: tauri::AppHandle,
    definition_id: i64,
) -> Result<SessionDefinition, ApiError> {
    facade(&app)?
        .session_definition_restore(definition_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_definition_select(
    app: tauri::AppHandle,
    definition_id: Option<i64>,
) -> Result<DefinitionSelectResult, ApiError> {
    facade(&app)?
        .tracking_definition_select(definition_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn analytics_overview(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsOverview, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &period);
        e2e_analytics("overview")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.analytics_overview(&period).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn analytics_hunting(app: tauri::AppHandle) -> Result<AnalyticsHunting, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = &app;
        e2e_analytics("hunting")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.analytics_hunting().await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn analytics_harvest(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsHarvest, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &period);
        e2e_analytics("harvest")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.analytics_harvest(&period).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn analytics_hunting_activity(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsHuntingActivity, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &period);
        e2e_analytics("huntingActivity")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.analytics_hunting_activity(&period).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn activity_stock(
    app: tauri::AppHandle,
    profession: Profession,
) -> Result<Vec<StockPosition>, ApiError> {
    facade(&app)?.activity_stock(profession).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn harvest_realised_markup(
    app: tauri::AppHandle,
) -> Result<Vec<RealisedTierMarkup>, ApiError> {
    facade(&app)?.harvest_realised_markup().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn hunting_realised_markup(
    app: tauri::AppHandle,
) -> Result<HuntingRealisedMarkup, ApiError> {
    facade(&app)?.hunting_realised_markup().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_listings(
    app: tauri::AppHandle,
    profession: Profession,
) -> Result<Vec<AuctionListing>, ApiError> {
    facade(&app)?.auction_listings(profession).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_listing_create(
    app: tauri::AppHandle,
    input: AuctionListingInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?.auction_listing_create(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_listing_confirm(
    app: tauri::AppHandle,
    input: AuctionConfirmInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?.auction_listing_confirm(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_listing_expire(
    app: tauri::AppHandle,
    input: AuctionExpireInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?.auction_listing_expire(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_convert(
    app: tauri::AppHandle,
    input: StockConversionInput,
) -> Result<(), ApiError> {
    facade(&app)?.stock_convert(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_private_sale(
    app: tauri::AppHandle,
    input: PrivateSaleInput,
) -> Result<(), ApiError> {
    facade(&app)?.stock_private_sale(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_remove(app: tauri::AppHandle, input: StockRemovalInput) -> Result<(), ApiError> {
    facade(&app)?.stock_remove(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_shrapnel_convert(
    app: tauri::AppHandle,
    input: ShrapnelConversionInput,
) -> Result<(), ApiError> {
    facade(&app)?.stock_shrapnel_convert(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn activity_history(
    app: tauri::AppHandle,
    profession: Profession,
) -> Result<Vec<ActivityHistoryEntry>, ApiError> {
    facade(&app)?.activity_history(profession).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_sale_revert(
    app: tauri::AppHandle,
    input: ActivityUndoInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?.auction_sale_revert(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auction_listing_undo(
    app: tauri::AppHandle,
    input: ActivityUndoInput,
) -> Result<(), ApiError> {
    facade(&app)?.auction_listing_undo(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_conversion_undo(
    app: tauri::AppHandle,
    input: ActivityUndoInput,
) -> Result<(), ApiError> {
    facade(&app)?.stock_conversion_undo(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn private_sale_undo(
    app: tauri::AppHandle,
    input: ActivityUndoInput,
) -> Result<(), ApiError> {
    facade(&app)?.private_sale_undo(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stock_removal_undo(
    app: tauri::AppHandle,
    input: ActivityUndoInput,
) -> Result<(), ApiError> {
    facade(&app)?.stock_removal_undo(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_list(
    app: tauri::AppHandle,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<LedgerPage, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &cursor, &limit);
        let entries: Vec<LedgerItem> = e2e_analytics("ledger")?;
        Ok(LedgerPage {
            total: entries.len() as i64,
            entries,
            next_cursor: None.into(),
        })
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.ledger_list(cursor, limit).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_summary(
    app: tauri::AppHandle,
    period: String,
) -> Result<LedgerSummary, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &period);
        // Derive the per-tag summary from the same committed ledger fixture
        // the entry list serves, keeping the net-impact card's baseline
        // consistent with the visible entries.
        let entries: Vec<LedgerItem> = e2e_analytics("ledger")?;
        let mut summary = LedgerSummary {
            gains: Default::default(),
            losses: Default::default(),
        };
        for entry in entries {
            let side = match serde_json::to_value(entry.kind) {
                Ok(value) if value == "markup" => &mut summary.gains,
                _ => &mut summary.losses,
            };
            *side.entry(entry.tag).or_insert(0.0) += entry.amount;
        }
        Ok(summary)
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.ledger_summary(&period).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_create(
    app: tauri::AppHandle,
    entry: LedgerEntryInput,
) -> Result<LedgerItem, ApiError> {
    facade(&app)?.ledger_create(entry).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_delete(app: tauri::AppHandle, entry_id: String) -> Result<(), ApiError> {
    facade(&app)?.ledger_delete(entry_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_presets_list(app: tauri::AppHandle) -> Result<Vec<LedgerPreset>, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = &app;
        e2e_analytics("presets")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.ledger_presets_list().await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_preset_create(
    app: tauri::AppHandle,
    preset: LedgerPresetInput,
) -> Result<LedgerPreset, ApiError> {
    facade(&app)?.ledger_preset_create(preset).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ledger_preset_delete(
    app: tauri::AppHandle,
    preset_id: String,
) -> Result<(), ApiError> {
    facade(&app)?.ledger_preset_delete(preset_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_list(app: tauri::AppHandle) -> Result<Vec<InventoryItem>, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = &app;
        e2e_analytics("inventory")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.inventory_list().await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_create(
    app: tauri::AppHandle,
    item: InventoryItemInput,
) -> Result<InventoryItem, ApiError> {
    facade(&app)?.inventory_create(item).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_update(
    app: tauri::AppHandle,
    item_id: String,
    patch: InventoryPatch,
) -> Result<InventoryItem, ApiError> {
    facade(&app)?.inventory_update(item_id, patch).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_delete(app: tauri::AppHandle, item_id: String) -> Result<(), ApiError> {
    facade(&app)?.inventory_delete(item_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_sell(
    app: tauri::AppHandle,
    item_id: String,
    sale: InventorySellInput,
) -> Result<InventorySellResult, ApiError> {
    facade(&app)?.inventory_sell(item_id, sale).await
}

// The capture blocks on the portal and the recogniser, so offload it the
// way the repair scan is offloaded: on a runtime worker its inner
// `block_on` panics outright, and the panic poisons the shared capture
// engine for every later scan in the process.
#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_sale_window_capture(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<SaleWindowCapture, ApiError> {
    let api = facade(&app)?;
    let capture = tokio::task::spawn_blocking(move || api.inventory_sale_window_capture())
        .await
        .map_err(|_| ApiError::invalid_state("sale window capture task failed"))??;
    // A read taken from the overlay was taken for a form the player is not
    // looking at. Waiting for the main window to be focused would leave a
    // form sitting in plain sight on a second screen, unchanged, looking
    // exactly like a capture that failed. So it is told, and fills where it
    // stands. Window orchestration, not a domain event.
    if window.label() == crate::SALE_CAPTURE_OVERLAY && capture.error.0.is_none() {
        let _ = app.emit_to("main", "sale-capture-ready", ());
    }
    Ok(capture)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_sale_window_take_capture(
    app: tauri::AppHandle,
) -> Result<Nullable<SaleWindowCapture>, ApiError> {
    Ok(facade(&app)?.inventory_sale_window_take_capture())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_draft_resolve(
    app: tauri::AppHandle,
    draft: InventorySaleDraft,
) -> Result<InventoryDraftResolution, ApiError> {
    facade(&app)?.inventory_draft_resolve(draft).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_equipment_listing_create(
    app: tauri::AppHandle,
    input: EquipmentListingInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?
        .inventory_equipment_listing_create(input)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inventory_equipment_trade(
    app: tauri::AppHandle,
    input: EquipmentTradeInput,
) -> Result<AuctionListing, ApiError> {
    facade(&app)?.inventory_equipment_trade(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_paste_preview(
    app: tauri::AppHandle,
    text: String,
) -> Result<MarketPastePreview, ApiError> {
    facade(&app)?.market_paste_preview(text).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_paste_commit(
    app: tauri::AppHandle,
    text: String,
) -> Result<MarketCommitResult, ApiError> {
    facade(&app)?.market_paste_commit(text).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_unit_price_set(
    app: tauri::AppHandle,
    item_name: String,
    ped_per_unit: f64,
) -> Result<MarketUnitPriceResult, ApiError> {
    facade(&app)?
        .market_unit_price_set(item_name, ped_per_unit)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_overview(app: tauri::AppHandle) -> Result<Vec<MarketOverviewRow>, ApiError> {
    facade(&app)?.market_overview().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_contribution_batch(
    app: tauri::AppHandle,
) -> Result<eo_api::Nullable<MarketContributionBatch>, ApiError> {
    facade(&app)?.market_contribution_batch().await
}

#[tauri::command(rename_all = "snake_case")]
pub fn market_auction_packet_threshold(
    app: tauri::AppHandle,
    max_fee_share_pct: f64,
) -> Result<MarketAuctionPacketThreshold, ApiError> {
    facade(&app)?.market_auction_packet_threshold(max_fee_share_pct)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_break_even(app: tauri::AppHandle) -> Result<MarketBreakEven, ApiError> {
    facade(&app)?.market_break_even().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_mob_ranking(
    app: tauri::AppHandle,
    horizon: MarketHorizon,
) -> Result<Vec<MarketMobRankingRow>, ApiError> {
    facade(&app)?.market_mob_ranking(horizon).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_harvest_markups(app: tauri::AppHandle) -> Result<MarketHarvestData, ApiError> {
    facade(&app)?.market_harvest_markups().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_hunt_markups(app: tauri::AppHandle) -> Result<MarketHarvestData, ApiError> {
    facade(&app)?.market_hunt_markups().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn market_item_history(
    app: tauri::AppHandle,
    item_name: String,
    horizon: MarketHorizon,
) -> Result<Vec<MarketHistoryPoint>, ApiError> {
    facade(&app)?.market_item_history(item_name, horizon).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_status(app: tauri::AppHandle) -> Result<ScanStatus, ApiError> {
    facade(&app)?.scan_status()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_start(
    app: tauri::AppHandle,
    page_count: Option<i64>,
) -> Result<ScanStatus, ApiError> {
    facade(&app)?.scan_start(page_count)
}

// The capture runs a synchronous screen grab + OCR read; offload it so a
// slow grab never ties up an async runtime worker.
#[tauri::command(rename_all = "snake_case")]
pub async fn scan_capture(app: tauri::AppHandle) -> Result<CaptureResult, ApiError> {
    let api = facade(&app)?;
    tokio::task::spawn_blocking(move || api.scan_capture())
        .await
        .map_err(|_| ApiError::invalid_state("scan capture task failed"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_cancel(app: tauri::AppHandle) -> Result<ScanStatus, ApiError> {
    facade(&app)?.scan_cancel()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_undo(app: tauri::AppHandle) -> Result<UndoResult, ApiError> {
    facade(&app)?.scan_undo()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_process(app: tauri::AppHandle) -> Result<ScanStatus, ApiError> {
    facade(&app)?.scan_process()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_accept(app: tauri::AppHandle) -> Result<AcceptResult, ApiError> {
    facade(&app)?.scan_accept()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_reject(app: tauri::AppHandle) -> Result<RejectResult, ApiError> {
    facade(&app)?.scan_reject()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_pending(app: tauri::AppHandle) -> Result<Option<SkillScanPending>, ApiError> {
    facade(&app)?.scan_pending()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_spacebar_capture(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<SpacebarResult, ApiError> {
    facade(&app)?.scan_set_spacebar_capture(enabled)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_sessions(
    app: tauri::AppHandle,
    cursor: Option<String>,
    limit: Option<i64>,
    definition_id: Option<i64>,
) -> Result<SessionPage, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &cursor, &limit);
        let sessions: Vec<eo_api::tracking::TrackingSession> =
            serde_json::from_value(crate::e2e_stub::analytics_sessions_fixture(definition_id))
                .map_err(ApiError::internal("e2e analytics fixture"))?;
        Ok(SessionPage {
            total: sessions.len() as i64,
            sessions,
            next_cursor: None.into(),
        })
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?
            .tracking_sessions(cursor, limit, definition_id)
            .await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_session_detail(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<SessionDetail, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = (&app, &session_id);
        e2e_dashboard("sessionDetail")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.tracking_session_detail(session_id).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_session_intervals(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<SessionIntervals, ApiError> {
    facade(&app)?.tracking_session_intervals(session_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_manual_mob_suggestions(
    app: tauri::AppHandle,
    q: String,
    limit: Option<i64>,
) -> Result<Vec<ManualMobSuggestion>, ApiError> {
    facade(&app)?
        .tracking_manual_mob_suggestions(q, limit)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_snapshot(app: tauri::AppHandle) -> Result<TrackingSnapshot, ApiError> {
    #[cfg(feature = "e2e-stub")]
    {
        let _ = &app;
        e2e_dashboard("snapshot")
    }
    #[cfg(not(feature = "e2e-stub"))]
    {
        facade(&app)?.tracking_snapshot().await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_start(app: tauri::AppHandle) -> Result<StartResult, ApiError> {
    facade(&app)?.tracking_start().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_stop(app: tauri::AppHandle) -> Result<StopResult, ApiError> {
    facade(&app)?.tracking_stop().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_weapon_decide(
    app: tauri::AppHandle,
    decision: WeaponMismatchDecision,
) -> Result<bool, ApiError> {
    facade(&app)?.tracking_weapon_decide(decision).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_release_mob(app: tauri::AppHandle) -> Result<ReleaseResult, ApiError> {
    facade(&app)?.tracking_release_mob().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_manual_mob_lock(
    app: tauri::AppHandle,
    species: String,
    maturity: Option<String>,
) -> Result<ManualMobLockResult, ApiError> {
    facade(&app)?
        .tracking_manual_mob_lock(species, maturity)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_session_config(
    app: tauri::AppHandle,
    session_name: Option<String>,
    skill_boost_percent: Option<i64>,
) -> Result<SessionConfigResult, ApiError> {
    facade(&app)?
        .tracking_session_config(session_name, skill_boost_percent)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_activity_options(
    app: tauri::AppHandle,
) -> Result<ActivityOptionsResult, ApiError> {
    facade(&app)?.tracking_activity_options().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_activity_activate(
    app: tauri::AppHandle,
    kind: ActivityTargetKind,
    quest_id: Option<i64>,
    label: Option<String>,
    additive: Option<bool>,
) -> Result<ActivityStateResult, ApiError> {
    facade(&app)?
        .tracking_activity_activate(kind, quest_id, label, additive)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_activity_deactivate(
    app: tauri::AppHandle,
    kind: ActivityTargetKind,
    quest_id: Option<i64>,
    label: Option<String>,
) -> Result<ActivityStateResult, ApiError> {
    facade(&app)?
        .tracking_activity_deactivate(kind, quest_id, label)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_reassign_session(
    app: tauri::AppHandle,
    session_id: String,
    definition_id: i64,
) -> Result<SessionReassignResult, ApiError> {
    facade(&app)?
        .tracking_reassign_session(session_id, definition_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_rename_mob(
    app: tauri::AppHandle,
    session_id: String,
    from_mob_name: String,
    to_mob_name: String,
) -> Result<MobEditResult, ApiError> {
    facade(&app)?
        .tracking_rename_mob(session_id, from_mob_name, to_mob_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_restore_mob(
    app: tauri::AppHandle,
    session_id: String,
    current_mob_name: String,
) -> Result<MobEditResult, ApiError> {
    facade(&app)?
        .tracking_restore_mob(session_id, current_mob_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_loot_item_activate(
    app: tauri::AppHandle,
    session_id: String,
    item_name: String,
) -> Result<LootItemEditResult, ApiError> {
    facade(&app)?
        .tracking_loot_item_activate(session_id, item_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_loot_item_deactivate(
    app: tauri::AppHandle,
    session_id: String,
    item_name: String,
) -> Result<LootItemEditResult, ApiError> {
    facade(&app)?
        .tracking_loot_item_deactivate(session_id, item_name)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_armour_cost(
    app: tauri::AppHandle,
    session_id: String,
    cost: f64,
) -> Result<ArmourCostResult, ApiError> {
    facade(&app)?.tracking_armour_cost(session_id, cost).await
}

// The repair scan runs a synchronous screen grab + one-shot OCR read;
// offload it so a slow grab never ties up an async runtime worker.
#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_repair_scan(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<RepairScanResult, ApiError> {
    let api = facade(&app)?;
    tokio::task::spawn_blocking(move || api.tracking_repair_scan(session_id))
        .await
        .map_err(|_| ApiError::invalid_state("repair scan task failed"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tracking_session_delete(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), ApiError> {
    facade(&app)?.tracking_session_delete(session_id).await
}

// The guide-mode demo reads: the frontend's guide-mode wrappers dispatch
// these instead of the live commands, sharing the same DTOs. They serve the
// parallel demo state (built lazily on first access); guide mode is never
// exercised in the native-shell e2e build, so they carry no `e2e-stub` branch.
#[tauri::command(rename_all = "snake_case")]
pub async fn demo_analytics_overview(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsOverview, ApiError> {
    facade(&app)?.demo_analytics_overview(&period).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_analytics_hunting(app: tauri::AppHandle) -> Result<AnalyticsHunting, ApiError> {
    facade(&app)?.demo_analytics_hunting().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_analytics_harvest(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsHarvest, ApiError> {
    facade(&app)?.demo_analytics_harvest(&period).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_analytics_hunting_activity(
    app: tauri::AppHandle,
    period: String,
) -> Result<AnalyticsHuntingActivity, ApiError> {
    facade(&app)?.demo_analytics_hunting_activity(&period).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_ledger_list(
    app: tauri::AppHandle,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<LedgerPage, ApiError> {
    facade(&app)?.demo_ledger_list(cursor, limit).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_ledger_summary(
    app: tauri::AppHandle,
    period: String,
) -> Result<LedgerSummary, ApiError> {
    facade(&app)?.demo_ledger_summary(&period).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_ledger_presets_list(
    app: tauri::AppHandle,
) -> Result<Vec<LedgerPreset>, ApiError> {
    facade(&app)?.demo_ledger_presets_list().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_inventory_list(app: tauri::AppHandle) -> Result<Vec<InventoryItem>, ApiError> {
    facade(&app)?.demo_inventory_list().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_tracking_sessions(
    app: tauri::AppHandle,
    cursor: Option<String>,
    limit: Option<i64>,
    definition_id: Option<i64>,
) -> Result<SessionPage, ApiError> {
    facade(&app)?
        .demo_tracking_sessions(cursor, limit, definition_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_tracking_session_detail(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<SessionDetail, ApiError> {
    facade(&app)?
        .demo_tracking_session_detail(&session_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn demo_tracking_snapshot(app: tauri::AppHandle) -> Result<TrackingSnapshot, ApiError> {
    facade(&app)?.demo_tracking_snapshot().await
}

// The hidden developer-tools family: native-only, each gated on developer
// mode (a gate-off command returns the typed not-found, kind "notFound"). The metrics read and the crash-reporting read/write are
// synchronous facade methods, so their wrappers do not `.await`.
#[tauri::command(rename_all = "snake_case")]
pub async fn dev_metrics(app: tauri::AppHandle) -> Result<MetricsSnapshot, ApiError> {
    facade(&app)?.dev_metrics()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_crash_reporting(app: tauri::AppHandle) -> Result<CrashReportingStatus, ApiError> {
    facade(&app)?.dev_crash_reporting()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_set_crash_reporting(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<CrashReportingStatus, ApiError> {
    facade(&app)?.dev_set_crash_reporting(enabled)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_compact_database(app: tauri::AppHandle) -> Result<CompactResult, ApiError> {
    facade(&app)?.dev_compact_database().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_rebuild_projections(app: tauri::AppHandle) -> Result<RebuildReport, ApiError> {
    facade(&app)?.dev_rebuild_projections().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_auction_fee_research_start(
    app: tauri::AppHandle,
) -> Result<AuctionFeeResearchStatus, ApiError> {
    let api = facade(&app)?;
    tokio::task::spawn_blocking(move || api.dev_auction_fee_research_start())
        .await
        .map_err(|_| ApiError::invalid_state("auction fee research start task failed"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_auction_fee_research_stop(
    app: tauri::AppHandle,
) -> Result<AuctionFeeResearchStatus, ApiError> {
    facade(&app)?.dev_auction_fee_research_stop()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_auction_fee_research_status(
    app: tauri::AppHandle,
) -> Result<AuctionFeeResearchStatus, ApiError> {
    facade(&app)?.dev_auction_fee_research_status()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_auction_fee_research_capture(
    app: tauri::AppHandle,
) -> Result<AuctionFeeOverlayStatus, ApiError> {
    let api = facade(&app)?;
    tokio::task::spawn_blocking(move || api.dev_auction_fee_research_capture())
        .await
        .map_err(|_| ApiError::invalid_state("auction fee capture task failed"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn dev_auction_fee_research_overlay_status(
    app: tauri::AppHandle,
) -> Result<AuctionFeeOverlayStatus, ApiError> {
    facade(&app)?.dev_auction_fee_research_overlay_status()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn planet_maps_list(app: tauri::AppHandle) -> Result<Vec<PlanetMap>, ApiError> {
    facade(&app)?.planet_maps()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pins_list(
    app: tauri::AppHandle,
    planet: String,
    map_view_id: Option<i64>,
) -> Result<Vec<MapPin>, ApiError> {
    facade(&app)?.map_pins_list(planet, map_view_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pins_viewport(
    app: tauri::AppHandle,
    planet: String,
    map_view_id: Option<i64>,
    lon_min: f64,
    lon_max: f64,
    lat_min: f64,
    lat_max: f64,
) -> Result<Vec<MapPin>, ApiError> {
    facade(&app)?
        .map_pins_viewport(planet, map_view_id, lon_min, lon_max, lat_min, lat_max)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pin_nearby(
    app: tauri::AppHandle,
    planet: String,
    map_view_id: Option<i64>,
    lon: f64,
    lat: f64,
) -> Result<Nullable<NearbyMapPin>, ApiError> {
    facade(&app)?
        .map_pin_nearby(planet, map_view_id, lon, lat)
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_views_list(
    app: tauri::AppHandle,
    planet: String,
) -> Result<Vec<MapView>, ApiError> {
    facade(&app)?.map_views_list(planet).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_view_create(
    app: tauri::AppHandle,
    planet: String,
    name: String,
) -> Result<MapView, ApiError> {
    facade(&app)?.map_view_create(planet, name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_view_rename(
    app: tauri::AppHandle,
    id: i64,
    name: String,
) -> Result<MapView, ApiError> {
    facade(&app)?.map_view_rename(id, name).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_view_delete(app: tauri::AppHandle, id: i64) -> Result<(), ApiError> {
    facade(&app)?.map_view_delete(id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pin_create(app: tauri::AppHandle, pin: MapPinInput) -> Result<MapPin, ApiError> {
    facade(&app)?.map_pin_create(pin).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pin_update(
    app: tauri::AppHandle,
    id: i64,
    patch: MapPinPatch,
) -> Result<MapPin, ApiError> {
    facade(&app)?.map_pin_update(id, patch).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pin_delete(app: tauri::AppHandle, id: i64) -> Result<(), ApiError> {
    facade(&app)?.map_pin_delete(id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn map_pin_cooldown(app: tauri::AppHandle, id: i64) -> Result<MapPin, ApiError> {
    facade(&app)?.map_pin_cooldown(id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_configs_list(
    app: tauri::AppHandle,
    planet: String,
    map_view_id: Option<i64>,
) -> Result<Vec<PinConfig>, ApiError> {
    facade(&app)?.pin_configs_list(planet, map_view_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_config_create(
    app: tauri::AppHandle,
    input: PinConfigInput,
) -> Result<PinConfig, ApiError> {
    facade(&app)?.pin_config_create(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_config_update(
    app: tauri::AppHandle,
    id: i64,
    input: PinConfigEditInput,
) -> Result<PinConfig, ApiError> {
    facade(&app)?.pin_config_update(id, input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_config_delete(app: tauri::AppHandle, id: i64) -> Result<(), ApiError> {
    facade(&app)?.pin_config_delete(id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_config_reorder(app: tauri::AppHandle, ids: Vec<i64>) -> Result<(), ApiError> {
    facade(&app)?.pin_config_reorder(ids).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maps_calibration_start(
    app: tauri::AppHandle,
) -> Result<CoordCalibrationStatus, ApiError> {
    facade(&app)?.maps_calibration_start()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maps_calibration_cancel(
    app: tauri::AppHandle,
) -> Result<CoordCalibrationStatus, ApiError> {
    facade(&app)?.maps_calibration_cancel()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn maps_calibration_status(
    app: tauri::AppHandle,
) -> Result<CoordCalibrationStatus, ApiError> {
    facade(&app)?.maps_calibration_status()
}

// The scan runs a synchronous screen grab + OCR read; offload it so a
// slow grab never ties up an async runtime worker.
#[tauri::command(rename_all = "snake_case")]
pub async fn maps_scan_coordinates(
    app: tauri::AppHandle,
    planet: Option<String>,
) -> Result<CoordScanResult, ApiError> {
    let api = facade(&app)?;
    tokio::task::spawn_blocking(move || api.maps_scan_coordinates(planet))
        .await
        .map_err(|_| ApiError::invalid_state("coordinate scan task failed"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_snapshot(
    app: tauri::AppHandle,
) -> Result<Nullable<NavigationRun>, ApiError> {
    facade(&app)?.navigation_snapshot().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_start(
    app: tauri::AppHandle,
    planet: String,
    map_view_id: Option<i64>,
    start_lon: f64,
    start_lat: f64,
    selected_pin_ids: Option<Vec<i64>>,
    hotkey: String,
) -> Result<NavigationRun, ApiError> {
    facade(&app)?
        .navigation_start(
            planet,
            map_view_id,
            start_lon,
            start_lat,
            selected_pin_ids,
            hotkey,
        )
        .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_update_position(
    app: tauri::AppHandle,
) -> Result<NavigationPositionResult, ApiError> {
    facade(&app)?.navigation_update_position().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_mark_visited(
    app: tauri::AppHandle,
    force: bool,
) -> Result<NavigationPositionResult, ApiError> {
    facade(&app)?.navigation_mark_visited(force).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_skip(app: tauri::AppHandle) -> Result<NavigationRun, ApiError> {
    facade(&app)?.navigation_skip().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_resolve_harvest(
    app: tauri::AppHandle,
    confirm: bool,
) -> Result<NavigationRun, ApiError> {
    facade(&app)?.navigation_resolve_harvest(confirm).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_undo(app: tauri::AppHandle) -> Result<NavigationRun, ApiError> {
    facade(&app)?.navigation_undo().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn navigation_end(app: tauri::AppHandle) -> Result<(), ApiError> {
    facade(&app)?.navigation_end().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn radar_calibration_start(
    app: tauri::AppHandle,
) -> Result<RadarCalibrationStatus, ApiError> {
    facade(&app)?.radar_calibration_start()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn radar_calibration_cancel(app: tauri::AppHandle) -> Result<(), ApiError> {
    facade(&app)?.radar_calibration_cancel()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn radar_calibration_status(
    app: tauri::AppHandle,
) -> Result<RadarCalibrationStatus, ApiError> {
    facade(&app)?.radar_calibration_status()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn radar_geometry(app: tauri::AppHandle) -> Result<Nullable<RadarGeometry>, ApiError> {
    facade(&app)?.radar_geometry().await
}

#[cfg(test)]
mod tests {
    /// The commands this module defines and the `generate_handler!`
    /// registration site hold in lock-step with the manifest (the
    /// single source the TypeScript bindings emit from); a command
    /// present on one side and absent on the other is a wiring bug
    /// this catches at test time.
    const TYPED_COMMANDS: &[&str] = &[
        "equipment_search",
        "equipment_library",
        "equipment_add",
        "equipment_update",
        "equipment_delete",
        "equipment_detail",
        "protection_overview",
        "protection_set_create",
        "protection_set_update",
        "protection_set_archive",
        "protection_set_restore",
        "protection_undo",
        "protection_unrecorded_sessions",
        "protection_session_status",
        "protection_recording_candidates",
        "protection_observation_confirm",
        "protection_repair_confirm",
        "protection_trade_terminal_scan",
        "healing_outputs",
        "healing_correction_tools",
        "healing_correct",
        "healing_correction_undo",
        "weapon_shots",
        "weapon_unpriced_sessions",
        "weapon_correction_weapons",
        "weapon_assign",
        "weapon_assignment_undo",
        "character_calibration",
        "character_stats",
        "character_skills",
        "character_professions",
        "character_prospect_options",
        "character_prospect",
        "character_profession_optimizer",
        "character_path_optimizer",
        "character_hp_optimizer",
        "character_activity_recommender",
        "settings_get",
        "settings_overlay_position",
        "settings_set_overlay_position",
        "settings_update",
        "codex_species",
        "codex_species_ranks",
        "codex_recommend",
        "codex_meta_attributes",
        "codex_calibrate",
        "codex_claim",
        "codex_unclaim",
        "codex_meta_claim",
        "codex_mastery_options",
        "codex_mastery_claim",
        "codex_mastery_unclaim",
        "quests_list",
        "quest_get",
        "quest_create",
        "quest_update",
        "quest_delete",
        "quest_start",
        "quest_complete",
        "quest_hand_in_begin",
        "quest_hand_in_state",
        "quest_hand_in_wait",
        "quest_hand_in_cancel",
        "quest_hand_in_confirm",
        "quest_rewards_unresolved",
        "quest_reward_review",
        "quest_cancel",
        "quests_mobs",
        "quests_analytics",
        "quest_families_list",
        "quest_family_create",
        "quest_family_update",
        "quest_family_delete",
        "session_definitions_list",
        "session_definition_create",
        "session_definition_update",
        "session_definition_archive",
        "session_definition_restore",
        "tracking_definition_select",
        "analytics_overview",
        "analytics_hunting",
        "analytics_harvest",
        "analytics_hunting_activity",
        "activity_stock",
        "harvest_realised_markup",
        "hunting_realised_markup",
        "auction_listings",
        "auction_listing_create",
        "auction_listing_confirm",
        "auction_listing_expire",
        "stock_convert",
        "stock_private_sale",
        "stock_remove",
        "stock_shrapnel_convert",
        "activity_history",
        "auction_sale_revert",
        "auction_listing_undo",
        "stock_conversion_undo",
        "private_sale_undo",
        "stock_removal_undo",
        "ledger_list",
        "ledger_summary",
        "ledger_create",
        "ledger_delete",
        "ledger_presets_list",
        "ledger_preset_create",
        "ledger_preset_delete",
        "inventory_list",
        "inventory_create",
        "inventory_update",
        "inventory_delete",
        "inventory_sell",
        "inventory_sale_window_capture",
        "inventory_sale_window_take_capture",
        "inventory_draft_resolve",
        "inventory_equipment_listing_create",
        "inventory_equipment_trade",
        "market_paste_preview",
        "market_paste_commit",
        "market_unit_price_set",
        "market_overview",
        "market_contribution_batch",
        "market_auction_packet_threshold",
        "market_break_even",
        "market_mob_ranking",
        "market_harvest_markups",
        "market_hunt_markups",
        "market_item_history",
        "scan_status",
        "scan_start",
        "scan_capture",
        "scan_cancel",
        "scan_undo",
        "scan_process",
        "scan_accept",
        "scan_reject",
        "scan_pending",
        "scan_spacebar_capture",
        "tracking_sessions",
        "tracking_session_detail",
        "tracking_session_intervals",
        "tracking_manual_mob_suggestions",
        "tracking_snapshot",
        "tracking_start",
        "tracking_stop",
        "tracking_weapon_decide",
        "tracking_release_mob",
        "tracking_manual_mob_lock",
        "tracking_session_config",
        "tracking_activity_options",
        "tracking_activity_activate",
        "tracking_activity_deactivate",
        "tracking_reassign_session",
        "tracking_rename_mob",
        "tracking_restore_mob",
        "tracking_loot_item_activate",
        "tracking_loot_item_deactivate",
        "tracking_armour_cost",
        "tracking_repair_scan",
        "tracking_session_delete",
        "demo_analytics_overview",
        "demo_analytics_hunting",
        "demo_analytics_hunting_activity",
        "demo_analytics_harvest",
        "demo_ledger_list",
        "demo_ledger_summary",
        "demo_ledger_presets_list",
        "demo_inventory_list",
        "demo_tracking_sessions",
        "demo_tracking_session_detail",
        "demo_tracking_snapshot",
        "dev_metrics",
        "dev_crash_reporting",
        "dev_set_crash_reporting",
        "dev_compact_database",
        "dev_rebuild_projections",
        "dev_auction_fee_research_start",
        "dev_auction_fee_research_stop",
        "dev_auction_fee_research_status",
        "dev_auction_fee_research_capture",
        "dev_auction_fee_research_overlay_status",
        "planet_maps_list",
        "map_pins_list",
        "map_pins_viewport",
        "map_pin_nearby",
        "map_views_list",
        "map_view_create",
        "map_view_rename",
        "map_view_delete",
        "map_pin_create",
        "map_pin_update",
        "map_pin_delete",
        "map_pin_cooldown",
        "pin_configs_list",
        "pin_config_create",
        "pin_config_update",
        "pin_config_delete",
        "pin_config_reorder",
        "maps_calibration_start",
        "maps_calibration_cancel",
        "maps_calibration_status",
        "maps_scan_coordinates",
        "navigation_snapshot",
        "navigation_start",
        "navigation_update_position",
        "navigation_mark_visited",
        "navigation_skip",
        "navigation_resolve_harvest",
        "navigation_undo",
        "navigation_end",
        "radar_calibration_start",
        "radar_calibration_cancel",
        "radar_calibration_status",
        "radar_geometry",
    ];

    #[test]
    fn the_registered_commands_match_the_manifest() {
        let manifest: Vec<&str> = eo_api::manifest::manifest()
            .iter()
            .map(|spec| spec.name)
            .collect();
        assert_eq!(TYPED_COMMANDS, manifest.as_slice());
    }

    #[test]
    fn the_application_acl_covers_exactly_the_registered_command_surface() {
        let mut expected = TYPED_COMMANDS.to_vec();
        expected.extend([
            "toggle_overlay",
            "toggle_cartography_overlay",
            "show_scan_overlay",
            "show_sale_capture_overlay",
            "capture_sale_from_overlay",
            "hide_sale_capture_overlay",
            "hide_scan_overlay",
            "show_navigation_overlays",
            "hide_navigation_overlays",
            "begin_navigation_area_selection",
            "capture_png",
            "planet_map_image",
            "check_for_update",
            "download_update",
            "install_update",
            "get_update_channel",
            "set_update_channel",
            "substrate_ready",
            "restart_app",
        ]);
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(crate::command_acl::APP_COMMANDS, expected.as_slice());
    }
}
