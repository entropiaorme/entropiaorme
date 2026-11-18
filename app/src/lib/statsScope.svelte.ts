/**
 * The instance-versus-lifetime scope the stat surfaces render in.
 *
 * One scope, shared by the dashboard and the overlay, because the two
 * show the same figures and a split mode would let them disagree about
 * what the numbers beside each other mean. It persists like the stat
 * selection itself, so a session resumes reading the way it was left,
 * and broadcasts across windows (the overlay is its own Tauri window)
 * the way the overlay's stat prefs already do.
 *
 * Scope is a VIEW state: flipping it never touches the user's stat
 * selection. Both scopes draw the same tiles, so a round trip is a no-op
 * on preferences and on the layout.
 */

import { emit } from '@tauri-apps/api/event';
import { getPreference, setPreference } from './preferences';

export type StatsScope = 'instance' | 'lifetime';

const KEY_SCOPE = 'statsScope';

/** Cross-window broadcast, so flipping on one surface moves the other. */
export const STATS_SCOPE_CHANGED_EVENT = 'stats-scope-changed';

export const DEFAULT_STATS_SCOPE: StatsScope = 'instance';

function sanitise(value: unknown): StatsScope {
	return value === 'lifetime' ? 'lifetime' : DEFAULT_STATS_SCOPE;
}

let scope = $state<StatsScope>(DEFAULT_STATS_SCOPE);

// A direct write is transient (the cross-window sync); a persisted
// change goes through `setStatsScope`.
export const statsScope = {
	get current(): StatsScope {
		return scope;
	},
	set current(value: StatsScope) {
		scope = value;
	},
};

export async function initStatsScope(): Promise<void> {
	scope = sanitise(await getPreference<unknown>(KEY_SCOPE, DEFAULT_STATS_SCOPE));
}

export async function setStatsScope(value: StatsScope): Promise<void> {
	const clean = sanitise(value);
	scope = clean;
	await setPreference(KEY_SCOPE, clean);
	void emit(STATS_SCOPE_CHANGED_EVENT, clean);
}
