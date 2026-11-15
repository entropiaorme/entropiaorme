/**
 * A quest reward valued at market: the one valuation Analytics, the Quests
 * tab, and the live session figures share, so a reward reads the same
 * wherever it shows. Pure over its inputs.
 */

import type { MarketHarvestData, MarketHarvestItem } from '$lib/api';
import {
	type ConfidenceMode,
	effectiveMarkup,
	manualValuation,
	marketOpportunity,
	NANOCUBE_FALLBACK_MARKUP,
} from './treeCuttingModel.svelte';

/** One reward line as recorded: its TT value in PED. */
export interface RewardLine {
	itemName: string;
	quantity: number;
	valuePed: number;
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
	items: readonly RewardLine[],
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
	items: readonly RewardLine[],
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
