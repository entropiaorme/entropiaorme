import { dataDir, join } from '@tauri-apps/api/path';
import { load, type Store } from '@tauri-apps/plugin-store';

const APP_DATA_FOLDER = 'EntropiaOrme';
const STORE_FILE = 'settings.json';

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

let storePromise: Promise<Store> | null = null;
function getStore(): Promise<Store> {
	if (!storePromise) {
		storePromise = (async () => {
			const base = await dataDir();
			const path = await join(base, APP_DATA_FOLDER, STORE_FILE);
			return load(path, { autoSave: true, defaults: {} });
		})();
	}
	return storePromise;
}

export async function getPreference<T>(key: string, defaultValue: T): Promise<T> {
	if (inTauri) {
		try {
			const store = await getStore();
			const value = await store.get<T>(key);
			return value === undefined || value === null ? defaultValue : value;
		} catch {
			// fall through to localStorage
		}
	}
	if (typeof localStorage !== 'undefined') {
		const raw = localStorage.getItem(key);
		if (raw === null) return defaultValue;
		try {
			return JSON.parse(raw) as T;
		} catch {
			return defaultValue;
		}
	}
	return defaultValue;
}

/** Keys earlier versions wrote for features that have since been removed.
 * Purged at every launch so a retired feature leaves nothing behind in the
 * settings file; the purge is idempotent, so entries are only ever added. */
export const RETIRED_PREFERENCE_KEYS: readonly string[] = [
	// The shared market-data service: consent flags, the contributor
	// token, and the cached snapshot.
	'market_data_opt_in',
	'market_data_opt_in_seen',
	'market_contribution_opt_in',
	'market_contributor_token',
	'market_snapshot_cache',
];

export async function purgeRetiredPreferences(): Promise<void> {
	if (inTauri) {
		try {
			const store = await getStore();
			for (const key of RETIRED_PREFERENCE_KEYS) await store.delete(key);
			return;
		} catch {
			// fall through to localStorage
		}
	}
	if (typeof localStorage !== 'undefined') {
		for (const key of RETIRED_PREFERENCE_KEYS) localStorage.removeItem(key);
	}
}

export async function setPreference<T>(key: string, value: T): Promise<void> {
	if (inTauri) {
		try {
			const store = await getStore();
			await store.set(key, value);
			return;
		} catch {
			// fall through to localStorage
		}
	}
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(key, JSON.stringify(value));
	}
}
