import { describe, expect, it } from 'vitest';
import { status } from './__fixtures__/measure';
import { connectionView, FINISH_SIGNING_IN, lastReading } from './connection';

describe('the connection row', () => {
	it('offers to connect, and explains signing in, before the player ever has', () => {
		const view = connectionView(status(), false);
		expect(view).toMatchObject({
			tone: 'none',
			title: 'Account readings',
			signIn: 'Connect',
			disconnect: false,
			explainsSignIn: true,
			describesReading: false,
		});
	});

	it('asks for a sign-in without a warning when the player has connected before', () => {
		const view = connectionView(status({ hasConnected: true }), false);
		expect(view.tone).toBe('none');
		expect(view.signIn).toBe('Sign in');
		expect(view.describesReading).toBe(true);
	});

	it('says to finish signing in while the window is open, and brings it forward', () => {
		const view = connectionView(status(), true);
		expect(view.lines).toEqual([FINISH_SIGNING_IN]);
		expect(view.signIn).toBe('Show sign-in window');
	});

	it('shows a connection, its last reading, and the session being read', () => {
		const view = connectionView(
			status({
				state: 'connected',
				hasConnected: true,
				readingSession: true,
				lastReadingAt: '2026-11-15T14:32:00Z',
			}),
			false,
		);
		expect(view.tone).toBe('positive');
		expect(view.title).toBe('Connected');
		expect(view.lines[0]).toBe('Reading this session');
		expect(view.lines[1]).toMatch(/^Last reading /);
		expect(view.signIn).toBeNull();
		expect(view.disconnect).toBe(true);
	});

	it('says when no reading has been taken yet', () => {
		expect(lastReading(null)).toBe('No reading yet');
		const view = connectionView(status({ state: 'connected', hasConnected: true }), false);
		expect(view.lines).toEqual(['No reading yet']);
	});

	it('warns when the site signed the player out, and offers to sign in again', () => {
		const view = connectionView(status({ state: 'expired', hasConnected: true }), false);
		expect(view).toMatchObject({
			tone: 'warning',
			title: 'Signed out by the site',
			signIn: 'Sign in again',
			disconnect: true,
		});
	});

	it('is checking, with nothing to press but Disconnect, while a kept session is confirmed', () => {
		const view = connectionView(status({ state: 'unconfirmed', hasConnected: true }), false);
		expect(view).toMatchObject({
			tone: 'neutral',
			title: 'Checking your sign-in',
			signIn: null,
			disconnect: true,
		});
	});
});
