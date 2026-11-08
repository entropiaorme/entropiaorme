import { describe, expect, it } from 'vitest';
import type { LootEntry } from '$lib/api';
import { composeLoot, MAX_VISIBLE, OTHER_KEY, segmentKeyOf } from './lootComposition';

const loot = (count: number): LootEntry[] =>
	Array.from({ length: count }, (_, index) => ({
		name: `Item ${index + 1}`,
		quantity: 1,
		ttValue: count - index,
	}));

describe('composeLoot', () => {
	it('ranks by TT and pools past the top items in the bar', () => {
		const composition = composeLoot(loot(10), null, false);
		expect(composition.segments).toHaveLength(MAX_VISIBLE + 1);
		expect(composition.segments.at(-1)).toMatchObject({ key: OTHER_KEY, count: 3, ttValue: 6 });
		expect(composition.rows).toEqual(composition.segments);
		expect(composition.pooled).toBe(3);
	});

	it('expands the list into every item while the bar stays pooled', () => {
		const composition = composeLoot(loot(10), null, true);
		expect(composition.rows).toHaveLength(10);
		expect(composition.rows.some((row) => row.isOther)).toBe(false);
		expect(composition.segments.at(-1)?.key).toBe(OTHER_KEY);
		// A pooled item lights the bar's pool segment.
		expect(segmentKeyOf(composition.rows[9])).toBe(OTHER_KEY);
		expect(segmentKeyOf(composition.rows[0])).toBe('Item 1');
	});

	it('pools nothing when the loot fits', () => {
		const composition = composeLoot(loot(3), null, false);
		expect(composition.pooled).toBe(0);
		expect(composition.rows).toHaveLength(3);
	});

	it('is empty with no loot TT', () => {
		expect(composeLoot([{ name: 'Nothing', quantity: 1, ttValue: 0 }], null, false).rows).toEqual(
			[],
		);
	});
});
