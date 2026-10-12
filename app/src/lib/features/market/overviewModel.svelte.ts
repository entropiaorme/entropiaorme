/**
 * Overview-tab view model: every observed item's latest readings on a
 * selected aggregation horizon, with the observation age as the
 * staleness signal. Presentation lives in the tab component.
 */

import type { MarketHorizon, MarketOverviewRow, MarketReading } from '$lib/api';
import { getMarketOverview } from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { filterByQuery } from '$lib/view/search';

export const HORIZONS: { id: MarketHorizon; label: string }[] = [
	{ id: 'day', label: 'Day' },
	{ id: 'week', label: 'Week' },
	{ id: 'month', label: 'Month' },
	{ id: 'year', label: 'Year' },
	{ id: 'decade', label: 'Decade' },
];

/** One table row: the selected horizon's reading flattened beside the
 * item identity and the staleness signal. */
export interface OverviewTableRow {
	itemName: string;
	tier: number;
	markupPct: number | null;
	salesPed: number;
	observedAt: number;
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

export type OverviewSortKey = keyof OverviewTableRow & string;

export function createOverviewModel() {
	let rows = $state<MarketOverviewRow[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let horizon = $state<MarketHorizon>('week');
	let search = $state('');
	let sortKey = $state<OverviewSortKey>('itemName');
	let sortDir = $state<'asc' | 'desc'>('asc');

	let loadEpoch = 0;

	async function loadData() {
		const epoch = ++loadEpoch;
		loading = true;
		error = null;
		try {
			const loaded = await getMarketOverview();
			if (epoch !== loadEpoch) return;
			rows = loaded;
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load market data');
		} finally {
			if (epoch === loadEpoch) loading = false;
		}
	}

	function readingFor(row: MarketOverviewRow): MarketReading {
		switch (horizon) {
			case 'day':
				return row.day;
			case 'week':
				return row.week;
			case 'month':
				return row.month;
			case 'year':
				return row.year;
			case 'decade':
				return row.decade;
		}
	}

	const tableRows = $derived.by<OverviewTableRow[]>(() =>
		rows.map((row) => {
			const reading = readingFor(row);
			return {
				itemName: row.itemName,
				tier: row.tier,
				markupPct: reading.markupPct,
				salesPed: reading.salesPed,
				observedAt: row.observedAt,
			};
		}),
	);

	const itemNames = $derived(rows.map((row) => row.itemName));

	// Search-filtered, sorted view of the flattened rows. Null markups
	// (no sales in the horizon) sort last in both directions.
	const sortedRows = $derived.by<OverviewTableRow[]>(() => {
		const filtered = filterByQuery(tableRows, search, (row) => row.itemName);
		const dir = sortDir === 'asc' ? 1 : -1;
		const key = sortKey;
		return [...filtered].sort((a, b) => {
			const aVal = a[key];
			const bVal = b[key];
			if (aVal == null && bVal == null) return 0;
			if (aVal == null) return 1;
			if (bVal == null) return -1;
			if (typeof aVal === 'number' && typeof bVal === 'number') return dir * (aVal - bVal);
			return dir * String(aVal).localeCompare(String(bVal));
		});
	});

	return {
		get rows() {
			return rows;
		},
		get tableRows() {
			return tableRows;
		},
		get sortedRows() {
			return sortedRows;
		},
		get search() {
			return search;
		},
		set search(value: string) {
			search = value;
		},
		get sortKey() {
			return sortKey;
		},
		set sortKey(value: OverviewSortKey) {
			sortKey = value;
		},
		get sortDir() {
			return sortDir;
		},
		set sortDir(value: 'asc' | 'desc') {
			sortDir = value;
		},
		get itemNames() {
			return itemNames;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get horizon() {
			return horizon;
		},
		set horizon(value: MarketHorizon) {
			horizon = value;
		},
		loadData,
	};
}

export type OverviewModel = ReturnType<typeof createOverviewModel>;
