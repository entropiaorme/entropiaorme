// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	documentVisibility,
	useVisiblePoll,
	type VisibilitySource,
	windowGeometryPoll,
} from './useVisiblePoll';

// A hand-driven visibility source: tests flip `visible` and fire the captured
// onChange to simulate hidden<->visible transitions deterministically.
function fakeSource(initiallyVisible: boolean): {
	source: VisibilitySource;
	setVisible: (visible: boolean) => void;
	unsubscribed: () => boolean;
	subscriberCount: () => number;
} {
	let visible = initiallyVisible;
	let unsubscribeCalls = 0;
	const listeners: ((visible: boolean) => void)[] = [];
	return {
		source: {
			isVisible: () => visible,
			subscribe: (onChange) => {
				listeners.push(onChange);
				return () => {
					unsubscribeCalls += 1;
					const i = listeners.indexOf(onChange);
					if (i >= 0) listeners.splice(i, 1);
				};
			},
		},
		setVisible: (next: boolean) => {
			visible = next;
			for (const listener of [...listeners]) listener(next);
		},
		unsubscribed: () => unsubscribeCalls > 0,
		subscriberCount: () => listeners.length,
	};
}

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

describe('documentVisibility', () => {
	it('falls back to always-visible with a noop unsubscribe when there is no DOM', () => {
		vi.stubGlobal('document', undefined);
		const source = documentVisibility();
		expect(source.isVisible()).toBe(true);
		const unsubscribe = source.subscribe(() => {});
		expect(() => unsubscribe()).not.toThrow();
	});

	it('maps visibilityState to isVisible (anything but "hidden" is visible)', () => {
		const states = { current: 'visible' };
		Object.defineProperty(document, 'visibilityState', {
			configurable: true,
			get: () => states.current,
		});
		const source = documentVisibility();
		expect(source.isVisible()).toBe(true);
		states.current = 'hidden';
		expect(source.isVisible()).toBe(false);
	});

	it('subscribes to visibilitychange and reports the new state; unsubscribe detaches', () => {
		const states = { current: 'visible' };
		Object.defineProperty(document, 'visibilityState', {
			configurable: true,
			get: () => states.current,
		});
		const seen: boolean[] = [];
		const source = documentVisibility();
		const unsubscribe = source.subscribe((visible) => seen.push(visible));

		states.current = 'hidden';
		document.dispatchEvent(new Event('visibilitychange'));
		states.current = 'visible';
		document.dispatchEvent(new Event('visibilitychange'));
		expect(seen).toEqual([false, true]);

		unsubscribe();
		document.dispatchEvent(new Event('visibilitychange'));
		expect(seen).toEqual([false, true]);
	});
});

