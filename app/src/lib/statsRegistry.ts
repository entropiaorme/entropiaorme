import type { LifetimeStats, TrackingStatus } from './api';
import { formatMultiplier, formatPed, formatPercent } from './utils/format';

export type StatId =
	| 'cycled'
	| 'loot_tt'
	| 'net'
	| 'rate'
	| 'pes'
	| 'pes_per_100'
	| 'latest_kill_loot'
	| 'avg_cost_per_kill'
	| 'avg_damage'
	| 'multiplier_last'
	| 'multiplier_avg'
	| 'multiplier_max'
	| 'max_damage'
	| 'dpp'
	| 'avg_dps'
	| 'crit_rate'
	| 'kills_count'
	| 'globals_count'
	| 'hofs_count'
	| 'uses_tree';

export type StatRender = {
	value: string;
	color: string;
	/**
	 * Set when the figure is known to leave something out, saying what:
	 * shots recorded without a price are missing from every cost the
	 * figure is built on. Surfaces mark the figure rather than let it read
	 * as a precise total.
	 */
	incomplete?: string;
};

export type StatDef = {
	id: StatId;
	/** The full name shown in the customise-stats list. */
	label: string;
	/**
	 * Compact name for the dashboard tiles and the overlay strip; the
	 * customise list keeps the full label. Room is deliberately left in
	 * this naming family for future activity counters (Shots, Uses
	 * (Heal), ...) beside Uses (Tree).
	 */
	shortLabel?: string;
	defaultEnabled: boolean;
	defaultOverlayEnabled?: boolean;
	render: (status: TrackingStatus | null) => StatRender;
	/**
	 * Render the stat over a session family's lifetime aggregate rather
	 * than the instance in play.
	 *
	 * The presence of this function IS the declaration that the stat
	 * has a lifetime form, which is why there is no separate list of
	 * lifetime-capable ids to drift out of step with the registry. A
	 * stat earns one only if it is a sum of per-instance totals or a
	 * ratio of two such sums: that is what makes a lifetime figure mean
	 * what it says. A "last" value (last loot, last multiplier) has no
	 * lifetime form at all, and an average over per-event data we do
	 * not aggregate per instance (crit rate, DPS, average multiplier)
	 * cannot be rebuilt from the summed parts.
	 */
	renderLifetime?: (lifetime: LifetimeStats) => StatRender;
};

const isActive = (s: TrackingStatus | null): s is TrackingStatus => s?.status === 'active';

const PLAIN = 'text-text';
const EMPTY: StatRender = { value: '—', color: PLAIN };

/**
 * Render a lifetime figure only when there are instances behind it.
 *
 * A family nobody has run yet aggregates to zero on every axis, and a
 * rendered `0.00` claims a measurement that was never taken. An empty
 * reading says so instead.
 */
function overSpan(
	lifetime: LifetimeStats,
	render: (lifetime: LifetimeStats) => StatRender,
): StatRender {
	return lifetime.instanceCount > 0 ? render(lifetime) : EMPTY;
}

/** What an incomplete cost leaves out, in the player's terms. */
export function unpricedNote(shots: number): string {
	return `Leaves out ${shots} ${shots === 1 ? 'shot' : 'shots'} that could not be priced`;
}

/**
 * Mark a cost-derived figure incomplete while shots sit unpriced. Only a
 * figure actually drawn is marked: an empty reading claims nothing.
 */
function costBased(shots: number | null | undefined, render: StatRender): StatRender {
	return shots && shots > 0 && render.value !== EMPTY.value
		? { ...render, incomplete: unpricedNote(shots) }
		: render;
}

/** A live figure built on the session's cost. */
function liveCost(
	render: (status: TrackingStatus) => StatRender,
): (status: TrackingStatus | null) => StatRender {
	return (status) => (isActive(status) ? costBased(status.unpricedShots, render(status)) : EMPTY);
}

/** A lifetime figure built on the family's cycled total. */
function lifetimeCost(
	render: (lifetime: LifetimeStats) => StatRender,
): (lifetime: LifetimeStats) => StatRender {
	return (lifetime) => costBased(lifetime.unpricedShots, overSpan(lifetime, render));
}

/** A net figure reads the same either side of the flip: signed, and
 * coloured by which side of break-even it falls. */
function signedNet(net: number): StatRender {
	const sign = net >= 0 ? '+' : '';
	return {
		value: `${sign}${formatPed(net)}`,
		color: net >= 0 ? 'text-positive' : 'text-negative',
	};
}

