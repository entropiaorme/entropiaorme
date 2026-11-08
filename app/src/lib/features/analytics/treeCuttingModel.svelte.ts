/**
 * Tree Cutting-tab view model. Durable effective yield tiers are the
 * source activities.
 * Each tier carries its realised TT and holding-independent market
 * opportunity over the board composition actually extracted.
 *
 * Two feeds compose here: the realised harvest aggregate (accounting
 * side) and the per-item market signals (the informational market
 * layer). They are merged in this frontend model; the accounting
 * boundary keeps them apart in the backend, and every MU figure is an
 * estimate, never realised P&L.
 *
 * Market opportunity is intrinsic to the observed market, not the
 * player's current holding. Markup premium, normalised turnover,
 * evidence horizon, and a fee-efficient parcel distinguish broad,
 * niche, thin, and unsupported direct markets. Unsupported items use
 * the nanocube recycling route as the conservative market floor.
 */

import {
	getAnalyticsHarvest,
	getHarvestRealisedMarkup,
	getMarketHarvestMarkups,
	type HarvestData,
	type MarketHarvestData,
	type MarketHarvestItem,
	type MarketManualMarkup,
} from '$lib/api';
import type {
	AuctionListingInput,
	HarvestLootItem,
	HarvestTierComparison,
	HarvestYieldTier,
} from '$lib/types/analytics';
import type { DerivedSource, HoldingOrigin } from '$lib/api/commands.gen';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { type AnalyticsRange, analyticsPeriod, isAnalyticsRange } from './analyticsRange';

// ── Holding-independent market opportunity ────────────────────────────

/** What the shared loot-sale intake knows before Inventory commits it. */
export type ActivityListingDraft = Omit<AuctionListingInput, 'profession'>;
export type ActivityTradeDraft = {
	itemName: string;
	quantity: number;
	soldFor: number;
	soldAt: string | null;
};

export type OpportunityKind = 'broad' | 'niche' | 'thin' | 'recycle';
export type ConfidenceTier = 'liquid' | 'middling' | 'illiquid';
export type ConfidenceMode = 'liquid' | 'liquidMiddling' | 'all';
/**
 * Where a valuation's markup came from: the item's own market reading, the
 * Nanocube floor (an unsupported or confidence-excluded market), Shrapnel's
 * fixed conversion, the player's own entry (`manual`), or the player's live
 * Nanocube mark (`nanocube_mark`, honoured by the live session figures only).
 */
export type MarkupBasis =
	| 'market'
	| 'nanocube'
	| 'shrapnel_conversion'
	| 'manual'
	| 'nanocube_mark';

const TIER_RANK: Record<ConfidenceTier, number> = { liquid: 3, middling: 2, illiquid: 1 };
const MODE_THRESHOLD: Record<ConfidenceMode, number> = { liquid: 3, liquidMiddling: 2, all: 1 };

const WEEKS_PER_MONTH = 4.345;
const WEEKS_PER_YEAR = 52.14;
/**
 * The opportunity thresholds below are provisional market heuristics.
 * Recalibrate them through dogfooding as more real items are bought and
 * sold, so the model stays aligned with the lived difficulty of realising
 * markup rather than becoming anchored to the first available examples.
 */
/** Stable capacity-classification proxy. The exact packet recommendation
 * stays separate until packet size versus volume is deliberately revisited. */
const CAPACITY_FEE_PROXY_PED = 0.5;
const CAPACITY_FEE_SHARE = 0.1;
/** A healthy parcel at or below 15% of weekly turnover is broad when the
 * markup itself is supported by weekly evidence. */
const BROAD_MAX_MARKET_SHARE = 0.15;
/** Above 75% of the turnover observed at the resolved horizon, even a
 * healthy parcel is too large for the direct market evidence to support. */
const SUPPORTED_MAX_MARKET_SHARE = 0.75;
/** A 200%+ direct market has enough unit margin to be economically
 * meaningful despite sparse cadence, so it is classified as niche. */
const NICHE_MIN_PREMIUM = 1;
/** Nanocube markup fallback (percent) when the market feed carries no
 * nanocube observation. */
