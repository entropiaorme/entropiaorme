/**
 * Hunting-tab view model: the session-definition analysis axis over the same
 * market and realised-outcome vocabulary Tree Cutting established.
 *
 * Sessions are the routines the player deliberately defined (keyed by
 * definition, never by recorded free text). Species evidence remains an
 * internal projection for reconciling Overall market and realised figures;
 * it is not currently presented as a comparison axis.
 *
 * Every MU figure is an estimate, never realised P&L. Realised markup
 * arrives only through recorded stock outcomes, attributed through the weighted
 * species and session-definition provenance of the source loot.
 */

import {
	getAnalyticsHuntingActivity,
	getHuntingRealisedMarkup,
	getMarketHuntMarkups,
	type HuntingActivityData,
	type MarketHarvestData,
	type MarketHarvestItem,
} from '$lib/api';
import type {
	HuntingActivityComparison,
	HuntingDefinitionComparison,
	HuntingRewardStatus,
	HuntingSpeciesComparison,
} from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { type AnalyticsRange, analyticsPeriod, isAnalyticsRange } from './analyticsRange';
import {
	type ConfidenceMode,
	effectiveMarkup,
	manualValuation,
	marketOpportunity,
	NANOCUBE_FALLBACK_MARKUP,
	projectLoot,
	type TreeCuttingItem,
} from './treeCuttingModel.svelte';

// ── Sessions axis ──────────────────────────────────────────────────────

/** One scope's completion reward, stated identically wherever economics are
 * presented. Sessions and Overall aggregate it from the activities that
 * partition them; an activity states its own. */
export type RewardContext = {
	/** Observed reward TT, exactly as captured at completion. */
	rewardTtPed: number;
	/** The same reward valued at current market. Null only when no reward
	 * items were recorded, never merely because none of them are tradeable. */
	rewardMuPed: number | null;
	/** Every distinct treatment the scope contains, `none` excluded. The
	 * figure above is the answer; this says what it is made of, and (via
	 * `unverified`) what it knowingly leaves out. */
	treatments: HuntingRewardStatus[];
};

/** One session-definition row with its display key (definitions are keyed
 * by id; the unassigned bucket by this sentinel). */
export type HuntingActivitySection = Omit<HuntingActivityComparison, 'variants'> & {
	key: string;
	isUnscoped: boolean;
	/** Current market projection over actual completion reward items. Missing
	 * or excluded item market data contributes TT at 100%, never a proxy. */
	rewardMuPed: number | null;
	/** The reward's projected value against cycled spend: the outlook's
	 * additive term, since a reward sits outside the expected-return model. */
	rewardMuRate: number | null;
	/** Long-run rate including the completion reward. `expectedMarketRate`
	 * deliberately stays loot-only so nothing reading it changes meaning. */
	expectedTotalRate: number | null;
	muProjectedReturns: number | null;
	lootMarkupFactor: number | null;
	expectedTtRate: number | null;
	expectedMarketRate: number | null;
	items: TreeCuttingItem[];
	variants: HuntingActivitySection[];
};

export type HuntingSessionSection = Omit<HuntingDefinitionComparison, 'activities'> & {
	key: string;
	isUnassigned: boolean;
	confirmedRewardPed: number;
	reward: RewardContext;
	rewardMuRate: number | null;
	expectedTotalRate: number | null;
	realisedMarkup: number;
	muProjectedReturns: number | null;
	muRate: number | null;
	lootMarkupFactor: number | null;
	expectedTtRate: number | null;
	expectedMarketRate: number | null;
	realisedReturns: number;
	realisedRate: number;
	items: TreeCuttingItem[];
	activities: HuntingActivitySection[];
};

export type HuntingSessionSortKey = 'name' | 'cycled' | 'realisedRate' | 'muRate';

// ── Dormant species projection ─────────────────────────────────────────

/** The stable label retained for diagnostics and contract-level proofs. */
export const UNCLASSIFIED_LABEL = 'Unclassified';

/** One species row with the merged market layer. Kept internally so Overall
 * remains reconcilable while the species comparison UI is dormant. */
