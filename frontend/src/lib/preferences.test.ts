// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// THE WRINKLE: `inTauri` in src/lib/preferences.ts is captured ONCE at module
// import time from `'__TAURI_INTERNALS__' in window`. So each scenario must
// (a) set or delete window.__TAURI_INTERNALS__, then (b) vi.resetModules(), then
// (c) dynamically `await import('$lib/preferences')` so a fresh module reads the
// just-mutated window. A top-of-file static import would freeze inTauri once.
//
// The two Tauri seams are mocked with hoisted vi.fns (vi.mock is hoisted above
// imports and survives vi.resetModules, but the module-internal `storePromise`
// memo is reset by it, which is exactly what the memoisation test needs).

const { loadMock, getMock, setMock, dataDirMock, joinMock } = vi.hoisted(() => ({
	loadMock: vi.fn(),
	getMock: vi.fn(),
	setMock: vi.fn(),
	dataDirMock: vi.fn(),
	joinMock: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-store', () => ({
	load: loadMock,
}));

vi.mock('@tauri-apps/api/path', () => ({
	dataDir: dataDirMock,
	join: joinMock,
}));

type Prefs = typeof import('$lib/preferences');

// Re-import a fresh copy of the module after setting/clearing the Tauri flag,
// so `inTauri` reflects the desired environment for the scenario under test.
async function importPreferences(tauri: boolean): Promise<Prefs> {
	if (tauri) {
		(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
	} else {
		delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
	}
	vi.resetModules();
	return import('$lib/preferences');
}

beforeEach(() => {
	loadMock.mockReset();
	getMock.mockReset();
	setMock.mockReset();
	dataDirMock.mockReset();
	joinMock.mockReset();

	// Default happy-path wiring for the store seam; individual tests override.
	getMock.mockResolvedValue(null);
	setMock.mockResolvedValue(undefined);
	loadMock.mockResolvedValue({ get: getMock, set: setMock });
	dataDirMock.mockResolvedValue('/data');
	joinMock.mockResolvedValue('/data/EntropiaOrme/settings.json');

	localStorage.clear();
});

afterEach(() => {
	delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
	vi.unstubAllGlobals();
	localStorage.clear();
});

describe('getPreference (inTauri = true)', () => {
	it('returns the value the store yields', async () => {
		getMock.mockResolvedValue('stored-value');
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('stored-value');
		expect(loadMock).toHaveBeenCalledTimes(1);
		expect(getMock).toHaveBeenCalledWith('theme');
	});

	it('returns the default when the store yields null', async () => {
		getMock.mockResolvedValue(null);
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
	});

	it('returns the default when the store yields undefined', async () => {
		getMock.mockResolvedValue(undefined);
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
	});

	it('preserves a falsy non-null stored value (0) rather than defaulting', async () => {
		// Guards the `=== undefined || === null` check against a naive falsy test.
		getMock.mockResolvedValue(0);
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('count', 99)).resolves.toBe(0);
	});

	it('falls through to localStorage when store.get throws', async () => {
		getMock.mockRejectedValue(new Error('store boom'));
		localStorage.setItem('theme', JSON.stringify('from-local'));
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('from-local');
	});

	it('falls through to localStorage when load throws', async () => {
		loadMock.mockRejectedValue(new Error('load boom'));
		localStorage.setItem('theme', JSON.stringify('from-local'));
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('from-local');
	});

	it('falls through and returns default when store throws and localStorage is empty', async () => {
		getMock.mockRejectedValue(new Error('store boom'));
		const { getPreference } = await importPreferences(true);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
	});
});

describe('getPreference (inTauri = false)', () => {
	it('returns the default when the key is missing from localStorage', async () => {
		const { getPreference } = await importPreferences(false);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
		expect(loadMock).not.toHaveBeenCalled();
	});

	it('returns the parsed value for present valid JSON', async () => {
		localStorage.setItem('theme', JSON.stringify({ mode: 'dark' }));
		const { getPreference } = await importPreferences(false);
		await expect(getPreference('theme', { mode: 'light' })).resolves.toEqual({
			mode: 'dark',
		});
	});

	it('returns the default when stored JSON is invalid (parse throws, caught)', async () => {
		localStorage.setItem('theme', 'not-json{');
		const { getPreference } = await importPreferences(false);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
	});

	it('never touches the store seam when not in Tauri', async () => {
		localStorage.setItem('theme', JSON.stringify('x'));
		const { getPreference } = await importPreferences(false);
		await getPreference('theme', 'fallback');
		expect(loadMock).not.toHaveBeenCalled();
		expect(getMock).not.toHaveBeenCalled();
	});
});

