/**
 * The live Cycled breakdown, grouped the way the player reads their spend:
 * weapons, then enhancers, healing, armour, consumables, and harvesting,
 * each item under its bucket with its share of the session's Cycled.
 *
 * The backend builds the lines and the total from the same in-memory
 * figures, so the lines always sum to the Cycled the tile shows; this
 * module only orders and labels them.
 */

import type { CycledBucket, CycledLine } from '$lib/api';

export interface BreakdownItem {
	label: string;
	costPed: number;
	/** The item's share of the session's Cycled, 0-100. */
	sharePct: number;
}

export interface BreakdownGroup {
	bucket: CycledBucket;
	title: string;
	items: BreakdownItem[];
	subtotalPed: number;
}

const BUCKET_ORDER: CycledBucket[] = [
	'weapon',
	'enhancer',
	'healing',
	'armour',
	'consumable',
	'harvesting',
];

const BUCKET_TITLE: Record<CycledBucket, string> = {
	weapon: 'Weapons',
	enhancer: 'Enhancers',
	healing: 'Healing',
	armour: 'Armour and plates',
	consumable: 'Consumables',
	harvesting: 'Harvesting',
};

/**
 * Group the breakdown lines by bucket, in bucket order, each group's items
 * dearest first. Buckets with no spend are left out.
 */
export function groupCycledBreakdown(
	lines: readonly CycledLine[],
	totalPed: number,
): BreakdownGroup[] {
	const share = (cost: number) => (totalPed > 0 ? (cost / totalPed) * 100 : 0);
	return BUCKET_ORDER.flatMap((bucket) => {
		const items = lines
			.filter((line) => line.bucket === bucket)
			.map((line) => ({ label: line.label, costPed: line.costPed, sharePct: share(line.costPed) }))
			.sort((a, b) => b.costPed - a.costPed);
		if (items.length === 0) return [];
		return [
			{
				bucket,
				title: BUCKET_TITLE[bucket],
				items,
				subtotalPed: items.reduce((sum, item) => sum + item.costPed, 0),
			},
		];
	});
}

/** Whether any armour cost has reached the session yet: armour arrives
 * when a repair or Trade Terminal reading is recorded, never per hit, so
 * the tip says so while none has. */
export function hasArmourLine(lines: readonly CycledLine[]): boolean {
	return lines.some((line) => line.bucket === 'armour');
}
