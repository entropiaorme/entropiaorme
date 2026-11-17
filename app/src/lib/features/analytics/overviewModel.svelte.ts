/**
 * Overview-tab view model: the period-scoped overview load, the per-source
 * return/cost configuration, and the derived donut, cumulative-P&L timeline,
 * and monthly rows. Presentation lives in the tab component; it composes
 * over this state.
 */

import { getAnalyticsOverview } from '$lib/api';
import type {
	LossesBreakdown,
	MonthlyEntry,
	OverviewStats,
	TimelineDay,
} from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { type AnalyticsRange, analyticsPeriod, isAnalyticsRange } from './analyticsRange';

// lootTt (gains) and trackingCost (losses) are always on; not in config.
export const PROGRESSION_GAIN_TAGS = new Set(['codex']);

export interface ReturnConfig {
	gainTags: Record<string, boolean>;
	lossTags: Record<string, boolean>;
}

// ── Donut chart geometry ──
export const PIE_R = 50;
export const PIE_C = 2 * Math.PI * PIE_R;

const segmentColors: Record<string, string> = {
	lootTt: '#38bdf8',
	questItemTt: '#34d399',
	item_sale: '#fbbf24',
	quest_reward: '#a78bfa',
	inventory_sale: '#f472b6',
	other: '#fb7185',
};

const tagLabels: Record<string, string> = {
	lootTt: 'TT Loot',
	questItemTt: 'Quest Item TT',
	item_sale: 'Auction Sales',
	quest_reward: 'Quest Rewards',
	inventory_sale: 'Mayhem',
	repair: 'Repairs',
	equipment: 'Equipment',
	other: 'Other',
};

export function labelFor(key: string): string {
	return tagLabels[key] || key.charAt(0).toUpperCase() + key.slice(1).replace(/_/g, ' ');
}

function colorFor(key: string): string {
	return segmentColors[key] || '#94a3b8';
}

/** The sessions' overhead in the window, by category: what they cost
 * beyond performing their activities. A band of the losses of its own,
 * beside Cycled and never inside it. */
export interface OverheadBand {
	total: number;
	categories: { key: OverheadKey; label: string; ped: number }[];
}

type OverheadKey = 'travel' | 'fieldLogistics' | 'unclassified';

const OVERHEAD_CATEGORIES: { key: OverheadKey; label: string }[] = [
	{ key: 'travel', label: 'Travel' },
	{ key: 'fieldLogistics', label: 'Field logistics' },
	{ key: 'unclassified', label: 'Unclassified' },
];

/** The overhead band of a window's losses, its categories in a fixed
 * order and only those holding something; `null` when the window has no
 * overhead. */
export function overheadBand(losses: LossesBreakdown): OverheadBand | null {
	if (losses.overhead.total <= 0) return null;
	return {
		total: losses.overhead.total,
		categories: OVERHEAD_CATEGORIES.map(({ key, label }) => ({
			key,
			label,
			ped: losses.overhead[key],
		})).filter((category) => category.ped > 0),
	};
}

/** The cumulative breakdown's totals, honouring the same tag toggles as
 * the donut, so its Costs are the donut's losses and its Returns the
 * donut's gains. */
export interface BreakdownTotals {
	returns: number;
	costs: number;
}

export interface PieView {
	rate: number;
	gains: number;
	losses: number;
	arcs: {
		label: string;
		ped: number;
		pct: number;
		color: string;
		length: number;
		offset: number;
	}[];
}

