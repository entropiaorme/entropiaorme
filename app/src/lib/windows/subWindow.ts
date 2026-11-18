/**
 * The overlay's sub-windows: what the host window and a sub-window say to
 * each other, and where a sub-window goes.
 *
 * A sub-window is anything the overlay opens beside itself: a menu, a
 * panel that says more about a line, a recording the player works through.
 * Each is a window of its own (see `satellite.ts`), so it is never cut to
 * the overlay's box, and all of them behave alike:
 *
 * - a click on its control opens it and a second click closes it;
 * - one is open at a time (one that stays, below, apart);
 * - Escape closes it, and so does a click anywhere else: on the overlay, or
 *   back in the game, which takes the keyboard from it;
 * - left alone, the pointer away from both its control and itself, it
 *   closes after a moment, unless the player is typing in it;
 * - one that **stays** is a task the player leaves for the game and comes
 *   back to (a quest hand-in, an armour recording): only its control, its
 *   own buttons, or Escape close it;
 * - one **attached** to a text field in the overlay (a typeahead's
 *   suggestions) takes no keyboard and follows that field instead.
 *
 * The host (`subWindowHost.svelte.ts`) decides when one opens and closes.
 * The sub-window (`SubWindowShell.svelte`) sizes itself to what it holds,
 * places itself against its control, and reports what happens in it.
 */

/** The events one sub-window OS window is driven and heard over. */
export interface SubWindowChannel {
	/** The window's label. */
	label: string;
	/** The route it loads. */
	url: string;
	/** Emitted by the sub-window once it listens (with `{ label }`). */
	ready: string;
	/** Host to sub-window: show this (a `SubWindowEnvelope`). */
	show: string;
	/** Host to sub-window: what is shown changed, or its control moved. */
	update: string;
	/** Host to sub-window: hide. */
	hide: string;
	/** Sub-window to host: a `SubWindowReport`. */
	report: string;
}

export function subWindowChannel(label: string, url: string): SubWindowChannel {
	return {
		label,
		url,
		ready: `${label}:ready`,
		show: `${label}:show`,
		update: `${label}:update`,
		hide: `${label}:hide`,
		report: `${label}:report`,
	};
}

/** A rectangle in logical screen pixels. */
export interface ScreenRect {
	x: number;
	y: number;
	width: number;
	height: number;
}

/**
 * Where a sub-window hangs from, in logical screen pixels: its control's
 * horizontal extent, and the edges of the drawn surface that holds the
 * control (a sub-window opens clear of the overlay, never over it).
 */
export interface SubWindowAnchor {
	left: number;
	right: number;
	/** The edge to hang below. */
	below: number;
	/** The edge to sit above, when there is no room below. */
	above: number;
}

export type SubWindowAlign = 'start' | 'centre';

/** What the host hands a sub-window to show. */
export interface SubWindowEnvelope<S = unknown> {
	/** Which sub-window this is; a report names it back. */
	id: string;
	/** What it shows. */
	state: S;
	anchor: SubWindowAnchor;
	/** The screen it must stay on, when the host could tell. */
	bounds: ScreenRect | null;
	gap: number;
	align: SubWindowAlign;
	/** Take the keyboard when shown. */
	focus: boolean;
	/** A task the player leaves and returns to: losing the keyboard does
	 * not close it. */
	stays: boolean;
}

/** Why a sub-window closed itself. */
export type SubWindowCloseReason = 'escape' | 'blur' | 'done';

/** What a sub-window tells its host. */
export type SubWindowReport =
	| { type: 'closed'; id: string; reason: SubWindowCloseReason }
	| { type: 'pointer'; id: string; inside: boolean }
	| { type: 'held'; id: string; held: boolean }
	| { type: 'interact'; id: string };

/** The least a sub-window keeps between itself and the screen's edge. */
export const SUB_WINDOW_MARGIN = 4;

export interface SubWindowPlacement {
	x: number;
	y: number;
	side: 'below' | 'above';
	/** The most height there is for it on that side. */
	maxHeight: number | null;
}

/**
 * Where a sub-window of `size` goes: below its anchor, its start edge on
 * the control's (or centred on it); above when it does not fit below and
 * fits above, or there is more room above; and always inside `bounds`.
 * Without bounds it hangs below, unclamped.
 */
export function placeSubWindow(
	anchor: SubWindowAnchor,
	size: { width: number; height: number },
	bounds: ScreenRect | null,
	gap: number,
	align: SubWindowAlign,
): SubWindowPlacement {
	const wantedX =
		align === 'centre' ? (anchor.left + anchor.right) / 2 - size.width / 2 : anchor.left;
	const belowY = anchor.below + gap;
	if (!bounds) return { x: wantedX, y: belowY, side: 'below', maxHeight: null };

	const left = bounds.x + SUB_WINDOW_MARGIN;
	const right = bounds.x + bounds.width - SUB_WINDOW_MARGIN;
	const top = bounds.y + SUB_WINDOW_MARGIN;
	const bottom = bounds.y + bounds.height - SUB_WINDOW_MARGIN;
	const x = Math.max(left, Math.min(wantedX, right - size.width));

	const roomBelow = bottom - belowY;
	const roomAbove = anchor.above - gap - top;
	const fitsBelow = size.height <= roomBelow;
	const fitsAbove = size.height <= roomAbove;
	const above = !fitsBelow && (fitsAbove || roomAbove > roomBelow);
	if (!above) {
		return { x, y: belowY, side: 'below', maxHeight: Math.max(0, roomBelow) };
	}
	const height = Math.min(size.height, Math.max(0, roomAbove));
	return {
		x,
		y: anchor.above - gap - height,
		side: 'above',
		maxHeight: Math.max(0, roomAbove),
	};
}
