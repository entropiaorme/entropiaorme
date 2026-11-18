/**
 * The live session's loot valued at market markup: the one place the
 * dashboard and the overlay read the session's loot composition and the
 * markup it is worth, so the Loot Composition widget, the Loot MU and Loot
 * MU Rate stats, and the expected figure built on them can never disagree.
 * The quest rewards confirmed on the session are read and valued here too,
 * apart from the loot: a reward is never loot.
 *
 * Reads are event-driven, never polled. The loot composition is re-read
 * when the session's loot moves (its Loot TT, kills, or swings change in
 * the tracking snapshot; a hand-in moves both, as the reward's clump leaves
 * the loot and the kills); the market signals are re-read when the set of
 * looted items grows or any market write lands (`market:updated`). A read
 * already in flight is never doubled: a change during it re-reads once it
 * settles.
 *
 * The valuation is the projection Analytics and Inventory use, live: the
 * player's manual markup where it is the more recent reading, the
 * confidence threshold chosen for the live figures, the Nanocube floor
 * for unsupported or untrusted markets, Shrapnel at its fixed conversion,
 * and the live-only Nanocube marks. Every figure here is an estimate,
 * never realised P&L.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { untrack } from 'svelte';
import {
	getMarketItemMarkups,
	getSessionDetail,
	type LootEntry,
	MARKET_TOPIC,
	type MarketHarvestData,
	type MarketHarvestItem,
	type SessionQuestReward,
	type SessionQuestRewardItem,
	type TrackingSnapshot,
} from '$lib/api';
import { projectRewardValue } from '$lib/features/analytics/rewardValue';
import {
	type ConfidenceMode,
	projectLoot,
	type TreeCuttingItem,
} from '$lib/features/analytics/treeCuttingModel.svelte';

/** What one read of the session gives: its loot, and its quest rewards
 * confirmed so far. */
export interface SessionComposition {
	loot: LootEntry[];
	rewards: SessionQuestReward[];
}

export interface LiveLootValuationPorts {
	readSession: (sessionId: string) => Promise<SessionComposition>;
	readMarket: (names: string[]) => Promise<MarketHarvestData>;
	followMarket: (onChange: () => void) => Promise<UnlistenFn>;
}

const DEFAULT_PORTS: LiveLootValuationPorts = {
	readSession: async (sessionId) => {
		const detail = await getSessionDetail(sessionId);
		return { loot: detail.lootBreakdown, rewards: detail.questRewards };
	},
	readMarket: (names) => getMarketItemMarkups(names),
	followMarket: (onChange) => listen(MARKET_TOPIC, onChange),
};

/** How much of the session's loot TT each valuation route covers. */
export interface ValuationCoverage {
	/** Valued at its own market reading. */
	marketTt: number;
	/** Valued at the player's own markup. */
	manualTt: number;
	/** Valued at the Nanocube floor: no reading, an unsupported market, or
	 * one below the live confidence threshold. */
	flooredTt: number;
	/** Valued at Nanocube because the player marked it. */
	markedTt: number;
	/** Shrapnel at its fixed conversion. */
	shrapnelTt: number;
	/** The market value of items with no TT (valued only by the player's
	 * per-unit entry): it adds to the loot's value with no TT beneath it. */
	noTtValuePed: number;
}

/** The quest rewards confirmed on a session or a family, valued. */
export interface RewardValuation {
	/** Every line, as recorded. */
	lines: SessionQuestRewardItem[];
	/** The reward at TT: liquid lines at face value, items at TT. */
	ttPed: number;
	/** The reward at market: liquid lines at face value, items through the
	 * one reward projection. */
	muPed: number;
}

/** The live valuation's figures, for the stats and the widget. */
export interface LiveValuationView {
	sessionId: string;
	items: TreeCuttingItem[];
	lootTt: number;
	/** The loot's estimated market value. */
	marketValuePed: number;
	/** Market value over loot TT, 100%-anchored: 1.04 reads 104%. Null with
	 * no loot TT. */
	lootMuFactor: number | null;
	coverage: ValuationCoverage;
	confidence: ConfidenceMode;
	/** The quest rewards confirmed so far, or null while none has been. */
	reward: RewardValuation | null;
}

