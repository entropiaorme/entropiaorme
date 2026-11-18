/**
 * The market-confidence copy shared by the activity item-composition
 * tables (Tree Cutting sub-activities, Hunting sessions and mobs): one source for
 * the confidence titles, the applied-markup labels, and the evidence
 * tooltip, so the two tabs explain the same model in the same words.
 */

import type { MarketManualMarkup } from '$lib/api';
import { formatAge } from '$lib/features/market/overviewModel.svelte';
import { formatPed, formatPercent } from '$lib/utils/format';
import type { MarkupBasis, TreeCuttingItem } from './treeCuttingModel.svelte';

/**
 * How a row's applied markup reads. When the player's own value is the one
 * in use, it reads in the form they entered it: a per-unit entry as `+0.70
 * ea` (whatever the item's TT, and the only reading an item with no TT has),
 * a percentage as a percentage. Everything else reads as a percentage of TT.
 */
export function appliedMarkupText(
	markupPct: number,
	basis: MarkupBasis | null,
	entry: MarketManualMarkup | null | undefined,
): string {
	if (basis === 'manual' && entry?.kind === 'ped_per_unit') return `+${formatPed(entry.value)} ea`;
	return formatPercent(markupPct / 100);
}

/** A manual entry as the player wrote it: `130.0%` or `+2.50 PED per unit`. */
export function manualEntryText(entry: MarketManualMarkup): string {
	return entry.kind === 'percent'
		? formatPercent(entry.value / 100)
		: `+${formatPed(entry.value)} PED per unit`;
}

/** When the entry was made, against now (epoch seconds). */
export function manualEntryAge(entry: MarketManualMarkup, nowEpoch = Date.now() / 1000): string {
	return formatAge(entry.recordedAt, nowEpoch);
}

/** The label for a row valued at the player's own markup: the entry as
 * written, with its percentage of TT beside a per-unit entry where the item
 * has TT. */
export function manualMarkupLabel(
	entry: MarketManualMarkup | null | undefined,
	effectiveMarkupPct: number,
	ttValue: number,
): string {
	if (entry?.kind !== 'ped_per_unit')
		return `Your markup ${formatPercent(effectiveMarkupPct / 100)}`;
	const each = `Your value +${formatPed(entry.value)} PED each`;
	return ttValue > 0
		? `${each} (${formatPercent(effectiveMarkupPct / 100)} of TT)`
		: `${each}; the item has no TT`;
}

/** The player's own markup: trusted as given, but a stand-in that a newer
 * market import replaces. */
export function manualMarkupTip(
	entry: MarketManualMarkup,
	nowEpoch = Date.now() / 1000,
): { title: string; subtitle: string; note: string } {
	return {
		title: 'Your markup',
		subtitle: `You entered ${manualEntryText(entry)} (${manualEntryAge(entry, nowEpoch)}). It is used wherever it is newer than the item's market import, whatever the confidence setting.`,
		note: 'A newer market import replaces it. Where the item trades on the market, an import is the firmer reading.',
	};
}

/** The live-only Nanocube mark. */
export function nanocubeMarkTip(effectiveMarkupPct: number): {
	title: string;
	subtitle: string;
	note: string;
} {
	return {
		title: 'Marked as Nanocube',
		subtitle: `You marked this item to be valued at the ${formatPercent(effectiveMarkupPct / 100)} Nanocube markup in the live session figures, whatever its market reading says.`,
		note: 'Inventory and Analytics still use its market reading. The mark stands until you lift it or enter your own markup.',
	};
}

export const marketPeriod = (horizon: string) =>
	horizon === 'week' || horizon === 'month' || horizon === 'year'
		? `last ${horizon}`
		: `over the last ${horizon}`;

export const marketShare = (value: number) => {
	const percent = value * 100;
	return percent < 0.1 ? 'less than 0.1%' : `${percent.toFixed(1)}%`;
};

export const confidenceTitle = (tier: TreeCuttingItem['tier']) => {
	if (tier === 'liquid')
		return 'High markup confidence: This markup should be practical to realise';
	if (tier === 'middling') {
		return 'Medium markup confidence: It may be difficult to realise this markup';
	}
	return 'Low markup confidence: Do not rely on realising this markup';
};

export const shrapnelMarkupLabel = (
	observedMarkupPct: number | null,
	effectiveMarkupPct: number,
) => {
	if (observedMarkupPct !== null) {
		return `Last seen market markup ${formatPercent(observedMarkupPct / 100)}; projections use the fixed ${formatPercent(effectiveMarkupPct / 100)} Shrapnel conversion value`;
	}
	return `Projections use the fixed ${formatPercent(effectiveMarkupPct / 100)} Shrapnel conversion value`;
};

