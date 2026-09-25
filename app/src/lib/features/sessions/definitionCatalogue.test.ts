import { describe, expect, it } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import { filterDefinitions, sortDefinitions } from './definitionCatalogue';

function definition(id: string, name: string): SessionDefinition {
	return {
		id,
		name,
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: false,
		isActive: true,
		instanceCount: 0,
		createdAt: 0,
		updatedAt: null,
		roster: [],
	};
}

describe('definition catalogue', () => {
	it('sorts names case-insensitively and naturally, with id as a stable tie-break', () => {
		const sorted = sortDefinitions([
			definition('9', 'Season 10'),
			definition('3', 'alpha'),
			definition('2', 'Alpha'),
			definition('8', 'Season 2'),
		]);

		expect(sorted.map((entry) => entry.id)).toEqual(['2', '3', '8', '9']);
	});

	it('filters by a trimmed, case-insensitive name fragment without reordering', () => {
		const definitions = [
			definition('1', 'ARIS Dailies'),
			definition('2', 'Cyrene Dailies'),
			definition('3', 'Tree Cutting'),
		];

		expect(filterDefinitions(definitions, '  DAIL  ').map((entry) => entry.id)).toEqual(['1', '2']);
		expect(filterDefinitions(definitions, '  ')).toBe(definitions);
	});
});
