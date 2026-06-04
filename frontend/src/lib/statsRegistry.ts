import type { TrackingStatus } from './api';
import { formatPed, formatPercent, formatMultiplier } from './utils/format';

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
	| 'hofs_count';

export type StatRender = {
	value: string;
	color: string;
};

export type StatDef = {
	id: StatId;
	label: string;
	defaultEnabled: boolean;
	defaultOverlayEnabled?: boolean;
	render: (status: TrackingStatus | null) => StatRender;
};

const isActive = (s: TrackingStatus | null): s is TrackingStatus => s?.status === 'active';

const PLAIN = 'text-text';
const EMPTY: StatRender = { value: '—', color: PLAIN };

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
		render: (status) =>
			isActive(status) ? { value: formatPed(status.cost ?? 0), color: PLAIN } : EMPTY,
	},
	loot_tt: {
		id: 'loot_tt',
		label: 'Loot TT',
		defaultEnabled: true,
		render: (status) =>
			isActive(status) ? { value: formatPed(status.returns ?? 0), color: PLAIN } : EMPTY,
	},
	net: {
		id: 'net',
		label: 'Net',
		defaultEnabled: true,
		defaultOverlayEnabled: true,
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const net = (status.returns ?? 0) - (status.cost ?? 0);
			const sign = net >= 0 ? '+' : '';
			return {
				value: `${sign}${formatPed(net)}`,
				color: net >= 0 ? 'text-positive' : 'text-negative',
			};
		},
	},
	rate: {
		id: 'rate',
		label: 'Rate',
		defaultEnabled: true,
		render: (status) =>
			isActive(status) ? { value: formatPercent(status.returnRate ?? 0), color: PLAIN } : EMPTY,
	},
	pes: {
		id: 'pes',
		label: 'PES',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) ? { value: formatPed(status.pes ?? 0), color: PLAIN } : EMPTY,
	},
	pes_per_100: {
		id: 'pes_per_100',
		label: 'PES/100',
		defaultEnabled: false,
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const cost = status.cost ?? 0;
			if (cost <= 0) return EMPTY;
			return { value: (((status.pes ?? 0) / cost) * 100).toFixed(2), color: PLAIN };
		},
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
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const kills = status.kill_count ?? 0;
			if (kills <= 0) return EMPTY;
			return { value: formatPed((status.cost ?? 0) / kills), color: PLAIN };
		},
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
		render: (status) =>
			isActive(status) && status.multiplierLast != null
				? { value: formatMultiplier(status.multiplierLast), color: PLAIN }
				: EMPTY,
	},
	multiplier_avg: {
		id: 'multiplier_avg',
		label: 'Avg Mult',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) && status.multiplierAvg != null
				? { value: formatMultiplier(status.multiplierAvg), color: PLAIN }
				: EMPTY,
	},
	multiplier_max: {
		id: 'multiplier_max',
		label: 'Max Mult',
		defaultEnabled: false,
		render: (status) =>
			isActive(status) && status.multiplierMax != null
				? { value: formatMultiplier(status.multiplierMax), color: PLAIN }
				: EMPTY,
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
		render: (status) => {
			if (!isActive(status)) return EMPTY;
			const weaponCostPed = status.weaponCost ?? 0;
			if (weaponCostPed <= 0) return EMPTY;
			const weaponDamage = status.weaponDamageDealt ?? status.damageDealtTotal ?? 0;
			// Backend weapon cost is PED; classic DPP is damage per PEC.
			const dpp = weaponDamage / (weaponCostPed * 100);
			return { value: dpp.toFixed(2), color: PLAIN };
		},
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
];

export function getStatDef(id: string): StatDef | null {
	return (STAT_DEFS as Record<string, StatDef | undefined>)[id] ?? null;
}
