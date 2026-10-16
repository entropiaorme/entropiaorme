import { describe, expect, it } from 'vitest';
import { forecastResult, routeForecast, STORED_ROUTE } from './__fixtures__/skilling';
import { isStoredRoute, routePick, routeRows, routeStatusLabel, sameRoute } from './routesView';

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

	it('lists every stored route before the forecast answers', () => {
		const rows = routeRows(null, [STORED_ROUTE]);
		expect(rows).toHaveLength(1);
		expect(rows[0].route).toBeNull();
		expect(rows[0].status).toBeNull();
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

	it('recognises only a well-formed stored route', () => {
		expect(isStoredRoute(STORED_ROUTE)).toBe(true);
		expect(isStoredRoute({ ...STORED_ROUTE, weaponId: 7 })).toBe(false);
		expect(isStoredRoute(null)).toBe(false);
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
