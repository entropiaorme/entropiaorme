/**
 * The routes table's rows: each a weapon on a mob maturity the player has
 * set up, priced to the goal, with the premium paid over the cheapest
 * route that answers. A route may carry the loot markup the player expects
 * (a planning assumption, never realised), in which case it is compared on
 * its loss after that markup, and the share of the clock it spends hunting.
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
	/** The markup the player expects the loot to sell at, percent of TT;
	 * absent or null prices the route at TT. */
	lootMarkupPercent?: number | null;
	/** The share of the clock the route spends hunting, percent; absent or
	 * null reads as all of it. */
	huntingSharePercent?: number | null;
}

/** The expected loot markup a route accepts, percent of TT: TT is always
 * on offer, so nothing sells below it. The backend holds the same range. */
export const MIN_LOOT_MARKUP = 100;
export const MAX_LOOT_MARKUP = 1000;
/** The smallest share of the clock a route can spend hunting, percent. */
export const MIN_HUNTING_SHARE = 1;

export interface RouteRow {
	key: string;
	index: number;
	stored: StoredRoute;
	/** The priced route; null while the forecast has not answered it. */
	route: SkillingRouteForecast | null;
	status: SkillingRouteStatus | null;
	/** What the route is compared on: TT lost less its expected markup, or
	 * TT lost when it sets none; null when the row does not answer. */
	cost: number | null;
	/** PED lost above the cheapest answering route; null when the row does
	 * not answer or nothing else does. */
	premium: number | null;
	/** That premium per hour of this route's play; null without both. */
	premiumPerHour: number | null;
	/** What overkill costs this route; null when the row does not answer. */
	overkill: OverkillReading | null;
	/** The expected markup at which this route would cost as much as the
	 * cheapest; null on the cheapest, the only route, or a row that does
	 * not answer. */
	breakEven: BreakEven | null;
}

/** The loot markup, percent of TT, at which a route would cost as much as
 * the cheapest route (`against`). `beyond` when even the highest markup a
 * route accepts would not close the gap. */
export interface BreakEven {
	percent: number;
	beyond: boolean;
	against: StoredRoute;
}

/**
 * What a route's overkill costs, read against its place in the table. The
 * overkill PED is the route's cost (after any expected markup) on damage
 * fired past the kill: the most a weapon that fits the mob better could
 * save there, whatever the route's rank. The reading is `headroom` on the
 * cheapest route (or the only one), `covers_premium` when overkill alone
 * outweighs the route's premium (a better fit could make it the cheapest),
 * `part_of_premium` otherwise (the mob and the weapon's economy are the
 * rest), and `no_cost` when the markup outweighs the loss, so the extra
 * cycling costs nothing.
 */
export type OverkillReading = {
	/** The share of the damage fired that lands past the kill. */
	share: number;
	/** TT lost on that damage. */
	ped: number;
	/** The route's TT lost with every hit's damage used. */
	fittedPed: number;
} & (
	| { kind: 'headroom' }
	| { kind: 'covers_premium'; premium: number; cheapestPed: number }
	| { kind: 'part_of_premium'; premium: number }
	| { kind: 'no_cost' }
);

export function routeKey(index: number): string {
	return `route:${index}`;
}

/** The route a stored pick asks the forecast for. */
export function routePick(route: StoredRoute) {
	return {
		setup: { weapon: { catalogId: route.weaponId } },
		species: route.species,
		maturity: route.maturity,
		lootMarkupPercent: within(route.lootMarkupPercent, MIN_LOOT_MARKUP, MAX_LOOT_MARKUP),
		huntingSharePercent: within(route.huntingSharePercent, MIN_HUNTING_SHARE, 100),
	};
}

/** A stored percentage settled into the range the forecast accepts, so a
 * preference written by hand cannot refuse the whole forecast; unset stays
 * unset. */
function within(value: number | null | undefined, min: number, max: number): number | undefined {
	return value == null ? undefined : Math.min(Math.max(value, min), max);
}

/** True when two stored routes are the same weapon on the same mob. */
export function sameRoute(a: StoredRoute, b: StoredRoute): boolean {
	return a.weaponId === b.weaponId && a.species === b.species && a.maturity === b.maturity;
}

