/**
 * What the settings say about reading keys on this machine: where hotbar
 * presses come from, or that they cannot be read and keyless tracking is
 * the way to play here.
 */

import type { KeyboardAccess } from '$lib/api/commands.gen';

export interface KeyboardNote {
	text: string;
	/** Keys cannot be read here, which the player should act on. */
	attention: boolean;
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
