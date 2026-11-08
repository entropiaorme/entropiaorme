/**
 * A session family's loot valued at market markup: the composition behind
 * the lifetime forms of the markup stats, valued by the same projection the
 * live session's loot is.
 *
 * Only read while one of those figures is on show. The ended instances'
 * composition is read once, and again only when the family's history moves;
 * the running instance's loot arrives from the live valuation, which already
 * follows it, so a kill costs this surface nothing beyond re-adding it.
 *
 * Past loot is valued at today's market, not the market it dropped into. A
 * Nanocube mark stays with the live session figures and is not applied here.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { untrack } from 'svelte';
import {
	getDefinitionLoot,
	getMarketItemMarkups,
	type LootEntry,
	MARKET_TOPIC,
	type MarketHarvestData,
	type TrackingSnapshot,
} from '$lib/api';
import type { ConfidenceMode } from '$lib/features/analytics/treeCuttingModel.svelte';
import { type LiveValuationView, valueLiveLoot } from './liveLootValuation.svelte';

export interface LifetimeLootValuationPorts {
	readFamilyLoot: (definitionId: number) => Promise<LootEntry[]>;
	readMarket: (names: string[]) => Promise<MarketHarvestData>;
	followMarket: (onChange: () => void) => Promise<UnlistenFn>;
}

const DEFAULT_PORTS: LifetimeLootValuationPorts = {
	readFamilyLoot: (definitionId) => getDefinitionLoot(definitionId),
	readMarket: (names) => getMarketItemMarkups(names),
	followMarket: (onChange) => listen(MARKET_TOPIC, onChange),
};

/** The family's loot with the running instance's added, line by item. */
export function mergeLoot(
	family: readonly LootEntry[],
	running: readonly LootEntry[],
): LootEntry[] {
	const byName = new Map(family.map((entry) => [entry.name, { ...entry }]));
	for (const entry of running) {
		const line = byName.get(entry.name);
		if (line) {
			line.quantity += entry.quantity;
			line.ttValue += entry.ttValue;
		} else {
			byName.set(entry.name, { ...entry });
		}
	}
	return [...byName.values()];
}

/**
 * What has to change for the ended instances' loot to have moved. While a
 * session runs that is only another instance ending, so kills never reach
 * the composition read. Idle, an edit to a past session's loot moves the
 * family's total.
 */
function familyKey(status: TrackingSnapshot | null): string | null {
	const definitionId = status?.sessionDefinitionId;
	const lifetime = status?.lifetime;
	if (!definitionId || !lifetime) return null;
	return status?.status === 'active'
		? `${definitionId}|${lifetime.instanceCount - 1}|running`
		: `${definitionId}|${lifetime.instanceCount}|${lifetime.lootTt}`;
}

