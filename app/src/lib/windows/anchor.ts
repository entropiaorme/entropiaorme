/**
 * DPI-aware anchor maths for satellite windows: convert a host-window DOM
 * element into screen coordinates a satellite can be positioned against.
 *
 * Two coordinate conventions, matching the two ways a satellite consumes an
 * anchor: physical pixels (host positions the satellite itself, via
 * `setPosition(PhysicalPosition)`) and logical pixels (the satellite
 * measures and positions itself from the anchor it is handed). The webview's
 * DOM rects are logical; the window's outer position is physical; the
 * window's scale factor bridges the two.
 */

import { getCurrentWindow } from '@tauri-apps/api/window';
import { OVERLAY_SURFACE_ATTRIBUTE } from './windowSize';

/**
 * The logical y a satellite dropped below `anchor` hangs from: the bottom
 * of the drawn surface that holds the anchor (a control inside the strip
 * drops its popup below the strip, never over the strip's own lower edge),
 * or the anchor's own bottom when it sits in no marked surface.
 */
function dropEdge(anchor: HTMLElement, rect: DOMRect): number {
	const surface = anchor.closest<HTMLElement>(`[${OVERLAY_SURFACE_ATTRIBUTE}]`);
	return surface ? Math.max(rect.bottom, surface.getBoundingClientRect().bottom) : rect.bottom;
}

/**
 * Physical screen position for a satellite dropped below `anchor` (below
 * its surface; see `dropEdge`), plus the anchor's logical width (for
 * sizing the satellite to its trigger). The
 * logical rect is scaled to physical BEFORE adding the (already physical)
 * window origin, and rounded only at the end, so fractional DPI scales
 * cannot accumulate off-by-one drift.
 */
export async function anchorBelow(
	anchor: HTMLElement,
	verticalGap: number,
): Promise<{ x: number; y: number; width: number }> {
	const currentWindow = getCurrentWindow();
	const [windowPosition, scaleFactor] = await Promise.all([
		currentWindow.outerPosition(),
		currentWindow.scaleFactor(),
	]);
	const rect = anchor.getBoundingClientRect();
	return {
		x: Math.round(windowPosition.x + rect.left * scaleFactor),
		y: Math.round(windowPosition.y + (dropEdge(anchor, rect) + verticalGap) * scaleFactor),
		width: rect.width,
	};
}

/**
 * Logical screen anchor for a self-positioning satellite: the horizontal
 * centre of `anchor` and the top edge for content dropped below it. Here the
 * physical window origin is descaled to logical instead, and nothing is
 * rounded: the satellite re-centres against this as it resizes, so it keeps
 * the fractional precision. Field names (`centerX`, `top`) match the
 * existing armour-cost popup contract (`lib/windows/overlayArmourCost.ts`).
 */
export async function anchorCentreBelow(
	anchor: HTMLElement,
	verticalGap: number,
): Promise<{ centerX: number; top: number }> {
	const currentWindow = getCurrentWindow();
	const [windowPosition, scaleFactor] = await Promise.all([
		currentWindow.outerPosition(),
		currentWindow.scaleFactor(),
	]);
	const rect = anchor.getBoundingClientRect();
	const windowLogicalX = windowPosition.x / scaleFactor;
	const windowLogicalY = windowPosition.y / scaleFactor;
	return {
		centerX: windowLogicalX + rect.left + rect.width / 2,
		top: windowLogicalY + dropEdge(anchor, rect) + verticalGap,
	};
}

export interface AnchorTracker {
	/** Request a re-sync on the next animation frame; repeat calls coalesce. */
	schedule(): void;
	/** Cancel a pending re-sync (teardown). */
	cancel(): void;
}

/**
 * Coalesce anchor re-sync requests onto animation frames. Layout-shifting
 * signals (resize observations, focus, window size sync) can fire in bursts;
 * `schedule()` ignores repeat calls while a frame is already pending, so a
 * burst costs one `sync()` per frame.
 */
export function createAnchorTracker(sync: () => void): AnchorTracker {
	let frame: number | null = null;

	return {
		schedule() {
			if (frame != null) return;
			frame = window.requestAnimationFrame(() => {
				frame = null;
				sync();
			});
		},
		cancel() {
			if (frame == null) return;
			window.cancelAnimationFrame(frame);
			frame = null;
		},
	};
}
