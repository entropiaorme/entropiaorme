import { $, $$, browser, expect } from '@wdio/globals';
import { ensureDashboard } from '../helpers/onboarding.mjs';
import { DEV_URL } from '../wdio.conf.mjs';

// Functional panel-flow coverage against the REAL Tauri shell. The deterministic
// backend stub (the e2e-stub feature's in-process typed read-command handlers)
// pins the dashboard's reads, so these assertions are reproducible run-to-run.
// The IPC checks exercise window.__TAURI_INTERNALS__, which only the native
// shell exposes (a browser-only e2e is structurally blind to it).
describe('dashboard (native Tauri shell)', () => {
	before(async () => {
		await ensureDashboard(browser, DEV_URL);
	});

	it('renders the dashboard shell: header, session island, stat grid', async () => {
		await expect($('h1')).toHaveText('Dashboard');
		await expect($('[data-testid="dashboard-stats-grid"]')).toBeExisting();
		const cells = await $$('[data-stat-cell]');
		expect(cells.length).toBeGreaterThan(0);
	});

	it('reflects the deterministic fixture (active session + recent events)', async () => {
		// The fixture pins an active session, so the island shows the live state.
		// Wait for the snapshot to hydrate (the island flips from "No active
		// session" to the live state) rather than racing the subscribe-then-
		// hydrate sequence: the read is otherwise non-deterministic on a slow
		// shell start.
		const area = await $('[data-testid="dashboard-area"]');
		await browser.waitUntil(async () => (await area.getText()).includes('Tracking active'), {
			timeout: 12000,
			timeoutMsg: 'dashboard never hydrated the active-session fixture',
		});
		// And the recent-events island shows the fixture's HOF event.
		const events = await $('[data-testid="dashboard-recent-events"]');
		await browser.waitUntil(async () => (await events.getText()).includes('HOF'), {
			timeout: 12000,
			timeoutMsg: 'recent-events never showed the fixture HOF event',
		});
	});

	it('floats the session catalogue above the widget grid and keeps it in the viewport', async () => {
		const trigger = await $('[data-testid="dashboard-session"] button[aria-haspopup="menu"]');
		await trigger.waitForClickable({ timeout: 10000 });
		await trigger.click();
		await $('[role="menu"]').waitForDisplayed({ timeout: 10000 });

		const geometry = await browser.execute(() => {
			const panel = document.querySelector('body > [role="menu"]');
			const grid = document.querySelector('[data-testid="dashboard-stats-grid"]');
			const results = panel?.querySelector('[data-testid="definition-results"]');
			if (!(panel instanceof HTMLElement) || !(grid instanceof HTMLElement)) return null;

			const panelRect = panel.getBoundingClientRect();
			const gridRect = grid.getBoundingClientRect();
			const intersection = {
				left: Math.max(panelRect.left, gridRect.left),
				right: Math.min(panelRect.right, gridRect.right),
				top: Math.max(panelRect.top, gridRect.top),
				bottom: Math.min(panelRect.bottom, gridRect.bottom),
			};
			const overlaps =
				intersection.left < intersection.right && intersection.top < intersection.bottom;
			const frontmost = overlaps
				? document.elementFromPoint(
						(intersection.left + intersection.right) / 2,
						(intersection.top + intersection.bottom) / 2,
					)
				: null;
			const panelStyle = getComputedStyle(panel);

			return {
				position: panelStyle.position,
				zIndex: Number(panelStyle.zIndex),
				portalled: panel.parentElement === document.body,
				overlaps,
				frontmost: frontmost instanceof Node && panel.contains(frontmost),
				bounded:
					panelRect.left >= 0 &&
					panelRect.top >= 0 &&
					panelRect.right <= window.innerWidth &&
					panelRect.bottom <= window.innerHeight,
				resultsOverflow:
					results instanceof HTMLElement ? getComputedStyle(results).overflowY : null,
			};
		});

		expect(geometry).not.toBeNull();
		expect(geometry.portalled).toBe(true);
		expect(geometry.position).toBe('fixed');
		expect(geometry.zIndex).toBeGreaterThan(0);
		expect(geometry.overlaps).toBe(true);
		expect(geometry.frontmost).toBe(true);
		expect(geometry.bounded).toBe(true);
		expect(geometry.resultsOverflow).toBe('auto');

		await browser.keys(['Escape']);
		await $('[role="menu"]').waitForExist({ reverse: true, timeout: 5000 });
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
		// The real command round-trip is exercised by the hydration test (the
		// dashboard's reads flow over the typed IPC commands) and the overlay-button
		// test below, both over the app's native IPC at the tauri:// origin.
	});

	it('drives the overlay button over the real IPC boundary', async () => {
		const overlayBtn = await $('[data-testid="dashboard-overlay-btn"] button');
		await expect(overlayBtn).toBeExisting();
		await overlayBtn.waitForClickable({ timeout: 10000 });
		await overlayBtn.click(); // wired to the app's invoke('toggle_overlay')
		await browser.pause(500);
		// Shell still responsive on the dashboard after the IPC-wired interaction.
		await expect($('h1')).toHaveText('Dashboard');
	});
});
