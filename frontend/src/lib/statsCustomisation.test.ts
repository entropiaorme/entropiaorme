import { get } from 'svelte/store';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ALL_STAT_IDS, STAT_DEFS, type StatId } from './statsRegistry';

// Mock the two side-effecting seams. These vi.fn()s are hoisted alongside the
// vi.mock factory; we read/reset them per test. Because the module under test
// holds singleton stores, each test re-imports it fresh via vi.resetModules()
// + dynamic import() so store state is order-independent.
const getPreference = vi.fn();
const setPreference = vi.fn();
const emit = vi.fn();

vi.mock('./preferences', () => ({
	getPreference: (...args: unknown[]) => getPreference(...args),
	setPreference: (...args: unknown[]) => setPreference(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
	emit: (...args: unknown[]) => emit(...args),
}));

type Mod = typeof import('./statsCustomisation');

// Fresh module (and fresh stores) per call.
async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./statsCustomisation');
}

const ids = (prefs: { id: StatId }[]): StatId[] => prefs.map((p) => p.id);

beforeEach(() => {
	getPreference.mockReset();
	setPreference.mockReset();
	emit.mockReset();
	// setPreference is awaited; ensure it resolves rather than returning undefined
	// (which await tolerates, but be explicit).
	setPreference.mockResolvedValue(undefined);
});

afterEach(() => {
	vi.useRealTimers();
});

describe('DEFAULT_STAT_PREFS / DEFAULT_OVERLAY_PREFS', () => {
	it('both have one entry per registered stat id, in registry order', async () => {
		const { DEFAULT_STAT_PREFS, DEFAULT_OVERLAY_PREFS } = await loadModule();
		expect(DEFAULT_STAT_PREFS).toHaveLength(19);
		expect(DEFAULT_OVERLAY_PREFS).toHaveLength(19);
		expect(ids(DEFAULT_STAT_PREFS)).toEqual(ALL_STAT_IDS);
		expect(ids(DEFAULT_OVERLAY_PREFS)).toEqual(ALL_STAT_IDS);
	});

	it('mirrors STAT_DEFS defaultEnabled for the dashboard defaults', async () => {
		const { DEFAULT_STAT_PREFS } = await loadModule();
		for (const pref of DEFAULT_STAT_PREFS) {
			expect(pref.enabled).toBe(STAT_DEFS[pref.id].defaultEnabled);
		}
	});

	it('mirrors STAT_DEFS defaultOverlayEnabled (??false) for the overlay defaults', async () => {
		const { DEFAULT_OVERLAY_PREFS } = await loadModule();
		for (const pref of DEFAULT_OVERLAY_PREFS) {
			expect(pref.enabled).toBe(STAT_DEFS[pref.id].defaultOverlayEnabled ?? false);
		}
		// Concretely, only the two stats flagged defaultOverlayEnabled are on.
		const enabled = DEFAULT_OVERLAY_PREFS.filter((p) => p.enabled).map((p) => p.id);
		expect(enabled).toEqual(['net', 'multiplier_last']);
	});

	it('initial store values are the defaults', async () => {
		const { dashboardStats, overlayStats, DEFAULT_STAT_PREFS, DEFAULT_OVERLAY_PREFS } =
			await loadModule();
		expect(get(dashboardStats)).toEqual(DEFAULT_STAT_PREFS);
		expect(get(overlayStats)).toEqual(DEFAULT_OVERLAY_PREFS);
	});
});