/** Every line of the rewards, in the order they were paid. */
export function rewardLines(rewards: readonly SessionQuestReward[]): SessionQuestRewardItem[] {
	return rewards.flatMap((reward) => reward.items);
}

/** Value reward lines through the one reward projection; null with none. */
export function valueRewards(
	lines: readonly SessionQuestRewardItem[],
	market: MarketHarvestData,
	marketByItem: Map<string, MarketHarvestItem>,
	confidence: ConfidenceMode,
): RewardValuation | null {
	if (lines.length === 0) return null;
	const ttPed = lines.reduce((sum, line) => sum + line.valuePed, 0);
	const muPed = projectRewardValue(lines, market, marketByItem, confidence) ?? ttPed;
	return { lines: [...lines], ttPed, muPed };
}

function coverageOf(items: readonly TreeCuttingItem[]): ValuationCoverage {
	const coverage: ValuationCoverage = {
		marketTt: 0,
		manualTt: 0,
		flooredTt: 0,
		markedTt: 0,
		shrapnelTt: 0,
		noTtValuePed: 0,
	};
	for (const item of items) {
		if (item.ttValue <= 0) coverage.noTtValuePed += item.marketValuePed;
		switch (item.markupBasis) {
			case 'market':
				coverage.marketTt += item.ttValue;
				break;
			case 'manual':
				coverage.manualTt += item.ttValue;
				break;
			case 'nanocube':
				coverage.flooredTt += item.ttValue;
				break;
			case 'nanocube_mark':
				coverage.markedTt += item.ttValue;
				break;
			case 'shrapnel_conversion':
				coverage.shrapnelTt += item.ttValue;
				break;
		}
	}
	return coverage;
}

/**
 * Value the session's loot live. Pure over its inputs, so the figures are
 * testable without the reads: null while the composition or the market
 * signals are not in hand (a failed market read is a figure the app cannot
 * give, not one worth 100%).
 */
export function valueLiveLoot(
	sessionId: string,
	loot: readonly LootEntry[],
	market: MarketHarvestData | null,
	confidence: ConfidenceMode,
	options: { live: boolean } = { live: true },
	rewards: readonly SessionQuestRewardItem[] = [],
): LiveValuationView | null {
	if (!market) return null;
	const marketByItem = new Map<string, MarketHarvestItem>(
		market.items.map((item) => [item.itemName, item]),
	);
	const lootItems = loot.map((entry) => ({
		itemName: entry.name,
		quantity: entry.quantity,
		valuePed: entry.ttValue,
	}));
	const projection = projectLoot(lootItems, 0, market, marketByItem, confidence, options);
	const lootTt = lootItems.reduce((sum, item) => sum + item.valuePed, 0);
	const marketValuePed = projection.muProjectedReturns ?? 0;
	return {
		sessionId,
		items: projection.items,
		lootTt,
		marketValuePed,
		lootMuFactor: lootTt > 0 ? marketValuePed / lootTt : null,
		coverage: coverageOf(projection.items),
		confidence,
		reward: valueRewards(rewards, market, marketByItem, confidence),
	};
}

/** What the snapshot says about the loot and the rewards: when it
 * changes, the composition behind it has moved. A completion is its own
 * signal, since a reward can arrive with no kill, and the kill a reward's
 * clump makes can land before the completion that claims it. */
function lootKey(status: TrackingSnapshot | null): string | null {
	if (status?.status !== 'active' || !status.session_id) return null;
	const completions = (status.recentEvents ?? [])
		.filter((event) => event.eventType === 'quest_completed')
		.map((event) => event.timestamp ?? '')
		.join(',');
	return [
		status.session_id,
		status.returns ?? 0,
		status.kill_count ?? 0,
		status.harvestSwings ?? 0,
		completions,
	].join('|');
}

