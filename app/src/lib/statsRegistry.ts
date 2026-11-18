import type { LifetimeStats, TrackingStatus } from './api';
import type { LiveValuationView } from './features/dashboard/liveLootValuation.svelte';
import { describeUnpriced } from './features/weapons/weaponReview';
import {
	atLooterBasis,
	DEFAULT_LOOTER_BASIS,
	type LooterBasis,
	looterBasisInitial,
} from './looterBasis.svelte';
import { formatMultiplier, formatPed, formatPercent, formatTime } from './utils/format';

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
	| 'uses_tree'
	| 'loot_mu'
	| 'mu_rate'
	| 'expected_mu_rate'
	| 'reward'
	| 'reward_mu';

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
	/** A quiet second reading beside the value (an expected figure's
	 * margin, `±0.4`). */
	detail?: string;
	/** What the figure is read at, for a surface with no control to show
	 * it (the overlay names an expected figure's looter basis this way). */
	qualifier?: string;
	/** A quiet word on where the figure comes from, for its tooltip: a
	 * cost figure resting on the account readings says up to when. */
	note?: string;
};

/**
 * What a live surface knows beyond the tracking snapshot: the session's
 * loot valued at market markup. Surfaces without it (or before it has
 * answered) pass none, and the markup stats read empty.
 */
