/**
 * The live Loot Composition widget's rows: the session's loot ranked by TT,
 * the share bar's segments (the top items, the rest pooled), and each
 * item's live market valuation beside it.
 *
 * The bar always pools past the top items so it stays legible; the list
 * can expand the pool into its items, because every item carries its own
 * markup to read and correct.
 */

import type { LootEntry } from '$lib/api';
import type { TreeCuttingItem } from '$lib/features/analytics/treeCuttingModel.svelte';

/** Distinct hues tuned to the dark theme, cohesive with the accent (sky)
 * and positive (teal) tokens. */
export const COMPOSITION_PALETTE = [
	'#38bdf8',
	'#2dd4bf',
	'#fbbf24',
	'#a78bfa',
	'#f472b6',
	'#34d399',
	'#60a5fa',
];
export const OTHER_COLOR = '#475569';
export const MAX_VISIBLE = 7;
export const OTHER_KEY = '__other__';

export interface CompositionRow {
	key: string;
	name: string;
	quantity: number;
	ttValue: number;
	sharePct: number;
	color: string;
	rank: number;
	isOther: boolean;
	/** Items the row pools (1 for an item). */
	count: number;
	/** The item's live valuation; null for the pool, or before the market
	 * signals are in. */
	valued: TreeCuttingItem | null;
}

export interface Composition {
	/** The share bar: the top items and one pooled segment. */
	segments: CompositionRow[];
	/** The list: the bar's rows, or every item when expanded. */
	rows: CompositionRow[];
	/** How many items the pool holds (0 when nothing is pooled). */
	pooled: number;
}

export function composeLoot(
	loot: readonly LootEntry[],
	valued: readonly TreeCuttingItem[] | null,
	expanded: boolean,
): Composition {
	const sorted = [...loot].sort((a, b) => b.ttValue - a.ttValue);
	const total = sorted.reduce((sum, item) => sum + item.ttValue, 0);
	if (sorted.length === 0 || total <= 0) return { segments: [], rows: [], pooled: 0 };
	const byName = new Map((valued ?? []).map((item) => [item.name, item]));

	const itemRows = sorted.map(
		(item, index): CompositionRow => ({
			key: item.name,
			name: item.name,
			quantity: item.quantity,
			ttValue: item.ttValue,
			sharePct: (item.ttValue / total) * 100,
			color:
				index < MAX_VISIBLE ? COMPOSITION_PALETTE[index % COMPOSITION_PALETTE.length] : OTHER_COLOR,
			rank: index + 1,
			isOther: false,
			count: 1,
			valued: byName.get(item.name) ?? null,
		}),
	);
	const top = itemRows.slice(0, MAX_VISIBLE);
	const rest = itemRows.slice(MAX_VISIBLE);
	if (rest.length === 0) return { segments: top, rows: top, pooled: 0 };

	const restTt = rest.reduce((sum, item) => sum + item.ttValue, 0);
	const pool: CompositionRow = {
		key: OTHER_KEY,
		name: `+${rest.length} more`,
		quantity: rest.reduce((sum, item) => sum + item.quantity, 0),
		ttValue: restTt,
		sharePct: (restTt / total) * 100,
		color: OTHER_COLOR,
		rank: MAX_VISIBLE + 1,
		isOther: true,
		count: rest.length,
		valued: null,
	};
	const segments = [...top, pool];
	return { segments, rows: expanded ? itemRows : segments, pooled: rest.length };
}

/** The bar segment an item row lights up: its own, or the pool's. */
export function segmentKeyOf(row: CompositionRow): string {
	return row.rank > MAX_VISIBLE && !row.isOther ? OTHER_KEY : row.key;
}
