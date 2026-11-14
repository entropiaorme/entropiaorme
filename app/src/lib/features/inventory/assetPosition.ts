/**
 * An asset held by quantity: a position whose TT and markup paid are totals
 * over its units, built up from purchases.
 *
 * The average is what the position's units cost each. Buying more moves it;
 * selling part never does, because units leave at whatever the average is at
 * that moment. That is the whole of the arithmetic, and it lives here so the
 * table, the purchase form, and the sale form cannot disagree about it.
 */

import type { AssetPurchase, InventoryItem } from '$lib/api/commands.gen';

/** A total shared across units, or null when there are none to share it. */
export function perUnit(total: number, quantity: number): number | null {
	return quantity > 0 ? total / quantity : null;
}

/** Markup paid in the game's percentage form (100 is TT), or null without TT. */
export function markupPct(ttValue: number, markupPaid: number): number | null {
	return ttValue > 0 ? 100 + (markupPaid / ttValue) * 100 : null;
}

/** Whether a position holds more than one unit, so per-unit figures say
 * something its totals do not. */
export function isStack(item: Pick<InventoryItem, 'quantity'>): boolean {
	return Math.abs(item.quantity - 1) > 1e-9;
}

/** What a purchase records, as entered. */
export interface PurchaseFigures {
	quantity: number;
	ttValue: number;
	markupPaid: number;
}

/** A position's size and average, before or after a purchase. */
export interface PositionAverage {
	quantity: number;
	markupPerUnit: number | null;
	costPerUnit: number | null;
}

function average(quantity: number, ttValue: number, markupPaid: number): PositionAverage {
	return {
		quantity,
		markupPerUnit: perUnit(markupPaid, quantity),
		costPerUnit: perUnit(ttValue + markupPaid, quantity),
	};
}

/** The position as it stands. */
export function currentAverage(item: InventoryItem): PositionAverage {
	return average(item.quantity, item.ttValue, item.markupPaid);
}

/**
 * The position once `figures` are recorded: a new purchase when `replacing`
 * is absent, or the correction of that purchase when it is given.
 */
export function averageAfter(
	item: InventoryItem,
	figures: PurchaseFigures,
	replacing: AssetPurchase | null = null,
): PositionAverage {
	const before = replacing ?? { quantity: 0, ttValue: 0, markupPaid: 0 };
	return average(
		item.quantity + figures.quantity - before.quantity,
		item.ttValue + figures.ttValue - before.ttValue,
		item.markupPaid + figures.markupPaid - before.markupPaid,
	);
}

/** Whether the asset's own form may change its figures: only while it is
 * still the one purchase that made it, and nothing has drawn on that. */
export function figuresEditable(item: InventoryItem): boolean {
	return item.purchases.length === 1 && item.purchases[0].editable;
}

/** Why the asset's own form cannot change its figures, or null when it can. */
export function figuresLockedReason(item: InventoryItem): string | null {
	if (figuresEditable(item)) return null;
	if (item.purchases.length > 1) return 'Its figures come from its purchases; change one of those.';
	return 'Units of it were sold, listed, or converted, so what it cost is fixed.';
}

/** The asset with its figures scaled to `quantity` of its units, as a part
 * of it leaves at the average cost. */
export function shareOf(
	item: InventoryItem,
	quantity: number,
): { ttValue: number; markupPaid: number } {
	const share = item.quantity > 0 ? Math.min(quantity, item.quantity) / item.quantity : 0;
	return { ttValue: item.ttValue * share, markupPaid: item.markupPaid * share };
}
