import { describe, expect, it, vi } from 'vitest';
import { ensureViewport } from './onboarding.mjs';

// Unit coverage for ensureViewport's inner-viewport recovery loop, driven by a
// mock WebdriverIO `browser` so the logic is verified deterministically without
// a real WebView2 shell. `execute` stands in for the in-page
// `window.innerWidth/innerHeight` read; `setWindowSize` is the relayout nudge.

/** A browser whose inner viewport is collapsed until `setWindowSize` is called
 *  (the nudge), then reports the target: the success path. */
function recoveringBrowser() {
	let nudged = false;
	return {
		execute: vi.fn(async () => (nudged ? { w: 1600, h: 1000 } : { w: 340, h: 242 })),
		setWindowSize: vi.fn(async () => {
			nudged = true;
		}),
		pause: vi.fn(async () => {}),
	};
}

/** A browser whose inner viewport is permanently collapsed: the nudge never
 *  recovers it, so ensureViewport must throw. */
function stuckBrowser() {
	return {
		execute: vi.fn(async () => ({ w: 340, h: 242 })),
		setWindowSize: vi.fn(async () => {}),
		pause: vi.fn(async () => {}),
	};
}

/** A browser already at target: ensureViewport returns without nudging. */
function healthyBrowser() {
	return {
		execute: vi.fn(async () => ({ w: 1600, h: 1000 })),
		setWindowSize: vi.fn(async () => {}),
		pause: vi.fn(async () => {}),
	};
}

describe('ensureViewport', () => {
	it('recovers a collapsed inner viewport via the relayout nudge', async () => {
		const browser = recoveringBrowser();
		const inner = await ensureViewport(browser);
		expect(inner).toEqual({ w: 1600, h: 1000 });
		// The nudge ran (setWindowSize), and the viewport was re-read after it.
		expect(browser.setWindowSize).toHaveBeenCalled();
		expect(browser.execute.mock.calls.length).toBeGreaterThanOrEqual(2);
	});

	it('returns immediately when the inner viewport is already at target', async () => {
		const browser = healthyBrowser();
		const inner = await ensureViewport(browser);
		expect(inner).toEqual({ w: 1600, h: 1000 });
		// No recovery needed: the nudge never ran.
		expect(browser.setWindowSize).not.toHaveBeenCalled();
	});

	it('throws loudly when the inner viewport cannot be recovered', async () => {
		const browser = stuckBrowser();
		await expect(ensureViewport(browser, { timeout: 30 })).rejects.toThrow(
			/inner viewport never recovered/,
		);
	});
});
