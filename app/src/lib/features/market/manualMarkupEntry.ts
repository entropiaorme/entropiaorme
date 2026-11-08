/**
 * The pure half of entering an item's markup by hand: reading what the
 * player typed, what it amounts to as a markup of TT, and what to start
 * the field with. The popover that hosts it is `ManualMarkupEditor`.
 *
 * Two forms, the way the game quotes markup: a percentage of TT for items
 * that stack, and PED above TT per unit for items that do not (`+2.50`).
 */

import type { MarketHarvestItem, MarketManualMarkupKind } from '$lib/api';

export type EntryKind = MarketManualMarkupKind;

export type ParsedEntry = { ok: true; value: number } | { ok: false; error: string };

/** Read a typed value, tolerating the decorations the game's own quotes
 * carry: a trailing `%` or `PED`, a leading `+`, a comma decimal. */
export function parseEntry(kind: EntryKind, text: string): ParsedEntry {
	const cleaned = text
		.trim()
		.replace(/^\+/, '')
		.replace(/\s*(%|ped)$/i, '')
		.replace(',', '.')
		.trim();
	if (cleaned === '') return { ok: false, error: 'Enter a markup' };
	if (!/^\d*\.?\d+$/.test(cleaned)) return { ok: false, error: 'Enter a number' };
	const value = Number(cleaned);
	if (kind === 'percent') {
		if (value <= 0) return { ok: false, error: 'A markup is above 0%' };
		if (value > 1_000_000) return { ok: false, error: 'That markup is out of range' };
	} else if (value > 10_000_000) {
		return { ok: false, error: 'That markup is out of range' };
	}
	return { ok: true, value };
}

/** The entry as a markup of TT, for the preview beside the field: null for
 * a PED amount on an item with no TT to be a markup over. */
export function entryMarkupPct(
	kind: EntryKind,
	value: number,
	unitTtPed: number | null,
): number | null {
	if (kind === 'percent') return value;
	if (unitTtPed === null || unitTtPed <= 0) return null;
	return ((unitTtPed + value) / unitTtPed) * 100;
}

/** An item's TT per unit, from the row it is shown in. */
export function unitTt(quantity: number, ttValue: number): number | null {
	return quantity > 0 ? ttValue / quantity : null;
}

/** The form a fresh entry starts in: the player's last entry, else a
 * percentage (what most loot is quoted in). */
export function initialKind(market: MarketHarvestItem | undefined): EntryKind {
	return market?.manual?.kind ?? 'percent';
}

/** What the field starts with: the player's standing entry in its own
 * form, else the market's percentage as a starting point, else empty. */
export function initialText(market: MarketHarvestItem | undefined, kind: EntryKind): string {
	const manual = market?.manual ?? null;
	if (manual && manual.kind === kind) return trimNumber(manual.value);
	if (kind === 'percent' && market?.markupPct != null) return trimNumber(market.markupPct);
	return '';
}

function trimNumber(value: number): string {
	return String(Math.round(value * 10_000) / 10_000);
}