export type StatContext = {
	valuation?: LiveValuationView | null;
	/** The session family's loot valued at market markup, for the lifetime
	 * forms of the markup stats. Absent or null while it cannot be given. */
	lifetimeValuation?: LiveValuationView | null;
	/** The looter basis the live expected figure is read at. */
	looterBasis?: LooterBasis;
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
	render: (status: TrackingStatus | null, context?: StatContext) => StatRender;
	/**
	 * Render the stat over a session family's lifetime aggregate rather
	 * than the instance in play.
	 *
	 * Required, so a new stat has to say what its family figure is and
	 * the flip keeps the same tiles in view. Each form is built from what
	 * folds across instances, which is what makes a lifetime figure mean
	 * what it says: a sum, a ratio of two sums, an extreme, or the
	 * family's most recent event. An average over kills is the mean over
	 * every kill the family recorded, never the mean of each instance's
	 * mean.
	 */
	renderLifetime: (lifetime: LifetimeStats, context?: StatContext) => StatRender;
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

/**
 * Mark a cost-derived figure incomplete while shots sit unpriced, with the
 * most they could add when it is known. Only a figure actually drawn is
 * marked: an empty reading claims nothing.
 */
function costBased(
	shots: number | null | undefined,
	render: StatRender,
	costBound?: number | null,
): StatRender {
	return shots && shots > 0 && render.value !== EMPTY.value
		? {
				...render,
				incomplete: [describeUnpriced(shots, costBound), render.incomplete]
					.filter(Boolean)
					.join('; '),
			}
		: render;
}

/** A live figure built on the session's cost. */
function liveCost(
	render: (status: TrackingStatus, context: StatContext) => StatRender,
): (status: TrackingStatus | null, context?: StatContext) => StatRender {
	return (status, context = {}) =>
		isActive(status)
			? costBased(status.unpricedShots, render(status, context), status.unpricedCostBound)
			: EMPTY;
}

/**
 * Where the live costs come from, while they rest on the account readings:
 * what the readings measured up to their latest reading, and the tracked
 * play since. Nothing while they do not (the player is not signed in, or
 * no reading after the session's first has measured it yet).
 */
function accountCostsNote(status: TrackingStatus | null): string | undefined {
	const through = isActive(status) ? status.costsMeasuredThrough : null;
	return through
		? `Costs from your account up to ${formatTime(through)}, tracked since`
		: undefined;
}

/** A live figure built on the session's Cycled or Net: the account note
 * rides on it while the costs rest on the readings and a figure is drawn. */
function accountCost(
	render: (status: TrackingStatus, context: StatContext) => StatRender,
): (status: TrackingStatus | null, context?: StatContext) => StatRender {
	const live = liveCost(render);
	return (status, context = {}) => {
		const rendered = live(status, context);
		const note = rendered.value !== EMPTY.value ? accountCostsNote(status) : undefined;
		return note ? { ...rendered, note } : rendered;
	};
}

/** The live valuation for the session in the snapshot, or none: a
 * valuation left over from a previous session never stands for this one. */
function liveValuation(status: TrackingStatus, context: StatContext): LiveValuationView | null {
	const valuation = context.valuation ?? null;
	return valuation && valuation.sessionId === status.session_id ? valuation : null;
}

/** A lifetime figure built on the family's cycled total. */
function lifetimeCost(
	render: (lifetime: LifetimeStats, context: StatContext) => StatRender,
): (lifetime: LifetimeStats, context?: StatContext) => StatRender {
	return (lifetime, context = {}) =>
		costBased(
			lifetime.unpricedShots,
			overSpan(lifetime, (l) => render(l, context)),
		);
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
		render: accountCost((status) => ({ value: formatPed(status.cost ?? 0), color: PLAIN })),
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
		// The overhead the account readings measured is in Net, never in
		// Cycled.
		render: accountCost((status) =>
			signedNet((status.returns ?? 0) - (status.cost ?? 0) - (status.overheadCost ?? 0)),
		),
		renderLifetime: lifetimeCost((l) => signedNet(l.net)),
	},
	rate: {
		id: 'rate',
		label: 'Rate',
		defaultEnabled: true,
		render: accountCost((status) => ({
			value: formatPercent(status.returnRate ?? 0),
			color: PLAIN,
		})),
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
		render: accountCost((status) => {
			const cost = status.cost ?? 0;
			if (cost <= 0) return EMPTY;
			return { value: (((status.pes ?? 0) / cost) * 100).toFixed(2), color: PLAIN };
		}),
		// A ratio of two sums that both flip, so it flips with them:
		// leaving it on the instance beside a lifetime PES and a
		// lifetime Cycled would be the arbitrary choice.
		renderLifetime: lifetimeCost((l) =>
			l.pesPer100 != null ? { value: l.pesPer100.toFixed(2), color: PLAIN } : EMPTY,
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
		// The family's most recent kill: the running instance's own once
		// it has one, so the two scopes agree from then on.
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) =>
				l.latestKillLoot != null ? { value: formatPed(l.latestKillLoot), color: PLAIN } : EMPTY,
			),
	},
	avg_cost_per_kill: {
		id: 'avg_cost_per_kill',
		label: 'Avg cost/kill',
		defaultEnabled: false,
		render: accountCost((status) => {
			const kills = status.kill_count ?? 0;
			if (kills <= 0) return EMPTY;
			return { value: formatPed((status.cost ?? 0) / kills), color: PLAIN };
		}),
		renderLifetime: lifetimeCost((l) =>
			l.kills > 0 ? { value: formatPed(l.cycled / l.kills), color: PLAIN } : EMPTY,
		),
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
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) =>
				l.kills > 0 ? { value: formatPed(l.damageTotal / l.kills), color: PLAIN } : EMPTY,
			),
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
		renderLifetime: lifetimeCost((l) =>
			l.multiplierLast != null
				? { value: formatMultiplier(l.multiplierLast), color: PLAIN }
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
		renderLifetime: lifetimeCost((l) =>
			l.multiplierAvg != null ? { value: formatMultiplier(l.multiplierAvg), color: PLAIN } : EMPTY,
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
		renderLifetime: lifetimeCost((l) =>
			l.multiplierMax != null ? { value: formatMultiplier(l.multiplierMax), color: PLAIN } : EMPTY,
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
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) =>
				l.kills > 0 ? { value: formatPed(l.maxDamage), color: PLAIN } : EMPTY,
			),
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
		renderLifetime: lifetimeCost((l) =>
			l.weaponCost > 0
				? { value: (l.weaponDamage / (l.weaponCost * 100)).toFixed(2), color: PLAIN }
				: EMPTY,
		),
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
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) =>
				l.durationSeconds > 0
					? { value: formatPed(l.damageTotal / l.durationSeconds), color: PLAIN }
					: EMPTY,
			),
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
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) =>
				l.shotsFired > 0
					? { value: formatPercent(l.criticalHits / l.shotsFired), color: PLAIN }
					: EMPTY,
			),
	},
	kills_count: {
		id: 'kills_count',
		label: 'Kills',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.kill_count ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: String(l.kills), color: PLAIN })),
	},
	globals_count: {
		id: 'globals_count',
		label: 'Globals',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.globalsCount ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: String(l.globals), color: PLAIN })),
	},
	hofs_count: {
		id: 'hofs_count',
		label: 'HOFs',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.hofsCount ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: String(l.hofs), color: PLAIN })),
	},
	uses_tree: {
		id: 'uses_tree',
		label: 'Uses (Tree)',
		shortLabel: 'Uses (T)',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: String(status.harvestSwings ?? 0), color: PLAIN } : EMPTY,
		renderLifetime: (lifetime) =>
			overSpan(lifetime, (l) => ({ value: String(l.harvestSwings), color: PLAIN })),
	},
	// The markup stats are estimates over the informational market layer:
	// what the loot would be worth at market, never realised P&L. The
	// lifetime forms value the family's past loot at today's market. Every
	// loot-only figure says so in its name (Loot TT, Loot MU, Loot MU Rate);
	// Expected MU Rate, without the prefix, also counts the quest rewards.
	loot_mu: {
		id: 'loot_mu',
		label: 'Loot MU',
		defaultEnabled: false,
		render: (status, context = {}) => {
			if (!isActive(status)) return EMPTY;
			const factor = liveValuation(status, context)?.lootMuFactor ?? null;
			return factor === null ? EMPTY : { value: formatPercent(factor), color: PLAIN };
		},
		renderLifetime: (lifetime, context = {}) =>
			overSpan(lifetime, () => {
				const factor = context.lifetimeValuation?.lootMuFactor ?? null;
				return factor === null ? EMPTY : { value: formatPercent(factor), color: PLAIN };
			}),
	},
	// The stored id predates the name and stays, so saved layouts keep it.
	mu_rate: {
		id: 'mu_rate',
		label: 'Loot MU Rate',
		defaultEnabled: false,
		render: accountCost((status, context) => {
			const valuation = liveValuation(status, context);
			const cost = status.cost ?? 0;
			if (!valuation || valuation.lootTt <= 0 || cost <= 0) return EMPTY;
			return { value: formatPercent(valuation.marketValuePed / cost), color: PLAIN };
		}),
		renderLifetime: lifetimeCost((l, context) => {
			const valuation = context.lifetimeValuation ?? null;
			if (!valuation || valuation.lootTt <= 0 || l.cycled <= 0) return EMPTY;
			return { value: formatPercent(valuation.marketValuePed / l.cycled), color: PLAIN };
		}),
	},
	expected_mu_rate: {
		id: 'expected_mu_rate',
		label: 'Expected MU Rate',
		shortLabel: 'Exp MU',
		defaultEnabled: false,
		// Modelled loot over the whole Cycled (the model's Efficiency span
		// for spend of unknown efficiency), valued at the loot's own markup:
		// what this play would return at that markup with the variance taken
		// out, plus the quest rewards confirmed so far at their market value
		// (Reward MU), as the Analytics outlook adds them. The margin is the
		// unknown-efficiency span; a reward has none.
		render: liveCost((status, context) => {
			const valuation = liveValuation(status, context);
			const cycle = status.expectedCycle ?? null;
			// Over the Cycled the expected loot was modelled on: the tracked
			// one, whatever the account readings measured. Without them the
			// two are the session's Cycled.
			const cost = status.costsMeasuredThrough ? (cycle?.cycledPed ?? 0) : (status.cost ?? 0);
			const factor = valuation?.lootMuFactor ?? null;
			if (!cycle || factor === null || cost <= 0) return EMPTY;
			const basis = context.looterBasis ?? DEFAULT_LOOTER_BASIS;
			const at = atLooterBasis(cycle.bases, basis);
			const rewardMu = valuation?.reward?.muPed ?? 0;
			const rate = (at.expectedLootTtPed * factor + rewardMu) / cost;
			const margin = ((at.highPed - at.lowPed) * factor) / cost / 2;
			return {
				value: formatPercent(rate),
				color: PLAIN,
				detail: margin >= 0.0005 ? `±${(margin * 100).toFixed(1)}` : undefined,
				qualifier: looterBasisInitial(basis),
			};
		}),
		// Over the instances that recorded weapon evidence, each at its own
		// session-start looter levels, against those instances' own Cycled.
		// The rest are in neither side of the loot ratio, and the figure says
		// so. The family's rewards join as their own rate over its whole
		// Cycled, which is the play they were earned beside.
		renderLifetime: lifetimeCost((l, context) => {
			const cycle = l.expectedCycle;
			const factor = context.lifetimeValuation?.lootMuFactor ?? null;
			if (!cycle || factor === null || cycle.cycledPed <= 0) return EMPTY;
			const basis = context.looterBasis ?? DEFAULT_LOOTER_BASIS;
			const at = atLooterBasis(cycle.bases, basis);
			const rewardMu = context.lifetimeValuation?.reward?.muPed ?? 0;
			const rate =
				(at.expectedLootTtPed * factor) / cycle.cycledPed +
				(l.cycled > 0 ? rewardMu / l.cycled : 0);
			const margin = ((at.highPed - at.lowPed) * factor) / cycle.cycledPed / 2;
			return {
				value: formatPercent(rate),
				color: PLAIN,
				detail: margin >= 0.0005 ? `±${(margin * 100).toFixed(1)}` : undefined,
				qualifier: looterBasisInitial(basis),
				incomplete:
					cycle.sessions < l.instanceCount
						? `Covers ${cycle.sessions} of ${l.instanceCount} sessions: earlier ones recorded no weapon evidence`
						: undefined,
			};
		}),
	},
	// The quest rewards confirmed on the session: never loot and never a
	// kill. A reward is a fixed amount handed in, not a return on what was
	// cycled, so it is stated in PED, never as a rate of its own. Empty until
	// a hand-in pays one.
	reward: {
		id: 'reward',
		label: 'Reward',
		defaultEnabled: false,
		render: (status, context = {}) => {
			if (!isActive(status)) return EMPTY;
			const reward = liveValuation(status, context)?.reward ?? null;
			return reward ? { value: formatPed(reward.ttPed), color: PLAIN } : EMPTY;
		},
		renderLifetime: (lifetime, context = {}) =>
			overSpan(lifetime, () => {
				const reward = context.lifetimeValuation?.reward ?? null;
				return reward ? { value: formatPed(reward.ttPed), color: PLAIN } : EMPTY;
			}),
	},
	// The same reward at market: liquid units (ammo) at face value, items
	// through the one reward projection. PED, where Loot MU is a percentage.
	reward_mu: {
		id: 'reward_mu',
		label: 'Reward MU',
		defaultEnabled: false,
		render: (status, context = {}) => {
			if (!isActive(status)) return EMPTY;
			const reward = liveValuation(status, context)?.reward ?? null;
			return reward ? { value: formatPed(reward.muPed), color: PLAIN } : EMPTY;
		},
		renderLifetime: (lifetime, context = {}) =>
			overSpan(lifetime, () => {
				const reward = context.lifetimeValuation?.reward ?? null;
				return reward ? { value: formatPed(reward.muPed), color: PLAIN } : EMPTY;
			}),
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
	'loot_mu',
	'mu_rate',
	'expected_mu_rate',
	'reward',
	'reward_mu',
];

/** The stats whose lifetime form values the family's loot or rewards. */
const LIFETIME_VALUED_STAT_IDS: readonly string[] = [
	'loot_mu',
	'mu_rate',
	'expected_mu_rate',
	'reward',
	'reward_mu',
];

/**
 * Whether a surface's selection draws a stat that needs the family's loot
 * valued. The valuation is only read while one is on show, so a surface
 * that never asks for a markup figure never pays for it.
 */
export function drawsLifetimeValuation(
	prefs: readonly { id: string; enabled: boolean }[],
): boolean {
	return prefs.some((pref) => pref.enabled && LIFETIME_VALUED_STAT_IDS.includes(pref.id));
}

export function getStatDef(id: string): StatDef | null {
	// Own-property guard: without it, prototype-chain keys ('__proto__',
	// 'constructor', ...) would resolve to inherited Object members typed as
	// StatDef rather than null. Ids come from the fixed StatId union and prefs
	// are sanitised against ALL_STAT_IDS, so this is defence in depth.
	if (!Object.hasOwn(STAT_DEFS, id)) return null;
	return (STAT_DEFS as Record<string, StatDef | undefined>)[id] ?? null;
}
