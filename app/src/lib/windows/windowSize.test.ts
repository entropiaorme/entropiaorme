// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the two seams the sync owns: the current window's setSize and the
// shell's hit-region command. DOM rects come from stubbed elements and
// animation frames are driven by hand, so the coalescing and dedupe logic
// runs deterministically.
const seams = vi.hoisted(() => ({
	setSize: vi.fn(async (_size: unknown) => {}),
	setWindowHitRegion: vi.fn(async (_shapes: unknown) => {}),
}));

vi.mock('@tauri-apps/api/window', () => ({
	getCurrentWindow: () => ({
		setSize: seams.setSize,
	}),
}));

vi.mock('@tauri-apps/api/dpi', () => ({
	LogicalSize: class {
		constructor(
			public width: number,
			public height: number,
		) {}
	},
}));

vi.mock('$lib/api/shell', () => ({
	setWindowHitRegion: seams.setWindowHitRegion,
}));

import {
	createWindowSizeSync,
	measureOverlaySurfaces,
	OVERLAY_SURFACE_ATTRIBUTE,
} from './windowSize';

interface Box {
	left: number;
	top: number;
	width: number;
	height: number;
}

function stubRect(el: HTMLElement, box: Box): void {
	el.getBoundingClientRect = () =>
		({
			...box,
			right: box.left + box.width,
			bottom: box.top + box.height,
			x: box.left,
			y: box.top,
			toJSON: () => ({}),
		}) as DOMRect;
}

/** A root (never itself a surface) holding one marked surface per box. */
function overlay(boxes: (Box & { radius?: string })[]): {
	root: HTMLElement;
	surfaces: HTMLElement[];
} {
	const root = document.createElement('div');
	const surfaces = boxes.map((box) => {
		const surface = document.createElement('div');
		surface.setAttribute(OVERLAY_SURFACE_ATTRIBUTE, '');
		if (box.radius) surface.style.borderRadius = box.radius;
		stubRect(surface, box);
		root.appendChild(surface);
		return surface;
	});
	const right = Math.max(0, ...boxes.map((box) => box.left + box.width));
	const bottom = Math.max(0, ...boxes.map((box) => box.top + box.height));
	stubRect(root, { left: 0, top: 0, width: right, height: bottom });
	document.body.appendChild(root);
	return { root, surfaces };
}

let pendingFrames: Map<number, FrameRequestCallback>;

async function flushFrames(): Promise<void> {
	const callbacks = [...pendingFrames.values()];
	pendingFrames.clear();
	for (const cb of callbacks) {
		cb(0);
	}
	// Let the async sync (and its afterSync continuation) settle.
	for (let i = 0; i < 6; i++) await Promise.resolve();
}

