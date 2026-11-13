import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createHoverPanel, HOVER_CLOSE_MS } from './hoverPanel.svelte';

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('createHoverPanel', () => {
	it('opens and closes for good on its toggle, telling the owner it closed', () => {
		const onClose = vi.fn();
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel({ onClose });
			expect(panel.expanded).toBe(false);
			panel.toggle();
			flushSync();
			expect(panel.expanded).toBe(true);
			panel.toggle();
			flushSync();
			expect(panel.expanded).toBe(false);
			expect(onClose).toHaveBeenCalledTimes(1);
		});
		cleanup();
	});

	it('opens under the pointer and closes a moment after it leaves, unless it comes back', () => {
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel();
			panel.pointerEnter();
			flushSync();
			expect(panel.expanded).toBe(true);
			panel.pointerLeave();
			vi.advanceTimersByTime(HOVER_CLOSE_MS - 10);
			panel.pointerEnter();
			vi.advanceTimersByTime(HOVER_CLOSE_MS);
			flushSync();
			expect(panel.expanded).toBe(true);
			panel.pointerLeave();
			vi.advanceTimersByTime(HOVER_CLOSE_MS);
			flushSync();
			expect(panel.expanded).toBe(false);
		});
		cleanup();
	});

	it('keeps a hover-opened panel open when its toggle is clicked', () => {
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel();
			panel.pointerEnter();
			panel.toggle();
			panel.pointerLeave();
			vi.advanceTimersByTime(HOVER_CLOSE_MS * 2);
			flushSync();
			expect(panel.expanded).toBe(true);
		});
		cleanup();
	});

	it('never expands while it is not available, and holds while the player works in it', () => {
		let available = false;
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel({ available: () => available });
			panel.toggle();
			panel.hold();
			flushSync();
			expect(panel.expanded).toBe(false);
			available = true;
			const live = createHoverPanel({ available: () => available });
			live.hold();
			live.pointerLeave();
			vi.advanceTimersByTime(HOVER_CLOSE_MS * 2);
			flushSync();
			expect(live.expanded).toBe(true);
			live.close();
			flushSync();
			expect(live.expanded).toBe(false);
		});
		cleanup();
	});

	it('leaves no timer behind once destroyed', () => {
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel();
			panel.pointerEnter();
			panel.pointerLeave();
			panel.destroy();
			expect(vi.getTimerCount()).toBe(0);
		});
		cleanup();
	});

	it('closes and forgets an open panel while unavailable, never reopening by itself', () => {
		let available = $state(true);
		const onClose = vi.fn();
		const cleanup = $effect.root(() => {
			const panel = createHoverPanel({ available: () => available, onClose });
			panel.toggle();
			flushSync();
			expect(panel.expanded).toBe(true);
			// The session ends.
			available = false;
			flushSync();
			expect(panel.expanded).toBe(false);
			expect(onClose).toHaveBeenCalledTimes(1);
			// A new one starts: the panel stays closed until asked.
			available = true;
			flushSync();
			expect(panel.expanded).toBe(false);
			// Hovered when it went: forgotten too.
			panel.pointerEnter();
			flushSync();
			expect(panel.expanded).toBe(true);
			available = false;
			flushSync();
			available = true;
			flushSync();
			expect(panel.expanded).toBe(false);
			panel.destroy();
		});
		cleanup();
	});
});
