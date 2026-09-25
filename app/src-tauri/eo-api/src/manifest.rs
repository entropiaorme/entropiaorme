//! The command manifest: the single machine-readable description of the
//! typed IPC surface, walked by `cargo xtask gen-ts` to emit the
//! TypeScript bindings.
//!
//! Every entry names one Tauri command (declared with
//! `rename_all = "snake_case"`, so the invoke argument keys are exactly
//! the names recorded here) together with the JSON Schemas of its
//! arguments and return type, read straight off the DTOs' serde
//! attributes. The shell asserts its registered command set against
//! this manifest, so a command cannot ship unbound and a binding cannot
//! outlive its command.

use schemars::schema_for;
use serde_json::Value;

use crate::activities::{ActivityOptionsResult, ActivityStateResult, ActivityTargetKind};
use crate::analytics::{
    ActivityHistoryEntry, ActivityUndoInput, AnalyticsHarvest, AnalyticsHunting,
    AnalyticsHuntingActivity, AnalyticsOverview, AuctionConfirmInput, AuctionExpireInput,
    AuctionListing, AuctionListingInput, EquipmentListingInput, EquipmentTradeInput,
    HuntingRealisedMarkup, InventoryDraftResolution, InventoryItem, InventoryItemInput,
    InventoryPatch, InventorySaleDraft, InventorySellInput, InventorySellResult, LedgerEntryInput,
    LedgerItem, LedgerPage, LedgerPreset, LedgerPresetInput, LedgerSummary, PrivateSaleInput,
    Profession, RealisedTierMarkup, SaleWindowCapture, ShrapnelConversionInput,
    StockConversionInput, StockPosition, StockRemovalInput,
};
use crate::character::{
    ActivityRecommenderQuery, ActivityRecommenderResult, CalibrationStatus,
    CharacterProspectOptions, ComputedCharacterStats, HpOptimizerResult, PathOptimizerResult,
    ProfessionLevel, ProfessionOptimizerResult, ProspectQuery, ProspectResult, SkillLevel,
};
use crate::codex::{
    CodexCalibrateResult, CodexClaimResult, CodexMasteryClaimResult, CodexMetaAttribute,
    CodexMetaClaimResult, CodexRecommendTarget, CodexSkillOption, CodexSpecies, CodexSpeciesRanks,
};
use crate::dev::{
    AuctionFeeOverlayStatus, AuctionFeeResearchStatus, CompactResult, CrashReportingStatus,
    MetricsSnapshot, RebuildReport,
};
use crate::equipment::{
    EquipmentDetail, EquipmentRequest, EquipmentSearchHit, EquipmentSummary, SearchKind,
};
use crate::healing::{
    HealingCorrectionTarget, HealingCorrectionTool, HealingOutputClassification, HealingOutputPage,
};
use crate::maps::{
    CoordCalibrationStatus, CoordScanResult, MapPin, MapPinInput, MapPinPatch, MapView,
    NavigationPositionResult, NavigationRun, NearbyMapPin, PinConfig, PinConfigEditInput,
    PinConfigInput, PlanetMap, RadarCalibrationStatus, RadarGeometry,
};
use crate::market::{
    MarketAuctionPacketThreshold, MarketBreakEven, MarketCommitResult, MarketContributionBatch,
    MarketHarvestData, MarketHistoryPoint, MarketHorizon, MarketMobRankingRow, MarketOverviewRow,
    MarketPastePreview, MarketUnitPriceResult,
};
use crate::protection::{
    ProtectionObservationInput, ProtectionObservationOutcome, ProtectionOverview,
    ProtectionRecordingCandidates, ProtectionRepairInput, ProtectionRepairOutcome,
    ProtectionScanResult, ProtectionSessionStatus, ProtectionSetInput, ProtectionSetUpdateInput,
    ProtectionStream, ProtectionUndoTarget,
};
use crate::quests::{
    Quest, QuestAnalyticsRow, QuestFamily, QuestFamilyInput, QuestHandInState, QuestInput,
    QuestRewardReviewInput, UnresolvedQuestReward,
};
use crate::scan::{
    AcceptResult, CaptureResult, RejectResult, ScanStatus, SkillScanPending, SpacebarResult,
    UndoResult,
};
use crate::session_definitions::{SessionDefinition, SessionDefinitionInput};
use crate::settings::{AppSettings, OverlayPosition, SettingsPatch};
use crate::tracking::{
    ArmourCostResult, DefinitionSelectResult, LootItemEditResult, ManualMobLockResult,
    ManualMobSuggestion, MobEditResult, ReleaseResult, RepairScanResult, SessionConfigResult,
    SessionDetail, SessionIntervals, SessionPage, SessionReassignResult, StartResult, StopResult,
    TrackingSnapshot,
};
use crate::weapons::{
    WeaponCorrectionWeapon, WeaponMismatchDecision, WeaponShotGroup, WeaponShotPage,
};
use crate::ApiError;
use crate::Nullable;

