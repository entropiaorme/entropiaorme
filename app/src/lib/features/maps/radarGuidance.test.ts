import { describe, expect, it } from 'vitest';
import {
	DOT_RADIUS,
	DRAW_LIMIT,
	guidanceMarks,
	type LabelStrip,
	LINE_START,
	type Point,
	positionIsCurrent,
	RADAR_EDGE,
	STEERING_AGE_INTERVALS,
	stripBox,
} from './radarGuidance';

function close(actual: number, expected: number) {
	expect(actual).toBeCloseTo(expected, 9);
}

/** The tree's dot, which the case expects to be drawn. */
function drawn(dot: Point | null): Point {
	if (!dot) throw new Error('the tree is not drawn');
	return dot;
}

/** The strip as the reader of the bundled profile looks for it. */
const STRIP: LabelStrip = { from: 0.1, to: 1.0, halfHeight: 0.1 };

describe('the radar guidance marks', () => {
	it('places a tree within reach where it is on the radar, by the edge range read', () => {
		// Due north, half the edge range away: half way from centre to edge.
		const marks = guidanceMarks(0, 110, 220, STRIP);
		const dot = drawn(marks.dot);
		close(dot.x, RADAR_EDGE);
		close(dot.y, RADAR_EDGE - RADAR_EDGE / 2);
		// The line runs from clear of the player's dot to the tree.
		expect(marks.segments).toHaveLength(1);
		close(marks.segments[0].from.y, RADAR_EDGE - LINE_START);
		expect(marks.segments[0].to).toEqual(dot);
	});

	it('follows the scale the radar shows: the same tree sits further out on a closer zoom', () => {
		const wide = guidanceMarks(0, 100, 327, STRIP);
		const nearer = guidanceMarks(0, 100, 219, STRIP);
		expect(RADAR_EDGE - drawn(nearer.dot).y).toBeGreaterThan(RADAR_EDGE - drawn(wide.dot).y);
		close(RADAR_EDGE - drawn(nearer.dot).y, (100 / 219) * RADAR_EDGE);
	});

	it('shows a tree beyond the drawable reach by direction only', () => {
		const marks = guidanceMarks(180, 215, 219, STRIP);
		expect(marks.dot).toBeNull();
		const last = marks.segments[marks.segments.length - 1];
		close(last.to.x, RADAR_EDGE);
		close(last.to.y, RADAR_EDGE + DRAW_LIMIT);
	});

	it('without a scale shows direction only, however near the tree', () => {
		for (const distance of [0, 5, 80, 5000, null]) {
			const marks = guidanceMarks(225, distance, null, STRIP);
			expect(marks.dot).toBeNull();
			expect(marks.segments).toHaveLength(1);
			const { from, to } = marks.segments[0];
			close(Math.hypot(to.x - RADAR_EDGE, to.y - RADAR_EDGE), DRAW_LIMIT);
			close(Math.hypot(from.x - RADAR_EDGE, from.y - RADAR_EDGE), LINE_START);
		}
	});

	it('never draws over the player at the centre', () => {
		for (const bearing of [0, 90, 200, 359]) {
			for (const distance of [2, 20, 150]) {
				const marks = guidanceMarks(bearing, distance, 219, STRIP);
				const points = [
					...marks.segments.flatMap((segment) => [segment.from, segment.to]),
					...(marks.dot ? [marks.dot] : []),
				];
				for (const point of points) {
					expect(Math.hypot(point.x - RADAR_EDGE, point.y - RADAR_EDGE)).toBeGreaterThanOrEqual(
						LINE_START - 1e-9,
					);
				}
			}
		}
		// A tree right by the player shows its direction, not a dot on the player.
		expect(guidanceMarks(0, 2, 219, STRIP).dot).toBeNull();
	});

	it('never draws inside the strip the ring distances are read from', () => {
		const box = stripBox(STRIP);
		const inside = (p: Point) => p.x > box.x0 && p.x < box.x1 && p.y > box.y0 && p.y < box.y1;
		for (let bearing = 60; bearing <= 120; bearing += 2) {
			for (const [distance, edge] of [
				[50, 219],
				[150, 219],
				[400, 219],
				[150, null],
			] as const) {
				const marks = guidanceMarks(bearing, distance, edge, STRIP);
				for (const { from, to } of marks.segments) {
					// Every point along each piece, not just its ends.
					for (let t = 0; t <= 1; t += 0.05) {
						const point = { x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t };
						expect(inside(point), `bearing ${bearing}, ${distance} m`).toBe(false);
					}
				}
				if (marks.dot) {
					const nearX = Math.min(Math.max(marks.dot.x, box.x0), box.x1);
					const nearY = Math.min(Math.max(marks.dot.y, box.y0), box.y1);
					expect(Math.hypot(marks.dot.x - nearX, marks.dot.y - nearY)).toBeGreaterThanOrEqual(
						DOT_RADIUS,
					);
				}
			}
		}
		// Due east, everything the line would cross is the strip: nothing is drawn.
		expect(guidanceMarks(90, 150, 219, STRIP)).toEqual({ segments: [], dot: null });
		// Without a strip known, the line is drawn whole.
		expect(guidanceMarks(90, 150, null, null).segments).toHaveLength(1);
	});
});

describe('steering from the last position', () => {
	it('steers only from a position read within a few poll intervals', () => {
		const limit = STEERING_AGE_INTERVALS * 2;
		expect(positionIsCurrent(0, 1_000, 1_000, 2)).toBe(true);
		expect(positionIsCurrent(1, 1_000, 1_000 + (limit - 1) * 1000, 2)).toBe(true);
		expect(positionIsCurrent(1, 1_000, 1_000 + limit * 1000 + 1, 2)).toBe(false);
		// Old when it arrived.
		expect(positionIsCurrent(limit + 1, 1_000, 1_000, 2)).toBe(false);
		// Never read.
		expect(positionIsCurrent(null, 1_000, 1_000, 2)).toBe(false);
	});
});
