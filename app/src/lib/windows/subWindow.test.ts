import { describe, expect, it } from 'vitest';
import { placeSubWindow, SUB_WINDOW_MARGIN, subWindowChannel } from './subWindow';

const screen = { x: 0, y: 0, width: 1920, height: 1080 };
/** A control at x 400..520, on a readout whose surface spans y 300..420. */
const anchor = { left: 400, right: 520, below: 420, above: 300 };
const size = { width: 272, height: 200 };

describe('where a sub-window goes', () => {
	it("hangs below its control, its start edge on the control's", () => {
		expect(placeSubWindow(anchor, size, screen, 6, 'start')).toEqual({
			x: 400,
			y: 426,
			side: 'below',
			maxHeight: 1080 - SUB_WINDOW_MARGIN - 426,
		});
	});

	it('centres on its control when asked', () => {
		expect(placeSubWindow(anchor, size, screen, 6, 'centre').x).toBe(460 - 136);
	});

	it('sits above when it does not fit below and fits above', () => {
		const low = { ...anchor, below: 1000, above: 880 };
		const at = placeSubWindow(low, size, screen, 6, 'start');
		expect(at.side).toBe('above');
		// Its lower edge is the gap above the surface.
		expect(at.y + size.height).toBe(880 - 6);
	});

	it('takes the side with more room when it fits neither, and says how much there is', () => {
		const tall = { width: 272, height: 900 };
		// More room below.
		const upper = placeSubWindow({ ...anchor, below: 300, above: 200 }, tall, screen, 6, 'start');
		expect(upper.side).toBe('below');
		expect(upper.maxHeight).toBe(1080 - SUB_WINDOW_MARGIN - 306);
		// More room above: it starts at the top of the room and ends at the gap.
		const lower = placeSubWindow({ ...anchor, below: 900, above: 800 }, tall, screen, 6, 'start');
		expect(lower.side).toBe('above');
		expect(lower.maxHeight).toBe(800 - 6 - SUB_WINDOW_MARGIN);
		expect(lower.y).toBe(SUB_WINDOW_MARGIN);
	});

	it('stays on the screen sideways', () => {
		const right = placeSubWindow({ ...anchor, left: 1800, right: 1900 }, size, screen, 6, 'start');
		expect(right.x).toBe(1920 - SUB_WINDOW_MARGIN - size.width);
		const left = placeSubWindow({ ...anchor, left: 10, right: 60 }, size, screen, 6, 'centre');
		expect(left.x).toBe(SUB_WINDOW_MARGIN);
	});

	it('keeps to the screen it is on, wherever that screen is', () => {
		const second = { x: 1920, y: -200, width: 1280, height: 720 };
		const at = placeSubWindow(
			{ left: 3100, right: 3180, below: 480, above: 400 },
			size,
			second,
			6,
			'start',
		);
		expect(at.x).toBe(1920 + 1280 - SUB_WINDOW_MARGIN - size.width);
		expect(at.side).toBe('above');
	});

	it('hangs below, unclamped, when the screen is not known', () => {
		expect(placeSubWindow(anchor, size, null, 6, 'start')).toEqual({
			x: 400,
			y: 426,
			side: 'below',
			maxHeight: null,
		});
	});
});

describe("a sub-window's channel", () => {
	it('names every event after its window', () => {
		expect(subWindowChannel('overlay-menu', '/overlay-menu')).toEqual({
			label: 'overlay-menu',
			url: '/overlay-menu',
			ready: 'overlay-menu:ready',
			show: 'overlay-menu:show',
			update: 'overlay-menu:update',
			hide: 'overlay-menu:hide',
			report: 'overlay-menu:report',
		});
	});
});
