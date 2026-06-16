import { $, browser, expect } from '@wdio/globals';
import { animationsFinished, ensureDashboard, settleSize } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Dashboard visual regression against committed baselines, captured in the real
// shell. Determinism is engineered, not hoped for: the stub backend pins the
// data, CSS animations are disabled for the shot, and the chart widgets' JS
// tweens are settled instantly by the e2e build (E2E_FREEZE_TWEENS, see
// src/lib/motion/testMotion.ts) since disableCSSAnimations cannot reach a
// `svelte/motion` tween. Every shot is element-scoped (checkElement) and
// deliberately EXCLUDES the session island: its elapsed timer ticks off the
// wall clock every second and would swing any shot that contains it. A
// full-viewport shot is intentionally not attempted (the auto-fill stat grid
// reflows as widgets mount + the scrollbar toggles, a layout race no animation
// freeze can settle). Baselines are generated and diffed in the same rendering
// environment (WebView2 on Windows); regenerate with `npm run test:visual:update`.
const VISUAL_OPTS = { disableCSSAnimations: true, hideScrollBars: true };
const BUDGET = 1.5; // small budget for sub-pixel AA noise; a real change is far larger

describe('dashboard visual regression (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
	});

	it('matches the committed stat-cell baseline', async () => {
		const grid = await $('[data-guide-anchor="dashboard-stats-grid"]');
		await grid.waitForExist({ timeout: 10000 });
		// Gate the shot on the loaded state: before the snapshot hydrates, the
		// stats render an em-dash placeholder (U+2014, written as an escape here so
		// the authoring lint does not flag the literal); capturing that transient
		// swings the diff wildly.
		await browser.waitUntil(async () => !(await grid.getText()).includes('\u2014'), {
			timeout: 12000,
			timeoutMsg: 'stat grid never reached the loaded (non-em-dash) state',
		});
		// Capture a single fixed-width stat cell (the first: "Cycled"), not the
		// whole grid: the grid's auto-fill columns reflow as the chart widgets
		// below it load, which makes a grid-wide shot non-deterministic. A
		// cell-scoped shot captures the cell's own fixed fixture value.
		const cell = await $('[data-stat-cell="0"]');
		await cell.waitForExist({ timeout: 10000 });
		// Settle the WAAPI stat-grid FLIP and the cell's own box before the shot:
		// the prior "visually identical yet 32%" diff was a sub-pixel transform
		// captured mid-flip (disableCSSAnimations cannot reach a Web Animation).
		await animationsFinished(browser);
		await settleSize(browser, cell, { min: 40 });
		const mismatch = await browser.checkElement(cell, 'dashboard-stat-cell', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the recent-events island baseline', async () => {
		// A pure fixture list (description + value, no timer), so it is the most
		// stable dashboard surface to broaden onto.
		const events = await $('[data-guide-anchor="dashboard-recent-events"]');
		await events.waitForExist({ timeout: 10000 });
		// Gate on the COMPLETE fixture, not just the first row to mention 'HOF':
		// require all three painted rows incl. the last ('Atrox Old'), so a
		// mid-hydration 1-of-3-rows capture cannot pass the gate.
		await browser.waitUntil(
			async () => {
				const rows = await events.$$('li');
				const text = await events.getText();
				return rows.length === 3 && text.includes('HOF') && text.includes('Atrox Old');
			},
			{ timeout: 12000, timeoutMsg: 'recent-events never hydrated all 3 fixture rows' },
		);
		await animationsFinished(browser);
		await settleSize(browser, events, { min: 90 });
		const mismatch = await browser.checkElement(events, 'dashboard-recent-events', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the loot-pulse widget baseline (tweens frozen)', async () => {
		// Default widget tab. Its SVG charts mount with JS-driven y-axis rescale
		// tweens; the e2e build settles them instantly so the shot captures the
		// settled end-state rather than a mid-rescale frame.
		const widgets = await $('[data-guide-anchor="dashboard-widgets-area"]');
		await widgets.waitForExist({ timeout: 10000 });
		// Gate on the real DATA chart, not just any svg: the empty-state
		// placeholder is an aria-hidden svg with no aria-label, so `svg` alone
		// passes pre-hydration. The data sparkline carries this aria-label.
		await widgets
			.$('svg[aria-label^="Per-kill multiplier sparkline"]')
			.waitForExist({ timeout: 12000 });
		await animationsFinished(browser);
		// The panel must have reached its full (non-collapsed) height: the prior
		// 88% diff was the flex-1 widgets area squeezed to the tab strip mid-reflow.
		await settleSize(browser, widgets, { min: 500 });
		const mismatch = await browser.checkElement(widgets, 'dashboard-loot-pulse', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the loot-composition widget baseline', async () => {
		const widgets = await $('[data-guide-anchor="dashboard-widgets-area"]');
		await widgets.scrollIntoView({ block: 'center' });
		const lootTab = await widgets.$('[data-tab-id="loot"]');
		await lootTab.waitForClickable({ timeout: 10000 });
		await lootTab.click();
		// Hydrates from the session-detail fixture (fetched once the snapshot's
		// session_id is known); gate on a known loot row AND the full ranked list
		// (5 fixture rows), not a fixed pause, so a pre-fetch / collapsed capture
		// cannot pass.
		await browser.waitUntil(async () => (await widgets.getText()).includes('Animal Oil Residue'), {
			timeout: 12000,
			timeoutMsg: 'loot composition never hydrated the fixture loot rows',
		});
		await browser.waitUntil(async () => (await widgets.$$('ul li')).length >= 5, {
			timeout: 12000,
			timeoutMsg: 'loot composition never rendered all ranked rows',
		});
		await animationsFinished(browser);
		await settleSize(browser, widgets, { min: 500 });
		const mismatch = await browser.checkElement(widgets, 'dashboard-loot-composition', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});
});
