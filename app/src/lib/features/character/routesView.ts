/**
 * The routes table's rows: each a weapon on a mob maturity the player has
 * set up, priced to the goal, with the premium paid over the cheapest
 * route that answers.
 */

import type {
	SkillingForecastResult,
	SkillingRouteForecast,
	SkillingRouteStatus,
} from '$lib/api/commands.gen';

/** A route as the player stores it. */
export interface StoredRoute {
	weaponId: string;
	weaponName: string;
	species: string;
	maturity: string;
	/** The mob as the picker shows it ("Puny Carabok"). */
	display: string;
}

export interface RouteRow {
	key: string;
	index: number;
	stored: StoredRoute;
	/** The priced route; null while the forecast has not answered it. */
	route: SkillingRouteForecast | null;
	status: SkillingRouteStatus | null;
	/** PED lost above the cheapest answering route; null when the row does
	 * not answer or nothing else does. */
	premium: number | null;
	/** That premium per hour of this route's play; null without both. */
	premiumPerHour: number | null;
}

export function routeKey(index: number): string {
	return `route:${index}`;
}

/** The route a stored pick asks the forecast for. */
export function routePick(route: StoredRoute) {
	return {
		setup: { weapon: { catalogId: route.weaponId } },
		species: route.species,
		maturity: route.maturity,
	};
}

/** True when two stored routes are the same weapon on the same mob. */
export function sameRoute(a: StoredRoute, b: StoredRoute): boolean {
	return a.weaponId === b.weaponId && a.species === b.species && a.maturity === b.maturity;
}

/** True when `value` has a stored route's shape (a preference read back). */
export function isStoredRoute(value: unknown): value is StoredRoute {
	if (typeof value !== 'object' || value === null) return false;
	const route = value as Record<string, unknown>;
	return ['weaponId', 'weaponName', 'species', 'maturity', 'display'].every(
		(key) => typeof route[key] === 'string',
	);
}

/** Every stored route in the player's order, priced where the forecast
 * answered it, with premiums against the cheapest that answers. */
export function routeRows(
	result: SkillingForecastResult | null,
	stored: StoredRoute[],
): RouteRow[] {
	const rows: RouteRow[] = stored.map((route, index) => {
		const priced = result?.routes.find((candidate) => candidate.index === index) ?? null;
		return {
			key: routeKey(index),
			index,
			stored: route,
			route: priced,
			status: priced?.status ?? null,
			premium: null,
			premiumPerHour: null,
		};
	});
	const ready = rows.filter((row) => row.route?.status === 'ready');
	if (ready.length > 1) {
		const cheapest = Math.min(...ready.map((row) => row.route?.pedLost ?? 0));
		for (const row of ready) {
			const route = row.route as SkillingRouteForecast;
			row.premium = route.pedLost - cheapest;
			row.premiumPerHour = route.hours && route.hours > 0 ? row.premium / route.hours : null;
		}
	}
	return rows;
}

/** Why a route does not answer, in a few words. */
export function routeStatusLabel(status: SkillingRouteStatus): string {
	switch (status) {
		case 'ready':
			return '';
		case 'reached':
			return 'Goal already reached';
		case 'does_not_train':
			return 'Does not train this';
		case 'out_of_range':
			return 'Beyond reach';
		case 'unknown_weapon':
			return 'Weapon not in the catalogue';
		case 'unknown_mob':
			return 'No HP or level for this mob';
		case 'unmodelled':
			return 'Cannot price this weapon';
	}
}