export function createOverviewModel() {
	let data = $state<OverviewStats | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let config = $state<ReturnConfig>({ gainTags: {}, lossTags: {} });
	let activeRange = $state<AnalyticsRange>('All Time');
	let showBreakdown = $state(false);

	function initConfig(stats: OverviewStats) {
		const gainTags: Record<string, boolean> = {};
		for (const tag of Object.keys(stats.returnsBreakdown.ledger)) {
			if (PROGRESSION_GAIN_TAGS.has(tag)) continue;
			gainTags[tag] = true;
		}
		const lossTags: Record<string, boolean> = {};
		for (const tag of Object.keys(stats.lossesBreakdown.ledger)) {
			lossTags[tag] = true;
		}
		config = { gainTags, lossTags };
	}

	// Monotonic token so a slow response for a superseded period cannot
	// overwrite the newer period's data (or its error) after a range change.
	let loadEpoch = 0;

	async function loadData(period: string) {
		const epoch = ++loadEpoch;
		loading = true;
		error = null;
		try {
			const loaded = await getAnalyticsOverview(period);
			if (epoch !== loadEpoch) return;
			data = loaded;
			initConfig(data);
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load overview');
		} finally {
			if (epoch === loadEpoch) loading = false;
		}
	}

	// ── Config-aware aggregation helpers ──
	function sumRecord(rec: Record<string, number>, enabled: Record<string, boolean>): number {
		let total = 0;
		for (const [tag, amount] of Object.entries(rec)) {
			if (enabled[tag]) total += amount;
		}
		return total;
	}

	function dayGains(d: TimelineDay): number {
		let total = d.lootTt + d.questItemTt;
		total += sumRecord(d.ledgerGains, config.gainTags);
		return total;
	}
	// Losses are Cycled, the sessions' overhead, and the enabled ledger
	// expenses; overhead is never part of Cycled.
	function dayLosses(d: TimelineDay): number {
		return d.trackingCost + d.overheadCost + sumRecord(d.ledgerLosses, config.lossTags);
	}
	function monthGains(m: MonthlyEntry): number {
		let total = m.lootTt + m.questItemTt;
		total += sumRecord(m.ledgerGains, config.gainTags);
		return total;
	}
	function monthLosses(m: MonthlyEntry): number {
		return m.trackingCost + m.overheadCost + sumRecord(m.ledgerLosses, config.lossTags);
	}

	// The window's returns and costs under the current toggles: the donut
	// and the cumulative breakdown both read these, so they cannot part.
	const totals = $derived.by((): BreakdownTotals | null => {
		if (!data) return null;
		const rb = data.returnsBreakdown;
		const lb = data.lossesBreakdown;
		let returns = rb.lootTt + rb.questItemTt;
		for (const [tag, amount] of Object.entries(rb.ledger)) {
			if (config.gainTags[tag] && amount > 0) returns += amount;
		}
		const costs = lb.trackingCost + lb.overhead.total + sumRecord(lb.ledger, config.lossTags);
		return { returns, costs };
	});

	const pieView = $derived.by((): PieView | null => {
		if (!data) return null;

		const rb = data.returnsBreakdown;

		// Build gain sources from config
		const sources: { key: string; ped: number }[] = [];
		if (rb.lootTt > 0) sources.push({ key: 'lootTt', ped: rb.lootTt });
		if (rb.questItemTt > 0) sources.push({ key: 'questItemTt', ped: rb.questItemTt });
		for (const [tag, amount] of Object.entries(rb.ledger)) {
			if (config.gainTags[tag] && amount > 0) sources.push({ key: tag, ped: amount });
		}

		const gains = sources.reduce((sum, s) => sum + s.ped, 0);
		const losses = totals?.costs ?? 0;

		if (losses <= 0 || gains <= 0) return null;

		const arcs: PieView['arcs'] = [];
		let offset = 0;
		for (const { key, ped } of sources) {
			const length = (ped / gains) * PIE_C;
			arcs.push({
				label: labelFor(key),
				ped,
				pct: ped / losses,
				color: colorFor(key),
				length,
				offset,
			});
			offset += length;
		}
		return { rate: gains / losses, gains, losses, arcs };
	});

	// ── Timeline (config-aware cumulative P&L) ──
	const chartPoints = $derived.by(() => {
		if (!data || data.timeline.length < 2) return [];
		let cumulative = 0;
		const vals = data.timeline.map((d) => {
			cumulative += dayGains(d) - dayLosses(d);
			return { date: d.date, net: cumulative };
		});
		const nets = vals.map((v) => v.net);
		const minV = Math.min(...nets, 0);
		const maxV = Math.max(...nets, 0);
		const range = maxV - minV || 1;
		// Y-mapping: the data line is bounded between y=28 (top) and y=140 (bottom).
		// Top reserves 18px of headroom so the end-of-period current-net label
		// (which sits above the rightmost dot) never overlaps the line, even when
		// the line peaks at the chart's all-time-high right edge.
		return vals.map((v, i) => ({
			x: 40 + (i / (vals.length - 1)) * 720,
			y: 28 + ((maxV - v.net) / range) * 112,
			value: Math.round(v.net * 100) / 100,
			date: v.date,
		}));
	});

	const chartPath = $derived(chartPoints.map((p) => `${p.x},${p.y}`).join(' '));

	const zeroY = $derived.by(() => {
		if (chartPoints.length < 2) return 84;
		const vals = chartPoints.map((p) => p.value);
		const minV = Math.min(...vals, 0);
		const maxV = Math.max(...vals, 0);
		const range = maxV - minV || 1;
		return 28 + ((maxV - 0) / range) * 112;
	});

	// Fill polygon closes at the zero line (not the bottom of the chart) so the
	// above-zero half lives between the data line and zeroY, the below-zero half
	// likewise. Each half is then clipped + tinted to its sign-coloured gradient.
	const chartFillPath = $derived.by(() => {
		if (chartPoints.length < 2) return '';
		const last = chartPoints[chartPoints.length - 1];
		const first = chartPoints[0];
		return `${chartPath} ${last.x},${zeroY} ${first.x},${zeroY}`;
	});

	// ── Monthly (config-aware) ──
	const monthlyRows = $derived.by(() => {
		if (!data) return [];
		return data.monthlyBreakdown.map((m) => {
			const gains = monthGains(m);
			const losses = monthLosses(m);
			const net = gains - losses;
			const globalRate = losses > 0 ? gains / losses : null;
			const cycled = m.trackingCost;
			const lootRate = cycled > 0 ? m.lootTt / cycled : null;
			return {
				month: m.month,
				cost: losses,
				returns: gains,
				net,
				lootRate,
				globalRate,
				pes: m.pes + m.codexPes + m.questPes,
			};
		});
	});

	return {
		get data() {
			return data;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get config() {
			return config;
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
		get showBreakdown() {
			return showBreakdown;
		},
		set showBreakdown(value: boolean) {
			showBreakdown = value;
		},
		get pieView() {
			return pieView;
		},
		/** The cumulative breakdown's totals: the donut's gains and losses. */
		get totals() {
			return totals;
		},
		/** The window's overhead band; `null` when it has none. */
		get overheadBand() {
			return data ? overheadBand(data.lossesBreakdown) : null;
		},
		get chartPoints() {
			return chartPoints;
		},
		get chartPath() {
			return chartPath;
		},
		get chartFillPath() {
			return chartFillPath;
		},
		get zeroY() {
			return zeroY;
		},
		get monthlyRows() {
			return monthlyRows;
		},

		loadData,
	};
}

export type OverviewModel = ReturnType<typeof createOverviewModel>;
