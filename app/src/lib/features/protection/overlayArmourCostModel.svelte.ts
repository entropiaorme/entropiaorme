import { ApiError } from '$lib/api';
import { anchorCentreBelow, createAnchorTracker } from '$lib/windows/anchor';
import {
	OVERLAY_ARMOUR_COST_UPDATE_EVENT,
	type OverlayArmourCostState,
} from '$lib/windows/overlayArmourCost';
import type { SatelliteWindow } from '$lib/windows/satellite';

/**
 * What the overlay route lends the popup controller: its satellite window,
 * the anchor gap it lays popups out with, and whether the Repair Terminal
 * reader is on. Everything else the controller owns; the popup reads its
 * own armour state, since recording needs no running session.
 */
export interface OverlayArmourCostPorts {
	window: SatelliteWindow;
	anchorGap: number;
	repairOcrEnabled: () => boolean;
}

/**
 * The overlay's armour-cost popup: opening it against an anchor, keeping its
 * placement in step as the strip resizes, and closing it. The popup is a
 * separate webview, so the controller holds the open/anchor state the route
 * used to carry inline.
 */
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
		if (!target.isConnected) return null;
		return {
			repairOcrEnabled: ports.repairOcrEnabled(),
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
		scheduleAnchorSync,
		noteClosed,
	};
}