/// One argument of a typed command.
pub struct ArgSpec {
    pub name: &'static str,
    pub schema: Value,
}

/// One typed command: its invoke name, arguments, and return schema
/// (`None` for a void return). Schemas are plain JSON values so the
/// generator needs no schema-crate dependency of its own.
pub struct CommandSpec {
    pub name: &'static str,
    pub args: Vec<ArgSpec>,
    pub returns: Option<Value>,
}

/// The full typed command surface, in emission order.
pub fn manifest() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "equipment_search",
            args: vec![
                ArgSpec {
                    name: "q",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "kind",
                    schema: schema(schema_for!(SearchKind)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<EquipmentSearchHit>))),
        },
        CommandSpec {
            name: "equipment_library",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<EquipmentSummary>))),
        },
        CommandSpec {
            name: "equipment_add",
            args: vec![ArgSpec {
                name: "req",
                schema: schema(schema_for!(EquipmentRequest)),
            }],
            returns: Some(schema(schema_for!(EquipmentSummary))),
        },
        CommandSpec {
            name: "equipment_update",
            args: vec![
                ArgSpec {
                    name: "item_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "req",
                    schema: schema(schema_for!(EquipmentRequest)),
                },
            ],
            returns: Some(schema(schema_for!(EquipmentSummary))),
        },
        CommandSpec {
            name: "equipment_delete",
            args: vec![ArgSpec {
                name: "item_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "equipment_detail",
            args: vec![ArgSpec {
                name: "item_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(EquipmentDetail))),
        },
        CommandSpec {
            name: "protection_overview",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_set_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ProtectionSetInput)),
            }],
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_set_update",
            args: vec![
                ArgSpec {
                    name: "set_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "input",
                    schema: schema(schema_for!(ProtectionSetUpdateInput)),
                },
            ],
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_set_archive",
            args: vec![ArgSpec {
                name: "set_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_set_restore",
            args: vec![ArgSpec {
                name: "set_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_undo",
            args: vec![ArgSpec {
                name: "target",
                schema: schema(schema_for!(ProtectionUndoTarget)),
            }],
            returns: Some(schema(schema_for!(ProtectionOverview))),
        },
        CommandSpec {
            name: "protection_unrecorded_sessions",
            args: vec![ArgSpec {
                name: "session_ids",
                schema: schema(schema_for!(Vec<String>)),
            }],
            returns: Some(schema(schema_for!(Vec<String>))),
        },
        CommandSpec {
            name: "protection_session_status",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(ProtectionSessionStatus))),
        },
        CommandSpec {
            name: "protection_recording_candidates",
            args: vec![ArgSpec {
                name: "stream",
                schema: schema(schema_for!(ProtectionStream)),
            }],
            returns: Some(schema(schema_for!(ProtectionRecordingCandidates))),
        },
        CommandSpec {
            name: "protection_observation_confirm",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ProtectionObservationInput)),
            }],
            returns: Some(schema(schema_for!(ProtectionObservationOutcome))),
        },
        CommandSpec {
            name: "protection_repair_confirm",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ProtectionRepairInput)),
            }],
            returns: Some(schema(schema_for!(ProtectionRepairOutcome))),
        },
        CommandSpec {
            name: "protection_trade_terminal_scan",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ProtectionScanResult))),
        },
        CommandSpec {
            name: "healing_outputs",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "classification",
                    schema: schema(schema_for!(HealingOutputClassification)),
                },
                ArgSpec {
                    name: "offset",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(HealingOutputPage))),
        },
        CommandSpec {
            name: "healing_correction_tools",
            args: vec![ArgSpec {
                name: "output_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(Vec<HealingCorrectionTool>))),
        },
        CommandSpec {
            name: "healing_correct",
            args: vec![ArgSpec {
                name: "target",
                schema: schema(schema_for!(HealingCorrectionTarget)),
            }],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "healing_correction_undo",
            args: vec![ArgSpec {
                name: "correction_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "weapon_shots",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "group",
                    schema: schema(schema_for!(WeaponShotGroup)),
                },
                ArgSpec {
                    name: "offset",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(WeaponShotPage))),
        },
        CommandSpec {
            name: "weapon_unpriced_sessions",
            args: vec![ArgSpec {
                name: "session_ids",
                schema: schema(schema_for!(Vec<String>)),
            }],
            returns: Some(schema(schema_for!(Vec<String>))),
        },
        CommandSpec {
            name: "weapon_correction_weapons",
            args: vec![ArgSpec {
                name: "evidence_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(Vec<WeaponCorrectionWeapon>))),
        },
        CommandSpec {
            name: "weapon_assign",
            args: vec![
                ArgSpec {
                    name: "evidence_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "equipment_id",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "weapon_assignment_undo",
            args: vec![ArgSpec {
                name: "correction_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "character_calibration",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CalibrationStatus))),
        },
        CommandSpec {
            name: "character_stats",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ComputedCharacterStats))),
        },
        CommandSpec {
            name: "character_skills",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<SkillLevel>))),
        },
        CommandSpec {
            name: "character_professions",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<ProfessionLevel>))),
        },
        CommandSpec {
            name: "character_prospect_options",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CharacterProspectOptions))),
        },
        CommandSpec {
            name: "character_prospect",
            args: vec![ArgSpec {
                name: "query",
                schema: schema(schema_for!(ProspectQuery)),
            }],
            returns: Some(schema(schema_for!(ProspectResult))),
        },
        CommandSpec {
            name: "character_profession_optimizer",
            args: vec![ArgSpec {
                name: "profession",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(ProfessionOptimizerResult))),
        },
        CommandSpec {
            name: "character_path_optimizer",
            args: vec![
                ArgSpec {
                    name: "profession",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "target_level",
                    schema: schema(schema_for!(Option<f64>)),
                },
                ArgSpec {
                    name: "ped_budget",
                    schema: schema(schema_for!(Option<f64>)),
                },
            ],
            returns: Some(schema(schema_for!(PathOptimizerResult))),
        },
        CommandSpec {
            name: "character_hp_optimizer",
            args: Vec::new(),
            returns: Some(schema(schema_for!(HpOptimizerResult))),
        },
        CommandSpec {
            name: "character_activity_recommender",
            args: vec![ArgSpec {
                name: "query",
                schema: schema(schema_for!(ActivityRecommenderQuery)),
            }],
            returns: Some(schema(schema_for!(ActivityRecommenderResult))),
        },
        CommandSpec {
            name: "settings_get",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AppSettings))),
        },
        CommandSpec {
            name: "settings_overlay_position",
            args: Vec::new(),
            returns: Some(schema(schema_for!(OverlayPosition))),
        },
        CommandSpec {
            name: "settings_set_overlay_position",
            args: vec![
                ArgSpec {
                    name: "x",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "y",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: None,
        },
        CommandSpec {
            name: "settings_update",
            args: vec![ArgSpec {
                name: "patch",
                schema: schema(schema_for!(SettingsPatch)),
            }],
            returns: Some(schema(schema_for!(AppSettings))),
        },
        CommandSpec {
            name: "codex_species",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<CodexSpecies>))),
        },
        CommandSpec {
            name: "codex_species_ranks",
            args: vec![ArgSpec {
                name: "species_name",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(CodexSpeciesRanks))),
        },
        CommandSpec {
            name: "codex_recommend",
            args: vec![
                ArgSpec {
                    name: "species_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "rank",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "professions",
                    schema: schema(schema_for!(Vec<String>)),
                },
                ArgSpec {
                    name: "target",
                    schema: schema(schema_for!(CodexRecommendTarget)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<CodexSkillOption>))),
        },
        CommandSpec {
            name: "codex_meta_attributes",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<CodexMetaAttribute>))),
        },
        CommandSpec {
            name: "codex_calibrate",
            args: vec![
                ArgSpec {
                    name: "species_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "rank",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(CodexCalibrateResult))),
        },
        CommandSpec {
            name: "codex_claim",
            args: vec![
                ArgSpec {
                    name: "species_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "rank",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "skill_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(CodexClaimResult))),
        },
        CommandSpec {
            name: "codex_unclaim",
            args: vec![ArgSpec {
                name: "species_name",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(CodexClaimResult))),
        },
        CommandSpec {
            name: "codex_meta_claim",
            args: vec![ArgSpec {
                name: "attribute_name",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(CodexMetaClaimResult))),
        },
        CommandSpec {
            name: "codex_mastery_options",
            args: vec![
                ArgSpec {
                    name: "professions",
                    schema: schema(schema_for!(Vec<String>)),
                },
                ArgSpec {
                    name: "target",
                    schema: schema(schema_for!(CodexRecommendTarget)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<CodexSkillOption>))),
        },
        CommandSpec {
            name: "codex_mastery_claim",
            args: vec![
                ArgSpec {
                    name: "species_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "skill_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(CodexMasteryClaimResult))),
        },
        CommandSpec {
            name: "codex_mastery_unclaim",
            args: vec![ArgSpec {
                name: "species_name",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(CodexMasteryClaimResult))),
        },
        CommandSpec {
            name: "quests_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<Quest>))),
        },
        CommandSpec {
            name: "quest_get",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quest_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(QuestInput)),
            }],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quest_update",
            args: vec![
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "input",
                    schema: schema(schema_for!(QuestInput)),
                },
            ],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quest_delete",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "quest_start",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quest_complete",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quest_hand_in_begin",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(QuestHandInState))),
        },
        CommandSpec {
            name: "quest_hand_in_state",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(QuestHandInState))),
        },
        CommandSpec {
            name: "quest_hand_in_wait",
            args: vec![
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "after_clump_id",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(QuestHandInState))),
        },
        CommandSpec {
            name: "quest_hand_in_cancel",
            args: vec![ArgSpec {
                name: "quest_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "quest_hand_in_confirm",
            args: vec![
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "clump_id",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: None,
        },
        CommandSpec {
            name: "quest_rewards_unresolved",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<UnresolvedQuestReward>))),
        },
        CommandSpec {
            name: "quest_reward_review",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(QuestRewardReviewInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "quest_cancel",
            args: vec![
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "undo_reward",
                    schema: schema(schema_for!(bool)),
                },
            ],
            returns: Some(schema(schema_for!(Quest))),
        },
        CommandSpec {
            name: "quests_mobs",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<String>))),
        },
        CommandSpec {
            name: "quests_analytics",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<QuestAnalyticsRow>))),
        },
        CommandSpec {
            name: "quest_families_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<QuestFamily>))),
        },
        CommandSpec {
            name: "quest_family_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(QuestFamilyInput)),
            }],
            returns: Some(schema(schema_for!(QuestFamily))),
        },
        CommandSpec {
            name: "quest_family_update",
            args: vec![
                ArgSpec {
                    name: "family_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "input",
                    schema: schema(schema_for!(QuestFamilyInput)),
                },
            ],
            returns: Some(schema(schema_for!(QuestFamily))),
        },
        CommandSpec {
            name: "quest_family_delete",
            args: vec![ArgSpec {
                name: "family_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "session_definitions_list",
            args: vec![ArgSpec {
                name: "include_inactive",
                schema: schema(schema_for!(Option<bool>)),
            }],
            returns: Some(schema(schema_for!(Vec<SessionDefinition>))),
        },
        CommandSpec {
            name: "session_definition_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(SessionDefinitionInput)),
            }],
            returns: Some(schema(schema_for!(SessionDefinition))),
        },
        CommandSpec {
            name: "session_definition_update",
            args: vec![
                ArgSpec {
                    name: "definition_id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "input",
                    schema: schema(schema_for!(SessionDefinitionInput)),
                },
            ],
            returns: Some(schema(schema_for!(SessionDefinition))),
        },
        CommandSpec {
            name: "session_definition_archive",
            args: vec![ArgSpec {
                name: "definition_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(SessionDefinition))),
        },
        CommandSpec {
            name: "session_definition_restore",
            args: vec![ArgSpec {
                name: "definition_id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(SessionDefinition))),
        },
        CommandSpec {
            name: "tracking_definition_select",
            args: vec![ArgSpec {
                name: "definition_id",
                schema: schema(schema_for!(Option<i64>)),
            }],
            returns: Some(schema(schema_for!(DefinitionSelectResult))),
        },
        CommandSpec {
            name: "analytics_overview",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsOverview))),
        },
        CommandSpec {
            name: "analytics_hunting",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AnalyticsHunting))),
        },
        CommandSpec {
            name: "analytics_harvest",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsHarvest))),
        },
        CommandSpec {
            name: "analytics_hunting_activity",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsHuntingActivity))),
        },
        CommandSpec {
            name: "activity_stock",
            args: vec![ArgSpec {
                name: "profession",
                schema: schema(schema_for!(Profession)),
            }],
            returns: Some(schema(schema_for!(Vec<StockPosition>))),
        },
        CommandSpec {
            name: "harvest_realised_markup",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<RealisedTierMarkup>))),
        },
        CommandSpec {
            name: "hunting_realised_markup",
            args: Vec::new(),
            returns: Some(schema(schema_for!(HuntingRealisedMarkup))),
        },
        CommandSpec {
            name: "auction_listings",
            args: vec![ArgSpec {
                name: "profession",
                schema: schema(schema_for!(Profession)),
            }],
            returns: Some(schema(schema_for!(Vec<AuctionListing>))),
        },
        CommandSpec {
            name: "auction_listing_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(AuctionListingInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "auction_listing_confirm",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(AuctionConfirmInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "auction_listing_expire",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(AuctionExpireInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "stock_convert",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(StockConversionInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "stock_private_sale",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(PrivateSaleInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "stock_remove",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(StockRemovalInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "stock_shrapnel_convert",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ShrapnelConversionInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "activity_history",
            args: vec![ArgSpec {
                name: "profession",
                schema: schema(schema_for!(Profession)),
            }],
            returns: Some(schema(schema_for!(Vec<ActivityHistoryEntry>))),
        },
        CommandSpec {
            name: "auction_sale_revert",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ActivityUndoInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "auction_listing_undo",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ActivityUndoInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "stock_conversion_undo",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ActivityUndoInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "private_sale_undo",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ActivityUndoInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "stock_removal_undo",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(ActivityUndoInput)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "ledger_list",
            args: vec![
                ArgSpec {
                    name: "cursor",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(LedgerPage))),
        },
        CommandSpec {
            name: "ledger_summary",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(LedgerSummary))),
        },
        CommandSpec {
            name: "ledger_create",
            args: vec![ArgSpec {
                name: "entry",
                schema: schema(schema_for!(LedgerEntryInput)),
            }],
            returns: Some(schema(schema_for!(LedgerItem))),
        },
        CommandSpec {
            name: "ledger_delete",
            args: vec![ArgSpec {
                name: "entry_id",
                schema: schema(schema_for!(String)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "ledger_presets_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<LedgerPreset>))),
        },
        CommandSpec {
            name: "ledger_preset_create",
            args: vec![ArgSpec {
                name: "preset",
                schema: schema(schema_for!(LedgerPresetInput)),
            }],
            returns: Some(schema(schema_for!(LedgerPreset))),
        },
        CommandSpec {
            name: "ledger_preset_delete",
            args: vec![ArgSpec {
                name: "preset_id",
                schema: schema(schema_for!(String)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "inventory_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<InventoryItem>))),
        },
        CommandSpec {
            name: "inventory_create",
            args: vec![ArgSpec {
                name: "item",
                schema: schema(schema_for!(InventoryItemInput)),
            }],
            returns: Some(schema(schema_for!(InventoryItem))),
        },
        CommandSpec {
            name: "inventory_update",
            args: vec![
                ArgSpec {
                    name: "item_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "patch",
                    schema: schema(schema_for!(InventoryPatch)),
                },
            ],
            returns: Some(schema(schema_for!(InventoryItem))),
        },
        CommandSpec {
            name: "inventory_delete",
            args: vec![ArgSpec {
                name: "item_id",
                schema: schema(schema_for!(String)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "inventory_sell",
            args: vec![
                ArgSpec {
                    name: "item_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "sale",
                    schema: schema(schema_for!(InventorySellInput)),
                },
            ],
            returns: Some(schema(schema_for!(InventorySellResult))),
        },
        CommandSpec {
            name: "inventory_sale_window_capture",
            args: vec![],
            returns: Some(schema(schema_for!(SaleWindowCapture))),
        },
        CommandSpec {
            name: "inventory_sale_window_take_capture",
            args: vec![],
            returns: Some(schema(schema_for!(Nullable<SaleWindowCapture>))),
        },
        CommandSpec {
            name: "inventory_draft_resolve",
            args: vec![ArgSpec {
                name: "draft",
                schema: schema(schema_for!(InventorySaleDraft)),
            }],
            returns: Some(schema(schema_for!(InventoryDraftResolution))),
        },
        CommandSpec {
            name: "inventory_equipment_listing_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(EquipmentListingInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "inventory_equipment_trade",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(EquipmentTradeInput)),
            }],
            returns: Some(schema(schema_for!(AuctionListing))),
        },
        CommandSpec {
            name: "market_paste_preview",
            args: vec![ArgSpec {
                name: "text",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(MarketPastePreview))),
        },
        CommandSpec {
            name: "market_paste_commit",
            args: vec![ArgSpec {
                name: "text",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(MarketCommitResult))),
        },
        CommandSpec {
            name: "market_unit_price_set",
            args: vec![
                ArgSpec {
                    name: "item_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "ped_per_unit",
                    schema: schema(schema_for!(f64)),
                },
            ],
            returns: Some(schema(schema_for!(MarketUnitPriceResult))),
        },
        CommandSpec {
            name: "market_overview",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<MarketOverviewRow>))),
        },
        CommandSpec {
            name: "market_contribution_batch",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Nullable<MarketContributionBatch>))),
        },
        CommandSpec {
            name: "market_auction_packet_threshold",
            args: vec![ArgSpec {
                name: "max_fee_share_pct",
                schema: schema(schema_for!(f64)),
            }],
            returns: Some(schema(schema_for!(MarketAuctionPacketThreshold))),
        },
        CommandSpec {
            name: "market_break_even",
            args: Vec::new(),
            returns: Some(schema(schema_for!(MarketBreakEven))),
        },
        CommandSpec {
            name: "market_mob_ranking",
            args: vec![ArgSpec {
                name: "horizon",
                schema: schema(schema_for!(MarketHorizon)),
            }],
            returns: Some(schema(schema_for!(Vec<MarketMobRankingRow>))),
        },
        CommandSpec {
            name: "market_harvest_markups",
            args: vec![],
            returns: Some(schema(schema_for!(MarketHarvestData))),
        },
        CommandSpec {
            name: "market_hunt_markups",
            args: vec![],
            returns: Some(schema(schema_for!(MarketHarvestData))),
        },
        CommandSpec {
            name: "market_item_history",
            args: vec![
                ArgSpec {
                    name: "item_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "horizon",
                    schema: schema(schema_for!(MarketHorizon)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<MarketHistoryPoint>))),
        },
        CommandSpec {
            name: "scan_status",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ScanStatus))),
        },
        CommandSpec {
            name: "scan_start",
            args: vec![ArgSpec {
                name: "page_count",
                schema: schema(schema_for!(Option<i64>)),
            }],
            returns: Some(schema(schema_for!(ScanStatus))),
        },
        CommandSpec {
            name: "scan_capture",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CaptureResult))),
        },
        CommandSpec {
            name: "scan_cancel",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ScanStatus))),
        },
        CommandSpec {
            name: "scan_undo",
            args: Vec::new(),
            returns: Some(schema(schema_for!(UndoResult))),
        },
        CommandSpec {
            name: "scan_process",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ScanStatus))),
        },
        CommandSpec {
            name: "scan_accept",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AcceptResult))),
        },
        CommandSpec {
            name: "scan_reject",
            args: Vec::new(),
            returns: Some(schema(schema_for!(RejectResult))),
        },
        CommandSpec {
            name: "scan_pending",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Option<SkillScanPending>))),
        },
        CommandSpec {
            name: "scan_spacebar_capture",
            args: vec![ArgSpec {
                name: "enabled",
                schema: schema(schema_for!(bool)),
            }],
            returns: Some(schema(schema_for!(SpacebarResult))),
        },
        CommandSpec {
            name: "tracking_sessions",
            args: vec![
                ArgSpec {
                    name: "cursor",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "definition_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(SessionPage))),
        },
        CommandSpec {
            name: "tracking_session_detail",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "tracking_session_intervals",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(SessionIntervals))),
        },
        CommandSpec {
            name: "tracking_manual_mob_suggestions",
            args: vec![
                ArgSpec {
                    name: "q",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<ManualMobSuggestion>))),
        },
        CommandSpec {
            name: "tracking_snapshot",
            args: Vec::new(),
            returns: Some(schema(schema_for!(TrackingSnapshot))),
        },
        CommandSpec {
            name: "tracking_start",
            args: Vec::new(),
            returns: Some(schema(schema_for!(StartResult))),
        },
        CommandSpec {
            name: "tracking_stop",
            args: Vec::new(),
            returns: Some(schema(schema_for!(StopResult))),
        },
        CommandSpec {
            name: "tracking_weapon_decide",
            args: vec![ArgSpec {
                name: "decision",
                schema: schema(schema_for!(WeaponMismatchDecision)),
            }],
            returns: Some(schema(schema_for!(bool))),
        },
        CommandSpec {
            name: "tracking_release_mob",
            args: Vec::new(),
            returns: Some(schema(schema_for!(ReleaseResult))),
        },
        CommandSpec {
            name: "tracking_manual_mob_lock",
            args: vec![
                ArgSpec {
                    name: "species",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "maturity",
                    schema: schema(schema_for!(Option<String>)),
                },
            ],
            returns: Some(schema(schema_for!(ManualMobLockResult))),
        },
        CommandSpec {
            name: "tracking_session_config",
            args: vec![
                ArgSpec {
                    name: "session_name",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "skill_boost_percent",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(SessionConfigResult))),
        },
        CommandSpec {
            name: "tracking_activity_options",
            args: vec![],
            returns: Some(schema(schema_for!(ActivityOptionsResult))),
        },
        CommandSpec {
            name: "tracking_activity_activate",
            args: vec![
                ArgSpec {
                    name: "kind",
                    schema: schema(schema_for!(ActivityTargetKind)),
                },
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "label",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "additive",
                    schema: schema(schema_for!(Option<bool>)),
                },
            ],
            returns: Some(schema(schema_for!(ActivityStateResult))),
        },
        CommandSpec {
            name: "tracking_activity_deactivate",
            args: vec![
                ArgSpec {
                    name: "kind",
                    schema: schema(schema_for!(ActivityTargetKind)),
                },
                ArgSpec {
                    name: "quest_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "label",
                    schema: schema(schema_for!(Option<String>)),
                },
            ],
            returns: Some(schema(schema_for!(ActivityStateResult))),
        },
        CommandSpec {
            name: "tracking_reassign_session",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "definition_id",
                    schema: schema(schema_for!(i64)),
                },
            ],
            returns: Some(schema(schema_for!(SessionReassignResult))),
        },
        CommandSpec {
            name: "tracking_rename_mob",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "from_mob_name",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "to_mob_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(MobEditResult))),
        },
        CommandSpec {
            name: "tracking_restore_mob",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "current_mob_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(MobEditResult))),
        },
        CommandSpec {
            name: "tracking_loot_item_activate",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "item_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(LootItemEditResult))),
        },
        CommandSpec {
            name: "tracking_loot_item_deactivate",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "item_name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(LootItemEditResult))),
        },
        CommandSpec {
            name: "tracking_armour_cost",
            args: vec![
                ArgSpec {
                    name: "session_id",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "cost",
                    schema: schema(schema_for!(f64)),
                },
            ],
            returns: Some(schema(schema_for!(ArmourCostResult))),
        },
        CommandSpec {
            name: "tracking_repair_scan",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(RepairScanResult))),
        },
        CommandSpec {
            name: "tracking_session_delete",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: None,
        },
        // The guide-mode demo reads: typed commands sharing the live
        // analytics and tracking DTOs, served over the parallel demo state.
        CommandSpec {
            name: "demo_analytics_overview",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsOverview))),
        },
        CommandSpec {
            name: "demo_analytics_hunting",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AnalyticsHunting))),
        },
        CommandSpec {
            name: "demo_analytics_hunting_activity",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsHuntingActivity))),
        },
        CommandSpec {
            name: "demo_analytics_harvest",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(AnalyticsHarvest))),
        },
        CommandSpec {
            name: "demo_ledger_list",
            args: vec![
                ArgSpec {
                    name: "cursor",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(LedgerPage))),
        },
        CommandSpec {
            name: "demo_ledger_summary",
            args: vec![ArgSpec {
                name: "period",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(LedgerSummary))),
        },
        CommandSpec {
            name: "demo_ledger_presets_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<LedgerPreset>))),
        },
        CommandSpec {
            name: "demo_inventory_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<InventoryItem>))),
        },
        CommandSpec {
            name: "demo_tracking_sessions",
            args: vec![
                ArgSpec {
                    name: "cursor",
                    schema: schema(schema_for!(Option<String>)),
                },
                ArgSpec {
                    name: "limit",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "definition_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(SessionPage))),
        },
        CommandSpec {
            name: "demo_tracking_session_detail",
            args: vec![ArgSpec {
                name: "session_id",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(SessionDetail))),
        },
        CommandSpec {
            name: "demo_tracking_snapshot",
            args: Vec::new(),
            returns: Some(schema(schema_for!(TrackingSnapshot))),
        },
        // The hidden developer-tools family: native-only, each gated on
        // developer mode (a gate-off command answers the not-found the HTTP
        // route's 404 stood for).
        CommandSpec {
            name: "dev_metrics",
            args: Vec::new(),
            returns: Some(schema(schema_for!(MetricsSnapshot))),
        },
        CommandSpec {
            name: "dev_crash_reporting",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CrashReportingStatus))),
        },
        CommandSpec {
            name: "dev_set_crash_reporting",
            args: vec![ArgSpec {
                name: "enabled",
                schema: schema(schema_for!(bool)),
            }],
            returns: Some(schema(schema_for!(CrashReportingStatus))),
        },
        CommandSpec {
            name: "dev_compact_database",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CompactResult))),
        },
        CommandSpec {
            name: "dev_rebuild_projections",
            args: Vec::new(),
            returns: Some(schema(schema_for!(RebuildReport))),
        },
        CommandSpec {
            name: "dev_auction_fee_research_start",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AuctionFeeResearchStatus))),
        },
        CommandSpec {
            name: "dev_auction_fee_research_stop",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AuctionFeeResearchStatus))),
        },
        CommandSpec {
            name: "dev_auction_fee_research_status",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AuctionFeeResearchStatus))),
        },
        CommandSpec {
            name: "dev_auction_fee_research_capture",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AuctionFeeOverlayStatus))),
        },
        CommandSpec {
            name: "dev_auction_fee_research_overlay_status",
            args: Vec::new(),
            returns: Some(schema(schema_for!(AuctionFeeOverlayStatus))),
        },
        // The planet-maps family: the catalogue read only. The raster
        // fetch answers raw bytes and rides a bespoke shell command
        // outside the manifest, like the manual-scan capture preview.
        CommandSpec {
            name: "planet_maps_list",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Vec<PlanetMap>))),
        },
        CommandSpec {
            name: "map_pins_list",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "map_view_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<MapPin>))),
        },
        CommandSpec {
            name: "map_pins_viewport",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "map_view_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "lon_min",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "lon_max",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "lat_min",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "lat_max",
                    schema: schema(schema_for!(f64)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<MapPin>))),
        },
        CommandSpec {
            name: "map_pin_nearby",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "map_view_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "lon",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "lat",
                    schema: schema(schema_for!(f64)),
                },
            ],
            returns: Some(schema(schema_for!(Nullable<NearbyMapPin>))),
        },
        CommandSpec {
            name: "map_views_list",
            args: vec![ArgSpec {
                name: "planet",
                schema: schema(schema_for!(String)),
            }],
            returns: Some(schema(schema_for!(Vec<MapView>))),
        },
        CommandSpec {
            name: "map_view_create",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(MapView))),
        },
        CommandSpec {
            name: "map_view_rename",
            args: vec![
                ArgSpec {
                    name: "id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "name",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(MapView))),
        },
        CommandSpec {
            name: "map_view_delete",
            args: vec![ArgSpec {
                name: "id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "map_pin_create",
            args: vec![ArgSpec {
                name: "pin",
                schema: schema(schema_for!(MapPinInput)),
            }],
            returns: Some(schema(schema_for!(MapPin))),
        },
        CommandSpec {
            name: "map_pin_update",
            args: vec![
                ArgSpec {
                    name: "id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "patch",
                    schema: schema(schema_for!(MapPinPatch)),
                },
            ],
            returns: Some(schema(schema_for!(MapPin))),
        },
        CommandSpec {
            name: "map_pin_delete",
            args: vec![ArgSpec {
                name: "id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "map_pin_cooldown",
            args: vec![ArgSpec {
                name: "id",
                schema: schema(schema_for!(i64)),
            }],
            returns: Some(schema(schema_for!(MapPin))),
        },
        CommandSpec {
            name: "pin_configs_list",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "map_view_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
            ],
            returns: Some(schema(schema_for!(Vec<PinConfig>))),
        },
        CommandSpec {
            name: "pin_config_create",
            args: vec![ArgSpec {
                name: "input",
                schema: schema(schema_for!(PinConfigInput)),
            }],
            returns: Some(schema(schema_for!(PinConfig))),
        },
        CommandSpec {
            name: "pin_config_update",
            args: vec![
                ArgSpec {
                    name: "id",
                    schema: schema(schema_for!(i64)),
                },
                ArgSpec {
                    name: "input",
                    schema: schema(schema_for!(PinConfigEditInput)),
                },
            ],
            returns: Some(schema(schema_for!(PinConfig))),
        },
        CommandSpec {
            name: "pin_config_delete",
            args: vec![ArgSpec {
                name: "id",
                schema: schema(schema_for!(i64)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "pin_config_reorder",
            args: vec![ArgSpec {
                name: "ids",
                schema: schema(schema_for!(Vec<i64>)),
            }],
            returns: None,
        },
        CommandSpec {
            name: "maps_calibration_start",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CoordCalibrationStatus))),
        },
        CommandSpec {
            name: "maps_calibration_cancel",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CoordCalibrationStatus))),
        },
        CommandSpec {
            name: "maps_calibration_status",
            args: Vec::new(),
            returns: Some(schema(schema_for!(CoordCalibrationStatus))),
        },
        CommandSpec {
            name: "maps_scan_coordinates",
            args: vec![ArgSpec {
                name: "planet",
                schema: schema(schema_for!(Option<String>)),
            }],
            returns: Some(schema(schema_for!(CoordScanResult))),
        },
        CommandSpec {
            name: "navigation_snapshot",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Nullable<NavigationRun>))),
        },
        CommandSpec {
            name: "navigation_start",
            args: vec![
                ArgSpec {
                    name: "planet",
                    schema: schema(schema_for!(String)),
                },
                ArgSpec {
                    name: "map_view_id",
                    schema: schema(schema_for!(Option<i64>)),
                },
                ArgSpec {
                    name: "start_lon",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "start_lat",
                    schema: schema(schema_for!(f64)),
                },
                ArgSpec {
                    name: "selected_pin_ids",
                    schema: schema(schema_for!(Option<Vec<i64>>)),
                },
                ArgSpec {
                    name: "hotkey",
                    schema: schema(schema_for!(String)),
                },
            ],
            returns: Some(schema(schema_for!(NavigationRun))),
        },
        CommandSpec {
            name: "navigation_update_position",
            args: Vec::new(),
            returns: Some(schema(schema_for!(NavigationPositionResult))),
        },
        CommandSpec {
            name: "navigation_mark_visited",
            args: vec![ArgSpec {
                name: "force",
                schema: schema(schema_for!(bool)),
            }],
            returns: Some(schema(schema_for!(NavigationPositionResult))),
        },
        CommandSpec {
            name: "navigation_skip",
            args: Vec::new(),
            returns: Some(schema(schema_for!(NavigationRun))),
        },
        CommandSpec {
            name: "navigation_resolve_harvest",
            args: vec![ArgSpec {
                name: "confirm",
                schema: schema(schema_for!(bool)),
            }],
            returns: Some(schema(schema_for!(NavigationRun))),
        },
        CommandSpec {
            name: "navigation_undo",
            args: Vec::new(),
            returns: Some(schema(schema_for!(NavigationRun))),
        },
        CommandSpec {
            name: "navigation_end",
            args: Vec::new(),
            returns: None,
        },
        CommandSpec {
            name: "radar_calibration_start",
            args: Vec::new(),
            returns: Some(schema(schema_for!(RadarCalibrationStatus))),
        },
        CommandSpec {
            name: "radar_calibration_cancel",
            args: Vec::new(),
            returns: None,
        },
        CommandSpec {
            name: "radar_calibration_status",
            args: Vec::new(),
            returns: Some(schema(schema_for!(RadarCalibrationStatus))),
        },
        CommandSpec {
            name: "radar_geometry",
            args: Vec::new(),
            returns: Some(schema(schema_for!(Nullable<RadarGeometry>))),
        },
    ]
}

/// The IPC error contract's schema, emitted alongside the commands.
pub fn error_schema() -> Value {
    schema(schema_for!(ApiError))
}

/// A derived schema as its plain JSON value.
fn schema(schema: schemars::Schema) -> Value {
    serde_json::to_value(schema).expect("a derived schema serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_are_unique_and_snake_case() {
        let specs = manifest();
        let mut names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        names.sort_unstable();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "duplicate command name");
        for name in names {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name} is not snake_case"
            );
        }
    }
}
