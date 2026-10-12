/**
 * Shared-tab view model: the centrally aggregated market snapshot the
 * app fetches from market-data.entropiaorme.com (download gated by its
 * opt-in), plus the explicit contribute-latest-paste action (gated by
 * its own opt-in and token). Presentation lives in the tab component.
 *
 * Snapshot markup is the same informational layer as local pastes:
 * estimates never enter the ledger or any realised figure.
 */

import type { MarketHorizon } from '$lib/api';
import { getMarketContributionBatch } from '$lib/api';
import {
	marketContributionOptIn,
	marketContributorToken,
	marketDataOptIn,
	marketSnapshotCache,
	type SnapshotItem,
	type SnapshotReading,
} from '$lib/marketData.svelte';
import { contributeMarketBatch, fetchMarketSnapshot } from '$lib/marketDataFetch';
import { filterByQuery } from '$lib/view/search';

/** One table row: the selected horizon's reading flattened beside the
 * item identity and the staleness signal (epoch seconds). */
export interface SharedTableRow {
	itemName: string;
	tier: number;
	markupPct: number | null;
	salesPed: number;
	observedAt: number;
}

/** ISO timestamp to epoch seconds; NaN-safe (0 sorts oldest). */
export function isoToEpoch(iso: string): number {
	const ms = Date.parse(iso);
	return Number.isNaN(ms) ? 0 : ms / 1000;
}

function readingFor(item: SnapshotItem, horizon: MarketHorizon): SnapshotReading {
	return item.readings[horizon];
}

export function createSharedModel() {
	let horizon = $state<MarketHorizon>('week');
	let search = $state('');
	let refreshing = $state(false);
	let refreshError = $state<string | null>(null);
	let contributing = $state(false);
	let contributionNote = $state<string | null>(null);

	const cache = $derived(marketSnapshotCache.current);

	const tableRows = $derived.by<SharedTableRow[]>(() => {
		const snapshot = cache;
		if (!snapshot) return [];
		return filterByQuery(snapshot.items, search, (item) => item.itemName).map((item) => {
			const reading = readingFor(item, horizon);
			return {
				itemName: item.itemName,
				tier: item.tier,
				markupPct: reading.markupPct,
				salesPed: reading.salesPed,
				observedAt: isoToEpoch(item.observedAt),
			};
		});
	});

	async function refresh(): Promise<void> {
		refreshing = true;
		refreshError = null;
		const result = await fetchMarketSnapshot();
		if (!result.ok) {
			refreshError = result.reason;
		}
		refreshing = false;
	}

	/** Send the latest accepted paste to the shared service. Explicit
	 * user action only; both consent gates re-checked downstream. */
	async function contributeLatest(): Promise<void> {
		contributing = true;
		contributionNote = null;
		try {
			const batch = await getMarketContributionBatch();
			if (!batch || batch.items.length === 0) {
				contributionNote = 'Nothing to send: accept a market paste first.';
				return;
			}
			const result = await contributeMarketBatch(batch);
			contributionNote = result.ok
				? `Sent ${result.stored} item${result.stored === 1 ? '' : 's'}.`
				: `Not sent: ${result.reason}.`;
		} catch (e) {
			contributionNote = `Not sent: ${e instanceof Error ? e.message : String(e)}.`;
		} finally {
			contributing = false;
		}
	}

	return {
		get cache() {
			return cache;
		},
		get fetchEnabled() {
			return marketDataOptIn.current;
		},
		get contributionEnabled() {
			return marketContributionOptIn.current && marketContributorToken.current.trim() !== '';
		},
		get tableRows() {
			return tableRows;
		},
		get horizon() {
			return horizon;
		},
		set horizon(value: MarketHorizon) {
			horizon = value;
		},
		get search() {
			return search;
		},
		set search(value: string) {
			search = value;
		},
		get refreshing() {
			return refreshing;
		},
		get refreshError() {
			return refreshError;
		},
		get contributing() {
			return contributing;
		},
		get contributionNote() {
			return contributionNote;
		},
		refresh,
		contributeLatest,
	};
}

export type SharedModel = ReturnType<typeof createSharedModel>;
