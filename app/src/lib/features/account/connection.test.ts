import { describe, expect, it } from 'vitest';
import { formatDate, formatTime } from '$lib/utils/format';
import { status } from './__fixtures__/measure';
import {
	connectionView,
	FINISH_SIGNING_IN,
	lastReading,
	offersReview,
	rowKey,
	SITE_UNREACHABLE,
} from './connection';

describe('the connection row', () => {
	it('offers to connect, and explains signing in, before the player ever has', () => {
		expect(connectionView(status())).toMatchObject({
			tone: 'none',
			title: 'Account readings',
			lines: [],
			signIn: 'Connect',
			disconnect: false,
			explainsSignIn: true,
			describesReading: false,
		});
	});

	it('asks for a sign-in without a warning when the player has connected before', () => {
		const view = connectionView(status({ hasConnected: true }));
		expect(view.tone).toBe('none');
		expect(view.signIn).toBe('Sign in');
		expect(view.describesReading).toBe(true);
	});

	it('says to finish signing in, and brings the window forward, only while the status says it is open', () => {
		const open = connectionView(status({ signingIn: true }));
		expect(open.lines).toEqual([FINISH_SIGNING_IN]);
		expect(open.signIn).toBe('Show sign-in window');
		const closed = connectionView(status({ signingIn: false }));
		expect(closed.lines).toEqual([]);
		expect(closed.signIn).toBe('Connect');
	});

	it('shows a connection, its last reading, and the session being read', () => {
		const view = connectionView(
			status({
				state: 'connected',
				hasConnected: true,
				readingSession: true,
				lastReadingAt: new Date().toISOString(),
			}),
		);
		expect(view.tone).toBe('positive');
		expect(view.title).toBe('Connected');
		expect(view.lines[0]).toBe('Reading this session');
		expect(view.lines[1]).toMatch(/^Last reading /);
		expect(view.signIn).toBeNull();
		expect(view.disconnect).toBe(true);
	});

	it('says when the last reading was taken: the time today, the date and time before', () => {
		const now = new Date('2026-11-16T15:00:00Z');
		expect(lastReading(null, now)).toBe('No reading yet');
		expect(lastReading('2026-11-16T14:32:00Z', now)).toBe(
			`Last reading ${formatTime('2026-11-16T14:32:00Z')}`,
		);
		expect(lastReading('2026-11-14T14:32:00Z', now)).toBe(
			`Last reading ${formatDate('2026-11-14T14:32:00Z')}, ${formatTime('2026-11-14T14:32:00Z')}`,
		);
	});

	it('warns when the site signed the player out, and offers to sign in again', () => {
		expect(connectionView(status({ state: 'expired', hasConnected: true }))).toMatchObject({
			tone: 'warning',
			title: 'Signed out by the site',
			signIn: 'Sign in again',
			disconnect: true,
		});
	});

	it('stays true when the site signs the player out while a sign-in window is open', () => {
		const view = connectionView(status({ state: 'expired', hasConnected: true, signingIn: true }));
		expect(view.title).toBe('Signed out by the site');
		expect(view.lines).toEqual([FINISH_SIGNING_IN]);
		expect(view.signIn).toBe('Show sign-in window');
	});

	it('is checking, with nothing to press but Disconnect, while a kept session is confirmed', () => {
		const checking = connectionView(
			status({ state: 'unconfirmed', hasConnected: true, confirming: 'checking' }),
		);
		expect(checking).toMatchObject({
			tone: 'neutral',
			title: 'Checking your sign-in',
			lines: [],
			signIn: null,
			disconnect: true,
		});
		const unreachable = connectionView(
			status({ state: 'unconfirmed', hasConnected: true, confirming: 'siteUnreachable' }),
		);
		expect(unreachable.lines).toEqual([SITE_UNREACHABLE]);
	});
});

describe('the review of measured sessions', () => {
	it('is offered once anything was read, or while the player is or has been connected', () => {
		expect(offersReview(null)).toBe(false);
		expect(offersReview(status())).toBe(false);
		expect(offersReview(status({ hasReadings: true }))).toBe(true);
		expect(offersReview(status({ hasConnected: true }))).toBe(true);
		expect(offersReview(status({ state: 'connected' }))).toBe(true);
	});
});

describe("the row's key", () => {
	it('changes with what the row shows, not with a reading taken', () => {
		const connected = status({ state: 'connected', hasConnected: true });
		expect(rowKey({ ...connected, lastReadingAt: '2026-11-16T14:00:00Z' })).toBe(rowKey(connected));
		expect(rowKey({ ...connected, state: 'expired' })).not.toBe(rowKey(connected));
		expect(rowKey(status({ signingIn: true }))).not.toBe(rowKey(status()));
	});
});