export const NANOCUBE_FALLBACK_MARKUP = 100.6;
/** Shrapnel's deterministic conversion into Universal Ammo. This is a
 * projected valuation until the player records the conversion; only then
 * does its 1% gain cross into realised accounting. */
export const SHRAPNEL_CONVERSION_MARKUP = 101;

export function isShrapnel(itemName: string): boolean {
	return itemName.trim().toLocaleLowerCase() === 'shrapnel';
}

/** The canonical item stock recycles into, at 1:1 TT. */
/** The resolved horizon's TT turnover normalised to a weekly rate, so
 * horizons compare without involving the player's position. */
export function weeklyEquivalentVolume(salesPed: number | null, horizon: string | null): number {
	if (salesPed == null || salesPed <= 0) return 0;
	if (horizon === 'week') return salesPed;
	if (horizon === 'month') return salesPed / WEEKS_PER_MONTH;
	if (horizon === 'year') return salesPed / WEEKS_PER_YEAR;
	return 0;
}

export type MarketOpportunity = {
	kind: OpportunityKind;
	ownMarkupPct: number | null;
	appliedMarkupPct: number;
	usesNanocube: boolean;
	horizon: string | null;
	salesPed: number | null;
	weeklySalesPed: number | null;
	/** The game's Sales PED field is TT turnover for these
	 * percentage-markup harvest items, normalised to one week. */
	weeklyEquivalentSalesPed: number;
	/** Gross direct-market premium transacted per normalised week. This is
	 * market-wide evidence, never personally capturable profit. */
	weeklyPremiumThroughput: number;
	/** Existing capacity-classification parcel. Separate from the exact
	 * backend recommendation until volume calibration is revisited. */
	efficientBatchTt: number | null;
	/** Efficient parcel as a share of turnover at the resolved horizon. */
	efficientBatchMarketShare: number | null;
	/** Efficient parcel expressed as weeks of normalised market turnover. */
	efficientBatchMarketWeeks: number | null;
};

/**
 * Classify an item's direct market without consulting personal stock.
 * Broad, niche, and thin markets all carry a real opportunity and retain
 * their own MU. Direct markets whose efficient parcel overwhelms recent
 * turnover, plus uncovered items, use the universal nanocube floor.
 */
export function marketOpportunity(
	market: MarketHarvestItem | undefined,
	nanocubeMarkupPct: number,
): MarketOpportunity {
	const ownMarkupPct = market?.markupPct ?? null;
	const horizon = market?.horizon ?? null;
	const salesPed = market?.salesPed ?? null;
	const weeklySalesPed = market?.readings.find((r) => r.horizon === 'week')?.salesPed ?? null;
	const weeklyEquivalentSalesPed = weeklyEquivalentVolume(salesPed, horizon);
	const premium = ownMarkupPct == null ? 0 : ownMarkupPct / 100 - 1;
	const efficientBatchTt =
		premium > 0 ? CAPACITY_FEE_PROXY_PED / (CAPACITY_FEE_SHARE * premium) : null;
	const efficientBatchMarketShare =
		efficientBatchTt !== null && salesPed !== null && salesPed > 0
			? efficientBatchTt / salesPed
			: null;
	const efficientBatchMarketWeeks =
		efficientBatchTt !== null && weeklyEquivalentSalesPed > 0
			? efficientBatchTt / weeklyEquivalentSalesPed
			: null;
	const weeklyPremiumThroughput = Math.max(0, premium) * weeklyEquivalentSalesPed;

	let directKind: Exclude<OpportunityKind, 'recycle'> | null = null;
	if (
		premium > 0 &&
		efficientBatchMarketShare !== null &&
		efficientBatchMarketShare <= SUPPORTED_MAX_MARKET_SHARE
	) {
		if (horizon === 'week' && efficientBatchMarketShare <= BROAD_MAX_MARKET_SHARE) {
			directKind = 'broad';
		} else if (premium >= NICHE_MIN_PREMIUM) {
			directKind = 'niche';
		} else {
			directKind = 'thin';
		}
	}

	const usesNanocube =
		directKind === null || ownMarkupPct === null || ownMarkupPct < nanocubeMarkupPct;
	const kind: OpportunityKind = usesNanocube ? 'recycle' : (directKind ?? 'thin');
	return {
		kind,
		ownMarkupPct,
		appliedMarkupPct: usesNanocube ? nanocubeMarkupPct : ownMarkupPct,
		usesNanocube,
		horizon,
		salesPed,
		weeklySalesPed,
		weeklyEquivalentSalesPed,
		weeklyPremiumThroughput,
		efficientBatchTt,
		efficientBatchMarketShare,
		efficientBatchMarketWeeks,
	};
}