/** True when `value` has a stored route's shape (a preference read back). */
export function isStoredRoute(value: unknown): value is StoredRoute {
	if (typeof value !== 'object' || value === null) return false;
	const route = value as Record<string, unknown>;
	return (
		['weaponId', 'weaponName', 'species', 'maturity', 'display'].every(
			(key) => typeof route[key] === 'string',
		) &&
		['lootMarkupPercent', 'huntingSharePercent'].every(
			(key) => route[key] == null || Number.isFinite(route[key]),
		)
	);
}

/**
 * A typed route percentage: blank clears it (null), a figure outside
 * `min`..`max` settles at the nearer end, and anything unreadable leaves
 * the stored value as it was (undefined).
 */
export function readRoutePercent(
	input: string,
	min: number,
	max: number,
): number | null | undefined {
	const text = input.trim();
	if (text === '') return null;
	const value = Number(text);
	if (!Number.isFinite(value)) return undefined;
	return Math.min(Math.max(value, min), max);
}

/** What a priced route is compared on: its loss after the expected markup,
 * else its TT loss. */
export function routeCost(route: SkillingRouteForecast): number {
	return route.netCost ?? route.pedLost;
}

/**
 * The expected loot markup at which `route` would cost `cheapestCost`: its
 * expected loot TT (cycled less TT lost) lifted until the markup nets the
 * gap. Null when the route expects no loot to lift.
 */
export function breakEvenMarkup(
	route: SkillingRouteForecast,
	cheapestCost: number,
	against: StoredRoute,
): BreakEven | null {
	const loot = route.cycledPed - route.pedLost;
	if (!(loot > 0)) return null;
	const percent = 100 * (1 + (route.pedLost - cheapestCost) / loot);
	return {
		percent: Math.min(percent, MAX_LOOT_MARKUP),
		beyond: percent > MAX_LOOT_MARKUP,
		against,
	};
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
			cost: null,
			premium: null,
			premiumPerHour: null,
			overkill: null,
			breakEven: null,
		};
	});
	const ready = rows.filter((row) => row.route?.status === 'ready');
	for (const row of ready) row.cost = routeCost(row.route as SkillingRouteForecast);
	const lead = ready.reduce<RouteRow | null>(
		(best, row) => (best === null || (row.cost ?? 0) < (best.cost ?? 0) ? row : best),
		null,
	);
	const cheapest = lead?.cost ?? 0;
	for (const row of ready) {
		const route = row.route as SkillingRouteForecast;
		if (ready.length > 1) {
			row.premium = (row.cost ?? 0) - cheapest;
			row.premiumPerHour = route.hours && route.hours > 0 ? row.premium / route.hours : null;
			if (lead && row !== lead && row.premium > 0) {
				row.breakEven = breakEvenMarkup(route, cheapest, lead.stored);
			}
		}
		row.overkill = overkillReading(route, row.premium, cheapest);
	}
	return rows;
}

/** Below this share overkill is a last-hit rounding, not something a
 * different weapon would change: the detail says nothing of it. */
export const MEANINGFUL_OVERKILL = 0.01;

/** A priced route's overkill, read against its premium over the cheapest
 * route's `cheapestPed` (a null premium: the only route that answers). */
export function overkillReading(
	route: SkillingRouteForecast,
	premium: number | null,
	cheapestPed: number,
): OverkillReading {
	const share = Math.min(Math.max(route.overkillShare, 0), 1);
	const cost = routeCost(route);
	const ped = cost * share;
	const base = { share, ped, fittedPed: cost - ped };
	if (cost <= 0) return { ...base, kind: 'no_cost' };
	if (premium === null || premium <= 0) return { ...base, kind: 'headroom' };
	if (ped >= premium) return { ...base, kind: 'covers_premium', premium, cheapestPed };
	return { ...base, kind: 'part_of_premium', premium };
}

/** HP fired past the kill on each kill of a mob of `hp`: what the damage
 * dealt beyond the kill comes to, when overkill is `share` of it. */
export function hpPastTheKill(hp: number, share: number): number {
	return share < 1 ? (hp * share) / (1 - share) : 0;
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
