/**
 * What the equipment page says while the tool in hand is read off the game
 * screen, as pure functions over the library.
 *
 * The screen names the tool held, so the library's holdable entries
 * (weapons, healing tools, harvesting tools) are switched on or off rather
 * than carried or bound to a key: the entries on are the ones carried, and
 * the reader matches what the bar shows against them. Entries the bar
 * cannot tell apart (one tool kept in two set-ups, or names the game cuts
 * short alike) are flagged on each member, and asked to keep one on. The
 * hotbar keys only start consumables and count heals; a weapon or
 * harvesting tool already on a slot is kept there for when the reading is
 * turned off.
 */

import type { Equipment } from '$lib/types';
import { listNames } from '$lib/utils/format';
import type { CarriedWeapon } from './carriedWeapons';

type Kind = Equipment['type'];

/** Whether an entry is something held, which the bar can name. */
export function isHoldable(kind: Kind): boolean {
	return kind === 'weapon' || kind === 'healing' || kind === 'tool';
}

/** What an entry's flag says when the bar cannot tell it from others. */
export interface LookalikeNote {
	/** "Looks the same as <others> on the game's bar." */
	text: string;
	/** More than one of the group is on: keep one. */
	leaveOne: boolean;
}

/** An entry as the player tells it apart from the ones the bar cannot:
 * the label the library read serves for it (the one rule the overlay names
 * such entries by too), else its name. */
export function entryLabel(labels: Readonly<Record<string, string>>, entry: Equipment): string {
	return labels[entry.id] ?? entry.name;
}

/**
 * The flag each member of a group the bar cannot tell apart carries, by
 * entry id. Groups name entry ids; an id the library does not hold is
 * skipped, and a group left with one member says nothing.
 */
export function lookalikeNotes(
	library: readonly Equipment[],
	groups: readonly (readonly string[])[],
	labels: Readonly<Record<string, string>>,
): Map<string, LookalikeNote> {
	const byId = new Map(library.map((entry) => [entry.id, entry]));
	const notes = new Map<string, LookalikeNote>();
	for (const group of groups) {
		const members = group.flatMap((id) => {
			const entry = byId.get(id);
			return entry ? [entry] : [];
		});
		if (members.length < 2) continue;
		const leaveOne = members.filter((entry) => entry.enabled).length > 1;
		for (const member of members) {
			const others = members
				.filter((other) => other.id !== member.id)
				.map((other) => entryLabel(labels, other));
			notes.set(member.id, {
				text: `Looks the same as ${listNames(others)} on the game's bar.`,
				leaveOne,
			});
		}
	}
	return notes;
}

/** The weapons carried while the screen is read: every weapon entry that is
 * on, none of them on a key, in the library's order. */
export function enabledCarried(equipment: readonly Equipment[]): CarriedWeapon[] {
	return equipment
		.filter((entry) => entry.type === 'weapon' && entry.enabled)
		.map((weapon) => ({ weapon, slot: null, parked: null }));
}

/** Whether a hotbar slot holding an entry of this kind is idle while the
 * screen is read: a consumable's key starts its dose and a healing tool's
 * key is a use of it when it is the one in hand, but the screen, not a key,
 * says which weapon or harvesting tool is held. */
export function slotIdleWhileReading(kind: Kind | null): boolean {
	return kind === 'weapon' || kind === 'tool';
}

/** The line at the top of the hotbar tab while the screen is read. */
export function hotbarReadingNote(options: {
	keyless: boolean;
	hotbarHooksEnabled: boolean;
}): string {
	const lead = 'The held tool is read from the screen.';
	if (options.keyless) {
		return `${lead} Keyless tracking is on for this machine: start consumables from the overlay.`;
	}
	if (!options.hotbarHooksEnabled) {
		return `${lead} Hotbar keys only start consumables and count heals, once the hotbar key listener is on in Settings.`;
	}
	return `${lead} Hotbar keys only start consumables and count heals.`;
}