/** Preserve the established High / Mid / Low volume UI over the richer
 * holding-independent opportunity model. Weekly thin markets have enough
 * observed cadence for medium confidence; fallback-horizon thin markets
 * and unsupported recycling routes remain low confidence. */
export function opportunityTier(opportunity: MarketOpportunity): ConfidenceTier {
	if (opportunity.kind === 'broad') return 'liquid';
	if (
		opportunity.kind === 'niche' ||
		(opportunity.kind === 'thin' && opportunity.horizon === 'week')
	) {
		return 'middling';
	}
	return 'illiquid';
}

/** Select the multiplier used by the MU aggregate. The user-facing
 * confidence toggle keeps its established behaviour, but eligibility is
 * now based on market-wide evidence rather than the player's holding. */
export function effectiveMarkup(
	opportunity: MarketOpportunity,
	nanocubeMarkupPct: number,
	mode: ConfidenceMode,
): { markupPct: number; floored: boolean } {
	const tier = opportunityTier(opportunity);
	const trustsTier = TIER_RANK[tier] >= MODE_THRESHOLD[mode];
	const trustsDirect = !opportunity.usesNanocube && trustsTier;
	return {
		markupPct: trustsDirect ? opportunity.appliedMarkupPct : nanocubeMarkupPct,
		floored: !trustsDirect,
	};
}

/** A row's value at the player's own markup entry: a percentage of its TT,
 * or its TT plus the entered PED per unit (exact whatever the unit TT). */
export interface ManualValuation {
	valuePed: number;
	/** The value as a markup of the row's TT; for a row with no TT, the
	 * entry's own percentage, or 100 when it has none. */
	markupPct: number;
	entry: MarketManualMarkup;
}

/**
 * The player's manual markup applied to one row, when it is the reading the
 * item's valuation uses: the backend names the more recent of the manual
 * entry and the market import as `preferred`. Null otherwise.
 */
export function manualValuation(
	market: MarketHarvestItem | undefined,
	quantity: number,
	ttValue: number,
): ManualValuation | null {
	const entry = market?.manual ?? null;
	if (!entry || market?.preferred !== 'manual') return null;
	const valuePed =
		entry.kind === 'percent' ? (ttValue * entry.value) / 100 : ttValue + quantity * entry.value;
	const markupPct = ttValue > 0 ? (valuePed / ttValue) * 100 : (entry.markupPct ?? 100);
	return { valuePed, markupPct, entry };
}

/** What the valuation route may take beyond the market reading: the
 * player's manual entry for the row, and whether the live Nanocube mark
 * applies (only the live session figures pass it). */
export interface ValuationOverrides {
	manual?: ManualValuation | null;
	nanocubeMarked?: boolean;
}

/** Resolve the valuation route shared by activity projections and Inventory.
 * Shrapnel deliberately ignores market confidence because its ordinary exit
 * is the game's deterministic 101% ammo conversion, not a speculative sale.
 * Then the live Nanocube mark, then the player's manual entry (trusted as
 * given, so neither the confidence filter nor the floor touches it), then
 * the market reading under the confidence mode. */
export function effectiveItemMarkup(
	itemName: string,
	opportunity: MarketOpportunity | null,
	nanocubeMarkupPct: number,
	mode: ConfidenceMode,
	overrides: ValuationOverrides = {},
): { markupPct: number; basis: MarkupBasis } | null {
	if (isShrapnel(itemName)) {
		return { markupPct: SHRAPNEL_CONVERSION_MARKUP, basis: 'shrapnel_conversion' };
	}
	if (overrides.nanocubeMarked) {
		return { markupPct: nanocubeMarkupPct, basis: 'nanocube_mark' };
	}
	if (overrides.manual) {
		return { markupPct: overrides.manual.markupPct, basis: 'manual' };
	}
	if (!opportunity) return null;
	const applied = effectiveMarkup(opportunity, nanocubeMarkupPct, mode);
	return {
		markupPct: applied.markupPct,
		basis: applied.floored ? 'nanocube' : 'market',
	};
}

