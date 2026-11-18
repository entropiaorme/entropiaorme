import { describe, expect, it } from 'vitest';
import { sourceForecast } from './__fixtures__/skilling';
import { sessionRows } from './sessionsView';

const expected = (ttCost: number) => ({ ttCost, weaponReturn: 0.92, coverage: 1 });

describe('sessionRows', () => {
	it('prices each answering session over the cheapest expected loss, in the forecast order', () => {
		const rows = sessionRows([
			sourceForecast({ definitionId: 1, ttCost: 9, expected: expected(4), hours: 2 }),
			sourceForecast({ definitionId: 2, ttCost: 5, expected: expected(10), hours: 3 }),
			sourceForecast({ definitionId: 3, status: 'does_not_train', ttCost: 0 }),
		]);
		expect(rows.map((row) => row.source.definitionId)).toEqual([1, 2, 3]);
		expect(rows[0].premium).toBe(0);
		// The recorded losses run the other way; the premium follows the model.
		expect(rows[1].premium).toBe(6);
		// Six PED more over three hours of that session: two PED an hour.
		expect(rows[1].premiumPerHour).toBe(2);
		expect(rows[2].premium).toBeNull();
	});

	it('leaves a session the model cannot price out of the comparison', () => {
		const rows = sessionRows([
			sourceForecast({ definitionId: 1, expected: expected(4) }),
			sourceForecast({ definitionId: 2, expected: expected(7) }),
			sourceForecast({ definitionId: 3, ttCost: 1, expected: null }),
		]);
		expect(rows.map((row) => row.premium)).toEqual([0, 3, null]);
	});

	it('leaves the premium out when only one session answers', () => {
		const rows = sessionRows([sourceForecast({ expected: expected(4) })]);
		expect(rows[0].premium).toBeNull();
		expect(rows[0].premiumPerHour).toBeNull();
	});
});
