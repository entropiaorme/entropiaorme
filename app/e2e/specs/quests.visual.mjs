import { $, browser, expect } from '@wdio/globals';
import { ensureDashboard, ensureViewport } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Quests visual regression, captured in the real shell. The quest read hits
// the real facade over an empty database, so this shot pins the surface's
// default empty state. Deterministic by construction (no fixture
// data reaches this surface); populated-state coverage needs fixture-served
// quest data and is a deliberate follow-up. Baselines are generated and
// diffed in the same rendering environment (WebView2 on Windows); regenerate
// with `npm run test:visual:update`.
const VISUAL_OPTS = { disableCSSAnimations: true, hideScrollBars: true };
const BUDGET = 1.5;

describe('quests visual regression (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
		await browser.url(`${DEV_URL}quests`);
		await ensureViewport(browser);
		const vp = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
		console.log(`[quests] viewport after load: ${vp.w}x${vp.h}`);
	});

	it('matches the quests empty-state baseline', async () => {
		const area = await $('main');
		await area.waitForExist({ timeout: 15000 });
		await browser.waitUntil(async () => (await area.getText()).includes('No quests yet'), {
			timeout: 12000,
			timeoutMsg: 'quests list never settled into the empty state',
		});
		await browser.pause(500);
		await ensureViewport(browser);
		const mismatch = await browser.checkElement(area, 'quests-list-empty', VISUAL_OPTS);
		expect(mismatch).toBeLessThanOrEqual(BUDGET);
	});
});
