/**
 * The routes table's rows: the planned routes (a weapon on a mob maturity,
 * priced by the model) and the recorded named sessions (priced by what they
 * recorded) as one list, each costed to the goal, with the premium the
 * player pays over the cheapest route that answers.
 */

import type {
	SkillingForecastResult,
	SkillingForecastSource,
	SkillingForecastStatus,
	SkillingRouteForecast,
	SkillingRouteStatus,
} from '$lib/api/commands.gen';

/** A planned route as the player stores it. */
export interface StoredRoute {
	weaponId: string;
	weaponName: string;
	species: string;
	maturity: string;
	/** The mob as the picker shows it ("Puny Carabok"). */
	display: string;
}

export type RouteStatus = SkillingRouteStatus | SkillingForecastStatus;

export type RouteRow =
	| (RowFigures & { kind: 'planned'; key: string; index: number; route: SkillingRouteForecast })
	| (RowFigures & { kind: 'recorded'; key: string; source: SkillingForecastSource });

interface RowFigures {
	name: string;
	status: RouteStatus;
	/** TT lost reaching the goal: expected for a planned route, as recorded
	 * (cycled less loot TT) for a named session. */
	pedLost: number;
	hours: number | null;
	cycledPed: number;
	/** PED lost above the cheapest answering route; null when the row does
	 * not answer or nothing else does. */
	premium: number | null;
	/** That premium per hour of this route's play; null without both. */
	premiumPerHour: number | null;
}

export function plannedKey(index: number): string {
	return `route:${index}`;
}

export function recordedKey(definitionId: number): string {
	return `session:${definitionId}`;
}

/** The planned route a stored pick asks the forecast for. */
export function routePick(route: StoredRoute) {
	return {
		setup: { weapon: { catalogId: route.weaponId } },
		species: route.species,
		maturity: route.maturity,
	};
}

/** True when `value` has a stored route's shape (a preference read back). */
export function isStoredRoute(value: unknown): value is StoredRoute {
	if (typeof value !== 'object' || value === null) return false;
	const route = value as Record<string, unknown>;
	return ['weaponId', 'weaponName', 'species', 'maturity', 'display'].every(
		(key) => typeof route[key] === 'string',
	);
}

/** Every route, answering ones first and cheapest first, with premiums
 * against the cheapest. Planned routes the forecast did not return (a
 * stale response) are left out. */
export function routeRows(
	result: SkillingForecastResult | null,
	stored: StoredRoute[],
): RouteRow[] {
	if (!result) return [];
	const planned: RouteRow[] = result.routes
		.filter((route) => route.index < stored.length)
		.map((route) => ({
			kind: 'planned',
			key: plannedKey(route.index),
			index: route.index,
			route,
			name: `${route.weapon || stored[route.index].weaponName} on ${route.mob}`,
			status: route.status,
			pedLost: route.pedLost,
			hours: route.hours,
			cycledPed: route.cycledPed,
			premium: null,
			premiumPerHour: null,
		}));
	const recorded: RouteRow[] = result.sources.map((source) => ({
		kind: 'recorded',
		key: recordedKey(source.definitionId),
		source,
		name: source.name,
		status: source.status,
		pedLost: source.ttCost,
		hours: source.status === 'ready' ? source.hours : null,
		cycledPed: source.cycledPed,
		premium: null,
		premiumPerHour: null,
	}));
	const rows = [...planned, ...recorded];
	const ready = rows.filter((row) => row.status === 'ready');
	const cheapest = ready.length > 1 ? Math.min(...ready.map((row) => row.pedLost)) : null;
	for (const row of ready) {
		if (cheapest === null) break;
		row.premium = row.pedLost - cheapest;
		row.premiumPerHour = row.hours && row.hours > 0 ? row.premium / row.hours : null;
	}
	const order = (row: RouteRow) => (row.status === 'ready' ? 0 : row.status === 'reached' ? 1 : 2);
	return rows
		.map((row, position) => ({ row, position }))
		.sort(
			(a, b) =>
				order(a.row) - order(b.row) ||
				(order(a.row) === 0 ? a.row.pedLost - b.row.pedLost : 0) ||
				a.position - b.position,
		)
		.map(({ row }) => row);
}

/** Why a route does not answer, in a few words. */
export function routeStatusLabel(status: RouteStatus): string {
	switch (status) {
		case 'ready':
			return '';
		case 'reached':
			return 'Goal already reached';
		case 'does_not_train':
			return 'Does not train this';
		case 'out_of_range':
			return 'Beyond reach';
		case 'no_evidence':
			return 'Nothing recorded';
		case 'unknown_weapon':
			return 'Weapon not in the catalogue';
		case 'unknown_mob':
			return 'No HP or level for this mob';
		case 'unmodelled':
			return 'Cannot price this weapon';
	}
}
