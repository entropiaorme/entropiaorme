// Auto-update client state and command wrappers.
//
// The app reaches the updater through Rust commands (not the JS updater plugin
// directly), so channel resolution and the forced-exit teardown live on the
// Rust side; this module is the thin frontend seam: the opt-out preference, the
// check / download / install flow, the download-progress subscription, and the
// derived stores the toast and the Updates page read.
//
// Networking posture: the launch-time check is an outbound call, so it is gated
// on the auto-update preference (default ON; the user opts out). It transmits no
// user data: a plain GET of a static per-channel manifest, the running version
// is compared locally, nothing about the user is sent.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { derived, get, type Readable, type Writable, writable } from 'svelte/store';

import { getPreference, setPreference } from './preferences';

/// The opt-out preference key. Default ON: a fresh profile auto-checks.
const KEY_AUTO_UPDATE_ENABLED = 'auto_update_enabled';
export const AUTO_UPDATE_PREFERENCE_KEY = KEY_AUTO_UPDATE_ENABLED;
export const DEFAULT_AUTO_UPDATE_ENABLED = true;

/// The download-progress event the Rust side emits (colon-form, matching the bus).
const DOWNLOAD_PROGRESS_EVENT = 'updater:download-progress';

/// Metadata about an available update (mirrors the Rust `UpdateInfo`).
export type UpdateInfo = {
	version: string;
	currentVersion: string;
	notes: string | null;
};

/// Bytes-arrived progress (mirrors the Rust `DownloadProgress`). `contentLength`
/// is null when the server uses chunked transfer; the UI shows indeterminate.
export type DownloadProgress = {
	downloaded: number;
	contentLength: number | null;
};

/// The update flow's phase, driving every update surface.
export type UpdatePhase =
	| 'idle' // no check run, or the result was dismissed
	| 'checking'
	| 'up-to-date'
	| 'available' // a newer release exists, not yet downloaded
	| 'downloading'
	| 'ready' // downloaded, awaiting the user's install-and-restart
	| 'installing'
	| 'error';

export const autoUpdateEnabled: Writable<boolean> = writable(DEFAULT_AUTO_UPDATE_ENABLED);
export const updatePhase: Writable<UpdatePhase> = writable('idle');
export const availableUpdate: Writable<UpdateInfo | null> = writable(null);
export const downloadProgress: Writable<DownloadProgress | null> = writable(null);
export const updateError: Writable<string | null> = writable(null);

/// Session-scoped toast dismissal. Not persisted: per the re-nudge decision, a
/// dismissed update stays silent only until the next launch check.
export const updateToastDismissed: Writable<boolean> = writable(false);

/// Whether an update is pending the user's attention (drives the sidebar dot).
export const updateAvailable: Readable<boolean> = derived(
	updatePhase,
	($phase) => $phase === 'available' || $phase === 'downloading' || $phase === 'ready',
);

/// Whether to show the toast: an update is pending and not dismissed this session.
export const showUpdateToast: Readable<boolean> = derived(
	[updateAvailable, updateToastDismissed],
	([$available, $dismissed]) => $available && !$dismissed,
);

/// Whether the Tauri IPC bridge is present. Checked at call time (not module
/// load) so the bridge can appear after import and so tests can toggle it.
function isTauri(): boolean {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

let unlistenProgress: UnlistenFn | null = null;

/// Hydrate the opt-out preference from the store. Call once on app start.
export async function initUpdater(): Promise<void> {
	const enabled = await getPreference<boolean>(
		KEY_AUTO_UPDATE_ENABLED,
		DEFAULT_AUTO_UPDATE_ENABLED,
	);
	autoUpdateEnabled.set(enabled);
}

/// Persist the opt-out preference.
export async function setAutoUpdateEnabled(value: boolean): Promise<void> {
	autoUpdateEnabled.set(value);
	await setPreference(KEY_AUTO_UPDATE_ENABLED, value);
}

/// Check the active channel's manifest for a newer release.
export async function checkForUpdate(): Promise<UpdateInfo | null> {
	if (!isTauri()) return null;
	updateError.set(null);
	updatePhase.set('checking');
	try {
		const info = await invoke<UpdateInfo | null>('check_for_update');
		if (info) {
			availableUpdate.set(info);
			updatePhase.set('available');
			updateToastDismissed.set(false);
		} else {
			availableUpdate.set(null);
			updatePhase.set('up-to-date');
		}
		return info;
	} catch (err) {
		updateError.set(String(err));
		updatePhase.set('error');
		return null;
	}
}

/// Download the available update (verifying its signature) and hold it for
/// install, surfacing progress. Idempotent on the progress listener.
export async function downloadUpdate(): Promise<void> {
	if (!isTauri()) return;
	updateError.set(null);
	downloadProgress.set(null);
	updatePhase.set('downloading');
	if (!unlistenProgress) {
		unlistenProgress = await listen<DownloadProgress>(DOWNLOAD_PROGRESS_EVENT, (event) => {
			downloadProgress.set(event.payload);
		});
	}
	try {
		const info = await invoke<UpdateInfo>('download_update');
		availableUpdate.set(info);
		updatePhase.set('ready');
	} catch (err) {
		updateError.set(String(err));
		updatePhase.set('error');
	}
}

/// Install the downloaded update and relaunch. On success the process exits
/// before this resolves (Windows), so only the failure path returns control.
export async function installUpdate(): Promise<void> {
	if (!isTauri()) return;
	updateError.set(null);
	updatePhase.set('installing');
	try {
		await invoke('install_update');
	} catch (err) {
		updateError.set(String(err));
		updatePhase.set('error');
	}
}

/// The current update channel (read-only in the UI for the 0.x window: only
/// stable is surfaced, though the channel plumbing supports beta).
export async function getUpdateChannel(): Promise<string> {
	if (!isTauri()) return 'stable';
	try {
		return await invoke<string>('get_update_channel');
	} catch {
		return 'stable';
	}
}

/// Dismiss the toast for this session.
export function dismissUpdateToast(): void {
	updateToastDismissed.set(true);
}

/// The launch-time check, gated on the opt-out preference. Silent on failure:
/// a failed update check must never disrupt startup.
export async function maybeCheckOnLaunch(): Promise<void> {
	if (!isTauri()) return;
	if (!get(autoUpdateEnabled)) return;
	await checkForUpdate();
}