describe('initStatsCustomisation', () => {
	it('falls back to the 19 defaults when the stored value is not an array (null)', async () => {
		getPreference.mockResolvedValue(null);
		const { initStatsCustomisation, dashboardStats, overlayStats, DEFAULT_STAT_PREFS } =
			await loadModule();
		await initStatsCustomisation();

		// sanitise(null) -> DEFAULT_STAT_PREFS for the dashboard.
		expect(get(dashboardStats)).toEqual(DEFAULT_STAT_PREFS);
		// Overlay is sanitise(null) -> DEFAULT_STAT_PREFS, then reorderToMatch to
		// the dashboard order. Note: sanitise() always returns DEFAULT_STAT_PREFS
		// (dashboard-flavoured enabled flags) for a non-array, NOT the overlay
		// defaults; reorderToMatch then preserves those flags in dashboard order.
		// So the resulting overlay equals DEFAULT_STAT_PREFS, not DEFAULT_OVERLAY_PREFS.
		expect(get(overlayStats)).toEqual(DEFAULT_STAT_PREFS);
		expect(ids(get(overlayStats))).toEqual(ALL_STAT_IDS);
	});

	it('reads both dashboard and overlay keys with their default fallbacks', async () => {
		getPreference.mockResolvedValue(null);
		const { initStatsCustomisation, DEFAULT_STAT_PREFS, DEFAULT_OVERLAY_PREFS } =
			await loadModule();
		await initStatsCustomisation();

		expect(getPreference).toHaveBeenCalledTimes(2);
		expect(getPreference).toHaveBeenCalledWith('dashboardStats', DEFAULT_STAT_PREFS);
		expect(getPreference).toHaveBeenCalledWith('overlayStats', DEFAULT_OVERLAY_PREFS);
	});

	it('cleans a valid partial array and appends missing ids disabled (always 19, no dupes)', async () => {
		// Dashboard: a partial list out of registry order, overlay: a different partial.
		getPreference.mockImplementation(async (key: string) => {
			if (key === 'dashboardStats') {
				return [
					{ id: 'net', enabled: true },
					{ id: 'cycled', enabled: false },
				];
			}
			return [{ id: 'multiplier_last', enabled: true }];
		});
		const { initStatsCustomisation, dashboardStats, overlayStats } = await loadModule();
		await initStatsCustomisation();

		const dash = get(dashboardStats);
		expect(dash).toHaveLength(19);
		// Provided ids keep their position/order; the rest appended in registry order.
		expect(dash[0]).toEqual({ id: 'net', enabled: true });
		expect(dash[1]).toEqual({ id: 'cycled', enabled: false });
		// No duplicate ids.
		expect(new Set(ids(dash)).size).toBe(19);
		// Appended ids are disabled.
		const appended = dash.slice(2);
		for (const p of appended) expect(p.enabled).toBe(false);

		// Overlay output is reorderToMatch'd to the dashboard's id order.
		const ov = get(overlayStats);
		expect(ov).toHaveLength(19);
		expect(ids(ov)).toEqual(ids(dash));
		// Its own enabled flag (multiplier_last:true) is preserved through the reorder.
		expect(ov.find((p) => p.id === 'multiplier_last')?.enabled).toBe(true);
		// Stats absent from the overlay's stored array land disabled.
		expect(ov.find((p) => p.id === 'net')?.enabled).toBe(false);
	});

	it('drops non-string ids, unknown ids, and duplicates; coerces enabled via Boolean', async () => {
		getPreference.mockImplementation(async (key: string) => {
			if (key === 'dashboardStats') {
				return [
					{ id: 'cycled', enabled: 1 }, // truthy non-bool -> true
					{ id: 'cycled', enabled: false }, // duplicate -> dropped
					{ id: 'loot_tt', enabled: 0 }, // falsy non-bool -> false
					{ id: 42, enabled: true }, // non-string id -> dropped
					{ id: 'not_a_real_stat', enabled: true }, // unknown id -> dropped
					{ enabled: true }, // missing id -> dropped
					null, // non-object -> dropped
					'cycled', // string item (not object) -> dropped
				];
			}
			return null;
		});
		const { initStatsCustomisation, dashboardStats } = await loadModule();
		await initStatsCustomisation();

		const dash = get(dashboardStats);
		expect(dash).toHaveLength(19);
		expect(new Set(ids(dash)).size).toBe(19);
		// cycled kept from the FIRST occurrence (enabled:1 -> Boolean -> true).
		expect(dash[0]).toEqual({ id: 'cycled', enabled: true });
		// loot_tt kept, enabled:0 -> false.
		expect(dash[1]).toEqual({ id: 'loot_tt', enabled: false });
		// The unknown id never appears.
		expect(ids(dash)).not.toContain('not_a_real_stat' as StatId);
		// Everything after the two cleaned entries is an appended (disabled) default.
		for (const p of dash.slice(2)) expect(p.enabled).toBe(false);
	});

	it('produces all 19 ids exactly once even from an empty array', async () => {
		getPreference.mockResolvedValue([]);
		const { initStatsCustomisation, dashboardStats, overlayStats } = await loadModule();
		await initStatsCustomisation();

		const dash = get(dashboardStats);
		expect(ids(dash)).toEqual(ALL_STAT_IDS); // empty -> pure append in registry order
		for (const p of dash) expect(p.enabled).toBe(false);
		expect(ids(get(overlayStats))).toEqual(ALL_STAT_IDS);
	});
});

