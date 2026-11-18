import { describe, expect, it } from 'vitest';
import type { AccountSignal } from '$lib/api/commands.gen';
import { overlaySignal, signInView } from './accountSignal';

const SIGNALS: AccountSignal[] = [
	'off',
	'notSignedIn',
	'checking',
	'signedIn',
	'signedOut',
	'failing',
];

describe('the dashboard sign-in control', () => {
	it('offers to connect before the player ever has, and says nothing more', () => {
		expect(signInView('off', false)).toMatchObject({ status: null, action: 'Connect account' });
	});

	it('alerts while the readings wait on a sign-in, and offers one', () => {
		expect(signInView('notSignedIn', false)).toMatchObject({
			status: { text: 'Readings paused', mark: 'alert' },
			action: 'Sign in',
		});
		expect(
			signInView('notSignedIn', false, { lastReadingAt: '2026-11-17T09:30:00Z' }).hint,
		).toMatch(/The last reading was at /);
	});

	it('alerts while signed in and the readings keep failing, saying since when', () => {
		const now = new Date('2026-11-17T12:00:00Z');
		const failing = signInView('failing', false, { failingSince: '2026-11-17T11:00:00Z' }, now);
		expect(failing.status?.mark).toBe('alert');
		expect(failing.status?.text).toMatch(/^Readings failing since /);
		expect(failing.action).toBeNull();
		expect(signInView('failing', false).status?.text).toBe('Readings failing');
	});

	it('is quiet green text once signed in, with no offer', () => {
		expect(signInView('signedIn', false)).toMatchObject({
			status: { text: 'Signed in', mark: 'live' },
			action: null,
		});
	});

	it('alerts when the site signs the player out, and offers to sign in again', () => {
		expect(signInView('signedOut', false)).toMatchObject({
			status: { text: 'Signed out', mark: 'alert' },
			action: 'Sign in again',
		});
	});

	it('only checks while a kept sign-in is confirmed', () => {
		expect(signInView('checking', false)).toMatchObject({
			status: { mark: 'checking' },
			action: null,
		});
	});

	it('brings the sign-in window back while it is open, keeping what it says', () => {
		expect(signInView('signedOut', true)).toMatchObject({
			status: { text: 'Signed out', mark: 'alert' },
			action: 'Finish signing in',
		});
		expect(signInView('off', true).action).toBe('Finish signing in');
		// Signed in, there is nothing to finish.
		expect(signInView('signedIn', true).action).toBeNull();
	});

	it('explains every state on hover', () => {
		for (const signal of SIGNALS) expect(signInView(signal, false).hint.length).toBeGreaterThan(0);
	});
});

describe('the overlay dot', () => {
	it('is drawn for a player who has connected, and only then', () => {
		expect(overlaySignal('off')).toBeNull();
		expect(overlaySignal('signedIn')?.mark).toBe('live');
		expect(overlaySignal('signedOut')?.mark).toBe('alert');
		expect(overlaySignal('notSignedIn')?.mark).toBe('alert');
		expect(overlaySignal('failing')?.mark).toBe('alert');
		expect(overlaySignal('checking')?.mark).toBe('checking');
	});

	it('points the player to the dashboard to sign in', () => {
		expect(overlaySignal('signedOut')?.label).toMatch(/dashboard/);
		expect(overlaySignal('notSignedIn')?.label).toMatch(/dashboard/);
	});
});
