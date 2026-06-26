import { $, browser, expect } from '@wdio/globals';
import { ensureDashboard, ensureViewport } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Analytics visual regression, captured in the real shell. Analytics is the
// densest deterministic surface in the app: the Overview tab's donut (fixed
// colour-by-key map, no hover by default) and cumulative-P&L polyline derive
// purely from the pinned fixture, with no timer / Date.now / random in the
// render path. Each tab is element-scoped via its guide anchor and gated on the
// loaded (non-skeleton) state. Baselines are generated and diffed in the same
// rendering environment (WebView2 on Windows); regenerate with
// `npm run test:visual:update`.
const VISUAL_OPTS = { disableCSSAnimations: true, hideScrollBars: true };
const BUDGET = 1.5;

describe('analytics visual regression (native Tauri shell)', () => {
	before(async () => {
		// Establish onboarding (persists prefs) then load the analytics route in
		// the real shell.
		await ensureDashboard(browser, DEV_URL);
		await browser.url(`${DEV_URL}analytics`);
		// The route navigation is the clearest inner-viewport-collapse trigger, so
		// recover the viewport before the tab captures below.
		await ensureViewport(browser);
		// Log the capture-context viewport: the baselines are captured at a fixed
		// window size, so a reflowed (wrong-size) layout here is the most likely
		// cause of a whole-tab diff and must be visible in the run output.
		const vp = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
		console.log(`[analytics] viewport after load: ${vp.w}x${vp.h}`);
	});

	async function selectTab(id) {
		const tab = await $(`[role="tablist"] [data-tab-id="${id}"]`);
		await tab.waitForClickable({ timeout: 10000 });
		await tab.click();
	}

	it('matches the overview tab baseline (donut + cumulative P&L)', async () => {
		// Overview is the default tab; the area renders only once data has loaded.
		const area = await $('[data-guide-anchor="analytics-overview-area"]');
		await area.waitForExist({ timeout: 15000 });
		// Gate on the charts actually rendering, not just the container existing:
		// the donut + cumulative-P&L are SVGs, so wait for one to mount before the
		// shot, or a half-onboarded / pre-hydration capture swings the diff wildly.
		await area.$('svg').waitForExist({ timeout: 12000 });
		// In parity with the ledger/activity shots, gate on hydrated content: the
		// donut centre renders "return rate" only once the fixture data has
		// populated the Overview, so a pre-hydration frame is never captured.
		await browser.waitUntil(async () => (await area.getText()).includes('return rate'), {
			timeout: 12000,
			timeoutMsg: 'overview never hydrated the fixture data',
		});
		await browser.pause(700);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'analytics-overview', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the ledger tab baseline', async () => {
		await selectTab('ledger');
		const area = await $('[data-guide-anchor="analytics-ledger-area"]');
		await area.waitForExist({ timeout: 15000 });
		await browser.waitUntil(async () => (await area.getText()).includes('L weapon purchase'), {
			timeout: 12000,
			timeoutMsg: 'ledger never hydrated the fixture entries',
		});
		await browser.pause(500);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'analytics-ledger', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the activity tab baseline', async () => {
		await selectTab('activity');
		const area = await $('[data-guide-anchor="analytics-activity-area"]');
		await area.waitForExist({ timeout: 15000 });
		await browser.waitUntil(async () => (await area.getText()).includes('Atrox Young'), {
			timeout: 12000,
			timeoutMsg: 'activity never hydrated the fixture comparisons',
		});
		await browser.pause(500);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'analytics-activity', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});
});