// ── Section derivation ─────────────────────────────────────────────────

const TIER_LABEL: Record<HarvestYieldTier, string> = {
	short: 'Short Boards',
	long: 'Boards',
	huge: 'Long Boards',
	unknown: 'Unclassified',
};

export type TreeCuttingItem = {
	name: string;
	quantity: number;
	ttValue: number;
	sharePct: number;
	opportunity: MarketOpportunity;
	ownMarkupPct: number | null;
	markupHorizon: string | null;
	tier: ConfidenceTier;
	effectiveMarkupPct: number;
	markupBasis: MarkupBasis;
	floored: boolean;
	salesPed: number | null;
	weeklySalesPed: number | null;
	/** The row's estimated market value at the applied markup. */
	marketValuePed: number;
	/** The player's standing manual entry for the item, applied or not. */
	manual: MarketManualMarkup | null;
	/** Whether the player marked the item for Nanocube valuation in the live
	 * session figures (applied only where the projection is live). */
	nanocubeMarked: boolean;
};

/** The combined stat line across every sub-activity (the "Overall" block). */
export type TreeCuttingOverall = {
	/** Markup recorded stock outcomes have realised, already inside `realisedReturns`. */
	realisedMarkup: number;
	cycled: number;
	returns: number;
	lootRate: number;
	muProjectedReturns: number | null;
	muRate: number | null;
	/** Loot TT plus the markup recorded stock outcomes have realised. It equals TT
	 * until something sells, which is the recognition boundary made
	 * visible. */
	realisedReturns: number;
	realisedRate: number;
};

export type TreeCuttingSection = {
	/** Markup recorded stock outcomes for this tier's output have realised, already
	 * inside `realisedReturns`. Zero until a sale is confirmed. */
	realisedMarkup: number;
	yieldTier: HarvestYieldTier;
	swings: number;
	cycled: number;
	returns: number;
	lootRate: number;
	/** Present-market counterfactual over this activity's observed output
	 * composition. Holding-independent and estimated only. */
	muProjectedReturns: number | null;
	muRate: number | null;
	realisedReturns: number;
	realisedRate: number;
	items: TreeCuttingItem[];
};

export type TreeCuttingActivitySortKey = 'yieldTier' | 'cycled' | 'realisedRate' | 'muRate';

export function treeCuttingActivityName(section: TreeCuttingSection): string {
	return TIER_LABEL[section.yieldTier];
}

export function harvestTierLabel(tier: HarvestYieldTier): string {
	return TIER_LABEL[tier];
}

/** One horizon's market reading (day/week/month/year) for the stock row's
 * markup detail view. */
export type StockHorizonReading = {
	horizon: string;
	markupPct: number | null;
	salesPed: number;
};

/** One item's current stock: what the player holds now, after everything
 * that has left through a listing or a conversion and back through an
 * expiry. Holdings are operational context only. The opportunity is the
 * same holding-independent profile used by every source activity. */
export type TreeCuttingStock = {
	/** TT per unit, known even when none is held. Inventory only. */
	unitTt?: number;
	/** Markup paid to produce this stock that a sale must recover. */
	paidMarkup?: number;
	/** The activities a sale would credit. Inventory only. */
	origins?: HoldingOrigin[];
	/** What a conversion made this stock from. Inventory only. */
	derivedFrom?: DerivedSource[];
	itemName: string;
	heldQty: number;
	heldTt: number;
	/** Out on an unresolved auction: gone from `heldQty`, but coming back
	 * if the listing expires, so it is shown rather than silently absent. */
	listedQty: number;
	/** The day/week/month/year breakdown for the detail view. */
	readings: StockHorizonReading[];
	opportunity: MarketOpportunity | null;
	markupPct: number | null;
	markupHorizon: string | null;
	tier: ConfidenceTier | null;
	effectiveMarkupPct: number | null;
	markupBasis: MarkupBasis | null;
	floored: boolean;
	salesPed: number | null;
	weeklySalesPed: number | null;
	recommendedPacketTt: number | null;
	/** The item's market signals, with the player's standing manual entry
	 * (applied or not); null when the feed has no row for it. */
	marketItem: MarketHarvestItem | null;
};

