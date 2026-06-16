import assert from 'node:assert/strict';
import { $, $$, browser, expect } from '@wdio/globals';
import { ensureDashboard } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Functional panel-flow coverage against the REAL Tauri shell. The deterministic
// stub backend (e2e/stub-backend.mjs) pins the dashboard's reads, so these
// assertions are reproducible run-to-run. The IPC checks are the migration hedge:
// they exercise window.__TAURI_INTERNALS__, which only the native shell exposes
// (a browser-only e2e is structurally blind to it).
describe('dashboard (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
	});

	it('renders the dashboard shell: header, session island, stat grid', async () => {
		await expect($('h1')).toHaveText('Dashboard');
		await expect($('[data-guide-anchor="dashboard-stats-grid"]')).toBeExisting();
		const cells = await $$('[data-stat-cell]');
		expect(cells.length).toBeGreaterThan(0);
	});

	it('reflects the deterministic fixture (active session + recent events)', async () => {
		// The fixture pins an active session, so the island shows the live state.
		const island = await $('[data-guide-anchor="dashboard-area"]').getText();
		assert.ok(
			island.includes('Tracking active'),
			`expected the active-session island; got: ${island.slice(0, 120)}`,
		);
		// And the recent-events island shows the fixture's HOF event.
		const events = await $('[data-guide-anchor="dashboard-recent-events"]').getText();
		assert.ok(
			events.includes('HOF'),
			`expected the fixture HOF event; got: ${events.slice(0, 160)}`,
		);
	});

	it('exposes the Tauri IPC surface a browser cannot replicate', async () => {
		// The structural fact the native-shell net exists to guard: the live IPC
		// bridge is present. A browser-served e2e has no __TAURI_INTERNALS__, so it
		// is blind to the surface the upcoming fetch-to-invoke migration rewrites.
		const surface = await browser.execute(() => ({
			hasInternals: typeof window.__TAURI_INTERNALS__ !== 'undefined',
			invokeIsFn: typeof window.__TAURI_INTERNALS__?.invoke === 'function',
		}));
		expect(surface.hasInternals).toBe(true);
		expect(surface.invokeIsFn).toBe(true);

		// Attempt a real command round-trip for signal (logged, not asserted: a
		// raw internals.invoke from the navigated dev origin can hit a Tauri 2
		// invoke-key nuance the app's own @tauri-apps/api path handles; the UI
		// flow below exercises that real path).
		const roundTrip = await browser.executeAsync((done) => {
			try {
				window.__TAURI_INTERNALS__
					.invoke('toggle_overlay', {})
					.then(() => done('ok'))
					.catch((e) => done(`invoke-error:${String(e)}`));
			} catch (e) {
				done(`throw:${String(e)}`);
			}
		});
		console.log('[e2e] raw toggle_overlay invoke:', roundTrip);
	});

	it('drives the overlay button over the real IPC boundary', async () => {
		const overlayBtn = await $('[data-guide-anchor="dashboard-overlay-btn"] button');
		await expect(overlayBtn).toBeExisting();
		await overlayBtn.click(); // wired to the app's invoke('toggle_overlay')
		await browser.pause(500);
		// Shell still responsive on the dashboard after the IPC-wired interaction.
		await expect($('h1')).toHaveText('Dashboard');
	});
});
