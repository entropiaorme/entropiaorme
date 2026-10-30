import { describe, expect, it } from 'vitest';
import { keyboardNote } from './keyboardAccess';

describe('keyboardNote', () => {
	it('names where keys are read from', () => {
		expect(keyboardNote({ observer: 'xinput', keyless: false, keylessAvailable: true })).toEqual({
			text: 'Keys are read through the X server.',
			attention: false,
		});
		expect(keyboardNote({ observer: 'evdev', keyless: false, keylessAvailable: true })?.text).toBe(
			'Keys are read from the input devices.',
		);
		expect(
			keyboardNote({ observer: 'windows_hook', keyless: false, keylessAvailable: true })?.text,
		).toBe('Keys are read through the Windows keyboard hook.');
	});

	it('points a machine that cannot read keys at keyless tracking', () => {
		expect(
			keyboardNote({ observer: 'unavailable', keyless: false, keylessAvailable: true }),
		).toEqual({
			text: 'This machine cannot read keys. Turn on keyless tracking below.',
			attention: true,
		});
		expect(
			keyboardNote({ observer: 'unavailable', keyless: false, keylessAvailable: false })?.text,
		).toBe('This machine cannot read keys.');
	});

	it('says nothing while keyless, when the keyboard is never probed', () => {
		expect(keyboardNote({ observer: null, keyless: true, keylessAvailable: true })).toBeNull();
	});
});