describe('setDashboardStats', () => {
	it('sets the store to the exact value passed', async () => {
		const value = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'cycled' as StatId, enabled: false },
		];
		const { setDashboardStats, dashboardStats } = await loadModule();
		await setDashboardStats(value);
		// Stored reference is the value as-is (no sanitise on the setter path).
		expect(get(dashboardStats)).toBe(value);
	});

	it('persists the dashboard value under the dashboard key', async () => {
		const value = [{ id: 'net' as StatId, enabled: true }];
		const { setDashboardStats } = await loadModule();
		await setDashboardStats(value);
		expect(setPreference).toHaveBeenCalledWith('dashboardStats', value);
	});

	it('reslaves the overlay to the new dashboard order, preserving overlay enabled flags', async () => {
		const { setDashboardStats, overlayStats, DEFAULT_OVERLAY_PREFS } = await loadModule();
		// overlayStats starts at DEFAULT_OVERLAY_PREFS: net=true, multiplier_last=true.
		const value = [
			{ id: 'multiplier_last' as StatId, enabled: false },
			{ id: 'net' as StatId, enabled: true },
		];
		await setDashboardStats(value);

		const ov = get(overlayStats);
		// Overlay is clamped to the dashboard value's id order (only those 2 ids).
		expect(ids(ov)).toEqual(['multiplier_last', 'net']);
		// Enabled flags come from the overlay's PRIOR state, not from `value`.
		expect(ov).toEqual([
			{ id: 'multiplier_last', enabled: true },
			{ id: 'net', enabled: true },
		]);
		// Sanity: this differs from the dashboard value's own flags.
		expect(DEFAULT_OVERLAY_PREFS.find((p) => p.id === 'net')?.enabled).toBe(true);
	});

	it('persists the reordered overlay and emits the change event exactly once', async () => {
		const { setDashboardStats, OVERLAY_STATS_CHANGED_EVENT, overlayStats } = await loadModule();
		const value = [
			{ id: 'cycled' as StatId, enabled: true },
			{ id: 'net' as StatId, enabled: false },
		];
		await setDashboardStats(value);
		const reordered = get(overlayStats);

		// Overlay persisted with the reordered payload (not `value`).
		expect(setPreference).toHaveBeenCalledWith('overlayStats', reordered);
		expect(emit).toHaveBeenCalledTimes(1);
		expect(emit).toHaveBeenCalledWith(OVERLAY_STATS_CHANGED_EVENT, reordered);
		// The reordered overlay matches the dashboard id order.
		expect(ids(reordered)).toEqual(['cycled', 'net']);
	});

	it('writes both preference keys (dashboard first, then overlay)', async () => {
		const value = [{ id: 'net' as StatId, enabled: true }];
		const { setDashboardStats } = await loadModule();
		await setDashboardStats(value);
		expect(setPreference).toHaveBeenCalledTimes(2);
		expect(setPreference.mock.calls[0][0]).toBe('dashboardStats');
		expect(setPreference.mock.calls[1][0]).toBe('overlayStats');
	});

	it('does NOT sanitise the value: duplicate ids propagate into the overlay order', async () => {
		// Candidate defect: the setters skip sanitise, so a
		// caller-supplied duplicate id passes straight through, and reorderToMatch
		// reproduces it in the overlay's id list (referenceOrder.map over the raw
		// dashboard ids). Pins the current behaviour.
		const value = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'net' as StatId, enabled: false },
		];
		const { setDashboardStats, dashboardStats, overlayStats } = await loadModule();
		await setDashboardStats(value);

		// Dashboard keeps the duplicates verbatim (no sanitise on the setter).
		expect(ids(get(dashboardStats))).toEqual(['net', 'net']);
		// The overlay reslave reproduces the duplicate rather than deduping it.
		expect(ids(get(overlayStats))).toEqual(['net', 'net']);
	});
});

describe('setOverlayStats', () => {
	it('clamps the overlay to the current dashboard order (overlay never owns ordering)', async () => {
		const { setOverlayStats, overlayStats } = await loadModule();
		// dashboardStats starts at DEFAULT_STAT_PREFS (full 19, registry order).
		// Pass an overlay value in a scrambled order with a couple enabled.
		const value = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'cycled' as StatId, enabled: true },
		];
		await setOverlayStats(value);

		const ov = get(overlayStats);
		// Output is the full dashboard order (19 ids), not the 2 passed.
		expect(ids(ov)).toEqual(ALL_STAT_IDS);
		expect(ov).toHaveLength(19);
		// The two enabled ids from `value` are preserved; everything else disabled.
		expect(ov.find((p) => p.id === 'net')?.enabled).toBe(true);
		expect(ov.find((p) => p.id === 'cycled')?.enabled).toBe(true);
		expect(ov.find((p) => p.id === 'rate')?.enabled).toBe(false);
	});

	it('persists the reordered overlay and emits exactly once', async () => {
		const { setOverlayStats, OVERLAY_STATS_CHANGED_EVENT, overlayStats } = await loadModule();
		const value = [{ id: 'net' as StatId, enabled: true }];
		await setOverlayStats(value);
		const reordered = get(overlayStats);

		expect(setPreference).toHaveBeenCalledTimes(1);
		expect(setPreference).toHaveBeenCalledWith('overlayStats', reordered);
		expect(emit).toHaveBeenCalledTimes(1);
		expect(emit).toHaveBeenCalledWith(OVERLAY_STATS_CHANGED_EVENT, reordered);
		// Does NOT touch the dashboard preference key.
		expect(setPreference).not.toHaveBeenCalledWith('dashboardStats', expect.anything());
	});

	it('follows a non-default dashboard order set via setDashboardStats', async () => {
		const { setDashboardStats, setOverlayStats, overlayStats } = await loadModule();
		// Re-order the dashboard to a short, scrambled list.
		await setDashboardStats([
			{ id: 'rate' as StatId, enabled: true },
			{ id: 'net' as StatId, enabled: true },
		]);
		// Now an overlay value in the opposite order clamps to the dashboard order.
		await setOverlayStats([
			{ id: 'net' as StatId, enabled: true },
			{ id: 'rate' as StatId, enabled: false },
		]);
		const ov = get(overlayStats);
		expect(ids(ov)).toEqual(['rate', 'net']);
		expect(ov).toEqual([
			{ id: 'rate', enabled: false },
			{ id: 'net', enabled: true },
		]);
	});
});
