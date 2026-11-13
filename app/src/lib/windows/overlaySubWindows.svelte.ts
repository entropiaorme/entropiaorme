/**
 * The tracking overlay's sub-windows, assembled: the two windows they open
 * in and the one host that opens, places and closes them (the rules are in
 * `subWindow.ts`).
 *
 * Menus and panels share a window, showing one at a time. An armour
 * recording, which stays while the player goes to the game, has a window of
 * its own, so a menu opened meanwhile does not take its place.
 */

import { listen } from '@tauri-apps/api/event';
import { anchorForSubWindow, currentScreenBounds } from './anchor';
import {
	OVERLAY_ARMOUR_COST_CHANNEL,
	OVERLAY_MENU_CHANNEL,
	OVERLAY_MENU_MIN_WIDTH,
} from './overlayMenu';
import { createSatelliteWindow } from './satellite';
import type { SubWindowChannel, SubWindowReport } from './subWindow';
import { createSubWindowHost, type SubWindowRequest } from './subWindowHost.svelte';

/** The gap between the overlay and a sub-window hung from it. */
export const OVERLAY_SUB_WINDOW_GAP = 6;

/** The armour recording's sub-window. */
export const ARMOUR_COST_SUB_WINDOW = 'armourCost';

export interface OverlaySubWindowDeps {
	/** A sub-window could not be opened: say so. */
	onError: (id: string, error: unknown) => void;
	/** A sub-window closed, by whatever cause. */
	onClosed?: (id: string) => void;
	/** The player did something in a sub-window. */
	onInteract?: (id: string) => void;
}

/** The window a channel's sub-windows open in. The failure messages keep
 * the overlay's established wording (they show as notices). */
function windowFor(
	channel: SubWindowChannel,
	width: number,
	height: number,
	messages: { creationTimeout: string; readyTimeout: string },
) {
	return createSatelliteWindow({
		label: channel.label,
		url: channel.url,
		width,
		height,
		readyEvent: channel.ready,
		showEvent: channel.show,
		hideEvent: channel.hide,
		messages: { ...messages, creationFailed: 'Unknown Tauri popup creation error' },
	});
}

export function createOverlaySubWindows(deps: OverlaySubWindowDeps) {
	const menuWindow = windowFor(OVERLAY_MENU_CHANNEL, OVERLAY_MENU_MIN_WIDTH, 44, {
		creationTimeout: 'Popup window creation timed out',
		readyTimeout: 'Popup route did not become ready',
	});
	const armourCostWindow = windowFor(OVERLAY_ARMOUR_COST_CHANNEL, 320, 64, {
		creationTimeout: 'Armour cost popup creation timed out',
		readyTimeout: 'Armour cost popup did not become ready',
	});
	const host = createSubWindowHost({
		slots: {
			menu: { window: menuWindow, channel: OVERLAY_MENU_CHANNEL },
			armour: { window: armourCostWindow, channel: OVERLAY_ARMOUR_COST_CHANNEL },
		},
		gap: OVERLAY_SUB_WINDOW_GAP,
		anchorOf: anchorForSubWindow,
		bounds: currentScreenBounds,
		listen: (event, handler) =>
			listen<SubWindowReport>(event, (message) => handler(message.payload)),
		onError: deps.onError,
		onClosed: (id) => deps.onClosed?.(id),
		onInteract: (id) => deps.onInteract?.(id),
	});

	return {
		...host,
		/** Whether the menu window holds the keyboard (a pick in progress). */
		menuFocused: () => menuWindow.isFocused(),
		/**
		 * A control was clicked: close its sub-window when it is open, else
		 * build what it shows and open it. `build` answering nothing opens
		 * nothing (it has said why).
		 */
		async toggleWith(
			request: Omit<SubWindowRequest, 'state'>,
			build: () => unknown | Promise<unknown>,
		): Promise<void> {
			if (host.isOpen(request.id)) {
				await host.close(request.id);
				return;
			}
			if (host.reclicked(request.id)) return;
			const state = await build();
			if (state == null) return;
			await host.open({ ...request, state });
		},
	};
}

export type OverlaySubWindows = ReturnType<typeof createOverlaySubWindows>;
