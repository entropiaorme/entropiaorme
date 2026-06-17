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
	it('falls back to the surface-appropriate defaults when the stored value is not an array (null)', async () => {
		getPreference.mockResolvedValue(null);
		const {
			initStatsCustomisation,
			dashboardStats,
			overlayStats,
			DEFAULT_STAT_PREFS,
			DEFAULT_OVERLAY_PREFS,
		} = await loadModule();
		await initStatsCustomisation();

		// sanitise(null, DEFAULT_STAT_PREFS) -> DEFAULT_STAT_PREFS for the dashboard.
		expect(get(dashboardStats)).toEqual(DEFAULT_STAT_PREFS);
		// A corrupt overlay pref recovers to the OVERLAY defaults, not the
		// dashboard's enabled flags; reorderToMatch is a no-op since both share
		// the registry id order.
		expect(get(overlayStats)).toEqual(DEFAULT_OVERLAY_PREFS);
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
	it('normalises the value to the canonical 19-stat list (not the raw reference)', async () => {
		const value = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'cycled' as StatId, enabled: false },
		];
		const { setDashboardStats, dashboardStats } = await loadModule();
		await setDashboardStats(value);
		const dash = get(dashboardStats);
		// Sanitised: a fresh array of all 19 ids, the provided ones kept first in order.
		expect(dash).not.toBe(value);
		expect(dash).toHaveLength(19);
		expect(dash[0]).toEqual({ id: 'net', enabled: true });
		expect(dash[1]).toEqual({ id: 'cycled', enabled: false });
		expect(new Set(ids(dash)).size).toBe(19);
		for (const p of dash.slice(2)) expect(p.enabled).toBe(false);
	});

	it('persists the sanitised dashboard list under the dashboard key', async () => {
		const value = [{ id: 'net' as StatId, enabled: true }];
		const { setDashboardStats, dashboardStats } = await loadModule();
		await setDashboardStats(value);
		// The persisted payload is the sanitised store value, not the raw input.
		expect(setPreference).toHaveBeenCalledWith('dashboardStats', get(dashboardStats));
		expect(get(dashboardStats)).toHaveLength(19);
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
		// Overlay is the full 19 in the sanitised dashboard order (provided ids first).
		expect(ov).toHaveLength(19);
		expect(ids(ov).slice(0, 2)).toEqual(['multiplier_last', 'net']);
		// Enabled flags come from the overlay's PRIOR state, not from `value`.
		expect(ov.find((p) => p.id === 'multiplier_last')?.enabled).toBe(true);
		expect(ov.find((p) => p.id === 'net')?.enabled).toBe(true);
		// A stat not enabled in the prior overlay stays disabled.
		expect(ov.find((p) => p.id === 'cycled')?.enabled).toBe(false);
		// Sanity: the overlay's net flag is its own default (true).
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
		// The reordered overlay matches the sanitised dashboard id order (provided first).
		expect(reordered).toHaveLength(19);
		expect(ids(reordered).slice(0, 2)).toEqual(['cycled', 'net']);
	});

	it('writes both preference keys (dashboard first, then overlay)', async () => {
		const value = [{ id: 'net' as StatId, enabled: true }];
		const { setDashboardStats } = await loadModule();
		await setDashboardStats(value);
		expect(setPreference).toHaveBeenCalledTimes(2);
		expect(setPreference.mock.calls[0][0]).toBe('dashboardStats');
		expect(setPreference.mock.calls[1][0]).toBe('overlayStats');
	});

	it('sanitises the value: duplicate ids are collapsed (first wins) in both stores', async () => {
		// The setter normalises through sanitise, so a caller-supplied duplicate id
		// is deduped (first occurrence wins) rather than propagating into the stores.
		const value = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'net' as StatId, enabled: false },
		];
		const { setDashboardStats, dashboardStats, overlayStats } = await loadModule();
		await setDashboardStats(value);

		// Dashboard: net appears exactly once (first occurrence, enabled:true); 19 total.
		const dash = get(dashboardStats);
		expect(dash).toHaveLength(19);
		expect(ids(dash).filter((id) => id === 'net')).toEqual(['net']);
		expect(dash[0]).toEqual({ id: 'net', enabled: true });
		// The overlay reslave is likewise free of the duplicate.
		expect(get(overlayStats)).toHaveLength(19);
		expect(ids(get(overlayStats)).filter((id) => id === 'net')).toEqual(['net']);
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
		// Sanitised dashboard order leads with rate, net (the provided ids); 19 total.
		expect(ov).toHaveLength(19);
		expect(ids(ov).slice(0, 2)).toEqual(['rate', 'net']);
		// Enabled flags come from the overlay value passed (rate:false, net:true).
		expect(ov.find((p) => p.id === 'rate')?.enabled).toBe(false);
		expect(ov.find((p) => p.id === 'net')?.enabled).toBe(true);
	});
});
