/**
 * The settings family, plus the overlay-position persistence. Thin
 * wrappers over the generated typed commands.
 */

import type { Hotbar, HotbarPreset } from '$lib/types/settings';
import type { AppSettings } from './commands.gen';
import * as commands from './commands.gen';

export const getSettings = commands.settingsGet;
/** How this machine reads keys, and whether it runs keyless. */
export const getInputStatus = commands.inputStatus;
export const updateSettings = commands.settingsUpdate;
export const getOverlayPosition = commands.settingsOverlayPosition;
export const saveOverlayPosition = commands.settingsSetOverlayPosition;

/** A stored slot map as the typed one. The wire carries the stored JSON
 * verbatim (`Record<string, unknown>`); slots are written as equipment
 * ids or null, so anything else reads as an empty slot rather than a
 * fabricated binding. */
function slotsFromWire(slots: Record<string, unknown>): Hotbar {
	const hotbar: Hotbar = {};
	for (const [slot, value] of Object.entries(slots)) {
		hotbar[slot] = typeof value === 'number' ? value : null;
	}
	return hotbar;
}

/** The settings response's live hotbar as the typed slot map. */
export function hotbarFromSettings(settings: AppSettings): Hotbar {
	return slotsFromWire(settings.hotbar);
}

/** Every preset's slot map, in preset order; the backend always sends
 * all three. */
export function hotbarPresetsFromSettings(settings: AppSettings): Hotbar[] {
	return [0, 1, 2].map((index) => slotsFromWire(settings.hotbarPresets[index] ?? {}));
}

/** The preset in force; the backend keeps it within the three. */
export function hotbarPresetFromSettings(settings: AppSettings): HotbarPreset {
	const preset = settings.activeHotbarPreset;
	return preset === 2 || preset === 3 ? preset : 1;
}
