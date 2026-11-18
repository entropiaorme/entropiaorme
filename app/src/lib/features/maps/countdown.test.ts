import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createCountdown } from './countdown.svelte';

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('createCountdown', () => {
	it('counts down a second at a time and acts once at zero', () => {
		const onElapsed = vi.fn();
		const countdown = createCountdown(3, onElapsed);
		expect(countdown.remaining).toBeNull();
		countdown.start();
		expect(countdown.remaining).toBe(3);
		vi.advanceTimersByTime(1000);
		expect(countdown.remaining).toBe(2);
		vi.advanceTimersByTime(1000);
		expect(countdown.remaining).toBe(1);
		expect(onElapsed).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1000);
		expect(countdown.remaining).toBeNull();
		expect(onElapsed).toHaveBeenCalledOnce();
		vi.advanceTimersByTime(5000);
		expect(onElapsed).toHaveBeenCalledOnce();
	});

	it('cancels without acting', () => {
		const onElapsed = vi.fn();
		const countdown = createCountdown(3, onElapsed);
		countdown.start();
		vi.advanceTimersByTime(1500);
		countdown.cancel();
		vi.advanceTimersByTime(5000);
		expect(countdown.remaining).toBeNull();
		expect(onElapsed).not.toHaveBeenCalled();
	});

	it('restarts from the top when started again', () => {
		const onElapsed = vi.fn();
		const countdown = createCountdown(3, onElapsed);
		countdown.start();
		vi.advanceTimersByTime(2000);
		countdown.start();
		expect(countdown.remaining).toBe(3);
		vi.advanceTimersByTime(2000);
		expect(onElapsed).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1000);
		expect(onElapsed).toHaveBeenCalledOnce();
	});
});
