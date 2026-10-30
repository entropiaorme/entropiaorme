import { describe, expect, it } from 'vitest';
import { forecastResult, routeForecast, STORED_ROUTE } from './__fixtures__/skilling';
import {
	breakEvenMarkup,
	hpPastTheKill,
	isStoredRoute,
	MAX_LOOT_MARKUP,
	MIN_LOOT_MARKUP,
	overkillReading,
	readRoutePercent,
	routePick,
	routeRows,
	routeStatusLabel,
	sameRoute,
} from './routesView';

const ROBBER = {
	...STORED_ROUTE,
	species: 'Bank Robber',
	maturity: 'Default',
	display: 'Default Bank Robber',
};

describe('routeRows', () => {
	it('keeps the player order and prices each route over the cheapest that answers', () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					routeForecast({ index: 0, pedLost: 3.6, hours: 2 }),
					routeForecast({ index: 1, pedLost: 1.6, mob: 'Default Bank Robber' }),
				],
			}),
			[STORED_ROUTE, ROBBER],
		);
		expect(rows.map((row) => row.key)).toEqual(['route:0', 'route:1']);
		expect(rows[1].premium).toBe(0);
		expect(rows[0].premium).toBeCloseTo(2);
		// Two PED more over two hours of this route: one PED an hour.
		expect(rows[0].premiumPerHour).toBeCloseTo(1);
	});

	it('leaves the premium out with only one answering route', () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					routeForecast({ status: 'unknown_mob', pedLost: 0, hours: null }),
					routeForecast({ index: 1 }),
				],
			}),
			[STORED_ROUTE, STORED_ROUTE],
		);
		expect(rows.map((row) => row.status)).toEqual(['unknown_mob', 'ready']);
		expect(rows.every((row) => row.premium === null)).toBe(true);
	});

	it('compares a route with an expected markup on its loss after it', () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					// The grind at TT: 0.83 PED lost.
					routeForecast({ index: 0, cycledPed: 50, pedLost: 0.83, hours: 4 }),
					// An instance mob: dearer at TT, but its 104% loot nets it below.
					routeForecast({
						index: 1,
						cycledPed: 100,
						pedLost: 4.6,
						markup: 3.82,
						netCost: 0.78,
						hours: 2,
					}),
				],
			}),
			[STORED_ROUTE, { ...ROBBER, lootMarkupPercent: 104 }],
		);
		expect(rows[1].cost).toBeCloseTo(0.78);
		expect(rows[1].premium).toBe(0);
		expect(rows[1].breakEven).toBeNull();
		expect(rows[0].premium).toBeCloseTo(0.05);
		// The grind would match it at 0.05 PED of markup on its 49.17 PED of loot.
		expect(rows[0].breakEven?.percent).toBeCloseTo(100 * (1 + 0.05 / 49.17));
		expect(rows[0].breakEven?.against).toEqual({ ...ROBBER, lootMarkupPercent: 104 });
	});

	it('reads a markup that outweighs the loss as the cheapest, with no overkill cost', () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					routeForecast({ index: 0, pedLost: 3.6 }),
					routeForecast({ index: 1, pedLost: 4, markup: 6, netCost: -2, overkillShare: 0.3 }),
				],
			}),
			[STORED_ROUTE, ROBBER],
		);
		expect(rows[1].premium).toBe(0);
		expect(rows[0].premium).toBeCloseTo(5.6);
		expect(rows[1].overkill?.kind).toBe('no_cost');
	});

	it('lists every stored route before the forecast answers', () => {
		const rows = routeRows(null, [STORED_ROUTE]);
		expect(rows).toHaveLength(1);
		expect(rows[0].route).toBeNull();
		expect(rows[0].status).toBeNull();
	});
});

describe('break-even markup', () => {
	it('lifts the expected loot until it nets the gap', () => {
		// 10 PED lost on 100 cycled: 90 of loot; the cheapest costs 1.
		const reading = breakEvenMarkup(routeForecast({ cycledPed: 100, pedLost: 10 }), 1, ROBBER);
		expect(reading?.percent).toBeCloseTo(110);
		expect(reading?.beyond).toBe(false);
	});

	it('says when even the highest markup would not close the gap', () => {
		const reading = breakEvenMarkup(routeForecast({ cycledPed: 10, pedLost: 9.9 }), 0, ROBBER);
		expect(reading).toMatchObject({ percent: MAX_LOOT_MARKUP, beyond: true });
	});

	it('has nothing to lift without loot', () => {
		expect(breakEvenMarkup(routeForecast({ cycledPed: 5, pedLost: 5 }), 0, ROBBER)).toBeNull();
	});
});