export function createLifetimeLootValuation(
	confidence: () => ConfidenceMode,
	runningLoot: () => LootEntry[] | null,
	ports: LifetimeLootValuationPorts = DEFAULT_PORTS,
) {
	let definitionId = $state<string | null>(null);
	let family = $state<LootEntry[] | null>(null);
	let market = $state<MarketHarvestData | null>(null);
	let marketFailed = $state(false);

	const loot = $derived(family === null ? null : mergeLoot(family, runningLoot() ?? []));

	let lastKey: string | null = null;
	let wantedNow = false;
	let familyInFlight = false;
	let familyStale = false;
	let lastMarketNames = '';
	let marketInFlight = false;
	let marketStale = false;

	function marketNames(): string[] {
		return [...new Set((loot ?? []).map((entry) => entry.name))].sort();
	}

	async function readMarket(): Promise<void> {
		const names = marketNames();
		if (names.length === 0) return;
		if (marketInFlight) {
			marketStale = true;
			return;
		}
		marketInFlight = true;
		const id = definitionId;
		try {
			const data = await ports.readMarket(names);
			if (definitionId === id) {
				market = data;
				marketFailed = false;
				lastMarketNames = names.join('\n');
			}
		} catch {
			if (definitionId === id) marketFailed = true;
		} finally {
			marketInFlight = false;
		}
		if (marketStale) {
			marketStale = false;
			await readMarket();
		}
	}

	/** Read the market signals if the set of items grew or the last read failed. */
	async function readMarketIfNew(): Promise<void> {
		if (!wantedNow) return;
		if (marketNames().join('\n') !== lastMarketNames || marketFailed) await readMarket();
	}

	async function readFamily(): Promise<void> {
		const id = definitionId;
		if (!id) return;
		if (familyInFlight) {
			familyStale = true;
			return;
		}
		familyInFlight = true;
		try {
			const entries = await ports.readFamilyLoot(Number(id));
			if (definitionId === id) family = entries;
		} catch {
			// The next change re-reads; the last good composition stands.
		} finally {
			familyInFlight = false;
		}
		if (familyStale) {
			familyStale = false;
			await readFamily();
		} else {
			await readMarketIfNew();
		}
	}

	/**
	 * Follow the tracking snapshot while the figures are on show. Hidden, it
	 * does nothing and forgets its place, so showing them again reads afresh
	 * rather than presenting a market that moved in the meantime.
	 */
	function sync(status: TrackingSnapshot | null, wanted: boolean): void {
		wantedNow = wanted;
		if (!wanted) {
			lastKey = null;
			return;
		}
		const key = familyKey(status);
		if (key === lastKey) return;
		const reappearing = lastKey === null;
		lastKey = key;
		const id = key === null ? null : (status?.sessionDefinitionId ?? null);
		if (id !== definitionId) {
			definitionId = id;
			family = null;
			market = null;
			marketFailed = false;
			lastMarketNames = '';
		}
		// Forgetting the names makes the read after the composition fetch
		// the market again, which a return from hidden always should.
		if (reappearing) lastMarketNames = '';
		if (id) void readFamily();
	}

	/** Re-read the market signals: a market write landed. */
	function marketChanged(): void {
		if (definitionId && wantedNow) void readMarket();
	}

	const view = $derived(
		definitionId && loot
			? valueLiveLoot(definitionId, loot, market, confidence(), { live: false })
			: null,
	);

	return {
		sync,
		marketChanged,
		readMarketIfNew,
		subscribe: () => ports.followMarket(marketChanged),
		/** The family's loot valued, or null while it cannot be given. */
		get view(): LiveValuationView | null {
			return view;
		},
		/** The composition being valued, running instance included. */
		get loot(): LootEntry[] | null {
			return loot;
		},
	};
}

export type LifetimeLootValuation = ReturnType<typeof createLifetimeLootValuation>;

/**
 * Create the family valuation for a surface and keep it in step with the
 * surface's snapshot and every market write. Call during component
 * initialisation.
 *
 * @param wanted Whether this surface is drawing a figure that needs it.
 * @param runningLoot The running instance's loot, from the live valuation.
 */
export function attachLifetimeLootValuation(
	status: () => TrackingSnapshot | null,
	confidence: () => ConfidenceMode,
	wanted: () => boolean,
	runningLoot: () => LootEntry[] | null,
): LifetimeLootValuation {
	const valuation = createLifetimeLootValuation(confidence, runningLoot);
	$effect(() => {
		const current = status();
		const show = wanted();
		untrack(() => valuation.sync(current, show));
	});
	// A running instance can loot something the family never has.
	$effect(() => {
		void valuation.loot;
		untrack(() => void valuation.readMarketIfNew());
	});
	$effect(() => {
		let detach: UnlistenFn | undefined;
		let disposed = false;
		void valuation.subscribe().then((unlisten) => {
			if (disposed) unlisten();
			else detach = unlisten;
		});
		return () => {
			disposed = true;
			detach?.();
		};
	});
	return valuation;
}
