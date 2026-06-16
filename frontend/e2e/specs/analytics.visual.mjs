import { $, browser, expect } from '@wdio/globals';
import { ensureDashboard } from '../helpers/onboarding.mjs';
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
	});

	async function selectTab(id) {
		const tab = await $(`[role="tablist"] [data-tab-id="${id}"]`);
		await tab.waitForExist({ timeout: 10000 });
		await tab.click();
	}

	it('matches the overview tab baseline (donut + cumulative P&L)', async () => {
		// Overview is the default tab; the area renders only once data has loaded.
		const area = await $('[data-guide-anchor="analytics-overview-area"]');
		await area.waitForExist({ timeout: 15000 });
		await browser.pause(700);
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
		const mismatch = await browser.checkElement(area, 'analytics-activity', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});
});