describe('overkill', () => {
	it("reads the cheapest route's overkill as headroom, however large", () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					routeForecast({ index: 0, pedLost: 3770, overkillShare: 0.22 }),
					routeForecast({ index: 1, pedLost: 2160, overkillShare: 0.67 }),
				],
			}),
			[STORED_ROUTE, ROBBER],
		);
		const cheapest = rows[1].overkill;
		expect(cheapest?.kind).toBe('headroom');
		expect(cheapest?.ped).toBeCloseTo(1447.2);
		expect(cheapest?.fittedPed).toBeCloseTo(712.8);
		// The dearer route's overkill (829.4) is only part of its 1,610 premium.
		expect(rows[0].overkill).toMatchObject({ kind: 'part_of_premium', premium: 1610 });
		expect(rows[0].overkill?.ped).toBeCloseTo(829.4);
	});

	it('says when overkill alone outweighs the premium', () => {
		const reading = overkillReading(routeForecast({ pedLost: 100, overkillShare: 0.5 }), 40, 60);
		expect(reading).toMatchObject({
			kind: 'covers_premium',
			ped: 50,
			fittedPed: 50,
			premium: 40,
			cheapestPed: 60,
		});
	});

	it('reads the only route as headroom, and leaves an unpriced row without', () => {
		const rows = routeRows(
			forecastResult({
				routes: [
					routeForecast({ index: 0, pedLost: 10, overkillShare: 0.3 }),
					routeForecast({ index: 1, status: 'unknown_mob', pedLost: 0, overkillShare: 0 }),
				],
			}),
			[STORED_ROUTE, ROBBER],
		);
		expect(rows[0].overkill?.kind).toBe('headroom');
		expect(rows[1].overkill).toBeNull();
	});

	it('turns the share back into HP dealt past each kill', () => {
		// Two thirds wasted on a 2 HP mob: 4 HP past each kill.
		expect(hpPastTheKill(2, 2 / 3)).toBeCloseTo(4);
		expect(hpPastTheKill(100, 0)).toBe(0);
	});
});

describe('stored routes', () => {
	it('asks the forecast for the weapon on the maturity', () => {
		expect(routePick(STORED_ROUTE)).toEqual({
			setup: { weapon: { catalogId: 'sword' } },
			species: 'Carabok',
			maturity: 'Puny',
		});
	});

	it('carries a markup and a hunting share only when set', () => {
		const pick = routePick({ ...STORED_ROUTE, lootMarkupPercent: 104, huntingSharePercent: 77 });
		expect(pick).toMatchObject({ lootMarkupPercent: 104, huntingSharePercent: 77 });
		const plain = routePick({ ...STORED_ROUTE, lootMarkupPercent: null });
		expect(plain.lootMarkupPercent).toBeUndefined();
		expect(JSON.parse(JSON.stringify(plain))).not.toHaveProperty('lootMarkupPercent');
		// A hand-edited preference past the range settles inside it.
		expect(
			routePick({ ...STORED_ROUTE, lootMarkupPercent: 50, huntingSharePercent: 400 }),
		).toMatchObject({ lootMarkupPercent: 100, huntingSharePercent: 100 });
	});

	it('recognises only a well-formed stored route', () => {
		expect(isStoredRoute(STORED_ROUTE)).toBe(true);
		expect(
			isStoredRoute({ ...STORED_ROUTE, lootMarkupPercent: 104, huntingSharePercent: null }),
		).toBe(true);
		expect(isStoredRoute({ ...STORED_ROUTE, lootMarkupPercent: '104' })).toBe(false);
		expect(isStoredRoute({ ...STORED_ROUTE, weaponId: 7 })).toBe(false);
		expect(isStoredRoute(null)).toBe(false);
	});

	it('reads a typed percentage: blank clears, out of range settles, junk keeps', () => {
		expect(readRoutePercent('104', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBe(104);
		expect(readRoutePercent(' 102.5 ', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBe(102.5);
		expect(readRoutePercent('', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBeNull();
		expect(readRoutePercent('90', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBe(100);
		expect(readRoutePercent('5000', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBe(1000);
		expect(readRoutePercent('abc', MIN_LOOT_MARKUP, MAX_LOOT_MARKUP)).toBeUndefined();
	});

	it('knows the same weapon on the same maturity', () => {
		expect(sameRoute(STORED_ROUTE, { ...STORED_ROUTE, display: 'renamed' })).toBe(true);
		expect(sameRoute(STORED_ROUTE, ROBBER)).toBe(false);
	});

	it('says why a route does not answer', () => {
		expect(routeStatusLabel('ready')).toBe('');
		expect(routeStatusLabel('unknown_weapon')).toBe('Weapon not in the catalogue');
		expect(routeStatusLabel('does_not_train')).toBe('Does not train this');
	});
});
