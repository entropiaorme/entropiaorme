// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import CountdownCapture from './CountdownCapture.svelte';

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('CountdownCapture', () => {
	it('counts down on the button and captures at zero', async () => {
		const oncapture = vi.fn();
		render(CountdownCapture, { props: { oncapture } });
		await fireEvent.click(screen.getByRole('button', { name: 'Capture in 3 s' }));
		expect(screen.getByText('Capturing in 3…')).toBeTruthy();
		await vi.advanceTimersByTimeAsync(3000);
		expect(oncapture).toHaveBeenCalledOnce();
		expect(screen.getByRole('button', { name: 'Capture in 3 s' })).toBeTruthy();
	});

	it('cancels from the same button', async () => {
		const oncapture = vi.fn();
		render(CountdownCapture, { props: { oncapture } });
		await fireEvent.click(screen.getByRole('button', { name: 'Capture in 3 s' }));
		await fireEvent.click(screen.getByRole('button', { name: /Capturing in/ }));
		await vi.advanceTimersByTimeAsync(5000);
		expect(oncapture).not.toHaveBeenCalled();
	});
});
