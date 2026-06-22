import { emit } from '@tauri-apps/api/event';
import { get, type Writable, writable } from 'svelte/store';
import { getPreference, setPreference } from './preferences';
import { ALL_STAT_IDS, STAT_DEFS, type StatId } from './statsRegistry';

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

export const dashboardStats: Writable<StatPref[]> = writable(DEFAULT_STAT_PREFS);
export const overlayStats: Writable<StatPref[]> = writable(DEFAULT_OVERLAY_PREFS);

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

// Stat order is global — dashboard is the canonical source. Overlay's order
// is always slaved to it; only per-stat enabled flags vary per surface.
function reorderToMatch(target: StatPref[], referenceOrder: StatId[]): StatPref[] {
	const enabledMap = new Map(target.map((p) => [p.id, p.enabled]));
	return referenceOrder.map((id) => ({
		id,
		enabled: enabledMap.get(id) ?? false,
	}));
}

export async function initStatsCustomisation(): Promise<void> {
	const [d, o] = await Promise.all([
		getPreference<unknown>(KEY_DASHBOARD, DEFAULT_STAT_PREFS),
		getPreference<unknown>(KEY_OVERLAY, DEFAULT_OVERLAY_PREFS),
	]);
	const dashboard = sanitise(d, DEFAULT_STAT_PREFS);
	dashboardStats.set(dashboard);
	const overlay = reorderToMatch(
		sanitise(o, DEFAULT_OVERLAY_PREFS),
		dashboard.map((p) => p.id),
	);
	overlayStats.set(overlay);
}

export async function setDashboardStats(value: StatPref[]): Promise<void> {
	// Normalise to the canonical 19-stat list (dedupe ids, drop unknowns, append
	// missing as disabled) so the stored shape matches what initStatsCustomisation
	// produces on load and a caller-supplied duplicate cannot propagate.
	const clean = sanitise(value, DEFAULT_STAT_PREFS);
	dashboardStats.set(clean);
	await setPreference(KEY_DASHBOARD, clean);
	// Slave overlay's order to the new dashboard order; preserve its enabled flags.
	const reorderedOverlay = reorderToMatch(
		get(overlayStats),
		clean.map((p) => p.id),
	);
	overlayStats.set(reorderedOverlay);
	await setPreference(KEY_OVERLAY, reorderedOverlay);
	void emit(OVERLAY_STATS_CHANGED_EVENT, reorderedOverlay);
}

export async function setOverlayStats(value: StatPref[]): Promise<void> {
	// Clamp to dashboard's canonical order — overlay never owns ordering.
	const reordered = reorderToMatch(
		value,
		get(dashboardStats).map((p) => p.id),
	);
	overlayStats.set(reordered);
	await setPreference(KEY_OVERLAY, reordered);
	void emit(OVERLAY_STATS_CHANGED_EVENT, reordered);
}