export function shrapnelConversionTip(
	observedMarkupPct: number | null,
	effectiveMarkupPct: number,
): { title: string; subtitle: string; note: string } {
	const observed =
		observedMarkupPct === null
			? ''
			: `The last seen market markup on Shrapnel was ${formatPercent(observedMarkupPct / 100)}. `;
	return {
		title: 'Fixed Shrapnel conversion value',
		subtitle: `${observed}EntropiaOrme uses ${formatPercent(effectiveMarkupPct / 100)} as a fixed value for Loot MU and expected-return projections, representing conversion to Universal Ammo.`,
		note: 'This remains projected value until the conversion is recorded. The 1% gain then enters Realised Net.',
	};
}

export const markupLabel = (item: TreeCuttingItem) => {
	if (item.markupBasis === 'shrapnel_conversion') {
		return shrapnelMarkupLabel(item.ownMarkupPct, item.effectiveMarkupPct);
	}
	if (item.markupBasis === 'manual')
		return manualMarkupLabel(item.manual, item.effectiveMarkupPct, item.ttValue);
	if (item.markupBasis === 'nanocube_mark') {
		return `Marked as Nanocube: live figures use ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube markup`;
	}
	if (item.floored && item.ownMarkupPct !== null) {
		return `Observed markup ${formatPercent(item.ownMarkupPct / 100)}; projections use ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube markup`;
	}
	if (item.ownMarkupPct == null) {
		return `Projections use ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube markup`;
	}
	return `Markup ${formatPercent(item.effectiveMarkupPct / 100)}`;
};

/** The evidence tooltip for one composition item: what sold, at what
 * markup, over which horizon, and the fee-efficient worked example. */
export function confidenceTip(item: TreeCuttingItem): {
	title: string;
	subtitle: string;
	example?: string;
	note?: string;
} {
	if (item.markupBasis === 'shrapnel_conversion') {
		return shrapnelConversionTip(item.ownMarkupPct, item.effectiveMarkupPct);
	}
	if (item.markupBasis === 'manual' && item.manual) {
		return manualMarkupTip(item.manual);
	}
	if (item.markupBasis === 'nanocube_mark') {
		return nanocubeMarkTip(item.effectiveMarkupPct);
	}
	const projectionNote = item.floored
		? `With the current confidence setting, MU projections use the ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube MU instead.`
		: undefined;
	if (item.ownMarkupPct == null) {
		return {
			title: confidenceTitle(item.tier),
			subtitle: 'No market MU is available for this item.',
			note: projectionNote,
		};
	}

	const horizon = item.markupHorizon;
	const salesPed = item.salesPed;
	let lead = 'No recent sales data is available for this item.';
	if (horizon && salesPed !== null) {
		if (horizon === 'week') {
			lead = `${formatPed(salesPed)} PED TT sold last week at ${formatPercent(item.ownMarkupPct / 100)} MU.`;
		} else {
			const weekly = item.weeklySalesPed;
			const weeklyReading =
				weekly == null || weekly <= 0
					? 'No sales in the last week.'
					: `${formatPed(weekly)} PED TT sold last week.`;
			lead = `${weeklyReading} The current ${formatPercent(item.ownMarkupPct / 100)} MU comes from ${formatPed(salesPed)} PED TT sold ${marketPeriod(horizon)}.`;
		}
	}

	const batchTt = item.opportunity.efficientBatchTt;
	const batchShare = item.opportunity.efficientBatchMarketShare;
	const batchMarkup = batchTt === null ? null : batchTt * Math.max(0, item.ownMarkupPct / 100 - 1);
	const example =
		batchTt !== null && batchMarkup !== null && batchShare !== null && horizon
			? `For example: A ${formatPed(batchTt)} PED TT sale at this MU would produce about ${formatPed(batchMarkup)} PED of markup. The minimum auction fee is 0.5 PED, or 10% of that markup. That sale would be ${marketShare(batchShare)} of the TT sold ${marketPeriod(horizon)}.`
			: undefined;
	const insufficientMarkup = batchTt === null || batchMarkup === null;
	const noExampleNote = example
		? projectionNote
		: [
				insufficientMarkup
					? 'The recorded MU does not provide enough markup to calculate a sale after fees.'
					: undefined,
				projectionNote,
			]
				.filter(Boolean)
				.join(' ');
	return {
		title: confidenceTitle(item.tier),
		subtitle: lead,
		example,
		note: noExampleNote || undefined,
	};
}
