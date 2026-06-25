// @vitest-environment happy-dom
import { get } from 'svelte/store';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the Tauri IPC + event seams and the preferences seam so the flow is
// observable without a running backend.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
vi.mock('./preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getPreference, setPreference } from './preferences';
import {
	AUTO_UPDATE_PREFERENCE_KEY,
	autoUpdateEnabled,
	availableUpdate,
	checkForUpdate,
	downloadUpdate,
	initUpdater,
	maybeCheckOnLaunch,
	setAutoUpdateEnabled,
	showUpdateToast,
	type UpdateInfo,
	updateAvailable,
	updatePhase,
	updateToastDismissed,
} from './updater';

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const getPreferenceMock = vi.mocked(getPreference);
const setPreferenceMock = vi.mocked(setPreference);

function withTauri(): void {
	(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}
function withoutTauri(): void {
	delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

const sampleUpdate: UpdateInfo = { version: '0.2.0', currentVersion: '0.1.0', notes: 'Fixes.' };

beforeEach(() => {
	autoUpdateEnabled.set(false);
	updatePhase.set('idle');
	availableUpdate.set(null);
	updateToastDismissed.set(false);
	invokeMock.mockReset();
	listenMock.mockReset();
	listenMock.mockResolvedValue(() => {});
	getPreferenceMock.mockReset();
	setPreferenceMock.mockReset();
	setPreferenceMock.mockResolvedValue(undefined);
	withTauri();
});

afterEach(() => {
	withoutTauri();
	vi.clearAllMocks();
});

describe('initUpdater', () => {
	it('loads auto-update OFF at runtime until chosen (the opt-out posture is panel-driven)', async () => {
		// The runtime store stays OFF until the user has made the choice, so the
		// launch check never fires before consent; the opt-out "on by default"
		// lives in the onboarding panel and the saved preference.
		getPreferenceMock.mockImplementation(
			async (_key: string, defaultValue: unknown) => defaultValue,
		);

		await initUpdater();

		expect(getPreferenceMock).toHaveBeenCalledWith(AUTO_UPDATE_PREFERENCE_KEY, false);
		expect(get(autoUpdateEnabled)).toBe(false);
	});

	it('honours a persisted opt-out', async () => {
		getPreferenceMock.mockResolvedValue(false);

		await initUpdater();

		expect(get(autoUpdateEnabled)).toBe(false);
	});
});

describe('setAutoUpdateEnabled', () => {
	it('sets the store and persists the choice', async () => {
		await setAutoUpdateEnabled(false);

		expect(get(autoUpdateEnabled)).toBe(false);
		expect(setPreferenceMock).toHaveBeenCalledWith(AUTO_UPDATE_PREFERENCE_KEY, false);
	});
});

describe('checkForUpdate', () => {
	it('marks an available update and clears any prior toast dismissal', async () => {
		updateToastDismissed.set(true);
		invokeMock.mockResolvedValue(sampleUpdate);

		const result = await checkForUpdate();

		expect(invokeMock).toHaveBeenCalledWith('check_for_update');
		expect(result).toEqual(sampleUpdate);
		expect(get(updatePhase)).toBe('available');
		expect(get(availableUpdate)).toEqual(sampleUpdate);
		expect(get(updateToastDismissed)).toBe(false);
	});

	it('marks up-to-date when the backend reports no update', async () => {
		invokeMock.mockResolvedValue(null);

		await checkForUpdate();

		expect(get(updatePhase)).toBe('up-to-date');
		expect(get(availableUpdate)).toBeNull();
	});

	it('surfaces an error phase when the check throws', async () => {
		invokeMock.mockRejectedValue('offline');

		await checkForUpdate();

		expect(get(updatePhase)).toBe('error');
	});

	it('is a no-op outside Tauri', async () => {
		withoutTauri();

		const result = await checkForUpdate();

		expect(result).toBeNull();
		expect(invokeMock).not.toHaveBeenCalled();
		expect(get(updatePhase)).toBe('idle');
	});
});

describe('downloadUpdate', () => {
	it('subscribes to progress, downloads, and reaches the ready phase', async () => {
		invokeMock.mockResolvedValue(sampleUpdate);

		await downloadUpdate();

		expect(listenMock).toHaveBeenCalledWith('updater:download-progress', expect.any(Function));
		expect(invokeMock).toHaveBeenCalledWith('download_update');
		expect(get(updatePhase)).toBe('ready');
	});

	it('surfaces an error phase when the download throws', async () => {
		invokeMock.mockRejectedValue('signature mismatch');

		await downloadUpdate();

		expect(get(updatePhase)).toBe('error');
	});
});

describe('maybeCheckOnLaunch', () => {
	it('checks when auto-update is enabled', async () => {
		autoUpdateEnabled.set(true);
		invokeMock.mockResolvedValue(null);

		await maybeCheckOnLaunch();

		expect(invokeMock).toHaveBeenCalledWith('check_for_update');
	});

	it('does nothing when the user has opted out', async () => {
		autoUpdateEnabled.set(false);

		await maybeCheckOnLaunch();

		expect(invokeMock).not.toHaveBeenCalled();
	});

	it('stays silent on failure (no error phase) for the launch check', async () => {
		autoUpdateEnabled.set(true);
		invokeMock.mockRejectedValue('offline');

		await maybeCheckOnLaunch();

		// A failed launch check must not leave /updates in an error state.
		expect(get(updatePhase)).toBe('idle');
	});
});

describe('derived stores', () => {
	it('updateAvailable tracks the pending phases', () => {
		updatePhase.set('idle');
		expect(get(updateAvailable)).toBe(false);
		updatePhase.set('available');
		expect(get(updateAvailable)).toBe(true);
		updatePhase.set('ready');
		expect(get(updateAvailable)).toBe(true);
		updatePhase.set('up-to-date');
		expect(get(updateAvailable)).toBe(false);
	});

	it('showUpdateToast is suppressed by dismissal', () => {
		updatePhase.set('available');
		updateToastDismissed.set(false);
		expect(get(showUpdateToast)).toBe(true);
		updateToastDismissed.set(true);
		expect(get(showUpdateToast)).toBe(false);
	});
});
