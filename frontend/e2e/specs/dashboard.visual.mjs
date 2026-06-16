import { $, browser, expect } from '@wdio/globals';
import { ensureDashboard } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Dashboard visual regression against a committed baseline, captured in the
// real shell. Determinism is engineered, not hoped for: the stub backend pins
// the data and CSS animations are disabled for the shot. The target is the
// session stat grid (element-scoped via checkElement) rather than the full
// viewport: element capture is immune to the post-onboarding page-in animation
// and the chart widgets' JS-driven mount tweens, both of which make a
// full-viewport shot non-deterministic. The grid renders fixed fixture values
// (no charts, no wall-clock surface), so it is a stable, meaningful regression
// net for the dashboard's headline numbers. Baselines are generated and diffed
// in the same rendering environment (WebView2 on Windows); regenerate with
// `npm run test:visual:update` (or delete the baseline and re-run).
describe('dashboard visual regression (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
		await browser.pause(1200); // let the grid settle its fixed-value layout
	});

	it('matches the committed stat-cell baseline', async () => {
		const grid = await $('[data-guide-anchor="dashboard-stats-grid"]');
		await grid.waitForExist({ timeout: 10000 });
		// Gate the shot on the loaded state: before the snapshot hydrates, the
		// stats render em-dashes; capturing that transient swings the diff wildly.
		await browser.waitUntil(async () => !(await grid.getText()).includes('—'), {
			timeout: 12000,
			timeoutMsg: 'stat grid never reached the loaded (non-em-dash) state',
		});
		await browser.pause(500);
		// Capture a single fixed-width stat cell (the first: "Cycled"), not the
		// whole grid: the grid's auto-fill columns reflow as the chart widgets
		// below it load and the scrollbar toggles, which makes a grid-wide shot
		// non-deterministic. A cell-scoped shot captures the cell's own content
		// (a fixed fixture value), immune to that reflow.
		const cell = await $('[data-stat-cell="0"]');
		await cell.waitForExist({ timeout: 10000 });
		const mismatch = await browser.checkElement(cell, 'dashboard-stat-cell', {
			disableCSSAnimations: true,
			hideScrollBars: true,
		});
		// Small budget for sub-pixel AA noise; a real value/layout change is far larger.
		expect(mismatch).toBeLessThanOrEqual(1.5);
	});
});
