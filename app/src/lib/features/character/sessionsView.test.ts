import { describe, expect, it } from 'vitest';
import { sourceForecast } from './__fixtures__/skilling';
import { sessionRows } from './sessionsView';

describe('sessionRows', () => {
	it('prices each answering session over the cheapest, in the forecast order', () => {
		const rows = sessionRows([
			sourceForecast({ definitionId: 1, ttCost: 4, hours: 2 }),
			sourceForecast({ definitionId: 2, ttCost: 10, hours: 3 }),
			sourceForecast({ definitionId: 3, status: 'does_not_train', ttCost: 0 }),
		]);
		expect(rows.map((row) => row.source.definitionId)).toEqual([1, 2, 3]);
		expect(rows[0].premium).toBe(0);
		expect(rows[1].premium).toBe(6);
		// Six PED more over three hours of that session: two PED an hour.
		expect(rows[1].premiumPerHour).toBe(2);
		expect(rows[2].premium).toBeNull();
	});

	it('leaves the premium out when only one session answers', () => {
		const rows = sessionRows([sourceForecast()]);
		expect(rows[0].premium).toBeNull();
		expect(rows[0].premiumPerHour).toBeNull();
	});
});
