import { tick } from 'svelte';
import { ApiError, type ProtectionOverview } from '$lib/api';
import { buildProtectionCostSteps } from '$lib/features/protection/protectionCostFlow';
import { anchorCentreBelow, createAnchorTracker } from '$lib/windows/anchor';
import {
	OVERLAY_ARMOUR_COST_UPDATE_EVENT,
	type OverlayArmourCostState,
} from '$lib/windows/overlayArmourCost';
import type { SatelliteWindow } from '$lib/windows/satellite';

/**
 * What the overlay route lends the popup controller: its satellite window,
 * the anchor gap it lays popups out with, and readers for the session and
 * armour state the popup renders from. Everything else the controller owns.
 */
/**
 * How many render flushes an anchor is given to appear before the open is
 * reported as failed. Generous enough for a readout that renders in a later
 * flush, short enough that a genuinely absent anchor fails visibly.
 */
const ANCHOR_FLUSHES = 8;

export interface OverlayArmourCostPorts {
	window: SatelliteWindow;
	anchorGap: number;
	sessionId: () => string | null;
	repairOcrEnabled: () => boolean;
	/** The session's stamped attribution: false means whole-session. */
	bySegment: () => boolean;
	protection: () => ProtectionOverview | null;
	/** The running session's own Cost control on the strip. */
	inSessionAnchor: () => HTMLElement | null;
}

/**
 * The overlay's armour-cost popup: opening it against an anchor, keeping its
 * placement in step as the strip resizes, and closing it. The popup is a
 * separate webview, so the controller holds the open/anchor state the route
 * used to carry inline.
 */
/**
 * Wait for an anchor the host is about to render. The control appears only
 * once the surface carrying it has, which is not guaranteed to be the flush
 * this is asked in, so a bounded few are given rather than assuming the
 * first. Each attempt yields the task as well as the flush: a host that
 * renders off a promise or a timer would otherwise never be observed, since
 * those settle after every flush this loop could drain.
 */
async function waitForAnchor(read: () => HTMLElement | null): Promise<HTMLElement | null> {
	for (let attempt = 0; attempt < ANCHOR_FLUSHES; attempt += 1) {
		await tick();
		const target = read();
		if (target?.isConnected) return target;
		await new Promise((resolve) => setTimeout(resolve, 0));
	}
	return null;
}

export function createOverlayArmourCostModel(ports: OverlayArmourCostPorts) {
	let open = $state(false);
	let error = $state<string | null>(null);
	let anchor: HTMLElement | null = $state(null);
	// Stamped when the popup self-closes (blur, ESC, post-save). The Cost-button
	// click handler races against the CLOSED event: if blur arrives first, `open`
	// flips to false before `toggle` reads it, and the click would reopen the
	// popup that the same gesture just dismissed. Gating the open branch on this
	// timestamp suppresses that reopen.
	let closedAt = 0;

	async function buildState(target: HTMLElement): Promise<OverlayArmourCostState | null> {
		const sessionId = ports.sessionId();
		if (!sessionId || !target.isConnected) return null;
		const overview = ports.protection();
		// Whole-session attribution asks which composed setup was worn, but only
		// when there is one to choose; an empty catalogue keeps the generic
		// combined reading, which needs no composition.
		const requiresLoadoutSelection = !ports.bySegment() && (overview?.loadouts.length ?? 0) > 0;
		const steps = requiresLoadoutSelection ? [] : buildProtectionCostSteps(overview);
		if (!requiresLoadoutSelection && steps.length === 0) return null;

		return {
			sessionId,
			repairOcrEnabled: ports.repairOcrEnabled(),
			steps,
			protection: overview,
			requiresLoadoutSelection,
			anchor: await anchorCentreBelow(target, ports.anchorGap),
		};
	}

	async function syncAnchor(): Promise<void> {
		if (!open || !anchor) return;
		const state = await buildState(anchor);
		if (!state) return;

		await ports.window.emitTo(OVERLAY_ARMOUR_COST_UPDATE_EVENT, state);
	}

	const tracker = createAnchorTracker(() => void syncAnchor());

	function scheduleAnchorSync(): void {
		if (!open || !anchor) return;
		tracker.schedule();
	}

	async function show(target: HTMLElement): Promise<boolean> {
		try {
			await ports.window.ensure();
			const state = await buildState(target);
			if (!state) return false;

			anchor = target;
			// The popup measures its panel, sizes+positions itself accurately, then
			// reveals + focuses on its own (never revealed from here) so it cannot
			// flash for one frame at the wrong (initial-guess) location.
			await ports.window.show(state, undefined, { reveal: false });
			error = null;
			open = true;
			scheduleAnchorSync();
			return true;
		} catch (cause) {
			open = false;
			anchor = null;
			error =
				cause instanceof ApiError || cause instanceof Error
					? cause.message
					: 'Popup window failed to open';
			console.error('Armour cost popup failed', cause);
			return false;
		}
	}

	function clearOpenState(): void {
		open = false;
		anchor = null;
		tracker.cancel();
	}

	async function hide(): Promise<void> {
		clearOpenState();
		await ports.window.hide();
	}

	async function toggle(event: MouseEvent): Promise<void> {
		if (open) {
			await hide();
			return;
		}
		if (Date.now() - closedAt < 250) return;
		const target = event.currentTarget as HTMLElement | null;
		if (!target) return;
		await show(target);
	}

	/** The armour workflow over the running session's own Cost control. */
	async function showInSession(): Promise<boolean> {
		const target = await waitForAnchor(ports.inSessionAnchor);
		if (!ports.sessionId()) {
			error = 'There is no session left to record an armour cost against';
			return false;
		}
		if (!target) {
			error = 'The armour cost window could not be opened';
			return false;
		}
		if (open) return true;
		return show(target);
	}

	/** The popup reported that it closed itself. */
	function noteClosed(): void {
		closedAt = Date.now();
		clearOpenState();
	}

	return {
		get open() {
			return open;
		},
		get error() {
			return error;
		},
		show,
		hide,
		toggle,
		showInSession,
		scheduleAnchorSync,
		noteClosed,
	};
}