/** Project one activity's loot composition at current market markup. Shared
 * with the Hunting model: the projection is identical maths whichever
 * activity's composition it runs over. */
export function projectLoot(
	lootItems: HarvestLootItem[],
	cycled: number,
	market: MarketHarvestData | null,
	marketByItem: Map<string, MarketHarvestItem>,
	confidenceMode: ConfidenceMode,
	options: { live?: boolean } = {},
): {
	items: TreeCuttingItem[];
	muProjectedReturns: number | null;
	muRate: number | null;
} {
	const nanocube = market?.nanocubeMarkupPct ?? NANOCUBE_FALLBACK_MARKUP;
	const totalTt = lootItems.reduce((sum, item) => sum + item.valuePed, 0);

	let marketProjected = 0;
	const items: TreeCuttingItem[] = lootItems.map((item) => {
		const m = marketByItem.get(item.itemName);
		const opportunity = marketOpportunity(m, nanocube);
		const tier = opportunityTier(opportunity);
		const manual = manualValuation(m, item.quantity, item.valuePed);
		const nanocubeMarked = m?.nanocubeMarked ?? false;
		const applied = effectiveItemMarkup(item.itemName, opportunity, nanocube, confidenceMode, {
			manual,
			nanocubeMarked: options.live === true && nanocubeMarked,
		});
		if (!applied) throw new Error(`Missing valuation route for ${item.itemName}`);
		const marketValuePed =
			applied.basis === 'manual' && manual
				? manual.valuePed
				: (item.valuePed * applied.markupPct) / 100;
		marketProjected += marketValuePed;
		return {
			name: item.itemName,
			quantity: item.quantity,
			ttValue: item.valuePed,
			sharePct: totalTt > 0 ? (item.valuePed / totalTt) * 100 : 0,
			opportunity,
			ownMarkupPct: opportunity.ownMarkupPct,
			markupHorizon: opportunity.horizon,
			tier,
			effectiveMarkupPct: applied.markupPct,
			markupBasis: applied.basis,
			floored: applied.basis === 'nanocube' || applied.basis === 'nanocube_mark',
			salesPed: opportunity.salesPed,
			weeklySalesPed: opportunity.weeklySalesPed,
			marketValuePed,
			manual: m?.manual ?? null,
			nanocubeMarked,
		};
	});

	const muProjectedReturns = market ? marketProjected : null;
	const muRate = muProjectedReturns !== null && cycled > 0 ? muProjectedReturns / cycled : null;
	return { items, muProjectedReturns, muRate };
}

function toSection(
	tier: HarvestTierComparison,
	market: MarketHarvestData | null,
	marketByItem: Map<string, MarketHarvestItem>,
	confidenceMode: ConfidenceMode,
	realisedMarkup: number,
): TreeCuttingSection {
	const projection = projectLoot(tier.lootItems, tier.cycled, market, marketByItem, confidenceMode);

	return {
		yieldTier: tier.yieldTier,
		swings: tier.swings,
		cycled: tier.cycled,
		returns: tier.returns,
		lootRate: tier.lootRate,
		muProjectedReturns: projection.muProjectedReturns,
		muRate: projection.muRate,
		realisedReturns: tier.returns + realisedMarkup,
		realisedRate: tier.cycled > 0 ? (tier.returns + realisedMarkup) / tier.cycled : 0,
		realisedMarkup,
		items: projection.items,
	};
}

