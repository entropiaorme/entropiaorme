import { $, browser, expect } from '@wdio/globals';
import { ensureDashboard, ensureViewport } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Character visual regression, captured in the real shell. The character
// reads hit the real facade over an empty database (nothing scanned or
// calibrated), so these shots pin the surface's DEFAULT (empty) states: the
// stats layout with its never-scanned indicator, and the skills table's
// empty state. Deterministic by construction (no fixture data reaches this
// surface); populated-state coverage needs fixture-served character data and
// is a deliberate follow-up. Baselines are generated and diffed in the same
// rendering environment (WebView2 on Windows); regenerate with
// `npm run test:visual:update`.
const VISUAL_OPTS = { disableCSSAnimations: true, hideScrollBars: true };
const BUDGET = 1.5;

describe('character visual regression (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
		await browser.url(`${DEV_URL}character`);
		await ensureViewport(browser);
		const vp = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
		console.log(`[character] viewport after load: ${vp.w}x${vp.h}`);
	});

	it('matches the stats default-state baseline (attributes, never scanned)', async () => {
		const area = await $('main');
		await area.waitForExist({ timeout: 15000 });
		// Gate on the loaded empty state: the attributes table's notice and the
		// never-scanned calibration line both render only after the reads settle.
		await browser.waitUntil(
			async () => {
				const text = await area.getText();
				return text.includes('No attributes calibrated yet') && text.includes('never');
			},
			{ timeout: 12000, timeoutMsg: 'character stats never settled into the empty state' },
		);
		await browser.pause(500);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'character-stats', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});

	it('matches the skills table empty-state baseline', async () => {
		const skillsTab = await $('button=Skills');
		await skillsTab.waitForClickable({ timeout: 10000 });
		await skillsTab.click();
		const area = await $('[data-testid="character-skills-table"]');
		await area.waitForExist({ timeout: 15000 });
		await browser.waitUntil(
			async () => (await area.getText()).includes('No skills calibrated yet'),
			{
				timeout: 12000,
				timeoutMsg: 'skills table never settled into the empty state',
			},
		);
		await browser.pause(500);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'character-skills-empty', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});
});