export function createLiveLootValuation(
	confidence: () => ConfidenceMode,
	ports: LiveLootValuationPorts = DEFAULT_PORTS,
) {
	let sessionId = $state<string | null>(null);
	let loot = $state<LootEntry[] | null>(null);
	let rewards = $state<SessionQuestReward[]>([]);
	let market = $state<MarketHarvestData | null>(null);
	let marketFailed = $state(false);

	let lastLootKey: string | null = null;
	let lootInFlight = false;
	let lootStale = false;
	let lastMarketNames = '';
	let marketInFlight = false;
	let marketStale = false;

	async function readLoot(): Promise<void> {
		const id = sessionId;
		if (!id) return;
		if (lootInFlight) {
			lootStale = true;
			return;
		}
		lootInFlight = true;
		try {
			const composition = await ports.readSession(id);
			if (sessionId === id) {
				loot = composition.loot;
				rewards = composition.rewards;
			}
		} catch {
			// The next loot change re-reads; the last good composition stands.
		} finally {
			lootInFlight = false;
		}
		if (lootStale) {
			lootStale = false;
			await readLoot();
		} else {
			await readMarketIfNew();
		}
	}

	function marketNames(): string[] {
		return [
			...new Set([
				...(loot ?? []).map((entry) => entry.name),
				...rewardLines(rewards).map((line) => line.itemName),
			]),
		].sort();
	}

	async function readMarket(): Promise<void> {
		const names = marketNames();
		if (names.length === 0) return;
		if (marketInFlight) {
			marketStale = true;
			return;
		}
		marketInFlight = true;
		const id = sessionId;
		try {
			const data = await ports.readMarket(names);
			if (sessionId === id) {
				market = data;
				marketFailed = false;
				lastMarketNames = names.join('\n');
			}
		} catch {
			if (sessionId === id) marketFailed = true;
		} finally {
			marketInFlight = false;
		}
		if (marketStale) {
			marketStale = false;
			await readMarket();
		}
	}

	async function readMarketIfNew(): Promise<void> {
		if (marketNames().join('\n') !== lastMarketNames || marketFailed) await readMarket();
	}

	/**
	 * Follow the tracking snapshot: a new session starts afresh, an idle
	 * one clears, and a change to the loot re-reads the composition.
	 */
	function sync(status: TrackingSnapshot | null): void {
		const key = lootKey(status);
		if (key === lastLootKey) return;
		lastLootKey = key;
		const id = key === null ? null : (status?.session_id ?? null);
		if (id !== sessionId) {
			sessionId = id;
			loot = null;
			rewards = [];
			market = null;
			marketFailed = false;
			lastMarketNames = '';
		}
		if (id) void readLoot();
	}

	/** Re-read the market signals whatever changed: a market write landed. */
	function marketChanged(): void {
		if (sessionId) void readMarket();
	}

	/** Follow market writes from any window; returns the detach function. */
	function subscribe(): Promise<UnlistenFn> {
		return ports.followMarket(marketChanged);
	}

	const view = $derived(
		sessionId && loot
			? valueLiveLoot(sessionId, loot, market, confidence(), { live: true }, rewardLines(rewards))
			: null,
	);

	return {
		sync,
		subscribe,
		marketChanged,
		/** The valued composition, or null while it cannot be given. */
		get view(): LiveValuationView | null {
			return view;
		},
		/** The session's loot composition, valued or not. */
		get loot(): LootEntry[] | null {
			return loot;
		},
		/** The quest rewards confirmed on the session so far. */
		get rewards(): SessionQuestReward[] {
			return rewards;
		},
		/** Every line of those rewards, as the family valuation folds them. */
		get rewardItems(): SessionQuestRewardItem[] {
			return rewardLines(rewards);
		},
		/** True until the first composition read for the session answers. */
		get loading(): boolean {
			return sessionId !== null && loot === null;
		},
		/** The market signals could not be read: markup figures are unavailable. */
		get marketFailed(): boolean {
			return marketFailed;
		},
		/** The raw market signals for the session's items (the widget's
		 * markup editor reads an item's current entry from them). */
		get market(): MarketHarvestData | null {
			return market;
		},
	};
}

export type LiveLootValuation = ReturnType<typeof createLiveLootValuation>;

/**
 * Create the live valuation for a surface and keep it in step: it follows
 * the surface's tracking snapshot and every market write for as long as the
 * surface lives. Call during component initialisation.
 */
export function attachLiveLootValuation(
	status: () => TrackingSnapshot | null,
	confidence: () => ConfidenceMode,
): LiveLootValuation {
	const valuation = createLiveLootValuation(confidence);
	$effect(() => {
		const current = status();
		untrack(() => valuation.sync(current));
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
