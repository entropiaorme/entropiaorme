/**
 * The overlay's equipment picker: what can be declared in hand without a
 * hotbar press. Picking a row declares it exactly as its press would, so
 * the rows are the items a press could declare: the carried weapons (the
 * candidates weapon attribution chooses among), every healer, and every
 * harvesting tool. Consumables are not held, so they start doses from
 * their own menu instead.
 */

import type { AppSettings, EquipmentSummary } from '$lib/api/commands.gen';
import { hotbarFromSettings, hotbarPresetsFromSettings } from '$lib/api/settings';
import { HOTBAR_SLOT_ORDER } from './carriedWeapons';

export type DeclarableKind = 'weapon' | 'healing' | 'tool';

export interface DeclarableOption {
	equipmentId: number;
	name: string;
	kind: DeclarableKind;
	/** The live preset's key that declares it in game, or none. */
	hotbarSlot: string | null;
}

const KIND_ORDER: readonly DeclarableKind[] = ['weapon', 'healing', 'tool'];

/** The declarable rows: weapons, then healers, then harvesting tools; within
 * each, the items on the live hotbar first in key order, then the rest by
 * name. Weapons are limited to the carried set (any preset's slots plus the
 * weapons carried without a key); with none carried, every weapon is
 * offered, so a fresh setup can still declare one. */
export function declarableOptions(
	library: EquipmentSummary[],
	settings: AppSettings,
): DeclarableOption[] {
	const live = hotbarFromSettings(settings);
	const slotOf = new Map<number, string>();
	for (const slot of HOTBAR_SLOT_ORDER) {
		const id = live[slot];
		if (id !== null && id !== undefined && !slotOf.has(id)) slotOf.set(id, slot);
	}
	const carried = new Set<number>(settings.carriedWeaponIds);
	for (const preset of hotbarPresetsFromSettings(settings)) {
		for (const id of Object.values(preset)) if (id !== null) carried.add(id);
	}
	const weaponsCarried = library.some(
		(item) => item.type === 'weapon' && carried.has(Number(item.id)),
	);

	const options: DeclarableOption[] = [];
	for (const item of library) {
		const equipmentId = Number(item.id);
		if (!Number.isFinite(equipmentId)) continue;
		if (item.type === 'consumable') continue;
		if (item.type === 'weapon' && weaponsCarried && !carried.has(equipmentId)) continue;
		options.push({
			equipmentId,
			name: item.name,
			kind: item.type,
			hotbarSlot: slotOf.get(equipmentId) ?? null,
		});
	}
	const keyRank = (option: DeclarableOption) =>
		option.hotbarSlot === null
			? HOTBAR_SLOT_ORDER.length
			: HOTBAR_SLOT_ORDER.indexOf(option.hotbarSlot as (typeof HOTBAR_SLOT_ORDER)[number]);
	return options.sort(
		(a, b) =>
			KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind) ||
			keyRank(a) - keyRank(b) ||
			a.name.localeCompare(b.name),
	);
}

/** The row's kind as the menu labels it. */
export function declarableKindLabel(kind: DeclarableKind): string {
	return kind === 'weapon' ? 'Weapon' : kind === 'healing' ? 'Healer' : 'Tool';
}
