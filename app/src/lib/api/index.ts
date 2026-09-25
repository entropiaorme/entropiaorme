/**
 * Backend API surface: the typed wrappers the app calls the backend
 * through, split per domain family.
 *
 * All backend communication goes through this module. Each wrapper
 * delegates to a generated typed command (`commands.gen.ts`, emitted
 * from the Rust command DTOs), which types the arguments and return
 * value against the backend contract at compile time; the generated
 * types are the authoritative contract, and the wrappers add only
 * argument shaping and the guide-mode read swap (`./guide`). The
 * shell's bespoke window/byte commands live in `./shell`.
 */

export * from './analytics';
export * from './character';
export { ApiError, type ThrownErrorKind } from './client';
export * from './codex';
// The generated shapes consumers reach through this barrel; `HuntingData`
// and `HarvestData` are the consumer-facing names of the analytics
// aggregates.
export type {
	ActiveActivityView,
	ActivityOption,
	ActivityOptionsResult,
	ActivitySummary,
	ActivityTargetKind,
	AnalyticsHarvest as HarvestData,
	AnalyticsHunting as HuntingData,
	AnalyticsHuntingActivity as HuntingActivityData,
	ApiErrorKind,
	AuctionConfirmInput,
	AuctionExpireInput,
	AuctionListing,
	AuctionListingInput,
	EquipmentListingInput,
	EquipmentTradeInput,
	ExpectedHuntingEconomics,
	InventoryDraftResolution,
	InventoryHoldingCandidate,
	InventoryItem,
	InventorySaleDraft,
	LifetimeStats,
	ManualMobSuggestion,
	MarketBreakEven,
	MarketCommitResult,
	MarketHarvestData,
	MarketHarvestHorizon,
	MarketHarvestItem,
	MarketHistoryPoint,
	MarketHorizon,
	MarketLooterLevel,
	MarketMobRankingRow,
	MarketOverviewRow,
	MarketPastePreview,
	MarketPastePreviewRow,
	MarketReading,
	MarketSkippedLine,
	MarketUnitPriceResult,
	MarketWeaponBreakEven,
	PrivateSaleInput,
	Profession,
	QuestHandInCandidate,
	QuestHandInItem,
	QuestHandInState,
	QuestRewardCandidate,
	QuestRewardReviewInput,
	RealisedSpeciesMarkup,
	RealisedTierMarkup,
	ShrapnelConversionInput,
	StockConversionInput,
	StockPosition,
	StockRemovalInput,
	UnresolvedQuestReward,
} from './commands.gen';
export * from './dev';
export * from './equipment';
export * from './healing';
export * from './inventory';
export * from './maps';
export * from './market';
export * from './protection';
export * from './quests';
export * from './scan';
export * from './sessionDefinitions';
export * from './settings';
// The shell's updater commands stay out of this barrel: $lib/updater
// owns that flow (phases, progress, stores) and imports them directly.
export {
	beginNavigationAreaSelection,
	captureSaleFromOverlay,
	hideNavigationOverlays,
	hideSaleCaptureOverlay,
	hideScanOverlay,
	manualSkillScanCapturePng,
	planetMapImage,
	showNavigationOverlays,
	showSaleCaptureOverlay,
	showScanOverlay,
	toggleCartographyOverlay,
	toggleOverlay,
} from './shell';
export * from './tracking';
