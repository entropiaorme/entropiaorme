import { describe, expect, it } from 'vitest';
import type { AccountSignal } from '$lib/api/commands.gen';
import { overlaySignal, signInView } from './accountSignal';

const SIGNALS: AccountSignal[] = ['off', 'notSignedIn', 'checking', 'signedIn', 'signedOut'];

describe('the dashboard sign-in control', () => {
	it('offers to sign in until signed in, and says nothing more', () => {
		expect(signInView('off', false)).toMatchObject({ status: null, action: 'Connect account' });
		expect(signInView('notSignedIn', false)).toMatchObject({ status: null, action: 'Sign in' });
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
		expect(overlaySignal('notSignedIn')?.mark).toBe('absent');
		expect(overlaySignal('checking')?.mark).toBe('checking');
	});

	it('points the player to the dashboard to sign in', () => {
		expect(overlaySignal('signedOut')?.label).toMatch(/dashboard/);
		expect(overlaySignal('notSignedIn')?.label).toMatch(/dashboard/);
	});
});