beforeEach(() => {
	seams.setSize.mockReset();
	seams.setSize.mockResolvedValue(undefined);
	seams.setWindowHitRegion.mockReset();
	seams.setWindowHitRegion.mockResolvedValue(undefined);
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

afterEach(() => {
	document.body.innerHTML = '';
	vi.restoreAllMocks();
});

describe('measureOverlaySurfaces', () => {
	it('sizes the window flush to the surfaces, ceiling fractional edges', () => {
		const { root } = overlay([
			{ left: 0, top: 0, width: 400.2, height: 52.4 },
			{ left: 0, top: 58.4, width: 400.2, height: 24.1 },
		]);
		const layout = measureOverlaySurfaces(root);
		expect(layout.width).toBe(401);
		expect(layout.height).toBe(83); // ceil(58.4 + 24.1)
	});

	it('reports each surface as a shape with its corner radius', () => {
		const { root } = overlay([
			{ left: 0, top: 0, width: 400, height: 52, radius: '12px' },
			{ left: 0, top: 58, width: 400, height: 24, radius: '8px' },
		]);
		expect(measureOverlaySurfaces(root).shapes).toEqual([
			{ x: 0, y: 0, width: 400, height: 52, radius: 12 },
			{ x: 0, y: 58, width: 400, height: 24, radius: 8 },
		]);
	});

	it('cuts to the smallest corner so no drawn pixel is lost', () => {
		const { root, surfaces } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const [surface] = surfaces;
		surface.style.borderTopLeftRadius = '12px';
		surface.style.borderTopRightRadius = '12px';
		surface.style.borderBottomRightRadius = '4px';
		surface.style.borderBottomLeftRadius = '12px';
		expect(measureOverlaySurfaces(root).shapes[0].radius).toBe(4);
	});

	it('resolves a percentage radius against the shorter side', () => {
		const { root } = overlay([{ left: 0, top: 0, width: 100, height: 40, radius: '50%' }]);
		expect(measureOverlaySurfaces(root).shapes[0].radius).toBe(20);
	});

	it('skips a collapsed surface (one fading out of layout)', () => {
		const { root } = overlay([
			{ left: 0, top: 0, width: 300, height: 40 },
			{ left: 0, top: 46, width: 0, height: 0 },
		]);
		const layout = measureOverlaySurfaces(root);
		expect(layout.shapes).toHaveLength(1);
		expect(layout.height).toBe(40);
	});

	it('counts the root itself when it is the surface', () => {
		const root = document.createElement('div');
		root.setAttribute(OVERLAY_SURFACE_ATTRIBUTE, '');
		stubRect(root, { left: 0, top: 0, width: 240, height: 96 });
		expect(measureOverlaySurfaces(root)).toEqual({
			width: 240,
			height: 96,
			shapes: [{ x: 0, y: 0, width: 240, height: 96, radius: 0 }],
		});
	});

	it('falls back to the root box with input everywhere when nothing is marked', () => {
		const root = document.createElement('div');
		stubRect(root, { left: 0, top: 0, width: 120.5, height: 30 });
		expect(measureOverlaySurfaces(root)).toEqual({ width: 121, height: 30, shapes: [] });
	});
});

describe('createWindowSizeSync', () => {
	it('coalesces a burst of schedules into one frame, then sizes and cuts the window', async () => {
		const { root } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const sizer = createWindowSizeSync(() => root);

		sizer.schedule();
		sizer.schedule();
		sizer.schedule();
		expect(pendingFrames.size).toBe(1);

		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);
		expect(seams.setSize).toHaveBeenCalledWith({ width: 100, height: 40 });
		expect(seams.setWindowHitRegion).toHaveBeenCalledWith([
			{ x: 0, y: 0, width: 100, height: 40, radius: 0 },
		]);
		// The size lands before the region that is cut out of it.
		expect(seams.setWindowHitRegion.mock.invocationCallOrder[0]).toBeGreaterThan(
			seams.setSize.mock.invocationCallOrder[0],
		);
	});

	it('dedupes an unchanged footprint and re-applies once the content changes', async () => {
		const { root, surfaces } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const sizer = createWindowSizeSync(() => root);

		sizer.schedule();
		await flushFrames();
		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);
		expect(seams.setWindowHitRegion).toHaveBeenCalledTimes(1);

		stubRect(surfaces[0], { left: 0, top: 0, width: 150, height: 40 });
		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenLastCalledWith({ width: 150, height: 40 });
		expect(seams.setWindowHitRegion).toHaveBeenCalledTimes(2);
	});

	it('re-cuts the region when a surface moves inside an unchanged footprint', async () => {
		const { root, surfaces } = overlay([
			{ left: 0, top: 0, width: 300, height: 40 },
			{ left: 0, top: 46, width: 300, height: 20 },
		]);
		const sizer = createWindowSizeSync(() => root);
		sizer.schedule();
		await flushFrames();

		// A narrower notice: same bounding box, a different cut.
		stubRect(surfaces[1], { left: 0, top: 46, width: 180, height: 20 });
		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);
		expect(seams.setWindowHitRegion).toHaveBeenCalledTimes(2);
	});

	it('forgets what failed to land, so the next request retries', async () => {
		const { root } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const sizer = createWindowSizeSync(() => root);
		seams.setSize.mockRejectedValueOnce(new Error('window mid-teardown'));
		seams.setWindowHitRegion.mockRejectedValueOnce(new Error('window mid-teardown'));

		sizer.schedule();
		await flushFrames();
		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(2);
		expect(seams.setWindowHitRegion).toHaveBeenCalledTimes(2);
	});

	it('only cuts the region when the host owns the window size', async () => {
		const { root } = overlay([{ left: 0, top: 0, width: 180, height: 120, radius: '8px' }]);
		const sizer = createWindowSizeSync(() => root, { resize: false });

		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).not.toHaveBeenCalled();
		expect(seams.setWindowHitRegion).toHaveBeenCalledWith([
			{ x: 0, y: 0, width: 180, height: 120, radius: 8 },
		]);
	});

	it('runs afterSync after every scheduled sync, applied or deduped', async () => {
		const afterSync = vi.fn();
		const { root } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const sizer = createWindowSizeSync(() => root, { afterSync });

		sizer.schedule();
		await flushFrames();
		expect(afterSync).toHaveBeenCalledTimes(1);
		expect(afterSync.mock.invocationCallOrder[0]).toBeGreaterThan(
			seams.setSize.mock.invocationCallOrder[0],
		);

		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);
		expect(afterSync).toHaveBeenCalledTimes(2);
	});

	it('no-ops without a root and cancel drops a pending sync', async () => {
		let root: HTMLElement | null = null;
		const sizer = createWindowSizeSync(() => root);

		sizer.schedule();
		expect(pendingFrames.size).toBe(0);
		expect(sizer.connect()).toBeTypeOf('function');
		expect(pendingFrames.size).toBe(0);

		root = overlay([{ left: 0, top: 0, width: 100, height: 40 }]).root;
		sizer.schedule();
		sizer.cancel();
		expect(pendingFrames.size).toBe(0);
		await flushFrames();
		expect(seams.setSize).not.toHaveBeenCalled();

		sizer.schedule();
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);
	});
});

