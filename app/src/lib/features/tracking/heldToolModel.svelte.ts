/**
 * Runes-native state for the strip's tool in hand while reading it off the
 * screen is turned on: the expanding panel of the screen's control, and the
 * one of the marker shown while the screen is not being read. Each is
 * available only in its own state, so a panel left open closes when its
 * state ends, and the overlay closes both when it hides.
 */

import type { ToolSourceView } from '$lib/api';
import { createHoverPanel } from '$lib/components/overlay/hoverPanel.svelte';

export function createHeldToolModel(deps: {
	/** The session's tool source as the snapshot last read it. */
	source: () => ToolSourceView | null | undefined;
}) {
	const screen = createHoverPanel({ available: () => deps.source()?.readingScreen === true });
	const unread = createHoverPanel({
		available: () => (deps.source()?.notReading ?? null) !== null,
	});
	return {
		screen,
		unread,
		/** Close both (the overlay hid). */
		close() {
			screen.close();
			unread.close();
		},
		destroy() {
			screen.destroy();
			unread.destroy();
		},
	};
}

export type HeldToolModel = ReturnType<typeof createHeldToolModel>;
