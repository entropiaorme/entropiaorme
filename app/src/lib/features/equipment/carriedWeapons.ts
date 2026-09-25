/**
 * The weapons the player carries, as weapon attribution sees them: every
 * weapon bound to a hotbar slot plus the ones carried without a hotkey,
 * each with the damage band its hits are checked against.
 *
 * A hotbar press says which weapon is in hand; each carried weapon's band
 * checks it. A hit only one other carried weapon explains is recorded to
 * that weapon, and a hit several explain (or none) is recorded without a
 * price. Where two bands overlap, only the hotbar can tell the weapons
 * apart, which is what the shared ranges below say.
 */

import type { Equipment } from '$lib/types';
import type { Hotbar } from '$lib/types/settings';

/** A critical hit reaches at most this multiple of a weapon's maximum. */
export const CRITICAL_REACH = 3;

/** The hotbar's slot keys, in the order the game shows them. */
export const HOTBAR_SLOT_ORDER = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'] as const;

export interface WeaponBand {
	id: string;
	name: string;
	/** The regular-hit band at full skill. */
	min: number;
	max: number;
	/** The highest a critical can reach. */
	critMax: number;
	/** How the player reaches it: a hotbar slot key, or none. */
	slot: string | null;
}

export interface SharedRange {
	first: string;
	second: string;
	min: number;
	max: number;
}

function weaponById(equipment: Equipment[], id: number | string | null | undefined) {
	if (id === null || id === undefined) return null;
	return equipment.find((item) => item.type === 'weapon' && String(item.id) === String(id)) ?? null;
}

/** The carried weapons in attribution order: the hotbar's in slot order,
 * then those carried without a slot, each once, with the slot they sit in. */
export function carriedWeapons(
	equipment: Equipment[],
	hotbar: Hotbar,
	carriedIds: readonly number[],
): { weapon: Equipment; slot: string | null }[] {
	const seen = new Set<string>();
	const out: { weapon: Equipment; slot: string | null }[] = [];
	for (const slot of HOTBAR_SLOT_ORDER) {
		const weapon = weaponById(equipment, hotbar[slot]);
		if (weapon && !seen.has(weapon.id)) {
			seen.add(weapon.id);
			out.push({ weapon, slot });
		}
	}
	for (const id of carriedIds) {
		const weapon = weaponById(equipment, id);
		if (weapon && !seen.has(weapon.id)) {
			seen.add(weapon.id);
			out.push({ weapon, slot: null });
		}
	}
	return out;
}

/** The weapons still offered for carrying without a hotkey: every weapon not
 * already on a hotbar slot or in the carried list, by name. */
export function addableWeapons(
	equipment: Equipment[],
	hotbar: Hotbar,
	carriedIds: readonly number[],
): Equipment[] {
	const taken = new Set(carriedWeapons(equipment, hotbar, carriedIds).map((c) => c.weapon.id));
	return equipment
		.filter((item) => item.type === 'weapon' && !taken.has(item.id))
		.sort((a, b) => a.name.localeCompare(b.name));
}

/** Each carried weapon's band; those with no damage figure are listed apart,
 * because evidence can neither name nor contradict them. */
export function weaponBands(carried: { weapon: Equipment; slot: string | null }[]): {
	banded: WeaponBand[];
	bandless: Equipment[];
} {
	const banded: WeaponBand[] = [];
	const bandless: Equipment[] = [];
	for (const { weapon, slot } of carried) {
		const { damageMin, damageMax } = weapon;
		if (damageMin == null || damageMax == null || damageMax <= 0) {
			bandless.push(weapon);
			continue;
		}
		banded.push({
			id: weapon.id,
			name: weapon.name,
			min: damageMin,
			max: damageMax,
			critMax: damageMax * CRITICAL_REACH,
			slot,
		});
	}
	return { banded, bandless };
}

/** Every pair of regular bands that overlap, and where. A hit in a shared
 * range is priced to the hotbar's weapon when it is one of the pair, and
 * left unpriced otherwise. */
export function sharedRanges(bands: WeaponBand[]): SharedRange[] {
	const shared: SharedRange[] = [];
	for (let i = 0; i < bands.length; i++) {
		for (let j = i + 1; j < bands.length; j++) {
			const min = Math.max(bands[i].min, bands[j].min);
			const max = Math.min(bands[i].max, bands[j].max);
			if (min <= max) {
				shared.push({ first: bands[i].name, second: bands[j].name, min, max });
			}
		}
	}
	return shared;
}

/** A damage range as the game prints damage. */
export function formatRange(min: number, max: number): string {
	return `${min.toFixed(1)}–${max.toFixed(1)}`;
}

/** Axis ticks for a chart spanning 0 to `top`: four equal steps, rounded to
 * a readable number. */
export function axisTicks(top: number): number[] {
	if (top <= 0) return [0];
	const rough = top / 4;
	const magnitude = 10 ** Math.floor(Math.log10(rough));
	const step = [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((s) => s >= rough) ?? rough;
	const ticks: number[] = [];
	for (let value = 0; value <= top + 1e-9; value += step) ticks.push(Number(value.toFixed(6)));
	if (ticks[ticks.length - 1] < top)
		ticks.push(Number((ticks[ticks.length - 1] + step).toFixed(6)));
	return ticks;
}