function elapsedSeconds(status: TrackingStatus): number | null {
	if (!status.started_at) return null;
	const started = new Date(status.started_at).getTime();
	if (Number.isNaN(started)) return null;
	return Math.max(0, (Date.now() - started) / 1000);
}

export const STAT_DEFS: Record<StatId, StatDef> = {
	cycled: {
		id: 'cycled',
		label: 'Cycled',
		defaultEnabled: true,
		render: liveCost((status) => ({ value: formatPed(status.cost ?? 0), color: PLAIN })),
		renderLifetime: lifetimeCost((l) => ({ value: formatPed(l.cycled), color: PLAIN })),
	},
	loot_tt: {
		id: 'loot_tt',
		label: 'Loot TT',
		defaultEnabled: true,
		render: (status) =>
			isActive(status) ? { value: formatPed(status.returns ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: formatPed(l.lootTt), color: PLAIN })),
	},
	net: {
		id: 'net',
		label: 'Net',
		defaultEnabled: true,
		defaultOverlayEnabled: true,
		render: liveCost((status) => signedNet((status.returns ?? 0) - (status.cost ?? 0))),
		renderLifetime: lifetimeCost((l) => signedNet(l.net)),
	},
	rate: {
		id: 'rate',
		label: 'Rate',
		defaultEnabled: true,
		render: liveCost((status) => ({ value: formatPercent(status.returnRate ?? 0), color: PLAIN })),
		// Already the ratio of the summed parts, computed backend-side;
		// never the mean of the per-instance rates.
		renderLifetime: lifetimeCost((l) => ({ value: formatPercent(l.returnRate), color: PLAIN })),
	},
	pes: {
		id: 'pes',
		label: 'PES',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: formatPed(status.pes ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: formatPed(l.pes), color: PLAIN })),
	},
	pes_per_100: {
		id: 'pes_per_100',
		label: 'PES/100',
		defaultEnabled: false,
		render: liveCost((status) => {
			const cost = status.cost ?? 0;
			if (cost <= 0) return EMPTY;
			return { value: (((status.pes ?? 0) / cost) * 100).toFixed(2), color: PLAIN };
		}),
		// A ratio of two sums that both flip, so it flips with them:
		// leaving it on the instance beside a lifetime PES and a
		// lifetime Cycled would be the arbitrary choice.
		renderLifetime: lifetimeCost((l) =>
			l.cycled > 0 ? { value: ((l.pes / l.cycled) * 100).toFixed(2), color: PLAIN } : EMPTY,
		),
	},
	latest_kill_loot: {
		id: 'latest_kill_loot',
		label: 'Last loot',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) && status.latestKillLoot != null
				? { value: formatPed(status.latestKillLoot), color: PLAIN }
				: EMPTY,
	},
	avg_cost_per_kill: {
		id: 'avg_cost_per_kill',
		label: 'Avg cost/kill',
		defaultEnabled: false,
		render: liveCost((status) => {
			const kills = status.kill_count ?? 0;
			if (kills <= 0) return EMPTY;
			return { value: formatPed((status.cost ?? 0) / kills), color: PLAIN };
		}),
	},
	avg_damage: {
		id: 'avg_damage',
		label: 'Avg dmg',
		defaultEnabled: false,
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const kills = status.kill_count ?? 0;
			if (kills <= 0) return EMPTY;
			return {
				value: formatPed((status.damageDealtTotal ?? 0) / kills),
				color: PLAIN,
			};
		},
	},
	multiplier_last: {
		id: 'multiplier_last',
		label: 'Last Mult',
		defaultEnabled: false,
		defaultOverlayEnabled: true,
		// A kill's multiplier divides by its weapon cost, so an unpriced
		// shot inflates it.
		render: liveCost((status) =>
			status.multiplierLast != null
				? { value: formatMultiplier(status.multiplierLast), color: PLAIN }
				: EMPTY,
		),
	},
	multiplier_avg: {
		id: 'multiplier_avg',
		label: 'Avg Mult',
		defaultEnabled: false,
		// A kill's multiplier divides by its weapon cost, so an unpriced
		// shot inflates it.
		render: liveCost((status) =>
			status.multiplierAvg != null
				? { value: formatMultiplier(status.multiplierAvg), color: PLAIN }
				: EMPTY,
		),
	},
	multiplier_max: {
		id: 'multiplier_max',
		label: 'Max Mult',
		defaultEnabled: false,
		// A kill's multiplier divides by its weapon cost, so an unpriced
		// shot inflates it.
		render: liveCost((status) =>
			status.multiplierMax != null
				? { value: formatMultiplier(status.multiplierMax), color: PLAIN }
				: EMPTY,
		),
	},
	max_damage: {
		id: 'max_damage',
		label: 'Max dmg',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) && status.maxDamage != null
				? { value: formatPed(status.maxDamage), color: PLAIN }
				: EMPTY,
	},
	dpp: {
		id: 'dpp',
		label: 'DPP',
		defaultEnabled: false,
		render: liveCost((status) => {
			const weaponCostPed = status.weaponCost ?? 0;
			if (weaponCostPed <= 0) return EMPTY;
			const weaponDamage = status.weaponDamageDealt ?? status.damageDealtTotal ?? 0;
			// Backend weapon cost is PED; classic DPP is damage per PEC.
			const dpp = weaponDamage / (weaponCostPed * 100);
			return { value: dpp.toFixed(2), color: PLAIN };
		}),
	},
	avg_dps: {
		id: 'avg_dps',
		label: 'Avg DPS',
		defaultEnabled: false,
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const elapsed = elapsedSeconds(status);
			if (elapsed == null || elapsed <= 0) return EMPTY;
			return {
				value: formatPed((status.damageDealtTotal ?? 0) / elapsed),
				color: PLAIN,
			};
		},
	},
	crit_rate: {
		id: 'crit_rate',
		label: 'Crit rate',
		defaultEnabled: false,
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const shots = status.shotsFiredTotal ?? 0;
			if (shots <= 0) return EMPTY;
			return {
				value: formatPercent((status.criticalHitsTotal ?? 0) / shots),
				color: PLAIN,
			};
		},
	},
	kills_count: {
		id: 'kills_count',
		label: 'Kills',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.kill_count ?? 0), color: PLAIN } : EMPTY,
	},
	globals_count: {
		id: 'globals_count',
		label: 'Globals',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.globalsCount ?? 0), color: PLAIN } : EMPTY,
	},
	hofs_count: {
		id: 'hofs_count',
		label: 'HOFs',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.hofsCount ?? 0), color: PLAIN } : EMPTY,
	},
	uses_tree: {
		id: 'uses_tree',
		label: 'Uses (Tree)',
		shortLabel: 'Uses (T)',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.harvestSwings ?? 0), color: PLAIN } : EMPTY,
	},
};

