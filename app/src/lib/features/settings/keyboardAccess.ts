/**
 * What the settings say about reading keys on this machine: what hotbar
 * presses are for, where they come from, or that they cannot be read and
 * keyless tracking is the way to play here.
 */

import type { KeyboardAccess } from '$lib/api/commands.gen';

export interface KeyboardNote {
	text: string;
	/** Keys cannot be read here, which the player should act on. */
	attention: boolean;
}

/** What the hotbar listener is for: the weapon or healer in hand, or, while
 * the tool in hand is read from the screen, consumables alone. */
export function hotbarListenerCopy(toolReading: boolean): string {
	return toolReading
		? 'Reads your number hotbar (1-9, 0) while a session runs, so pressing a consumable starts its timer. The tool in hand is read from the screen.'
		: 'Reads your number hotbar (1-9, 0) while a session runs, so each press tells the app which weapon or healer is in hand. When off, weapon costs follow damage ranges alone and healing is not billed.';
}

/** What hotbar presses not reaching the app means: weapon costs left to
 * damage ranges, or, while the tool in hand is read from the screen,
 * consumables pressed on the hotbar not starting their timers. */
export function listenerInactiveCopy(toolReading: boolean): string {
	return toolReading
		? 'Your hotbar presses are not reaching the app, so consumables pressed on it do not start their timers. Check the hotbar key listener in Settings.'
		: 'Your hotbar presses are not reaching the app, so weapon costs follow damage ranges alone. Check the hotbar key listener in Settings.';
}

/** The one line under the hotbar listener, or none when nothing needs
 * saying (keyless tracking speaks for itself). */
export function keyboardNote(access: KeyboardAccess): KeyboardNote | null {
	if (access.keyless) return null;
	switch (access.observer) {
		case 'xinput':
			return { text: 'Keys are read through the X server.', attention: false };
		case 'evdev':
			return { text: 'Keys are read from the input devices.', attention: false };
		case 'windows_hook':
			return { text: 'Keys are read through the Windows keyboard hook.', attention: false };
		case 'unavailable':
			return {
				text: access.keylessAvailable
					? 'This machine cannot read keys. Turn on keyless tracking below.'
					: 'This machine cannot read keys.',
				attention: true,
			};
		default:
			return null;
	}
}
