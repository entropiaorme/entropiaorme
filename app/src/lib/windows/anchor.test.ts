// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the one Tauri seam the maths depends on: the host window's physical
// origin and scale factor. The DOM rect comes from a stubbed element.
const seams = vi.hoisted(() => ({
	outerPosition: vi.fn(async () => ({ x: 0, y: 0 })),
	scaleFactor: vi.fn(async () => 1),
}));

vi.mock('@tauri-apps/api/window', () => ({
	getCurrentWindow: () => ({
		outerPosition: seams.outerPosition,
		scaleFactor: seams.scaleFactor,
	}),
}));

import { anchorBelow, anchorCentreBelow, createAnchorTracker } from './anchor';
import { OVERLAY_SURFACE_ATTRIBUTE } from './windowSize';

function fakeAnchor(rect: { left: number; bottom: number; width: number }): HTMLElement {
	const el = document.createElement('div');
	el.getBoundingClientRect = () =>
		({
			left: rect.left,
			bottom: rect.bottom,
			width: rect.width,
			top: 0,
			right: rect.left + rect.width,
			height: 0,
			x: rect.left,
			y: 0,
			toJSON: () => ({}),
		}) as DOMRect;
	return el;
}

/** `anchor` placed inside a marked surface whose bottom edge is `bottom`. */
function insideSurface(anchor: HTMLElement, bottom: number): HTMLElement {
	const surface = document.createElement('div');
	surface.setAttribute(OVERLAY_SURFACE_ATTRIBUTE, '');
	surface.getBoundingClientRect = () => ({ bottom }) as DOMRect;
	surface.appendChild(anchor);
	return surface;
}

beforeEach(() => {
	seams.outerPosition.mockResolvedValue({ x: 100, y: 200 });
	seams.scaleFactor.mockResolvedValue(1);
});

afterEach(() => {
	vi.restoreAllMocks();
});

describe('anchorBelow', () => {
	it('offsets the logical rect by the physical window origin at scale 1', async () => {
		const anchor = fakeAnchor({ left: 10.4, bottom: 20.2, width: 55 });

		await expect(anchorBelow(anchor, 6)).resolves.toEqual({
			x: 110, // round(100 + 10.4)
			y: 226, // round(200 + (20.2 + 6))
			width: 55,
		});
	});

	it('scales the rect (not the window origin) to physical at scale 2, rounding last', async () => {
		seams.scaleFactor.mockResolvedValue(2);
		const anchor = fakeAnchor({ left: 10.4, bottom: 20.2, width: 55 });

		await expect(anchorBelow(anchor, 6)).resolves.toEqual({
			x: 121, // round(100 + 10.4 * 2): the origin is already physical
			y: 252, // round(200 + (20.2 + 6) * 2): the gap is logical, so it scales too
			width: 55, // the anchor's width stays logical (it sizes the satellite)
		});
	});
});

describe('dropping below the anchor surface', () => {
	it('hangs a popup below the surface holding its trigger, not over its lower edge', async () => {
		// A field 8px above the strip's bottom: the menu must clear the strip.
		const anchor = fakeAnchor({ left: 10, bottom: 40, width: 120 });
		insideSurface(anchor, 48);

		await expect(anchorBelow(anchor, 6)).resolves.toMatchObject({ y: 254 }); // 200 + 48 + 6
		const centred = await anchorCentreBelow(anchor, 6);
		expect(centred.top).toBeCloseTo(254, 10);
	});

	it('keeps the anchor edge when it already sits below its surface', async () => {
		const anchor = fakeAnchor({ left: 10, bottom: 60, width: 120 });
		insideSurface(anchor, 48);

		await expect(anchorBelow(anchor, 6)).resolves.toMatchObject({ y: 266 }); // 200 + 60 + 6
	});
});

describe('anchorCentreBelow', () => {
	it('returns the logical centre and top edge at scale 1, unrounded', async () => {
		const anchor = fakeAnchor({ left: 10.4, bottom: 20.2, width: 55 });

		const result = await anchorCentreBelow(anchor, 6);
		expect(result.centerX).toBeCloseTo(137.9, 10); // 100 + 10.4 + 55 / 2
		expect(result.top).toBeCloseTo(226.2, 10); // 200 + 20.2 + 6
	});

	it('descales the physical window origin to logical at scale 2', async () => {
		seams.scaleFactor.mockResolvedValue(2);
		const anchor = fakeAnchor({ left: 10.4, bottom: 20.2, width: 55 });

		const result = await anchorCentreBelow(anchor, 6);
		expect(result.centerX).toBeCloseTo(87.9, 10); // 100 / 2 + 10.4 + 55 / 2
		expect(result.top).toBeCloseTo(126.2, 10); // 200 / 2 + 20.2 + 6
	});
});

describe('createAnchorTracker', () => {
	let pendingFrames: Map<number, FrameRequestCallback>;

	beforeEach(() => {
		pendingFrames = new Map();
		let nextFrameId = 1;
		vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
			const id = nextFrameId++;
			pendingFrames.set(id, cb);
			return id;
		});
		vi.spyOn(window, 'cancelAnimationFrame').mockImplementation((id) => {
			pendingFrames.delete(id);
		});
	});

	function flushFrames(): void {
		const callbacks = [...pendingFrames.values()];
		pendingFrames.clear();
		for (const cb of callbacks) {
			cb(0);
		}
	}

	it('coalesces a burst of schedule calls into one sync per frame', () => {
		const sync = vi.fn();
		const tracker = createAnchorTracker(sync);

		tracker.schedule();
		tracker.schedule();
		tracker.schedule();
		expect(pendingFrames.size).toBe(1);

		flushFrames();
		expect(sync).toHaveBeenCalledTimes(1);

		// The tracker re-arms after a flush: the next burst gets its own frame.
		tracker.schedule();
		flushFrames();
		expect(sync).toHaveBeenCalledTimes(2);
	});

	it('cancel drops the pending frame and is a no-op when none is pending', () => {
		const sync = vi.fn();
		const tracker = createAnchorTracker(sync);

		tracker.cancel(); // nothing pending: no throw, no frame touched

		tracker.schedule();
		tracker.cancel();
		expect(pendingFrames.size).toBe(0);
		flushFrames();
		expect(sync).not.toHaveBeenCalled();

		// A cancelled tracker schedules again cleanly.
		tracker.schedule();
		flushFrames();
		expect(sync).toHaveBeenCalledTimes(1);
	});
});
