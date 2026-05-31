/**
 * Consolidated scan store: the single source of the manual skill-scan status for
 * any window that passively observes it (the character view today).
 *
 * Hydration-only and event-driven, mirroring `trackingStore`. `hydrate()` reads
 * `/scan/skills/status` once; `subscribeScan()` listens the backend scan frames
 * the event relay re-emits onto the Tauri bus and re-reads the status on each, so
 * an observer updates by subscription rather than by polling the status endpoint
 * every 500ms.
 *
 * Routing discipline (the load-bearing constraint, as in `trackingStore`): a
 * relayed frame is a pure trigger. We never fold a frame field into rendered
 * state; every value comes from the status read. The producer coalesces, so a
 * re-read happens once per discrete status change (a phase transition or a
 * per-page capture / OCR progress step), never on a timer. The relay's reconnect
 * nudge carries no payload, and because we re-read rather than reduce, an absent
 * payload can never be mistaken for an idle scan.
 */
import { writable, type Writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import { getManualSkillScanStatus, type ScanManualStatus } from '$lib/api';

/**
 * The Tauri-bus topic the event relay re-emits each backend scan frame on: the
 * colon form of the `scan.status.changed` wire topic (Tauri event names forbid
 * dots). See `lib/realtime/eventRelay.ts`.
 */
export const SCAN_TOPIC = 'scan:status:changed';

/** The manual skill-scan status, or `null` before the first hydration. */
export const scanStatus: Writable<ScanManualStatus | null> = writable(null);

let inFlight = false;
let refetchQueued = false;

/**
 * Re-read the scan status and publish it. Overlapping calls coalesce: a frame
 * arriving mid-read queues exactly one follow-up read, so the store always
 * settles on the latest state and two reads can never race to write out of
 * order. A failed read leaves the last good status in place; the next frame (or
 * the relay's reconnect nudge) re-reads.
 */
export async function hydrate(): Promise<void> {
	if (inFlight) {
		refetchQueued = true;
		return;
	}
	inFlight = true;
	try {
		do {
			refetchQueued = false;
			scanStatus.set(await getManualSkillScanStatus());
		} while (refetchQueued);
	} catch {
		// Transient read failure: keep the last good status rather than blanking.
	} finally {
		inFlight = false;
	}
}

/**
 * Subscribe to the relayed backend scan frames and keep the status current.
 * Returns a teardown that detaches the listener. Each frame (a status change or
 * the relay's payload-less reconnect nudge) triggers one status read; see the
 * routing discipline in the module header.
 */
export function subscribeScan(): Promise<UnlistenFn> {
	return listen(SCAN_TOPIC, () => {
		void hydrate();
	});
}