describe('setPreference (inTauri = true)', () => {
	it('writes through the store and does not write localStorage', async () => {
		const { setPreference } = await importPreferences(true);
		await setPreference('theme', { mode: 'dark' });
		expect(setMock).toHaveBeenCalledWith('theme', { mode: 'dark' });
		expect(localStorage.getItem('theme')).toBeNull();
	});

	it('falls through to localStorage when store.set throws', async () => {
		setMock.mockRejectedValue(new Error('set boom'));
		const { setPreference } = await importPreferences(true);
		await setPreference('theme', { mode: 'dark' });
		expect(localStorage.getItem('theme')).toBe(JSON.stringify({ mode: 'dark' }));
	});

	it('falls through to localStorage when load throws', async () => {
		loadMock.mockRejectedValue(new Error('load boom'));
		const { setPreference } = await importPreferences(true);
		await setPreference('count', 7);
		expect(localStorage.getItem('count')).toBe(JSON.stringify(7));
	});
});

describe('setPreference (inTauri = false)', () => {
	it('writes the JSON-stringified value to localStorage', async () => {
		const { setPreference } = await importPreferences(false);
		await setPreference('theme', { mode: 'dark' });
		expect(setMock).not.toHaveBeenCalled();
		expect(localStorage.getItem('theme')).toBe(JSON.stringify({ mode: 'dark' }));
	});
});

describe('localStorage absent (typeof localStorage === undefined)', () => {
	// The final fall-through arm of both functions, unreachable under happy-dom
	// unless localStorage is stubbed away. Covers a frozen/headless runtime with
	// neither Tauri nor a DOM storage.
	it('getPreference returns the default when there is no localStorage', async () => {
		const { getPreference } = await importPreferences(false);
		vi.stubGlobal('localStorage', undefined);
		await expect(getPreference('theme', 'fallback')).resolves.toBe('fallback');
	});

	it('setPreference silently drops the write when there is no localStorage', async () => {
		const { setPreference } = await importPreferences(false);
		vi.stubGlobal('localStorage', undefined);
		// Silent no-op: resolves without throwing and never reaches the store.
		await expect(setPreference('theme', { mode: 'dark' })).resolves.toBeUndefined();
		expect(setMock).not.toHaveBeenCalled();
	});
});

describe('getStore memoisation (inTauri = true)', () => {
	it('loads the store exactly once across multiple getPreference calls', async () => {
		const { getPreference } = await importPreferences(true);
		await getPreference('a', null);
		await getPreference('b', null);
		await getPreference('c', null);
		expect(loadMock).toHaveBeenCalledTimes(1);
		// dataDir/join are part of the memoised path resolution: also once each.
		expect(dataDirMock).toHaveBeenCalledTimes(1);
		expect(joinMock).toHaveBeenCalledTimes(1);
	});

	it('resolves the store path from dataDir + join with the app folder + file', async () => {
		const { getPreference } = await importPreferences(true);
		await getPreference('a', null);
		expect(dataDirMock).toHaveBeenCalledTimes(1);
		expect(joinMock).toHaveBeenCalledWith('/data', 'EntropiaOrme', 'settings.json');
		expect(loadMock).toHaveBeenCalledWith('/data/EntropiaOrme/settings.json', {
			autoSave: true,
			defaults: {},
		});
	});

	it('keeps the rejected store memo: a failed load is not retried on later calls', async () => {
		// storePromise memoises the rejected promise, so once load throws every
		// subsequent get/set reuses it (and falls through) rather than re-loading.
		loadMock.mockRejectedValue(new Error('load boom'));
		const { getPreference } = await importPreferences(true);
		await getPreference('a', 'fallback');
		await getPreference('b', 'fallback');
		expect(loadMock).toHaveBeenCalledTimes(1);
	});
});
