import { describe, expect, it } from 'vitest';
import { hotbarListenerCopy, keyboardNote, listenerInactiveCopy } from './keyboardAccess';

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

describe('hotbarListenerCopy', () => {
	it('says presses name the tool in hand while the screen does not', () => {
		expect(hotbarListenerCopy(false)).toMatch(/which weapon or healer is in hand/);
	});

	it('says presses start consumables alone while the tool in hand is read from the screen', () => {
		const copy = hotbarListenerCopy(true);
		expect(copy).toMatch(/pressing a consumable starts its timer/);
		expect(copy).toMatch(/tool in hand is read from the screen/);
		expect(copy).not.toMatch(/weapon or healer/);
		expect(copy).not.toMatch(/damage ranges/);
	});
});

describe('listenerInactiveCopy', () => {
	it('says weapon costs are left to damage ranges while the screen does not name the tool', () => {
		expect(listenerInactiveCopy(false)).toBe(
			'Your hotbar presses are not reaching the app, so weapon costs follow damage ranges alone. Check the hotbar key listener in Settings.',
		);
	});

	it('says only consumable timers are affected while the tool in hand is read from the screen', () => {
		expect(listenerInactiveCopy(true)).toMatch(
			/consumables pressed on it do not start their timers/,
		);
		expect(listenerInactiveCopy(true)).not.toMatch(/weapon|damage/);
	});
});