export function createTreeCuttingModel() {
	let data = $state<HarvestData | null>(null);
	let market = $state<MarketHarvestData | null>(null);
	let realisedByTier = $state<Map<HarvestYieldTier, number>>(new Map());
	let loading = $state(true);
	let error = $state<string | null>(null);
	let confidenceMode = $state<ConfidenceMode>('liquidMiddling');
	let activeRange = $state<AnalyticsRange>('All Time');
	// Which board activity replaces Overall. Null is deliberately Overall.
	let selectedTier = $state<HarvestYieldTier | null>(null);

	let loadEpoch = 0;

	async function loadData(period: string = 'all') {
		const epoch = ++loadEpoch;
		loading = true;
		error = null;
		try {
			// The activity aggregate is the spine; market and confirmed-markup
			// context are best effort and may degrade without blanking it.
			const [harvest, markets, realised] = await Promise.all([
				getAnalyticsHarvest(period),
				getMarketHarvestMarkups().catch(() => null),
				getHarvestRealisedMarkup().catch(() => []),
			]);
			if (epoch !== loadEpoch) return;
			data = harvest;
			market = markets;
			realisedByTier = new Map(realised.map((row) => [row.yieldTier, row.netMarkup]));
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load tree cutting data');
		} finally {
			if (epoch === loadEpoch) loading = false;
		}
	}

	const sections = $derived.by<TreeCuttingSection[]>(() => {
		if (!data) return [];
		const marketByItem = new Map((market?.items ?? []).map((item) => [item.itemName, item]));
		// Ordered by cycled volume (busiest first), with the diagnostic
		// Unclassified bucket kept after the three attributable activities.
		// This is also the fallback selection order.
		return data.tierComparisons
			.map((tier) =>
				toSection(
					tier,
					market,
					marketByItem,
					confidenceMode,
					realisedByTier.get(tier.yieldTier) ?? 0,
				),
			)
			.sort(
				(a, b) =>
					Number(a.yieldTier === 'unknown') - Number(b.yieldTier === 'unknown') ||
					b.cycled - a.cycled ||
					treeCuttingActivityName(a).localeCompare(treeCuttingActivityName(b)),
			);
	});

	const activityTable = createTableModel<TreeCuttingSection>({
		rows: () => sections,
		pageSize: Number.MAX_SAFE_INTEGER,
		searchText: (row) => [treeCuttingActivityName(row)],
		initialSort: { key: 'cycled', dir: 'desc' },
		defaultSortDirs: {
			yieldTier: 'asc',
			cycled: 'desc',
			realisedRate: 'desc',
			muRate: 'desc',
		},
		comparators: {
			yieldTier: (a, b) => treeCuttingActivityName(a).localeCompare(treeCuttingActivityName(b)),
		},
	});

	/** The selected board activity, or Overall when no current tier resolves. */
	const selectedSection = $derived.by<TreeCuttingSection | null>(() => {
		if (selectedTier === null) return null;
		return sections.find((s) => s.yieldTier === selectedTier) ?? null;
	});

	const overall = $derived.by<TreeCuttingOverall | null>(() => {
		if (sections.length === 0) return null;
		const cycled = sections.reduce((sum, s) => sum + s.cycled, 0);
		const returns = sections.reduce((sum, s) => sum + s.returns, 0);
		const lootRate = cycled > 0 ? returns / cycled : 0;
		// Market figures aggregate only when at least one section carries
		// them; a section without market context contributes nothing.
		const anyMarket = sections.some((s) => s.muProjectedReturns !== null);
		const muProjectedReturns = anyMarket
			? sections.reduce((sum, s) => sum + (s.muProjectedReturns ?? 0), 0)
			: null;
		const muRate = muProjectedReturns !== null && cycled > 0 ? muProjectedReturns / cycled : null;
		const realisedMarkup = sections.reduce((sum, s) => sum + s.realisedMarkup, 0);
		const realisedReturns = returns + realisedMarkup;
		return {
			cycled,
			returns,
			lootRate,
			muProjectedReturns,
			muRate,
			realisedReturns,
			realisedRate: cycled > 0 ? realisedReturns / cycled : 0,
			realisedMarkup,
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
		get sections() {
			return sections;
		},
		get activityTable() {
			return activityTable;
		},
		get selectedSection() {
			return selectedSection;
		},
		selectSection(yieldTier: HarvestYieldTier | null) {
			selectedTier = yieldTier;
		},
		loadData,
	};
}

export type TreeCuttingModel = ReturnType<typeof createTreeCuttingModel>;