export type HuntingTargetSection = HuntingSpeciesComparison & {
	key: string;
	label: string;
	isUnclassified: boolean;
	realisedMarkup: number;
	muProjectedReturns: number | null;
	muRate: number | null;
	lootMarkupFactor: number | null;
	expectedTtRate: number | null;
	expectedMarketRate: number | null;
	realisedReturns: number;
	realisedRate: number;
	items: TreeCuttingItem[];
};

/** The combined direct + market stat line across the whole activity. */
export type HuntingOverallLine = {
	cycled: number;
	returns: number;
	lootRate: number;
	reward: RewardContext;
	rewardMuRate: number | null;
	expectedTotalRate: number | null;
	muProjectedReturns: number | null;
	muRate: number | null;
	lootMarkupFactor: number | null;
	expectedTtRate: number | null;
	expectedMarketRate: number | null;
	expected: HuntingActivityData['overall']['expected'];
	realisedMarkup: number;
	realisedReturns: number;
	realisedRate: number;
	/** Confirmed markup whose species has no economic row in the selected period:
	 * still real, still counted, and disclosed rather than dropped. */
	realisedOutsidePeriod: number;
};

export function createHuntingModel() {
	let data = $state<HuntingActivityData | null>(null);
	let market = $state<MarketHarvestData | null>(null);
	let realisedBySpecies = $state<Map<string, number>>(new Map());
	let realisedByDefinition = $state<Map<number, number>>(new Map());
	let loading = $state(true);
	let error = $state<string | null>(null);
	let confidenceMode = $state<ConfidenceMode>('liquidMiddling');
	let activeRange = $state<AnalyticsRange>('All Time');
	let selectedSessionKey = $state<string | null>(null);

	let loadEpoch = 0;

	function projectedEconomics(
		compositionTt: number,
		marketValue: number | null,
		expectedTtRate: number | null,
	) {
		const lootMarkupFactor =
			compositionTt > 0 && marketValue !== null ? marketValue / compositionTt : null;
		return {
			lootMarkupFactor,
			expectedTtRate,
			expectedMarketRate:
				expectedTtRate !== null && lootMarkupFactor !== null
					? expectedTtRate * lootMarkupFactor
					: null,
		};
	}

	async function loadData(period: string = 'all') {
		const epoch = ++loadEpoch;
		const navigationStarted = performance.now();
		let backendCompleted = navigationStarted;
		loading = true;
		error = null;
		try {
			// The comparison aggregate is the spine; market and confirmed-markup
			// context are best effort and may degrade without blanking it.
			const [activity, markets, realised] = await Promise.all([
				getAnalyticsHuntingActivity(period),
				getMarketHuntMarkups().catch(() => null),
				getHuntingRealisedMarkup().catch(() => ({ species: [], definitions: [] })),
			]);
			if (epoch !== loadEpoch) return;
			backendCompleted = performance.now();
			data = activity;
			market = markets;
			realisedBySpecies = new Map(realised.species.map((row) => [row.mobSpecies, row.netMarkup]));
			realisedByDefinition = new Map(
				realised.definitions.map((row) => [row.definitionId, row.netMarkup]),
			);
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load hunting data');
		} finally {
			if (epoch === loadEpoch) {
				loading = false;
				if (typeof requestAnimationFrame === 'function') {
					requestAnimationFrame(() => {
						console.debug('analytics.hunting.first_paint', {
							backendMs: Number((backendCompleted - navigationStarted).toFixed(2)),
							paintMs: Number((performance.now() - navigationStarted).toFixed(2)),
							resultCount: (data?.definitions.length ?? 0) + (data?.species.length ?? 0),
						});
					});
				}
			}
		}
	}

	// ── Sessions ──

	const sessionSections = $derived.by<HuntingSessionSection[]>(() => {
		if (!data) return [];
		const marketByItem = new Map<string, MarketHarvestItem>(
			(market?.items ?? []).map((item) => [item.itemName, item]),
		);
		const activitySection = (
			row: HuntingActivityComparison,
			parentKey: string,
			index: number,
		): HuntingActivitySection => {
			const key = `${parentKey}/${row.kind}:${row.label}:${index}`;
			const projection = projectLoot(
				row.lootItems,
				row.cycled,
				market,
				marketByItem,
				confidenceMode,
			);
			const rewardMuPed = projectRewardValue(row.rewardItems, market, marketByItem, confidenceMode);
			const projected = projectedEconomics(
				row.lootItems.reduce((sum, item) => sum + item.valuePed, 0),
				projection.muProjectedReturns,
				row.expected?.expectedTtRate ?? null,
			);
			const rates = rewardRates(
				rewardContextOf({ ...row, rewardMuPed }),
				row.cycled,
				projected.expectedMarketRate,
			);
			return {
				...row,
				key,
				isUnscoped: row.kind === 'ambient',
				rewardMuPed,
				...rates,
				muProjectedReturns: projection.muProjectedReturns,
				...projected,
				items: projection.items,
				variants: row.variants.map((variant, variantIndex) =>
					activitySection(variant, key, variantIndex),
				),
			};
		};
		return data.definitions.map((row) => {
			const projection = projectLoot(
				row.lootItems,
				row.cycled,
				market,
				marketByItem,
				confidenceMode,
			);
			const realisedMarkup =
				row.definitionId === null ? 0 : (realisedByDefinition.get(row.definitionId) ?? 0);
			// Top-level activities partition the definition. Family variants are
			// explanatory children whose reward is already present in their parent.
			const key = row.definitionId === null ? 'unassigned' : `definition:${row.definitionId}`;
			const activities = row.activities.map((activity, index) =>
				activitySection(activity, key, index),
			);
			const reward = mergeRewardContexts(activities.map(rewardContextOf));
			const confirmedRewardPed = reward.rewardTtPed;
			const realisedReturns = row.returns + confirmedRewardPed + realisedMarkup;
			const projected = projectedEconomics(
				row.lootItems.reduce((sum, item) => sum + item.valuePed, 0),
				projection.muProjectedReturns,
				row.expected?.expectedTtRate ?? null,
			);
			return {
				...row,
				key,
				isUnassigned: row.definitionId === null,
				confirmedRewardPed,
				reward,
				...rewardRates(reward, row.cycled, projected.expectedMarketRate),
				realisedMarkup,
				muProjectedReturns: projection.muProjectedReturns,
				muRate: projection.muRate,
				...projected,
				realisedReturns,
				realisedRate: row.cycled > 0 ? realisedReturns / row.cycled : 0,
				items: projection.items,
				activities,
			};
		});
	});

	const sessionTable = createTableModel<HuntingSessionSection>({
		rows: () => sessionSections,
		pageSize: Number.MAX_SAFE_INTEGER,
		searchText: (row) => [row.name],
		initialSort: { key: 'cycled', dir: 'desc' },
		defaultSortDirs: {
			name: 'asc',
			cycled: 'desc',
			realisedRate: 'desc',
			muRate: 'desc',
		},
		comparators: {
			name: (a, b) => a.name.localeCompare(b.name),
		},
	});

	/** The session whose detail replaces Overall. A null selection means
	 * Overall; an invalid selection also returns there when a period switch
	 * retires the selected key. */
	const selectedSession = $derived.by<HuntingSessionSection | null>(() => {
		if (selectedSessionKey === null) return null;
		return sessionSections.find((s) => s.key === selectedSessionKey) ?? null;
	});

	// ── Dormant species projection ──

	const targetSections = $derived.by<HuntingTargetSection[]>(() => {
		if (!data) return [];
		const marketByItem = new Map<string, MarketHarvestItem>(
			(market?.items ?? []).map((item) => [item.itemName, item]),
		);
		return data.species.map((row) => {
			const isUnclassified = row.mobSpecies === '';
			const projection = projectLoot(
				row.lootItems,
				row.cycled,
				market,
				marketByItem,
				confidenceMode,
			);
			const realisedMarkup = realisedBySpecies.get(row.mobSpecies) ?? 0;
			const realisedReturns = row.returns + realisedMarkup;
			const projected = projectedEconomics(
				row.lootItems.reduce((sum, item) => sum + item.valuePed, 0),
				projection.muProjectedReturns,
				row.expected?.expectedTtRate ?? null,
			);
			return {
				...row,
				key: isUnclassified ? 'unclassified' : `species:${row.mobSpecies}`,
				label: isUnclassified ? UNCLASSIFIED_LABEL : row.mobSpecies,
				isUnclassified,
				realisedMarkup,
				muProjectedReturns: projection.muProjectedReturns,
				muRate: projection.muRate,
				...projected,
				realisedReturns,
				realisedRate: row.cycled > 0 ? realisedReturns / row.cycled : 0,
				items: projection.items,
			};
		});
	});

	// ── Overall ──

	const overall = $derived.by<HuntingOverallLine | null>(() => {
		if (!data || data.overall.cycled <= 0) return null;
		// Market figures aggregate over the species sections so the headline
		// reconciles with the rows beneath it; a section without market
		// context contributes nothing.
		const anyMarket = targetSections.some((s) => s.muProjectedReturns !== null);
		const muProjectedReturns = anyMarket
			? targetSections.reduce((sum, s) => sum + (s.muProjectedReturns ?? 0), 0)
			: null;
		const cycled = data.overall.cycled;
		const muRate = muProjectedReturns !== null && cycled > 0 ? muProjectedReturns / cycled : null;
		const projected = projectedEconomics(
			targetSections.reduce(
				(sum, section) => sum + section.items.reduce((itemSum, item) => itemSum + item.ttValue, 0),
				0,
			),
			muProjectedReturns,
			data.overall.expected?.expectedTtRate ?? null,
		);
		// Realised markup sums over EVERY species with recognised stock outcomes, not
		// only those hunted in the selected period: the money exists either
		// way, and the remainder is disclosed rather than silently dropped.
		const realisedMarkup = [...realisedBySpecies.values()].reduce((sum, v) => sum + v, 0);
		const realisedInPeriod = targetSections.reduce((sum, s) => sum + s.realisedMarkup, 0);
		const reward = mergeRewardContexts(sessionSections.map((session) => session.reward));
		const realisedReturns = data.overall.returns + reward.rewardTtPed + realisedMarkup;
		return {
			realisedOutsidePeriod: realisedMarkup - realisedInPeriod,
			cycled,
			returns: data.overall.returns,
			lootRate: data.overall.lootRate,
			reward,
			...rewardRates(reward, cycled, projected.expectedMarketRate),
			muProjectedReturns,
			muRate,
			...projected,
			expected: data.overall.expected,
			realisedMarkup,
			realisedReturns,
			realisedRate: cycled > 0 ? realisedReturns / cycled : 0,
		};
	});

	return {
		get data() {
			return data;
		},
		get overall() {
			return overall;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get confidenceMode() {
			return confidenceMode;
		},
		set confidenceMode(mode: ConfidenceMode) {
			confidenceMode = mode;
		},
		get activeRange() {
			return activeRange;
		},
		set activeRange(value: string) {
			if (isAnalyticsRange(value)) activeRange = value;
		},
		get period() {
			return analyticsPeriod(activeRange);
		},
		get sessionSections() {
			return sessionSections;
		},
		get sessionTable() {
			return sessionTable;
		},
		get selectedSession() {
			return selectedSession;
		},
		selectSession(key: string | null) {
			selectedSessionKey = key;
		},
		get targetSections() {
			return targetSections;
		},
		loadData,
	};
}

/** Universal Ammo is liquid PED in item form: its exit is face value, never a
 * market sale, so it is neither projected nor floored to the nanocube proxy.
 * (Shrapnel's own carve-out is `effectiveItemMarkup`'s 101% conversion.) */
export function isTradeableRewardItem(itemName: string): boolean {
	return itemName.trim().toLocaleLowerCase() !== 'universal ammo';
}

/** Value observed reward items at current direct item markup. A missing or
 * confidence-excluded market observation leaves that item's TT unchanged;
 * the generic loot projection's Nanocube substitution is deliberately not
 * used for quest rewards. */
export function projectRewardItems(
	items: HuntingActivityComparison['rewardItems'],
	market: MarketHarvestData | null,
	marketByItem: Map<string, MarketHarvestItem>,
	confidenceMode: ConfidenceMode,
): number | null {
	const stockItems = items.filter((item) => isTradeableRewardItem(item.itemName));
	if (stockItems.length === 0) return null;
	const nanocube = market?.nanocubeMarkupPct ?? NANOCUBE_FALLBACK_MARKUP;
	return stockItems.reduce((sum, item) => {
		const marketItem = marketByItem.get(item.itemName);
		if (!market || !marketItem) return sum + item.valuePed;
		// The player's own value stands wherever it is the more recent.
		const manual = manualValuation(marketItem, item.quantity, item.valuePed);
		if (manual) return sum + manual.valuePed;
		if (marketItem.unitPricePed !== null) {
			return sum + item.quantity * marketItem.unitPricePed;
		}
		const opportunity = marketOpportunity(marketItem, nanocube);
		const applied = effectiveMarkup(opportunity, nanocube, confidenceMode);
		const markupPct = opportunity.usesNanocube || applied.floored ? 100 : applied.markupPct;
		return sum + (item.valuePed * markupPct) / 100;
	}, 0);
}

/** The whole reward valued at current market: its tradeable component
 * projected, plus every liquid unit at face value. `projectRewardItems`
 * answers the narrower stock question and is shared with the Quests tab,
 * which composes it the same way. Null only when nothing was recorded. */
export function projectRewardValue(
	items: HuntingActivityComparison['rewardItems'],
	market: MarketHarvestData | null,
	marketByItem: Map<string, MarketHarvestItem>,
	confidenceMode: ConfidenceMode,
): number | null {
	if (items.length === 0) return null;
	const totalTt = items.reduce((sum, item) => sum + item.valuePed, 0);
	const stockTt = items
		.filter((item) => isTradeableRewardItem(item.itemName))
		.reduce((sum, item) => sum + item.valuePed, 0);
	const projected = projectRewardItems(items, market, marketByItem, confidenceMode) ?? 0;
	return totalTt - stockTt + projected;
}

/** Fold several scopes' rewards into one. Every distinct treatment is
 * retained rather than collapsed to a single label, so the disclosure can
 * name each one behind the figure instead of the figure standing for a
 * classification the aggregate cannot claim. */
export function mergeRewardContexts(contexts: RewardContext[]): RewardContext {
	const treatments = new Set(contexts.flatMap((context) => context.treatments));
	const valued = contexts.filter((context) => context.rewardMuPed !== null);
	return {
		rewardTtPed: contexts.reduce((sum, context) => sum + context.rewardTtPed, 0),
		rewardMuPed: valued.length
			? valued.reduce((sum, context) => sum + (context.rewardMuPed ?? 0), 0)
			: null,
		treatments: [...treatments],
	};
}

/** One activity's own reward, as the same context its scope aggregates. */
export function rewardContextOf(activity: {
	confirmedRewardPed: number;
	rewardMuPed: number | null;
	rewardStatus: HuntingRewardStatus;
}): RewardContext {
	return {
		rewardTtPed: activity.confirmedRewardPed,
		rewardMuPed: activity.rewardMuPed,
		treatments: activity.rewardStatus === 'none' ? [] : [activity.rewardStatus],
	};
}

/** A reward stands entirely outside the expected-return model, so it enters
 * the long-run outlook additively rather than as another factor. */
function rewardRates(reward: RewardContext, cycled: number, expectedMarketRate: number | null) {
	const rewardMuRate =
		reward.rewardMuPed !== null && cycled > 0 ? reward.rewardMuPed / cycled : null;
	return {
		rewardMuRate,
		expectedTotalRate:
			expectedMarketRate !== null ? expectedMarketRate + (rewardMuRate ?? 0) : null,
	};
}

export type HuntingModel = ReturnType<typeof createHuntingModel>;
