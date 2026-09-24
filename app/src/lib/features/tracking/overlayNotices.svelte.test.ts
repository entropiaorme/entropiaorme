// @vitest-environment happy-dom

import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
	createOverlayNotices,
	NOTICE_MAX_MS,
	NOTICE_MAX_VISIBLE,
	NOTICE_MIN_MS,
	noticeDurationMs,
} from './overlayNotices.svelte';

// The overlay's notice channel under fake timers: every notice clears
// itself, a hover holds it, and a session's cumulative warning list
// notifies each message once. The DOM environment is for `follow`: it
// brings in Svelte's client runtime, under which effects actually run.

const guardrail =
	'Harvest guardrail: Short Boards were looted while ChopChop Jr was equipped; costs are attributed to Timber Saw';

function texts(notices: ReturnType<typeof createOverlayNotices>) {
	return notices.current.map((notice) => notice.text);
}

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('noticeDurationMs', () => {
	it('floors a short line and caps a long one', () => {
		expect(noticeDurationMs('Saved')).toBe(NOTICE_MIN_MS);
		expect(noticeDurationMs('x'.repeat(1000))).toBe(NOTICE_MAX_MS);
		const mid = noticeDurationMs(guardrail);
		expect(mid).toBeGreaterThan(NOTICE_MIN_MS);
		expect(mid).toBeLessThanOrEqual(NOTICE_MAX_MS);
	});
});

describe('push', () => {
	it('shows the full message and clears it once its time is up', () => {
		const notices = createOverlayNotices();
		notices.push(guardrail, 'warning');
		expect(notices.current).toEqual([{ id: 1, text: guardrail, tone: 'warning' }]);

		vi.advanceTimersByTime(noticeDurationMs(guardrail) - 1);
		expect(texts(notices)).toEqual([guardrail]);
		vi.advanceTimersByTime(1);
		expect(notices.current).toEqual([]);
	});

	it('restarts a message already on screen rather than stacking a duplicate', () => {
		const notices = createOverlayNotices();
		const message = 'Protection selection failed';
		notices.push(message);
		vi.advanceTimersByTime(noticeDurationMs(message) - 500);
		notices.push(message);
		expect(notices.current).toHaveLength(1);
		vi.advanceTimersByTime(noticeDurationMs(message) - 1);
		expect(notices.current).toHaveLength(1);
		vi.advanceTimersByTime(1);
		expect(notices.current).toHaveLength(0);
	});

	it('ignores a blank message', () => {
		const notices = createOverlayNotices();
		notices.push('   ');
		expect(notices.current).toEqual([]);
	});

	it('drops the oldest when a burst outruns the visible limit', () => {
		const notices = createOverlayNotices();
		const burst = Array.from({ length: NOTICE_MAX_VISIBLE + 1 }, (_, i) => `failure ${i}`);
		for (const message of burst) notices.push(message);
		expect(texts(notices)).toEqual(burst.slice(1));
		// The dropped notice's timer went with it.
		vi.advanceTimersByTime(NOTICE_MAX_MS);
		expect(notices.current).toEqual([]);
		expect(vi.getTimerCount()).toBe(0);
	});
});

describe('hold and release', () => {
	it('keeps notices up while held and gives them their full time again on release', () => {
		const notices = createOverlayNotices();
		notices.push('Protection selection failed');
		notices.hold();
		notices.push('Failed to set skill boost');
		vi.advanceTimersByTime(NOTICE_MAX_MS * 4);
		expect(notices.current).toHaveLength(2);

		notices.release();
		vi.advanceTimersByTime(NOTICE_MAX_MS);
		expect(notices.current).toEqual([]);
	});
});

describe('observeWarnings', () => {
	it('notifies each warning once per session, however many frames repeat it', () => {
		const notices = createOverlayNotices();
		notices.observeWarnings('s1', [guardrail]);
		notices.observeWarnings('s1', [guardrail]);
		expect(texts(notices)).toEqual([guardrail]);

		vi.advanceTimersByTime(NOTICE_MAX_MS);
		notices.observeWarnings('s1', [guardrail]);
		expect(notices.current).toEqual([]);

		const healing = 'Healing evidence could not be saved; no unverified cost was added';
		notices.observeWarnings('s1', [guardrail, healing]);
		expect(notices.current).toEqual([{ id: 2, text: healing, tone: 'warning' }]);
	});

	it('lets a new session raise the same warning again', () => {
		const notices = createOverlayNotices();
		notices.observeWarnings('s1', [guardrail]);
		vi.advanceTimersByTime(NOTICE_MAX_MS);
		notices.observeWarnings(null, []);
		notices.observeWarnings('s2', [guardrail]);
		expect(texts(notices)).toEqual([guardrail]);
	});
});

describe('follow', () => {
	it('notifies each message a channel reports, including a repeat after a clear', () => {
		let channel = $state<string | null>(null);
		const notices = createOverlayNotices();
		const cleanup = $effect.root(() => {
			notices.follow(() => channel);
		});
		flushSync();
		expect(notices.current).toEqual([]);

		channel = 'Protection selection failed';
		flushSync();
		expect(texts(notices)).toEqual(['Protection selection failed']);

		vi.advanceTimersByTime(NOTICE_MAX_MS);
		channel = null;
		flushSync();
		channel = 'Protection selection failed';
		flushSync();
		expect(texts(notices)).toEqual(['Protection selection failed']);
		cleanup();
	});
});

describe('destroy', () => {
	it('clears every notice and timer', () => {
		const notices = createOverlayNotices();
		notices.push('one');
		notices.push('two');
		notices.destroy();
		expect(notices.current).toEqual([]);
		expect(vi.getTimerCount()).toBe(0);
	});
});
