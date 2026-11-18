/**
 * The Market tab's item list: every observed item's latest readings,
 * one resolved markup per row, and an expandable detail carrying all
 * five horizons and the item's history. Presentation lives in the page.
 */

import type { MarketHistoryPoint, MarketHorizon, MarketOverviewRow } from '$lib/api';
import { getMarketItemHistory, getMarketOverview } from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { filterByQuery } from '$lib/view/search';

export const HORIZONS: { id: MarketHorizon; label: string }[] = [
	{ id: 'day', label: 'Day' },
	{ id: 'week', label: 'Week' },
	{ id: 'month', label: 'Month' },
	{ id: 'year', label: 'Year' },
	{ id: 'decade', label: 'Decade' },
];

/** The horizons a row's markup resolves through, in preference order:
 * the same week, then month, then year fallback the analytics apply. */
const RESOLUTION_ORDER: MarketHorizon[] = ['week', 'month', 'year'];

/** One row's resolved reading: the first horizon in the resolution order
 * that carries a markup, else the week with no markup (no recent sales). */
export interface ResolvedReading {
	horizon: MarketHorizon;
	markupPct: number | null;
	salesPed: number;
}

export function resolveReading(row: MarketOverviewRow): ResolvedReading {
	for (const horizon of RESOLUTION_ORDER) {
		const reading = row[horizon];
		if (reading.markupPct !== null) {
			return { horizon, markupPct: reading.markupPct, salesPed: reading.salesPed };
		}
	}
	return { horizon: 'week', markupPct: null, salesPed: row.week.salesPed };
}

/** Compact volume label: `13.5K PED`, `6.4M PED`, `451.90 PED`. */
export function formatSalesPed(value: number): string {
	if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M PED`;
	if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K PED`;
	return `${value.toFixed(2)} PED`;
}

/** Observation age in whole days against a supplied now (epoch seconds). */
export function ageDays(observedAt: number, nowEpoch: number): number {
	return Math.max(0, Math.floor((nowEpoch - observedAt) / 86_400));
}

/** Staleness label: `today`, `1d ago`, `3w ago`. */
export function formatAge(observedAt: number, nowEpoch: number): string {
	const days = ageDays(observedAt, nowEpoch);
	if (days === 0) return 'today';
	if (days < 14) return `${days}d ago`;
	return `${Math.floor(days / 7)}w ago`;
}

export interface MarketItemRow {
	row: MarketOverviewRow;
	resolved: ResolvedReading;
}

export function createOverviewModel() {
	let rows = $state<MarketOverviewRow[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let search = $state('');

	let expandedItem = $state<string | null>(null);
	let history = $state<MarketHistoryPoint[]>([]);
	let historyLoading = $state(false);
	let historyError = $state<string | null>(null);

	let loadEpoch = 0;
	let historyEpoch = 0;

	async function loadData() {
		const epoch = ++loadEpoch;
		loading = true;
		error = null;
		try {
			const loaded = await getMarketOverview();
			if (epoch !== loadEpoch) return;
			rows = loaded;
			if (expandedItem !== null && !loaded.some((row) => row.itemName === expandedItem)) {
				collapse();
			}
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load market data');
		} finally {
			if (epoch === loadEpoch) loading = false;
		}
	}

	const items = $derived<MarketItemRow[]>(
		rows.map((row) => ({ row, resolved: resolveReading(row) })),
	);

	// The backend serves rows sorted by item name; the search keeps that order.
	const visibleItems = $derived(filterByQuery(items, search, (item) => item.row.itemName));

	function collapse() {
		historyEpoch++;
		expandedItem = null;
		history = [];
		historyLoading = false;
		historyError = null;
	}

	/** Expands an item (collapsing any other) and loads its history on the
	 * row's resolved horizon; toggling the open item collapses it. */
	async function toggle(itemName: string) {
		if (expandedItem === itemName) {
			collapse();
			return;
		}
		const item = items.find((candidate) => candidate.row.itemName === itemName);
		if (!item) return;
		const epoch = ++historyEpoch;
		expandedItem = itemName;
		history = [];
		historyError = null;
		historyLoading = true;
		try {
			const loaded = await getMarketItemHistory(itemName, item.resolved.horizon);
			if (epoch !== historyEpoch) return;
			// Newest first: the detail reads as a recent log.
			history = [...loaded].reverse();
		} catch (e) {
			if (epoch !== historyEpoch) return;
			historyError = describeError(e, 'Failed to load the item history');
		} finally {
			if (epoch === historyEpoch) historyLoading = false;
		}
	}

	return {
		get items() {
			return items;
		},
		get visibleItems() {
			return visibleItems;
		},
		get search() {
			return search;
		},
		set search(value: string) {
			search = value;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get expandedItem() {
			return expandedItem;
		},
		get history() {
			return history;
		},
		get historyLoading() {
			return historyLoading;
		},
		get historyError() {
			return historyError;
		},
		loadData,
		toggle,
	};
}

export type OverviewModel = ReturnType<typeof createOverviewModel>;
