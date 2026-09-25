/**
 * The shell-surface commands: the handful of bespoke window and byte
 * commands the Tauri shell registers outside the typed eo-api manifest
 * (window lifecycle is the shell's, not the facade's, and the capture
 * preview answers raw bytes rather than a JSON DTO). This module is the
 * single home for their invoke strings; nothing else in the frontend
 * calls `invoke` with a bare command name.
 *
 * Their error contract is the shell's (a plain string or a plugin
 * error), not `ApiErrorPayload`, so they ride `invoke` directly rather
 * than the typed transport in `./invoke`.
 */

import { invoke } from '@tauri-apps/api/core';

/** Toggle the pre-spawned tracking overlay window's visibility. */
export async function toggleOverlay(): Promise<void> {
	await invoke('toggle_overlay');
}

/** Toggle the pre-spawned cartography pin overlay window. */
export async function toggleCartographyOverlay(): Promise<void> {
	await invoke('toggle_cartography_overlay');
}

export async function showNavigationOverlays(): Promise<void> {
	await invoke('show_navigation_overlays');
}

export async function hideNavigationOverlays(): Promise<void> {
	await invoke('hide_navigation_overlays');
}

/** Transfer route-area selection from the floating HUD to the main Maps window. */
export async function beginNavigationAreaSelection(
	requestId: number,
	planet: string,
	mapViewId: number | null,
): Promise<void> {
	await invoke('begin_navigation_area_selection', {
		request_id: requestId,
		planet,
		map_view_id: mapViewId,
	});
}

/** Show and focus the pre-spawned scan overlay window. */
export async function showScanOverlay(): Promise<void> {
	await invoke('show_scan_overlay');
}

/** Hide the scan overlay window. */
export async function hideScanOverlay(): Promise<void> {
	await invoke('hide_scan_overlay');
}

/** Show the sale-window capture button where a fullscreen game leaves it
 * reachable. It is the main window's button in another place. */
export async function showSaleCaptureOverlay(): Promise<void> {
	await invoke('show_sale_capture_overlay');
}

/** Hide the sale-window capture overlay. */
export async function hideSaleCaptureOverlay(): Promise<void> {
	await invoke('hide_sale_capture_overlay');
}

export interface OverlayCaptureReply {
	message: string;
	failed: boolean;
}

/** Consume the one-shot capture authority granted when Inventory opened the overlay. */
export async function captureSaleFromOverlay(): Promise<OverlayCaptureReply> {
	return invoke('capture_sale_from_overlay');
}

/** Metadata about an available update (mirrors the Rust `UpdateInfo`). */
export type UpdateInfo = {
	version: string;
	currentVersion: string;
	notes: string | null;
};

/** Ask the updater to check the release manifest; null when current. */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
	return invoke('check_for_update');
}

/** Download the available update; resolves once staged. */
export async function downloadUpdate(): Promise<UpdateInfo> {
	return invoke('download_update');
}

/** The configured update channel. */
export async function getUpdateChannel(): Promise<string> {
	return invoke('get_update_channel');
}

/** Hand off to the updater: install the staged update and restart. */
export async function installUpdate(): Promise<void> {
	await invoke('install_update');
}

/** The manual-scan capture preview PNG for a page, as a base64 `data:`
 * URL for an `<img>` `src`. */
export async function manualSkillScanCapturePng(page: number): Promise<string> {
	const encoded = await invoke<string>('capture_png', { page });
	return `data:image/png;base64,${encoded}`;
}

/** A bundled planet map's raster as a base64 `data:` URL for an `<img>`
 * `src`. `mime` comes from the planet's `planet_maps_list` record. */
export async function planetMapImage(planet: string, mime: string): Promise<string> {
	const encoded = await invoke<string>('planet_map_image', { planet });
	return `data:${mime};base64,${encoded}`;
}

/** Why the backend declined to start (mirrors the Rust `DeclineReason`). */
export type SubstrateDeclineReason =
	| 'data_dir_unavailable'
	| 'database_below_baseline'
	| 'database_unreadable'
	| 'game_data_unavailable'
	| 'tracking_unavailable'
	| 'unexpected';

/** The settled startup outcome (mirrors the Rust `SubstrateOutcome`). */
export type SubstrateOutcome =
	| { state: 'ready' }
	| { state: 'failed'; reason: SubstrateDeclineReason; detail: string };

/** Wait for the backend to finish starting and answer how it went. Resolves
 * at once when startup has already settled, so a late caller cannot miss it. */
export async function awaitSubstrate(): Promise<SubstrateOutcome> {
	return invoke('substrate_ready');
}

/** Relaunch the app (the startup failure surface's recovery action). */
export async function restartApp(): Promise<void> {
	await invoke('restart_app');
}