export const ALL_STAT_IDS: StatId[] = [
	'cycled',
	'loot_tt',
	'net',
	'rate',
	'pes',
	'pes_per_100',
	'latest_kill_loot',
	'avg_cost_per_kill',
	'avg_damage',
	'multiplier_last',
	'multiplier_avg',
	'multiplier_max',
	'max_damage',
	'dpp',
	'avg_dps',
	'crit_rate',
	'kills_count',
	'globals_count',
	'hofs_count',
	'uses_tree',
];

/**
 * The stats that have a lifetime form, derived from the registry itself
 * so it cannot drift: a stat is lifetime-capable exactly when its
 * definition declares a `renderLifetime`.
 *
 * Render paths ask `isLifetimeCapable` per stat rather than reading this
 * list. It exists as the enumerable form of the same fact, which is what
 * lets a test assert the capable set as a whole and catch a stat that
 * gained or lost a lifetime render without anyone meaning it to.
 */
export const LIFETIME_STAT_IDS: StatId[] = ALL_STAT_IDS.filter(
	(id) => STAT_DEFS[id].renderLifetime !== undefined,
);

/**
 * What lifetime mode falls back to when a user's customised selection
 * contains nothing with a lifetime form. Rendering an empty grid would
 * read as broken, so the flip shows the four headline figures instead.
 * Preferences are untouched either way: this is what gets DRAWN, never
 * what gets stored.
 */
export const HEADLINE_LIFETIME_STAT_IDS: StatId[] = ['cycled', 'loot_tt', 'net', 'rate'];

export function isLifetimeCapable(id: string): boolean {
	return getStatDef(id)?.renderLifetime !== undefined;
}

export function getStatDef(id: string): StatDef | null {
	// Own-property guard: without it, prototype-chain keys ('__proto__',
	// 'constructor', ...) would resolve to inherited Object members typed as
	// StatDef rather than null. Ids come from the fixed StatId union and prefs
	// are sanitised against ALL_STAT_IDS, so this is defence in depth.
	if (!Object.hasOwn(STAT_DEFS, id)) return null;
	return (STAT_DEFS as Record<string, StatDef | undefined>)[id] ?? null;
}