describe('createWindowSizeSync connect', () => {
	let observed: Set<Element>;
	let resizeCallbacks: (() => void)[];

	beforeEach(() => {
		observed = new Set();
		resizeCallbacks = [];
		vi.stubGlobal(
			'ResizeObserver',
			class {
				constructor(callback: () => void) {
					resizeCallbacks.push(callback);
				}
				observe(el: Element) {
					observed.add(el);
				}
				disconnect() {
					observed.clear();
				}
			},
		);
	});

	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it('watches the root and every surface, and syncs once on connect', async () => {
		const { root, surfaces } = overlay([
			{ left: 0, top: 0, width: 100, height: 40 },
			{ left: 0, top: 46, width: 100, height: 20 },
		]);
		const sizer = createWindowSizeSync(() => root);
		const disconnect = sizer.connect();

		expect(observed).toEqual(new Set([root, ...surfaces]));
		await flushFrames();
		expect(seams.setSize).toHaveBeenCalledTimes(1);

		// A surface resizing schedules a sync.
		resizeCallbacks[0]();
		expect(pendingFrames.size).toBe(1);

		disconnect();
		expect(observed.size).toBe(0);
		expect(pendingFrames.size).toBe(0);
	});

	it('picks up a surface mounted after connecting', async () => {
		const { root } = overlay([{ left: 0, top: 0, width: 100, height: 40 }]);
		const sizer = createWindowSizeSync(() => root);
		const disconnect = sizer.connect();
		await flushFrames();

		const notice = document.createElement('div');
		notice.setAttribute(OVERLAY_SURFACE_ATTRIBUTE, '');
		stubRect(notice, { left: 0, top: 46, width: 100, height: 20 });
		root.appendChild(notice);
		// MutationObserver delivers on a microtask.
		await Promise.resolve();
		expect(observed.has(notice)).toBe(true);

		await flushFrames();
		expect(seams.setWindowHitRegion).toHaveBeenLastCalledWith([
			{ x: 0, y: 0, width: 100, height: 40, radius: 0 },
			{ x: 0, y: 46, width: 100, height: 20, radius: 0 },
		]);
		disconnect();
	});
});
