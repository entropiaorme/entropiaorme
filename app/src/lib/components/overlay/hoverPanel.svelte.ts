/**
 * Runes-native state for an overlay control that expands into a panel: by
 * click or keyboard, where it stays until closed, or by the pointer resting
 * on the control or its panel, letting it close a moment after the pointer
 * leaves, so crossing from the control into the panel does not close it.
 *
 * Shared by every such control on the strip (the mob and the tool in
 * hand) so they open, hold and close alike. While the control is not
 * available (its session ended, its source moved) the panel is closed and
 * forgotten, never left to reopen by itself when it is available again: an
 * open panel is a surface that takes clicks meant for the game.
 */

import { untrack } from 'svelte';

/** How long the panel stays after the pointer leaves it, so the pointer can
 * cross from the control into the panel below it. */
export const HOVER_CLOSE_MS = 250;

export function createHoverPanel(
	options: {
		/** Whether the control can expand at all now. */
		available?: () => boolean;
		/** Told whenever the panel is closed by the player, so the owner can
		 * drop what was in progress in it. */
		onClose?: () => void;
	} = {},
) {
	const available = options.available ?? (() => true);
	let sticky = $state(false);
	let hovered = $state(false);
	let closeTimer: ReturnType<typeof setTimeout> | undefined;

	const expanded = $derived(available() && (sticky || hovered));

	function clearCloseTimer() {
		if (closeTimer === undefined) return;
		clearTimeout(closeTimer);
		closeTimer = undefined;
	}

	function close() {
		clearCloseTimer();
		sticky = false;
		hovered = false;
		options.onClose?.();
	}

	// Unavailable, it is closed for good: what held it open is dropped.
	const stopWatching = $effect.root(() => {
		$effect(() => {
			if (available()) return;
			untrack(() => {
				if (sticky || hovered || closeTimer !== undefined) close();
			});
		});
	});

	return {
		get expanded() {
			return expanded;
		},
		/** The toggle (a click or a key): open it for good, or close it once
		 * it is open for good. Open only under the pointer, a click keeps it
		 * open rather than closing what the player just reached for. */
		toggle() {
			clearCloseTimer();
			if (expanded && sticky) close();
			else sticky = true;
		},
		/** Close it (Escape, or an action that is done with it). */
		close,
		pointerEnter() {
			clearCloseTimer();
			hovered = true;
		},
		pointerLeave() {
			clearCloseTimer();
			closeTimer = setTimeout(() => {
				closeTimer = undefined;
				hovered = false;
			}, HOVER_CLOSE_MS);
		},
		/** The player is working in the panel (typing, picking): keep it open
		 * whatever the pointer does. */
		hold() {
			clearCloseTimer();
			if (available()) sticky = true;
		},
		destroy() {
			clearCloseTimer();
			stopWatching();
		},
	};
}

export type HoverPanel = ReturnType<typeof createHoverPanel>;
