import { emit } from '@tauri-apps/api/event';
import { getPreference, setPreference } from './preferences';
import {
	ALL_STAT_IDS,
	HEADLINE_LIFETIME_STAT_IDS,
	isLifetimeCapable,
	STAT_DEFS,
	type StatId,
} from './statsRegistry';
import type { StatsScope } from './statsScope.svelte';

export type StatPref = { id: StatId; enabled: boolean };

const KEY_DASHBOARD = 'dashboardStats';
const KEY_OVERLAY = 'overlayStats';

// Cross-window broadcast: emitted when overlay prefs change so other Tauri
// windows (notably the overlay itself) can sync without reloading.
export const OVERLAY_STATS_CHANGED_EVENT = 'overlay-stats-changed';

export const DEFAULT_STAT_PREFS: StatPref[] = ALL_STAT_IDS.map((id) => ({
	id,
	enabled: STAT_DEFS[id].defaultEnabled,
}));

export const DEFAULT_OVERLAY_PREFS: StatPref[] = ALL_STAT_IDS.map((id) => ({
	id,
	enabled: STAT_DEFS[id].defaultOverlayEnabled ?? false,
}));

let dashboard = $state<StatPref[]>(DEFAULT_STAT_PREFS);
let overlay = $state<StatPref[]>(DEFAULT_OVERLAY_PREFS);

// Direct writes are transient (the overlay window's broadcast sync); a
// persisted change goes through the setters below.
export const dashboardStats = {
	get current(): StatPref[] {
		return dashboard;
	},
	set current(value: StatPref[]) {
		dashboard = value;
	},
};

export const overlayStats = {
	get current(): StatPref[] {
		return overlay;
	},
	set current(value: StatPref[]) {
		overlay = value;
	},
};

// `fallback` is the surface-appropriate default returned when the stored value
// is unusable (not an array): DEFAULT_STAT_PREFS for the dashboard,
// DEFAULT_OVERLAY_PREFS for the overlay. A corrupt overlay pref must recover to
// the overlay defaults, not the dashboard's enabled flags.
function sanitise(prefs: unknown, fallback: StatPref[]): StatPref[] {
	if (!Array.isArray(prefs)) return fallback;
	const seen = new Set<string>();
	const cleaned: StatPref[] = [];
	for (const item of prefs) {
		if (
			item &&
			typeof item === 'object' &&
			typeof (item as StatPref).id === 'string' &&
			ALL_STAT_IDS.includes((item as StatPref).id) &&
			!seen.has((item as StatPref).id)
		) {
			seen.add((item as StatPref).id);
			cleaned.push({
				id: (item as StatPref).id,
				enabled: Boolean((item as StatPref).enabled),
			});
		}
	}
	for (const id of ALL_STAT_IDS) {
		if (!seen.has(id)) cleaned.push({ id, enabled: false });
	}
	return cleaned;
}

// Stat order is global: dashboard is the canonical source. Overlay's order
// is always slaved to it; only per-stat enabled flags vary per surface.
function reorderToMatch(target: StatPref[], referenceOrder: StatId[]): StatPref[] {
	const enabledMap = new Map(target.map((p) => [p.id, p.enabled]));
	return referenceOrder.map((id) => ({
		id,
		enabled: enabledMap.get(id) ?? false,
	}));
}

/**
 * What a surface DRAWS for a given scope, out of what the user has
 * selected. Never a mutation: the stored selection is the superset and
 * survives the flip untouched, so flipping to lifetime and back is a
 * no-op on preferences.
 *
 * Lifetime mode draws the selection narrowed to the stats that have a
 * lifetime form, because a grid mixing family totals with this
 * instance's figures would not be readable as either. From the user's
 * side it looks like the instance-only stats deselected themselves;
 * underneath, nothing was deselected.
 *
 * When the selection contains nothing with a lifetime form, the flip
 * falls back to the headline figures rather than drawing an empty grid,
 * which would read as broken. The fallback keeps the user's own stat
 * ordering.
 *
 * `fallback` exists because that reasoning is the dashboard's, not the
 * overlay's. The overlay renders no pill group at all for an empty
 * selection, so switching every pill off is a supported resting state
 * there; conjuring four back on a scope flip would override a choice
 * the user made deliberately.
 */
export function scopedStats(
	prefs: StatPref[],
	scope: StatsScope,
	{ fallback = true }: { fallback?: boolean } = {},
): StatPref[] {
	const enabled = prefs.filter((pref) => pref.enabled);
	if (scope === 'instance') return enabled;
	const capable = enabled.filter((pref) => isLifetimeCapable(pref.id));
	if (capable.length > 0 || !fallback) return capable;
	const headline = prefs
		.filter((pref) => HEADLINE_LIFETIME_STAT_IDS.includes(pref.id))
		.map((pref) => ({ ...pref, enabled: true }));
	return headline.length > 0
		? headline
		: HEADLINE_LIFETIME_STAT_IDS.map((id) => ({ id, enabled: true }));
}

/**
 * Whether the drawn set is the user's own selection rather than the
 * headline fallback. A fallback view is showing stats the user did not
 * pick, so reordering it would persist an order they never asked for.
 */
export function isOwnSelection(prefs: StatPref[], scope: StatsScope): boolean {
	if (scope === 'instance') return true;
	return prefs.some((pref) => pref.enabled && isLifetimeCapable(pref.id));
}

export async function initStatsCustomisation(): Promise<void> {
	const [d, o] = await Promise.all([
		getPreference<unknown>(KEY_DASHBOARD, DEFAULT_STAT_PREFS),
		getPreference<unknown>(KEY_OVERLAY, DEFAULT_OVERLAY_PREFS),
	]);
	const cleanDashboard = sanitise(d, DEFAULT_STAT_PREFS);
	dashboard = cleanDashboard;
	overlay = reorderToMatch(
		sanitise(o, DEFAULT_OVERLAY_PREFS),
		cleanDashboard.map((p) => p.id),
	);
}

export async function setDashboardStats(value: StatPref[]): Promise<void> {
	// Normalise to the canonical 19-stat list (dedupe ids, drop unknowns, append
	// missing as disabled) so the stored shape matches what initStatsCustomisation
	// produces on load and a caller-supplied duplicate cannot propagate.
	const clean = sanitise(value, DEFAULT_STAT_PREFS);
	dashboard = clean;
	await setPreference(KEY_DASHBOARD, clean);
	// Slave overlay's order to the new dashboard order; preserve its enabled flags.
	const reorderedOverlay = reorderToMatch(
		overlay,
		clean.map((p) => p.id),
	);
	overlay = reorderedOverlay;
	await setPreference(KEY_OVERLAY, reorderedOverlay);
	void emit(OVERLAY_STATS_CHANGED_EVENT, reorderedOverlay);
}

export async function setOverlayStats(value: StatPref[]): Promise<void> {
	// Clamp to dashboard's canonical order: overlay never owns ordering.
	const reordered = reorderToMatch(
		value,
		dashboard.map((p) => p.id),
	);
	overlay = reordered;
	await setPreference(KEY_OVERLAY, reordered);
	void emit(OVERLAY_STATS_CHANGED_EVENT, reordered);
}
