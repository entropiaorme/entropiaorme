import { describe, expect, it } from 'vitest';
import type { RecentEvent } from '$lib/api';
import { acknowledgeHead, feedHead, formatEventTime, hasUnseenEvents } from './recentEvents';

function event(overrides: Partial<RecentEvent> = {}): RecentEvent {
	return {
		id: 'ne-0',
		type: 'global',
		eventType: 'global_kill',
		description: 'Global! Combibo Young',
		value: 52.4,
		timestamp: '2026-01-01T14:05:00.000Z',
		...overrides,
	};
}

describe('feedHead', () => {
	it('is null for an empty feed', () => {
		expect(feedHead([])).toBeNull();
	});

	it('tells a new head apart from an old one despite positional ids', () => {
		const old = event();
		const fresh = event({ timestamp: '2026-01-01T14:06:00.000Z' });
		expect(feedHead([fresh, { ...old, id: 'ne-1' }])).not.toBe(feedHead([old]));
	});

	it('is stable while the head is unchanged', () => {
		expect(feedHead([event(), event({ id: 'ne-1' })])).toBe(feedHead([event()]));
	});
});

describe('event attention', () => {
	it('treats the first read as seen, whichever tab is open', () => {
		const seen = acknowledgeHead(undefined, 'a', false);
		expect(seen).toBe('a');
		expect(hasUnseenEvents(seen, 'a', false)).toBe(false);
	});

	it('claims nothing before the feed has been read', () => {
		expect(acknowledgeHead(undefined, undefined, false)).toBeUndefined();
		expect(hasUnseenEvents(undefined, undefined, false)).toBe(false);
	});

	it('flags a new head that arrives while another tab is open', () => {
		const seen = acknowledgeHead('a', 'b', false);
		expect(seen).toBe('a');
		expect(hasUnseenEvents(seen, 'b', false)).toBe(true);
	});

	it('clears once the feed is watched', () => {
		expect(hasUnseenEvents('a', 'b', true)).toBe(false);
		const seen = acknowledgeHead('a', 'b', true);
		expect(hasUnseenEvents(seen, 'b', false)).toBe(false);
	});

	it('does not flag a feed emptied by a new session', () => {
		expect(hasUnseenEvents('a', null, false)).toBe(false);
	});

	it('flags the first event of a new session', () => {
		expect(hasUnseenEvents(null, 'a', false)).toBe(true);
	});
});

describe('formatEventTime', () => {
	it('renders the time of day', () => {
		expect(formatEventTime('2026-01-01T14:05:00.000Z')).toBe('2:05 PM');
	});

	it('is null without a usable timestamp', () => {
		expect(formatEventTime(null)).toBeNull();
		expect(formatEventTime('not a time')).toBeNull();
	});
});
