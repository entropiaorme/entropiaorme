/**
 * Consolidated scan store: the single source of the skill-scan status for
 * each window that shows it (the scan overlay and the character view).
 *
 * A `createSnapshotStore` instance over the bridged scan topic: hydration-only
 * and event-driven, with the coalesced re-read, keep-last-good-on-failure, and
 * pure-trigger frame semantics implemented (and tested) by the factory; see
 * `lib/realtime/snapshotStore.svelte.ts` for the routing discipline. The
 * producer coalesces, so a re-read happens once per discrete status change (a
 * phase transition, a page collected, or a change in what stops the view in
 * the game being collected), never on a timer.
 */

import { getSkillScanStatus, type SkillScanStatus } from '$lib/api';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';

/**
 * The Tauri-bus topic the shell's event bridge emits each backend scan frame on:
 * the colon form of the `scan.status.changed` wire topic (Tauri event names
 * forbid dots). See `spawn_domain_event_bridge` in the shell.
 */
export const SCAN_TOPIC = 'scan:status:changed';

/** The skill-scan status store; `current` is `null` before the first hydration. */
export const scanStatus = createSnapshotStore<SkillScanStatus>(SCAN_TOPIC, getSkillScanStatus);

/** Re-read the scan status and publish it; overlapping calls coalesce. */
export const hydrate = scanStatus.hydrate;

/**
 * Subscribe to the bridged backend scan frames and keep the status current.
 * Returns a teardown that detaches the listener.
 */
export const subscribeScan = scanStatus.subscribe;
