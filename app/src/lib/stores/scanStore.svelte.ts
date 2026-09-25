/**
 * Consolidated scan store: the single source of the manual skill-scan status
 * for any window that passively observes it (the character view today).
 *
 * A `createSnapshotStore` instance over the bridged scan topic: hydration-only
 * and event-driven, with the coalesced re-read, keep-last-good-on-failure, and
 * pure-trigger frame semantics implemented (and tested) by the factory; see
 * `lib/realtime/snapshotStore.svelte.ts` for the routing discipline. The
 * producer coalesces, so a re-read happens once per discrete status change (a
 * phase transition or a per-page capture / OCR progress step), never on a
 * timer.
 */

import { getManualSkillScanStatus, type ScanManualStatus } from '$lib/api';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';

/**
 * The Tauri-bus topic the shell's event bridge emits each backend scan frame on:
 * the colon form of the `scan.status.changed` wire topic (Tauri event names
 * forbid dots). See `spawn_domain_event_bridge` in the shell.
 */
export const SCAN_TOPIC = 'scan:status:changed';

/** The manual skill-scan status store; `current` is `null` before the first hydration. */
export const scanStatus = createSnapshotStore<ScanManualStatus>(
	SCAN_TOPIC,
	getManualSkillScanStatus,
);

/** Re-read the scan status and publish it; overlapping calls coalesce. */
export const hydrate = scanStatus.hydrate;

/**
 * Subscribe to the bridged backend scan frames and keep the status current.
 * Returns a teardown that detaches the listener.
 */
export const subscribeScan = scanStatus.subscribe;