describe('useVisiblePoll', () => {
	it('runs an immediate tick and then ticks on the interval while visible', () => {
		const { source } = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source });
		expect(tick).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(3000);
		expect(tick).toHaveBeenCalledTimes(4);
		stop();
	});

	it('suppresses the start-up tick when immediate is false', () => {
		const { source } = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, immediate: false, source });
		expect(tick).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1000);
		expect(tick).toHaveBeenCalledTimes(1);
		stop();
	});

	it('does not arm at all when the source starts hidden', () => {
		const { source } = fakeSource(false);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source });
		vi.advanceTimersByTime(10_000);
		expect(tick).not.toHaveBeenCalled();
		stop();
	});

	it('clears the timer while hidden: no ticks accumulate in the background', () => {
		const fake = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source: fake.source });
		expect(tick).toHaveBeenCalledTimes(1);

		fake.setVisible(false);
		vi.advanceTimersByTime(10_000);
		expect(tick).toHaveBeenCalledTimes(1);
		stop();
	});

	it('fires one catch-up tick on resume and re-arms the interval', () => {
		const fake = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source: fake.source });
		fake.setVisible(false);
		expect(tick).toHaveBeenCalledTimes(1);

		fake.setVisible(true);
		expect(tick).toHaveBeenCalledTimes(2);
		vi.advanceTimersByTime(2000);
		expect(tick).toHaveBeenCalledTimes(4);
		stop();
	});

	it('suppresses the catch-up tick when tickOnResume is false', () => {
		const fake = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, {
			intervalMs: 1000,
			tickOnResume: false,
			source: fake.source,
		});
		fake.setVisible(false);
		fake.setVisible(true);
		expect(tick).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1000);
		expect(tick).toHaveBeenCalledTimes(2);
		stop();
	});

	it('guards against double-arming on repeated visible notifications', () => {
		const fake = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source: fake.source });
		expect(tick).toHaveBeenCalledTimes(1);

		// A second visible notification while already armed must not stack a
		// second interval (or re-run the catch-up tick).
		fake.setVisible(true);
		expect(tick).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1000);
		expect(tick).toHaveBeenCalledTimes(2);
		stop();
	});

	it('stop() clears the timer and detaches the visibility subscription', () => {
		const fake = fakeSource(true);
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000, source: fake.source });
		expect(fake.subscriberCount()).toBe(1);

		stop();
		expect(fake.unsubscribed()).toBe(true);
		expect(fake.subscriberCount()).toBe(0);
		vi.advanceTimersByTime(10_000);
		expect(tick).toHaveBeenCalledTimes(1);
	});

	it('logs an async tick rejection instead of leaving it unhandled', async () => {
		const error = vi.spyOn(console, 'error').mockImplementation(() => {});
		const { source } = fakeSource(true);
		const stop = useVisiblePoll(() => Promise.reject(new Error('tick boom')), {
			intervalMs: 1000,
			source,
		});
		// Let the rejection settle through the microtask queue.
		await Promise.resolve();
		await Promise.resolve();
		expect(error).toHaveBeenCalledWith('useVisiblePoll: tick failed', expect.any(Error));
		stop();
	});

	it('defaults to the document visibility source when none is injected', () => {
		// Pin the document visible explicitly rather than relying on the DOM
		// library's default, so the assertion cannot drift with happy-dom.
		Object.defineProperty(document, 'visibilityState', {
			configurable: true,
			get: () => 'visible',
		});
		const tick = vi.fn();
		const stop = useVisiblePoll(tick, { intervalMs: 1000 });
		expect(tick).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1000);
		expect(tick).toHaveBeenCalledTimes(2);
		stop();
	});

	it('lets a synchronous throw from the immediate tick propagate to the caller', () => {
		const { source } = fakeSource(true);
		expect(() =>
			useVisiblePoll(
				() => {
					throw new Error('sync boom');
				},
				{ intervalMs: 1000, source },
			),
		).toThrow('sync boom');
	});
});

describe('windowGeometryPoll', () => {
	it('ticks on the interval with no immediate tick and no pause knob', () => {
		const tick = vi.fn();
		const stop = windowGeometryPoll(tick, 500);
		expect(tick).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1500);
		expect(tick).toHaveBeenCalledTimes(3);
		stop();
	});

	it('keeps ticking across document visibility changes (deliberately unpausable)', () => {
		const states = { current: 'visible' };
		Object.defineProperty(document, 'visibilityState', {
			configurable: true,
			get: () => states.current,
		});
		const tick = vi.fn();
		const stop = windowGeometryPoll(tick, 500);

		states.current = 'hidden';
		document.dispatchEvent(new Event('visibilitychange'));
		vi.advanceTimersByTime(1000);
		expect(tick).toHaveBeenCalledTimes(2);
		stop();
	});

	it('stop() clears the timer', () => {
		const tick = vi.fn();
		const stop = windowGeometryPoll(tick, 500);
		stop();
		vi.advanceTimersByTime(5000);
		expect(tick).not.toHaveBeenCalled();
	});

	it('logs an async tick rejection with its own label', async () => {
		const error = vi.spyOn(console, 'error').mockImplementation(() => {});
		const stop = windowGeometryPoll(() => Promise.reject(new Error('geometry boom')), 500);
		vi.advanceTimersByTime(500);
		await Promise.resolve();
		await Promise.resolve();
		expect(error).toHaveBeenCalledWith('windowGeometryPoll: tick failed', expect.any(Error));
		stop();
	});
});
